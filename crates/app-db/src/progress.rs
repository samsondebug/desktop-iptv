//! Playback progress ("continue watching") keyed by (item_type, item_id).

use crate::{Db, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressRecord {
    pub item_type: String,
    pub item_id: i64,
    pub position_s: i64,
    pub duration_s: Option<i64>,
    pub finished: bool,
    pub updated: String,
}

/// A watched-position threshold: past this fraction the item counts as finished.
pub const FINISHED_FRACTION: f64 = 0.95;

impl Db {
    pub fn set_progress(&self, item_type: &str, item_id: i64, position_s: i64, duration_s: Option<i64>) -> Result<()> {
        let finished = match duration_s {
            Some(d) if d > 0 => (position_s as f64) / (d as f64) >= FINISHED_FRACTION,
            _ => false,
        };
        self.with_write(|c| {
            c.execute(
                "INSERT INTO playback_progress(item_type, item_id, position_s, duration_s, finished, updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
                 ON CONFLICT(item_type, item_id) DO UPDATE SET position_s = excluded.position_s,
                   duration_s = COALESCE(excluded.duration_s, playback_progress.duration_s),
                   finished = excluded.finished, updated = CURRENT_TIMESTAMP",
                params![item_type, item_id, position_s.max(0), duration_s, finished as i32],
            )?;
            Ok(())
        })
    }

    pub fn get_progress(&self, item_type: &str, item_id: i64) -> Result<Option<ProgressRecord>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT item_type, item_id, position_s, duration_s, finished, updated FROM playback_progress WHERE item_type = ?1 AND item_id = ?2",
                params![item_type, item_id],
                row_to_progress,
            )
            .optional()?)
        })
    }

    pub fn clear_progress(&self, item_type: &str, item_id: i64) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "DELETE FROM playback_progress WHERE item_type = ?1 AND item_id = ?2",
                params![item_type, item_id],
            )?;
            Ok(())
        })
    }

    /// Unfinished VOD/episode items, most recent first.
    pub fn continue_watching(&self, limit: usize) -> Result<Vec<ProgressRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT item_type, item_id, position_s, duration_s, finished, updated FROM playback_progress
                 WHERE finished = 0 AND item_type IN ('vod', 'episode') AND position_s > 30
                 ORDER BY updated DESC LIMIT ?1",
            )?;
            let it = st.query_map(params![limit as i64], row_to_progress)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }
}

fn row_to_progress(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProgressRecord> {
    Ok(ProgressRecord {
        item_type: r.get(0)?,
        item_id: r.get(1)?,
        position_s: r.get(2)?,
        duration_s: r.get(3)?,
        finished: r.get::<_, i64>(4)? != 0,
        updated: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.set_progress("vod", 7, 120, Some(6000)).unwrap();
        let p = db.get_progress("vod", 7).unwrap().unwrap();
        assert_eq!(p.position_s, 120);
        assert!(!p.finished);
        db.set_progress("vod", 7, 5900, None).unwrap();
        let p = db.get_progress("vod", 7).unwrap().unwrap();
        assert_eq!(p.duration_s, Some(6000), "duration kept");
        assert!(!p.finished, "finished is only computed when duration is passed");
        db.set_progress("vod", 7, 5900, Some(6000)).unwrap();
        assert!(db.get_progress("vod", 7).unwrap().unwrap().finished);
        db.set_progress("episode", 7, 200, Some(1000)).unwrap();
        let cw = db.continue_watching(10).unwrap();
        assert_eq!(cw.len(), 1);
        assert_eq!(cw[0].item_type, "episode");
        db.clear_progress("episode", 7).unwrap();
        assert!(db.continue_watching(10).unwrap().is_empty());
    }
}
