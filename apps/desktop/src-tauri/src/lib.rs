//! Tauri command layer — thin by design (CLAUDE.md §2 "app-ui: Tauri commands + events only").
//!
//! Frontend ↔ Rust talks only through the typed contracts in `app-core::ipc` (mirrored in
//! `src/lib/ipc.ts`). No video bytes, no HTTP, no secrets cross this boundary.

mod commands;
mod dvr;
mod state;
mod sync;

use app_engine::EngineEvent;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

pub use state::AppState;

/// Process-wide app handle for callbacks that only receive `&AppState`.
pub static APP: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// Name of the event that carries `app_engine::EngineEvent` payloads to the frontend.
pub const EV_ENGINE: &str = "engine_event";
/// `sync::ProgressPayload` (ImportProgressEvent + phase)
pub const EV_IMPORT_PROGRESS: &str = "import_progress";
/// `sync::ImportDone`
pub const EV_IMPORT_DONE: &str = "import_done";

/// Where the log file lives: `<app data dir>/sktv.log` (same folder as the catalog; the folder
/// keeps its pre-rename name `dev.desktopiptv.app` — that is the bundle identifier).
/// Truncated at every start so a support report is always "this run". Every line already went
/// through `redact` at the call site.
pub fn log_path() -> Option<std::path::PathBuf> {
    let dir = dirs::data_dir()?.join("dev.desktopiptv.app");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("sktv.log"))
}

fn init_tracing() {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("DESKTOP_IPTV_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,desktop_iptv_lib=debug,app_engine=debug,app_net=info"));
    let file = log_path().and_then(|p| std::fs::File::create(p).ok());
    match file {
        Some(f) => {
            let writer = std::io::stderr.and(std::sync::Mutex::new(f));
            let _ = fmt()
                .with_env_filter(filter)
                .with_target(true)
                .with_ansi(false)
                .compact()
                .with_writer(writer)
                .try_init();
        }
        None => {
            let _ = fmt().with_env_filter(filter).with_target(true).compact().try_init();
        }
    }
}

/// Native surface id for libmpv's `wid` from the main window handle.
fn window_id(window: &tauri::WebviewWindow) -> Option<i64> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as i64),
        RawWindowHandle::Xlib(h) => Some(h.window as i64),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as i64),
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as i64),
        RawWindowHandle::Wayland(_) => {
            tracing::warn!("Wayland surface: libmpv `wid` embedding is unavailable; set GDK_BACKEND=x11");
            None
        }
        _ => None,
    }
}

/// Engine events → frontend, plus the backend-side reactions (VOD progress, auto-next).
fn on_engine_event(app: &tauri::AppHandle, ev: EngineEvent) {
    let _ = app.emit(EV_ENGINE, &ev);
    let Some(state) = app.try_state::<AppState>() else { return };
    match ev {
        EngineEvent::Telemetry(t) => {
            // Persist VOD progress every ~5 s while playing.
            let due = {
                let mut pb = state.playback.lock().unwrap();
                if !pb.is_vod() || t.time_pos_s < 1.0 {
                    false
                } else {
                    let due = pb.progress_saved_at.map(|at| at.elapsed() >= Duration::from_secs(5)).unwrap_or(true)
                        && (t.time_pos_s as i64 - pb.last_saved_pos).abs() >= 3;
                    if due {
                        pb.progress_saved_at = Some(Instant::now());
                        pb.last_saved_pos = t.time_pos_s as i64;
                    }
                    due
                }
            };
            if due {
                commands::playback::save_progress_now(&state);
            }
        }
        EngineEvent::PlaybackStarted { .. } => {
            state.playback.lock().unwrap().reconnect_attempts = 0;
        }
        // Live TV: an end-of-file or a decode/network error is a provider hiccup — reconnect.
        EngineEvent::EndFile { ref reason, ref error, .. } if state.playback.lock().unwrap().is_live() => {
            let why = error.clone().unwrap_or_else(|| reason.clone());
            if commands::playback::schedule_reconnect(app, &state, &why).is_none() {
                tracing::warn!(reason, ?error, "live stream ended; not reconnecting");
            }
        }
        EngineEvent::EndFile { reason, .. } if reason == "eof" => {
            // Auto-next episode (CLAUDE.md §8 "Continue watching + auto-next episode").
            let item = state.playback.lock().unwrap().item.clone();
            if let state::PlaybackItem::Episode { id, .. } = item {
                let _ = state.db.set_progress("episode", id, i64::MAX / 4, Some(1)); // mark finished
                if let Ok(Some(next)) = state.db.next_episode(id) {
                    tracing::info!(episode = next.id, "auto-next");
                    let start = state
                        .db
                        .get_progress("episode", next.id)
                        .ok()
                        .flatten()
                        .filter(|p| !p.finished)
                        .map(|p| p.position_s as f64);
                    if commands::playback::do_load(
                        &state,
                        &next.stream_url,
                        app_core::ProfileMode::Stable,
                        state::PlaybackItem::Episode { id: next.id, series_id: next.series_id },
                        start,
                    )
                    .is_ok()
                    {
                        commands::playback::emit_playback(app, &state);
                    }
                }
            } else if let state::PlaybackItem::Vod { id } = item {
                let _ = state.db.set_progress("vod", id, i64::MAX / 4, Some(1));
            }
        }
        _ => {}
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    // libmpv embeds via X11 window ids; force the X11 backend under Wayland sessions (XWayland).
    #[cfg(all(unix, not(target_os = "macos")))]
    if std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Self-updater: signed `latest.json` on GitHub Releases (see docs/RELEASING.md). Desktop
            // only; the check itself runs from the webview so the UI owns the prompt and progress.
            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
            let _ = APP.set(app.handle().clone());
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("catalog.db");
            tracing::info!(path = %db_path.display(), "opening catalog");
            let db = Arc::new(app_db::Db::open(&db_path)?);
            let config = db.load_config()?;

            let window = app.get_webview_window("main").expect("main window exists");
            let wid = window_id(&window);

            let engine = app_engine::create_engine(app_engine::EngineOptions {
                wid,
                hw_decoding: config.hw_decoding.clone(),
                default_profile: app_core::ProfileMode::parse(&config.default_profile).unwrap_or_default(),
                stable_cache_secs: config.stable_cache_secs,
                audio_boost: config.audio_boost,
                audio_delay_ms: config.audio_delay_ms,
                log_level: std::env::var("DESKTOP_IPTV_MPV_LOG").unwrap_or_else(|_| "warn".into()),
                libmpv_path: None,
                user_agent: Some(app_net::http::DEFAULT_USER_AGENT.to_string()),
            });

            let handle = app.handle().clone();
            engine.set_listener(Arc::new(move |ev| on_engine_event(&handle, ev)));

            let state = AppState::new(db.clone(), engine, data_dir);
            let _ = commands::parental::apply_filter(&state);
            app.manage(state);
            app.manage(dvr::DvrState::default());

            // Housekeeping: recordings that were scheduled while the app was closed, interrupted
            // downloads, then the DVR scheduler loop.
            let _ = db.expire_missed_recordings(commands::now_unix());
            if let Ok(n) = db.fail_interrupted_recordings() {
                if n > 0 {
                    tracing::warn!(n, "recordings interrupted by a previous shutdown marked failed");
                }
            }
            if let Some(state) = app.try_state::<AppState>() {
                dvr::resume_queued(app.handle(), &state);
            }
            dvr::spawn_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() != "main" {
                    return;
                }
                if let Some(state) = window.try_state::<AppState>() {
                    commands::playback::save_progress_now(&state);
                    for (_, p) in state.panes.lock().unwrap().drain() {
                        p.engine.shutdown();
                    }
                    state.engine.shutdown();
                }
                // Closing the main window closes the app (panes included).
                for (_, w) in window.app_handle().webview_windows() {
                    let _ = w.close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // backup
            commands::backup_export,
            commands::backup_inspect,
            commands::backup_import,
            // diagnostics
            commands::diag_http_trace,
            commands::diag_clear_trace,
            commands::diag_probe,
            commands::diag_check_source,
            commands::diag_report,
            // bootstrap / config / license / theme
            commands::get_bootstrap,
            commands::get_config,
            commands::set_config,
            commands::accept_legal,
            commands::get_license_state,
            commands::activate_license,
            commands::get_machine_guid,
            commands::get_theme_tokens,
            commands::set_theme_tokens,
            // playlists / catalog
            commands::add_playlist,
            commands::refresh_playlist,
            commands::delete_playlist,
            commands::rename_playlist,
            commands::list_playlists,
            commands::playlist_meta,
            commands::set_stream_format,
            commands::is_syncing,
            commands::list_groups,
            commands::list_channels,
            commands::count_channels,
            commands::search_channels,
            commands::get_channel,
            commands::get_favorites,
            commands::get_favorite_ids,
            commands::set_favorite,
            commands::get_recents,
            // playback
            commands::get_playback_state,
            commands::play_channel,
            commands::play_vod,
            commands::play_episode,
            commands::load_stream,
            commands::stop_playback,
            commands::set_profile,
            commands::set_pause,
            commands::set_mute,
            commands::set_volume,
            commands::seek,
            commands::set_video_rect,
            commands::get_telemetry,
            commands::engine_get_property,
            commands::engine_set_property,
            commands::engine_command,
            commands::engine_tracks,
            commands::engine_select_track,
            commands::open_in_external_player,
            // epg
            commands::epg_grid,
            commands::epg_now_next,
            commands::epg_stats,
            commands::set_epg_offset,
            commands::set_epg_override,
            commands::get_epg_override,
            commands::epg_search_ids,
            commands::list_epg_sources,
            commands::add_epg_source,
            commands::delete_epg_source,
            commands::refresh_epg,
            // vod
            commands::list_vod,
            commands::count_vod,
            commands::vod_groups,
            commands::search_vod,
            commands::get_vod,
            commands::series_detail,
            commands::continue_watching,
            commands::get_progress,
            commands::clear_progress,
            commands::refresh_vod,
            // parental
            commands::parental_status,
            commands::set_parental_pin,
            commands::clear_parental_pin,
            commands::unlock_parental,
            commands::lock_parental,
            commands::set_parental_keywords,
            // dvr
            commands::record_now,
            commands::schedule_recording,
            commands::stop_recording,
            commands::list_recordings,
            commands::delete_recording,
            commands::play_recording,
            commands::connection_budget,
            commands::download_item,
            commands::list_downloads,
            commands::pause_download,
            commands::resume_download,
            commands::delete_download,
            commands::play_download,
            commands::media_dir,
            commands::set_media_dir,
            // windows
            commands::set_mini_mode,
            commands::is_mini_mode,
            commands::list_panes,
            commands::pane_open,
            commands::pane_play,
            commands::pane_stop,
            commands::pane_close,
            commands::pane_audio,
            commands::pane_set_video_rect,
            commands::pane_telemetry,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
