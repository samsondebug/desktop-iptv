//! Per-channel stream-health snapshots ("this channel played at 1080p H.264 5 Mbps an hour
//! ago"). Fed from live playback telemetry and on-demand diagnostics probes — SKTV never
//! scans a provider's catalog in the background.

use crate::{Db, Result};
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct HealthRecord {
    pub channel_id: i64,
    pub ok: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<i64>,
    /// 'playback' | 'probe'
    pub source: String,
    pub checked_at: i64,
}

impl Db {
    #[allow(clippy::too_many_arguments)]
    pub fn set_channel_health(
        &self,
        channel_id: i64,
        ok: bool,
        width: Option<i64>,
        height: Option<i64>,
        codec: Option<&str>,
        bitrate_kbps: Option<i64>,
        source: &str,
        checked_at: i64,
    ) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO channel_health(channel_id, ok, width, height, codec, bitrate_kbps, source, checked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(channel_id) DO UPDATE SET ok = excluded.ok, width = excluded.width,
                   height = excluded.height, codec = excluded.codec, bitrate_kbps = excluded.bitrate_kbps,
                   source = excluded.source, checked_at = excluded.checked_at",
                params![channel_id, ok as i64, width, height, codec, bitrate_kbps, source, checked_at],
            )?;
            Ok(())
        })
    }

    /// Health rows for the given channels (visible rows only — the caller pages).
    pub fn channel_health(&self, channel_ids: &[i64]) -> Result<Vec<HealthRecord>> {
        if channel_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = channel_ids.iter().copied().take(500).collect();
        let marks = vec!["?"; ids.len()].join(",");
        self.with_read(|c| {
            let mut st = c.prepare(&format!(
                "SELECT channel_id, ok, width, height, codec, bitrate_kbps, source, checked_at
                 FROM channel_health WHERE channel_id IN ({marks})"
            ))?;
            let it = st.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
                Ok(HealthRecord {
                    channel_id: r.get(0)?,
                    ok: r.get::<_, i64>(1)? != 0,
                    width: r.get(2)?,
                    height: r.get(3)?,
                    codec: r.get(4)?,
                    bitrate_kbps: r.get(5)?,
                    source: r.get(6)?,
                    checked_at: r.get(7)?,
                })
            })?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::channels::{ChannelInsert, PlaylistInsert};
    use crate::Db;

    #[test]
    fn upsert_and_fetch() {
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
        db.upsert_channels(
            p,
            &[ChannelInsert {
                source_id: "1".into(),
                name: "One".into(),
                stream_url: "http://x/1.ts".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let id = db.list_channels(p, None, 10, 0).unwrap()[0].id;
        db.set_channel_health(id, true, Some(1920), Some(1080), Some("h264"), Some(5200), "playback", 1000).unwrap();
        db.set_channel_health(id, true, Some(1280), Some(720), Some("h264"), Some(2500), "probe", 2000).unwrap();
        let rows = db.channel_health(&[id, 9999]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].height, Some(720));
        assert_eq!(rows[0].source, "probe");
        assert!(db.channel_health(&[]).unwrap().is_empty());
    }
}
