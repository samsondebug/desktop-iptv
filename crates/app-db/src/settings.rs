//! Key/value settings table + typed helpers for config, last channel, trial and license.

use crate::{Db, Result};
use app_core::config::*;
use app_core::ConfigPayload;
use rusqlite::{params, OptionalExtension};

impl Db {
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get::<_, String>(0))
                .optional()?)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_write(|c| {
            c.execute(
                "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.with_write(|c| {
            c.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
            Ok(())
        })
    }

    pub fn load_config(&self) -> Result<ConfigPayload> {
        Ok(match self.get_setting(SETTINGS_KEY_CONFIG)? {
            Some(raw) => config_from_json(&raw),
            None => ConfigPayload::default(),
        })
    }

    pub fn save_config(&self, cfg: &ConfigPayload) -> Result<ConfigPayload> {
        let cfg = cfg.clone().sanitized();
        self.set_setting(SETTINGS_KEY_CONFIG, &config_to_json(&cfg))?;
        Ok(cfg)
    }

    pub fn last_channel_id(&self) -> Result<Option<i64>> {
        Ok(self.get_setting(SETTINGS_KEY_LAST_CHANNEL)?.and_then(|s| s.parse().ok()))
    }

    pub fn set_last_channel_id(&self, id: i64) -> Result<()> {
        self.set_setting(SETTINGS_KEY_LAST_CHANNEL, &id.to_string())
    }

    pub fn trial_started_at(&self) -> Result<Option<i64>> {
        Ok(self.get_setting(SETTINGS_KEY_TRIAL_STARTED_AT)?.and_then(|s| s.parse().ok()))
    }

    /// Start the 72h trial clock if it has not started. Returns the start timestamp.
    pub fn ensure_trial_started(&self, now_unix: i64) -> Result<i64> {
        if let Some(t) = self.trial_started_at()? {
            return Ok(t);
        }
        self.set_setting(SETTINGS_KEY_TRIAL_STARTED_AT, &now_unix.to_string())?;
        Ok(now_unix)
    }

    pub fn license_token(&self) -> Result<Option<String>> {
        self.get_setting(SETTINGS_KEY_LICENSE_TOKEN)
    }

    pub fn set_license_token(&self, token: &str) -> Result<()> {
        self.set_setting(SETTINGS_KEY_LICENSE_TOKEN, token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let cfg = db.load_config().unwrap();
        assert_eq!(cfg.default_profile, "stable");
        let mut cfg2 = cfg.clone();
        cfg2.hud_enabled = true;
        cfg2.audio_boost = 999;
        let saved = db.save_config(&cfg2).unwrap();
        assert_eq!(saved.audio_boost, 130);
        assert!(db.load_config().unwrap().hud_enabled);
        assert_eq!(db.last_channel_id().unwrap(), None);
        db.set_last_channel_id(42).unwrap();
        assert_eq!(db.last_channel_id().unwrap(), Some(42));
        assert_eq!(db.ensure_trial_started(100).unwrap(), 100);
        assert_eq!(db.ensure_trial_started(999).unwrap(), 100, "does not restart");
    }
}
