//! Typed IPC contracts (CLAUDE.md §4). Mirror every change in `apps/desktop/src/lib/ipc.ts`.

use serde::{Deserialize, Serialize};

/// Playback profile. Exactly two on day one (CLAUDE.md §0 non-negotiable 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProfileMode {
    LowLatency,
    /// Default shipped profile for new users (fewer "it buffers" reviews).
    #[default]
    Stable,
}

impl ProfileMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileMode::LowLatency => "low_latency",
            ProfileMode::Stable => "stable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "low_latency" => Some(ProfileMode::LowLatency),
            "stable" => Some(ProfileMode::Stable),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPayload {
    pub app_theme: String,
    /// "auto-safe" | "d3d11va" | "d3d11va-copy" | "videotoolbox" | "videotoolbox-copy" | "no"
    pub hw_decoding: String,
    /// "low_latency" | "stable"
    pub default_profile: String,
    pub auto_play_last: bool,
    pub max_multiscreen_instances: u8,
    pub hide_vod_tabs: bool,
    pub hide_brand_chrome: bool,
    pub hud_enabled: bool,
    /// 100..=130
    pub audio_boost: u16,
    pub audio_delay_ms: i32,
    /// Seconds of demuxer cache for the `stable` profile (20..=60).
    pub stable_cache_secs: u16,
    /// Whether the user has acknowledged the player-only legal block.
    pub legal_accepted: bool,
}

impl Default for ConfigPayload {
    fn default() -> Self {
        Self {
            app_theme: "dark".into(),
            hw_decoding: "auto-safe".into(),
            default_profile: ProfileMode::Stable.as_str().into(),
            auto_play_last: true,
            max_multiscreen_instances: 2,
            hide_vod_tabs: false,
            hide_brand_chrome: false,
            hud_enabled: false,
            audio_boost: 100,
            audio_delay_ms: 0,
            stable_cache_secs: 20,
            legal_accepted: false,
        }
    }
}

impl ConfigPayload {
    /// Clamp every field into its legal range. Called on every write.
    pub fn sanitized(mut self) -> Self {
        self.audio_boost = self.audio_boost.clamp(100, 130);
        self.audio_delay_ms = self.audio_delay_ms.clamp(-5000, 5000);
        self.max_multiscreen_instances = self.max_multiscreen_instances.clamp(1, 4);
        self.stable_cache_secs = self.stable_cache_secs.clamp(20, 60);
        if ProfileMode::parse(&self.default_profile).is_none() {
            self.default_profile = ProfileMode::Stable.as_str().into();
        }
        const HWDEC: &[&str] =
            &["auto-safe", "d3d11va", "d3d11va-copy", "videotoolbox", "videotoolbox-copy", "vaapi", "vaapi-copy", "no"];
        if !HWDEC.contains(&self.hw_decoding.as_str()) {
            self.hw_decoding = "auto-safe".into();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateLicenseCommand {
    pub license_key: String,
    pub machine_guid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseStateResponse {
    pub is_valid: bool,
    pub expires_at: Option<i64>,
    /// "free" | "trial" | "pro_lifetime"
    pub tier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FtsQueryRequest {
    pub query_string: String,
    pub playlist_id: i64,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRecord {
    pub id: i64,
    pub playlist_id: i64,
    pub source_id: String,
    pub name: String,
    pub normalized_name: String,
    pub group_title: Option<String>,
    pub logo: Option<String>,
    pub stream_url: String,
    pub tvg_id: Option<String>,
    pub catchup_days: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchPlaylistRequest {
    pub playlist_id: i64,
    pub target_url: String,
    pub user_agent: Option<String>,
    pub auth_credentials: Option<AuthCredentials>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthCredentials {
    pub username: Option<String>,
    pub password: Option<String>,
    pub mac_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadStreamCommand {
    pub stream_url: String,
    /// "low_latency" | "stable"
    pub profile_mode: String,
    pub audio_boost: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineTelemetryEvent {
    pub active_profile: String,
    pub width: u32,
    pub height: u32,
    pub codec_name: String,
    pub bitrate_kbps: u32,
    pub fps: f32,
    pub dropped_frames: u64,
    pub cache_duration_secs: f64,
    pub is_underrun: bool,
    pub zap_ms: Option<u64>,
    /// VOD only: current position / total length in seconds (0 for live).
    #[serde(default)]
    pub time_pos_s: f64,
    #[serde(default)]
    pub duration_s: f64,
    #[serde(default)]
    pub paused: bool,
}

/// Paged list request for the virtualized Live list. Keyset paging by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListChannelsRequest {
    pub playlist_id: i64,
    pub group_title: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// Progress event emitted while a playlist is being imported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportProgressEvent {
    pub playlist_id: i64,
    pub stage: String, // "fetching" | "parsing" | "indexing" | "done" | "error"
    pub channels: u64,
    pub bytes: u64,
    pub message: Option<String>,
}

/// Group summary for the rail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    pub group_title: String,
    pub channel_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistSummary {
    pub id: i64,
    pub r#type: String,
    pub name: String,
    pub base_url_redacted: String,
    pub channel_count: i64,
    pub created: String,
}

/// Summary returned after an import completes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncStats {
    pub inserted: u64,
    pub updated: u64,
    pub skipped: u64,
    pub groups: u64,
    pub elapsed_ms: u64,
    pub warnings: Vec<String>,
    /// EPG hint discovered in the source (M3U `url-tvg`), raw (may contain credentials).
    #[serde(default, skip_serializing)]
    pub epg_url: Option<String>,
}
