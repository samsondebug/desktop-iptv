//! Channel curation: per-channel rename & hide, per-group hide. Overrides live in their own
//! tables (`channel_overrides`, `group_overrides`) so playlist refreshes never wipe them; list
//! queries apply them via [`curation_clause`] and the COALESCE in the channel column sets.

use crate::normalize::normalize_name;
use crate::{Db, Result};
use rusqlite::params;
use serde::Serialize;

/// SQL fragment (starting with " AND ") that excludes hidden channels and channels in hidden
/// groups. `p` is the qualified prefix of the channels table in the outer query — `"channels."`
/// or an alias like `"ch."` — required so the correlated subquery resolves the outer columns.
pub fn curation_clause(p: &str) -> String {
    format!(
        " AND {p}id NOT IN (SELECT channel_id FROM channel_overrides WHERE hidden = 1) \
          AND NOT EXISTS (SELECT 1 FROM group_overrides go WHERE go.playlist_id = {p}playlist_id AND go.\"group\" = {p}\"group\" AND go.hidden = 1)"
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct HiddenChannel {
    pub channel_id: i64,
    pub name: String,
    pub playlist_id: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HiddenGroup {
    pub playlist_id: i64,
    pub group_title: String,
    pub channel_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CurationState {
    pub hidden_channels: Vec<HiddenChannel>,
    pub hidden_groups: Vec<HiddenGroup>,
    pub renamed_count: i64,
}

impl Db {
    /// Set (Some) or clear (None/empty) a channel's display name. The search index row is
    /// re-written so search matches the new name (until the next source change re-indexes it).
    pub fn set_channel_name(&self, channel_id: i64, custom_name: Option<&str>) -> Result<()> {
        let custom = custom_name.map(str::trim).filter(|s| !s.is_empty());
        self.with_write(|c| {
            match custom {
                Some(n) => c.execute(
                    "INSERT INTO channel_overrides(channel_id, custom_name) VALUES (?1, ?2)
                     ON CONFLICT(channel_id) DO UPDATE SET custom_name = excluded.custom_name",
                    params![channel_id, n],
                )?,
                None => c.execute(
                    "UPDATE channel_overrides SET custom_name = NULL WHERE channel_id = ?1",
                    params![channel_id],
                )?,
            };
            c.execute(
                "DELETE FROM channel_overrides WHERE channel_id = ?1 AND custom_name IS NULL AND hidden = 0",
                params![channel_id],
            )?;
            // Keep FTS in step with what the UI shows.
            let row = c
                .query_row(
                    r#"SELECT normalized_name, COALESCE("group", ''), playlist_id FROM channels WHERE id = ?1"#,
                    params![channel_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)),
                )
                .ok();
            if let Some((orig_norm, group, playlist_id)) = row {
                let indexed = custom.map(normalize_name).unwrap_or(orig_norm);
                c.execute("DELETE FROM search_idx WHERE content_type = 'channel' AND item_id = ?1", params![channel_id])?;
                c.execute(
                    "INSERT INTO search_idx(name, group_name, content_type, item_id, playlist_id) VALUES (?1, ?2, 'channel', ?3, ?4)",
                    params![indexed, group, channel_id, playlist_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn set_channel_hidden(&self, channel_id: i64, hidden: bool) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO channel_overrides(channel_id, hidden) VALUES (?1, ?2)
                 ON CONFLICT(channel_id) DO UPDATE SET hidden = excluded.hidden",
                params![channel_id, hidden as i64],
            )?;
            c.execute(
                "DELETE FROM channel_overrides WHERE channel_id = ?1 AND custom_name IS NULL AND hidden = 0",
                params![channel_id],
            )?;
            Ok(())
        })
    }

    pub fn set_group_hidden(&self, playlist_id: i64, group_title: &str, hidden: bool) -> Result<()> {
        self.with_write(|c| {
            if hidden {
                c.execute(
                    r#"INSERT INTO group_overrides(playlist_id, "group", hidden) VALUES (?1, ?2, 1)
                       ON CONFLICT(playlist_id, "group") DO UPDATE SET hidden = 1"#,
                    params![playlist_id, group_title],
                )?;
            } else {
                c.execute(
                    r#"DELETE FROM group_overrides WHERE playlist_id = ?1 AND "group" = ?2"#,
                    params![playlist_id, group_title],
                )?;
            }
            Ok(())
        })
    }

    /// Everything currently hidden or renamed (for the Settings management list).
    pub fn curation_state(&self) -> Result<CurationState> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT o.channel_id, COALESCE(o.custom_name, ch.name), ch.playlist_id
                 FROM channel_overrides o JOIN channels ch ON ch.id = o.channel_id
                 WHERE o.hidden = 1 ORDER BY ch.name",
            )?;
            let hidden_channels = st
                .query_map([], |r| {
                    Ok(HiddenChannel { channel_id: r.get(0)?, name: r.get(1)?, playlist_id: r.get(2)? })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut st = c.prepare_cached(
                r#"SELECT go.playlist_id, go."group",
                          (SELECT COUNT(*) FROM channels ch WHERE ch.playlist_id = go.playlist_id AND ch."group" = go."group")
                   FROM group_overrides go WHERE go.hidden = 1 ORDER BY go."group""#,
            )?;
            let hidden_groups = st
                .query_map([], |r| {
                    Ok(HiddenGroup { playlist_id: r.get(0)?, group_title: r.get(1)?, channel_count: r.get(2)? })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let renamed_count: i64 =
                c.query_row("SELECT COUNT(*) FROM channel_overrides WHERE custom_name IS NOT NULL", [], |r| r.get(0))?;
            Ok(CurationState { hidden_channels, hidden_groups, renamed_count })
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::channels::{ChannelInsert, PlaylistInsert};
    use crate::Db;

    fn seed() -> (Db, i64) {
        let db = Db::open_in_memory().unwrap();
        let p = db
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
        let mk = |sid: &str, name: &str, group: &str| ChannelInsert {
            source_id: sid.into(),
            name: name.into(),
            group_title: Some(group.into()),
            logo: None,
            stream_url: format!("http://x/{sid}.ts"),
            tvg_id: None,
            tvg_name: None,
            catchup: false,
            catchup_days: 0,
        };
        db.upsert_channels(p, &[mk("1", "BBC One", "UK"), mk("2", "Shopping TV", "Shop"), mk("3", "ITV", "UK")])
            .unwrap();
        (db, p)
    }

    #[test]
    fn rename_shows_and_searches() {
        let (db, p) = seed();
        let id = db.list_channels(p, None, 100, 0).unwrap()[0].id;
        db.set_channel_name(id, Some("Beeb")).unwrap();
        let rows = db.list_channels(p, None, 100, 0).unwrap();
        assert_eq!(rows.iter().find(|c| c.id == id).unwrap().name, "Beeb");
        assert_eq!(db.get_channel(id).unwrap().name, "Beeb");
        assert_eq!(db.search_channels("beeb", p, 10, 0).unwrap().len(), 1);
        // Clearing restores the source name (and its search entry).
        db.set_channel_name(id, None).unwrap();
        assert_eq!(db.get_channel(id).unwrap().name, "BBC One");
        assert_eq!(db.search_channels("bbc", p, 10, 0).unwrap().len(), 1);
        assert_eq!(db.curation_state().unwrap().renamed_count, 0);
    }

    #[test]
    fn hide_channel_and_group() {
        let (db, p) = seed();
        let rows = db.list_channels(p, None, 100, 0).unwrap();
        assert_eq!(rows.len(), 3);
        let shopping = rows.iter().find(|c| c.name == "Shopping TV").unwrap().id;
        db.set_channel_hidden(shopping, true).unwrap();
        assert_eq!(db.list_channels(p, None, 100, 0).unwrap().len(), 2);
        assert_eq!(db.channel_count(p, None).unwrap(), 2);
        // Hidden group removes its channels and the group row.
        db.set_group_hidden(p, "UK", true).unwrap();
        assert_eq!(db.list_channels(p, None, 100, 0).unwrap().len(), 0);
        assert!(db.list_groups(p).unwrap().iter().all(|g| g.group_title != "UK"));
        assert_eq!(db.search_channels("itv", p, 10, 0).unwrap().len(), 0);
        let st = db.curation_state().unwrap();
        assert_eq!(st.hidden_channels.len(), 1);
        assert_eq!(st.hidden_groups.len(), 1);
        // Unhide both.
        db.set_group_hidden(p, "UK", false).unwrap();
        db.set_channel_hidden(shopping, false).unwrap();
        assert_eq!(db.list_channels(p, None, 100, 0).unwrap().len(), 3);
    }
}
