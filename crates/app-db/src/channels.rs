//! Playlists + channels CRUD, chunked upsert, paged listing, groups, favorites, recents.

use crate::normalize::normalize_name;
use crate::{Db, DbError, Result, MAX_ROWS_PER_TX};
use app_core::{ChannelRecord, GroupSummary, PlaylistSummary};
use rusqlite::{params, Connection, OptionalExtension};

/// A channel as produced by a parser/adapter — no id yet, name not yet normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInsert {
    pub source_id: String,
    pub name: String,
    pub group_title: Option<String>,
    pub logo: Option<String>,
    pub stream_url: String,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub catchup: bool,
    pub catchup_days: i32,
}

/// UI-safe playlist metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaylistMeta {
    pub id: i64,
    pub r#type: String,
    pub epg_offset_min: i32,
    pub stream_format: String,
    pub account_json: Option<String>,
    pub last_synced: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PlaylistInsert {
    pub r#type: String, // 'm3u' | 'xtream' | 'stalker'
    pub name: String,
    pub base_url: String,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub mac: Option<String>,
    pub ua: Option<String>,
}

const CHANNEL_COLS: &str =
    r#"id, playlist_id, source_id, name, normalized_name, "group", logo, stream_url, tvg_id, catchup_days"#;
/// Same columns qualified with the `ch` alias for joins.
const CHANNEL_COLS_CH: &str = r#"ch.id, ch.playlist_id, ch.source_id, ch.name, ch.normalized_name, ch."group", ch.logo, ch.stream_url, ch.tvg_id, ch.catchup_days"#;

fn row_to_channel(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChannelRecord> {
    Ok(ChannelRecord {
        id: r.get(0)?,
        playlist_id: r.get(1)?,
        source_id: r.get(2)?,
        name: r.get(3)?,
        normalized_name: r.get(4)?,
        group_title: r.get(5)?,
        logo: r.get(6)?,
        stream_url: r.get(7)?,
        tvg_id: r.get(8)?,
        catchup_days: r.get(9)?,
    })
}

impl Db {
    // ---------- playlists ----------

    pub fn insert_playlist(&self, p: &PlaylistInsert) -> Result<i64> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO playlists(type, name, base_url, user, pass, mac, ua) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![p.r#type, p.name, p.base_url, p.user, p.pass, p.mac, p.ua],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        self.with_write(|c| {
            let n = c.execute("DELETE FROM playlists WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(DbError::NotFound);
            }
            Ok(())
        })
    }

    pub fn list_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        self.with_read(|c| {
            let mut st = c.prepare(
                "SELECT p.id, p.type, p.name, p.base_url, p.created,
                        (SELECT COUNT(*) FROM channels ch WHERE ch.playlist_id = p.id)
                 FROM playlists p ORDER BY p.id",
            )?;
            let rows = st.query_map([], |r| {
                Ok(PlaylistSummary {
                    id: r.get(0)?,
                    r#type: r.get(1)?,
                    name: r.get(2)?,
                    base_url_redacted: app_core::redact::redact_url(&r.get::<_, String>(3)?),
                    created: r.get(4)?,
                    channel_count: r.get(5)?,
                })
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Per-playlist metadata safe for the UI (no secrets).
    pub fn playlist_meta(&self, id: i64) -> Result<PlaylistMeta> {
        self.with_read(|c| {
            c.query_row(
                "SELECT id, type, epg_offset_min, stream_format, account_json, last_synced, last_error FROM playlists WHERE id = ?1",
                params![id],
                |r| {
                    Ok(PlaylistMeta {
                        id: r.get(0)?,
                        r#type: r.get(1)?,
                        epg_offset_min: r.get(2)?,
                        stream_format: r.get(3)?,
                        account_json: r.get(4)?,
                        last_synced: r.get(5)?,
                        last_error: r.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or(DbError::NotFound)
        })
    }

    /// Xtream account info (status, expiry, max connections…) — never the password.
    pub fn set_playlist_account_json(&self, id: i64, json: Option<&str>) -> Result<()> {
        self.with_write(|c| {
            c.execute("UPDATE playlists SET account_json = ?2 WHERE id = ?1", params![id, json])?;
            Ok(())
        })
    }

    pub fn set_playlist_sync_result(&self, id: i64, error: Option<&str>) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "UPDATE playlists SET last_synced = CURRENT_TIMESTAMP, last_error = ?2 WHERE id = ?1",
                params![id, error.map(app_core::redact::redact)],
            )?;
            Ok(())
        })
    }

    pub fn set_playlist_stream_format(&self, id: i64, fmt: &str) -> Result<()> {
        let fmt = if fmt == "m3u8" { "m3u8" } else { "ts" };
        self.with_write(|c| {
            c.execute("UPDATE playlists SET stream_format = ?2 WHERE id = ?1", params![id, fmt])?;
            Ok(())
        })
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> Result<()> {
        self.with_write(|c| {
            c.execute("UPDATE playlists SET name = ?2 WHERE id = ?1", params![id, name.trim()])?;
            Ok(())
        })
    }

    /// Bump the sync generation for a playlist before an upsert pass, then remove rows the pass
    /// did not touch with [`Db::delete_stale_channels`].
    pub fn next_sync_gen(&self, playlist_id: i64) -> Result<i64> {
        self.with_read(|c| {
            let g: i64 = c.query_row(
                "SELECT COALESCE(MAX(sync_gen), 0) + 1 FROM channels WHERE playlist_id = ?1",
                params![playlist_id],
                |r| r.get(0),
            )?;
            Ok(g)
        })
    }

    pub fn delete_stale_channels(&self, playlist_id: i64, keep_gen: i64) -> Result<u64> {
        self.with_write(|c| {
            Ok(c.execute(
                "DELETE FROM channels WHERE playlist_id = ?1 AND sync_gen < ?2",
                params![playlist_id, keep_gen],
            )? as u64)
        })
    }

    /// Raw playlist row (with secrets) — for adapters only. Never send to the UI.
    pub fn playlist_source(&self, id: i64) -> Result<PlaylistInsert> {
        self.with_read(|c| {
            c.query_row(
                "SELECT type, name, base_url, user, pass, mac, ua FROM playlists WHERE id = ?1",
                params![id],
                |r| {
                    Ok(PlaylistInsert {
                        r#type: r.get(0)?,
                        name: r.get(1)?,
                        base_url: r.get(2)?,
                        user: r.get(3)?,
                        pass: r.get(4)?,
                        mac: r.get(5)?,
                        ua: r.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or(DbError::NotFound)
        })
    }

    // ---------- channels ----------

    /// Upsert a batch of channels. Chunks into transactions of at most 5,000 rows.
    /// Returns (inserted, updated).
    pub fn upsert_channels(&self, playlist_id: i64, rows: &[ChannelInsert]) -> Result<(u64, u64)> {
        self.upsert_channels_gen(playlist_id, rows, None)
    }

    /// Same as [`Db::upsert_channels`] but stamps `sync_gen` so untouched rows can be removed
    /// afterwards with [`Db::delete_stale_channels`].
    pub fn upsert_channels_gen(
        &self,
        playlist_id: i64,
        rows: &[ChannelInsert],
        sync_gen: Option<i64>,
    ) -> Result<(u64, u64)> {
        let mut inserted = 0u64;
        let mut updated = 0u64;
        for chunk in rows.chunks(MAX_ROWS_PER_TX) {
            let (i, u) = self.with_write(|c| upsert_chunk(c, playlist_id, chunk, sync_gen))?;
            inserted += i;
            updated += u;
        }
        Ok((inserted, updated))
    }

    pub fn channel_count(&self, playlist_id: i64, group_title: Option<&str>) -> Result<i64> {
        let hidden = self.hidden_clause(&["name", "\"group\""]);
        self.with_read(|c| {
            let n: i64 = match group_title {
                Some(g) => c.query_row(
                    &format!(r#"SELECT COUNT(*) FROM channels WHERE playlist_id = ?1 AND "group" = ?2{hidden}"#),
                    params![playlist_id, g],
                    |r| r.get(0),
                )?,
                None => c.query_row(
                    &format!("SELECT COUNT(*) FROM channels WHERE playlist_id = ?1{hidden}"),
                    params![playlist_id],
                    |r| r.get(0),
                )?,
            };
            Ok(n)
        })
    }

    /// Paged listing in playlist order (id ASC). Used by the virtualized Live list.
    pub fn list_channels(
        &self,
        playlist_id: i64,
        group_title: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChannelRecord>> {
        let limit = limit.clamp(1, 2_000) as i64;
        let offset = offset as i64;
        let hidden = self.hidden_clause(&["name", "\"group\""]);
        self.with_read(|c| {
            let rows = match group_title {
                Some(g) => {
                    let mut st = c.prepare_cached(&format!(
                        r#"SELECT {CHANNEL_COLS} FROM channels WHERE playlist_id = ?1 AND "group" = ?2{hidden} ORDER BY id LIMIT ?3 OFFSET ?4"#
                    ))?;
                    let it = st.query_map(params![playlist_id, g, limit, offset], row_to_channel)?;
                    it.collect::<std::result::Result<Vec<_>, _>>()?
                }
                None => {
                    let mut st = c.prepare_cached(&format!(
                        "SELECT {CHANNEL_COLS} FROM channels WHERE playlist_id = ?1{hidden} ORDER BY id LIMIT ?2 OFFSET ?3"
                    ))?;
                    let it = st.query_map(params![playlist_id, limit, offset], row_to_channel)?;
                    it.collect::<std::result::Result<Vec<_>, _>>()?
                }
            };
            Ok(rows)
        })
    }

    /// Row id of the channel with this per-playlist `source_id`, if it exists.
    pub fn channel_by_source_id(&self, playlist_id: i64, source_id: &str) -> Result<Option<i64>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT id FROM channels WHERE playlist_id = ?1 AND source_id = ?2",
                params![playlist_id, source_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?)
        })
    }

    pub fn get_channel(&self, id: i64) -> Result<ChannelRecord> {
        self.with_read(|c| {
            c.query_row(&format!("SELECT {CHANNEL_COLS} FROM channels WHERE id = ?1"), params![id], row_to_channel)
                .optional()?
                .ok_or(DbError::NotFound)
        })
    }

    pub fn list_groups(&self, playlist_id: i64) -> Result<Vec<GroupSummary>> {
        let hidden = self.hidden_clause(&["name", "\"group\""]);
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                r#"SELECT COALESCE("group", ''), COUNT(*) FROM channels WHERE playlist_id = ?1{hidden}
                   GROUP BY "group" ORDER BY MIN(id)"#
            ))?;
            let rows = st.query_map(params![playlist_id], |r| {
                Ok(GroupSummary { group_title: r.get(0)?, channel_count: r.get(1)? })
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    // ---------- favorites / recents ----------

    pub fn set_favorite(&self, item_type: &str, item_id: i64, on: bool) -> Result<()> {
        self.with_write(|c| {
            if on {
                c.execute(
                    "INSERT OR IGNORE INTO favorites(user_scope, item_type, item_id) VALUES ('default', ?1, ?2)",
                    params![item_type, item_id],
                )?;
            } else {
                c.execute(
                    "DELETE FROM favorites WHERE user_scope = 'default' AND item_type = ?1 AND item_id = ?2",
                    params![item_type, item_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn favorite_channels(&self) -> Result<Vec<ChannelRecord>> {
        self.with_read(|c| {
            let hidden = self.hidden_clause(&["name", "\"group\""]);
            let mut st = c.prepare_cached(&format!(
                "SELECT {CHANNEL_COLS} FROM channels WHERE id IN
                 (SELECT item_id FROM favorites WHERE user_scope = 'default' AND item_type = 'channel'){hidden} ORDER BY id"
            ))?;
            let it = st.query_map([], row_to_channel)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn favorite_ids(&self) -> Result<Vec<i64>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT item_id FROM favorites WHERE user_scope = 'default' AND item_type = 'channel' ORDER BY item_id",
            )?;
            let it = st.query_map([], |r| r.get::<_, i64>(0))?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn touch_recent(&self, channel_id: i64) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO recents(channel_id, viewed_at) VALUES (?1, CURRENT_TIMESTAMP)
                 ON CONFLICT(channel_id) DO UPDATE SET viewed_at = CURRENT_TIMESTAMP",
                params![channel_id],
            )?;
            // keep the table small
            c.execute(
                "DELETE FROM recents WHERE channel_id NOT IN (SELECT channel_id FROM recents ORDER BY viewed_at DESC LIMIT 50)",
                [],
            )?;
            Ok(())
        })
    }

    pub fn recent_channels(&self, limit: usize) -> Result<Vec<ChannelRecord>> {
        self.with_read(|c| {
            let hidden = self.hidden_clause(&["ch.name", "ch.\"group\""]);
            let mut st = c.prepare_cached(&format!(
                "SELECT {CHANNEL_COLS_CH} FROM channels ch JOIN recents r ON r.channel_id = ch.id WHERE 1=1{hidden} ORDER BY r.viewed_at DESC LIMIT ?1"
            ))?;
            let it = st.query_map(params![limit as i64], row_to_channel)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }
}

fn upsert_chunk(
    c: &mut Connection,
    playlist_id: i64,
    chunk: &[ChannelInsert],
    sync_gen: Option<i64>,
) -> Result<(u64, u64)> {
    let tx = c.transaction()?;
    let before: i64 =
        tx.query_row("SELECT COUNT(*) FROM channels WHERE playlist_id = ?1", params![playlist_id], |r| r.get(0))?;
    {
        let mut st = tx.prepare_cached(
            r#"INSERT INTO channels(playlist_id, source_id, name, normalized_name, "group", logo, stream_url, tvg_id, tvg_name, catchup, catchup_days, sync_gen)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, COALESCE(?12, 0))
               ON CONFLICT(playlist_id, source_id) DO UPDATE SET
                 name = excluded.name,
                 normalized_name = excluded.normalized_name,
                 "group" = excluded."group",
                 logo = excluded.logo,
                 stream_url = excluded.stream_url,
                 tvg_id = excluded.tvg_id,
                 tvg_name = excluded.tvg_name,
                 catchup = excluded.catchup,
                 catchup_days = excluded.catchup_days
               WHERE channels.name IS NOT excluded.name
                  OR channels."group" IS NOT excluded."group"
                  OR channels.logo IS NOT excluded.logo
                  OR channels.stream_url IS NOT excluded.stream_url
                  OR channels.tvg_id IS NOT excluded.tvg_id
                  OR channels.tvg_name IS NOT excluded.tvg_name
                  OR channels.catchup IS NOT excluded.catchup
                  OR channels.catchup_days IS NOT excluded.catchup_days"#,
        )?;
        for row in chunk {
            let normalized = normalize_name(&row.name);
            st.execute(params![
                playlist_id,
                row.source_id,
                row.name,
                normalized,
                row.group_title,
                row.logo,
                row.stream_url,
                row.tvg_id,
                row.tvg_name,
                row.catchup as i32,
                row.catchup_days,
                sync_gen,
            ])?;
        }
        if let Some(g) = sync_gen {
            // Stamp the generation separately so unchanged rows are not re-indexed by the FTS
            // trigger (which fires on any UPDATE that mentions the indexed columns).
            let mut stamp = tx.prepare_cached(
                "UPDATE channels SET sync_gen = ?3 WHERE playlist_id = ?1 AND source_id = ?2 AND sync_gen <> ?3",
            )?;
            for row in chunk {
                stamp.execute(params![playlist_id, row.source_id, g])?;
            }
        }
    }
    let after: i64 =
        tx.query_row("SELECT COUNT(*) FROM channels WHERE playlist_id = ?1", params![playlist_id], |r| r.get(0))?;
    tx.commit()?;
    let inserted = (after - before).max(0) as u64;
    let updated = (chunk.len() as u64).saturating_sub(inserted);
    Ok((inserted, updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pl(db: &Db) -> i64 {
        db.insert_playlist(&PlaylistInsert {
            r#type: "m3u".into(),
            name: "test".into(),
            base_url: "http://h/get.php?username=u&password=p".into(),
            user: None,
            pass: None,
            mac: None,
            ua: None,
        })
        .unwrap()
    }

    fn ch(i: usize, group: &str) -> ChannelInsert {
        ChannelInsert {
            source_id: format!("src-{i}"),
            name: format!("Chänñel {i}"),
            group_title: Some(group.into()),
            logo: None,
            stream_url: format!("http://h/live/u/p/{i}.ts"),
            tvg_id: Some(format!("tvg{i}")),
            tvg_name: None,
            catchup: false,
            catchup_days: 0,
        }
    }

    #[test]
    fn upsert_paging_groups() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        let rows: Vec<_> = (0..12_000).map(|i| ch(i, if i % 2 == 0 { "News" } else { "Sports" })).collect();
        let (ins, upd) = db.upsert_channels(p, &rows).unwrap();
        assert_eq!((ins, upd), (12_000, 0));
        let (ins, upd) = db.upsert_channels(p, &rows[..10]).unwrap();
        assert_eq!((ins, upd), (0, 10));
        assert_eq!(db.channel_count(p, None).unwrap(), 12_000);
        assert_eq!(db.channel_count(p, Some("News")).unwrap(), 6_000);

        let page = db.list_channels(p, None, 100, 200).unwrap();
        assert_eq!(page.len(), 100);
        assert_eq!(page[0].name, "Chänñel 200");
        assert_eq!(page[0].normalized_name, "channel 200");

        let sports = db.list_channels(p, Some("Sports"), 3, 0).unwrap();
        assert_eq!(sports.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Chänñel 1", "Chänñel 3", "Chänñel 5"]);

        let groups = db.list_groups(p).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group_title, "News");
        assert_eq!(groups[0].channel_count, 6_000);

        let pls = db.list_playlists().unwrap();
        assert_eq!(pls[0].channel_count, 12_000);
        assert!(pls[0].base_url_redacted.contains("password=***"));
    }

    #[test]
    fn hidden_keywords_filter_everything() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        let mut rows = vec![ch(1, "News"), ch(2, "Adult XXX"), ch(3, "Sports")];
        rows[2].name = "Late Night xXx Show".into();
        db.upsert_channels(p, &rows).unwrap();
        assert_eq!(db.channel_count(p, None).unwrap(), 3);
        db.set_hidden_keywords(&["xxx".into(), "  ".into(), "O'Reilly".into()]);
        assert_eq!(db.channel_count(p, None).unwrap(), 1);
        assert_eq!(db.list_channels(p, None, 10, 0).unwrap()[0].name, "Chänñel 1");
        assert_eq!(db.list_groups(p).unwrap().len(), 1);
        assert_eq!(db.search_count("channel", p).unwrap(), 1);
        db.set_hidden_keywords(&[]);
        assert_eq!(db.channel_count(p, None).unwrap(), 3);
    }

    #[test]
    fn sync_gen_removes_stale_rows() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        db.upsert_channels(p, &[ch(1, "g"), ch(2, "g"), ch(3, "g")]).unwrap();
        let g = db.next_sync_gen(p).unwrap();
        db.upsert_channels_gen(p, &[ch(1, "g"), ch(3, "g")], Some(g)).unwrap();
        assert_eq!(db.delete_stale_channels(p, g).unwrap(), 1);
        let names: Vec<_> = db.list_channels(p, None, 10, 0).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Chänñel 1", "Chänñel 3"]);
        assert_eq!(db.search_count("channel 2", p).unwrap(), 0, "FTS row removed too");
    }

    #[test]
    fn favorites_recents_cascade() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        db.upsert_channels(p, &[ch(1, "g"), ch(2, "g")]).unwrap();
        let first = db.list_channels(p, None, 1, 0).unwrap()[0].id;
        db.set_favorite("channel", first, true).unwrap();
        assert_eq!(db.favorite_ids().unwrap(), vec![first]);
        db.touch_recent(first).unwrap();
        assert_eq!(db.recent_channels(10).unwrap().len(), 1);
        db.delete_playlist(p).unwrap();
        assert_eq!(db.recent_channels(10).unwrap().len(), 0, "cascade");
        assert!(matches!(db.get_channel(first), Err(DbError::NotFound)));
    }
}
