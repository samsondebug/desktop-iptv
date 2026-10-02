//! Channel-name cleanup rules: user-defined regex find/replace applied to channel names at
//! import time ("US| 4K ⚡ ESPN" → "ESPN"). Rules live in the `settings` table as JSON and are
//! compiled once into the `Db`; `$1`-style capture references work in the replacement. The
//! original provider name arrives again on every refresh, so editing rules + "apply now" (or
//! the next refresh) is always non-destructive.

use crate::normalize::normalize_name;
use crate::{Db, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

pub const SETTING_KEY: &str = "name_rules";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NameRule {
    pub pattern: String,
    pub replacement: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Compile the enabled rules, or name the first bad pattern.
fn compile(rules: &[NameRule]) -> std::result::Result<Vec<(regex::Regex, String)>, String> {
    rules
        .iter()
        .filter(|r| r.enabled && !r.pattern.is_empty())
        .map(|r| {
            regex::Regex::new(&r.pattern)
                .map(|re| (re, r.replacement.clone()))
                .map_err(|e| format!("bad pattern “{}”: {e}", r.pattern))
        })
        .collect()
}

fn apply(compiled: &[(regex::Regex, String)], name: &str) -> String {
    let mut out = name.to_string();
    for (re, rep) in compiled {
        out = re.replace_all(&out, rep.as_str()).into_owned();
    }
    let trimmed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        name.to_string() // a rule that erases the whole name is a mistake; keep the original
    } else {
        trimmed
    }
}

impl Db {
    /// Install (and persist) the rules. Errors name the first invalid regex; nothing changes then.
    pub fn set_name_rules(&self, rules: &[NameRule]) -> std::result::Result<usize, String> {
        let compiled = compile(rules)?;
        let json = serde_json::to_string(rules).map_err(|e| e.to_string())?;
        self.set_setting(SETTING_KEY, &json).map_err(|e| e.to_string())?;
        let n = compiled.len();
        *self.name_rules.write().unwrap() = compiled;
        Ok(n)
    }

    /// Load persisted rules into the compiled set (app start). Bad patterns are skipped.
    pub fn load_name_rules(&self) -> Result<Vec<NameRule>> {
        let rules: Vec<NameRule> =
            self.get_setting(SETTING_KEY)?.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        let compiled = rules
            .iter()
            .filter(|r| r.enabled && !r.pattern.is_empty())
            .filter_map(|r| regex::Regex::new(&r.pattern).ok().map(|re| (re, r.replacement.clone())))
            .collect();
        *self.name_rules.write().unwrap() = compiled;
        Ok(rules)
    }

    /// Run `name` through the installed rules (identity when none are set).
    pub fn apply_name_rules(&self, name: &str) -> String {
        let rules = self.name_rules.read().unwrap();
        if rules.is_empty() {
            return name.to_string();
        }
        apply(&rules, name)
    }

    pub fn name_rules_active(&self) -> bool {
        !self.name_rules.read().unwrap().is_empty()
    }

    /// Preview `rules` (not installed) against this playlist's current names: the first
    /// `limit` channels whose name would change, as (before, after).
    pub fn preview_name_rules(
        &self,
        rules: &[NameRule],
        playlist_id: i64,
        limit: usize,
    ) -> std::result::Result<Vec<(String, String)>, String> {
        let compiled = compile(rules)?;
        if compiled.is_empty() {
            return Ok(Vec::new());
        }
        self.with_read(|c| {
            let mut st = c.prepare("SELECT name FROM channels WHERE playlist_id = ?1 ORDER BY id LIMIT 5000")?;
            let names = st
                .query_map(params![playlist_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(names
                .into_iter()
                .filter_map(|n| {
                    let after = apply(&compiled, &n);
                    (after != n).then_some((n, after))
                })
                .take(limit.clamp(1, 100))
                .collect())
        })
        .map_err(|e| e.to_string())
    }

    /// Apply the installed rules to every stored channel name right now (instead of waiting
    /// for the next playlist refresh). Returns how many rows changed. FTS follows via triggers.
    pub fn reapply_name_rules(&self) -> Result<u64> {
        if !self.name_rules_active() {
            return Ok(0);
        }
        let rows: Vec<(i64, String)> = self.with_read(|c| {
            let mut st = c.prepare("SELECT id, name FROM channels")?;
            let it = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            Ok(it.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let changes: Vec<(i64, String, String)> = {
            let rules = self.name_rules.read().unwrap();
            rows.into_iter()
                .filter_map(|(id, name)| {
                    let after = apply(&rules, &name);
                    (after != name).then(|| {
                        let norm = normalize_name(&after);
                        (id, after, norm)
                    })
                })
                .collect()
        };
        let mut changed = 0u64;
        for chunk in changes.chunks(crate::MAX_ROWS_PER_TX) {
            changed += self.with_write(|c| {
                let tx = c.transaction()?;
                {
                    let mut st =
                        tx.prepare_cached("UPDATE channels SET name = ?2, normalized_name = ?3 WHERE id = ?1")?;
                    for (id, name, norm) in chunk {
                        st.execute(params![id, name, norm])?;
                    }
                }
                tx.commit()?;
                Ok(chunk.len() as u64)
            })?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{ChannelInsert, PlaylistInsert};

    fn seed() -> (Db, i64) {
        let db = Db::open_in_memory().unwrap();
        let p = db
            .insert_playlist(&PlaylistInsert {
                r#type: "m3u".into(),
                name: "t".into(),
                base_url: "file:///t.m3u".into(),
                user: None,
                pass: None,
                mac: None,
                ua: None,
            })
            .unwrap();
        (db, p)
    }

    fn rule(p: &str, r: &str) -> NameRule {
        NameRule { pattern: p.into(), replacement: r.into(), enabled: true }
    }

    #[test]
    fn rules_apply_at_import_and_on_demand() {
        let (db, p) = seed();
        let mk = |sid: &str, name: &str| ChannelInsert {
            source_id: sid.into(),
            name: name.into(),
            stream_url: format!("http://x/{sid}.ts"),
            ..Default::default()
        };
        // Import without rules: raw provider junk.
        db.upsert_channels(p, &[mk("1", "US| ESPN 4K ⚡"), mk("2", "UK|  BBC One HD")]).unwrap();
        assert_eq!(db.list_channels(p, None, 10, 0).unwrap()[0].name, "US| ESPN 4K ⚡");

        db.set_name_rules(&[rule(r"^[A-Z]{2}\|\s*", ""), rule(r"\s*⚡\s*", "")]).unwrap();
        // Existing rows: apply now.
        assert_eq!(db.reapply_name_rules().unwrap(), 2);
        let names: Vec<String> = db.list_channels(p, None, 10, 0).unwrap().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["ESPN 4K", "BBC One HD"]);
        // Search follows the cleaned names.
        assert_eq!(db.search_channels("espn", p, 10, 0).unwrap().len(), 1);

        // New imports are cleaned on the way in.
        db.upsert_channels(p, &[mk("3", "DE| RTL ⚡")]).unwrap();
        let names: Vec<String> = db.list_channels(p, None, 10, 0).unwrap().into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"RTL".to_string()));
    }

    #[test]
    fn bad_regex_rejected_and_erasing_rule_keeps_original() {
        let (db, _p) = seed();
        assert!(db.set_name_rules(&[rule("(", "x")]).is_err());
        db.set_name_rules(&[rule(".*", "")]).unwrap();
        assert_eq!(db.apply_name_rules("ESPN"), "ESPN");
    }

    #[test]
    fn capture_groups_and_preview() {
        let (db, p) = seed();
        db.upsert_channels(
            p,
            &[ChannelInsert {
                source_id: "1".into(),
                name: "ESPN (US)".into(),
                stream_url: "http://x/1.ts".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let rules = vec![rule(r"^(.*) \((US|UK)\)$", "$2: $1")];
        let prev = db.preview_name_rules(&rules, p, 10).unwrap();
        assert_eq!(prev, vec![("ESPN (US)".to_string(), "US: ESPN".to_string())]);
        // Nothing installed yet — names unchanged.
        assert_eq!(db.list_channels(p, None, 10, 0).unwrap()[0].name, "ESPN (US)");
    }
}
