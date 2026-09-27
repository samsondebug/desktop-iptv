-- desktop-iptv schema v2: EPG sources/offsets, richer VOD, progress by item type, downloads,
-- recording metadata, stale-row tracking. Applied inside a transaction by app-db.

ALTER TABLE playlists ADD COLUMN epg_offset_min INTEGER NOT NULL DEFAULT 0;
ALTER TABLE playlists ADD COLUMN stream_format TEXT NOT NULL DEFAULT 'ts';   -- xtream: 'ts' | 'm3u8'
ALTER TABLE playlists ADD COLUMN account_json TEXT;                          -- xtream account info, never the password
ALTER TABLE playlists ADD COLUMN last_synced DATETIME;
ALTER TABLE playlists ADD COLUMN last_error TEXT;

ALTER TABLE channels ADD COLUMN sync_gen INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN chno INTEGER;

CREATE TABLE IF NOT EXISTS epg_sources (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  playlist_id INTEGER NOT NULL,
  url TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_synced DATETIME,
  last_error TEXT,
  programme_count INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_epg_sources_playlist ON epg_sources(playlist_id);

-- epg_programmes.start/stop are stored as unix seconds (INTEGER) from v2 on.
CREATE INDEX IF NOT EXISTS idx_epg_stop ON epg_programmes(playlist_id, stop);

ALTER TABLE vod_items ADD COLUMN category TEXT;
ALTER TABLE vod_items ADD COLUMN description TEXT;
ALTER TABLE vod_items ADD COLUMN rating REAL;
ALTER TABLE vod_items ADD COLUMN genre TEXT;
ALTER TABLE vod_items ADD COLUMN duration_s INTEGER;
ALTER TABLE vod_items ADD COLUMN stream_url TEXT;
ALTER TABLE vod_items ADD COLUMN container_ext TEXT;
ALTER TABLE vod_items ADD COLUMN backdrop TEXT;
ALTER TABLE vod_items ADD COLUMN added INTEGER;            -- unix seconds from the provider
ALTER TABLE vod_items ADD COLUMN sync_gen INTEGER NOT NULL DEFAULT 0;
ALTER TABLE vod_items ADD COLUMN episodes_synced DATETIME;
CREATE INDEX IF NOT EXISTS idx_vod_category ON vod_items(playlist_id, kind, category);
CREATE INDEX IF NOT EXISTS idx_vod_added ON vod_items(playlist_id, kind, added DESC);

ALTER TABLE episodes ADD COLUMN source_id TEXT;
ALTER TABLE episodes ADD COLUMN title TEXT;
ALTER TABLE episodes ADD COLUMN poster TEXT;
ALTER TABLE episodes ADD COLUMN container_ext TEXT;

-- Progress keyed by (item_type, item_id) so movies and episodes cannot collide.
CREATE TABLE IF NOT EXISTS playback_progress (
  item_type TEXT NOT NULL CHECK(item_type IN ('vod', 'episode', 'channel')),
  item_id INTEGER NOT NULL,
  position_s INTEGER NOT NULL,
  duration_s INTEGER,
  finished INTEGER NOT NULL DEFAULT 0,
  updated DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (item_type, item_id)
);
CREATE INDEX IF NOT EXISTS idx_progress_updated ON playback_progress(updated DESC);

ALTER TABLE recordings ADD COLUMN title TEXT;
ALTER TABLE recordings ADD COLUMN bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE recordings ADD COLUMN error TEXT;
ALTER TABLE recordings ADD COLUMN created DATETIME DEFAULT CURRENT_TIMESTAMP;
CREATE INDEX IF NOT EXISTS idx_recordings_status ON recordings(status, start);

CREATE TABLE IF NOT EXISTS downloads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_type TEXT NOT NULL CHECK(item_type IN ('vod', 'episode')),
  item_id INTEGER NOT NULL,
  title TEXT NOT NULL,
  path TEXT NOT NULL,
  bytes_done INTEGER NOT NULL DEFAULT 0,
  bytes_total INTEGER,
  status TEXT NOT NULL CHECK(status IN ('queued', 'downloading', 'paused', 'completed', 'failed')),
  error TEXT,
  created DATETIME DEFAULT CURRENT_TIMESTAMP,
  updated DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status, id);
