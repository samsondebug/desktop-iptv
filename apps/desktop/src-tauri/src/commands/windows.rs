//! Mini (PiP) mode with persisted geometry, and multiscreen panes (CLAUDE.md §1 gaps, §6.5).

use super::{err, require_feature, CmdResult};
use crate::state::{AppState, Pane};
use app_core::license::Feature;
use app_engine::{EngineEvent, EngineOptions};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

const KEY_MAIN_GEOMETRY: &str = "main_geometry";
const KEY_PIP_GEOMETRY: &str = "pip_geometry";

fn read_geometry(window: &tauri::WebviewWindow) -> Option<Geometry> {
    let scale = window.scale_factor().ok()?;
    let pos = window.outer_position().ok()?.to_logical::<f64>(scale);
    // Some X11 setups (no window manager, headless) report a 0×0 outer size; the inner size is
    // always real, so prefer it whenever the outer one is unusable.
    let mut size = window.outer_size().ok()?.to_logical::<f64>(scale);
    if size.width < 1.0 || size.height < 1.0 {
        size = window.inner_size().ok()?.to_logical::<f64>(scale);
    }
    let g = Geometry { x: pos.x, y: pos.y, w: size.width, h: size.height };
    g.is_sane().then_some(g)
}

impl Geometry {
    /// Rejects the degenerate values a headless X server or a minimised window can produce.
    fn is_sane(&self) -> bool {
        self.w >= 200.0
            && self.h >= 120.0
            && self.w <= 16384.0
            && self.h <= 16384.0
            && self.x.is_finite()
            && self.y.is_finite()
    }
}

fn load_geometry(state: &AppState, key: &str) -> Option<Geometry> {
    state
        .db
        .get_setting(key)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Geometry>(&j).ok())
        .filter(Geometry::is_sane)
}

fn save_geometry(state: &AppState, key: &str, g: &Geometry) {
    let _ = state.db.set_setting(key, &serde_json::to_string(g).unwrap());
}

/// Toggle mini mode: a small, frameless, always-on-top player. Geometry is remembered per mode.
#[tauri::command]
pub fn set_mini_mode(app: AppHandle, state: State<'_, AppState>, on: bool) -> CmdResult<bool> {
    let window = app.get_webview_window("main").ok_or("main window missing")?;
    let mut mini = state.mini_mode.lock().unwrap();
    if on == *mini {
        return Ok(on);
    }
    if on {
        if let Some(g) = read_geometry(&window) {
            save_geometry(&state, KEY_MAIN_GEOMETRY, &g);
        }
        let pip = load_geometry(&state, KEY_PIP_GEOMETRY).unwrap_or_else(|| {
            // Bottom-right of the current monitor by default.
            let (mw, mh) = window
                .current_monitor()
                .ok()
                .flatten()
                .map(|m| {
                    let s = m.scale_factor();
                    let sz = m.size().to_logical::<f64>(s);
                    (sz.width, sz.height)
                })
                .unwrap_or((1920.0, 1080.0));
            Geometry { x: mw - 480.0 - 24.0, y: mh - 270.0 - 80.0, w: 480.0, h: 270.0 }
        });
        let _ = window.set_fullscreen(false);
        let _ = window.unmaximize();
        window.set_decorations(false).map_err(err)?;
        window.set_always_on_top(true).map_err(err)?;
        window.set_min_size(Some(LogicalSize::new(240.0, 135.0))).map_err(err)?;
        window.set_size(LogicalSize::new(pip.w, pip.h)).map_err(err)?;
        window.set_position(LogicalPosition::new(pip.x, pip.y)).map_err(err)?;
    } else {
        if let Some(g) = read_geometry(&window) {
            save_geometry(&state, KEY_PIP_GEOMETRY, &g);
        }
        window.set_always_on_top(false).map_err(err)?;
        window.set_decorations(true).map_err(err)?;
        window.set_min_size(Some(LogicalSize::new(1024.0, 640.0))).map_err(err)?;
        if let Some(g) = load_geometry(&state, KEY_MAIN_GEOMETRY) {
            let _ = window.set_size(LogicalSize::new(g.w, g.h));
            let _ = window.set_position(LogicalPosition::new(g.x, g.y));
        } else {
            let _ = window.set_size(LogicalSize::new(1440.0, 900.0));
            let _ = window.center();
        }
    }
    *mini = on;
    Ok(on)
}

#[tauri::command]
pub fn is_mini_mode(state: State<'_, AppState>) -> bool {
    *state.mini_mode.lock().unwrap()
}

// ---------- multiscreen panes ----------

#[derive(Debug, Clone, Serialize)]
pub struct PaneInfo {
    pub label: String,
    pub playing: bool,
    pub channel_id: Option<i64>,
    pub has_audio: bool,
}

fn pane_infos(state: &AppState) -> Vec<PaneInfo> {
    let mut v: Vec<PaneInfo> = state
        .panes
        .lock()
        .unwrap()
        .iter()
        .map(|(label, p)| PaneInfo {
            label: label.clone(),
            playing: p.playing,
            channel_id: p.channel_id,
            has_audio: p.has_audio,
        })
        .collect();
    v.sort_by(|a, b| a.label.cmp(&b.label));
    v
}

#[tauri::command]
pub fn list_panes(state: State<'_, AppState>) -> Vec<PaneInfo> {
    pane_infos(&state)
}

fn window_id(window: &tauri::WebviewWindow) -> Option<i64> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as i64),
        RawWindowHandle::Xlib(h) => Some(h.window as i64),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as i64),
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as i64),
        _ => None,
    }
}

/// Open a new pane window with its own engine. Returns the window label.
#[tauri::command]
pub fn pane_open(app: AppHandle, state: State<'_, AppState>, channel_id: Option<i64>) -> CmdResult<PaneInfo> {
    require_feature(&state, Feature::Multiscreen, "Multiscreen")?;
    let cfg = state.db.load_config().map_err(err)?;
    let count = state.panes.lock().unwrap().len();
    let max_extra = (cfg.max_multiscreen_instances.clamp(1, 4) as usize).saturating_sub(1);
    if count >= max_extra {
        return Err(format!(
            "Multiscreen limit reached ({} panes incl. main). Raise it in Settings → Playback.",
            cfg.max_multiscreen_instances.max(1)
        ));
    }
    let mut n = 2;
    let label = loop {
        let l = format!("pane-{n}");
        if app.get_webview_window(&l).is_none() {
            break l;
        }
        n += 1;
    };
    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(format!("index.html?pane={label}").into()))
        .title(format!("desktop-iptv — pane {n}"))
        .inner_size(720.0, 405.0)
        .min_inner_size(320.0, 180.0)
        .transparent(true)
        .build()
        .map_err(err)?;
    let wid = window_id(&window);
    let engine = app_engine::create_engine(EngineOptions {
        wid,
        // Copy-back decode for extra instances (CLAUDE.md §6.5): survives iGPUs, avoids zero-copy contention.
        hw_decoding: match cfg.hw_decoding.as_str() {
            "no" => "no".into(),
            _ => "auto-copy-safe".into(),
        },
        default_profile: app_core::ProfileMode::Stable,
        stable_cache_secs: cfg.stable_cache_secs,
        audio_boost: 0,
        audio_delay_ms: 0,
        log_level: "warn".into(),
        libmpv_path: None,
        user_agent: Some(app_net::http::DEFAULT_USER_AGENT.into()),
    });
    let _ = engine.set_mute(true); // only one pane has audio (main by default)
    let app2 = app.clone();
    let label2 = label.clone();
    engine.set_listener(Arc::new(move |ev: EngineEvent| {
        let _ = app2.emit("engine_event_pane", serde_json::json!({ "label": label2, "event": ev }));
    }));
    #[cfg(windows)]
    if let Some(w) = wid {
        app_engine::win_zorder::push_mpv_child_to_bottom(w);
    }
    state
        .panes
        .lock()
        .unwrap()
        .insert(label.clone(), Pane { engine, playing: false, channel_id: None, has_audio: false });
    // Close → tear the engine down.
    let app3 = app.clone();
    let label3 = label.clone();
    window.on_window_event(move |ev| {
        if let tauri::WindowEvent::Destroyed = ev {
            if let Some(state) = app3.try_state::<AppState>() {
                if let Some(p) = state.panes.lock().unwrap().remove(&label3) {
                    p.engine.shutdown();
                }
                let _ = app3.emit("panes_changed", pane_infos(&state));
            }
        }
    });
    if let Some(id) = channel_id {
        pane_play_inner(&state, &label, id)?;
    }
    let _ = app.emit("panes_changed", pane_infos(&state));
    Ok(pane_infos(&state).into_iter().find(|p| p.label == label).unwrap())
}

fn pane_play_inner(state: &AppState, label: &str, channel_id: i64) -> CmdResult<()> {
    let ch = state.db.get_channel(channel_id).map_err(err)?;
    let mut panes = state.panes.lock().unwrap();
    let pane = panes.get_mut(label).ok_or("pane not found")?;
    pane.engine
        .load(&ch.stream_url, app_core::ProfileMode::Stable, if pane.has_audio { 100 } else { 0 }, None)
        .map_err(err)?;
    pane.playing = true;
    pane.channel_id = Some(channel_id);
    Ok(())
}

#[tauri::command]
pub fn pane_play(app: AppHandle, state: State<'_, AppState>, label: String, channel_id: i64) -> CmdResult<()> {
    pane_play_inner(&state, &label, channel_id)?;
    let _ = app.emit("panes_changed", pane_infos(&state));
    Ok(())
}

#[tauri::command]
pub fn pane_stop(app: AppHandle, state: State<'_, AppState>, label: String) -> CmdResult<()> {
    {
        let mut panes = state.panes.lock().unwrap();
        let pane = panes.get_mut(&label).ok_or("pane not found")?;
        let _ = pane.engine.stop();
        pane.playing = false;
        pane.channel_id = None;
    }
    let _ = app.emit("panes_changed", pane_infos(&state));
    Ok(())
}

#[tauri::command]
pub fn pane_close(app: AppHandle, state: State<'_, AppState>, label: String) -> CmdResult<()> {
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }
    if let Some(p) = state.panes.lock().unwrap().remove(&label) {
        p.engine.shutdown();
    }
    let _ = app.emit("panes_changed", pane_infos(&state));
    Ok(())
}

/// Give audio to one pane (or "main"); everything else is muted — audio lock (CLAUDE.md §6.5).
#[tauri::command]
pub fn pane_audio(app: AppHandle, state: State<'_, AppState>, label: String) -> CmdResult<()> {
    let main_muted = label != "main";
    {
        let mut pb = state.playback.lock().unwrap();
        let _ = state.engine.set_mute(main_muted);
        if !main_muted {
            pb.muted = false;
            let _ = state.engine.set_volume(pb.volume);
        } else {
            pb.muted = true;
        }
    }
    {
        let mut panes = state.panes.lock().unwrap();
        for (l, p) in panes.iter_mut() {
            let on = *l == label;
            p.has_audio = on;
            let _ = p.engine.set_mute(!on);
            if on {
                let _ = p.engine.set_volume(100);
            }
        }
    }
    let _ = app.emit("panes_changed", pane_infos(&state));
    let _ = app.emit(super::playback::EV_PLAYBACK, super::playback::playback_state(&state));
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn pane_set_video_rect(
    state: State<'_, AppState>,
    label: String,
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
    let panes = state.panes.lock().unwrap();
    let pane = panes.get(&label).ok_or("pane not found")?;
    pane.engine
        .set_video_margins(
            (x / win_w).clamp(0.0, 1.0),
            (1.0 - (x + w) / win_w).clamp(0.0, 1.0),
            (y / win_h).clamp(0.0, 1.0),
            (1.0 - (y + h) / win_h).clamp(0.0, 1.0),
        )
        .map_err(err)
}

#[tauri::command]
pub fn pane_telemetry(state: State<'_, AppState>, label: String) -> CmdResult<app_core::EngineTelemetryEvent> {
    let panes = state.panes.lock().unwrap();
    Ok(panes.get(&label).ok_or("pane not found")?.engine.telemetry())
}
