//! Tauri command layer — thin by design (CLAUDE.md §2 "app-ui: Tauri commands + events only").
//!
//! Frontend ↔ Rust talks only through the typed contracts in `app-core::ipc` (mirrored in
//! `src/lib/ipc.ts`). No video bytes, no HTTP, no secrets cross this boundary.

mod commands;
mod state;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::Arc;
use tauri::{Emitter, Manager};

pub use state::AppState;

/// Name of the event that carries `app_engine::EngineEvent` payloads to the frontend.
pub const EV_ENGINE: &str = "engine_event";
/// `app_core::ImportProgressEvent`
pub const EV_IMPORT_PROGRESS: &str = "import_progress";
/// `commands::ImportDone`
pub const EV_IMPORT_DONE: &str = "import_done";

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("DESKTOP_IPTV_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,desktop_iptv_lib=debug,app_engine=debug,app_net=info"));
    let _ = fmt().with_env_filter(filter).with_target(true).compact().try_init();
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
        .setup(|app| {
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

            // Engine events → frontend.
            let handle = app.handle().clone();
            engine.set_listener(Arc::new(move |ev| {
                let _ = handle.emit(EV_ENGINE, &ev);
            }));

            app.manage(AppState::new(db, engine, data_dir));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                if let Some(state) = window.try_state::<AppState>() {
                    state.engine.shutdown();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap,
            commands::get_config,
            commands::set_config,
            commands::accept_legal,
            commands::add_playlist,
            commands::refresh_playlist,
            commands::delete_playlist,
            commands::list_playlists,
            commands::list_groups,
            commands::list_channels,
            commands::count_channels,
            commands::search_channels,
            commands::get_channel,
            commands::get_favorites,
            commands::get_favorite_ids,
            commands::set_favorite,
            commands::get_recents,
            commands::play_channel,
            commands::load_stream,
            commands::stop_playback,
            commands::set_profile,
            commands::set_pause,
            commands::set_mute,
            commands::set_volume,
            commands::set_video_rect,
            commands::get_telemetry,
            commands::engine_get_property,
            commands::engine_set_property,
            commands::engine_command,
            commands::get_license_state,
            commands::activate_license,
            commands::get_machine_guid,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
