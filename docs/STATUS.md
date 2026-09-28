# Status — full 90-day plan implemented (v0.1.0 candidate)

Last updated: 2026-09-27. Six commits, ~15.6k lines of Rust across 5 crates (90 tests, clippy
`-D warnings` clean, `cargo fmt` clean), ~5.4k lines of TypeScript/React (tsc + vite green),
105 Tauri commands.

Verified on Linux (Xvfb, real libmpv 0.37, mock Xtream + Stalker panel in `fixtures/`) **and on
Windows 11 with a real Xtream provider** (2026-09-28, RTX 4070 SUPER, 2560×1440): the NSIS installer
built from Linux with `cargo-xwin` installed cleanly, libmpv 0.41 loaded from `lib/`, the transparent
WebView2 chrome composites over the native mpv child window, sign-in → 4,951 channels + 25,777
programmes + 10,578 VOD titles in ~8 s, zap 650–1,250 ms on the Stable profile, fullscreen, mini
player, a second pane playing a different channel, and a tap recording that kept growing on its own
connection after zapping away. macOS is still untested.

## Exit tests per phase (CLAUDE.md §12)

| Phase | Exit test | Status | Evidence |
|---|---|---|---|
| 1–14 | 20k-channel M3U in SQLite | ✅ | `bench_import 20000`: import 456 ms, refresh 84 ms |
| 1–14 | 60 fps scroll | 🟡 | Virtualized (TanStack) + 200-row SQL pages; fps meter in the HUD; number to be recorded on Windows |
| 1–14 | zap < 800 ms | ✅ (local) | 140 ms over HTTP MPEG-TS under Xvfb; measured click → `PLAYBACK_RESTART` |
| 15–30 | 7-day EPG beside live video; refresh does not drop frames | ✅ | Mock panel: 1000 ch + 14,402 programmes + 264 VOD in ~0.5 s; XMLTV streamed (gzip) into 5k-row chunks; grid with now-line / 8 h window / hover / right-click record |
| 31–50 | resume position correct; PIN hides filtered groups | ✅ | progress saved every 5 s + on stop; resume prompt; auto-next; parental keywords as a SQL clause on every catalog query |
| 51–70 | scheduled recording while 2-pane playback on one connection tap | ✅ | mpv `stream-record` tap on the playing channel (verified >10 KB in headless test); on zap the job continues on its own raw connection appending to the same file; pane windows each run their own engine with copy-hwdec and an audio lock |
| 71–90 | diagnostics isolate a bad URL and export a fully redacted log | ✅ | HTTP trace ring buffer (redirect chain, first 200 bytes), headless probe (container/codec/TTFF/error), source checks ("got HTML not M3U", Xtream account), one-click report — all through `redact()` with tests asserting no password/token/MAC leaks |

## What is built, by area

**Catalog (`app-db`, `app-net`)** — WAL/FTS5 schema (migrations 0001 + 0002), PRAGMA set on every
connection, chunked change-detecting upserts, stale-row removal by `sync_gen`, streaming M3U parser
with 512-byte preflight, XMLTV streaming importer (gzip, encodings), Xtream Codes adapter
(live/VOD/series/series_info/xmltv, account info without the password), **Stalker/MAC adapter**
(handshake → bearer token, genres, all channels, `create_link` at play time; live only; behind
Settings → Playlists → Experimental).

**Engine (`app-engine`)** — libmpv loaded at runtime, one long-lived instance, `loadfile replace`,
§6.2 profile matrix, property-observation telemetry, zap timing keyed by playlist entry id,
`stream-record` tap, `start` for VOD resume, seek, video margins from the pane rect, stub fallback,
**headless probe** (`vo=null`/`ao=null`) for diagnostics.

**Money features** — recordings (tap → raw continuation, HLS via headless mpv, scheduler every 10 s,
10-min overrun, interrupted jobs marked failed at startup), VOD downloads (Range resume, `.part`,
sidecar `.srt`, free-space warning), mini/PiP window with persisted geometry per mode, multiscreen
panes (`index.html?pane=<label>`, own engine, copy hwdec, audio lock, connection-budget warning
from Xtream `max_connections`), HMAC machine-bound license with 72 h trial and feature gates.

**Support product** — Diagnostics workbench (Report / HTTP trace / Stream probe / Sources),
encrypted backup/restore (`DIPTVBK1` container: Argon2id → AES-256-GCM; playlists with logins,
favorites + EPG overrides re-applied by `source_id` after the restored playlist syncs, settings,
theme tokens, parental keywords; PIN + license deliberately excluded), release workflow
(`.github/workflows/release.yml`: NSIS/MSI, DMG ×2, deb/AppImage; libmpv fetched by
`scripts/fetch-libmpv.ps1` / `scripts/fetch-libmpv-macos.sh`).

**UI** — split canvas per §8, EPG grid, poster grids with continue-watching, series modal, record
dialog + Library modal, Settings (Playback / Interface / Playlists / Guide / Parental / License /
About & backup), Diagnostics, keyboard map, themes via CSS tokens.

## Deviations from CLAUDE.md (deliberate)

1. FTS triggers key on a derived rowid (channel `id*2`, vod `id*2+1`) — the handoff's
   `WHERE item_id = old.id` on an UNINDEXED column is O(n) per row (55 s → 84 ms on a 20k refresh).
2. libmpv is loaded via `libloading` instead of link-time `libmpv-sys` — no import library, stub
   fallback when the DLL is absent, one binary for machines with and without mpv.
3. Record tee: the §6.4 "one HTTP GET → broadcast → file + mpv" design is realised with mpv's
   own `stream-record` (mpv already owns the single connection; the file gets the raw demuxed
   stream). When the user zaps away, the job re-opens its own connection and appends. Scheduled
   jobs for a channel that is not playing open their own connection from the start.
4. Passwords live in the SQLite `pass` column (file in the per-user app data dir) rather than the
   OS keychain — keychain integration is a follow-up; they never leave Rust unredacted.

## Found on Windows and fixed the same day

* The app root painted its own background over the player pane: on Windows/macOS the webview is
  *above* mpv (on Linux/X11 it is below), so the root is now transparent and every chrome region is
  opaque on its own.
* Embedded EIA-608 captions rendered as garbage over live channels → `sid=no` by default.
* A live stream ending (provider node rotation, connection cap) left the player idle → ffmpeg-level
  `reconnect_at_eof` / `reconnect_on_http_error=4xx,5xx` plus an app-level reconnect with backoff
  (1/2/4/8/15 s, 10 attempts) and a notice in the UI; a tap recording continues on its own
  connection across the reload.
* No log file for support → `<app data>/sktv.log`, truncated per run, tail included in the
  diagnostics report.
* Cross-building from Linux: `cargo install cargo-xwin`, `apt install nsis`, then
  `npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis` produces the
  same installer the release workflow does (7 min on a laptop-class box).

## Packaging (how this ships, compared with the established players)

The installer is NSIS (`installMode: both` — per-user without UAC, or per-machine into
`C:\Program Files`), bundling `lib/libmpv-2.dll` as a resource. The commercial reference app ships
a full `mpv.exe` (117 MB) plus `ffmpeg.exe`/`ffprobe.exe` (200 MB) and drives mpv as a child
process over IPC; this app links libmpv in-process (120 MB DLL, no IPC, zap in one call). What is
still missing to match a store-quality release: code signing (Azure Trusted Signing or an OV cert —
unsigned builds trip SmartScreen on download), the updater plugin + `latest.json`, a real icon and
product name, and optionally an MSIX wrap for the Microsoft Store.

## Distribution (2026-09-28)

* Self-updater: `tauri-plugin-updater` polls the signed `latest.json` on GitHub Releases 12 s after
  start and every 6 h; "Install & restart" toast + manual check in Settings → About. Minisign
  public key in `tauri.conf.json`; private key is the `TAURI_SIGNING_PRIVATE_KEY` repo secret.
* Windows exe imports only system DLLs + UCRT (tauri-build links vcruntime statically; checked
  with `pefile`) — no VC++ redistributable needed; NSIS fetches WebView2 when missing; per-user
  or per-machine install.
* Download page `site/index.html` → GitHub Pages (`pages.yml`), reads the latest release from the
  API, OS-aware button, legal block, SmartScreen/Gatekeeper notes.
* `release.yml` publishes (not drafts) on `v*` tags; Intel macOS moved to `macos-15-intel`.

* **v0.1.0 released 2026-09-28** from `release.yml`: signed Windows exe/msi (Azure Artifact
  Signing, publisher David Krouskoff), macOS DMGs (unsigned), Linux deb/AppImage/rpm, `latest.json`.
  Download page: https://samsondebug.github.io/desktop-iptv/
* **v0.2.0 — renamed to SKTV.** `productName`, window titles, About/toasts, User-Agent
  (`SKTV/<version> (libmpv)`), backup wording, log file (`sktv.log`), recordings folder
  (`<Videos>/SKTV`), release names, download page. Unchanged on purpose: bundle identifier
  `dev.desktopiptv.app` (app-data folder stays, so playlists/settings/licence carry over), crate
  and repo names, `MACHINE_ID_SALT` (machine GUID → trial/licence state unchanged), WiX
  `upgradeCode` pinned to the desktop-iptv value. `nsis/hooks.nsh` uninstalls a leftover
  desktop-iptv (per-machine or per-user) before SKTV installs, so the 0.1.0 → 0.2.0 self-update
  ends with one copy. `bundle.publisher` is now "David Krouskoff" (Add/Remove showed
  "desktopiptv").

## Next

1. Notarize macOS (Apple Developer account) — until then Mac users use *Open Anyway* once.
2. macOS: run the DMG once on real hardware (render API path may be needed).

## Known gaps

* Linux/X11: the mpv child window occludes the GTK webview, so overlay chrome is not visible over
  video (video plays). Needs the render-API path (GtkGLArea). Windows is verified unaffected.
* macOS: passes the `NSView*` as `wid`; §6.1 recommends the render API on an OpenGL/Metal view.
  Untested.
* Stalker: no VOD, no portal EPG (add an XMLTV source), no catch-up; recordings only while the
  channel is playing (links expire).
* Adaptive cache (§6.3) intentionally not shipped (flagged for later, opt-in).
* Sports Hub not built (per CLAUDE.md: only once everything else is boringly stable).
