//! EPG storage and queries. Programme times are unix seconds (INTEGER).
//!
//! * Programmes are keyed by (playlist_id, channel_tvg_id, start) — re-importing replaces.
//! * `epg_overrides` maps a channel to a manual tvg-id (the #1 support ticket, CLAUDE.md §7).
//! * The per-playlist offset (minutes) is applied at query time, never baked into rows, so the
//!   user can change it without re-importing.

use crate::{Db, DbError, Result, MAX_ROWS_PER_TX};
use rusqlite::{params, params_from_iter, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgrammeInsert {
    pub channel_tvg_id: String,
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub desc: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Programme {
    pub channel_tvg_id: String,
    pub start: i64,
    pub stop: i64,
    pub title: String,
    pub desc: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpgStats {
    pub programmes: i64,
    pub channels_with_epg: i64,
    pub min_start: Option<i64>,
    pub max_stop: Option<i64>,
    pub offset_min: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpgSource {
    pub id: i64,
    pub playlist_id: i64,
    pub url_redacted: String,
    pub enabled: bool,
    pub last_synced: Option<String>,
    pub last_error: Option<String>,
    pub programme_count: i64,
}

fn row_to_programme(r: &rusqlite::Row<'_>) -> rusqlite::Result<Programme> {
    Ok(Programme { channel_tvg_id: r.get(0)?, start: r.get(1)?, stop: r.get(2)?, title: r.get(3)?, desc: r.get(4)? })
}

impl Db {
    /// Insert/replace programmes in ≤5k-row transactions. Returns rows written.
    pub fn upsert_programmes(&self, playlist_id: i64, rows: &[ProgrammeInsert]) -> Result<u64> {
        let mut written = 0u64;
        for chunk in rows.chunks(MAX_ROWS_PER_TX) {
            self.with_write(|c| {
                let tx = c.transaction()?;
                {
                    let mut st = tx.prepare_cached(
                        "INSERT INTO epg_programmes(playlist_id, channel_tvg_id, start, stop, title, desc)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(playlist_id, channel_tvg_id, start) DO UPDATE SET
                           stop = excluded.stop, title = excluded.title, desc = excluded.desc",
                    )?;
                    for p in chunk {
                        if p.stop <= p.start || p.channel_tvg_id.is_empty() {
                            continue;
                        }
                        st.execute(params![playlist_id, p.channel_tvg_id, p.start, p.stop, p.title, p.desc])?;
                        written += 1;
                    }
                }
                tx.commit()?;
                Ok(())
            })?;
        }
        Ok(written)
    }

    /// Delete programmes that ended before `before` (unix). Keeps the table bounded.
    pub fn prune_programmes(&self, playlist_id: i64, before: i64) -> Result<u64> {
        self.with_write(|c| {
            let n = c.execute(
                "DELETE FROM epg_programmes WHERE playlist_id = ?1 AND stop < ?2",
                params![playlist_id, before],
            )?;
            Ok(n as u64)
        })
    }

    pub fn clear_programmes(&self, playlist_id: i64) -> Result<u64> {
        self.with_write(|c| {
            Ok(c.execute("DELETE FROM epg_programmes WHERE playlist_id = ?1", params![playlist_id])? as u64)
        })
    }

    /// Programmes overlapping [from, to] for the given tvg-ids (already offset-adjusted by the
    /// caller if needed — see [`Db::epg_offset_secs`]). Ordered by tvg-id then start.
    pub fn programmes_range(&self, playlist_id: i64, tvg_ids: &[&str], from: i64, to: i64) -> Result<Vec<Programme>> {
        if tvg_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<&str> = tvg_ids.iter().take(400).copied().collect();
        let placeholders = std::iter::repeat_n("?", ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT channel_tvg_id, start, stop, title, desc FROM epg_programmes
             WHERE playlist_id = ? AND stop > ? AND start < ? AND channel_tvg_id IN ({placeholders})
             ORDER BY channel_tvg_id, start"
        );
        self.with_read(|c| {
            let mut st = c.prepare_cached(&sql)?;
            let mut values: Vec<rusqlite::types::Value> = vec![playlist_id.into(), from.into(), to.into()];
            values.extend(ids.iter().map(|s| rusqlite::types::Value::Text((*s).to_string())));
            let it = st.query_map(params_from_iter(values), row_to_programme)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// (now, next) for one tvg-id at `now` (unix).
    pub fn now_next(&self, playlist_id: i64, tvg_id: &str, now: i64) -> Result<(Option<Programme>, Option<Programme>)> {
        self.with_read(|c| {
            let cur = c
                .query_row(
                    "SELECT channel_tvg_id, start, stop, title, desc FROM epg_programmes
                     WHERE playlist_id = ?1 AND channel_tvg_id = ?2 AND start <= ?3 AND stop > ?3
                     ORDER BY start DESC LIMIT 1",
                    params![playlist_id, tvg_id, now],
                    row_to_programme,
                )
                .optional()?;
            let next = c
                .query_row(
                    "SELECT channel_tvg_id, start, stop, title, desc FROM epg_programmes
                     WHERE playlist_id = ?1 AND channel_tvg_id = ?2 AND start > ?3
                     ORDER BY start ASC LIMIT 1",
                    params![playlist_id, tvg_id, now],
                    row_to_programme,
                )
                .optional()?;
            Ok((cur, next))
        })
    }

    pub fn epg_stats(&self, playlist_id: i64) -> Result<EpgStats> {
        self.with_read(|c| {
            let (programmes, channels_with_epg, min_start, max_stop): (i64, i64, Option<i64>, Option<i64>) = c.query_row(
                "SELECT COUNT(*), COUNT(DISTINCT channel_tvg_id), MIN(start), MAX(stop) FROM epg_programmes WHERE playlist_id = ?1",
                params![playlist_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
            let offset_min: i32 = c
                .query_row("SELECT epg_offset_min FROM playlists WHERE id = ?1", params![playlist_id], |r| r.get(0))
                .optional()?
                .unwrap_or(0);
            Ok(EpgStats { programmes, channels_with_epg, min_start, max_stop, offset_min })
        })
    }

    /// Distinct tvg-ids that have programmes (used to show which channels lack a guide).
    pub fn epg_channel_ids(&self, playlist_id: i64) -> Result<HashSet<String>> {
        self.with_read(|c| {
            let mut st =
                c.prepare_cached("SELECT DISTINCT channel_tvg_id FROM epg_programmes WHERE playlist_id = ?1")?;
            let it = st.query_map(params![playlist_id], |r| r.get::<_, String>(0))?;
            Ok(it.collect::<std::result::Result<HashSet<_>, _>>()?)
        })
    }

    // ---------- offset ----------

    pub fn epg_offset_min(&self, playlist_id: i64) -> Result<i32> {
        self.with_read(|c| {
            Ok(c.query_row("SELECT epg_offset_min FROM playlists WHERE id = ?1", params![playlist_id], |r| r.get(0))
                .optional()?
                .unwrap_or(0))
        })
    }

    pub fn set_epg_offset_min(&self, playlist_id: i64, minutes: i32) -> Result<()> {
        let minutes = minutes.clamp(-24 * 60, 24 * 60);
        self.with_write(|c| {
            c.execute("UPDATE playlists SET epg_offset_min = ?2 WHERE id = ?1", params![playlist_id, minutes])?;
            Ok(())
        })
    }

    // ---------- overrides ----------

    pub fn set_epg_override(&self, channel_id: i64, tvg_id: Option<&str>) -> Result<()> {
        self.with_write(|c| {
            match tvg_id.map(str::trim).filter(|s| !s.is_empty()) {
                Some(t) => {
                    c.execute(
                        "INSERT INTO epg_overrides(channel_id, tvg_id_manual) VALUES (?1, ?2)
                         ON CONFLICT(channel_id) DO UPDATE SET tvg_id_manual = excluded.tvg_id_manual",
                        params![channel_id, t],
                    )?;
                }
                None => {
                    c.execute("DELETE FROM epg_overrides WHERE channel_id = ?1", params![channel_id])?;
                }
            }
            Ok(())
        })
    }

    pub fn epg_override(&self, channel_id: i64) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row("SELECT tvg_id_manual FROM epg_overrides WHERE channel_id = ?1", params![channel_id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
        })
    }

    /// channel_id → manual tvg-id for a whole playlist (one query for the grid).
    pub fn epg_overrides_for_playlist(&self, playlist_id: i64) -> Result<Vec<(i64, String)>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT o.channel_id, o.tvg_id_manual FROM epg_overrides o JOIN channels ch ON ch.id = o.channel_id WHERE ch.playlist_id = ?1",
            )?;
            let it = st.query_map(params![playlist_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    // ---------- sources ----------

    pub fn add_epg_source(&self, playlist_id: i64, url: &str) -> Result<i64> {
        self.with_write(|c| {
            c.execute("INSERT INTO epg_sources(playlist_id, url) VALUES (?1, ?2)", params![playlist_id, url.trim()])?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn delete_epg_source(&self, id: i64) -> Result<()> {
        self.with_write(|c| {
            if c.execute("DELETE FROM epg_sources WHERE id = ?1", params![id])? == 0 {
                return Err(DbError::NotFound);
            }
            Ok(())
        })
    }

    /// Raw URL (may contain credentials) — adapters only.
    pub fn epg_source_url(&self, id: i64) -> Result<(i64, String)> {
        self.with_read(|c| {
            c.query_row("SELECT playlist_id, url FROM epg_sources WHERE id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or(DbError::NotFound)
        })
    }

    pub fn list_epg_sources(&self, playlist_id: i64) -> Result<Vec<EpgSource>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT id, playlist_id, url, enabled, last_synced, last_error, programme_count FROM epg_sources WHERE playlist_id = ?1 ORDER BY id",
            )?;
            let it = st.query_map(params![playlist_id], |r| {
                Ok(EpgSource {
                    id: r.get(0)?,
                    playlist_id: r.get(1)?,
                    url_redacted: app_core::redact::redact(&r.get::<_, String>(2)?),
                    enabled: r.get::<_, i64>(3)? != 0,
                    last_synced: r.get(4)?,
                    last_error: r.get(5)?,
                    programme_count: r.get(6)?,
                })
            })?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// All enabled source ids+urls for a playlist (adapters only).
    pub fn enabled_epg_sources(&self, playlist_id: i64) -> Result<Vec<(i64, String)>> {
        self.with_read(|c| {
            let mut st =
                c.prepare_cached("SELECT id, url FROM epg_sources WHERE playlist_id = ?1 AND enabled = 1 ORDER BY id")?;
            let it = st.query_map(params![playlist_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn mark_epg_source(&self, id: i64, programme_count: i64, error: Option<&str>) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "UPDATE epg_sources SET last_synced = CURRENT_TIMESTAMP, last_error = ?2, programme_count = ?3 WHERE id = ?1",
                params![id, error.map(app_core::redact::redact), programme_count],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::PlaylistInsert;

    fn pl(db: &Db) -> i64 {
        db.insert_playlist(&PlaylistInsert {
            r#type: "m3u".into(),
            name: "t".into(),
            base_url: "x".into(),
            user: None,
            pass: None,
            mac: None,
            ua: None,
        })
        .unwrap()
    }

    fn prog(id: &str, start: i64, stop: i64, title: &str) -> ProgrammeInsert {
        ProgrammeInsert { channel_tvg_id: id.into(), start, stop, title: title.into(), desc: None }
    }

    #[test]
    fn upsert_query_now_next_prune() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        let rows = vec![
            prog("a", 0, 3600, "A1"),
            prog("a", 3600, 7200, "A2"),
            prog("a", 7200, 10800, "A3"),
            prog("b", 1800, 5400, "B1"),
            prog("bad", 100, 50, "inverted"),
        ];
        assert_eq!(db.upsert_programmes(p, &rows).unwrap(), 4);
        let (now, next) = db.now_next(p, "a", 4000).unwrap();
        assert_eq!(now.unwrap().title, "A2");
        assert_eq!(next.unwrap().title, "A3");
        let r = db.programmes_range(p, &["a", "b"], 3000, 4000).unwrap();
        assert_eq!(r.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["A1", "A2", "B1"], "overlap semantics");
        // replace on same (tvg, start)
        db.upsert_programmes(p, &[prog("a", 3600, 7200, "A2 renamed")]).unwrap();
        assert_eq!(db.now_next(p, "a", 4000).unwrap().0.unwrap().title, "A2 renamed");
        let st = db.epg_stats(p).unwrap();
        assert_eq!((st.programmes, st.channels_with_epg), (4, 2));
        assert_eq!(db.prune_programmes(p, 4000).unwrap(), 1);
        assert!(db.epg_channel_ids(p).unwrap().contains("b"));
        db.set_epg_offset_min(p, 90).unwrap();
        assert_eq!(db.epg_offset_min(p).unwrap(), 90);
    }

    #[test]
    fn overrides_and_sources() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        db.upsert_channels(
            p,
            &[crate::channels::ChannelInsert {
                source_id: "s".into(),
                name: "n".into(),
                group_title: None,
                logo: None,
                stream_url: "u".into(),
                tvg_id: Some("orig".into()),
                tvg_name: None,
                catchup: false,
                catchup_days: 0,
            }],
        )
        .unwrap();
        let ch = db.list_channels(p, None, 1, 0).unwrap()[0].id;
        db.set_epg_override(ch, Some("manual.id")).unwrap();
        assert_eq!(db.epg_override(ch).unwrap().as_deref(), Some("manual.id"));
        assert_eq!(db.epg_overrides_for_playlist(p).unwrap(), vec![(ch, "manual.id".to_string())]);
        db.set_epg_override(ch, None).unwrap();
        assert_eq!(db.epg_override(ch).unwrap(), None);
        let sid = db.add_epg_source(p, "http://h/xmltv.php?username=u&password=p").unwrap();
        db.mark_epg_source(sid, 12, Some("boom password=p")).unwrap();
        let srcs = db.list_epg_sources(p).unwrap();
        assert!(srcs[0].url_redacted.contains("password=***"));
        assert!(srcs[0].last_error.as_deref().unwrap().contains("password=***"));
        assert_eq!(srcs[0].programme_count, 12);
        assert_eq!(db.enabled_epg_sources(p).unwrap().len(), 1);
    }
}
