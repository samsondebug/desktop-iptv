use app_engine::PlayerEngine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What the engine is currently playing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaybackItem {
    #[default]
    None,
    Channel {
        id: i64,
    },
    Vod {
        id: i64,
    },
    Episode {
        id: i64,
        series_id: i64,
    },
    /// Raw URL from Diagnostics.
    Url,
}

/// Playback bookkeeping the command layer needs across calls.
#[derive(Debug, Clone, Default)]
pub struct Playback {
    pub item: PlaybackItem,
    /// Unredacted; never leaves Rust.
    pub stream_url: Option<String>,
    pub profile: app_core::ProfileMode,
    pub volume: u16,
    pub muted: bool,
    pub paused: bool,
    /// Last time VOD progress was persisted.
    pub progress_saved_at: Option<Instant>,
    pub last_saved_pos: i64,
}

impl Playback {
    pub fn channel_id(&self) -> Option<i64> {
        match self.item {
            PlaybackItem::Channel { id } => Some(id),
            _ => None,
        }
    }
    pub fn is_vod(&self) -> bool {
        matches!(self.item, PlaybackItem::Vod { .. } | PlaybackItem::Episode { .. })
    }
}

/// A multiscreen pane: its own window, its own engine (CLAUDE.md §6.5).
pub struct Pane {
    pub engine: Arc<dyn PlayerEngine>,
    pub playing: bool,
    pub channel_id: Option<i64>,
    pub has_audio: bool,
}

pub struct AppState {
    pub db: Arc<app_db::Db>,
    pub engine: Arc<dyn PlayerEngine>,
    pub data_dir: PathBuf,
    pub playback: Mutex<Playback>,
    pub machine_guid: String,
    /// Parental lock: unlocked for this session?
    pub parental_unlocked: Mutex<bool>,
    /// Multiscreen panes by window label.
    pub panes: Mutex<std::collections::HashMap<String, Pane>>,
    /// Mini (PiP) mode active?
    pub mini_mode: Mutex<bool>,
}

impl AppState {
    pub fn new(db: Arc<app_db::Db>, engine: Arc<dyn PlayerEngine>, data_dir: PathBuf) -> Self {
        let cfg = db.load_config().unwrap_or_default();
        let playback = Playback {
            profile: app_core::ProfileMode::parse(&cfg.default_profile).unwrap_or_default(),
            volume: cfg.audio_boost,
            ..Default::default()
        };
        Self {
            db,
            engine,
            data_dir,
            playback: Mutex::new(playback),
            machine_guid: app_core::license::machine_guid(),
            parental_unlocked: Mutex::new(false),
            panes: Mutex::new(Default::default()),
            mini_mode: Mutex::new(false),
        }
    }
}
