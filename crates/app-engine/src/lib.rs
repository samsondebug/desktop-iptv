//! `app-engine` — the libmpv controller (CLAUDE.md §6).
//!
//! * One long-lived instance. Zap = `loadfile <url> replace`. Never recreated per channel.
//! * Two profiles (`low_latency`, `stable`) injected as options from Rust.
//! * Telemetry from observed properties → [`EngineEvent::Telemetry`] → tiny HUD.
//! * `zap_ms` measured from `load()` to `MPV_EVENT_PLAYBACK_RESTART` (first frame shown).
//! * If libmpv cannot be loaded the [`StubEngine`] takes over so the catalog UI keeps working.

pub mod ffi;
pub mod mpv;
pub mod profiles;
pub mod stub;
#[cfg(windows)]
pub mod win_zorder;

use app_core::{EngineTelemetryEvent, ProfileMode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("libmpv unavailable: {0}")]
    Unavailable(String),
    #[error("mpv: {0}")]
    Mpv(String),
    #[error("engine is shut down")]
    ShutDown,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Discrete engine events, forwarded to the UI by the Tauri layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Periodic snapshot (~2 Hz while playing, on change otherwise).
    Telemetry(EngineTelemetryEvent),
    /// First frame of a newly loaded stream is on screen.
    PlaybackStarted { url_redacted: String, zap_ms: u64 },
    /// Stream ended. `error` is set for `MPV_END_FILE_REASON_ERROR`.
    EndFile { reason: String, error: Option<String>, url_redacted: String },
    /// Buffering state flipped.
    Buffering { active: bool },
    /// mpv log line at warn/error level, already redacted.
    Log { level: String, text: String },
    /// Engine shut down (app exit or fatal).
    Shutdown,
}

pub type EventListener = Arc<dyn Fn(EngineEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Native surface id (HWND / X11 window / NSView pointer). `None` → headless (`vo=null`).
    pub wid: Option<i64>,
    /// User setting, e.g. "auto-safe". Mapped per-OS by [`profiles::hwdec_for_platform`].
    pub hw_decoding: String,
    pub default_profile: ProfileMode,
    pub stable_cache_secs: u16,
    pub audio_boost: u16,
    pub audio_delay_ms: i32,
    /// mpv msg-level for the internal log (e.g. "warn", "info", "v").
    pub log_level: String,
    /// Explicit libmpv path (else searched, see [`ffi::candidate_dirs`]).
    pub libmpv_path: Option<std::path::PathBuf>,
    /// User-Agent for HTTP streams.
    pub user_agent: Option<String>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            wid: None,
            hw_decoding: "auto-safe".into(),
            default_profile: ProfileMode::Stable,
            stable_cache_secs: 20,
            audio_boost: 100,
            audio_delay_ms: 0,
            log_level: "warn".into(),
            libmpv_path: None,
            user_agent: None,
        }
    }
}

/// Backend-agnostic player controller. Implemented by [`mpv::MpvEngine`] and [`stub::StubEngine`].
pub trait PlayerEngine: Send + Sync {
    /// "mpv" | "stub"
    fn kind(&self) -> &'static str;
    /// Human-readable backend description for the About/Diagnostics panel.
    fn describe(&self) -> String;

    /// Zap: `loadfile <url> replace`. Applies `profile` first if it differs from the active one.
    /// `start_secs` resumes VOD at a position (ignored for live streams).
    fn load(&self, url: &str, profile: ProfileMode, audio_boost: u16, start_secs: Option<f64>) -> Result<()>;
    /// Absolute seek (VOD).
    fn seek(&self, secs: f64) -> Result<()>;
    fn stop(&self) -> Result<()>;

    /// Switch profile. Live-switchable keys apply immediately; the caller decides whether to
    /// reload the stream for the rest (the Tauri layer reloads when a stream is active).
    fn set_profile(&self, profile: ProfileMode) -> Result<()>;
    fn active_profile(&self) -> ProfileMode;

    fn set_pause(&self, pause: bool) -> Result<()>;
    fn set_mute(&self, mute: bool) -> Result<()>;
    /// 0..=130
    fn set_volume(&self, percent: u16) -> Result<()>;
    fn set_audio_delay_ms(&self, ms: i32) -> Result<()>;
    /// Where the video is drawn inside the native surface, as fractions of the window
    /// (`video-margin-ratio-*`). This is how the split canvas keeps video in the player pane.
    fn set_video_margins(&self, left: f32, right: f32, top: f32, bottom: f32) -> Result<()>;

    /// Raw escape hatches for the Advanced panel and diagnostics. Never exposed unredacted.
    fn set_property(&self, name: &str, value: &str) -> Result<()>;
    fn get_property(&self, name: &str) -> Result<Option<String>>;
    fn command(&self, args: &[&str]) -> Result<()>;

    /// Latest telemetry snapshot.
    fn telemetry(&self) -> EngineTelemetryEvent;
    /// Register the (single) event listener.
    fn set_listener(&self, listener: EventListener);
    /// Shut the engine down (idempotent).
    fn shutdown(&self);
}

/// Build the best available engine: real libmpv when it loads, otherwise the stub.
pub fn create_engine(opts: EngineOptions) -> Arc<dyn PlayerEngine> {
    // `DESKTOP_IPTV_ENGINE=stub` forces the stub (UI development, screenshots, CI).
    if std::env::var("DESKTOP_IPTV_ENGINE").map(|v| v == "stub").unwrap_or(false) {
        tracing::warn!("DESKTOP_IPTV_ENGINE=stub — video disabled");
        return Arc::new(stub::StubEngine::new(opts, "forced by DESKTOP_IPTV_ENGINE=stub".into()));
    }
    match mpv::MpvEngine::new(opts.clone()) {
        Ok(e) => {
            tracing::info!(backend = %e.describe(), "engine ready");
            #[cfg(windows)]
            if let Some(wid) = opts.wid {
                win_zorder::push_mpv_child_to_bottom(wid);
            }
            Arc::new(e)
        }
        Err(err) => {
            tracing::warn!(%err, "libmpv unavailable — using stub engine");
            Arc::new(stub::StubEngine::new(opts, err.to_string()))
        }
    }
}
