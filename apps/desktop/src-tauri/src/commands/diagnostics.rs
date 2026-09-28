//! Diagnostics workbench (CLAUDE.md §9): HTTP trace, source check, isolated headless probe and
//! the one-click sanitized report. Everything that leaves here is redacted in Rust — the UI only
//! appends its own (already redacted) engine log lines.

use super::{err, license_state, now_unix, CmdResult};
use crate::state::AppState;
use app_core::redact::redact;
use app_core::{PRODUCT_NAME, PRODUCT_VERSION};
use app_engine::probe::ProbeResult;
use app_engine::EngineOptions;
use app_net::adapters::{CatalogAdapter, XtreamAdapter};
use app_net::importer::{open_source, ImportSource, NoopSink};
use app_net::trace::{self, HttpTrace};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::State;

static PROBE_RUNNING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn diag_http_trace() -> Vec<HttpTrace> {
    trace::snapshot()
}

#[tauri::command]
pub fn diag_clear_trace() {
    trace::clear();
}

/// Headless libmpv probe of one URL. Never touches the main player. One at a time.
#[tauri::command]
pub async fn diag_probe(
    state: State<'_, AppState>,
    url: String,
    profile: Option<String>,
    timeout_secs: Option<u64>,
) -> CmdResult<ProbeResult> {
    let url = url.trim().to_string();
    let parsed = url::Url::parse(&url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http(s) URLs can be probed".into());
    }
    if PROBE_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("a probe is already running".into());
    }
    let cfg = state.db.load_config().map_err(err)?;
    let opts = EngineOptions {
        wid: None,
        hw_decoding: cfg.hw_decoding.clone(),
        default_profile: profile
            .as_deref()
            .and_then(app_core::ProfileMode::parse)
            .unwrap_or_else(|| app_core::ProfileMode::parse(&cfg.default_profile).unwrap_or_default()),
        stable_cache_secs: cfg.stable_cache_secs,
        audio_boost: 100,
        audio_delay_ms: 0,
        log_level: "warn".into(),
        libmpv_path: None,
        user_agent: Some(app_net::http::DEFAULT_USER_AGENT.into()),
    };
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(20).clamp(5, 120));
    let started = Instant::now();
    let url2 = url.clone();
    let res = tauri::async_runtime::spawn_blocking(move || app_engine::probe::probe_stream(opts, &url2, timeout)).await;
    PROBE_RUNNING.store(false, Ordering::SeqCst);
    let r = res.map_err(|e| format!("probe task failed: {e}"))?;
    // The probe goes through mpv's own HTTP stack, so leave one trace line for the timeline.
    trace::record(
        "probe",
        "mpv",
        &url,
        None,
        started.elapsed().as_millis() as u64,
        r.container.as_deref(),
        None,
        r.error.as_deref(),
    );
    Ok(r)
}

/// What a source check found out, without importing anything.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceCheck {
    pub playlist_id: i64,
    pub kind: String,
    pub ok: bool,
    pub elapsed_ms: u64,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub final_url: Option<String>,
    /// Preflight verdict: `m3u`, `html`, `xmltv`, `json`, …
    pub payload: Option<String>,
    pub preview: Option<String>,
    pub entries: u64,
    pub entries_without_url: u64,
    pub groups: u64,
    pub epg_hint: Option<String>,
    /// Xtream: status / expiry / connections — never the password.
    pub account: Option<app_net::adapters::AccountInfo>,
    pub error: Option<String>,
}

/// Fetch + preflight + parse-count a playlist source (M3U) or authenticate (Xtream). Read-only.
#[tauri::command]
pub async fn diag_check_source(state: State<'_, AppState>, playlist_id: i64) -> CmdResult<SourceCheck> {
    let src = state.db.playlist_source(playlist_id).map_err(err)?;
    let started = Instant::now();
    let mut out = SourceCheck { playlist_id, kind: src.r#type.clone(), ..Default::default() };
    match src.r#type.as_str() {
        "xtream" => {
            let adapter =
                XtreamAdapter::from_playlist(state.db.clone(), playlist_id, Arc::new(NoopSink)).map_err(err)?;
            match adapter.authenticate().await {
                Ok(info) => {
                    out.ok = info.status.as_deref().map(|s| s.eq_ignore_ascii_case("active")).unwrap_or(true);
                    out.status = Some(200);
                    out.account = Some(info);
                }
                Err(e) => out.error = Some(err(e)),
            }
        }
        "m3u" => {
            let source = if src.base_url.starts_with("http://") || src.base_url.starts_with("https://") {
                ImportSource::Url { url: src.base_url.clone(), user_agent: src.ua.clone() }
            } else {
                ImportSource::File { path: src.base_url.clone().into() }
            };
            match check_m3u(&source, &mut out).await {
                Ok(()) => {}
                Err(e) => out.error = Some(err(e)),
            }
        }
        "stalker" => {
            let adapter =
                app_net::adapters::StalkerAdapter::from_playlist(state.db.clone(), playlist_id, Arc::new(NoopSink))
                    .map_err(err)?;
            match adapter.authenticate().await {
                Ok(info) => {
                    out.ok = true;
                    out.status = Some(200);
                    out.account = Some(info);
                }
                Err(e) => out.error = Some(err(e)),
            }
        }
        other => out.error = Some(format!("source check not implemented for {other}")),
    }
    out.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(out)
}

async fn check_m3u(source: &ImportSource, out: &mut SourceCheck) -> app_net::Result<()> {
    use app_net::m3u::{M3uEntry, M3uParser};
    use app_net::preflight::{self, SNIFF_BYTES};
    let opened = open_source(source).await?;
    out.status = opened.status;
    out.content_type = opened.content_type.clone();
    out.content_length = opened.content_length;
    out.final_url = opened.final_url_redacted.clone();
    let mut stream = opened.stream;
    let mut head: Vec<u8> = Vec::with_capacity(SNIFF_BYTES);
    let mut chunks: Vec<bytes::Bytes> = Vec::new();
    while head.len() < SNIFF_BYTES {
        match stream.next().await {
            Some(c) => {
                let c = c?;
                head.extend_from_slice(&c[..c.len().min(SNIFF_BYTES - head.len())]);
                chunks.push(c);
            }
            None => break,
        }
    }
    let kind = preflight::sniff(&head, out.content_type.as_deref());
    out.payload = Some(format!("{kind:?}").to_lowercase());
    out.preview = Some(preflight::preview_of(&head));
    if let Err(e) = preflight::check(&head, out.content_type.as_deref()) {
        out.error = Some(e.message.clone());
        return Ok(());
    }
    // Count entries through the streaming parser — no DB writes, bounded work.
    let mut parser = M3uParser::new();
    let mut entries: Vec<M3uEntry> = Vec::new();
    let mut groups = std::collections::HashSet::new();
    let mut handle = |entries: &mut Vec<M3uEntry>, out: &mut SourceCheck| {
        for e in entries.drain(..) {
            if e.url.is_empty() {
                out.entries_without_url += 1;
            } else {
                out.entries += 1;
            }
            if let Some(g) = e.group_title {
                groups.insert(g);
            }
        }
    };
    for c in chunks.drain(..) {
        parser.feed(&c, &mut entries);
        handle(&mut entries, out);
    }
    let mut bytes = 0u64;
    while let Some(c) = stream.next().await {
        let c = c?;
        bytes += c.len() as u64;
        parser.feed(&c, &mut entries);
        handle(&mut entries, out);
        if bytes > 512 * 1024 * 1024 {
            out.error = Some("stopped counting after 512 MB — this is not a playlist".into());
            return Ok(());
        }
    }
    parser.finish(&mut entries);
    handle(&mut entries, out);
    out.groups = groups.len() as u64;
    out.epg_hint = parser.header().tvg_url.as_deref().map(redact);
    out.ok = out.entries > 0;
    if out.entries == 0 {
        out.error = Some("no channels found in the playlist".into());
    }
    Ok(())
}

/// The sanitized report body. The UI appends its engine log + telemetry lines.
#[tauri::command]
pub fn diag_report(state: State<'_, AppState>) -> CmdResult<String> {
    let mut s = String::new();
    let cfg = state.db.load_config().map_err(err)?;
    let lic = license_state(&state)?;
    let now = now_unix();
    s.push_str(&format!(
        "{PRODUCT_NAME} {PRODUCT_VERSION} · {} {} · {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        fmt_unix(now)
    ));
    s.push_str(&format!("engine: {}\n", state.engine.describe()));
    s.push_str(&format!(
        "config: profile={} hwdec={} stable_cache={}s boost={}% delay={}ms multiscreen_max={} hud={} hide_vod={} theme={}\n",
        cfg.default_profile,
        cfg.hw_decoding,
        cfg.stable_cache_secs,
        cfg.audio_boost,
        cfg.audio_delay_ms,
        cfg.max_multiscreen_instances,
        cfg.hud_enabled,
        cfg.hide_vod_tabs,
        cfg.app_theme
    ));
    s.push_str(&format!(
        "license: tier={} valid={}{}\n",
        lic.tier,
        lic.is_valid,
        lic.expires_at.map(|t| format!(" expires={}", fmt_unix(t))).unwrap_or_default()
    ));
    s.push_str(&format!("parental: {} keywords hidden\n", state.db.hidden_keywords().len()));

    // ---- playback ----
    let pb = super::playback::playback_state(&state);
    let t = state.engine.telemetry();
    s.push_str(&format!(
        "\nplayback: {:?} url={} profile={} vol={}{}{}\n",
        pb.item,
        pb.stream_url_redacted.as_deref().unwrap_or("-"),
        pb.profile,
        pb.volume,
        if pb.muted { " muted" } else { "" },
        if pb.paused { " paused" } else { "" }
    ));
    s.push_str(&format!(
        "telemetry: {}x{} {} {} kbps {:.2} fps dropped={} cache={:.1}s underrun={} zap={}\n",
        t.width,
        t.height,
        t.codec_name,
        t.bitrate_kbps,
        t.fps,
        t.dropped_frames,
        t.cache_duration_secs,
        t.is_underrun,
        t.zap_ms.map(|z| format!("{z} ms")).unwrap_or_else(|| "-".into())
    ));
    let panes = state.panes.lock().unwrap();
    if !panes.is_empty() {
        s.push_str(&format!(
            "panes: {} extra ({})\n",
            panes.len(),
            panes
                .iter()
                .map(|(l, p)| format!("{l}:{}", if p.playing { "playing" } else { "idle" }))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    drop(panes);

    // ---- sources ----
    s.push_str("\nsources:\n");
    for p in state.db.list_playlists().map_err(err)? {
        let meta = state.db.playlist_meta(p.id).ok();
        s.push_str(&format!(
            "  #{} {} \"{}\" {} · {} channels",
            p.id, p.r#type, p.name, p.base_url_redacted, p.channel_count
        ));
        if let Some(m) = &meta {
            s.push_str(&format!(" · format={} · epg_offset={}min", m.stream_format, m.epg_offset_min));
            if let Some(ls) = &m.last_synced {
                s.push_str(&format!(" · synced={ls}"));
            }
        }
        s.push('\n');
        if let Some(m) = &meta {
            if let Some(e) = &m.last_error {
                s.push_str(&format!("     last error: {}\n", redact(e)));
            }
            if let Some(a) = m.account_json.as_deref().and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) {
                let g = |k: &str| {
                    a.get(k).map(|v| v.to_string().trim_matches('"').to_string()).unwrap_or_else(|| "-".into())
                };
                s.push_str(&format!(
                    "     account: status={} exp={} max_connections={} active={} trial={} formats={}\n",
                    g("status"),
                    a.get("exp_date").and_then(|v| v.as_i64()).map(fmt_unix).unwrap_or_else(|| "-".into()),
                    g("max_connections"),
                    g("active_cons"),
                    g("is_trial"),
                    g("allowed_output_formats")
                ));
            }
        }
        if let Ok(e) = state.db.epg_stats(p.id) {
            s.push_str(&format!(
                "     epg: {} programmes for {} channels{}\n",
                e.programmes,
                e.channels_with_epg,
                match (e.min_start, e.max_stop) {
                    (Some(a), Some(b)) => format!(" ({} → {})", fmt_unix(a), fmt_unix(b)),
                    _ => String::new(),
                }
            ));
        }
        if let Ok(srcs) = state.db.list_epg_sources(p.id) {
            for e in srcs {
                s.push_str(&format!(
                    "     epg source: {}{}\n",
                    redact(&e.url_redacted),
                    e.last_error.as_deref().map(|x| format!(" · error: {}", redact(x))).unwrap_or_default()
                ));
            }
        }
        for (kind, label) in [("movie", "movies"), ("series", "series")] {
            if let Ok(n) = state.db.vod_count(p.id, kind, None) {
                if n > 0 {
                    s.push_str(&format!("     {label}: {n}\n"));
                }
            }
        }
    }

    // ---- jobs ----
    if let Ok(recs) = state.db.list_recordings() {
        let active = recs.iter().filter(|r| r.status == "recording").count();
        let scheduled = recs.iter().filter(|r| r.status == "scheduled").count();
        let failed = recs.iter().filter(|r| r.status == "failed").count();
        s.push_str(&format!(
            "\nrecordings: {} active, {} scheduled, {} failed, {} total\n",
            active,
            scheduled,
            failed,
            recs.len()
        ));
        for r in recs.iter().filter(|r| r.status == "failed").take(5) {
            s.push_str(&format!("  failed #{}: {}\n", r.id, r.error.as_deref().map(redact).unwrap_or_default()));
        }
    }
    if let Ok(dls) = state.db.list_downloads() {
        if !dls.is_empty() {
            s.push_str(&format!(
                "downloads: {}\n",
                dls.iter().map(|d| format!("#{} {}", d.id, d.status)).collect::<Vec<_>>().join(", ")
            ));
        }
    }

    // ---- http trace ----
    let traces = trace::snapshot();
    s.push_str(&format!("\nhttp trace ({} requests, newest last, secrets redacted):\n", traces.len()));
    s.push_str(&trace::render(&traces));
    Ok(redact(&s))
}

fn fmt_unix(t: i64) -> String {
    // UTC, no chrono dependency: days since epoch → civil date (Howard Hinnant's algorithm).
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs / 60) % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::fmt_unix;

    #[test]
    fn formats_unix_time() {
        assert_eq!(fmt_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(fmt_unix(1_893_456_000), "2030-01-01T00:00:00Z");
        assert_eq!(fmt_unix(951_782_400), "2000-02-29T00:00:00Z");
    }
}
