//! DVR: recordings (single-connection tap on the watched channel, raw HTTP tee or headless mpv
//! for other channels), the scheduler, and VOD downloads with resume (CLAUDE.md §6.4).

use crate::state::AppState;
use app_core::redact::redact;
use app_db::recordings::{DownloadRecord, RecordingRecord};
use app_engine::{EngineOptions, PlayerEngine};
use app_net::downloader::{download_with_resume, free_space};
use app_net::recorder::{start_recording, RecordControl};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

pub const EV_DVR: &str = "dvr_event";
pub const DEFAULT_EXTRA_END_S: i64 = 600;
const MAX_CONCURRENT_DOWNLOADS: usize = 2;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DvrEvent {
    RecordingStarted { id: i64, mode: String },
    RecordingProgress { id: i64, bytes: u64 },
    RecordingStopped { id: i64, status: String, bytes: u64, error: Option<String> },
    DownloadProgress { id: i64, bytes_done: u64, bytes_total: Option<u64> },
    DownloadDone { id: i64, status: String, error: Option<String> },
}

enum JobKind {
    /// mpv `stream-record` on the live engine (same connection as playback).
    Tap,
    /// Raw HTTP tee to disk (extra connection).
    Raw(RecordControl, tokio::task::JoinHandle<app_net::Result<app_net::recorder::RecordOutcome>>),
    /// Headless mpv instance (HLS sources; extra connection).
    Headless(Arc<dyn PlayerEngine>),
}

struct RunningJob {
    kind: JobKind,
    #[allow(dead_code)]
    channel_id: i64,
    stream_url: String,
    user_agent: Option<String>,
    path: PathBuf,
    stop_at: i64,
}

#[derive(Default)]
pub struct DvrState {
    jobs: Mutex<HashMap<i64, RunningJob>>,
    downloads: Mutex<HashMap<i64, watch::Sender<bool>>>,
    active_downloads: Mutex<usize>,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn safe_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    cleaned.trim().chars().take(80).collect()
}

/// Where recordings/downloads go: `<Videos>/desktop-iptv/…` when a Videos folder exists, else app data.
pub fn media_dir(app: &AppHandle, state: &AppState, sub: &str) -> PathBuf {
    let base = state
        .db
        .get_setting("media_dir")
        .ok()
        .flatten()
        .map(PathBuf::from)
        .or_else(|| app.path().video_dir().ok().map(|v| v.join("desktop-iptv")))
        .unwrap_or_else(|| state.data_dir.clone());
    let dir = base.join(sub);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn dvr(app: &AppHandle) -> tauri::State<'_, DvrState> {
    app.state::<DvrState>()
}

// ---------- recordings ----------

/// Start a recording job for a row in status `scheduled`/`recording`.
pub fn start_job(app: &AppHandle, state: &AppState, rec: &RecordingRecord) -> Result<String, String> {
    let dvr = dvr(app);
    if dvr.jobs.lock().unwrap().contains_key(&rec.id) {
        return Ok("running".into());
    }
    let ch = state.db.get_channel(rec.channel_id).map_err(|e| e.to_string())?;
    let src = state.db.playlist_source(ch.playlist_id).map_err(|e| e.to_string())?;
    let stop_at = rec.stop + rec.extra_end_s;
    let path = PathBuf::from(&rec.path);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let (watching_this, playing_url) = {
        let pb = state.playback.lock().unwrap();
        (pb.channel_id() == Some(rec.channel_id), pb.stream_url.clone())
    };
    let engine_can_tap = state.engine.kind() == "mpv" || state.engine.kind() == "stub";
    let is_stalker = ch.stream_url.starts_with(app_net::adapters::STALKER_SCHEME);
    // Stalker links are tokenised at play time: while the channel is playing we know the real URL
    // (and can tap or continue on it); a scheduled job for an idle Stalker channel cannot start.
    let stream_url = if is_stalker {
        match (&playing_url, watching_this) {
            (Some(u), true) => u.clone(),
            _ => return Err("Stalker channels can only be recorded while they are playing (links expire)".into()),
        }
    } else {
        ch.stream_url.clone()
    };
    let mode;
    let kind = if watching_this && engine_can_tap && state.engine.record_path().is_none() {
        state.engine.set_record(path.to_str()).map_err(|e| e.to_string())?;
        mode = "tap";
        JobKind::Tap
    } else if stream_url.split('?').next().unwrap_or("").ends_with(".m3u8") {
        // HLS: a raw GET would only fetch the playlist; use a headless mpv with stream-record.
        let engine = app_engine::create_engine(EngineOptions {
            wid: None,
            hw_decoding: "no".into(),
            default_profile: app_core::ProfileMode::Stable,
            stable_cache_secs: 20,
            audio_boost: 0,
            audio_delay_ms: 0,
            log_level: "warn".into(),
            libmpv_path: None,
            user_agent: src.ua.clone().or_else(|| Some(app_net::http::DEFAULT_USER_AGENT.into())),
        });
        if engine.kind() != "mpv" {
            return Err("HLS recording needs libmpv (not available)".into());
        }
        engine.set_record(path.to_str()).map_err(|e| e.to_string())?;
        engine.load(&stream_url, app_core::ProfileMode::Stable, 0, None).map_err(|e| e.to_string())?;
        mode = "headless";
        JobKind::Headless(engine)
    } else {
        let deadline = tokio::time::Instant::now() + Duration::from_secs((stop_at - now_unix()).max(1) as u64);
        let app2 = app.clone();
        let id = rec.id;
        // Callers may be on the main thread (sync commands, engine callbacks): enter the runtime.
        let _rt = tauri::async_runtime::handle();
        let _guard = _rt.inner().enter();
        let (control, join) = start_recording(
            stream_url.clone(),
            src.ua.clone(),
            path.clone(),
            deadline,
            Arc::new(move |bytes| {
                let _ = app2.emit(EV_DVR, DvrEvent::RecordingProgress { id, bytes });
            }),
        );
        mode = "raw";
        JobKind::Raw(control, join)
    };

    let _ = state.db.set_recording_status(rec.id, "recording", None, None);
    dvr.jobs.lock().unwrap().insert(
        rec.id,
        RunningJob { kind, channel_id: rec.channel_id, stream_url, user_agent: src.ua.clone(), path, stop_at },
    );
    let _ = app.emit(EV_DVR, DvrEvent::RecordingStarted { id: rec.id, mode: mode.into() });
    tracing::info!(id = rec.id, mode, channel = ch.name, "recording started");
    Ok(mode.into())
}

/// Stop a running job; `status` is what the row becomes ("completed" | "failed").
pub fn stop_job(app: &AppHandle, state: &AppState, id: i64, status: &str, error: Option<String>) {
    let job = dvr(app).jobs.lock().unwrap().remove(&id);
    let Some(job) = job else { return };
    match job.kind {
        JobKind::Tap => {
            if state.engine.record_path().as_deref() == job.path.to_str() {
                let _ = state.engine.set_record(None);
            }
        }
        JobKind::Raw(control, _join) => control.stop(),
        JobKind::Headless(engine) => {
            let _ = engine.set_record(None);
            let _ = engine.stop();
            engine.shutdown();
        }
    }
    // The muxer closes asynchronously; measure after a short grace period.
    let app2 = app.clone();
    let path = job.path.clone();
    let status = status.to_string();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let bytes = file_size(&path);
        let final_status = if status == "completed" && bytes == 0 { "failed" } else { status.as_str() };
        let err = if final_status == "failed" && error.is_none() && bytes == 0 {
            Some("no data was written".to_string())
        } else {
            error.clone()
        };
        if let Some(state) = app2.try_state::<AppState>() {
            let _ = state.db.set_recording_status(id, final_status, Some(bytes as i64), err.as_deref());
        }
        let _ = app2.emit(EV_DVR, DvrEvent::RecordingStopped { id, status: final_status.into(), bytes, error: err });
        tracing::info!(id, status = final_status, bytes, "recording stopped");
    });
}

/// The main engine is about to load a different stream: any tap job must move to a raw tee so
/// the recording continues (appending to the same file) on its own connection.
pub fn on_stream_change(app: &AppHandle, state: &AppState) {
    let dvr = dvr(app);
    let mut jobs = dvr.jobs.lock().unwrap();
    let tap_ids: Vec<i64> = jobs.iter().filter(|(_, j)| matches!(j.kind, JobKind::Tap)).map(|(id, _)| *id).collect();
    for id in tap_ids {
        let _ = state.engine.set_record(None);
        let job = jobs.get_mut(&id).unwrap();
        if job.stream_url.split('?').next().unwrap_or("").ends_with(".m3u8") {
            tracing::warn!(id, "tap recording of an HLS stream ends because the channel changed");
            let path = job.path.clone();
            jobs.remove(&id);
            drop(jobs);
            let _ = state.db.set_recording_status(
                id,
                "completed",
                Some(file_size(&path) as i64),
                Some("stopped early: channel changed (HLS source)"),
            );
            let _ = app.emit(
                EV_DVR,
                DvrEvent::RecordingStopped {
                    id,
                    status: "completed".into(),
                    bytes: file_size(&path),
                    error: Some("channel changed".into()),
                },
            );
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs((job.stop_at - now_unix()).max(1) as u64);
        let app2 = app.clone();
        let _rt = tauri::async_runtime::handle();
        let _guard = _rt.inner().enter();
        // The recorder counts only what *it* writes; the file already holds the tapped part.
        let base = file_size(&job.path);
        let (control, join) = start_recording(
            job.stream_url.clone(),
            job.user_agent.clone(),
            job.path.clone(),
            deadline,
            Arc::new(move |bytes| {
                let _ = app2.emit(EV_DVR, DvrEvent::RecordingProgress { id, bytes: base + bytes });
            }),
        );
        job.kind = JobKind::Raw(control, join);
        tracing::info!(id, "tap recording continued on its own connection");
    }
}

/// Number of provider connections we hold beyond the main player (recordings on their own
/// connection + multiscreen panes). Used for the connection-aware warning (CLAUDE.md §6.5).
pub fn extra_connections(app: &AppHandle) -> usize {
    dvr(app).jobs.lock().unwrap().values().filter(|j| !matches!(j.kind, JobKind::Tap)).count()
}

/// Scheduler tick: start due jobs, stop finished ones, expire missed ones.
pub fn tick(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let now = now_unix();
    let _ = state.db.expire_missed_recordings(now);
    if let Ok(due) = state.db.due_recordings(now) {
        for rec in due {
            if let Err(e) = start_job(app, &state, &rec) {
                tracing::warn!(id = rec.id, error = %redact(&e), "could not start scheduled recording");
                let _ = state.db.set_recording_status(rec.id, "failed", None, Some(&e));
            }
        }
    }
    let dvr_state = dvr(app);
    let finished: Vec<(i64, Option<String>)> = {
        let mut jobs = dvr_state.jobs.lock().unwrap();
        let mut out = Vec::new();
        for (id, job) in jobs.iter_mut() {
            if now >= job.stop_at {
                out.push((*id, None));
            } else if let JobKind::Raw(_, join) = &job.kind {
                if join.is_finished() {
                    out.push((*id, Some("the stream ended before the scheduled stop".to_string())));
                }
            }
        }
        out
    };
    for (id, err) in finished {
        stop_job(app, &state, id, "completed", err);
    }
    // Progress for tap/headless jobs (raw jobs report themselves).
    let progress: Vec<(i64, u64)> = dvr_state
        .jobs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, j)| !matches!(j.kind, JobKind::Raw(..)))
        .map(|(id, j)| (*id, file_size(&j.path)))
        .collect();
    for (id, bytes) in progress {
        let _ = app.emit(EV_DVR, DvrEvent::RecordingProgress { id, bytes });
        let _ = state.db.set_recording_status(id, "recording", Some(bytes as i64), None);
    }
}

pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tick(&app);
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

/// Ensure a running job for `channel_id` (if any) is cancelled when the row is deleted.
pub fn cancel_job(app: &AppHandle, state: &AppState, id: i64) {
    stop_job(app, state, id, "failed", Some("cancelled".into()));
}

pub fn recording_file_name(channel_name: &str, title: Option<&str>, start: i64) -> String {
    let ts = {
        // yyyymmdd-hhmm local-ish (UTC is fine for a filename)
        let secs = start;
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!("{y:04}{m:02}{d:02}-{:02}{:02}", rem / 3600, (rem % 3600) / 60)
    };
    let base = match title {
        Some(t) if !t.trim().is_empty() => format!("{} - {}", safe_name(channel_name), safe_name(t)),
        _ => safe_name(channel_name),
    };
    format!("{ts} {base}.ts")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------- downloads ----------

pub fn start_download(
    app: &AppHandle,
    state: &AppState,
    row: &DownloadRecord,
    url: String,
    user_agent: Option<String>,
) {
    let dvr = dvr(app);
    {
        let mut active = dvr.active_downloads.lock().unwrap();
        if dvr.downloads.lock().unwrap().contains_key(&row.id) {
            return;
        }
        if *active >= MAX_CONCURRENT_DOWNLOADS {
            let _ = state.db.update_download(row.id, "queued", None, None, None);
            return;
        }
        *active += 1;
    }
    let (tx, rx) = watch::channel(false);
    dvr.downloads.lock().unwrap().insert(row.id, tx);
    let _ = state.db.update_download(row.id, "downloading", None, None, None);
    let app2 = app.clone();
    let id = row.id;
    let path = PathBuf::from(&row.path);
    tauri::async_runtime::spawn(async move {
        let app3 = app2.clone();
        let progress = Arc::new(move |done: u64, total: Option<u64>| {
            let _ = app3.emit(EV_DVR, DvrEvent::DownloadProgress { id, bytes_done: done, bytes_total: total });
            if let Some(state) = app3.try_state::<AppState>() {
                let _ = state.db.update_download(id, "downloading", Some(done as i64), total.map(|t| t as i64), None);
            }
        });
        let result = download_with_resume(&url, user_agent.as_deref(), &path, progress, rx).await;
        let (status, error) = match result {
            Ok(o) if o.completed => ("completed", None),
            Ok(_) => ("paused", None),
            Err(e) => ("failed", Some(redact(&e.to_string()))),
        };
        if let Some(state) = app2.try_state::<AppState>() {
            let done = if status == "completed" { Some(file_size(&path) as i64) } else { None };
            let _ = state.db.update_download(id, status, done, None, error.as_deref());
        }
        let dvr = app2.state::<DvrState>();
        dvr.downloads.lock().unwrap().remove(&id);
        {
            let mut active = dvr.active_downloads.lock().unwrap();
            *active = active.saturating_sub(1);
        }
        let _ = app2.emit(EV_DVR, DvrEvent::DownloadDone { id, status: status.into(), error });
        // Pull the next queued download.
        if let Some(state) = app2.try_state::<AppState>() {
            resume_queued(&app2, &state);
        }
    });
}

/// Start queued downloads up to the concurrency limit (also on app start).
pub fn resume_queued(app: &AppHandle, state: &AppState) {
    let Ok(rows) = state.db.resumable_downloads() else { return };
    for row in rows {
        if dvr(app).downloads.lock().unwrap().contains_key(&row.id) {
            continue;
        }
        let Some((url, ua)) = download_source(state, &row) else {
            let _ = state.db.update_download(row.id, "failed", None, None, Some("source no longer available"));
            continue;
        };
        start_download(app, state, &row, url, ua);
    }
}

pub fn download_source(state: &AppState, row: &DownloadRecord) -> Option<(String, Option<String>)> {
    let (url, playlist_id) = match row.item_type.as_str() {
        "vod" => {
            let v = state.db.get_vod(row.item_id).ok()?;
            (v.stream_url?, v.playlist_id)
        }
        "episode" => {
            let e = state.db.get_episode(row.item_id).ok()?;
            let series = state.db.get_vod(e.series_id).ok()?;
            (e.stream_url, series.playlist_id)
        }
        _ => return None,
    };
    let ua = state.db.playlist_source(playlist_id).ok().and_then(|p| p.ua);
    Some((url, ua))
}

pub fn pause_download(app: &AppHandle, id: i64) -> bool {
    let dvr = dvr(app);
    let tx = dvr.downloads.lock().unwrap().remove(&id);
    match tx {
        Some(tx) => {
            let _ = tx.send(true);
            true
        }
        None => false,
    }
}

pub fn disk_warning(path: &Path, needed: Option<u64>) -> Option<String> {
    let free = free_space(path)?;
    let need = needed.unwrap_or(14 * 1024 * 1024 * 1024); // 4 h × 8 Mbps ≈ 14 GB (CLAUDE.md §6.4)
    if free < need {
        Some(format!(
            "Low disk space: {:.1} GB free, about {:.1} GB may be needed.",
            free as f64 / 1e9,
            need as f64 / 1e9
        ))
    } else {
        None
    }
}
