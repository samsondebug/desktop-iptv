-- SKTV schema v3: programme reminders (notify shortly before a guide entry starts).
-- start/stop are unix seconds, matching recordings.

CREATE TABLE IF NOT EXISTS reminders (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_id INTEGER NOT NULL,
  start INTEGER NOT NULL,
  stop INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL,
  fired INTEGER NOT NULL DEFAULT 0,
  created DATETIME DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(channel_id, start),
  FOREIGN KEY(channel_id) REFERENCES channels(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_reminders_due ON reminders(fired, start);
