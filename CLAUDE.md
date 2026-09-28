# CLAUDE.md — FINAL BUILD HANDOFF

**Status:** FINAL. This file absorbs all research passes, the module-level technical spec, the Zero-Class architecture deck, product verification notes, and the implementation playbook.

**Repo state:** all 90 days implemented (v0.1.0 candidate) — see `docs/STATUS.md` (exit tests per phase, deviations, what to verify on real hardware), `docs/ENGINE.md` (embed/profiles/telemetry) and `docs/TEARDOWN-NOTES.md`. Deliberate deviations from this file are recorded in STATUS.md: FTS triggers key on a derived rowid (the `WHERE item_id = old.id` form is O(n) per row); libmpv is loaded at runtime via `libloading` instead of link-time `libmpv-sys`; the record tee uses mpv's `stream-record` on the playing connection with a raw-GET continuation on zap; playlist passwords live in the SQLite `pass` column (not the keychain yet).

**How to use:** Put this file at the repo root as `CLAUDE.md`. Read it once before scaffolding. Do not ask for architecture decisions that this file already made. If two sections conflict, win order is: legal block → never put frames through JS → SQLite-not-JSON → this file’s 90-day order.

You are building a **player-only** desktop IPTV client inspired by IPTV Player Zero (Built By Board Ltd, iptvplayerzero.com). You are **not** cloning their brand, assets, or name. You are implementing the architecture, UX shape, and engineering rules in this file.

Target: Windows + macOS first, Linux same codebase (Tauri). Architecture name internally: **Zero-Class Native Embedded**.

---

## 0A. Zero-Class engineering philosophy (from architecture deck)

Four pillars. Every feature must serve at least one. If it serves none, do not build it yet.

1. **Native libmpv for truth.** No HTML5 `<video>`. No web-wrapper frame copying. Video bytes are decoded and rendered by the OS GPU. Pipeline is Demuxer (HLS/MPEG-TS) → Decoder (`d3d11va` / VideoToolbox) → Renderer (`vo=gpu-next`) on a native surface. JS is overlay only.
2. **SQLite WAL for speed.** Playlists are never loaded into RAM as JSON arrays. Stream, index, query. Instant search across 40k+ channels. Virtualize every list. 20k rows at 60 fps is an exit test, not a stretch goal.
3. **The split canvas.** Desktop-native UI. Watch a live broadcast and scroll the full EPG grid at the same time. This is the product, not a stretched mobile app.
4. **Dedicated diagnostics.** Built-in HTTP tracing and stream probing to prove whether buffering belongs to the app, the network, or the provider. Redact passwords and MACs on export.

Zap secret from the deck: keep **one** long-lived mpv instance; inject URLs with `loadfile replace`. Target **<1s** channel change. Do not tear down the pipeline on zap.

Latency honesty from the deck: you cannot eliminate provider delay. You only expose the cache tradeoff (Low Latency `cache-secs=3` vs Stable `cache-secs=20–60`). Never promise “zero latency.” Provider HLS windows are often 6–20 seconds server-side.

---

## 0B. High-level system map (from architecture deck)

```
UI Layer          Tauri + React or Svelte
                  Virtualized lists (TanStack)
                  Split-layout UI state
                  Transparent overlay over native video

        ↕ typed IPC only (no video bytes)

Rust Core         Playlist / EPG parse off main thread
                  reqwest + custom UA + timeouts
                  Machine-bound license token (HMAC)

Data Layer        SQLite WAL + FTS5
                  playlists, channels, vod, epg, progress

Video Engine      Embedded libmpv on HWND / NSView / EGL
                  Single-connection record tee
                  ffmpeg/mpv encode only after file close
```

Stack choice locked: **Tauri v2 + Rust + libmpv**. Electron is rejected (150–300 MB, frame-copy risk). Flutter + media_kit is allowed only if the human explicitly switches stacks.

Budget for pain (deck): embedding libmpv is OS-specific. Write targeted Windows (`d3d11va` + `wid`) and macOS (`VideoToolbox` + render API / NSOpenGLView, not `wid`).

Data-model warnings from the deck (treat as bugs if violated):

- Stream the M3U. Never `read_to_string` a 40 MB list on the main thread.
- Normalize Unicode at index time (`pokémon` == `pokemon`).
- Virtualize every list.

Adapter priority from the deck:

1. Xtream Codes first (~80% of happy users, richest metadata).
2. M3U/M3U8 fallback (sniff Content-Type / first 512 bytes so HTML login pages are not parsed as playlists).
3. Stalker/MAC last, feature-flagged.

Defensive engineering from the deck:

- Filters break zero-copy hwdec → default `auto-safe`, always have copy-back fallback.
- 4 simultaneous streams trip cheap-plan connection caps → hard-limit N, warn, prefer copy hwdec on multi-pane so iGPUs survive.
- Users will demand zero latency → refuse in copy; document provider segment windows.

Monetization from the deck: 72-hour full trial on first import. ~$13 lifetime. Token check in Rust, never JS. No subscription in v1. No analytics by default.

---

## 0. Mission and non-negotiables

**One-sentence product:** Be the TiviMate of the desktop — libmpv for truth, SQLite for speed, EPG + player on one canvas, diagnostics that prove the provider is at fault, cheap lifetime unlock.

**Non-negotiables**

1. Player-only. Do not ship channels, playlists, stream URLs, or any content catalog of your own.
2. Video frames never enter Chromium / WebView / `<video>` / Canvas / WebGL copy paths. libmpv renders to a native OS surface.
3. Catalog lives in SQLite WAL + FTS5. Never load a 40k-channel M3U into a JS array as the source of truth.
4. One long-lived libmpv instance. Zap with `loadfile replace`. Do not recreate the player per channel.
5. Two named playback profiles only on day one: `low_latency` and `stable`. No 12 sliders in v1.
6. Secrets never appear in logs, diagnostics exports, crash reports, or screenshots. Redact `password`, `pass`, `token`, `mac`, `ticket`, `Authorization`.
7. Legal block on first-run, Settings, website copy, and store listing:

> This app does not provide channels, playlists, or stream URLs. You bring your own source. We do not support illegal services. We support the player, not the reseller.

8. Default product name in code: `desktop-iptv` / crate names `app-*`. Do not use "IPTV Player Zero", "Zero", or "Built By Board" as the product name.

---

## 1. What Zero is (context only — do not copy branding)

Official product: desktop player for Windows + macOS. Sources: M3U/M3U8, Xtream Codes, Stalker/MAC Portal. Developer: Built By Board Ltd (UK 15604498). Site: https://iptvplayerzero.com/

**Not this product:** Android/Firestick blogs titled "IPTV Player Zero"; the Smart TV app "Zero Play".

### Verified vs unverified (do not treat unverified as requirements)

**Verified from official site / stores / reviews**

- Win + Mac only. Direct + Microsoft Store + Mac App Store.
- Direct build observed v1.9.26 (~113.5 MB Windows). Mac App Store has lagged (e.g. 1.8.81 / ~172 MB). Fast cadence; site copy sometimes one patch behind.
- Free download. 72-hour full PRO after first playlist import. Then cramped free tier or $12.99 lifetime / 3-pack $29.99 / 5-pack $36.99. No subscription.
- Mac App Store: developer collects no data.
- Features marketed: live + EPG in one pane, folders/favorites/recents, recording, VOD downloads, continue watching, auto-next, PiP, multiscreen (beta), Sports Hub, themes, parental PIN + keyword filter, EPG offset, multiple EPGs, backup/restore, Diagnostics companion.
- Engine teardown (netthings.pt, May 2026): mpv, Windows `d3d11va`, stats panel (res/codec/bitrate), picture sliders, deband, motion smoothing, audio delay, volume to 130%, Low Latency vs Large Cache (~60s).
- Diagnostics app traces HTTP and can verify with embedded/external mpv. Redacted exports.
- Vendor comparison claims: only stable dedicated player on both desktop OSes; all three source types; scheduled recording; VOD downloads; catch-up up to 14 days; version picker / channel version grouping; Sports Hub. Treat 14-day catch-up and version grouping as **vendor marketing** until you implement your own equivalent.

**Unverified / do not block v1 on these**

- TV Maze live enrichment, TMDB optional posters, OpenSubtitles login.
- "End recording late" as a shipped Zero control (still implement it — it is a good feature).
- In-app Discord button / Reddit handle `u/mrpickledegg`.
- Specific UX bugs: no double-click fullscreen, unhideable Z logo, hover-out animation, cursor vanishing across monitors.

**Implement the gaps anyway (differentiation)**

- Double-click fullscreen.
- Hide-chrome / hide-brand toggle.
- Persist PiP size/position on Mac and Windows.
- Linux target from day one (Tauri + libmpv).
- On-player HUD for fps/res/bitrate.
- Documented Advanced mpv option map.
- Connection-aware multiscreen warning.
- Adaptive cache **after** static profiles work. Do not ship adaptive in week 1.

---

## 2. Stack

```
UI:        Tauri v2 + React 18 or Svelte 5 + TypeScript + Tailwind
           Virtualized lists: @tanstack/react-virtual (or equivalent)
Video:     libmpv via C-FFI (libmpv-sys or libmpv2) + raw-window-handle
           Windows/Linux: wid → HWND / X11 / Wayland
           macOS: mpv_render_context_create → NSOpenGLView / Cocoa layer
           NEVER html5 video, hls.js, mediaelement, Electron <video>
Core:      Rust workspace, tokio runtime
DB:        rusqlite + WAL + FTS5 (deadpool-sqlite optional)
Net:       reqwest + rustls, explicit timeouts, redirect limits, custom UA
Record:    single-connection tee of raw bytes (see §6.4) — not a second HTTP GET
License:   HMAC-SHA256 machine-bound token validated in Rust only
Packaging: Tauri bundler; separate updater later
```

**Why not Electron:** frames crossing Chromium adds copy latency, 300–500 MB RAM, weak license hiding.

**Flutter + media_kit is an allowed alternate stack** only if the user explicitly chooses it. Default is Tauri + Rust.

### Crate topology

```
app-core     lifecycle, config, license, window orchestration
app-db       SQLite pool, migrations, FTS, queries
app-net      HTTP, adapters (Xtream/M3U/Stalker), recording tap
app-engine   libmpv controller, profiles, telemetry, adaptive cache
app-ui       Tauri commands + events only (thin)
desktop/     Tauri app + frontend
```

Frontend talks to Rust only through typed IPC. Frontend never stores passwords in localStorage in the clear. Passwords live in the OS keychain / encrypted `pass` column accessed from Rust.

---

## 3. Repo layout to create

```
desktop-iptv/
  CLAUDE.md                          (this file, or a short pointer to it)
  Cargo.toml                         workspace
  crates/
    app-core/
    app-db/
    app-net/
    app-engine/
    app-ui/                          tauri command layer if split
  apps/desktop/
    src-tauri/
      tauri.conf.json
      src/main.rs
      capabilities/
    src/                             React/Svelte
      features/live/
      features/epg/
      features/vod/
      features/player/
      features/settings/
      features/diagnostics/
      lib/ipc.ts
  docs/
  migrations/
```

First command sequence when starting:

1. `npm create tauri-app` (or equivalent) in `apps/desktop`.
2. Init Cargo workspace at repo root; move Rust into `crates/` + `apps/desktop/src-tauri`.
3. Add `rusqlite` with bundled + hooks, `reqwest`, `tokio`, `serde`, `thiserror`, `tracing`.
4. Stub `app-engine` with a fake player (log "loadfile") so UI can develop before libmpv links.
5. Link libmpv on Windows via bundled DLL; macOS via system or bundled dylib; Linux via `libmpv-dev`.

---

## 4. Typed IPC contracts

Implement these types in Rust and mirror in TypeScript. Do not invent parallel shapes.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPayload {
    pub app_theme: String,
    pub hw_decoding: String, // "auto-safe" | "d3d11va" | "d3d11va-copy" | "videotoolbox" | "videotoolbox-copy" | "no"
    pub default_profile: String, // "low_latency" | "stable"
    pub auto_play_last: bool,
    pub max_multiscreen_instances: u8,
    pub hide_vod_tabs: bool,
    pub hide_brand_chrome: bool,
    pub hud_enabled: bool,
    pub audio_boost: u16, // 100..=130
    pub audio_delay_ms: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateLicenseCommand {
    pub license_key: String,
    pub machine_guid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseStateResponse {
    pub is_valid: bool,
    pub expires_at: Option<i64>,
    pub tier: String, // "free" | "trial" | "pro_lifetime"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtsQueryRequest {
    pub query_string: String,
    pub playlist_id: i64,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub id: i64,
    pub playlist_id: i64,
    pub source_id: String,
    pub name: String,
    pub normalized_name: String,
    pub group_title: Option<String>,
    pub logo: Option<String>,
    pub stream_url: String,
    pub tvg_id: Option<String>,
    pub catchup_days: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchPlaylistRequest {
    pub playlist_id: i64,
    pub target_url: String,
    pub user_agent: Option<String>,
    pub auth_credentials: Option<AuthCredentials>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCredentials {
    pub username: Option<String>,
    pub password: Option<String>,
    pub mac_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadStreamCommand {
    pub stream_url: String,
    pub profile_mode: String, // "low_latency" | "stable"
    pub audio_boost: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineTelemetryEvent {
    pub active_profile: String,
    pub width: u32,
    pub height: u32,
    pub codec_name: String,
    pub bitrate_kbps: u32,
    pub fps: f32,
    pub dropped_frames: u64,
    pub cache_duration_secs: f64,
    pub is_underrun: bool,
    pub zap_ms: Option<u64>,
}
```

```ts
export interface RenderStateSignal {
  viewport_start_index: number;
  viewport_end_index: number;
  selected_category: string | null;
  active_channel_id: number | null;
}
```

---

## 5. SQLite — PRAGMA, DDL, FTS

Apply on every connection:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -64000;          -- ~64 MB page cache
PRAGMA mmap_size = 268435456;        -- 256 MB mmap
PRAGMA busy_timeout = 5000;
```

Ingest rules:

- Parse M3U / Xtream / XMLTV on `tokio::task::spawn_blocking` or dedicated worker.
- Multi-row INSERT inside explicit transactions, **max 5,000 rows per chunk**.
- Normalize names at write time: lowercase, strip diacritics (`Pokémon` → `pokemon`), strip punctuation.
- Never `read_to_string` a huge remote M3U on the UI thread.

### DDL (implement exactly, then migrate)

```sql
CREATE TABLE IF NOT EXISTS playlists (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  type TEXT NOT NULL CHECK(type IN ('m3u', 'xtream', 'stalker')),
  name TEXT NOT NULL,
  base_url TEXT NOT NULL,
  user TEXT,
  pass TEXT,
  mac TEXT,
  ua TEXT,
  created DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS channels (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id INTEGER NOT NULL,
  source_id TEXT NOT NULL,
  name TEXT NOT NULL,
  normalized_name TEXT NOT NULL,
  "group" TEXT,
  logo TEXT,
  stream_url TEXT NOT NULL,
  tvg_id TEXT,
  tvg_name TEXT,
  catchup INTEGER DEFAULT 0,
  catchup_days INTEGER DEFAULT 0,
  UNIQUE(playlist_id, source_id),
  FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS vod_items (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('movie', 'series')),
  source_id TEXT NOT NULL,
  title TEXT NOT NULL,
  normalized_title TEXT NOT NULL,
  poster TEXT,
  year INTEGER,
  tmdb_id INTEGER,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(playlist_id, kind, source_id),
  FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS episodes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  series_id INTEGER NOT NULL,
  season INTEGER NOT NULL,
  episode INTEGER NOT NULL,
  stream_url TEXT NOT NULL,
  duration INTEGER,
  UNIQUE(series_id, season, episode),
  FOREIGN KEY(series_id) REFERENCES vod_items(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS epg_programmes (
  playlist_id INTEGER NOT NULL,
  channel_tvg_id TEXT NOT NULL,
  start DATETIME NOT NULL,
  stop DATETIME NOT NULL,
  title TEXT NOT NULL,
  desc TEXT,
  PRIMARY KEY (playlist_id, channel_tvg_id, start),
  FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS epg_overrides (
  channel_id INTEGER PRIMARY KEY,
  tvg_id_manual TEXT NOT NULL,
  FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS favorites (
  user_scope TEXT NOT NULL DEFAULT 'default',
  item_type TEXT NOT NULL CHECK(item_type IN ('channel', 'vod', 'series')),
  item_id INTEGER NOT NULL,
  PRIMARY KEY (user_scope, item_type, item_id)
);

CREATE TABLE IF NOT EXISTS progress (
  item_id INTEGER PRIMARY KEY,
  position_s INTEGER NOT NULL,
  updated DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS recordings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_id INTEGER NOT NULL,
  start DATETIME NOT NULL,
  stop DATETIME NOT NULL,
  extra_end_s INTEGER DEFAULT 600,
  path TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('scheduled', 'recording', 'completed', 'failed')),
  FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_channels_playlist ON channels(playlist_id);
CREATE INDEX IF NOT EXISTS idx_channels_tvg_id ON channels(tvg_id);
CREATE INDEX IF NOT EXISTS idx_channels_normalized_name ON channels(normalized_name);
CREATE INDEX IF NOT EXISTS idx_channels_group ON channels(playlist_id, "group");
CREATE INDEX IF NOT EXISTS idx_vod_playlist_kind ON vod_items(playlist_id, kind);
CREATE INDEX IF NOT EXISTS idx_episodes_series ON episodes(series_id);
CREATE INDEX IF NOT EXISTS idx_epg_lookup ON epg_programmes(playlist_id, channel_tvg_id, start, stop);

CREATE VIRTUAL TABLE IF NOT EXISTS search_idx USING fts5(
  name,
  group_name,
  content_type UNINDEXED,
  item_id UNINDEXED,
  playlist_id UNINDEXED
);

CREATE TRIGGER IF NOT EXISTS trg_channels_fts_insert AFTER INSERT ON channels BEGIN
  INSERT INTO search_idx(name, group_name, content_type, item_id, playlist_id)
  VALUES (new.normalized_name, COALESCE(new."group", ''), 'channel', new.id, new.playlist_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_fts_delete AFTER DELETE ON channels BEGIN
  DELETE FROM search_idx WHERE content_type = 'channel' AND item_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_fts_update AFTER UPDATE ON channels BEGIN
  DELETE FROM search_idx WHERE content_type = 'channel' AND item_id = old.id;
  INSERT INTO search_idx(name, group_name, content_type, item_id, playlist_id)
  VALUES (new.normalized_name, COALESCE(new."group", ''), 'channel', new.id, new.playlist_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_vod_fts_insert AFTER INSERT ON vod_items BEGIN
  INSERT INTO search_idx(name, group_name, content_type, item_id, playlist_id)
  VALUES (new.normalized_title, new.kind, 'vod', new.id, new.playlist_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_vod_fts_delete AFTER DELETE ON vod_items BEGIN
  DELETE FROM search_idx WHERE content_type = 'vod' AND item_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_vod_fts_update AFTER UPDATE ON vod_items BEGIN
  DELETE FROM search_idx WHERE content_type = 'vod' AND item_id = old.id;
  INSERT INTO search_idx(name, group_name, content_type, item_id, playlist_id)
  VALUES (new.normalized_title, new.kind, 'vod', new.id, new.playlist_id);
END;
```

Also store `last_channel_id` in `settings`.

---

## 6. Playback engine

### 6.1 Lifecycle

1. On app start: `mpv_create()` once. Initialize, bind surface, apply default profile.
2. Windows/Linux: `mpv_set_option(..., "wid", HWND/X11 id)`.
3. macOS: do **not** rely on `wid` (tearing / Cocoa hierarchy failures). Use `mpv_render_context_create` on `NSOpenGLView`.
4. Zap: `mpv_command_string(handle, "loadfile <url> replace")`.
5. hwdec fallback: Windows `d3d11va` → `d3d11va-copy` → `no`. macOS `videotoolbox` → `videotoolbox-copy` → `no`. Default user setting: `auto-safe`.
6. Observe properties: `video-params/w`, `h`, `video-codec`, `audio-codec`, `estimated-vf-fps`, `video-bitrate`, `demuxer-cache-duration`, `paused-for-cache`, `frame-drop-count`.
7. Measure `zap_ms` = click timestamp → first decoded frame / un-paused playback.

Target: **<800 ms zap** on a healthy HLS/TS stream. 20k-row list scrolls at 60 fps.

### 6.2 Profile matrix

| Parameter | low_latency | stable |
|---|---|---|
| cache | yes | yes |
| cache-secs | 3 | 20 (allow user 20–60) |
| demuxer-readahead-secs | 3 | 20 |
| demuxer-max-bytes | 32MiB | 400MiB |
| demuxer-max-back-bytes | 8MiB | 50MiB |
| hwdec | auto-safe | auto-safe |
| vo | gpu-next | gpu-next |
| video-latency-hacks | yes | no |
| deband | no | no (user optional) |
| interpolation | no | no |
| scale | bilinear | bilinear (advanced: better) |
| network-timeout | 8 | 15 |
| reconnect | 1 / streamed 1 / delay_max 5 | delay_max 10 |
| volume-max | 130 | 130 |

**Forbidden:** `cache=no`. It kills HLS readahead.

Inject as mpv options in Rust, not a user-visible conf file, except an Advanced panel that lists the same keys.

Low latency baseline blob:

```ini
cache=yes
cache-secs=3
demuxer-readahead-secs=3
demuxer-max-bytes=32MiB
demuxer-max-back-bytes=8MiB
hwdec=auto-safe
vo=gpu-next
untimed=no
video-latency-hacks=yes
deband=no
interpolation=no
scale=bilinear
network-timeout=8
stream-lavf-o-add=reconnect=1
stream-lavf-o-add=reconnect_streamed=1
stream-lavf-o-add=reconnect_delay_max=5
volume-max=130
audio-delay=0
```

Default **shipped** profile for new users: `stable` (fewer 1-star "it buffers" reviews). Sports users switch to `low_latency`.

### 6.3 Adaptive cache (phase 4+, behind flag)

Only when `low_latency` is active:

- If ≥2 underruns in a rolling 60s window: step `cache-secs` 3 → 6 → 10.
- If demuxer cache > 8s: temporarily `speed=1.05` until cache < 3s, then `speed=1.00`.
- Do not enable by default in v1. Users hate rubber-banding audio. When you ship it, make it opt-in "Auto stabilize".

### 6.4 Single-connection record tap

Providers often ban a second HTTP connection.

Required design:

1. `app-net` opens **one** HTTP(S) GET for the live URL.
2. Bytes go into `tokio::sync::broadcast` (or a tee to two consumers).
3. Branch A: append raw MPEG-TS/HLS to `recordings.path` (`.ts` or `.mkv` container; do not remux to MP4 until the file is closed).
4. Branch B: feed the same bytes into libmpv via a custom stream / pipe / memory protocol.

If tee is too hard in week 1, record-only-when-not-previewing is an acceptable interim, but the exit test for days 51–70 is **watch + record on one connection**.

Recording jobs: SQLite `recordings` row, `extra_end_s` default 600 (10 min overrun). App must stay running for scheduled jobs.

VOD download: separate GET with Range resume, write media + sidecar `.srt`. Disk-space warning (4h × 8 Mbps ≈ 14 GB).

### 6.5 Multiscreen

- Default max 2, hard max 4.
- Only one pane has audio.
- Prefer copy hwdec (`d3d11va-copy`) for multi-instance.
- If Xtream account `max_connections` is known, warn when panes + recordings would exceed it.
- Each pane is another decoder. Do not spawn 4 zero-copy 1080p60 on an iGPU without a warning.

### 6.6 What you cannot fix (copy into Help)

Provider HLS window (often 6–20s), overloaded node, Wi-Fi jitter, connection caps, DRM/token streams. Zap time ≠ live delay.

---

## 7. Source adapters

Trait:

```rust
#[async_trait]
pub trait CatalogAdapter: Send + Sync {
    async fn authenticate(&self) -> Result<AccountInfo>;
    async fn sync_live(&self, playlist_id: i64) -> Result<SyncStats>;
    async fn sync_vod(&self, playlist_id: i64) -> Result<SyncStats>;
    async fn sync_epg(&self, playlist_id: i64) -> Result<SyncStats>;
}
```

UI never calls HTTP.

### 7.1 Xtream — implement first

```
GET {base}/player_api.php?username=&password=
GET ...&action=get_live_categories
GET ...&action=get_vod_categories
GET ...&action=get_series_categories
GET ...&action=get_live_streams
GET ...&action=get_vod_streams
GET ...&action=get_series
GET ...&action=get_series_info&series_id=
GET {base}/xmltv.php?username=&password=
```

Live playback URL typical form (do not hardcode only one; some panels differ):

`{base}/live/{user}/{pass}/{stream_id}.ts` or `.m3u8`

Map:

| API field | Table.column | Rule |
|---|---|---|
| stream_id | source_id | string |
| name | name/title | UTF-8 + normalize |
| category_id | group | resolve via category map |
| stream_icon | logo/poster | URL validate |
| epg_channel_id | tvg_id | direct |
| releasedate | year | YYYY from date |
| added | created_at | unix → ISO |
| series_id | episodes.series_id | FK |

Store `max_connections`, `exp_date`, `status` from login JSON in settings or a `accounts` table. Never log password.

### 7.2 M3U — implement second

Stream parse line-by-line.

Preflight: first 512 bytes. If HTML (`<html`, `<!doctype`) or XMLTV (`<tv`, `<?xml` with programme tags and no `#EXTM3U`) → hard error with diagnostics, do not parse as playlist.

Parse `#EXTINF` attrs: `tvg-id`, `tvg-name`, `tvg-logo`, `group-title`, `catchup`, `timeshift`, `tvg-rec`.

Handle `#EXTM3U`, `#EXTGRP`, extra headers, UTF-8 BOM.

### 7.3 Stalker / MAC — last, feature-flagged

Handshake, token, MAC identity, portal routes. Log every hop redacted. Do not block v1 launch on Stalker.

---

## 8. UI / UX specification

### Layout (must)

```
[ clock | PRO/trial | brand ]     [ Live | Movies | Series ]     [ playlist | bell | gear ]

[ rail ]   [ native video surface + overlay chrome ]   [ Now / Next card ]
           [ transport ]
           [ EPG grid  OR  VOD poster grid ]
```

**Rail:** Favorites, Recordings, Recently Viewed, category search, All, Create folder, groups. Later: Sports Hub, Multiscreen.

**Player chrome:** info, PiP, settings, favorite, record, expand, LIVE badge, volume. Optional always-on HUD: `1920x1080 · H.264 · 3.3 Mbps · 50fps · cache 2.1s`.

**EPG:** now-line, hour window (default 8h), hover description, right-click schedule record, Edit EPG / offset, Now jump, fullscreen EPG.

**Interaction**

- Watch + browse guide simultaneously.
- Last channel auto-play / auto-expand.
- Global FTS search live + VOD.
- Continue watching + auto-next episode.
- Themes via CSS variables; dark default; user-exportable token JSON.
- Hide Movies/Series tabs.
- Hide brand chrome.
- Double-click video → fullscreen.
- Keyboard: `/` search, `j/k` or arrows channels, `←/→` EPG time, `f` fullscreen, `m` mute, `space` pause (VOD only; live pause is optional).

**Performance UX**

- Skeleton EPG. Show `Guide importing 34%`. Never block first paint on XMLTV.
- Virtualize every list.
- Lazy posters + disk cache. TMDB is optional and later.
- Test mixed-DPI multi-monitor.

Do not build Sports Hub until playback + EPG are stable.

---

## 9. Diagnostics

Ship as a Settings tab first. Companion process later if needed.

Must capture:

1. Method, redacted URL, status, timing, content-type, first 200 bytes.
2. Redirect chain.
3. Parse result: channel count, groups, missing URLs, "got HTML not M3U".
4. Xtream account: status, exp, max connections — no password.
5. Isolated headless mpv probe: container, codec, time-to-first-frame, error string.
6. One-click copy sanitized report.

Redact regex keys: `password=`, `pass=`, `token=`, `mac=`, `ticket=`, `Authorization`.

In-app isolation copy:

1. Other channel same source works → this stream.
2. Other source works → this provider.
3. Ethernet / VPN off changes it → path.
4. Stop recordings / multiscreen → local load.

Change one variable per test.

---

## 10. License, trial, privacy

Tiers: `free` | `trial` | `pro_lifetime`.

Trial: start on first successful playlist import, 72 hours, no card.

Free after trial: one playlist, live TV works, trimmed EPG horizon, no recording/downloads/unlimited VOD.

PRO: unlimited playlists/favorites, full EPG, reminders, recording, downloads, backup, multiscreen >1.

License check in Rust. Device GUID + signed token. Store IAP path later.

Telemetry default **off**. If added: opt-in crash + engine error codes only. No playlist URLs.

Backup/restore: encrypted export of playlists (without plaintext passwords if possible — or keychain re-link), favorites, settings, EPG overrides.

---

## 11. Defaults

| Setting | Default | Why |
|---|---|---|
| profile | stable | fewer buffer complaints |
| hwdec | auto-safe | 4K HEVC |
| EPG offset | 0 | don't auto-guess |
| autoplay last | on | cable-box feel |
| refresh on launch | on + backoff | stale lists look like bugs |
| parental PIN | off | |
| analytics | off | trust |
| theme | dark | |
| adaptive cache | off | |

---

## 12. 90-day plan and exit tests

### Days 1–14 — ugly but plays

- Tauri + crate workspace
- Native surface stub + libmpv bind
- SQLite WAL + DDL
- Streaming M3U import
- Virtualized channel list
- `loadfile replace` zap
- two profiles

**Exit:** 20k-channel M3U in SQLite; 60 fps scroll; zap <800 ms on a healthy stream.

### Days 15–30 — Xtream + EPG

- Xtream adapter
- XMLTV import + offset + overrides
- Live + EPG split view
- Favorites, recents
- OSD telemetry
- sanitized error log

**Exit:** 7-day EPG rendered beside live video; catalog refresh does not drop UI frames.

### Days 31–50 — VOD + UX

- Movies/series grids
- progress + auto-next
- parental PIN + keyword filter
- theme tokens
- multiple playlists

**Exit:** resume position correct; PIN hides filtered groups.

### Days 51–70 — money features

- single-connection record + overrun
- VOD download + srt
- PiP persist geometry
- 2-pane multiscreen, audio lock, copy hwdec
- license + 72h trial

**Exit:** scheduled recording while 2-pane playback on one connection tap.

### Days 71–90 — support product

- diagnostics workbench + redaction
- Stalker behind flag
- encrypted backup/restore
- packaging Win/Mac/Linux
- Sports Hub only if the rest is boringly stable

**Exit:** diagnostics isolates a bad URL and exports a fully redacted log.

---

## 13. Engineering checklist

**Playback**

- [ ] Single long-lived libmpv
- [ ] Native embed, no WebView frames
- [ ] Two profiles + Advanced
- [ ] hwdec auto-safe + copy fallback
- [ ] Reconnect flags
- [ ] zap_ms logged
- [ ] Audio delay + 130% boost
- [ ] External mpv/VLC escape hatch

**Catalog**

- [ ] Streaming parser
- [ ] FTS5 search
- [ ] tvg-id + manual override
- [ ] Unicode normalize
- [ ] Incremental EPG

**Net**

- [ ] Custom UA, timeouts, redirect limit
- [ ] Detect HTML/XMLTV-as-M3U
- [ ] Redact secrets everywhere

**Desktop**

- [ ] Virtualized lists
- [ ] Watch + guide same window
- [ ] Last channel restore
- [ ] Mixed-DPI tested
- [ ] Double-click fullscreen
- [ ] Hide chrome

**Ops**

- [ ] Player-only legal block
- [ ] Copy diagnostics
- [ ] Opt-in crash only
- [ ] Updater (can be later)

---

## 14. Help / FAQ copy to ship in-app

**Does this include channels?** No. Player-only. You provide M3U, Xtream, or Stalker credentials.

**Does it buffer?** Buffering follows the stream, the source, the network, or the device. Change one variable. Open Diagnostics.

**Which source type?** Xtream first if offered. M3U is flexible. Stalker is for portal providers.

**Can I record?** PRO feature, supported sources. Keep the app running. Use end-late for sports. Recording uses the same connection as preview when possible.

**Why am I behind the pub TV?** Provider HLS window. Low Latency shrinks *our* cache only. Switch server/CDN with your provider.

---

## 15. Implementation order for the first coding session

Do these in order. Do not skip ahead to Sports Hub, themes, or Stalker.

1. Scaffold Tauri + workspace + this file as `CLAUDE.md` in the repo root.
2. `app-db` migrations + PRAGMA + Channel CRUD + FTS.
3. M3U file/URL importer with HTML preflight.
4. Virtualized Live list in the UI reading paged SQL (`LIMIT/OFFSET` or keyset).
5. `app-engine` stub, then real libmpv bind on the current OS.
6. Click channel → `LoadStreamCommand` → `loadfile replace`.
7. Profile toggle low_latency / stable.
8. Telemetry event → tiny HUD.
9. Only then Xtream.

When stuck on libmpv embed, keep the catalog UI working against a stub engine that logs URLs. Do not block the whole app on video.

---

## 16. Sources absorbed into this file

This handoff is the merge of:

- Official IPTV Player Zero site, guides, Diagnostics page, Windows/Mac pages, store listings
- Third-party reviews (netthings.pt mpv/d3d11va teardown; iptvranking; iptvexplained)
- Independent verification pass (flagged TV Maze / TMDB / OpenSubtitles / Discord / specific UX bugs as unverified)
- Module-level technical spec (crate topology, IPC types, full DDL + FTS triggers, adaptive cache, record tee)
- Zero-Class architecture deck (philosophy, stack table, system map, split-canvas IA, defensive edge cases, 90-day assembly, legal/monetization)
- Open-source embed notes (MaxVideoPlayer, IPTVnator, media_kit / clubTivi, Nightmare TV libmpv pipeline)

Do not scrape or copy Zero artwork, trademarks, or store listing text into the product.

## 16B. Reference links (research only)

- https://iptvplayerzero.com/
- https://iptvplayerzero.com/guides/why-iptv-streams-buffer
- https://iptvplayerzero.com/guides/iptv-player-comparison
- https://iptvplayerzero.com/guides/m3u-vs-xtream-vs-stalker
- https://iptvplayerzero.com/iptv-zero-diagnostics
- https://nightmaretv.net/learn/libmpv-iptv-windows
- Open-source embed references: MaxVideoPlayer (Tauri+libmpv), IPTVnator (per-OS embed pain), clubTivi / media_kit

Do not scrape or copy Zero's artwork, trademarks, or store listing text into the product.

---

## 17. Definition of done for v0.1

A stranger can:

1. Install on Windows or Mac.
2. Add an M3U URL.
3. Scroll a large list smoothly.
4. Play a channel in a native surface (not a web video tag).
5. Switch channels quickly.
6. Toggle Low Latency / Stable.
7. See redacted errors if the URL is HTML.
8. Read the player-only disclaimer.

If that works, you have a product. Everything else is iteration.
