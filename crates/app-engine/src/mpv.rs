//! The real engine: one libmpv instance bound to a native surface.

use crate::ffi::{self, cstr, LibMpv};
use crate::profiles::{hwdec_for_platform, profile_options, startup_options};
use crate::{EngineError, EngineEvent, EngineOptions, EventListener, PlayerEngine, Result};
use app_core::{EngineTelemetryEvent, ProfileMode};
use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// Observation ids (reply_userdata) for PROPERTY_CHANGE events.
const OBS_W: u64 = 1;
const OBS_H: u64 = 2;
const OBS_VCODEC: u64 = 3;
const OBS_FPS: u64 = 4;
const OBS_VBITRATE: u64 = 5;
const OBS_CACHE_DUR: u64 = 6;
const OBS_PAUSED_FOR_CACHE: u64 = 7;
const OBS_DROPPED: u64 = 8;
const OBS_IDLE: u64 = 9;
const OBS_BUFFERING_PCT: u64 = 10;
const OBS_TIME_POS: u64 = 11;
const OBS_DURATION: u64 = 12;
const OBS_PAUSE: u64 = 13;

const OBSERVED: &[(u64, &str, c_int)] = &[
    (OBS_W, "video-params/w", ffi::MPV_FORMAT_INT64),
    (OBS_H, "video-params/h", ffi::MPV_FORMAT_INT64),
    (OBS_VCODEC, "video-codec", ffi::MPV_FORMAT_STRING),
    (OBS_FPS, "estimated-vf-fps", ffi::MPV_FORMAT_DOUBLE),
    (OBS_VBITRATE, "video-bitrate", ffi::MPV_FORMAT_DOUBLE),
    (OBS_CACHE_DUR, "demuxer-cache-duration", ffi::MPV_FORMAT_DOUBLE),
    (OBS_PAUSED_FOR_CACHE, "paused-for-cache", ffi::MPV_FORMAT_FLAG),
    (OBS_DROPPED, "frame-drop-count", ffi::MPV_FORMAT_INT64),
    (OBS_IDLE, "core-idle", ffi::MPV_FORMAT_FLAG),
    (OBS_BUFFERING_PCT, "cache-buffering-state", ffi::MPV_FORMAT_INT64),
    (OBS_TIME_POS, "time-pos", ffi::MPV_FORMAT_DOUBLE),
    (OBS_DURATION, "duration", ffi::MPV_FORMAT_DOUBLE),
    (OBS_PAUSE, "pause", ffi::MPV_FORMAT_FLAG),
];

/// Raw handle wrapper. libmpv's client API is thread-safe for commands/properties;
/// `mpv_wait_event` is only ever called from the event thread.
#[derive(Clone, Copy)]
struct Handle(*mut ffi::mpv_handle);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

/// A `load()` in flight: set by `load()`, resolved by PLAYBACK_RESTART (→ zap_ms) or by the
/// END_FILE of the entry that START_FILE assigned to it (→ load failed).
struct LoadState {
    t0: Instant,
    url: String,
    entry: Option<i64>,
}

struct Shared {
    lib: Arc<LibMpv>,
    handle: Handle,
    telemetry: Mutex<EngineTelemetryEvent>,
    listener: Mutex<Option<EventListener>>,
    profile: Mutex<ProfileMode>,
    load_started: Mutex<Option<LoadState>>,
    current_url_redacted: Mutex<String>,
    shut: AtomicBool,
    stable_cache_secs: u16,
}

impl Shared {
    fn emit(&self, ev: EngineEvent) {
        let l = self.listener.lock().unwrap().clone();
        if let Some(l) = l {
            l(ev);
        }
    }

    fn check(&self, code: c_int, what: &str) -> Result<()> {
        if code == ffi::MPV_ERROR_SUCCESS {
            Ok(())
        } else {
            Err(EngineError::Mpv(format!("{what}: {}", self.lib.err_str(code))))
        }
    }

    fn ensure_alive(&self) -> Result<()> {
        if self.shut.load(Ordering::SeqCst) {
            Err(EngineError::ShutDown)
        } else {
            Ok(())
        }
    }

    fn set_prop(&self, name: &str, value: &str) -> Result<()> {
        self.ensure_alive()?;
        let n = cstr(name);
        let v = cstr(value);
        // SAFETY: valid handle + NUL-terminated strings.
        let rc = unsafe { (self.lib.set_property_string)(self.handle.0, n.as_ptr(), v.as_ptr()) };
        self.check(rc, &format!("set {name}"))
    }

    fn get_prop(&self, name: &str) -> Result<Option<String>> {
        self.ensure_alive()?;
        let n = cstr(name);
        // SAFETY: valid handle; result freed with mpv_free.
        unsafe {
            let p = (self.lib.get_property_string)(self.handle.0, n.as_ptr());
            if p.is_null() {
                return Ok(None);
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.lib.free)(p as *mut c_void);
            Ok(Some(s))
        }
    }

    fn cmd(&self, args: &[&str]) -> Result<()> {
        self.ensure_alive()?;
        let owned: Vec<_> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        // SAFETY: NULL-terminated array of valid C strings.
        let rc = unsafe { (self.lib.command)(self.handle.0, ptrs.as_ptr()) };
        let shown: Vec<String> = args.iter().map(|a| app_core::redact::redact(a)).collect();
        self.check(rc, &format!("command {shown:?}"))
    }
}

pub struct MpvEngine {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    description: String,
}

impl MpvEngine {
    pub fn new(opts: EngineOptions) -> Result<Self> {
        let lib =
            Arc::new(LibMpv::load(opts.libmpv_path.as_deref()).map_err(|e| EngineError::Unavailable(e.to_string()))?);
        let (major, minor) = lib.api_version();
        if major < 2 {
            return Err(EngineError::Unavailable(format!("libmpv client API {major}.{minor} too old (need ≥ 2.0)")));
        }

        #[cfg(unix)]
        unsafe {
            // libmpv refuses to initialise unless LC_NUMERIC is "C".
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }

        // SAFETY: plain create.
        let handle = unsafe { (lib.create)() };
        if handle.is_null() {
            return Err(EngineError::Mpv("mpv_create returned NULL".into()));
        }
        let handle = Handle(handle);

        let hwdec = hwdec_for_platform(&opts.hw_decoding);
        let mut startup = startup_options(opts.wid, &hwdec, &opts.log_level);
        startup.extend(profile_options(opts.default_profile, opts.stable_cache_secs));
        if let Some(ua) = &opts.user_agent {
            startup.push(("user-agent".into(), ua.clone()));
        }
        startup.push(("volume".into(), opts.audio_boost.clamp(0, 130).to_string()));
        startup.push(("audio-delay".into(), format!("{:.3}", opts.audio_delay_ms as f64 / 1000.0)));

        for (k, v) in &startup {
            let kk = cstr(k);
            let vv = cstr(v);
            // SAFETY: valid handle + strings; before initialize.
            let rc = unsafe { (lib.set_option_string)(handle.0, kk.as_ptr(), vv.as_ptr()) };
            if rc != ffi::MPV_ERROR_SUCCESS {
                // Non-fatal: an unknown option on an older libmpv should not kill playback.
                tracing::warn!(option = %k, value = %v, error = %lib.err_str(rc), "mpv option rejected");
            }
        }

        // SAFETY: valid handle.
        let rc = unsafe { (lib.initialize)(handle.0) };
        if rc != ffi::MPV_ERROR_SUCCESS {
            let msg = lib.err_str(rc);
            // SAFETY: handle not initialised → destroy is still required.
            unsafe { (lib.terminate_destroy)(handle.0) };
            return Err(EngineError::Mpv(format!("mpv_initialize: {msg}")));
        }

        let shared = Arc::new(Shared {
            lib: lib.clone(),
            handle,
            telemetry: Mutex::new(EngineTelemetryEvent {
                active_profile: opts.default_profile.as_str().into(),
                ..Default::default()
            }),
            listener: Mutex::new(None),
            profile: Mutex::new(opts.default_profile),
            load_started: Mutex::new(None),
            current_url_redacted: Mutex::new(String::new()),
            shut: AtomicBool::new(false),
            stable_cache_secs: opts.stable_cache_secs,
        });

        for (id, name, fmt) in OBSERVED {
            let n = cstr(name);
            // SAFETY: valid handle.
            let rc = unsafe { (lib.observe_property)(handle.0, *id, n.as_ptr(), *fmt) };
            if rc != ffi::MPV_ERROR_SUCCESS {
                tracing::warn!(property = name, error = %lib.err_str(rc), "observe failed");
            }
        }
        {
            let lvl = cstr("warn");
            // SAFETY: valid handle.
            unsafe { (lib.request_log_messages)(handle.0, lvl.as_ptr()) };
        }

        let version = shared.get_prop("mpv-version").ok().flatten().unwrap_or_else(|| "mpv".into());
        let description = format!(
            "{version} · client API {major}.{minor} · {} · hwdec={hwdec} · wid={}",
            lib.path.display(),
            opts.wid.map(|w| w.to_string()).unwrap_or_else(|| "none (headless)".into())
        );

        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || event_loop(thread_shared))
            .map_err(|e| EngineError::Other(e.to_string()))?;

        Ok(Self { shared, thread: Mutex::new(Some(thread)), description })
    }
}

impl Drop for MpvEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl PlayerEngine for MpvEngine {
    fn kind(&self) -> &'static str {
        "mpv"
    }

    fn describe(&self) -> String {
        self.description.clone()
    }

    fn load(&self, url: &str, profile: ProfileMode, audio_boost: u16, start_secs: Option<f64>) -> Result<()> {
        self.shared.ensure_alive()?;
        if self.active_profile() != profile {
            self.set_profile(profile)?;
        }
        let redacted = app_core::redact::redact(url);
        *self.shared.load_started.lock().unwrap() =
            Some(LoadState { t0: Instant::now(), url: redacted.clone(), entry: None });
        *self.shared.current_url_redacted.lock().unwrap() = redacted.clone();
        {
            let mut t = self.shared.telemetry.lock().unwrap();
            *t = EngineTelemetryEvent { active_profile: profile.as_str().into(), ..Default::default() };
        }
        self.shared.set_prop("volume", &audio_boost.clamp(0, 130).to_string())?;
        self.shared.set_prop("pause", "no")?;
        // `start` applies to the next loaded file; "none" restores the default.
        let start = start_secs.filter(|s| *s > 1.0).map(|s| format!("{s:.1}")).unwrap_or_else(|| "none".into());
        let _ = self.shared.set_prop("start", &start);
        tracing::info!(url = %redacted, profile = profile.as_str(), "loadfile replace");
        self.shared.cmd(&["loadfile", url, "replace"])
    }

    fn seek(&self, secs: f64) -> Result<()> {
        self.shared.cmd(&["seek", &format!("{:.2}", secs.max(0.0)), "absolute"])
    }

    fn stop(&self) -> Result<()> {
        *self.shared.load_started.lock().unwrap() = None;
        self.shared.cmd(&["stop"])
    }

    fn set_profile(&self, profile: ProfileMode) -> Result<()> {
        self.shared.ensure_alive()?;
        let mut failures = Vec::new();
        for (k, v) in profile_options(profile, self.shared.stable_cache_secs) {
            if let Err(e) = self.shared.set_prop(&k, &v) {
                // List options (stream-lavf-o-add) and a few others are startup-only; the
                // reload the caller does afterwards picks them up via the option defaults.
                failures.push(format!("{k}={v}: {e}"));
            }
        }
        if !failures.is_empty() {
            tracing::debug!(?failures, "profile keys not live-switchable");
        }
        *self.shared.profile.lock().unwrap() = profile;
        self.shared.telemetry.lock().unwrap().active_profile = profile.as_str().into();
        Ok(())
    }

    fn active_profile(&self) -> ProfileMode {
        *self.shared.profile.lock().unwrap()
    }

    fn set_pause(&self, pause: bool) -> Result<()> {
        self.shared.set_prop("pause", if pause { "yes" } else { "no" })
    }

    fn set_mute(&self, mute: bool) -> Result<()> {
        self.shared.set_prop("mute", if mute { "yes" } else { "no" })
    }

    fn set_volume(&self, percent: u16) -> Result<()> {
        self.shared.set_prop("volume", &percent.min(130).to_string())
    }

    fn set_audio_delay_ms(&self, ms: i32) -> Result<()> {
        self.shared.set_prop("audio-delay", &format!("{:.3}", ms as f64 / 1000.0))
    }

    fn set_video_margins(&self, left: f32, right: f32, top: f32, bottom: f32) -> Result<()> {
        let c = |v: f32| format!("{:.4}", v.clamp(0.0, 0.95));
        self.shared.set_prop("video-margin-ratio-left", &c(left))?;
        self.shared.set_prop("video-margin-ratio-right", &c(right))?;
        self.shared.set_prop("video-margin-ratio-top", &c(top))?;
        self.shared.set_prop("video-margin-ratio-bottom", &c(bottom))
    }

    fn set_property(&self, name: &str, value: &str) -> Result<()> {
        self.shared.set_prop(name, value)
    }

    fn get_property(&self, name: &str) -> Result<Option<String>> {
        self.shared.get_prop(name).map(|o| o.map(|s| app_core::redact::redact(&s)))
    }

    fn command(&self, args: &[&str]) -> Result<()> {
        self.shared.cmd(args)
    }

    fn telemetry(&self) -> EngineTelemetryEvent {
        self.shared.telemetry.lock().unwrap().clone()
    }

    fn set_listener(&self, listener: EventListener) {
        *self.shared.listener.lock().unwrap() = Some(listener);
    }

    fn shutdown(&self) {
        if self.shared.shut.swap(true, Ordering::SeqCst) {
            return;
        }
        // Ask mpv to quit; the event thread sees SHUTDOWN, destroys the handle and exits.
        let q = cstr("quit");
        let args = [q.as_ptr(), std::ptr::null()];
        // SAFETY: handle still valid until the event thread destroys it.
        unsafe {
            let _ = (self.shared.lib.command)(self.shared.handle.0, args.as_ptr());
            (self.shared.lib.wakeup)(self.shared.handle.0);
        }
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
        self.shared.emit(EngineEvent::Shutdown);
    }
}

// ---- event thread -------------------------------------------------------------------

fn event_loop(s: Arc<Shared>) {
    let lib = s.lib.clone();
    let h = s.handle;
    let mut dirty = false;
    let mut last_emit = Instant::now();
    let mut playing = false;
    let mut last_log = String::new();
    // Entry id of the file currently being played, from START_FILE. END_FILE events for an
    // *older* entry (the stream we just replaced) must not clear zap state or reach the UI.
    let mut current_entry: i64 = -1;

    loop {
        // SAFETY: only this thread calls wait_event; the returned event is valid until the next call.
        let ev = unsafe { (lib.wait_event)(h.0, 0.25) };
        if ev.is_null() {
            continue;
        }
        // SAFETY: non-null event pointer from libmpv.
        let ev = unsafe { &*ev };
        match ev.event_id {
            ffi::MPV_EVENT_NONE => {}
            ffi::MPV_EVENT_SHUTDOWN => break,
            ffi::MPV_EVENT_PROPERTY_CHANGE => {
                // SAFETY: data is mpv_event_property for this event id.
                let prop = unsafe { &*(ev.data as *const ffi::mpv_event_property) };
                let mut t = s.telemetry.lock().unwrap();
                match ev.reply_userdata {
                    OBS_W => t.width = prop_i64(prop).unwrap_or(0).max(0) as u32,
                    OBS_H => t.height = prop_i64(prop).unwrap_or(0).max(0) as u32,
                    OBS_VCODEC => t.codec_name = prop_str(prop).unwrap_or_default(),
                    OBS_FPS => t.fps = prop_f64(prop).unwrap_or(0.0) as f32,
                    OBS_VBITRATE => t.bitrate_kbps = (prop_f64(prop).unwrap_or(0.0) / 1000.0).round().max(0.0) as u32,
                    OBS_CACHE_DUR => t.cache_duration_secs = prop_f64(prop).unwrap_or(0.0),
                    OBS_DROPPED => t.dropped_frames = prop_i64(prop).unwrap_or(0).max(0) as u64,
                    OBS_PAUSED_FOR_CACHE => {
                        let v = prop_flag(prop).unwrap_or(false);
                        if v != t.is_underrun {
                            t.is_underrun = v;
                            drop(t);
                            s.emit(EngineEvent::Buffering { active: v });
                            t = s.telemetry.lock().unwrap();
                        }
                    }
                    OBS_IDLE => playing = !prop_flag(prop).unwrap_or(true),
                    OBS_TIME_POS => t.time_pos_s = prop_f64(prop).unwrap_or(0.0).max(0.0),
                    OBS_DURATION => t.duration_s = prop_f64(prop).unwrap_or(0.0).max(0.0),
                    OBS_PAUSE => t.paused = prop_flag(prop).unwrap_or(false),
                    OBS_BUFFERING_PCT => {}
                    _ => {}
                }
                drop(t);
                dirty = true;
            }
            ffi::MPV_EVENT_PLAYBACK_RESTART => {
                let started = s.load_started.lock().unwrap().take();
                if let Some(LoadState { t0, url, .. }) = started {
                    let zap = t0.elapsed().as_millis() as u64;
                    s.telemetry.lock().unwrap().zap_ms = Some(zap);
                    tracing::info!(zap_ms = zap, url = %url, "playback started");
                    s.emit(EngineEvent::PlaybackStarted { url_redacted: url, zap_ms: zap });
                    dirty = true;
                }
            }
            ffi::MPV_EVENT_START_FILE => {
                // SAFETY: data is mpv_event_start_file for this event id.
                let sf = unsafe { &*(ev.data as *const ffi::mpv_event_start_file) };
                current_entry = sf.playlist_entry_id;
                if let Some(ls) = s.load_started.lock().unwrap().as_mut() {
                    if ls.entry.is_none() {
                        ls.entry = Some(sf.playlist_entry_id);
                    }
                }
            }
            ffi::MPV_EVENT_END_FILE => {
                // SAFETY: data is mpv_event_end_file for this event id.
                let ef = unsafe { &*(ev.data as *const ffi::mpv_event_end_file) };
                {
                    let mut pending = s.load_started.lock().unwrap();
                    match pending.as_ref() {
                        // A load is in flight and this END_FILE is for a *different* entry:
                        // it is the stream we just replaced. Nothing to report.
                        Some(ls) if ls.entry != Some(ef.playlist_entry_id) => continue,
                        // The in-flight load itself ended before its first frame → failed load.
                        Some(_) => {
                            *pending = None;
                        }
                        None => {
                            if ef.playlist_entry_id != current_entry {
                                continue;
                            }
                        }
                    }
                }
                let reason = match ef.reason {
                    ffi::MPV_END_FILE_REASON_EOF => "eof",
                    ffi::MPV_END_FILE_REASON_STOP => "stop",
                    ffi::MPV_END_FILE_REASON_QUIT => "quit",
                    ffi::MPV_END_FILE_REASON_ERROR => "error",
                    ffi::MPV_END_FILE_REASON_REDIRECT => "redirect",
                    _ => "unknown",
                };
                let error = (ef.reason == ffi::MPV_END_FILE_REASON_ERROR).then(|| lib.err_str(ef.error));
                let url = s.current_url_redacted.lock().unwrap().clone();
                if reason != "redirect" {
                    s.telemetry.lock().unwrap().zap_ms = None;
                    tracing::info!(reason, ?error, url = %url, "end-file");
                    s.emit(EngineEvent::EndFile { reason: reason.into(), error, url_redacted: url });
                }
            }
            ffi::MPV_EVENT_LOG_MESSAGE => {
                // SAFETY: data is mpv_event_log_message for this event id.
                let lm = unsafe { &*(ev.data as *const ffi::mpv_event_log_message) };
                let level = cstr_to_string(lm.level);
                let prefix = cstr_to_string(lm.prefix);
                let text = app_core::redact::redact(cstr_to_string(lm.text).trim_end());
                if text.is_empty() || text == last_log || is_probe_noise(&text) {
                    continue;
                }
                last_log = text.clone();
                let line = format!("[{prefix}] {text}");
                match level.as_str() {
                    "error" | "fatal" => tracing::error!(target: "mpv", "{line}"),
                    _ => tracing::warn!(target: "mpv", "{line}"),
                }
                s.emit(EngineEvent::Log { level, text: line });
            }
            _ => {}
        }

        if dirty && (last_emit.elapsed() >= Duration::from_millis(500) || !playing) {
            let snap = s.telemetry.lock().unwrap().clone();
            s.emit(EngineEvent::Telemetry(snap));
            dirty = false;
            last_emit = Instant::now();
        }
    }

    // SAFETY: SHUTDOWN received → nobody else may use the handle; destroy it here.
    unsafe { (lib.terminate_destroy)(h.0) };
    s.shut.store(true, Ordering::SeqCst);
    tracing::info!("mpv event thread exited");
}

/// hwdec=auto-safe probes every backend and ffmpeg logs each miss as an *error*.
/// Those are expected and would only scare users in the diagnostics panel.
fn is_probe_noise(text: &str) -> bool {
    text.contains("AVHWDeviceContext")
        || text.contains("Could not dynamically load")
        || text.contains("Cannot load lib")
        || text.contains("Failed to initialize hwdec")
}

fn prop_i64(p: &ffi::mpv_event_property) -> Option<i64> {
    if p.format == ffi::MPV_FORMAT_INT64 && !p.data.is_null() {
        // SAFETY: format says int64.
        Some(unsafe { *(p.data as *const i64) })
    } else if p.format == ffi::MPV_FORMAT_DOUBLE && !p.data.is_null() {
        // SAFETY: format says double.
        Some(unsafe { *(p.data as *const f64) } as i64)
    } else {
        None
    }
}

fn prop_f64(p: &ffi::mpv_event_property) -> Option<f64> {
    if p.format == ffi::MPV_FORMAT_DOUBLE && !p.data.is_null() {
        // SAFETY: format says double.
        Some(unsafe { *(p.data as *const f64) })
    } else if p.format == ffi::MPV_FORMAT_INT64 && !p.data.is_null() {
        // SAFETY: format says int64.
        Some(unsafe { *(p.data as *const i64) } as f64)
    } else {
        None
    }
}

fn prop_flag(p: &ffi::mpv_event_property) -> Option<bool> {
    if p.format == ffi::MPV_FORMAT_FLAG && !p.data.is_null() {
        // SAFETY: format says flag (int).
        Some(unsafe { *(p.data as *const c_int) } != 0)
    } else {
        None
    }
}

fn prop_str(p: &ffi::mpv_event_property) -> Option<String> {
    if p.format == ffi::MPV_FORMAT_STRING && !p.data.is_null() {
        // SAFETY: format says string → data is char**.
        let ptr = unsafe { *(p.data as *const *const c_char) };
        if ptr.is_null() {
            return None;
        }
        Some(cstr_to_string(ptr))
    } else {
        None
    }
}

fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: libmpv guarantees NUL-terminated strings for event payloads.
    unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
}
