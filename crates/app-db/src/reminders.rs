//! Programme reminders: notify shortly before a guide entry starts. Times are unix seconds.

use crate::{Db, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderRecord {
    pub id: i64,
    pub channel_id: i64,
    pub channel_name: String,
    pub start: i64,
    pub stop: i64,
    pub title: String,
}

const COLS: &str = "r.id, r.channel_id, ch.name, r.start, r.stop, r.title";

fn row_to_reminder(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReminderRecord> {
    Ok(ReminderRecord {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        channel_name: r.get(2)?,
        start: r.get(3)?,
        stop: r.get(4)?,
        title: r.get(5)?,
    })
}

impl Db {
    /// Insert (or replace an unfired duplicate for the same channel+start). Returns the row id.
    pub fn add_reminder(&self, channel_id: i64, start: i64, stop: i64, title: &str) -> Result<i64> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO reminders(channel_id, start, stop, title) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(channel_id, start) DO UPDATE SET title = excluded.title, stop = excluded.stop, fired = 0",
                params![channel_id, start, stop, title],
            )?;
            Ok(c.query_row(
                "SELECT id FROM reminders WHERE channel_id = ?1 AND start = ?2",
                params![channel_id, start],
                |r| r.get(0),
            )?)
        })
    }

    pub fn delete_reminder(&self, id: i64) -> Result<()> {
        self.with_write(|c| {
            c.execute("DELETE FROM reminders WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Upcoming (unfired, still in the future or currently airing) reminders, soonest first.
    pub fn list_reminders(&self, now: i64) -> Result<Vec<ReminderRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {COLS} FROM reminders r JOIN channels ch ON ch.id = r.channel_id
                 WHERE r.fired = 0 AND (r.stop > ?1 OR r.start > ?1) ORDER BY r.start"
            ))?;
            let it = st.query_map(params![now], row_to_reminder)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Reminders whose window `[start - lead, start + grace]` contains `now`, marked fired
    /// atomically so a tick never notifies twice. Ones missed by more than `grace` (the app was
    /// closed) are silently marked fired.
    pub fn take_due_reminders(&self, now: i64, lead_secs: i64, grace_secs: i64) -> Result<Vec<ReminderRecord>> {
        self.with_write(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {COLS} FROM reminders r JOIN channels ch ON ch.id = r.channel_id
                 WHERE r.fired = 0 AND r.start - ?2 <= ?1 AND r.start + ?3 >= ?1 ORDER BY r.start"
            ))?;
            let due = st
                .query_map(params![now, lead_secs, grace_secs], row_to_reminder)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            // Mark everything whose lead window has been entered — including reminders missed by
            // more than `grace` (app was closed) so they never fire late.
            c.execute("UPDATE reminders SET fired = 1 WHERE fired = 0 AND start - ?2 <= ?1", params![now, lead_secs])?;
            Ok(due)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn seed_channel(db: &Db) -> i64 {
        use crate::channels::PlaylistInsert;
        let pid = db
            .insert_playlist(&PlaylistInsert {
                r#type: "m3u".into(),
                name: "t".into(),
                base_url: "file:///t.m3u".into(),
                user: None,
                pass: None,
                mac: None,
                ua: None,
            })
            .unwrap();
        db.with_write(|c| {
            c.execute(
                "INSERT INTO channels(playlist_id, source_id, name, normalized_name, stream_url) VALUES (?1, 's1', 'News One', 'news one', 'http://x/1.ts')",
                params![pid],
            )?;
            Ok(c.last_insert_rowid())
        })
        .unwrap()
    }

    #[test]
    fn add_list_fire() {
        let db = db();
        let ch = seed_channel(&db);
        let id = db.add_reminder(ch, 1000, 1600, "The Match").unwrap();
        assert!(id > 0);
        // Duplicate replaces, does not add.
        let id2 = db.add_reminder(ch, 1000, 1600, "The Match (updated)").unwrap();
        assert_eq!(id, id2);
        assert_eq!(db.list_reminders(500).unwrap().len(), 1);

        // Not due yet at t=800 with 60 s lead.
        assert!(db.take_due_reminders(800, 60, 300).unwrap().is_empty());
        // Due inside the lead window; second call returns nothing (fired).
        let due = db.take_due_reminders(950, 60, 300).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].title, "The Match (updated)");
        assert!(db.take_due_reminders(960, 60, 300).unwrap().is_empty());
        assert!(db.list_reminders(960).unwrap().is_empty());
    }

    #[test]
    fn missed_reminders_fire_silently() {
        let db = db();
        let ch = seed_channel(&db);
        db.add_reminder(ch, 1000, 1600, "Old").unwrap();
        // Way past the grace window: not returned, but marked fired.
        assert!(db.take_due_reminders(5000, 60, 300).unwrap().is_empty());
        assert!(db.list_reminders(5000).unwrap().is_empty());
    }
}
