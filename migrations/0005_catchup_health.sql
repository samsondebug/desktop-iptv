-- SKTV schema v5: catch-up metadata on channels + per-channel stream-health snapshots.

-- How this channel's archive is reached: 'default' | 'append' | 'shift' | 'flussonic' | 'xc'
-- (NULL = provider gave only a flag/days; the playlist type decides). `catchup_source` is the
-- raw template from the playlist (may embed credentials — never leaves Rust).
ALTER TABLE channels ADD COLUMN catchup_kind TEXT;
ALTER TABLE channels ADD COLUMN catchup_source TEXT;

-- Last known stream health, written from live playback telemetry and on-demand probes.
CREATE TABLE IF NOT EXISTS channel_health (
  channel_id INTEGER PRIMARY KEY,
  ok INTEGER NOT NULL DEFAULT 1,
  width INTEGER,
  height INTEGER,
  codec TEXT,
  bitrate_kbps INTEGER,
  source TEXT NOT NULL DEFAULT 'playback' CHECK(source IN ('playback', 'probe')),
  checked_at INTEGER NOT NULL,
  FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);
