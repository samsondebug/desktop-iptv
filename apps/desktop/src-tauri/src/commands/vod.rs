//! Movies / series / episodes / continue watching.

use super::{err, require_feature, CmdResult};
use crate::state::AppState;
use crate::sync::{spawn_sync, SyncScope};
use app_db::progress::ProgressRecord;
use app_db::vod::{EpisodeRecord, VodFilter, VodGroup, VodRecord};
use app_net::adapters::XtreamAdapter;
use app_net::importer::NoopSink;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Deserialize)]
pub struct ListVodRequest {
    pub playlist_id: i64,
    /// "movie" | "series"
    pub kind: String,
    pub category: Option<String>,
    /// "added" | "title" | "year" | "rating"
    pub sort: Option<String>,
    /// Filter chips (year window / minimum rating); absent = everything.
    #[serde(default)]
    pub filter: Option<VodFilter>,
    pub limit: usize,
    pub offset: usize,
}

#[tauri::command]
pub fn list_vod(state: State<'_, AppState>, req: ListVodRequest) -> CmdResult<Vec<VodRecord>> {
    state
        .db
        .list_vod_filtered(
            req.playlist_id,
            &req.kind,
            req.category.as_deref(),
            req.sort.as_deref().unwrap_or("added"),
            &req.filter.unwrap_or_default(),
            req.limit,
            req.offset,
        )
        .map_err(err)
}

#[tauri::command]
pub fn count_vod(
    state: State<'_, AppState>,
    playlist_id: i64,
    kind: String,
    category: Option<String>,
    filter: Option<VodFilter>,
) -> CmdResult<i64> {
    state.db.vod_count_filtered(playlist_id, &kind, category.as_deref(), &filter.unwrap_or_default()).map_err(err)
}

#[tauri::command]
pub fn vod_groups(state: State<'_, AppState>, playlist_id: i64, kind: String) -> CmdResult<Vec<VodGroup>> {
    state.db.vod_groups(playlist_id, &kind).map_err(err)
}

#[tauri::command]
pub fn search_vod(
    state: State<'_, AppState>,
    playlist_id: i64,
    query: String,
    kind: Option<String>,
    limit: usize,
) -> CmdResult<Vec<VodRecord>> {
    state.db.search_vod(&query, playlist_id, kind.as_deref(), limit).map_err(err)
}

#[tauri::command]
pub fn get_vod(state: State<'_, AppState>, vod_id: i64) -> CmdResult<VodRecord> {
    state.db.get_vod(vod_id).map_err(err)
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesDetail {
    pub series: VodRecord,
    pub episodes: Vec<EpisodeRecord>,
    pub progress: Vec<ProgressRecord>,
    pub fetched_now: bool,
}

/// Episodes for a series. Fetched lazily from the provider (Xtream `get_series_info`) the first
/// time, or when `force` is set; cached in SQLite afterwards.
#[tauri::command]
pub async fn series_detail(state: State<'_, AppState>, series_id: i64, force: bool) -> CmdResult<SeriesDetail> {
    let series = state.db.get_vod(series_id).map_err(err)?;
    let mut fetched_now = false;
    if series.kind == "series" && (force || series.episodes_synced.is_none()) {
        let src = state.db.playlist_source(series.playlist_id).map_err(err)?;
        if src.r#type == "xtream" {
            let adapter =
                XtreamAdapter::from_playlist(state.db.clone(), series.playlist_id, Arc::new(NoopSink)).map_err(err)?;
            let (info, episodes) = adapter.client.series_info(&series.source_id).await.map_err(err)?;
            if let Some(mut v) = info {
                v.category = series.category.clone();
                v.kind = "series".into();
                v.source_id = series.source_id.clone();
                let _ = state.db.upsert_vod(series.playlist_id, &[v]);
            }
            state.db.upsert_episodes(series_id, &episodes).map_err(err)?;
            fetched_now = true;
        }
    }
    let series = state.db.get_vod(series_id).map_err(err)?;
    let episodes = state.db.episodes(series_id).map_err(err)?;
    let mut progress = Vec::new();
    for e in &episodes {
        if let Ok(Some(p)) = state.db.get_progress("episode", e.id) {
            progress.push(p);
        }
    }
    Ok(SeriesDetail { series, episodes, progress, fetched_now })
}

#[derive(Debug, Clone, Serialize)]
pub struct ContinueItem {
    pub progress: ProgressRecord,
    pub vod: Option<VodRecord>,
    pub episode: Option<EpisodeRecord>,
    pub series: Option<VodRecord>,
}

#[tauri::command]
pub fn continue_watching(state: State<'_, AppState>) -> CmdResult<Vec<ContinueItem>> {
    let rows = state.db.continue_watching(30).map_err(err)?;
    let mut out = Vec::new();
    for p in rows {
        match p.item_type.as_str() {
            "vod" => {
                if let Ok(v) = state.db.get_vod(p.item_id) {
                    out.push(ContinueItem { progress: p, vod: Some(v), episode: None, series: None });
                }
            }
            "episode" => {
                if let Ok(e) = state.db.get_episode(p.item_id) {
                    let series = state.db.get_vod(e.series_id).ok();
                    out.push(ContinueItem { progress: p, vod: None, episode: Some(e), series });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn get_progress(state: State<'_, AppState>, item_type: String, item_id: i64) -> CmdResult<Option<ProgressRecord>> {
    state.db.get_progress(&item_type, item_id).map_err(err)
}

#[tauri::command]
pub fn clear_progress(state: State<'_, AppState>, item_type: String, item_id: i64) -> CmdResult<()> {
    state.db.clear_progress(&item_type, item_id).map_err(err)
}

#[tauri::command]
pub fn refresh_vod(app: AppHandle, state: State<'_, AppState>, playlist_id: i64) -> CmdResult<bool> {
    require_feature(&state, app_core::license::Feature::UnlimitedVod, "VOD refresh")?;
    Ok(spawn_sync(app, state.db.clone(), playlist_id, SyncScope::VodOnly))
}
