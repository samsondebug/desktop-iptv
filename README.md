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

### libmpv

The engine loads libmpv **at runtime** (no import library needed), searching
`DESKTOP_IPTV_LIBMPV`, the exe directory, `lib/`, `resources/lib/`, then the system path.
Without it the app starts with a stub engine (no video) so the catalog UI keeps working.

| OS      | Get it                                                                                   |
|---------|------------------------------------------------------------------------------------------|
| Windows | `libmpv-2.dll` from a static mpv-dev build (e.g. zhongfly/mpv-winbuild `mpv-dev-lgpl-x86_64-*.7z`) → `apps/desktop/src-tauri/lib/` |
| macOS   | `brew install mpv` (found in `/opt/homebrew/lib`)                                        |
| Linux   | `apt install libmpv2` (Debian/Ubuntu) or your distro's libmpv                            |

See [`docs/ENGINE.md`](./docs/ENGINE.md) for how the native surface embed works per OS.

## Keyboard

`/` search · `j`/`k` or `↑`/`↓` move · `Enter` play · `f` fullscreen (also double-click) · `m` mute ·
`space` pause · `p` toggle Low latency / Stable · `Shift+S` settings · `Shift+D` diagnostics · `Esc` back.

## License

MIT for the code. mpv/libmpv is LGPL-2.1+ (use an LGPL build, dynamically loaded — as this app does).
