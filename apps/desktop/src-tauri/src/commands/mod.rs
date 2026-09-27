//! Tauri commands. Every error string passes through `redact` before it reaches the UI.

pub mod catalog;
pub mod dvr;
pub mod epg;
pub mod parental;
pub mod playback;
pub mod vod;
pub mod windows;

pub use catalog::*;
pub use dvr::*;
pub use epg::*;
pub use parental::*;
pub use playback::*;
pub use vod::*;
pub use windows::*;

use crate::state::AppState;
use app_core::redact::redact;
use app_core::*;
use serde::Serialize;
use tauri::State;

pub type CmdResult<T> = Result<T, String>;

pub fn err<E: std::fmt::Display>(e: E) -> String {
    redact(&e.to_string())
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn license_state(state: &AppState) -> CmdResult<LicenseStateResponse> {
    let trial = state.db.trial_started_at().map_err(err)?;
    let token = state.db.license_token().map_err(err)?;
    Ok(app_core::license::compute_state(now_unix(), trial, token.as_deref(), &state.machine_guid))
}

pub fn require_feature(state: &AppState, feature: app_core::license::Feature, what: &str) -> CmdResult<()> {
    let lic = license_state(state)?;
    if app_core::license::allows(&lic.tier, feature) {
        Ok(())
    } else {
        Err(format!("{what} is a PRO feature. Your 72-hour trial has ended — unlock PRO (one-time) in Settings."))
    }
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
    pub parental: parental::ParentalStatus,
    pub data_dir: String,
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
        parental: parental::status(&state)?,
        data_dir: state.data_dir.display().to_string(),
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

// ---------- license ----------

#[tauri::command]
pub fn get_license_state(state: State<'_, AppState>) -> CmdResult<LicenseStateResponse> {
    license_state(&state)
}

#[tauri::command]
pub fn activate_license(state: State<'_, AppState>, cmd: ValidateLicenseCommand) -> CmdResult<LicenseStateResponse> {
    let _ = cmd.machine_guid; // we always validate against our own guid
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

/// Theme tokens export/import (CLAUDE.md §8: user-exportable token JSON).
#[tauri::command]
pub fn get_theme_tokens(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    state.db.get_setting("theme_tokens").map_err(err)
}

#[tauri::command]
pub fn set_theme_tokens(state: State<'_, AppState>, json: Option<String>) -> CmdResult<()> {
    match json.filter(|j| !j.trim().is_empty()) {
        Some(j) => {
            // Validate: must be a flat object of string values.
            let v: serde_json::Value = serde_json::from_str(&j).map_err(|e| format!("invalid JSON: {e}"))?;
            let obj = v.as_object().ok_or("theme tokens must be a JSON object")?;
            if obj.values().any(|x| !x.is_string()) {
                return Err("theme token values must be strings (CSS values)".into());
            }
            state.db.set_setting("theme_tokens", &j).map_err(err)
        }
        None => state.db.delete_setting("theme_tokens").map_err(err),
    }
}
