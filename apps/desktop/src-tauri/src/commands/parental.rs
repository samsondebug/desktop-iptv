//! Parental PIN + keyword filter (CLAUDE.md §12 days 31–50). The PIN is stored as a salted
//! SHA-256; keywords hide matching channels/groups/VOD until the session is unlocked.

use super::{err, CmdResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;

const KEY_PIN: &str = "parental_pin_hash";
const KEY_KEYWORDS: &str = "parental_keywords";
const DEFAULT_KEYWORDS: &[&str] = &["xxx", "adult", "porn", "18+"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentalStatus {
    pub enabled: bool,
    pub unlocked: bool,
    pub keywords: Vec<String>,
}

fn hash_pin(state: &AppState, pin: &str) -> String {
    let d = Sha256::digest(format!("{}|{}|parental", state.machine_guid, pin.trim()).as_bytes());
    hex::encode(d)
}

fn keywords(state: &AppState) -> CmdResult<Vec<String>> {
    Ok(match state.db.get_setting(KEY_KEYWORDS).map_err(err)? {
        Some(raw) => serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default(),
        None => DEFAULT_KEYWORDS.iter().map(|s| s.to_string()).collect(),
    })
}

/// Apply the filter to the catalog according to enabled/unlocked state.
pub fn apply_filter(state: &AppState) -> CmdResult<()> {
    let enabled = state.db.get_setting(KEY_PIN).map_err(err)?.is_some();
    let unlocked = *state.parental_unlocked.lock().unwrap();
    if enabled && !unlocked {
        state.db.set_hidden_keywords(&keywords(state)?);
    } else {
        state.db.set_hidden_keywords(&[]);
    }
    Ok(())
}

pub fn status(state: &AppState) -> CmdResult<ParentalStatus> {
    Ok(ParentalStatus {
        enabled: state.db.get_setting(KEY_PIN).map_err(err)?.is_some(),
        unlocked: *state.parental_unlocked.lock().unwrap(),
        keywords: keywords(state)?,
    })
}

#[tauri::command]
pub fn parental_status(state: State<'_, AppState>) -> CmdResult<ParentalStatus> {
    status(&state)
}

/// Set or change the PIN. `current_pin` is required when one is already set.
#[tauri::command]
pub fn set_parental_pin(state: State<'_, AppState>, pin: String, current_pin: Option<String>) -> CmdResult<ParentalStatus> {
    let pin = pin.trim();
    if pin.len() < 4 || pin.len() > 12 || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err("PIN must be 4–12 digits".into());
    }
    if let Some(existing) = state.db.get_setting(KEY_PIN).map_err(err)? {
        let given = current_pin.unwrap_or_default();
        if hash_pin(&state, &given) != existing {
            return Err("Current PIN is incorrect".into());
        }
    }
    state.db.set_setting(KEY_PIN, &hash_pin(&state, pin)).map_err(err)?;
    *state.parental_unlocked.lock().unwrap() = false;
    apply_filter(&state)?;
    status(&state)
}

#[tauri::command]
pub fn clear_parental_pin(state: State<'_, AppState>, current_pin: String) -> CmdResult<ParentalStatus> {
    if let Some(existing) = state.db.get_setting(KEY_PIN).map_err(err)? {
        if hash_pin(&state, &current_pin) != existing {
            return Err("Current PIN is incorrect".into());
        }
    }
    state.db.delete_setting(KEY_PIN).map_err(err)?;
    *state.parental_unlocked.lock().unwrap() = false;
    apply_filter(&state)?;
    status(&state)
}

#[tauri::command]
pub fn unlock_parental(state: State<'_, AppState>, pin: String) -> CmdResult<ParentalStatus> {
    let existing = state.db.get_setting(KEY_PIN).map_err(err)?.ok_or("No PIN is set")?;
    if hash_pin(&state, &pin) != existing {
        // Small fixed delay blunts brute force from a script.
        std::thread::sleep(std::time::Duration::from_millis(400));
        return Err("Incorrect PIN".into());
    }
    *state.parental_unlocked.lock().unwrap() = true;
    apply_filter(&state)?;
    status(&state)
}

#[tauri::command]
pub fn lock_parental(state: State<'_, AppState>) -> CmdResult<ParentalStatus> {
    *state.parental_unlocked.lock().unwrap() = false;
    apply_filter(&state)?;
    status(&state)
}

#[tauri::command]
pub fn set_parental_keywords(state: State<'_, AppState>, keywords: Vec<String>, pin: Option<String>) -> CmdResult<ParentalStatus> {
    if let Some(existing) = state.db.get_setting(KEY_PIN).map_err(err)? {
        let unlocked = *state.parental_unlocked.lock().unwrap();
        if !unlocked && hash_pin(&state, &pin.unwrap_or_default()) != existing {
            return Err("PIN required to change the keyword list".into());
        }
    }
    let cleaned: Vec<String> = keywords.iter().map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect();
    state.db.set_setting(KEY_KEYWORDS, &serde_json::to_string(&cleaned).unwrap()).map_err(err)?;
    apply_filter(&state)?;
    status(&state)
}
