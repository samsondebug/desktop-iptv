use app_engine::PlayerEngine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Playback bookkeeping the command layer needs across calls.
#[derive(Debug, Clone, Default)]
pub struct Playback {
    pub channel_id: Option<i64>,
    /// Unredacted; never leaves Rust.
    pub stream_url: Option<String>,
    pub profile: app_core::ProfileMode,
    pub volume: u16,
    pub muted: bool,
    pub paused: bool,
}

pub struct AppState {
    pub db: Arc<app_db::Db>,
    pub engine: Arc<dyn PlayerEngine>,
    pub data_dir: PathBuf,
    pub playback: Mutex<Playback>,
    pub machine_guid: String,
}

impl AppState {
    pub fn new(db: Arc<app_db::Db>, engine: Arc<dyn PlayerEngine>, data_dir: PathBuf) -> Self {
        let cfg = db.load_config().unwrap_or_default();
        let playback = Playback {
            profile: app_core::ProfileMode::parse(&cfg.default_profile).unwrap_or_default(),
            volume: cfg.audio_boost,
            ..Default::default()
        };
        Self { db, engine, data_dir, playback: Mutex::new(playback), machine_guid: app_core::license::machine_guid() }
    }
}
