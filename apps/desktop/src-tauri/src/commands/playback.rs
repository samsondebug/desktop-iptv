//! Playback: channels, VOD, raw URLs; profile/pause/mute/volume/seek; video rect; engine access.

use super::{err, require_feature, CmdResult};
use crate::state::{AppState, PlaybackItem};
use app_core::*;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub const EV_PLAYBACK: &str = "playback_state";

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackState {
    pub item: PlaybackItem,
    pub channel_id: Option<i64>,
    pub stream_url_redacted: Option<String>,
    pub profile: String,
    pub volume: u16,
    pub muted: bool,
    pub paused: bool,
    pub engine_kind: String,
    pub is_vod: bool,
}

pub fn playback_state(state: &AppState) -> PlaybackState {
    let pb = state.playback.lock().unwrap();
    PlaybackState {
        item: pb.item.clone(),
        channel_id: pb.channel_id(),
        stream_url_redacted: pb.stream_url.as_deref().map(app_core::redact::redact),
        profile: pb.profile.as_str().into(),
        volume: pb.volume,
        muted: pb.muted,
        paused: pb.paused,
        engine_kind: state.engine.kind().into(),
        is_vod: pb.is_vod(),
    }
}

/// Emit the playback state (used when the backend changes it on its own, e.g. auto-next).
pub fn emit_playback(app: &AppHandle, state: &AppState) {
    let _ = app.emit(EV_PLAYBACK, playback_state(state));
}

pub fn do_load(
    state: &AppState,
    url: &str,
    profile: ProfileMode,
    item: PlaybackItem,
    start_secs: Option<f64>,
) -> CmdResult<()> {
    // A tap recording follows the engine's stream: hand it its own connection first.
    if state.engine.record_path().is_some() {
        if let Some(app) = crate::APP.get() {
            crate::dvr::on_stream_change(app, state);
        }
    }
    let boost = {
        let pb = state.playback.lock().unwrap();
        if pb.muted {
            0
        } else {
            pb.volume
        }
    };
    state.engine.load(url, profile, boost, start_secs).map_err(err)?;
    let mut pb = state.playback.lock().unwrap();
    pb.stream_url = Some(url.to_string());
    pb.profile = profile;
    pb.paused = false;
    pb.item = item;
    pb.progress_saved_at = None;
    pb.last_saved_pos = start_secs.unwrap_or(0.0) as i64;
    pb.user_stopped = false;
    pb.generation = pb.generation.wrapping_add(1);
    Ok(())
}

/// Maximum consecutive automatic reconnects for a live stream before giving up.
pub const MAX_RECONNECTS: u32 = 10;

/// Backoff for the n-th consecutive reconnect (1-based): 1 s, 2 s, 4 s, 8 s, then 15 s.
pub fn reconnect_delay(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs((1u64 << attempt.saturating_sub(1).min(3)).min(15))
}

/// A live stream ended (EOF or error) without the user asking for it: reload the same URL after
/// a backoff, keeping the item and profile. Returns the attempt number, or `None` when there is
/// nothing to retry (VOD, user stop, too many attempts).
pub fn schedule_reconnect(app: &AppHandle, state: &AppState, why: &str) -> Option<u32> {
    let (url, item, profile, attempt, generation) = {
        let mut pb = state.playback.lock().unwrap();
        if !pb.is_live() || pb.user_stopped {
            return None;
        }
        let url = pb.stream_url.clone()?;
        pb.reconnect_attempts += 1;
        if pb.reconnect_attempts > MAX_RECONNECTS {
            return None;
        }
        (url, pb.item.clone(), pb.profile, pb.reconnect_attempts, pb.generation)
    };
    let delay = reconnect_delay(attempt);
    tracing::warn!(attempt, delay_ms = delay.as_millis() as u64, why, "live stream ended — reconnecting");
    let _ = app.emit(
        EV_NOTICE,
        PlaybackNotice {
            kind: "reconnecting".into(),
            message: format!("Stream ended ({why}) — reconnecting, attempt {attempt}/{MAX_RECONNECTS}"),
            attempt,
        },
    );
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        let Some(state) = app2.try_state::<AppState>() else { return };
        {
            let pb = state.playback.lock().unwrap();
            // The user zapped, stopped, or something else reloaded meanwhile: stand down.
            if pb.generation != generation || pb.user_stopped || pb.item != item {
                return;
            }
        }
        match do_load(&state, &url, profile, item, None) {
            Ok(()) => {
                // Keep the attempt counter across this reload; PlaybackStarted resets it.
                state.playback.lock().unwrap().reconnect_attempts = attempt;
                emit_playback(&app2, &state);
            }
            Err(e) => tracing::warn!(error = %e, "reconnect load failed"),
        }
    });
    Some(attempt)
}

pub const EV_NOTICE: &str = "playback_notice";

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackNotice {
    pub kind: String,
    pub message: String,
    pub attempt: u32,
}

#[tauri::command]
pub fn get_playback_state(state: State<'_, AppState>) -> CmdResult<PlaybackState> {
    Ok(playback_state(&state))
}

/// Stalker channels store a portal `cmd`, not a URL: exchange it for a tokenised link now.
/// Everything else passes through untouched.
pub async fn resolve_stream_url(state: &AppState, ch: &ChannelRecord) -> CmdResult<String> {
    if !ch.stream_url.starts_with(app_net::adapters::STALKER_SCHEME) {
        return Ok(ch.stream_url.clone());
    }
    let adapter = app_net::adapters::StalkerAdapter::from_playlist(
        state.db.clone(),
        ch.playlist_id,
        std::sync::Arc::new(app_net::importer::NoopSink),
    )
    .map_err(err)?;
    let url = adapter.client.create_link(&ch.stream_url).await.map_err(err)?;
    tracing::info!(channel = %ch.name, url = %app_core::redact::redact(&url), "stalker link resolved");
    Ok(url)
}

/// Click on a channel → `LoadStreamCommand` → `loadfile replace` (CLAUDE.md §15 step 6).
#[tauri::command]
pub async fn play_channel(state: State<'_, AppState>, channel_id: i64) -> CmdResult<PlaybackState> {
    let ch = state.db.get_channel(channel_id).map_err(err)?;
    let url = resolve_stream_url(&state, &ch).await?;
    let profile = state.playback.lock().unwrap().profile;
    do_load(&state, &url, profile, PlaybackItem::Channel { id: channel_id }, None)?;
    let _ = state.db.touch_recent(channel_id);
    let _ = state.db.set_last_channel_id(channel_id);
    Ok(playback_state(&state))
}

/// Play a movie, resuming from saved progress unless `from_start`.
#[tauri::command]
pub fn play_vod(state: State<'_, AppState>, vod_id: i64, from_start: bool) -> CmdResult<PlaybackState> {
    let v = state.db.get_vod(vod_id).map_err(err)?;
    let url = v.stream_url.clone().ok_or("This title has no stream URL (series? open an episode)")?;
    let start = if from_start {
        None
    } else {
        state.db.get_progress("vod", vod_id).map_err(err)?.filter(|p| !p.finished).map(|p| p.position_s as f64)
    };
    // VOD always uses the stable profile: nothing to be "live" about.
    do_load(&state, &url, ProfileMode::Stable, PlaybackItem::Vod { id: vod_id }, start)?;
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn play_episode(state: State<'_, AppState>, episode_id: i64, from_start: bool) -> CmdResult<PlaybackState> {
    let e = state.db.get_episode(episode_id).map_err(err)?;
    let start = if from_start {
        None
    } else {
        state.db.get_progress("episode", episode_id).map_err(err)?.filter(|p| !p.finished).map(|p| p.position_s as f64)
    };
    do_load(
        &state,
        &e.stream_url,
        ProfileMode::Stable,
        PlaybackItem::Episode { id: episode_id, series_id: e.series_id },
        start,
    )?;
    Ok(playback_state(&state))
}

/// Raw stream URL (Diagnostics / "open URL" box). Not tied to a catalog row.
#[tauri::command]
pub fn load_stream(state: State<'_, AppState>, cmd: LoadStreamCommand) -> CmdResult<PlaybackState> {
    let profile = ProfileMode::parse(&cmd.profile_mode).unwrap_or(state.playback.lock().unwrap().profile);
    state.playback.lock().unwrap().volume = cmd.audio_boost.clamp(0, 130);
    do_load(&state, &cmd.stream_url, profile, PlaybackItem::Url, None)?;
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn stop_playback(state: State<'_, AppState>) -> CmdResult<PlaybackState> {
    save_progress_now(&state);
    state.engine.stop().map_err(err)?;
    let mut pb = state.playback.lock().unwrap();
    pb.item = PlaybackItem::None;
    pb.stream_url = None;
    pb.user_stopped = true;
    pb.generation = pb.generation.wrapping_add(1);
    drop(pb);
    Ok(playback_state(&state))
}

/// Persist VOD progress immediately (called on stop / switch / exit).
pub fn save_progress_now(state: &AppState) {
    let (item, pos, dur) = {
        let pb = state.playback.lock().unwrap();
        let t = state.engine.telemetry();
        (pb.item.clone(), t.time_pos_s, t.duration_s)
    };
    let dur = if dur > 0.0 { Some(dur as i64) } else { None };
    match item {
        PlaybackItem::Vod { id } if pos > 0.0 => {
            let _ = state.db.set_progress("vod", id, pos as i64, dur);
        }
        PlaybackItem::Episode { id, .. } if pos > 0.0 => {
            let _ = state.db.set_progress("episode", id, pos as i64, dur);
        }
        _ => {}
    }
}

/// Toggle low_latency / stable. Reloads the current stream so startup-only options apply.
#[tauri::command]
pub fn set_profile(state: State<'_, AppState>, profile: String) -> CmdResult<PlaybackState> {
    let p = ProfileMode::parse(&profile).ok_or_else(|| "unknown profile".to_string())?;
    state.engine.set_profile(p).map_err(err)?;
    let (url, item, is_vod) = {
        let mut pb = state.playback.lock().unwrap();
        pb.profile = p;
        (pb.stream_url.clone(), pb.item.clone(), pb.is_vod())
    };
    if let Some(url) = url {
        if is_vod {
            // Keep position across the reload.
            let pos = state.engine.telemetry().time_pos_s;
            do_load(&state, &url, p, item, if pos > 1.0 { Some(pos) } else { None })?;
        } else {
            do_load(&state, &url, p, item, None)?;
        }
    }
    Ok(playback_state(&state))
}

#[tauri::command]
pub fn set_pause(state: State<'_, AppState>, paused: bool) -> CmdResult<PlaybackState> {
    state.engine.set_pause(paused).map_err(err)?;
    state.playback.lock().unwrap().paused = paused;
    if paused {
        save_progress_now(&state);
    }
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

#[tauri::command]
pub fn seek(state: State<'_, AppState>, secs: f64) -> CmdResult<()> {
    if !state.playback.lock().unwrap().is_vod() {
        return Err("Seeking is only available for movies and series".into());
    }
    state.engine.seek(secs).map_err(err)
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
    const DENY: &[&str] =
        &["wid", "vo", "input-ipc-server", "script", "scripts", "config-dir", "ytdl", "load-scripts", "stream-record"];
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
    if first == "set" && args.get(1).map(|s| s == "stream-record").unwrap_or(false) {
        return Err("recording is controlled by the Record button".into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    state.engine.command(&refs).map_err(err)
}

/// Audio/subtitle track lists straight from mpv (`track-list` JSON).
#[tauri::command]
pub fn engine_tracks(state: State<'_, AppState>) -> CmdResult<serde_json::Value> {
    let raw = state.engine.get_property("track-list").map_err(err)?.unwrap_or_else(|| "[]".into());
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn engine_select_track(state: State<'_, AppState>, kind: String, id: String) -> CmdResult<()> {
    let prop = match kind.as_str() {
        "audio" => "aid",
        "sub" => "sid",
        "video" => "vid",
        _ => return Err("unknown track kind".into()),
    };
    state.engine.set_property(prop, &id).map_err(err)
}

/// External-player escape hatch (CLAUDE.md §13). The URL (with credentials) is handed to the
/// OS handler from Rust; it never crosses into the webview.
#[tauri::command]
pub fn open_in_external_player(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    require_feature(&state, app_core::license::Feature::LiveTv, "External player")?;
    let url = state.playback.lock().unwrap().stream_url.clone().ok_or("Nothing is playing")?;
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>).map_err(err)
}
