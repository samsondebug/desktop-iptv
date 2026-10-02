//! Recording jobs and VOD downloads (rows only — the workers live in app-net / the Tauri layer).
//! Times are unix seconds.

use crate::{Db, DbError, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingRecord {
    pub id: i64,
    pub channel_id: i64,
    pub channel_name: String,
    pub title: Option<String>,
    pub start: i64,
    pub stop: i64,
    pub extra_end_s: i64,
    pub path: String,
    pub status: String, // scheduled | recording | completed | failed
    pub bytes: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub id: i64,
    pub item_type: String,
    pub item_id: i64,
    pub title: String,
    pub path: String,
    pub bytes_done: i64,
    pub bytes_total: Option<i64>,
    pub status: String, // queued | downloading | paused | completed | failed
    pub error: Option<String>,
}

const REC_COLS: &str =
    "r.id, r.channel_id, ch.name, r.title, r.start, r.stop, r.extra_end_s, r.path, r.status, r.bytes, r.error";

fn row_to_rec(r: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingRecord> {
    Ok(RecordingRecord {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        channel_name: r.get(2)?,
        title: r.get(3)?,
        start: r.get(4)?,
        stop: r.get(5)?,
        extra_end_s: r.get(6)?,
        path: r.get(7)?,
        status: r.get(8)?,
        bytes: r.get(9)?,
        error: r.get(10)?,
    })
}

const DL_COLS: &str = "id, item_type, item_id, title, path, bytes_done, bytes_total, status, error";

fn row_to_dl(r: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRecord> {
    Ok(DownloadRecord {
        id: r.get(0)?,
        item_type: r.get(1)?,
        item_id: r.get(2)?,
        title: r.get(3)?,
        path: r.get(4)?,
        bytes_done: r.get(5)?,
        bytes_total: r.get(6)?,
        status: r.get(7)?,
        error: r.get(8)?,
    })
}

impl Db {
    #[allow(clippy::too_many_arguments)]
    pub fn add_recording(
        &self,
        channel_id: i64,
        title: Option<&str>,
        start: i64,
        stop: i64,
        extra_end_s: i64,
        path: &str,
        status: &str,
    ) -> Result<i64> {
        if stop <= start {
            return Err(DbError::Other("recording stop must be after start".into()));
        }
        self.with_write(|c| {
            c.execute(
                "INSERT INTO recordings(channel_id, title, start, stop, extra_end_s, path, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![channel_id, title, start, stop, extra_end_s.max(0), path, status],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn set_recording_status(&self, id: i64, status: &str, bytes: Option<i64>, error: Option<&str>) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "UPDATE recordings SET status = ?2, bytes = COALESCE(?3, bytes), error = ?4 WHERE id = ?1",
                params![id, status, bytes, error.map(app_core::redact::redact)],
            )?;
            Ok(())
        })
    }

    pub fn delete_recording(&self, id: i64) -> Result<()> {
        self.with_write(|c| {
            if c.execute("DELETE FROM recordings WHERE id = ?1", params![id])? == 0 {
                return Err(DbError::NotFound);
            }
            Ok(())
        })
    }

    pub fn list_recordings(&self) -> Result<Vec<RecordingRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {REC_COLS} FROM recordings r JOIN channels ch ON ch.id = r.channel_id ORDER BY r.start DESC LIMIT 500"
            ))?;
            let it = st.query_map([], row_to_rec)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn get_recording(&self, id: i64) -> Result<RecordingRecord> {
        self.with_read(|c| {
            c.query_row(
                &format!(
                    "SELECT {REC_COLS} FROM recordings r JOIN channels ch ON ch.id = r.channel_id WHERE r.id = ?1"
                ),
                params![id],
                row_to_rec,
            )
            .optional()?
            .ok_or(DbError::NotFound)
        })
    }

    /// Scheduled jobs whose start time has arrived (and whose end, incl. overrun, is still ahead).
    pub fn due_recordings(&self, now: i64) -> Result<Vec<RecordingRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {REC_COLS} FROM recordings r JOIN channels ch ON ch.id = r.channel_id
                 WHERE r.status = 'scheduled' AND r.start <= ?1 AND (r.stop + r.extra_end_s) > ?1 ORDER BY r.start"
            ))?;
            let it = st.query_map(params![now], row_to_rec)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Scheduled jobs whose whole window (incl. overrun) passed while the app was closed.
    pub fn expire_missed_recordings(&self, now: i64) -> Result<u64> {
        self.with_write(|c| {
            Ok(c.execute(
                "UPDATE recordings SET status = 'failed', error = 'missed: app was not running' WHERE status = 'scheduled' AND (stop + extra_end_s) <= ?1",
                params![now],
            )? as u64)
        })
    }

    /// Rows left in `recording` by a previous process (crash / force quit): no job survives a restart,
    /// so mark them failed with whatever bytes reached disk. Call once at startup, before the scheduler.
    pub fn fail_interrupted_recordings(&self) -> Result<u64> {
        self.with_write(|c| {
            Ok(c.execute(
                "UPDATE recordings SET status = 'failed', error = 'interrupted: app closed while recording' WHERE status = 'recording'",
                [],
            )? as u64)
        })
    }

    // ---------- downloads ----------

    pub fn add_download(&self, item_type: &str, item_id: i64, title: &str, path: &str) -> Result<i64> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO downloads(item_type, item_id, title, path, status) VALUES (?1, ?2, ?3, ?4, 'queued')",
                params![item_type, item_id, title, path],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_download(
        &self,
        id: i64,
        status: &str,
        bytes_done: Option<i64>,
        bytes_total: Option<i64>,
        error: Option<&str>,
    ) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "UPDATE downloads SET status = ?2, bytes_done = COALESCE(?3, bytes_done), bytes_total = COALESCE(?4, bytes_total),
                 error = ?5, updated = CURRENT_TIMESTAMP WHERE id = ?1",
                params![id, status, bytes_done, bytes_total, error.map(app_core::redact::redact)],
            )?;
            Ok(())
        })
    }

    pub fn delete_download(&self, id: i64) -> Result<()> {
        self.with_write(|c| {
            if c.execute("DELETE FROM downloads WHERE id = ?1", params![id])? == 0 {
                return Err(DbError::NotFound);
            }
            Ok(())
        })
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!("SELECT {DL_COLS} FROM downloads ORDER BY id DESC LIMIT 500"))?;
            let it = st.query_map([], row_to_dl)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn get_download(&self, id: i64) -> Result<DownloadRecord> {
        self.with_read(|c| {
            c.query_row(&format!("SELECT {DL_COLS} FROM downloads WHERE id = ?1"), params![id], row_to_dl)
                .optional()?
                .ok_or(DbError::NotFound)
        })
    }

    /// Downloads that should (re)start: queued, or interrupted mid-transfer by an app exit.
    pub fn resumable_downloads(&self) -> Result<Vec<DownloadRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {DL_COLS} FROM downloads WHERE status IN ('queued', 'downloading') ORDER BY id"
            ))?;
            let it = st.query_map([], row_to_dl)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{ChannelInsert, PlaylistInsert};

    #[test]
    fn recordings_and_downloads() {
        let db = Db::open_in_memory().unwrap();
        let p = db
            .insert_playlist(&PlaylistInsert {
                r#type: "m3u".into(),
                name: "t".into(),
                base_url: "x".into(),
                user: None,
                pass: None,
                mac: None,
                ua: None,
            })
            .unwrap();
        db.upsert_channels(
            p,
            &[ChannelInsert {
                source_id: "s".into(),
                name: "ESPN".into(),
                group_title: None,
                logo: None,
                stream_url: "u".into(),
                tvg_id: None,
                tvg_name: None,
                catchup: false,
                catchup_days: 0,
                catchup_kind: None,
                catchup_source: None,
            }],
        )
        .unwrap();
        let ch = db.list_channels(p, None, 1, 0).unwrap()[0].id;
        let id = db.add_recording(ch, Some("Game"), 1000, 2000, 600, "/tmp/x.ts", "scheduled").unwrap();
        assert!(db.add_recording(ch, None, 2000, 1000, 0, "/tmp", "scheduled").is_err());
        assert_eq!(db.due_recordings(999).unwrap().len(), 0);
        assert_eq!(db.due_recordings(1000).unwrap().len(), 1);
        assert_eq!(db.due_recordings(2500).unwrap().len(), 1, "still inside the overrun");
        assert_eq!(db.due_recordings(2601).unwrap().len(), 0);
        db.set_recording_status(id, "recording", Some(1234), None).unwrap();
        let r = db.get_recording(id).unwrap();
        assert_eq!((r.status.as_str(), r.bytes, r.channel_name.as_str()), ("recording", 1234, "ESPN"));
        let id2 = db.add_recording(ch, None, 10, 20, 0, "/tmp/y.ts", "scheduled").unwrap();
        assert_eq!(db.expire_missed_recordings(100).unwrap(), 1);
        assert_eq!(db.get_recording(id2).unwrap().status, "failed");
        assert_eq!(db.list_recordings().unwrap().len(), 2);

        let d = db.add_download("vod", 1, "Movie", "/tmp/m.mkv").unwrap();
        db.update_download(d, "downloading", Some(500), Some(1000), None).unwrap();
        assert_eq!(db.resumable_downloads().unwrap().len(), 1);
        db.update_download(d, "failed", None, None, Some("http://h/movie/u/p/1.mkv 403")).unwrap();
        let rec = db.get_download(d).unwrap();
        assert_eq!(rec.bytes_done, 500);
        assert!(rec.error.unwrap().contains("***"));
        db.delete_download(d).unwrap();
        assert!(db.list_downloads().unwrap().is_empty());
    }
}
