//! EPG: grid data for visible channels, now/next, offset, overrides, sources.

use super::{err, CmdResult};
use crate::state::AppState;
use crate::sync::{spawn_sync, SyncScope};
use app_db::epg::{EpgSource, EpgStats, Programme};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Deserialize)]
pub struct EpgGridRequest {
    pub playlist_id: i64,
    pub channel_ids: Vec<i64>,
    /// unix seconds, inclusive window
    pub from: i64,
    pub to: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpgGridRow {
    pub channel_id: i64,
    pub tvg_id: Option<String>,
    pub programmes: Vec<Programme>,
}

/// Resolve the effective tvg-id per channel: manual override → channel.tvg_id.
fn effective_ids(state: &AppState, playlist_id: i64, channel_ids: &[i64]) -> CmdResult<Vec<(i64, Option<String>)>> {
    let overrides: HashMap<i64, String> = state.db.epg_overrides_for_playlist(playlist_id).map_err(err)?.into_iter().collect();
    let mut out = Vec::with_capacity(channel_ids.len());
    for &id in channel_ids.iter().take(500) {
        let tvg = match overrides.get(&id) {
            Some(t) => Some(t.clone()),
            None => state.db.get_channel(id).ok().and_then(|c| c.tvg_id).filter(|t| !t.is_empty()),
        };
        out.push((id, tvg));
    }
    Ok(out)
}

/// Programmes for the visible channel rows. Times returned are already offset-adjusted.
#[tauri::command]
pub fn epg_grid(state: State<'_, AppState>, req: EpgGridRequest) -> CmdResult<Vec<EpgGridRow>> {
    let offset = state.db.epg_offset_min(req.playlist_id).map_err(err)? as i64 * 60;
    let ids = effective_ids(&state, req.playlist_id, &req.channel_ids)?;
    let tvg_ids: Vec<&str> = ids.iter().filter_map(|(_, t)| t.as_deref()).collect();
    let progs = state.db.programmes_range(req.playlist_id, &tvg_ids, req.from - offset, req.to - offset).map_err(err)?;
    let mut by_tvg: HashMap<&str, Vec<Programme>> = HashMap::new();
    for p in &progs {
        by_tvg.entry(p.channel_tvg_id.as_str()).or_default().push(Programme {
            channel_tvg_id: p.channel_tvg_id.clone(),
            start: p.start + offset,
            stop: p.stop + offset,
            title: p.title.clone(),
            desc: p.desc.clone(),
        });
    }
    Ok(ids
        .into_iter()
        .map(|(channel_id, tvg)| EpgGridRow {
            programmes: tvg.as_deref().and_then(|t| by_tvg.get(t).cloned()).unwrap_or_default(),
            channel_id,
            tvg_id: tvg,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct NowNext {
    pub channel_id: i64,
    pub tvg_id: Option<String>,
    pub now: Option<Programme>,
    pub next: Option<Programme>,
}

#[tauri::command]
pub fn epg_now_next(state: State<'_, AppState>, channel_id: i64) -> CmdResult<NowNext> {
    let ch = state.db.get_channel(channel_id).map_err(err)?;
    let offset = state.db.epg_offset_min(ch.playlist_id).map_err(err)? as i64 * 60;
    let tvg = match state.db.epg_override(channel_id).map_err(err)? {
        Some(t) => Some(t),
        None => ch.tvg_id.clone().filter(|t| !t.is_empty()),
    };
    let Some(t) = tvg.clone() else {
        return Ok(NowNext { channel_id, tvg_id: None, now: None, next: None });
    };
    let now = super::now_unix() - offset;
    let (a, b) = state.db.now_next(ch.playlist_id, &t, now).map_err(err)?;
    let shift = |p: Programme| Programme { start: p.start + offset, stop: p.stop + offset, ..p };
    Ok(NowNext { channel_id, tvg_id: tvg, now: a.map(shift), next: b.map(shift) })
}

#[tauri::command]
pub fn epg_stats(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<EpgStats> {
    state.db.epg_stats(playlist_id).map_err(err)
}

#[tauri::command]
pub fn set_epg_offset(state: State<'_, AppState>, playlist_id: i64, minutes: i32) -> CmdResult<()> {
    state.db.set_epg_offset_min(playlist_id, minutes).map_err(err)
}

#[tauri::command]
pub fn set_epg_override(state: State<'_, AppState>, channel_id: i64, tvg_id: Option<String>) -> CmdResult<()> {
    state.db.set_epg_override(channel_id, tvg_id.as_deref()).map_err(err)
}

#[tauri::command]
pub fn get_epg_override(state: State<'_, AppState>, channel_id: i64) -> CmdResult<Option<String>> {
    state.db.epg_override(channel_id).map_err(err)
}

/// tvg-ids present in the guide that contain `q` (for the "Edit EPG" picker).
#[tauri::command]
pub fn epg_search_ids(state: State<'_, AppState>, playlist_id: i64, q: String) -> CmdResult<Vec<String>> {
    let ids = state.db.epg_channel_ids(playlist_id).map_err(err)?;
    let q = q.trim().to_lowercase();
    let mut out: Vec<String> = ids.into_iter().filter(|id| q.is_empty() || id.to_lowercase().contains(&q)).collect();
    out.sort();
    out.truncate(200);
    Ok(out)
}

#[tauri::command]
pub fn list_epg_sources(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<Vec<EpgSource>> {
    state.db.list_epg_sources(playlist_id).map_err(err)
}

#[tauri::command]
pub fn add_epg_source(app: AppHandle, state: State<'_, AppState>, playlist_id: i64, url: String) -> CmdResult<i64> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("EPG URL must start with http:// or https://".into());
    }
    let id = state.db.add_epg_source(playlist_id, u).map_err(err)?;
    spawn_sync(app, state.db.clone(), playlist_id, SyncScope::EpgOnly);
    Ok(id)
}

#[tauri::command]
pub fn delete_epg_source(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.db.delete_epg_source(id).map_err(err)
}

#[tauri::command]
pub fn refresh_epg(app: AppHandle, state: State<'_, AppState>, playlist_id: i64) -> CmdResult<bool> {
    Ok(spawn_sync(app, state.db.clone(), playlist_id, SyncScope::EpgOnly))
}
