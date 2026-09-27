//! Fake player for UI development and CI (CLAUDE.md §3 step 4). Logs every `loadfile`,
//! emits plausible telemetry and a `PlaybackStarted` ~350 ms after each load.

use crate::{EngineEvent, EngineOptions, EventListener, PlayerEngine, Result};
use app_core::{EngineTelemetryEvent, ProfileMode};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct StubEngine {
    reason: String,
    profile: Mutex<ProfileMode>,
    listener: Mutex<Option<EventListener>>,
    telemetry: Mutex<EngineTelemetryEvent>,
    props: Mutex<std::collections::HashMap<String, String>>,
    generation: AtomicU64,
    shut: AtomicBool,
    started: Instant,
}

impl StubEngine {
    pub fn new(opts: EngineOptions, reason: String) -> Self {
        Self {
            reason,
            profile: Mutex::new(opts.default_profile),
            listener: Mutex::new(None),
            telemetry: Mutex::new(EngineTelemetryEvent {
                active_profile: opts.default_profile.as_str().into(),
                ..Default::default()
            }),
            props: Mutex::new(Default::default()),
            generation: AtomicU64::new(0),
            shut: AtomicBool::new(false),
            started: Instant::now(),
        }
    }

    fn emit(&self, ev: EngineEvent) {
        if let Some(l) = self.listener.lock().unwrap().clone() {
            l(ev);
        }
    }
}

impl PlayerEngine for StubEngine {
    fn kind(&self) -> &'static str {
        "stub"
    }

    fn describe(&self) -> String {
        format!("stub engine (no video) — {}", self.reason)
    }

    fn load(&self, url: &str, profile: ProfileMode, audio_boost: u16) -> Result<()> {
        let redacted = app_core::redact::redact(url);
        tracing::info!(url = %redacted, profile = profile.as_str(), audio_boost, "[stub] loadfile replace");
        *self.profile.lock().unwrap() = profile;
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut t = self.telemetry.lock().unwrap();
            *t = EngineTelemetryEvent { active_profile: profile.as_str().into(), zap_ms: None, ..Default::default() };
        }
        let listener = self.listener.lock().unwrap().clone();
        let tele = Arc::new(Mutex::new(self.telemetry.lock().unwrap().clone()));
        let t0 = Instant::now();
        let started_at = self.started;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(350));
            let zap = t0.elapsed().as_millis() as u64;
            let snapshot = EngineTelemetryEvent {
                active_profile: profile.as_str().into(),
                width: 1920,
                height: 1080,
                codec_name: "h264 (stub)".into(),
                bitrate_kbps: 4200,
                fps: 50.0,
                dropped_frames: 0,
                cache_duration_secs: if profile == ProfileMode::LowLatency { 2.4 } else { 18.9 },
                is_underrun: false,
                zap_ms: Some(zap),
            };
            *tele.lock().unwrap() = snapshot.clone();
            if let Some(l) = &listener {
                l(EngineEvent::PlaybackStarted { url_redacted: redacted.clone(), zap_ms: zap });
                l(EngineEvent::Telemetry(snapshot.clone()));
                // keep ticking a little so the HUD looks alive
                for i in 0..6u64 {
                    std::thread::sleep(Duration::from_millis(500));
                    let mut s = snapshot.clone();
                    s.cache_duration_secs = (s.cache_duration_secs + (i as f64) * 0.1).min(60.0);
                    s.bitrate_kbps = 4000 + ((started_at.elapsed().as_millis() as u32 + i as u32 * 97) % 600);
                    l(EngineEvent::Telemetry(s));
                }
            }
            let _ = gen;
        });
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        tracing::info!("[stub] stop");
        *self.telemetry.lock().unwrap() =
            EngineTelemetryEvent { active_profile: self.profile.lock().unwrap().as_str().into(), ..Default::default() };
        Ok(())
    }

    fn set_profile(&self, profile: ProfileMode) -> Result<()> {
        *self.profile.lock().unwrap() = profile;
        self.telemetry.lock().unwrap().active_profile = profile.as_str().into();
        tracing::info!(profile = profile.as_str(), "[stub] set_profile");
        Ok(())
    }

    fn active_profile(&self) -> ProfileMode {
        *self.profile.lock().unwrap()
    }

    fn set_pause(&self, pause: bool) -> Result<()> {
        self.props.lock().unwrap().insert("pause".into(), if pause { "yes" } else { "no" }.into());
        Ok(())
    }

    fn set_mute(&self, mute: bool) -> Result<()> {
        self.props.lock().unwrap().insert("mute".into(), if mute { "yes" } else { "no" }.into());
        Ok(())
    }

    fn set_volume(&self, percent: u16) -> Result<()> {
        self.props.lock().unwrap().insert("volume".into(), percent.min(130).to_string());
        Ok(())
    }

    fn set_audio_delay_ms(&self, ms: i32) -> Result<()> {
        self.props.lock().unwrap().insert("audio-delay".into(), format!("{:.3}", ms as f64 / 1000.0));
        Ok(())
    }

    fn set_video_margins(&self, left: f32, right: f32, top: f32, bottom: f32) -> Result<()> {
        tracing::debug!(left, right, top, bottom, "[stub] video margins");
        Ok(())
    }

    fn set_property(&self, name: &str, value: &str) -> Result<()> {
        self.props.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }

    fn get_property(&self, name: &str) -> Result<Option<String>> {
        Ok(self.props.lock().unwrap().get(name).cloned())
    }

    fn command(&self, args: &[&str]) -> Result<()> {
        tracing::info!(?args, "[stub] command");
        Ok(())
    }

    fn telemetry(&self) -> EngineTelemetryEvent {
        self.telemetry.lock().unwrap().clone()
    }

    fn set_listener(&self, listener: EventListener) {
        *self.listener.lock().unwrap() = Some(listener);
    }

    fn shutdown(&self) {
        if !self.shut.swap(true, Ordering::SeqCst) {
            self.emit(EngineEvent::Shutdown);
        }
    }
}
