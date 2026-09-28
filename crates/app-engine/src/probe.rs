//! Isolated headless stream probe for the diagnostics workbench (CLAUDE.md §9 item 5).
//!
//! A throw-away libmpv instance (`vo=null`, `ao=null`, no window) loads the URL exactly the way
//! the player would — same profile options, UA and hwdec setting — and reports what the demuxer
//! and decoder saw: container, codecs, resolution, time-to-first-frame, or the error string.
//! Nothing is drawn, nothing is heard, and the main player is untouched, so a user can test a
//! URL while a channel keeps playing.

use crate::{mpv::MpvEngine, EngineEvent, EngineOptions, PlayerEngine};
use app_core::redact::redact;
use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeResult {
    pub ok: bool,
    pub url: String,
    pub profile: String,
    pub engine: String,
    pub elapsed_ms: u64,
    /// Load → first decoded frame (`MPV_EVENT_PLAYBACK_RESTART`).
    pub ttff_ms: Option<u64>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub hwdec: Option<String>,
    pub cache_secs: Option<f64>,
    pub error: Option<String>,
    /// Redacted mpv warn/error lines emitted during the probe (newest last, capped).
    pub log: Vec<String>,
}

const MAX_LOG_LINES: usize = 40;

/// Blocking. Call from `spawn_blocking`. `timeout` bounds the whole probe (connect + first frame).
pub fn probe_stream(mut opts: EngineOptions, url: &str, timeout: Duration) -> ProbeResult {
    let started = Instant::now();
    let mut out = ProbeResult { url: redact(url), profile: opts.default_profile.as_str().into(), ..Default::default() };

    if std::env::var("DESKTOP_IPTV_ENGINE").map(|v| v == "stub").unwrap_or(false) {
        out.engine = "stub".into();
        out.error = Some("stub engine (DESKTOP_IPTV_ENGINE=stub): no libmpv to probe with".into());
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        return out;
    }
    opts.wid = None; // headless: vo=null / ao=null
    opts.log_level = "warn".into();
    let engine = match MpvEngine::new(opts) {
        Ok(e) => e,
        Err(e) => {
            out.engine = "none".into();
            out.error = Some(format!("libmpv unavailable: {e}"));
            out.elapsed_ms = started.elapsed().as_millis() as u64;
            return out;
        }
    };
    out.engine = engine.describe();

    let (tx, rx) = mpsc::channel::<EngineEvent>();
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    engine.set_listener(Arc::new(move |ev: EngineEvent| {
        if let EngineEvent::Log { level, text } = &ev {
            if let Ok(mut l) = log2.lock() {
                if l.len() >= MAX_LOG_LINES {
                    l.remove(0);
                }
                l.push(format!("[{level}] {text}"));
            }
            return;
        }
        let _ = tx.send(ev);
    }));

    let profile = engine.active_profile();
    if let Err(e) = engine.load(url, profile, 100, None) {
        out.error = Some(redact(&e.to_string()));
        engine.shutdown();
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        out.log = log.lock().map(|l| l.clone()).unwrap_or_default();
        return out;
    }

    let deadline = started + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            out.error = Some(format!("no first frame within {} s (still connecting or buffering)", timeout.as_secs()));
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(EngineEvent::PlaybackStarted { zap_ms, .. }) => {
                out.ok = true;
                out.ttff_ms = Some(zap_ms);
                break;
            }
            Ok(EngineEvent::EndFile { reason, error, .. }) => {
                out.error = Some(match error {
                    Some(e) => e,
                    None => format!("stream ended before the first frame (reason: {reason})"),
                });
                break;
            }
            Ok(EngineEvent::Shutdown) => {
                out.error = Some("engine shut down".into());
                break;
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                out.error =
                    Some(format!("no first frame within {} s (still connecting or buffering)", timeout.as_secs()));
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                out.error = Some("engine event loop stopped".into());
                break;
            }
        }
    }

    // Whatever the demuxer learned, even on failure (a codec may be known before decode fails).
    let get = |name: &str| engine.get_property(name).ok().flatten().filter(|s| !s.is_empty());
    out.container = get("file-format");
    out.video_codec = get("video-format").or_else(|| get("video-codec"));
    out.audio_codec = get("audio-codec-name").or_else(|| get("audio-codec"));
    out.width = get("video-params/w").and_then(|s| s.parse().ok());
    out.height = get("video-params/h").and_then(|s| s.parse().ok());
    out.fps = get("container-fps").and_then(|s| s.parse().ok());
    out.hwdec = get("hwdec-current");
    out.cache_secs = get("demuxer-cache-duration").and_then(|s| s.parse().ok());
    engine.shutdown();
    out.elapsed_ms = started.elapsed().as_millis() as u64;
    out.log = log.lock().map(|l| l.clone()).unwrap_or_default();
    if let Some(e) = &out.error {
        out.error = Some(redact(e));
    }
    out
}

impl ProbeResult {
    /// One-paragraph verdict for the report.
    pub fn summary(&self) -> String {
        if self.ok {
            format!(
                "OK: first frame in {} ms · {} · video {}{} · audio {} · hwdec {}",
                self.ttff_ms.unwrap_or(0),
                self.container.as_deref().unwrap_or("?"),
                self.video_codec.as_deref().unwrap_or("?"),
                match (self.width, self.height) {
                    (Some(w), Some(h)) =>
                        format!(" {w}×{h}{}", self.fps.map(|f| format!(" @ {f:.3}")).unwrap_or_default()),
                    _ => String::new(),
                },
                self.audio_codec.as_deref().unwrap_or("?"),
                self.hwdec.as_deref().unwrap_or("no")
            )
        } else {
            format!(
                "FAILED after {} ms: {}{}",
                self.elapsed_ms,
                self.error.as_deref().unwrap_or("unknown"),
                self.container.as_deref().map(|c| format!(" (container detected: {c})")).unwrap_or_default()
            )
        }
    }
}
