//! FTS5 global search across live + VOD (CLAUDE.md §5, §8).
//!
//! User text is normalized the same way names were at index time, then turned into a
//! prefix query: `"tok1"* "tok2"*`. All tokens must match (implicit AND). Quoting each
//! token means user-typed operators/punctuation can never break the MATCH expression.

use crate::normalize::normalize_name;
use crate::{Db, Result};
use app_core::ChannelRecord;
use rusqlite::params;

/// Build a safe FTS5 MATCH expression from free text. Returns `None` for an empty query.
pub fn build_match(query: &str) -> Option<String> {
    let norm = normalize_name(query);
    let mut parts = Vec::new();
    for tok in norm.split_whitespace() {
        let escaped = tok.replace('"', "\"\"");
        parts.push(format!("\"{escaped}\"*"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

impl Db {
    /// Search channels in one playlist (or all playlists when `playlist_id == 0`).
    pub fn search_channels(
        &self,
        query: &str,
        playlist_id: i64,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ChannelRecord>> {
        let Some(m) = build_match(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.clamp(1, 1_000) as i64;
        let hidden = self.hidden_clause(&["ch.name", "ch.\"group\""]);
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                r#"SELECT ch.id, ch.playlist_id, ch.source_id, ch.name, ch.normalized_name, ch."group", ch.logo,
                          ch.stream_url, ch.tvg_id, ch.catchup_days
                   FROM search_idx s
                   JOIN channels ch ON ch.id = s.item_id
                   WHERE search_idx MATCH ?1
                     AND s.content_type = 'channel'
                     AND (?2 = 0 OR s.playlist_id = ?2){hidden}
                   ORDER BY bm25(search_idx, 10.0, 1.0), ch.id
                   LIMIT ?3 OFFSET ?4"#
            ))?;
            let it = st.query_map(params![m, playlist_id, limit, offset as i64], |r| {
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
            })?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn search_count(&self, query: &str, playlist_id: i64) -> Result<i64> {
        let Some(m) = build_match(query) else {
            return Ok(0);
        };
        let hidden = self.hidden_clause(&["ch.name", "ch.\"group\""]);
        self.with_read(|c| {
            Ok(c.query_row(
                &format!("SELECT COUNT(*) FROM search_idx s JOIN channels ch ON ch.id = s.item_id WHERE search_idx MATCH ?1 AND s.content_type = 'channel' AND (?2 = 0 OR s.playlist_id = ?2){hidden}"),
                params![m, playlist_id],
                |r| r.get(0),
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{ChannelInsert, PlaylistInsert};

    #[test]
    fn match_builder() {
        assert_eq!(build_match("  "), None);
        assert_eq!(build_match("Pokémon"), Some("\"pokemon\"*".into()));
        assert_eq!(build_match("us: espn \"OR\" NOT"), Some("\"us\"* \"espn\"* \"or\"* \"not\"*".into()));
    }

    #[test]
    fn searches_normalized_prefixes() {
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
        let mk = |id: &str, name: &str, group: &str| ChannelInsert {
            source_id: id.into(),
            name: name.into(),
            group_title: Some(group.into()),
            logo: None,
            stream_url: "http://x".into(),
            tvg_id: None,
            tvg_name: None,
            catchup: false,
            catchup_days: 0,
        };
        db.upsert_channels(
            p,
            &[
                mk("1", "US: ESPN HD", "Sports"),
                mk("2", "ESPN2", "Sports"),
                mk("3", "Télé Québec", "Canada"),
                mk("4", "BBC One", "UK"),
                mk("5", "Pokémon TV", "Kids"),
            ],
        )
        .unwrap();
        let r = db.search_channels("espn", p, 10, 0).unwrap();
        assert_eq!(r.len(), 2);
        let r = db.search_channels("tele que", p, 10, 0).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "Télé Québec");
        let r = db.search_channels("pokemon", 0, 10, 0).unwrap();
        assert_eq!(r.len(), 1);
        // group names are indexed too
        let r = db.search_channels("sports", p, 10, 0).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(db.search_count("bbc", p).unwrap(), 1);
        // update keeps FTS in sync
        db.upsert_channels(p, &[mk("4", "BBC Two", "UK")]).unwrap();
        let r = db.search_channels("bbc two", p, 10, 0).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(db.search_count("bbc one", p).unwrap(), 0);
    }
}
