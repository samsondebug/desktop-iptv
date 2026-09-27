//! Recording / download commands (PRO features, CLAUDE.md §10 & §12 days 51–70).

use super::{err, now_unix, require_feature, CmdResult};
use crate::dvr;
use crate::state::AppState;
use app_core::license::Feature;
use app_db::recordings::{DownloadRecord, RecordingRecord};
use serde::Serialize;
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Serialize)]
pub struct RecordStartResult {
    pub id: i64,
    pub mode: String,
    pub path: String,
    pub warning: Option<String>,
}

/// Record the given channel starting now for `minutes` (+ overrun applies only to scheduled jobs).
#[tauri::command]
pub fn record_now(
    app: AppHandle,
    state: State<'_, AppState>,
    channel_id: i64,
    minutes: i64,
    title: Option<String>,
) -> CmdResult<RecordStartResult> {
    require_feature(&state, Feature::Recording, "Recording")?;
    let ch = state.db.get_channel(channel_id).map_err(err)?;
    let start = now_unix();
    let stop = start + minutes.clamp(1, 24 * 60) * 60;
    let dir = dvr::media_dir(&app, &state, "Recordings");
    let path = dir.join(dvr::recording_file_name(&ch.name, title.as_deref(), start));
    let warning = dvr::disk_warning(&dir, None);
    let id = state
        .db
        .add_recording(channel_id, title.as_deref(), start, stop, 0, &path.display().to_string(), "scheduled")
        .map_err(err)?;
    let rec = state.db.get_recording(id).map_err(err)?;
    let mode = dvr::start_job(&app, &state, &rec).inspect_err(|e| {
        let _ = state.db.set_recording_status(id, "failed", None, Some(e));
    })?;
    Ok(RecordStartResult { id, mode, path: path.display().to_string(), warning })
}

/// Schedule a recording (typically from an EPG programme). Starts immediately if already due.
#[tauri::command]
pub fn schedule_recording(
    app: AppHandle,
    state: State<'_, AppState>,
    channel_id: i64,
    start: i64,
    stop: i64,
    title: Option<String>,
    extra_end_s: Option<i64>,
) -> CmdResult<i64> {
    require_feature(&state, Feature::Recording, "Recording")?;
    let ch = state.db.get_channel(channel_id).map_err(err)?;
    if stop <= now_unix() {
        return Err("That programme has already ended".into());
    }
    let dir = dvr::media_dir(&app, &state, "Recordings");
    let path = dir.join(dvr::recording_file_name(&ch.name, title.as_deref(), start));
    let id = state
        .db
        .add_recording(
            channel_id,
            title.as_deref(),
            start,
            stop,
            extra_end_s.unwrap_or(dvr::DEFAULT_EXTRA_END_S),
            &path.display().to_string(),
            "scheduled",
        )
        .map_err(err)?;
    if start <= now_unix() {
        dvr::tick(&app);
    }
    Ok(id)
}

#[tauri::command]
pub fn stop_recording(app: AppHandle, state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    dvr::stop_job(&app, &state, id, "completed", None);
    Ok(())
}

#[tauri::command]
pub fn list_recordings(state: State<'_, AppState>) -> CmdResult<Vec<RecordingRecord>> {
    state.db.list_recordings().map_err(err)
}

#[tauri::command]
pub fn delete_recording(app: AppHandle, state: State<'_, AppState>, id: i64, delete_file: bool) -> CmdResult<()> {
    let rec = state.db.get_recording(id).map_err(err)?;
    dvr::cancel_job(&app, &state, id);
    if delete_file {
        let _ = std::fs::remove_file(&rec.path);
    }
    state.db.delete_recording(id).map_err(err)
}

/// Play a finished recording in the main player.
#[tauri::command]
pub fn play_recording(state: State<'_, AppState>, id: i64) -> CmdResult<super::playback::PlaybackState> {
    let rec = state.db.get_recording(id).map_err(err)?;
    if !std::path::Path::new(&rec.path).exists() {
        return Err("The recording file no longer exists".into());
    }
    super::playback::do_load(&state, &rec.path, app_core::ProfileMode::Stable, crate::state::PlaybackItem::Url, None)?;
    Ok(super::playback::playback_state(&state))
}

/// Provider connections in use vs. the Xtream account limit (for the multiscreen/recording warning).
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionBudget {
    pub in_use: usize,
    pub max: Option<u32>,
}

#[tauri::command]
pub fn connection_budget(
    app: AppHandle,
    state: State<'_, AppState>,
    playlist_id: Option<i64>,
) -> CmdResult<ConnectionBudget> {
    let main = usize::from(state.playback.lock().unwrap().stream_url.is_some());
    let panes = state.panes.lock().unwrap().values().filter(|p| p.playing).count();
    let in_use = main + panes + dvr::extra_connections(&app);
    let max = playlist_id
        .and_then(|id| state.db.playlist_meta(id).ok())
        .and_then(|m| m.account_json)
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v.get("max_connections").and_then(|x| x.as_u64()).map(|x| x as u32));
    Ok(ConnectionBudget { in_use, max })
}

// ---------- downloads ----------

#[tauri::command]
pub fn download_item(
    app: AppHandle,
    state: State<'_, AppState>,
    item_type: String,
    item_id: i64,
) -> CmdResult<DownloadRecord> {
    require_feature(&state, Feature::Downloads, "Downloads")?;
    let (title, ext) = match item_type.as_str() {
        "vod" => {
            let v = state.db.get_vod(item_id).map_err(err)?;
            if v.stream_url.is_none() {
                return Err("This title has no downloadable stream".into());
            }
            (v.title.clone(), v.container_ext.clone().unwrap_or_else(|| "mkv".into()))
        }
        "episode" => {
            let e = state.db.get_episode(item_id).map_err(err)?;
            let s = state.db.get_vod(e.series_id).map_err(err)?;
            (
                format!(
                    "{} S{:02}E{:02}{}",
                    s.title,
                    e.season,
                    e.episode,
                    e.title.as_deref().map(|t| format!(" {t}")).unwrap_or_default()
                ),
                e.container_ext.clone().unwrap_or_else(|| "mkv".into()),
            )
        }
        _ => return Err("unknown item type".into()),
    };
    let dir = dvr::media_dir(&app, &state, "Downloads");
    let mut path = dir.join(format!(
        "{}.{}",
        dvr::recording_file_name(&title, None, 0).trim_start_matches("19700101-0000 ").trim_end_matches(".ts"),
        ext
    ));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{} ({n}).{ext}", title));
        n += 1;
    }
    let id = state.db.add_download(&item_type, item_id, &title, &path.display().to_string()).map_err(err)?;
    let row = state.db.get_download(id).map_err(err)?;
    if let Some((url, ua)) = dvr::download_source(&state, &row) {
        dvr::start_download(&app, &state, &row, url, ua);
    }
    state.db.get_download(id).map_err(err)
}

#[tauri::command]
pub fn list_downloads(state: State<'_, AppState>) -> CmdResult<Vec<DownloadRecord>> {
    state.db.list_downloads().map_err(err)
}

#[tauri::command]
pub fn pause_download(app: AppHandle, state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    if !dvr::pause_download(&app, id) {
        state.db.update_download(id, "paused", None, None, None).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub fn resume_download(app: AppHandle, state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.db.update_download(id, "queued", None, None, None).map_err(err)?;
    dvr::resume_queued(&app, &state);
    Ok(())
}

#[tauri::command]
pub fn delete_download(app: AppHandle, state: State<'_, AppState>, id: i64, delete_file: bool) -> CmdResult<()> {
    let row = state.db.get_download(id).map_err(err)?;
    dvr::pause_download(&app, id);
    if delete_file {
        let _ = std::fs::remove_file(&row.path);
        let _ = std::fs::remove_file(app_net::downloader::part_path(std::path::Path::new(&row.path)));
    }
    state.db.delete_download(id).map_err(err)
}

#[tauri::command]
pub fn play_download(state: State<'_, AppState>, id: i64) -> CmdResult<super::playback::PlaybackState> {
    let row = state.db.get_download(id).map_err(err)?;
    if row.status != "completed" {
        return Err("Download not finished yet".into());
    }
    super::playback::do_load(&state, &row.path, app_core::ProfileMode::Stable, crate::state::PlaybackItem::Url, None)?;
    Ok(super::playback::playback_state(&state))
}

#[tauri::command]
pub fn media_dir(app: AppHandle, state: State<'_, AppState>) -> CmdResult<String> {
    Ok(dvr::media_dir(&app, &state, "").display().to_string())
}

#[tauri::command]
pub fn set_media_dir(state: State<'_, AppState>, path: Option<String>) -> CmdResult<()> {
    match path.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
            state.db.set_setting("media_dir", p.trim()).map_err(err)
        }
        None => state.db.delete_setting("media_dir").map_err(err),
    }
}
