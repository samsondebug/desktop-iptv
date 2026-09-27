//! Source adapters (CLAUDE.md §7). Priority: Xtream → M3U → Stalker (flagged).
//!
//! Day 1–14 ships the M3U adapter. Xtream lands in days 15–30 behind the same trait so the
//! Tauri command layer never changes shape.

use crate::importer::{import_m3u, ImportSource, ProgressSink};
use crate::{NetError, Result};
use app_core::SyncStats;
use app_db::Db;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountInfo {
    pub status: Option<String>,
    pub exp_date: Option<i64>,
    pub max_connections: Option<u32>,
    pub active_connections: Option<u32>,
    pub server_timezone: Option<String>,
}

#[async_trait]
pub trait CatalogAdapter: Send + Sync {
    async fn authenticate(&self) -> Result<AccountInfo>;
    async fn sync_live(&self, playlist_id: i64) -> Result<SyncStats>;
    async fn sync_vod(&self, playlist_id: i64) -> Result<SyncStats>;
    async fn sync_epg(&self, playlist_id: i64) -> Result<SyncStats>;
}

/// M3U/M3U8 by URL or local file.
pub struct M3uAdapter {
    pub db: Arc<Db>,
    pub source: ImportSource,
    pub sink: Arc<dyn ProgressSink>,
}

#[async_trait]
impl CatalogAdapter for M3uAdapter {
    async fn authenticate(&self) -> Result<AccountInfo> {
        // M3U has no account concept; a successful HEAD/GET is the only "auth".
        Ok(AccountInfo::default())
    }

    async fn sync_live(&self, playlist_id: i64) -> Result<SyncStats> {
        import_m3u(self.db.clone(), playlist_id, self.source.clone(), self.sink.clone()).await
    }

    async fn sync_vod(&self, _playlist_id: i64) -> Result<SyncStats> {
        // M3U VOD entries are imported as channels in day 1–14; VOD split lands in days 31–50.
        Ok(SyncStats::default())
    }

    async fn sync_epg(&self, _playlist_id: i64) -> Result<SyncStats> {
        Err(NetError::Other("XMLTV import lands in days 15–30".into()))
    }
}

/// Placeholder so the shape exists; implemented in days 15–30.
pub struct XtreamAdapter;

#[async_trait]
impl CatalogAdapter for XtreamAdapter {
    async fn authenticate(&self) -> Result<AccountInfo> {
        Err(NetError::Other("Xtream adapter not implemented yet (days 15–30)".into()))
    }
    async fn sync_live(&self, _: i64) -> Result<SyncStats> {
        Err(NetError::Other("Xtream adapter not implemented yet (days 15–30)".into()))
    }
    async fn sync_vod(&self, _: i64) -> Result<SyncStats> {
        Err(NetError::Other("Xtream adapter not implemented yet (days 15–30)".into()))
    }
    async fn sync_epg(&self, _: i64) -> Result<SyncStats> {
        Err(NetError::Other("Xtream adapter not implemented yet (days 15–30)".into()))
    }
}
