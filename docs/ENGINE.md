# Engine notes — libmpv embed, profiles, telemetry

## Why dynamic loading

`app-engine/src/ffi.rs` declares the ~16 libmpv client-API entry points we use and loads them
with `libloading` at runtime. Consequences:

* The workspace builds on any machine — no `libmpv.dll.a` / `mpv.lib` at link time, no bindgen.
* A missing `libmpv-2.dll` degrades to the stub engine instead of a crash at startup.
* mpv (LGPL) stays a separately-loaded shared library.

Struct layouts (`mpv_event`, `mpv_event_property`, `mpv_event_end_file`, `mpv_event_log_message`,
`mpv_event_start_file`) and enum values follow `mpv/client.h`, client API ≥ 2.0 (mpv ≥ 0.35).
The engine refuses older API versions.

## One instance, `loadfile … replace`

`MpvEngine::new` runs once at app start (`setup` in `src-tauri/src/lib.rs`). Zapping is
`loadfile <url> replace`; the pipeline is never torn down. `EngineEvent::PlaybackStarted.zap_ms`
is measured from `load()` to `MPV_EVENT_PLAYBACK_RESTART` (first frame shown).

In-flight loads are tracked by playlist entry id (`MPV_EVENT_START_FILE`), so the `END_FILE`
that mpv emits for the *replaced* stream never clears the new stream's zap timer or reaches the
UI as a "stream ended" error.

## Native surface per OS

| OS      | `wid` value                                   | Notes |
|---------|-----------------------------------------------|-------|
| Windows | `HWND` of the Tauri main window                | mpv creates a child window inside it. The Tauri window is `transparent: true` and `html, body { background: transparent }`; the WebView2 layer composites over the mpv child. Same technique as `tauri-plugin-libmpv` ("fully tested" on Windows). Default hwdec `auto-safe` → `d3d11va`. |
| Linux   | X11 window id (`GDK_BACKEND=x11` is forced)    | Wayland has no `wid` embedding; XWayland works. |
| macOS   | `NSView*` of the main window (day-1 path)      | Works for a first cut but CLAUDE.md §6.1 wants `mpv_render_context_create` on an `NSOpenGLView`/CAOpenGLLayer for a tear-free Cocoa hierarchy. Tracked as a follow-up in `docs/STATUS.md`. |

Video placement inside the player pane uses `video-margin-ratio-{left,right,top,bottom}`:
the React `PlayerPane` reports its rect (`set_video_rect`) on mount/resize/fullscreen and the
engine converts it to margins. All non-player UI is opaque; only the pane is transparent.

`force-window=yes` + `background-color=#000` make mpv paint black immediately so a transparent
window never shows the desktop before the first stream.

## Profiles (`app-engine/src/profiles.rs`)

Exactly two, injected as mpv options from Rust (no user-visible mpv.conf):

| key                     | low_latency | stable          |
|-------------------------|-------------|-----------------|
| cache-secs              | 3           | 20 (user 20–60) |
| demuxer-readahead-secs  | 3           | = cache-secs    |
| demuxer-max-bytes       | 32MiB       | 400MiB          |
| demuxer-max-back-bytes  | 8MiB        | 50MiB           |
| video-latency-hacks     | yes         | no              |
| network-timeout         | 8           | 15              |
| reconnect_delay_max     | 5           | 10              |

Common: `cache=yes` (never `no`), `hwdec=auto-safe`, `vo=gpu-next`, `deband=no`, `interpolation=no`,
`scale=bilinear`, `volume-max=130`, `reconnect=1`, `reconnect_streamed=1`.

Switching profiles sets the live-switchable keys immediately and the Tauri layer reloads the
current stream so startup-only keys apply (`set_profile` command).

## Telemetry

Observed properties → `EngineTelemetryEvent` (≤ 2 Hz while playing):
`video-params/w|h`, `video-codec`, `estimated-vf-fps`, `video-bitrate`, `demuxer-cache-duration`,
`paused-for-cache` (→ `Buffering` event), `frame-drop-count`, `core-idle`.

mpv log lines at warn/error are forwarded as `EngineEvent::Log` after `redact()`, except the
hwdec probe noise (`AVHWDeviceContext … Cannot load libcuda`), which is expected with `auto-safe`.

## Headless test

`crates/app-engine/tests/headless_mpv.rs` generates a 12 s MPEG-TS/H.264 file with ffmpeg,
plays it with `vo=null`/`ao=null`, and asserts: `PlaybackStarted` with `zap_ms < 5 s`, 320×240
telemetry with an H.264 codec name, a profile switch plus a second `loadfile replace`, and a
volume round-trip. It passes on Linux with `libmpv2` installed (measured zap ≈ 15–20 ms on a
local file) and skips cleanly where libmpv or ffmpeg is absent.

## Record tap, probe and panes (days 51–90)

* **Record tap** — `PlayerEngine::set_record(Some(path))` sets mpv's `stream-record`, which
  writes the raw demuxed stream (MPEG-TS stays MPEG-TS) from the *existing* connection. No second
  HTTP GET. Verified in `tests/headless_mpv.rs` (file grows >10 KB while playing). On zap the Tauri
  layer (`dvr::on_stream_change`) clears the tap and continues the job on its own raw GET
  (`app_net::recorder`) appending to the same file — the stream is the same bytes, so the file
  stays a valid TS. HLS (`.m3u8`) jobs use a second, headless engine instead of a raw GET.
* **Headless probe** — `app_engine::probe::probe_stream(opts, url, timeout)` creates a throw-away
  engine with `wid = None` (→ `vo=null`, `ao=null`, `force-window=no`), loads the URL with the
  same profile/UA/hwdec the player would use, waits for `PLAYBACK_RESTART` or `END_FILE`, then
  reads `file-format`, `video-format`, `audio-codec-name`, `video-params/w|h`, `container-fps`,
  `hwdec-current`, `demuxer-cache-duration`. Errors and log lines are redacted. One probe at a
  time (Tauri command guards with an atomic).
* **Panes** — every extra window (`index.html?pane=<label>`) gets its own `MpvEngine` bound to
  that window's native handle with `hw_decoding = auto-copy-safe` (copy-back survives iGPUs and
  avoids zero-copy contention, CLAUDE.md §6.5). Only one engine is unmuted at a time
  (`pane_audio`). Closing the window shuts the engine down.
