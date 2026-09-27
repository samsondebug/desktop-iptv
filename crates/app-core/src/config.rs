//! Config persistence helpers. The actual store is the `settings` table in `app-db`;
//! this module only defines keys and (de)serialization so both crates agree.

use crate::ipc::ConfigPayload;

pub const SETTINGS_KEY_CONFIG: &str = "config";
pub const SETTINGS_KEY_LAST_CHANNEL: &str = "last_channel_id";
pub const SETTINGS_KEY_TRIAL_STARTED_AT: &str = "trial_started_at";
pub const SETTINGS_KEY_LICENSE_TOKEN: &str = "license_token";
pub const SETTINGS_KEY_SCHEMA_VERSION: &str = "schema_version";

pub fn config_to_json(cfg: &ConfigPayload) -> String {
    serde_json::to_string(cfg).expect("ConfigPayload serializes")
}

/// Tolerant parse: unknown/missing fields fall back to defaults so old settings rows survive upgrades.
pub fn config_from_json(raw: &str) -> ConfigPayload {
    let default = serde_json::to_value(ConfigPayload::default()).expect("default serializes");
    let mut merged = default;
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
        if let serde_json::Value::Object(target) = &mut merged {
            for (k, v) in map {
                target.insert(k, v);
            }
        }
    }
    serde_json::from_value::<ConfigPayload>(merged).unwrap_or_default().sanitized()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tolerance() {
        let cfg = ConfigPayload { audio_boost: 200, ..Default::default() };
        let json = config_to_json(&cfg);
        let back = config_from_json(&json);
        assert_eq!(back.audio_boost, 130, "clamped on read");
        let partial = config_from_json(r#"{"hud_enabled":true,"unknown_field":1}"#);
        assert!(partial.hud_enabled);
        assert_eq!(partial.default_profile, "stable");
        let garbage = config_from_json("not json");
        assert_eq!(garbage.app_theme, "dark");
    }
}
