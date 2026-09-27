-- desktop-iptv schema v1 (CLAUDE.md §5). Applied inside a transaction by app-db.
-- Do not edit in place after shipping: add 0002_*.sql instead.

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

-- Global FTS index over live + VOD. The FTS rowid is derived from the source row id so
-- DELETE/UPDATE triggers are O(log n) instead of scanning an UNINDEXED column:
--   channel → rowid = id * 2      vod → rowid = id * 2 + 1
CREATE VIRTUAL TABLE IF NOT EXISTS search_idx USING fts5(
  name,
  group_name,
  content_type UNINDEXED,
  item_id UNINDEXED,
  playlist_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS trg_channels_fts_insert AFTER INSERT ON channels BEGIN
  INSERT INTO search_idx(rowid, name, group_name, content_type, item_id, playlist_id)
  VALUES (new.id * 2, new.normalized_name, COALESCE(new."group", ''), 'channel', new.id, new.playlist_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_fts_delete AFTER DELETE ON channels BEGIN
  DELETE FROM search_idx WHERE rowid = old.id * 2;
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_fts_update AFTER UPDATE OF normalized_name, "group", playlist_id ON channels BEGIN
  DELETE FROM search_idx WHERE rowid = old.id * 2;
  INSERT INTO search_idx(rowid, name, group_name, content_type, item_id, playlist_id)
  VALUES (new.id * 2, new.normalized_name, COALESCE(new."group", ''), 'channel', new.id, new.playlist_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_vod_fts_insert AFTER INSERT ON vod_items BEGIN
  INSERT INTO search_idx(rowid, name, group_name, content_type, item_id, playlist_id)
  VALUES (new.id * 2 + 1, new.normalized_title, new.kind, 'vod', new.id, new.playlist_id);
END;

CREATE TRIGGER IF NOT EXISTS trg_vod_fts_delete AFTER DELETE ON vod_items BEGIN
  DELETE FROM search_idx WHERE rowid = old.id * 2 + 1;
END;

CREATE TRIGGER IF NOT EXISTS trg_vod_fts_update AFTER UPDATE OF normalized_title, kind, playlist_id ON vod_items BEGIN
  DELETE FROM search_idx WHERE rowid = old.id * 2 + 1;
  INSERT INTO search_idx(rowid, name, group_name, content_type, item_id, playlist_id)
  VALUES (new.id * 2 + 1, new.normalized_title, new.kind, 'vod', new.id, new.playlist_id);
END;

-- Recently viewed (rail) — not in the original DDL but needed for "Recently Viewed" on day 1.
CREATE TABLE IF NOT EXISTS recents (
  channel_id INTEGER PRIMARY KEY,
  viewed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_recents_viewed ON recents(viewed_at DESC);
