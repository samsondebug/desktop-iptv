-- SKTV schema v4: channel curation — per-channel rename & hide, per-group hide.
-- Overrides live beside the synced rows so a playlist refresh never wipes them.

CREATE TABLE IF NOT EXISTS channel_overrides (
  channel_id INTEGER PRIMARY KEY,
  custom_name TEXT,
  hidden INTEGER NOT NULL DEFAULT 0,
  FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS group_overrides (
  playlist_id INTEGER NOT NULL,
  "group" TEXT NOT NULL,
  hidden INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (playlist_id, "group"),
  FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
);
