# Status — full 90-day plan implemented (v0.1.0 candidate)

Last updated: 2026-09-27. Six commits, ~15.6k lines of Rust across 5 crates (90 tests, clippy
`-D warnings` clean, `cargo fmt` clean), ~5.4k lines of TypeScript/React (tsc + vite green),
105 Tauri commands.

Everything below was verified on Linux (Xvfb, real libmpv 0.37 for playback / headless tests,
stub engine for UI screenshots) against the mock Xtream + Stalker panel in `fixtures/`. The
Windows and macOS builds have **not** been run on real hardware yet — that is the first thing to do
on Dave's PC (see "Next on a real machine").

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

## Next on a real machine (Windows first)

1. `scripts/fetch-libmpv.ps1`, then `npm run tauri dev` in `apps/desktop`. Confirm the transparent
   WebView2 composites over the mpv child window (`win_zorder` pushes it to `HWND_BOTTOM`).
2. Real provider: zap time, profile switch, HUD numbers, EPG offset, record + zap continuation,
   download resume after a network drop, mini mode geometry after restart, two panes.
3. Diagnostics on a bad URL: HTTP trace shows the redirect/HTML; probe shows the error; report has
   no secrets (search it for the password before sharing).
4. Tag `v0.1.0` and let `release.yml` produce the installers.

## Known gaps

* Linux/X11: the mpv child window occludes the GTK webview, so overlay chrome is not visible over
  video (video plays). Needs the render-API path (GtkGLArea). Windows/macOS are not affected by
  this specific issue.
* macOS: passes the `NSView*` as `wid`; §6.1 recommends the render API on an OpenGL/Metal view.
  Untested.
* Stalker: no VOD, no portal EPG (add an XMLTV source), no catch-up; recordings only while the
  channel is playing (links expire).
* Adaptive cache (§6.3) intentionally not shipped (flagged for later, opt-in).
* Sports Hub not built (per CLAUDE.md: only once everything else is boringly stable).
* Updater not wired (Tauri updater plugin is a small follow-up once a release URL exists).
