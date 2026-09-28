# desktop-iptv

Player-only desktop IPTV client. **libmpv for truth, SQLite for speed, EPG + player on one canvas,
diagnostics that prove the provider is at fault, cheap lifetime unlock.**

> This app does not provide channels, playlists, or stream URLs. You bring your own source.
> We do not support illegal services. We support the player, not the reseller.

Architecture and rules live in [`CLAUDE.md`](./CLAUDE.md) (the build handoff). Status and exit
tests are tracked in [`docs/STATUS.md`](./docs/STATUS.md).

## Stack

Tauri v2 · Rust workspace · libmpv (runtime-loaded, native surface via `wid`) · SQLite WAL + FTS5 ·
React 19 + TypeScript + Tailwind 4 + TanStack Virtual.

```
crates/app-core     config, license (HMAC machine-bound), typed IPC contracts, secret redaction
crates/app-db       SQLite pool, migrations, FTS5, paged queries
crates/app-net      reqwest client, streaming M3U parser + preflight, importer, adapters
crates/app-engine   libmpv controller (dynamic FFI), profiles, telemetry, stub fallback
apps/desktop        Tauri app (src-tauri = thin command layer) + React frontend
```

## Features

Live TV + 8-hour EPG grid on one canvas · Xtream Codes, M3U/M3U8 (URL or file) and experimental
Stalker/MAC portals · Movies & Series with resume, continue-watching and auto-next · favorites,
recents, groups, instant FTS search across 40k+ rows · Low-latency / Stable profiles, hwdec
`auto-safe`, HUD with codec/bitrate/fps/cache/zap · recording (single-connection tap, scheduled from
the guide, 10-min overrun) · VOD downloads with resume · mini/PiP window · multiscreen panes with an
audio lock · parental PIN + keyword filter · themes as CSS tokens · encrypted backup/restore ·
Diagnostics: HTTP trace, headless stream probe, source checks, one-click redacted report · 72-hour
trial, then a one-time PRO unlock.

## Build

Prerequisites: Rust ≥ 1.80, Node ≥ 20, and the Tauri v2 platform deps
(<https://tauri.app/start/prerequisites/>).

```sh
cd apps/desktop
npm install
npm run tauri dev        # dev server + app
npm run tauri build      # installers in apps/desktop/src-tauri/target/release/bundle
```

Rust-only checks (no GUI toolchain needed on Linux CI):

```sh
cargo test -p app-core -p app-db -p app-net -p app-engine
cargo run --release -p app-net --example bench_import -- 20000   # import + search benchmark
```

Local end-to-end without a provider: `python3 fixtures/mock_xtream.py 8090 /path/to/any.ts` serves a
synthetic Xtream panel (user/pass, 1000 channels, EPG, VOD) and a Stalker portal
(MAC `00:1A:79:12:34:56`) that both play the given MPEG-TS file as a paced live stream.

Releases: push a `v*` tag — `.github/workflows/release.yml` builds Windows (NSIS/MSI, bundles
libmpv), macOS (DMG, bundles libmpv from Homebrew) and Linux (deb/AppImage) installers. A Windows
installer can also be cross-built from Linux: `cargo install cargo-xwin && apt install nsis`, drop
`libmpv-2.dll` into `apps/desktop/src-tauri/lib/`, then
`npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis`.

### libmpv

The engine loads libmpv **at runtime** (no import library needed), searching
`DESKTOP_IPTV_LIBMPV`, the exe directory, `lib/`, `resources/lib/`, then the system path.
Without it the app starts with a stub engine (no video) so the catalog UI keeps working.

| OS      | Get it                                                                                   |
|---------|------------------------------------------------------------------------------------------|
| Windows | `scripts/fetch-libmpv.ps1` (downloads `libmpv-2.dll` from the shinchiro mpv-winbuild releases into `apps/desktop/src-tauri/lib/`, bundled as a resource) |
| macOS   | `brew install mpv` (found in `/opt/homebrew/lib`)                                        |
| Linux   | `apt install libmpv2` (Debian/Ubuntu) or your distro's libmpv                            |

See [`docs/ENGINE.md`](./docs/ENGINE.md) for how the native surface embed works per OS.

## Keyboard

`/` search · `j`/`k` or `↑`/`↓` move · `Enter` play · `←`/`→` guide window (live) or seek ±10 s (VOD) ·
`f` fullscreen (also double-click) · `m` mute · `space` pause · `p` toggle Low latency / Stable ·
`r` record the current channel · `i` mini player · `1`/`2`/`3` tabs · `Shift+S` settings ·
`Shift+D` diagnostics · `Esc` back.

## License

MIT for the code. mpv/libmpv is LGPL-2.1+ (use an LGPL build, dynamically loaded — as this app does).
