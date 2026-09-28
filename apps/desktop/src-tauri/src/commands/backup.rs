//! Encrypted backup / restore (CLAUDE.md §10). The payload is JSON sealed by
//! `app_core::backup` (AES-256-GCM + Argon2id). What goes in:
//!
//! * playlists with their credentials, EPG offset, stream format, XMLTV sources
//! * favorites and EPG overrides — keyed by the channel's `source_id`, because channel row ids
//!   are regenerated on import; they are applied after the restored playlist finishes syncing
//! * settings: config, theme tokens, parental keywords
//!
//! Left out on purpose: the parental PIN hash and the license token (both machine-bound),
//! recordings/downloads (files on disk), programme data (re-imported), history.

use super::{err, CmdResult};
use crate::state::AppState;
use crate::sync::{spawn_sync, SyncScope};
use app_core::backup as crypto;
use app_core::{PRODUCT_NAME, PRODUCT_VERSION};
use app_db::channels::PlaylistInsert;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

pub const KEY_PENDING_FAVORITES: &str = "restore_pending_favorites";
pub const KEY_PENDING_OVERRIDES: &str = "restore_pending_overrides";
const KEY_THEME: &str = "theme_tokens";
const KEY_KEYWORDS: &str = "parental_keywords";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlaylistBackup {
    r#type: String,
    name: String,
    base_url: String,
    user: Option<String>,
    pass: Option<String>,
    mac: Option<String>,
    ua: Option<String>,
    epg_offset_min: i32,
    stream_format: String,
    epg_sources: Vec<String>,
    /// `source_id`s of favorite channels.
    favorites: Vec<String>,
    /// `(source_id, tvg_id_manual)`.
    epg_overrides: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Payload {
    format: u32,
    product: String,
    version: String,
    created_unix: i64,
    config: app_core::ConfigPayload,
    theme_tokens: Option<String>,
    parental_keywords: Option<Vec<String>>,
    playlists: Vec<PlaylistBackup>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupSummary {
    pub path: String,
    pub bytes: u64,
    pub playlists: usize,
    pub favorites: usize,
    pub epg_overrides: usize,
    pub epg_sources: usize,
}

fn collect(state: &AppState) -> CmdResult<Payload> {
    let mut playlists = Vec::new();
    for p in state.db.list_playlists().map_err(err)? {
        let src = state.db.playlist_source(p.id).map_err(err)?;
        let meta = state.db.playlist_meta(p.id).map_err(err)?;
        let epg_sources = state.db.list_epg_sources(p.id).map_err(err)?;
        // Sources are stored redacted for the UI; fetch the raw URLs by id.
        let mut raw_sources = Vec::new();
        for s in &epg_sources {
            if let Ok((_, url)) = state.db.epg_source_url(s.id) {
                raw_sources.push(url);
            }
        }
        let favorites: Vec<String> = state
            .db
            .favorite_channels()
            .map_err(err)?
            .into_iter()
            .filter(|c| c.playlist_id == p.id)
            .map(|c| c.source_id)
            .collect();
        let mut overrides = Vec::new();
        for (channel_id, tvg) in state.db.epg_overrides_for_playlist(p.id).map_err(err)? {
            if let Ok(ch) = state.db.get_channel(channel_id) {
                overrides.push((ch.source_id, tvg));
            }
        }
        playlists.push(PlaylistBackup {
            r#type: src.r#type,
            name: src.name,
            base_url: src.base_url,
            user: src.user,
            pass: src.pass,
            mac: src.mac,
            ua: src.ua,
            epg_offset_min: meta.epg_offset_min,
            stream_format: meta.stream_format,
            epg_sources: raw_sources,
            favorites,
            epg_overrides: overrides,
        });
    }
    let keywords =
        state.db.get_setting(KEY_KEYWORDS).map_err(err)?.and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok());
    Ok(Payload {
        format: 1,
        product: PRODUCT_NAME.into(),
        version: PRODUCT_VERSION.into(),
        created_unix: super::now_unix(),
        config: state.db.load_config().map_err(err)?,
        theme_tokens: state.db.get_setting(KEY_THEME).map_err(err)?,
        parental_keywords: keywords,
        playlists,
    })
}

/// Write an encrypted backup to `path`.
#[tauri::command]
pub fn backup_export(state: State<'_, AppState>, path: String, passphrase: String) -> CmdResult<BackupSummary> {
    export_to(&state, path, &passphrase)
}

pub fn export_to(state: &AppState, path: String, passphrase: &str) -> CmdResult<BackupSummary> {
    let payload = collect(state)?;
    let json = serde_json::to_vec(&payload).map_err(err)?;
    let sealed = crypto::seal(passphrase, &json).map_err(|e| e.to_string())?;
    std::fs::write(&path, &sealed).map_err(|e| format!("could not write backup: {e}"))?;
    Ok(BackupSummary {
        path,
        bytes: sealed.len() as u64,
        playlists: payload.playlists.len(),
        favorites: payload.playlists.iter().map(|p| p.favorites.len()).sum(),
        epg_overrides: payload.playlists.iter().map(|p| p.epg_overrides.len()).sum(),
        epg_sources: payload.playlists.iter().map(|p| p.epg_sources.len()).sum(),
    })
}

/// Peek at a backup: is it ours, and does the passphrase open it? Returns the summary only.
#[tauri::command]
pub fn backup_inspect(path: String, passphrase: String) -> CmdResult<BackupSummary> {
    let (payload, bytes) = read_payload(&path, &passphrase)?;
    Ok(BackupSummary {
        path,
        bytes,
        playlists: payload.playlists.len(),
        favorites: payload.playlists.iter().map(|p| p.favorites.len()).sum(),
        epg_overrides: payload.playlists.iter().map(|p| p.epg_overrides.len()).sum(),
        epg_sources: payload.playlists.iter().map(|p| p.epg_sources.len()).sum(),
    })
}

fn read_payload(path: &str, passphrase: &str) -> CmdResult<(Payload, u64)> {
    let file = std::fs::read(path).map_err(|e| format!("could not read backup: {e}"))?;
    if !crypto::looks_like_backup(&file) {
        return Err("This is not an SKTV backup file.".into());
    }
    let json = crypto::open(passphrase, &file).map_err(|e| e.to_string())?;
    let payload: Payload = serde_json::from_slice(&json).map_err(|e| format!("backup payload is not readable: {e}"))?;
    if payload.format != 1 {
        return Err(format!("backup format {} is newer than this app understands", payload.format));
    }
    Ok((payload, file.len() as u64))
}

/// Restore. `replace = true` deletes existing playlists first; otherwise playlists whose
/// `(type, base_url, user|mac)` already exist are skipped. Favorites / overrides are queued and
/// applied when each restored playlist finishes its first sync.
#[tauri::command]
pub fn backup_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    passphrase: String,
    replace: bool,
) -> CmdResult<BackupSummary> {
    let (summary, to_sync) = import_from(&state, path, &passphrase, replace)?;
    // Kick off syncs for the restored playlists (favorites/overrides land when they finish).
    for id in to_sync {
        spawn_sync(app.clone(), state.db.clone(), id, SyncScope::Full);
    }
    Ok(summary)
}

/// Restore without touching the network. Returns the summary and the playlist ids to sync.
pub fn import_from(
    state: &AppState,
    path: String,
    passphrase: &str,
    replace: bool,
) -> CmdResult<(BackupSummary, Vec<i64>)> {
    let (payload, bytes) = read_payload(&path, passphrase)?;

    if replace {
        for p in state.db.list_playlists().map_err(err)? {
            state.db.delete_playlist(p.id).map_err(err)?;
        }
    }
    let existing: Vec<PlaylistInsert> = state
        .db
        .list_playlists()
        .map_err(err)?
        .into_iter()
        .filter_map(|p| state.db.playlist_source(p.id).ok())
        .collect();

    let mut restored = 0usize;
    let mut pending_favs: Vec<(i64, Vec<String>)> = Vec::new();
    let mut pending_over: Vec<(i64, Vec<(String, String)>)> = Vec::new();
    for p in &payload.playlists {
        let dup = existing
            .iter()
            .any(|e| e.r#type == p.r#type && e.base_url == p.base_url && e.user == p.user && e.mac == p.mac);
        if dup {
            continue;
        }
        let id = state
            .db
            .insert_playlist(&PlaylistInsert {
                r#type: p.r#type.clone(),
                name: p.name.clone(),
                base_url: p.base_url.clone(),
                user: p.user.clone(),
                pass: p.pass.clone(),
                mac: p.mac.clone(),
                ua: p.ua.clone(),
            })
            .map_err(err)?;
        let _ = state.db.set_epg_offset_min(id, p.epg_offset_min);
        let _ = state.db.set_playlist_stream_format(id, &p.stream_format);
        for u in &p.epg_sources {
            let _ = state.db.add_epg_source(id, u);
        }
        if !p.favorites.is_empty() {
            pending_favs.push((id, p.favorites.clone()));
        }
        if !p.epg_overrides.is_empty() {
            pending_over.push((id, p.epg_overrides.clone()));
        }
        restored += 1;
    }
    // Merge with anything still pending from an earlier restore.
    let mut favs: Vec<(i64, Vec<String>)> = state
        .db
        .get_setting(KEY_PENDING_FAVORITES)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    favs.extend(pending_favs);
    let mut over: Vec<(i64, Vec<(String, String)>)> = state
        .db
        .get_setting(KEY_PENDING_OVERRIDES)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    over.extend(pending_over);
    let _ = state.db.set_setting(KEY_PENDING_FAVORITES, &serde_json::to_string(&favs).unwrap_or_default());
    let _ = state.db.set_setting(KEY_PENDING_OVERRIDES, &serde_json::to_string(&over).unwrap_or_default());

    // Settings: keep this machine's legal acceptance and the flag state of a newer app.
    let mut cfg = payload.config.clone();
    cfg.legal_accepted = state.db.load_config().map_err(err)?.legal_accepted;
    let _ = state.db.save_config(&cfg);
    match &payload.theme_tokens {
        Some(t) => {
            let _ = state.db.set_setting(KEY_THEME, t);
        }
        None => {
            let _ = state.db.delete_setting(KEY_THEME);
        }
    }
    if let Some(k) = &payload.parental_keywords {
        let _ = state.db.set_setting(KEY_KEYWORDS, &serde_json::to_string(k).unwrap_or_default());
        state.db.set_hidden_keywords(k);
    }

    let to_sync: Vec<i64> =
        state.db.list_playlists().map_err(err)?.into_iter().filter(|p| p.channel_count == 0).map(|p| p.id).collect();
    Ok((
        BackupSummary {
            path,
            bytes,
            playlists: restored,
            favorites: favs.iter().map(|(_, v)| v.len()).sum(),
            epg_overrides: over.iter().map(|(_, v)| v.len()).sum(),
            epg_sources: payload.playlists.iter().map(|p| p.epg_sources.len()).sum(),
        },
        to_sync,
    ))
}

/// Called by the sync pipeline after a playlist's live sync: apply queued favorites/overrides
/// whose channels now exist. Returns how many were applied.
pub fn apply_pending(db: &app_db::Db, playlist_id: i64) -> usize {
    let mut applied = 0usize;
    // favorites
    if let Some(mut favs) = db
        .get_setting(KEY_PENDING_FAVORITES)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Vec<(i64, Vec<String>)>>(&j).ok())
    {
        if let Some(pos) = favs.iter().position(|(id, _)| *id == playlist_id) {
            let (_, ids) = favs.remove(pos);
            for source_id in ids {
                if let Ok(Some(ch)) = db.channel_by_source_id(playlist_id, &source_id) {
                    if db.set_favorite("channel", ch, true).is_ok() {
                        applied += 1;
                    }
                }
            }
            let _ = db.set_setting(KEY_PENDING_FAVORITES, &serde_json::to_string(&favs).unwrap_or_default());
        }
    }
    // overrides
    if let Some(mut over) = db
        .get_setting(KEY_PENDING_OVERRIDES)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str::<Vec<(i64, Vec<(String, String)>)>>(&j).ok())
    {
        if let Some(pos) = over.iter().position(|(id, _)| *id == playlist_id) {
            let (_, rows) = over.remove(pos);
            for (source_id, tvg) in rows {
                if let Ok(Some(ch)) = db.channel_by_source_id(playlist_id, &source_id) {
                    if db.set_epg_override(ch, Some(&tvg)).is_ok() {
                        applied += 1;
                    }
                }
            }
            let _ = db.set_setting(KEY_PENDING_OVERRIDES, &serde_json::to_string(&over).unwrap_or_default());
        }
    }
    if applied > 0 {
        tracing::info!(playlist_id, applied, "restored favorites / EPG overrides applied");
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_db::channels::ChannelInsert;
    use std::sync::Arc;

    fn state() -> AppState {
        let db = Arc::new(app_db::Db::open_in_memory().unwrap());
        let engine = Arc::new(app_engine::stub::StubEngine::new(app_engine::EngineOptions::default(), "test".into()));
        AppState::new(db, engine, std::env::temp_dir())
    }

    #[test]
    fn export_then_import_restores_everything_but_machine_bound_items() {
        let a = state();
        let p =
            a.db.insert_playlist(&PlaylistInsert {
                r#type: "xtream".into(),
                name: "prov".into(),
                base_url: "http://h:8080".into(),
                user: Some("bob".into()),
                pass: Some("hunter2".into()),
                mac: None,
                ua: None,
            })
            .unwrap();
        a.db.upsert_channels(
            p,
            &[
                ChannelInsert {
                    source_id: "1".into(),
                    name: "One".into(),
                    group_title: None,
                    logo: None,
                    stream_url: "http://h/live/bob/hunter2/1.ts".into(),
                    tvg_id: None,
                    tvg_name: None,
                    catchup: false,
                    catchup_days: 0,
                },
                ChannelInsert {
                    source_id: "2".into(),
                    name: "Two".into(),
                    group_title: None,
                    logo: None,
                    stream_url: "http://h/live/bob/hunter2/2.ts".into(),
                    tvg_id: None,
                    tvg_name: None,
                    catchup: false,
                    catchup_days: 0,
                },
            ],
        )
        .unwrap();
        let two = a.db.channel_by_source_id(p, "2").unwrap().unwrap();
        a.db.set_favorite("channel", two, true).unwrap();
        a.db.set_epg_override(two, Some("two.uk")).unwrap();
        a.db.set_epg_offset_min(p, 60).unwrap();
        a.db.add_epg_source(p, "http://h/xmltv.php?username=bob&password=hunter2").unwrap();
        a.db.set_setting("theme_tokens", r##"{"--accent":"#f00"}"##).unwrap();
        a.db.set_setting("parental_pin_hash", "machine-bound").unwrap();
        let mut cfg = a.db.load_config().unwrap();
        cfg.hud_enabled = true;
        a.db.save_config(&cfg).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.diptvbk").display().to_string();
        let sum = export_to(&a, file.clone(), "correct horse").unwrap();
        assert_eq!((sum.playlists, sum.favorites, sum.epg_overrides, sum.epg_sources), (1, 1, 1, 1));
        let raw = std::fs::read(&file).unwrap();
        assert!(!raw.windows(7).any(|w| w == b"hunter2"));

        // Fresh machine.
        let b = state();
        assert!(import_from(&b, file.clone(), "wrong", false).is_err());
        let (sum, to_sync) = import_from(&b, file.clone(), "correct horse", false).unwrap();
        assert_eq!(sum.playlists, 1);
        let pl = b.db.list_playlists().unwrap();
        assert_eq!(pl.len(), 1);
        assert_eq!(to_sync, vec![pl[0].id]);
        let src = b.db.playlist_source(pl[0].id).unwrap();
        assert_eq!(src.pass.as_deref(), Some("hunter2"));
        assert_eq!(b.db.epg_offset_min(pl[0].id).unwrap(), 60);
        assert_eq!(b.db.list_epg_sources(pl[0].id).unwrap().len(), 1);
        assert!(b.db.load_config().unwrap().hud_enabled);
        assert_eq!(b.db.get_setting("theme_tokens").unwrap().as_deref(), Some(r##"{"--accent":"#f00"}"##));
        assert!(b.db.get_setting("parental_pin_hash").unwrap().is_none(), "PIN hash must not travel");

        // Favorites are pending until the channels exist…
        assert!(b.db.favorite_ids().unwrap().is_empty());
        b.db.upsert_channels(
            pl[0].id,
            &[ChannelInsert {
                source_id: "2".into(),
                name: "Two".into(),
                group_title: None,
                logo: None,
                stream_url: "http://h/2.ts".into(),
                tvg_id: None,
                tvg_name: None,
                catchup: false,
                catchup_days: 0,
            }],
        )
        .unwrap();
        assert_eq!(apply_pending(&b.db, pl[0].id), 2);
        let two_b = b.db.channel_by_source_id(pl[0].id, "2").unwrap().unwrap();
        assert_eq!(b.db.favorite_ids().unwrap(), vec![two_b]);
        assert_eq!(b.db.epg_override(two_b).unwrap().as_deref(), Some("two.uk"));
        assert_eq!(apply_pending(&b.db, pl[0].id), 0, "applied once");

        // Merge skips the duplicate; replace wipes first.
        let (sum, _) = import_from(&b, file.clone(), "correct horse", false).unwrap();
        assert_eq!(sum.playlists, 0);
        let (sum, _) = import_from(&b, file, "correct horse", true).unwrap();
        assert_eq!(sum.playlists, 1);
        assert_eq!(b.db.list_playlists().unwrap().len(), 1);
    }
}
