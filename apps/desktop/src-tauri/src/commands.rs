//! Tauri commands. Every error string passes through `redact` before it reaches the UI.

use crate::state::AppState;
use crate::{EV_IMPORT_DONE, EV_IMPORT_PROGRESS};
use app_core::redact::redact;
use app_core::*;
use app_db::channels::PlaylistInsert;
use app_net::importer::{ImportSource, ProgressSink};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    redact(&e.to_string())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ---------- bootstrap / config ----------

#[derive(Debug, Clone, Serialize)]
pub struct Bootstrap {
    pub version: String,
    pub product: String,
    pub legal_block: String,
    pub config: ConfigPayload,
    pub license: LicenseStateResponse,
    pub playlists: Vec<PlaylistSummary>,
    pub engine_kind: String,
    pub engine_description: String,
    pub last_channel_id: Option<i64>,
    pub platform: String,
}

#[tauri::command]
pub fn get_bootstrap(state: State<'_, AppState>) -> CmdResult<Bootstrap> {
    let config = state.db.load_config().map_err(err)?;
    Ok(Bootstrap {
        version: PRODUCT_VERSION.into(),
        product: PRODUCT_NAME.into(),
        legal_block: LEGAL_BLOCK.into(),
        license: license_state(&state)?,
        playlists: state.db.list_playlists().map_err(err)?,
        engine_kind: state.engine.kind().into(),
        engine_description: state.engine.describe(),
        last_channel_id: state.db.last_channel_id().map_err(err)?,
        platform: std::env::consts::OS.into(),
        config,
    })
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> CmdResult<ConfigPayload> {
    state.db.load_config().map_err(err)
}

#[tauri::command]
pub fn set_config(state: State<'_, AppState>, config: ConfigPayload) -> CmdResult<ConfigPayload> {
    let saved = state.db.save_config(&config).map_err(err)?;
    // Apply the live-applicable bits to the engine immediately.
    let _ = state.engine.set_audio_delay_ms(saved.audio_delay_ms);
    {
        let mut pb = state.playback.lock().unwrap();
        pb.volume = saved.audio_boost;
        if !pb.muted {
            let _ = state.engine.set_volume(saved.audio_boost);
        }
    }
    Ok(saved)
}

#[tauri::command]
pub fn accept_legal(state: State<'_, AppState>) -> CmdResult<ConfigPayload> {
    let mut cfg = state.db.load_config().map_err(err)?;
    cfg.legal_accepted = true;
    state.db.save_config(&cfg).map_err(err)
}

// ---------- playlists / import ----------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddPlaylistSource {
    M3uUrl { url: String, user_agent: Option<String> },
    M3uFile { path: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportDone {
    pub playlist_id: i64,
    pub ok: bool,
    pub stats: Option<SyncStats>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub preview: Option<String>,
}

struct EmitSink {
    app: AppHandle,
}

impl ProgressSink for EmitSink {
    fn progress(&self, ev: ImportProgressEvent) {
        let _ = self.app.emit(EV_IMPORT_PROGRESS, &ev);
    }
}

fn spawn_import(app: AppHandle, db: Arc<app_db::Db>, playlist_id: i64, source: ImportSource) {
    tauri::async_runtime::spawn(async move {
        let sink: Arc<dyn ProgressSink> = Arc::new(EmitSink { app: app.clone() });
        let result = app_net::import_m3u(db.clone(), playlist_id, source, sink).await;
        let done = match result {
            Ok(stats) => {
                if stats.inserted + stats.updated > 0 {
                    // First successful import starts the 72 h trial clock (CLAUDE.md §10).
                    let _ = db.ensure_trial_started(now_unix());
                }
                ImportDone { playlist_id, ok: true, stats: Some(stats), error: None, error_kind: None, preview: None }
            }
            Err(app_net::NetError::Preflight(p)) => ImportDone {
                playlist_id,
                ok: false,
                stats: None,
                error: Some(p.message.clone()),
                error_kind: Some(format!("{:?}", p.kind).to_lowercase()),
                preview: Some(p.preview.clone()),
            },
            Err(e) => ImportDone {
                playlist_id,
                ok: false,
                stats: None,
                error: Some(redact(&e.to_string())),
                error_kind: Some("network".into()),
                preview: None,
            },
        };
        let _ = app.emit(EV_IMPORT_DONE, &done);
    });
}

#[tauri::command]
pub fn add_playlist(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    source: AddPlaylistSource,
) -> CmdResult<i64> {
    let name = name.trim();
    let (row, import_src) = match &source {
        AddPlaylistSource::M3uUrl { url, user_agent } => {
            let url = url.trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("Playlist URL must start with http:// or https://".into());
            }
            (
                PlaylistInsert {
                    r#type: "m3u".into(),
                    name: if name.is_empty() { host_of(&url) } else { name.to_string() },
                    base_url: url.clone(),
                    user: None,
                    pass: None,
                    mac: None,
                    ua: user_agent.clone().filter(|s| !s.trim().is_empty()),
                },
                ImportSource::Url { url, user_agent: user_agent.clone() },
            )
        }
        AddPlaylistSource::M3uFile { path } => {
            let p = std::path::PathBuf::from(path);
            if !p.is_file() {
                return Err("Playlist file not found".into());
            }
            (
                PlaylistInsert {
                    r#type: "m3u".into(),
                    name: if name.is_empty() {
                        p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Playlist".into())
                    } else {
                        name.to_string()
                    },
                    base_url: format!("file://{}", p.display()),
                    user: None,
                    pass: None,
                    mac: None,
                    ua: None,
                },
                ImportSource::File { path: p },
            )
        }
    };
    // Free tier after the trial: one playlist only (CLAUDE.md §10).
    let lic = license_state(&state)?;
    if !app_core::license::allows(&lic.tier, app_core::license::Feature::MultiplePlaylists)
        && !state.db.list_playlists().map_err(err)?.is_empty()
    {
        return Err("The free tier supports one playlist. Unlock PRO for unlimited playlists.".into());
    }
    let id = state.db.insert_playlist(&row).map_err(err)?;
    spawn_import(app, state.db.clone(), id, import_src);
    Ok(id)
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.to_string())).unwrap_or_else(|| "Playlist".into())
}

#[tauri::command]
pub fn refresh_playlist(app: AppHandle, state: State<'_, AppState>, playlist_id: i64) -> CmdResult<()> {
    let src = state.db.playlist_source(playlist_id).map_err(err)?;
    let import_src = if let Some(path) = src.base_url.strip_prefix("file://") {
        ImportSource::File { path: path.into() }
    } else {
        ImportSource::Url { url: src.base_url.clone(), user_agent: src.ua.clone() }
    };
    spawn_import(app, state.db.clone(), playlist_id, import_src);
    Ok(())
}

#[tauri::command]
pub fn delete_playlist(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<()> {
    state.db.delete_playlist(playlist_id).map_err(err)
}

#[tauri::command]
pub fn list_playlists(state: State<'_, AppState>) -> CmdResult<Vec<PlaylistSummary>> {
    state.db.list_playlists().map_err(err)
}

// ---------- catalog ----------

#[tauri::command]
pub fn list_groups(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<Vec<GroupSummary>> {
    state.db.list_groups(playlist_id).map_err(err)
}

#[tauri::command]
pub fn list_channels(state: State<'_, AppState>, req: ListChannelsRequest) -> CmdResult<Vec<ChannelRecord>> {
    state.db.list_channels(req.playlist_id, req.group_title.as_deref(), req.limit, req.offset).map_err(err)
}

#[tauri::command]
pub fn count_channels(state: State<'_, AppState>, playlist_id: i64, group_title: Option<String>) -> CmdResult<i64> {
    state.db.channel_count(playlist_id, group_title.as_deref()).map_err(err)
}

#[tauri::command]
pub fn search_channels(state: State<'_, AppState>, req: FtsQueryRequest) -> CmdResult<Vec<ChannelRecord>> {
    state.db.search_channels(&req.query_string, req.playlist_id, req.limit, req.offset).map_err(err)
}

#[tauri::command]
pub fn get_channel(state: State<'_, AppState>, channel_id: i64) -> CmdResult<ChannelRecord> {
    state.db.get_channel(channel_id).map_err(err)
}

#[tauri::command]
pub fn get_favorites(state: State<'_, AppState>) -> CmdResult<Vec<ChannelRecord>> {
    state.db.favorite_channels().map_err(err)
}

#[tauri::command]
pub fn get_favorite_ids(state: State<'_, AppState>) -> CmdResult<Vec<i64>> {
    state.db.favorite_ids().map_err(err)
}

#[tauri::command]
pub fn set_favorite(state: State<'_, AppState>, channel_id: i64, on: bool) -> CmdResult<()> {
    state.db.set_favorite("channel", channel_id, on).map_err(err)
}

#[tauri::command]
pub fn get_recents(state: State<'_, AppState>) -> CmdResult<Vec<ChannelRecord>> {
    state.db.recent_channels(30).map_err(err)
}

// ---------- playback ----------

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackState {
    pub channel_id: Option<i64>,
    pub stream_url_redacted: Option<String>,
    pub profile: String,
    pub volume: u16,
    pub muted: bool,
    pub paused: bool,
    pub engine_kind: String,
}

fn playback_state(state: &AppState) -> PlaybackState {
    let pb = state.playback.lock().unwrap();
    PlaybackState {
        channel_id: pb.channel_id,
        stream_url_redacted: pb.stream_url.as_deref().map(redact),
        profile: pb.profile.as_str().into(),
        volume: pb.volume,
        muted: pb.muted,
        paused: pb.paused,
        engine_kind: state.engine.kind().into(),
    }
}

fn do_load(state: &AppState, url: &str, profile: ProfileMode) -> CmdResult<()> {
    let boost = {
        let pb = state.playback.lock().unwrap();
        if pb.muted {
            0
        } else {
            pb.volume
        }
    };
    state.engine.load(url, profile, boost).map_err(err)?;
    let mut pb = state.playback.lock().unwrap();
    pb.stream_url = Some(url.to_string());
    pb.profile = profile;
    pb.paused = false;
    Ok(())
}

/// Click on a channel → `LoadStreamCommand` → `loadfile replace` (CLAUDE.md §15 step 6).
#[tauri::command]
pub fn play_channel(state: State<'_, AppState>, channel_id: i64) -> CmdResult<PlaybackState> {
    let ch = state.db.get_channel(channel_id).map_err(err)?;
    let profile = state.playback.lock().unwrap().profile;
    do_load(&state, &ch.stream_url, profile)?;
    state.playback.lock().unwrap().channel_id = Some(channel_id);
    let _ = state.db.touch_recent(channel_id);
    let _ = state.db.set_last_channel_id(channel_id);
    Ok(playback_state(&state))
}

/// Raw stream URL (Diagnostics / "open URL" box). Not tied to a channel row.
#[tauri::command]
pub fn load_stream(state: State<'_, AppState>, cmd: LoadStreamCommand) -> CmdResult<PlaybackState> {
    let profile = ProfileMode::parse(&cmd.profile_mode).unwrap_or(state.playback.lock().unwrap().profile);
    {
        let mut pb = state.playback.lock().unwrap();
        pb.volume = cmd.audio_boost.clamp(0, 130);
        pb.channel_id = None;
    }
    do_load(&state, &cmd.stream_url, profile)?;
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn stop_playback(state: State<'_, AppState>) -> CmdResult<PlaybackState> {
    state.engine.stop().map_err(err)?;
    let mut pb = state.playback.lock().unwrap();
    pb.channel_id = None;
    pb.stream_url = None;
    drop(pb);
    Ok(playback_state(&state))
}

/// Toggle low_latency / stable. Reloads the current stream so startup-only options apply.
#[tauri::command]
pub fn set_profile(state: State<'_, AppState>, profile: String) -> CmdResult<PlaybackState> {
    let p = ProfileMode::parse(&profile).ok_or_else(|| "unknown profile".to_string())?;
    state.engine.set_profile(p).map_err(err)?;
    let url = {
        let mut pb = state.playback.lock().unwrap();
        pb.profile = p;
        pb.stream_url.clone()
    };
    if let Some(url) = url {
        do_load(&state, &url, p)?;
    }
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn set_pause(state: State<'_, AppState>, paused: bool) -> CmdResult<PlaybackState> {
    state.engine.set_pause(paused).map_err(err)?;
    state.playback.lock().unwrap().paused = paused;
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn set_mute(state: State<'_, AppState>, muted: bool) -> CmdResult<PlaybackState> {
    state.engine.set_mute(muted).map_err(err)?;
    state.playback.lock().unwrap().muted = muted;
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn set_volume(state: State<'_, AppState>, volume: u16) -> CmdResult<PlaybackState> {
    let v = volume.min(130);
    state.engine.set_volume(v).map_err(err)?;
    let mut pb = state.playback.lock().unwrap();
    pb.volume = v;
    pb.muted = false;
    drop(pb);
    Ok(playback_state(&state))
}

/// The UI reports where the player pane sits (CSS px, relative to the webview) and the webview
/// size; the engine turns that into `video-margin-ratio-*` so video stays inside the pane.
#[tauri::command]
pub fn set_video_rect(
    state: State<'_, AppState>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    win_w: f32,
    win_h: f32,
) -> CmdResult<()> {
    if win_w <= 0.0 || win_h <= 0.0 || w <= 0.0 || h <= 0.0 {
        return Ok(());
    }
    let left = (x / win_w).clamp(0.0, 1.0);
    let top = (y / win_h).clamp(0.0, 1.0);
    let right = (1.0 - (x + w) / win_w).clamp(0.0, 1.0);
    let bottom = (1.0 - (y + h) / win_h).clamp(0.0, 1.0);
    state.engine.set_video_margins(left, right, top, bottom).map_err(err)
}

#[tauri::command]
pub fn get_telemetry(state: State<'_, AppState>) -> CmdResult<EngineTelemetryEvent> {
    Ok(state.engine.telemetry())
}

#[tauri::command]
pub fn engine_get_property(state: State<'_, AppState>, name: String) -> CmdResult<Option<String>> {
    state.engine.get_property(&name).map_err(err)
}

/// Advanced panel. A small denylist keeps the UI from re-pointing the engine at a file/URL.
#[tauri::command]
pub fn engine_set_property(state: State<'_, AppState>, name: String, value: String) -> CmdResult<()> {
    const DENY: &[&str] = &["wid", "vo", "input-ipc-server", "script", "scripts", "config-dir", "ytdl", "load-scripts"];
    if DENY.contains(&name.as_str()) || name.starts_with("input-") {
        return Err(format!("property '{name}' cannot be changed from the UI"));
    }
    state.engine.set_property(&name, &value).map_err(err)
}

#[tauri::command]
pub fn engine_command(state: State<'_, AppState>, args: Vec<String>) -> CmdResult<()> {
    const ALLOW: &[&str] = &[
        "seek",
        "frame-step",
        "cycle",
        "set",
        "add",
        "multiply",
        "show-text",
        "stop",
        "screenshot-to-file",
        "audio-reload",
        "video-reload",
    ];
    let first = args.first().map(String::as_str).unwrap_or("");
    if !ALLOW.contains(&first) {
        return Err(format!("command '{first}' is not allowed from the UI"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    state.engine.command(&refs).map_err(err)
}

// ---------- license ----------

fn license_state(state: &AppState) -> CmdResult<LicenseStateResponse> {
    let trial = state.db.trial_started_at().map_err(err)?;
    let token = state.db.license_token().map_err(err)?;
    Ok(app_core::license::compute_state(now_unix(), trial, token.as_deref(), &state.machine_guid))
}

#[tauri::command]
pub fn get_license_state(state: State<'_, AppState>) -> CmdResult<LicenseStateResponse> {
    license_state(&state)
}

#[tauri::command]
pub fn activate_license(state: State<'_, AppState>, cmd: ValidateLicenseCommand) -> CmdResult<LicenseStateResponse> {
    // The UI passes the guid it was shown; we still validate against *our* guid.
    let _ = cmd.machine_guid;
    match app_core::license::verify_token(&cmd.license_key, &state.machine_guid) {
        Some(_) => {
            state.db.set_license_token(cmd.license_key.trim()).map_err(err)?;
            license_state(&state)
        }
        None => Err("This license key is not valid for this machine.".into()),
    }
}

#[tauri::command]
pub fn get_machine_guid(state: State<'_, AppState>) -> CmdResult<String> {
    Ok(state.machine_guid.clone())
}
