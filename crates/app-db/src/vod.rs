//! Movies / series catalog (vod_items + episodes). Same rules as channels: chunked upserts,
//! normalized titles at write time, FTS via triggers, paged listing.

use crate::normalize::normalize_name;
use crate::{Db, DbError, Result, MAX_ROWS_PER_TX};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VodInsert {
    /// 'movie' | 'series'
    pub kind: String,
    pub source_id: String,
    pub title: String,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub rating: Option<f64>,
    pub genre: Option<String>,
    pub duration_s: Option<i64>,
    /// Movies: direct stream URL. Series: None (episodes carry URLs).
    pub stream_url: Option<String>,
    pub container_ext: Option<String>,
    pub added: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeInsert {
    pub source_id: Option<String>,
    pub season: i32,
    pub episode: i32,
    pub title: Option<String>,
    pub stream_url: String,
    pub duration: Option<i64>,
    pub poster: Option<String>,
    pub container_ext: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VodRecord {
    pub id: i64,
    pub playlist_id: i64,
    pub kind: String,
    pub source_id: String,
    pub title: String,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub year: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub category: Option<String>,
    pub description: Option<String>,
    pub rating: Option<f64>,
    pub genre: Option<String>,
    pub duration_s: Option<i64>,
    pub stream_url: Option<String>,
    pub container_ext: Option<String>,
    pub added: Option<i64>,
    pub episodes_synced: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodeRecord {
    pub id: i64,
    pub series_id: i64,
    pub source_id: Option<String>,
    pub season: i32,
    pub episode: i32,
    pub title: Option<String>,
    pub stream_url: String,
    pub duration: Option<i64>,
    pub poster: Option<String>,
    pub container_ext: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VodGroup {
    pub category: String,
    pub count: i64,
}

const VOD_COLS: &str = "id, playlist_id, kind, source_id, title, poster, backdrop, year, tmdb_id, category, description, rating, genre, duration_s, stream_url, container_ext, added, episodes_synced";

fn row_to_vod(r: &rusqlite::Row<'_>) -> rusqlite::Result<VodRecord> {
    Ok(VodRecord {
        id: r.get(0)?,
        playlist_id: r.get(1)?,
        kind: r.get(2)?,
        source_id: r.get(3)?,
        title: r.get(4)?,
        poster: r.get(5)?,
        backdrop: r.get(6)?,
        year: r.get(7)?,
        tmdb_id: r.get(8)?,
        category: r.get(9)?,
        description: r.get(10)?,
        rating: r.get(11)?,
        genre: r.get(12)?,
        duration_s: r.get(13)?,
        stream_url: r.get(14)?,
        container_ext: r.get(15)?,
        added: r.get(16)?,
        episodes_synced: r.get(17)?,
    })
}

const EP_COLS: &str = "id, series_id, source_id, season, episode, title, stream_url, duration, poster, container_ext";

fn row_to_episode(r: &rusqlite::Row<'_>) -> rusqlite::Result<EpisodeRecord> {
    Ok(EpisodeRecord {
        id: r.get(0)?,
        series_id: r.get(1)?,
        source_id: r.get(2)?,
        season: r.get(3)?,
        episode: r.get(4)?,
        title: r.get(5)?,
        stream_url: r.get(6)?,
        duration: r.get(7)?,
        poster: r.get(8)?,
        container_ext: r.get(9)?,
    })
}

impl Db {
    /// Upsert VOD items. Returns (inserted, touched).
    pub fn upsert_vod(&self, playlist_id: i64, rows: &[VodInsert]) -> Result<(u64, u64)> {
        let mut inserted = 0u64;
        let mut touched = 0u64;
        for chunk in rows.chunks(MAX_ROWS_PER_TX) {
            self.with_write(|c| {
                let tx = c.transaction()?;
                let before: i64 =
                    tx.query_row("SELECT COUNT(*) FROM vod_items WHERE playlist_id = ?1", params![playlist_id], |r| r.get(0))?;
                {
                    let mut st = tx.prepare_cached(
                        "INSERT INTO vod_items(playlist_id, kind, source_id, title, normalized_title, poster, backdrop, year, tmdb_id,
                                               category, description, rating, genre, duration_s, stream_url, container_ext, added)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                         ON CONFLICT(playlist_id, kind, source_id) DO UPDATE SET
                           title = excluded.title, normalized_title = excluded.normalized_title, poster = excluded.poster,
                           backdrop = excluded.backdrop, year = excluded.year, tmdb_id = excluded.tmdb_id,
                           category = excluded.category, description = excluded.description, rating = excluded.rating,
                           genre = excluded.genre, duration_s = excluded.duration_s, stream_url = excluded.stream_url,
                           container_ext = excluded.container_ext, added = excluded.added
                         WHERE vod_items.title IS NOT excluded.title OR vod_items.poster IS NOT excluded.poster
                            OR vod_items.category IS NOT excluded.category OR vod_items.stream_url IS NOT excluded.stream_url
                            OR vod_items.description IS NOT excluded.description OR vod_items.rating IS NOT excluded.rating
                            OR vod_items.year IS NOT excluded.year OR vod_items.added IS NOT excluded.added",
                    )?;
                    for v in chunk {
                        if v.kind != "movie" && v.kind != "series" {
                            continue;
                        }
                        st.execute(params![
                            playlist_id,
                            v.kind,
                            v.source_id,
                            v.title,
                            normalize_name(&v.title),
                            v.poster,
                            v.backdrop,
                            v.year,
                            v.tmdb_id,
                            v.category,
                            v.description,
                            v.rating,
                            v.genre,
                            v.duration_s,
                            v.stream_url,
                            v.container_ext,
                            v.added,
                        ])?;
                        touched += 1;
                    }
                }
                let after: i64 =
                    tx.query_row("SELECT COUNT(*) FROM vod_items WHERE playlist_id = ?1", params![playlist_id], |r| r.get(0))?;
                tx.commit()?;
                inserted += (after - before).max(0) as u64;
                Ok(())
            })?;
        }
        Ok((inserted, touched.saturating_sub(inserted)))
    }

    pub fn vod_count(&self, playlist_id: i64, kind: &str, category: Option<&str>) -> Result<i64> {
        self.with_read(|c| {
            Ok(match category {
                Some(g) => c.query_row(
                    "SELECT COUNT(*) FROM vod_items WHERE playlist_id = ?1 AND kind = ?2 AND category = ?3",
                    params![playlist_id, kind, g],
                    |r| r.get(0),
                )?,
                None => c.query_row(
                    "SELECT COUNT(*) FROM vod_items WHERE playlist_id = ?1 AND kind = ?2",
                    params![playlist_id, kind],
                    |r| r.get(0),
                )?,
            })
        })
    }

    /// Paged listing. `sort` = "added" (newest first, default) | "title" | "year" | "rating".
    pub fn list_vod(
        &self,
        playlist_id: i64,
        kind: &str,
        category: Option<&str>,
        sort: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<VodRecord>> {
        let order = match sort {
            "title" => "normalized_title ASC, id",
            "year" => "year DESC, id",
            "rating" => "rating DESC, id",
            _ => "COALESCE(added, 0) DESC, id DESC",
        };
        let limit = limit.clamp(1, 1_000) as i64;
        self.with_read(|c| {
            let rows = match category {
                Some(g) => {
                    let mut st = c.prepare_cached(&format!(
                        "SELECT {VOD_COLS} FROM vod_items WHERE playlist_id = ?1 AND kind = ?2 AND category = ?3 ORDER BY {order} LIMIT ?4 OFFSET ?5"
                    ))?;
                    let it = st.query_map(params![playlist_id, kind, g, limit, offset as i64], row_to_vod)?;
                    it.collect::<std::result::Result<Vec<_>, _>>()?
                }
                None => {
                    let mut st = c.prepare_cached(&format!(
                        "SELECT {VOD_COLS} FROM vod_items WHERE playlist_id = ?1 AND kind = ?2 ORDER BY {order} LIMIT ?3 OFFSET ?4"
                    ))?;
                    let it = st.query_map(params![playlist_id, kind, limit, offset as i64], row_to_vod)?;
                    it.collect::<std::result::Result<Vec<_>, _>>()?
                }
            };
            Ok(rows)
        })
    }

    pub fn vod_groups(&self, playlist_id: i64, kind: &str) -> Result<Vec<VodGroup>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(
                "SELECT COALESCE(category, ''), COUNT(*) FROM vod_items WHERE playlist_id = ?1 AND kind = ?2 GROUP BY category ORDER BY MIN(id)",
            )?;
            let it = st.query_map(params![playlist_id, kind], |r| Ok(VodGroup { category: r.get(0)?, count: r.get(1)? }))?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn get_vod(&self, id: i64) -> Result<VodRecord> {
        self.with_read(|c| {
            c.query_row(&format!("SELECT {VOD_COLS} FROM vod_items WHERE id = ?1"), params![id], row_to_vod)
                .optional()?
                .ok_or(DbError::NotFound)
        })
    }

    /// FTS over VOD titles (content_type = 'vod'), optionally restricted to one kind.
    pub fn search_vod(
        &self,
        query: &str,
        playlist_id: i64,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<VodRecord>> {
        let Some(m) = crate::search::build_match(query) else {
            return Ok(Vec::new());
        };
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {} FROM search_idx s JOIN vod_items v ON v.id = s.item_id
                 WHERE search_idx MATCH ?1 AND s.content_type = 'vod' AND (?2 = 0 OR s.playlist_id = ?2) AND (?3 IS NULL OR v.kind = ?3)
                 ORDER BY bm25(search_idx, 10.0, 1.0), v.id LIMIT ?4",
                VOD_COLS.split(", ").map(|c| format!("v.{c}")).collect::<Vec<_>>().join(", ")
            ))?;
            let it = st.query_map(params![m, playlist_id, kind, limit.clamp(1, 1_000) as i64], row_to_vod)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    // ---------- episodes ----------

    pub fn upsert_episodes(&self, series_id: i64, rows: &[EpisodeInsert]) -> Result<u64> {
        let mut n = 0u64;
        self.with_write(|c| {
            let tx = c.transaction()?;
            {
                let mut st = tx.prepare_cached(
                    "INSERT INTO episodes(series_id, source_id, season, episode, title, stream_url, duration, poster, container_ext)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(series_id, season, episode) DO UPDATE SET
                       source_id = excluded.source_id, title = excluded.title, stream_url = excluded.stream_url,
                       duration = excluded.duration, poster = excluded.poster, container_ext = excluded.container_ext",
                )?;
                for e in rows {
                    st.execute(params![
                        series_id, e.source_id, e.season, e.episode, e.title, e.stream_url, e.duration, e.poster, e.container_ext
                    ])?;
                    n += 1;
                }
            }
            tx.execute("UPDATE vod_items SET episodes_synced = CURRENT_TIMESTAMP WHERE id = ?1", params![series_id])?;
            tx.commit()?;
            Ok(())
        })?;
        Ok(n)
    }

    pub fn episodes(&self, series_id: i64) -> Result<Vec<EpisodeRecord>> {
        self.with_read(|c| {
            let mut st = c.prepare_cached(&format!(
                "SELECT {EP_COLS} FROM episodes WHERE series_id = ?1 ORDER BY season, episode"
            ))?;
            let it = st.query_map(params![series_id], row_to_episode)?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn get_episode(&self, id: i64) -> Result<EpisodeRecord> {
        self.with_read(|c| {
            c.query_row(&format!("SELECT {EP_COLS} FROM episodes WHERE id = ?1"), params![id], row_to_episode)
                .optional()?
                .ok_or(DbError::NotFound)
        })
    }

    /// The episode after `episode_id` in (season, episode) order, if any — for auto-next.
    pub fn next_episode(&self, episode_id: i64) -> Result<Option<EpisodeRecord>> {
        let cur = self.get_episode(episode_id)?;
        self.with_read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {EP_COLS} FROM episodes WHERE series_id = ?1 AND (season > ?2 OR (season = ?2 AND episode > ?3))
                     ORDER BY season, episode LIMIT 1"
                ),
                params![cur.series_id, cur.season, cur.episode],
                row_to_episode,
            )
            .optional()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::PlaylistInsert;

    fn pl(db: &Db) -> i64 {
        db.insert_playlist(&PlaylistInsert {
            r#type: "xtream".into(),
            name: "t".into(),
            base_url: "x".into(),
            user: None,
            pass: None,
            mac: None,
            ua: None,
        })
        .unwrap()
    }

    fn movie(i: i64, cat: &str) -> VodInsert {
        VodInsert {
            kind: "movie".into(),
            source_id: format!("m{i}"),
            title: format!("Mövie {i}"),
            category: Some(cat.into()),
            stream_url: Some(format!("http://h/movie/u/p/{i}.mkv")),
            added: Some(1_000 + i),
            year: Some(2000 + (i % 20) as i32),
            ..Default::default()
        }
    }

    #[test]
    fn vod_upsert_list_search_episodes() {
        let db = Db::open_in_memory().unwrap();
        let p = pl(&db);
        let rows: Vec<_> = (0..500).map(|i| movie(i, if i % 2 == 0 { "Action" } else { "Drama" })).collect();
        let (ins, _) = db.upsert_vod(p, &rows).unwrap();
        assert_eq!(ins, 500);
        assert_eq!(db.vod_count(p, "movie", None).unwrap(), 500);
        assert_eq!(db.vod_count(p, "movie", Some("Action")).unwrap(), 250);
        let newest = db.list_vod(p, "movie", None, "added", 3, 0).unwrap();
        assert_eq!(newest[0].title, "Mövie 499");
        let by_title = db.list_vod(p, "movie", Some("Drama"), "title", 2, 0).unwrap();
        assert_eq!(by_title[0].title, "Mövie 1");
        assert_eq!(db.vod_groups(p, "movie").unwrap().len(), 2);
        let hits = db.search_vod("movie 12", p, Some("movie"), 50).unwrap();
        assert!(hits.iter().all(|h| h.title.starts_with("Mövie 12")));
        assert!(hits.len() >= 11);

        let series =
            VodInsert { kind: "series".into(), source_id: "s1".into(), title: "Show".into(), ..Default::default() };
        db.upsert_vod(p, &[series]).unwrap();
        let s = db.list_vod(p, "series", None, "title", 10, 0).unwrap()[0].clone();
        assert!(s.episodes_synced.is_none());
        let eps: Vec<_> = (1..=3)
            .map(|e| EpisodeInsert {
                season: 1,
                episode: e,
                stream_url: format!("http://h/series/u/p/{e}.mkv"),
                ..Default::default()
            })
            .collect();
        assert_eq!(db.upsert_episodes(s.id, &eps).unwrap(), 3);
        assert!(db.get_vod(s.id).unwrap().episodes_synced.is_some());
        let list = db.episodes(s.id).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(db.next_episode(list[0].id).unwrap().unwrap().episode, 2);
        assert!(db.next_episode(list[2].id).unwrap().is_none());
    }
}
