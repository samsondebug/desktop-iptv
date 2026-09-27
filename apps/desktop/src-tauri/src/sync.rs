//! Background catalog/EPG/VOD synchronisation. One pipeline per playlist type; every stage
//! reports through `import_progress` / `import_done` events so the UI never blocks on it.

use crate::{EV_IMPORT_DONE, EV_IMPORT_PROGRESS};
use app_core::redact::redact;
use app_core::{ImportProgressEvent, SyncStats};
use app_net::adapters::{CatalogAdapter, XtreamAdapter};
use app_net::importer::{ImportSource, ProgressSink};
use app_net::xmltv::{import_xmltv, XmltvOptions};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// Which part of a playlist to sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScope {
    /// Live (+ EPG afterwards, + VOD for Xtream).
    Full,
    EpgOnly,
    VodOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportDone {
    pub playlist_id: i64,
    /// "live" | "epg" | "vod" | "account"
    pub phase: String,
    pub ok: bool,
    pub stats: Option<SyncStats>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ProgressPayload {
    playlist_id: i64,
    phase: String,
    stage: String,
    channels: u64,
    bytes: u64,
    message: Option<String>,
}

/// Forwards importer progress to the webview, tagging it with the current phase.
pub struct EmitSink {
    app: AppHandle,
    phase: Mutex<String>,
}

impl EmitSink {
    pub fn new(app: AppHandle, phase: &str) -> Arc<Self> {
        Arc::new(Self { app, phase: Mutex::new(phase.into()) })
    }
    pub fn set_phase(&self, phase: &str) {
        *self.phase.lock().unwrap() = phase.into();
    }
}

impl ProgressSink for EmitSink {
    fn progress(&self, ev: ImportProgressEvent) {
        let phase = self.phase.lock().unwrap().clone();
        let _ = self.app.emit(
            EV_IMPORT_PROGRESS,
            ProgressPayload {
                playlist_id: ev.playlist_id,
                phase,
                stage: ev.stage,
                channels: ev.channels,
                bytes: ev.bytes,
                message: ev.message,
            },
        );
    }
}

/// Playlists with a sync in flight (prevents double refreshes).
static IN_FLIGHT: std::sync::LazyLock<Mutex<HashSet<i64>>> = std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn is_syncing(playlist_id: i64) -> bool {
    IN_FLIGHT.lock().unwrap().contains(&playlist_id)
}

fn done(app: &AppHandle, playlist_id: i64, phase: &str, result: Result<SyncStats, app_net::NetError>) -> bool {
    let payload = match result {
        Ok(stats) => ImportDone { playlist_id, phase: phase.into(), ok: true, stats: Some(stats), error: None, error_kind: None, preview: None },
        Err(app_net::NetError::Preflight(p)) => ImportDone {
            playlist_id,
            phase: phase.into(),
            ok: false,
            stats: None,
            error: Some(p.message.clone()),
            error_kind: Some(format!("{:?}", p.kind).to_lowercase()),
            preview: Some(p.preview.clone()),
        },
        Err(e) => ImportDone {
            playlist_id,
            phase: phase.into(),
            ok: false,
            stats: None,
            error: Some(redact(&e.to_string())),
            error_kind: Some("network".into()),
            preview: None,
        },
    };
    let ok = payload.ok;
    let _ = app.emit(EV_IMPORT_DONE, &payload);
    ok
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Kick off a sync for `playlist_id`. Returns false if one is already running.
pub fn spawn_sync(app: AppHandle, db: Arc<app_db::Db>, playlist_id: i64, scope: SyncScope) -> bool {
    {
        let mut set = IN_FLIGHT.lock().unwrap();
        if !set.insert(playlist_id) {
            return false;
        }
    }
    tauri::async_runtime::spawn(async move {
        let result = run_sync(&app, db.clone(), playlist_id, scope).await;
        if let Err(e) = result {
            tracing::warn!(playlist_id, error = %redact(&e.to_string()), "sync pipeline aborted");
        }
        IN_FLIGHT.lock().unwrap().remove(&playlist_id);
    });
    true
}

async fn run_sync(app: &AppHandle, db: Arc<app_db::Db>, playlist_id: i64, scope: SyncScope) -> Result<(), app_net::NetError> {
    let src = db.playlist_source(playlist_id)?;
    let sink = EmitSink::new(app.clone(), "live");

    match src.r#type.as_str() {
        "xtream" => {
            let adapter = XtreamAdapter::from_playlist(db.clone(), playlist_id, sink.clone())?;
            if scope == SyncScope::Full {
                sink.set_phase("account");
                if let Err(e) = adapter.authenticate().await {
                    done(app, playlist_id, "account", Err(e));
                    return Ok(());
                }
                sink.set_phase("live");
                let live = adapter.sync_live(playlist_id).await;
                let ok = done(app, playlist_id, "live", live);
                if ok {
                    let _ = db.ensure_trial_started(now_unix());
                }
            }
            if matches!(scope, SyncScope::Full | SyncScope::EpgOnly) {
                sink.set_phase("epg");
                let epg = adapter.sync_epg(playlist_id).await;
                done(app, playlist_id, "epg", epg);
            }
            if matches!(scope, SyncScope::Full | SyncScope::VodOnly) {
                sink.set_phase("vod");
                let vod = adapter.sync_vod(playlist_id).await;
                done(app, playlist_id, "vod", vod);
            }
        }
        _ => {
            // M3U (and, later, Stalker): live from the playlist URL/file, EPG from epg_sources.
            if scope == SyncScope::Full {
                let import_src = if let Some(path) = src.base_url.strip_prefix("file://") {
                    ImportSource::File { path: path.into() }
                } else {
                    ImportSource::Url { url: src.base_url.clone(), user_agent: src.ua.clone() }
                };
                let live = app_net::import_m3u(db.clone(), playlist_id, import_src, sink.clone()).await;
                // Auto-register the EPG the playlist advertises (once).
                if let Ok(stats) = &live {
                    if let Some(url) = &stats.epg_url {
                        if db.list_epg_sources(playlist_id).map(|v| v.is_empty()).unwrap_or(false) {
                            let _ = db.add_epg_source(playlist_id, url);
                        }
                    }
                    if stats.inserted + stats.updated > 0 {
                        let _ = db.ensure_trial_started(now_unix());
                    }
                }
                let _ = db.set_playlist_sync_result(playlist_id, live.as_ref().err().map(|e| e.to_string()).as_deref());
                done(app, playlist_id, "live", live);
            }
            if matches!(scope, SyncScope::Full | SyncScope::EpgOnly) {
                sink.set_phase("epg");
                let sources = db.enabled_epg_sources(playlist_id)?;
                if sources.is_empty() {
                    if scope == SyncScope::EpgOnly {
                        done(app, playlist_id, "epg", Err(app_net::NetError::Other("No EPG source configured for this playlist. Add an XMLTV URL in Settings → Guide.".into())));
                    }
                } else {
                    let mut total = SyncStats::default();
                    let mut last_err: Option<app_net::NetError> = None;
                    for (sid, url) in sources {
                        let r = import_xmltv(
                            db.clone(),
                            playlist_id,
                            ImportSource::Url { url, user_agent: src.ua.clone() },
                            sink.clone(),
                            XmltvOptions::default(),
                        )
                        .await;
                        match r {
                            Ok(s) => {
                                let _ = db.mark_epg_source(sid, s.inserted as i64, None);
                                total.inserted += s.inserted;
                                total.skipped += s.skipped;
                                total.groups += s.groups;
                                total.elapsed_ms += s.elapsed_ms;
                                total.warnings.extend(s.warnings);
                            }
                            Err(e) => {
                                let _ = db.mark_epg_source(sid, 0, Some(&e.to_string()));
                                last_err = Some(e);
                            }
                        }
                    }
                    let result = match (total.inserted, last_err) {
                        (0, Some(e)) => Err(e),
                        (_, Some(e)) => {
                            total.warnings.push(format!("one EPG source failed: {}", redact(&e.to_string())));
                            Ok(total)
                        }
                        (_, None) => Ok(total),
                    };
                    done(app, playlist_id, "epg", result);
                }
            }
        }
    }
    Ok(())
}
