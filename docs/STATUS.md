# Status — days 1–14 ("ugly but plays")

Last updated: 2026-09-27 (scaffold session).

## Exit tests (CLAUDE.md §12, days 1–14)

| Exit test                                   | Status | Evidence |
|---------------------------------------------|--------|----------|
| 20k-channel M3U in SQLite                   | ✅ | `bench_import 20000`: import **456 ms**, refresh **84 ms**, db 8.3 MB (Linux, release) |
| 60 fps scroll on 20k rows                   | 🟡 | TanStack virtual list + 200-row SQL pages; on-screen fps meter while scrolling. Needs a run on the Windows box to record the number. |
| zap < 800 ms on a healthy stream            | 🟡 | Engine measures `zap_ms` (click → `PLAYBACK_RESTART`) and shows it in the HUD + Now panel. Headless local-file zap = 15–20 ms. Needs a real provider stream on Windows. |
| Native surface, no `<video>`                | ✅ (code) / 🟡 (Windows visual) | libmpv via `wid` = HWND, transparent WebView2 overlay. Verified on Linux/X11 under Xvfb that video renders natively in the pane rect with correct margins (screenshot in session). Windows compositing check pending on Dave's PC. |
| Two profiles                                | ✅ | `low_latency` / `stable` with the §6.2 matrix; toggle reloads the stream |
| Redacted HTML-as-M3U error                  | ✅ | Preflight rejects HTML/XMLTV/JSON with a redacted 200-byte preview; toast + import_done event |
| Player-only legal block                     | ✅ | First-run gate + Settings footer |

## What is built

**Rust (31 tests green, clippy clean)**

* `app-core` — IPC contracts (§4 + paging/import/group/playlist shapes), config with clamping,
  HMAC machine-bound license + 72 h trial clock + feature gates, secret redaction (query keys,
  Xtream path creds, basic-auth URLs, JSON keys, Authorization headers).
* `app-db` — WAL/FTS5 catalog, exact §5 DDL (+ `recents`), PRAGMA set on every connection,
  migrations via `user_version`, chunked upsert (≤5k rows/tx, no-op when unchanged), paged
  listing, groups, favorites, recents, settings helpers. FTS triggers use derived rowids so
  update/delete are O(log n) — the handoff's `WHERE item_id = old.id` on an UNINDEXED column was
  O(n) per row (55 s → 84 ms for a 20k refresh).
* `app-net` — reqwest/rustls(ring) client (UA, timeouts, redirect cap), 512-byte preflight,
  push-based streaming M3U parser (BOM, CRLF, `#EXTINF` attrs incl. `catchup`/`timeshift`/`tvg-rec`,
  `#EXTGRP`, `#EXTVLCOPT`/`#KODIPROP`, headerless lists, hostile-line cap), importer with
  progress events, `CatalogAdapter` trait (M3U implemented, Xtream stubbed).
* `app-engine` — runtime-loaded libmpv FFI, single instance, `loadfile replace`, profile matrix,
  observed-property telemetry, zap timing, in-flight load tracking by playlist entry id, log
  redaction, stub engine fallback. Headless test plays real MPEG-TS through real libmpv.
* `apps/desktop/src-tauri` — 33 commands, 3 event streams, engine bound to the main window
  handle, DB in the app data dir, trial starts on first successful import, free tier = 1 playlist.

**Frontend (tsc + vite build green)**

Split canvas: top bar (clock · tier badge · Live/Movies/Series · playlist picker · import
progress) · left rail (Favorites, Recents, All, group filter, virtualized groups) · transparent
player pane with overlay chrome (LIVE badge, HUD, transport, profile toggle, favorite, fullscreen,
auto-hide) · Now panel (channel, engine stats, zap) · virtualized channel list with FTS search ·
Settings (profiles, cache, hwdec, boost, delay, theme, playlists, license) · Diagnostics
(redacted engine log, raw-URL probe, copy sanitized report) · first-run legal gate · toasts ·
keyboard map.

## Not yet (by design — later phases)

Xtream adapter (15–30) · XMLTV/EPG grid + Now/Next (15–30) · VOD (31–50) · record tee,
downloads, PiP, multiscreen (51–70) · HTTP tracing, Stalker, backup, packaging polish (71–90).

## Verified end-to-end in this session (Linux, Xvfb, real libmpv 0.37)

* App boot → catalog created → engine bound to X11 window id → auto-play of the last channel
  over HTTP (MPEG-TS from a local server) → `PLAYBACK_RESTART` with **zap 140 ms** → video drawn
  natively inside the reported player rect.
* UI (stub engine for screenshots): 20k-row virtualized list, group rail, FTS search
  (`channel 1234` → 11 hits, diacritics-insensitive), keyboard play, Settings, Diagnostics,
  fullscreen with HUD, Add-playlist dialog.
* Import via UI over HTTP: HTML URL rejected with first-bytes preview; 20k M3U imported in 0.8 s
  with the EPG hint redacted (`password=***`); trial badge flips to `TRIAL · 72h`; free tier
  refuses a second playlist.

## Follow-ups discovered while building

0. **Linux overlay** — with `wid` embedding on X11 the mpv child window sits *above* the GTK
   webview (X child windows always occlude the parent's own drawing), so the HTML chrome is not
   visible over video on Linux. Windows (WebView2 composition) and macOS behave differently;
   Linux needs the render-API path (GtkGLArea / shared texture). Video itself plays fine.
1. **macOS embed** — day-1 path passes the `NSView*` as `wid`; §6.1 wants the render API on an
   OpenGL view. Do this before any Mac testing.
2. **Windows visual check** — confirm the transparent WebView2 composites over the mpv child
   window on Dave's PC (it does for `tauri-plugin-libmpv`; if not, `SetWindowPos(HWND_BOTTOM)`
   on the mpv child is the fallback).
3. **Stale channel removal on refresh** — upsert keeps rows that disappeared from the source.
   Add a `sync_gen` column in migration 0002 and delete rows not touched by the latest sync.
4. **Keyset paging** — `LIMIT/OFFSET` at offset 20k measured 0.4 ms; fine for now.
5. **hwdec fallback chain at runtime** — `profiles::hwdec_fallback` exists; wire it to
   `END_FILE(error)` + decoder errors once we see real failures.
6. **Playlist passwords** — Xtream credentials should go to the OS keychain (§2) when that
   adapter lands; M3U URLs with embedded creds currently live in `playlists.base_url`
   (redacted everywhere they are displayed).
