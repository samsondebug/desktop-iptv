//! Playlists, channels, groups, search, favorites, recents.

use super::{err, license_state, CmdResult};
use crate::state::AppState;
use crate::sync::{spawn_sync, SyncScope};
use app_core::*;
use app_db::channels::{PlaylistInsert, PlaylistMeta};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddPlaylistSource {
    M3uUrl {
        url: String,
        user_agent: Option<String>,
        epg_url: Option<String>,
    },
    M3uFile {
        path: String,
        epg_url: Option<String>,
    },
    Xtream {
        base_url: String,
        username: String,
        password: String,
        stream_format: Option<String>,
        user_agent: Option<String>,
    },
    /// Experimental (Settings → Advanced → Stalker portals).
    Stalker {
        portal_url: String,
        mac: String,
        user_agent: Option<String>,
        epg_url: Option<String>,
    },
}

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.to_string())).unwrap_or_else(|| "Playlist".into())
}

#[tauri::command]
pub fn add_playlist(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    source: AddPlaylistSource,
) -> CmdResult<i64> {
    let name = name.trim();
    // Free tier after the trial: one playlist only (CLAUDE.md §10).
    let lic = license_state(&state)?;
    if !app_core::license::allows(&lic.tier, app_core::license::Feature::MultiplePlaylists)
        && !state.db.list_playlists().map_err(err)?.is_empty()
    {
        return Err("The free tier supports one playlist. Unlock PRO for unlimited playlists.".into());
    }

    let (row, epg_url, stream_format) = match &source {
        AddPlaylistSource::M3uUrl { url, user_agent, epg_url } => {
            let url = url.trim().to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("Playlist URL must start with http:// or https://".into());
            }
            (
                PlaylistInsert {
                    r#type: "m3u".into(),
                    name: if name.is_empty() { host_of(&url) } else { name.to_string() },
                    base_url: url,
                    user: None,
                    pass: None,
                    mac: None,
                    ua: user_agent.clone().filter(|s| !s.trim().is_empty()),
                },
                epg_url.clone(),
                None,
            )
        }
        AddPlaylistSource::M3uFile { path, epg_url } => {
            let p = std::path::PathBuf::from(path);
            if !p.is_file() {
                return Err("Playlist file not found".into());
            }
            (
                PlaylistInsert {
                    r#type: "m3u".into(),
                    name: if name.is_empty() {
                        p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Playlist".into())
                    } else {
                        name.to_string()
                    },
                    base_url: format!("file://{}", p.display()),
                    user: None,
                    pass: None,
                    mac: None,
                    ua: None,
                },
                epg_url.clone(),
                None,
            )
        }
        AddPlaylistSource::Xtream { base_url, username, password, stream_format, user_agent } => {
            let client = app_net::adapters::XtreamClient::new(base_url, username, password, user_agent.as_deref())
                .map_err(err)?;
            if username.trim().is_empty() || password.is_empty() {
                return Err("Username and password are required".into());
            }
            (
                PlaylistInsert {
                    r#type: "xtream".into(),
                    name: if name.is_empty() { host_of(client.base_url()) } else { name.to_string() },
                    base_url: client.base_url().to_string(),
                    user: Some(username.trim().to_string()),
                    pass: Some(password.clone()),
                    mac: None,
                    ua: user_agent.clone().filter(|s| !s.trim().is_empty()),
                },
                None,
                stream_format.clone(),
            )
        }
        AddPlaylistSource::Stalker { portal_url, mac, user_agent, epg_url } => {
            if !state.db.load_config().map_err(err)?.stalker_enabled {
                return Err("Stalker portals are experimental — enable them in Settings → Advanced first.".into());
            }
            let mac = app_net::adapters::stalker::normalize_mac(mac)
                .ok_or("MAC address must be 12 hex digits, e.g. 00:1A:79:12:34:56")?;
            let candidates = app_net::adapters::stalker::endpoint_candidates(portal_url).map_err(err)?;
            let base = portal_url.trim().to_string();
            (
                PlaylistInsert {
                    r#type: "stalker".into(),
                    name: if name.is_empty() { host_of(&candidates[0]) } else { name.to_string() },
                    base_url: base,
                    user: None,
                    pass: None,
                    mac: Some(mac),
                    ua: user_agent.clone().filter(|s| !s.trim().is_empty()),
                },
                epg_url.clone(),
                None,
            )
        }
    };
    let id = state.db.insert_playlist(&row).map_err(err)?;
    if let Some(f) = stream_format {
        state.db.set_playlist_stream_format(id, &f).map_err(err)?;
    }
    if let Some(u) = epg_url.filter(|u| u.trim().starts_with("http")) {
        state.db.add_epg_source(id, u.trim()).map_err(err)?;
    }
    spawn_sync(app, state.db.clone(), id, SyncScope::Full);
    Ok(id)
}

#[tauri::command]
pub fn refresh_playlist(app: AppHandle, state: State<'_, AppState>, playlist_id: i64) -> CmdResult<bool> {
    state.db.playlist_source(playlist_id).map_err(err)?;
    Ok(spawn_sync(app, state.db.clone(), playlist_id, SyncScope::Full))
}

#[tauri::command]
pub fn delete_playlist(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<()> {
    state.db.delete_playlist(playlist_id).map_err(err)
}

#[tauri::command]
pub fn rename_playlist(state: State<'_, AppState>, playlist_id: i64, name: String) -> CmdResult<()> {
    if name.trim().is_empty() {
        return Err("Name cannot be empty".into());
    }
    state.db.rename_playlist(playlist_id, &name).map_err(err)
}

#[tauri::command]
pub fn list_playlists(state: State<'_, AppState>) -> CmdResult<Vec<PlaylistSummary>> {
    state.db.list_playlists().map_err(err)
}

#[tauri::command]
pub fn playlist_meta(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<PlaylistMeta> {
    state.db.playlist_meta(playlist_id).map_err(err)
}

/// Xtream: switch live URLs between .ts and .m3u8 (takes effect on the next refresh).
#[tauri::command]
pub fn set_stream_format(
    app: AppHandle,
    state: State<'_, AppState>,
    playlist_id: i64,
    format: String,
) -> CmdResult<()> {
    state.db.set_playlist_stream_format(playlist_id, &format).map_err(err)?;
    spawn_sync(app, state.db.clone(), playlist_id, SyncScope::Full);
    Ok(())
}

#[tauri::command]
pub fn is_syncing(playlist_id: i64) -> bool {
    crate::sync::is_syncing(playlist_id)
}

// ---------- catalog ----------

#[tauri::command]
pub fn list_groups(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<Vec<GroupSummary>> {
    state.db.list_groups(playlist_id).map_err(err)
}

#[tauri::command]
pub fn list_channels(state: State<'_, AppState>, req: ListChannelsRequest) -> CmdResult<Vec<ChannelRecord>> {
    state.db.list_channels(req.playlist_id, req.group_title.as_deref(), req.limit, req.offset).map_err(err)
}

#[tauri::command]
pub fn count_channels(state: State<'_, AppState>, playlist_id: i64, group_title: Option<String>) -> CmdResult<i64> {
    state.db.channel_count(playlist_id, group_title.as_deref()).map_err(err)
}

#[tauri::command]
pub fn search_channels(state: State<'_, AppState>, req: FtsQueryRequest) -> CmdResult<Vec<ChannelRecord>> {
    state.db.search_channels(&req.query_string, req.playlist_id, req.limit, req.offset).map_err(err)
}

#[tauri::command]
pub fn get_channel(state: State<'_, AppState>, channel_id: i64) -> CmdResult<ChannelRecord> {
    state.db.get_channel(channel_id).map_err(err)
}

#[tauri::command]
pub fn get_favorites(state: State<'_, AppState>) -> CmdResult<Vec<ChannelRecord>> {
    state.db.favorite_channels().map_err(err)
}

#[tauri::command]
pub fn get_favorite_ids(state: State<'_, AppState>) -> CmdResult<Vec<i64>> {
    state.db.favorite_ids().map_err(err)
}

#[tauri::command]
pub fn set_favorite(state: State<'_, AppState>, channel_id: i64, on: bool) -> CmdResult<()> {
    state.db.set_favorite("channel", channel_id, on).map_err(err)
}

#[tauri::command]
pub fn get_recents(state: State<'_, AppState>) -> CmdResult<Vec<ChannelRecord>> {
    state.db.recent_channels(30).map_err(err)
}

// ---------- curation (rename / hide) ----------

/// Rename a channel's display name (None/empty clears back to the source name).
#[tauri::command]
pub fn rename_channel(state: State<'_, AppState>, channel_id: i64, name: Option<String>) -> CmdResult<()> {
    state.db.set_channel_name(channel_id, name.as_deref()).map_err(err)
}

#[tauri::command]
pub fn set_channel_hidden(state: State<'_, AppState>, channel_id: i64, hidden: bool) -> CmdResult<()> {
    state.db.set_channel_hidden(channel_id, hidden).map_err(err)
}

#[tauri::command]
pub fn set_group_hidden(
    state: State<'_, AppState>,
    playlist_id: i64,
    group_title: String,
    hidden: bool,
) -> CmdResult<()> {
    state.db.set_group_hidden(playlist_id, &group_title, hidden).map_err(err)
}

/// Everything currently hidden or renamed (Settings → Playlists → Hidden & renamed).
#[tauri::command]
pub fn curation_state(state: State<'_, AppState>) -> CmdResult<app_db::curation::CurationState> {
    state.db.curation_state().map_err(err)
}

// ---------- channel-name cleanup rules ----------

#[derive(Debug, Clone, Serialize)]
pub struct NameRulePreviewRow {
    pub before: String,
    pub after: String,
}

#[tauri::command]
pub fn get_name_rules(state: State<'_, AppState>) -> CmdResult<Vec<app_db::namerules::NameRule>> {
    state.db.load_name_rules().map_err(err)
}

#[derive(Debug, Clone, Serialize)]
pub struct NameRulesResult {
    pub rules: usize,
    pub changed: u64,
}

/// Save (and compile) the rules; optionally rewrite existing channel names right away.
/// New imports/refreshes always apply the saved rules.
#[tauri::command]
pub fn set_name_rules(
    state: State<'_, AppState>,
    rules: Vec<app_db::namerules::NameRule>,
    apply_now: bool,
) -> CmdResult<NameRulesResult> {
    let n = state.db.set_name_rules(&rules)?;
    let changed = if apply_now { state.db.reapply_name_rules().map_err(err)? } else { 0 };
    Ok(NameRulesResult { rules: n, changed })
}

/// Dry-run `rules` against this playlist's current names (nothing is saved or changed).
#[tauri::command]
pub fn preview_name_rules(
    state: State<'_, AppState>,
    rules: Vec<app_db::namerules::NameRule>,
    playlist_id: i64,
) -> CmdResult<Vec<NameRulePreviewRow>> {
    Ok(state
        .db
        .preview_name_rules(&rules, playlist_id, 12)?
        .into_iter()
        .map(|(before, after)| NameRulePreviewRow { before, after })
        .collect())
}
