//! Exercises the real libmpv FFI path headlessly (`vo=null`, `ao=null`).
//! Skips (passes) when libmpv or ffmpeg is not installed, so CI without media libs stays green.

use app_core::ProfileMode;
use app_engine::{create_engine, EngineEvent, EngineOptions};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_ts(path: &std::path::Path) -> bool {
    // 90 s of MPEG-TS / H.264 test pattern — the same container as a typical IPTV live stream.
    let st = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=90:size=320x240:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=90",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-f",
            "mpegts",
        ])
        .arg(path)
        .status();
    matches!(st, Ok(s) if s.success())
}

#[test]
fn plays_a_mpegts_file_and_reports_zap_and_telemetry() {
    let dir = std::env::temp_dir().join("desktop-iptv-engine-test");
    std::fs::create_dir_all(&dir).unwrap();
    let ts = dir.join("pattern.ts");
    if !make_ts(&ts) {
        eprintln!("ffmpeg unavailable — skipping engine test");
        return;
    }

    let engine = create_engine(EngineOptions {
        default_profile: ProfileMode::LowLatency,
        log_level: "warn".into(),
        ..Default::default()
    });
    if engine.kind() != "mpv" {
        eprintln!("libmpv unavailable ({}) — skipping engine test", engine.describe());
        return;
    }
    eprintln!("engine: {}", engine.describe());
    // Option plumbing: the escaped list value must survive mpv's key/value parser intact.
    let lavf = engine.get_property("stream-lavf-o").unwrap().unwrap_or_default();
    eprintln!("stream-lavf-o = {lavf}");
    assert!(lavf.contains("reconnect_on_http_error=4xx,5xx"), "lavf list mangled: {lavf}");
    assert!(lavf.contains("reconnect_at_eof=1"), "{lavf}");
    assert_eq!(engine.get_property("sid").unwrap().as_deref(), Some("no"));

    let (tx, rx) = mpsc::channel::<EngineEvent>();
    engine.set_listener(Arc::new(move |ev| {
        let _ = tx.send(ev);
    }));

    let t0 = Instant::now();
    engine.load(ts.to_str().unwrap(), ProfileMode::LowLatency, 100, None).unwrap();

    let mut started: Option<u64> = None;
    let mut ended: Option<String> = None;
    let mut snapshot: Option<app_core::EngineTelemetryEvent> = None;
    while t0.elapsed() < Duration::from_secs(20) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(EngineEvent::PlaybackStarted { zap_ms, .. }) => started = Some(zap_ms),
            Ok(EngineEvent::Telemetry(t)) => {
                if t.width == 320 && t.height == 240 && !t.codec_name.is_empty() {
                    snapshot = Some(t);
                }
            }
            Ok(EngineEvent::EndFile { reason, error, .. }) => {
                ended = Some(format!("{reason} {error:?}"));
                break;
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        if started.is_some() && snapshot.is_some() {
            break;
        }
    }
    eprintln!("zap_ms={started:?} ended={ended:?} telemetry={snapshot:?}");
    assert!(started.is_some(), "PLAYBACK_RESTART never arrived (ended={ended:?})");
    assert!(started.unwrap() < 5_000, "zap took {}ms", started.unwrap());
    let tele = snapshot.expect("no 320x240 telemetry with a codec name");
    assert!(tele.codec_name.to_lowercase().contains("264"), "codec: {}", tele.codec_name);
    assert_eq!(tele.active_profile, "low_latency");

    // Profile switch + a second load on the same instance (zap path).
    engine.set_profile(ProfineModeCompat::stable()).unwrap();
    engine.load(ts.to_str().unwrap(), ProfileMode::Stable, 120, Some(4.0)).unwrap();
    let t1 = Instant::now();
    let mut second = None;
    while t1.elapsed() < Duration::from_secs(10) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(EngineEvent::PlaybackStarted { zap_ms, .. }) => {
                second = Some(zap_ms);
                break;
            }
            Ok(EngineEvent::Telemetry(_)) => {}
            Ok(other) => eprintln!("phase2 event: {other:?}"),
            Err(_) => {}
        }
    }
    assert!(second.is_some(), "second loadfile replace did not restart playback");

    // Single-connection record tap: stream-record writes the raw stream while playing. Use the
    // low-latency profile so the demuxer has not already cached the whole file.
    engine.load(ts.to_str().unwrap(), ProfileMode::LowLatency, 120, None).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let rec = dir.join("tap.ts");
    let _ = std::fs::remove_file(&rec);
    engine.set_record(rec.to_str()).unwrap();
    assert_eq!(engine.record_path().as_deref(), rec.to_str());
    std::thread::sleep(Duration::from_millis(1500));
    engine.set_record(None).unwrap();
    assert!(engine.record_path().is_none());
    // The demuxer thread closes the muxer asynchronously; give it a moment before measuring.
    std::thread::sleep(Duration::from_millis(700));
    let size = std::fs::metadata(&rec).map(|m| m.len()).unwrap_or(0);
    eprintln!("stream-record wrote {size} bytes");
    assert!(size > 10_000, "stream-record produced only {size} bytes");
    assert_eq!(engine.get_property("volume").unwrap().unwrap().parse::<f64>().unwrap().round() as i64, 120);
    engine.shutdown();
}

struct ProfineModeCompat;
impl ProfineModeCompat {
    fn stable() -> ProfileMode {
        ProfileMode::Stable
    }
}

#[test]
fn probe_reports_codecs_and_errors() {
    let dir = std::env::temp_dir().join("desktop-iptv-engine-test");
    std::fs::create_dir_all(&dir).unwrap();
    let ts = dir.join("probe.ts");
    if !make_ts(&ts) {
        eprintln!("ffmpeg unavailable — skipping probe test");
        return;
    }
    if create_engine(EngineOptions::default()).kind() != "mpv" {
        eprintln!("libmpv unavailable — skipping probe test");
        return;
    }
    let url = format!("file://{}", ts.display());
    let r = app_engine::probe::probe_stream(EngineOptions::default(), &url, Duration::from_secs(20));
    eprintln!("probe: {}", r.summary());
    assert!(r.ok, "probe failed: {:?} log={:?}", r.error, r.log);
    assert!(r.ttff_ms.is_some());
    assert_eq!(r.container.as_deref(), Some("mpegts"));
    assert!(r.video_codec.as_deref().unwrap_or("").to_lowercase().contains("h264"), "{:?}", r.video_codec);
    assert_eq!((r.width, r.height), (Some(320), Some(240)));

    // A URL that cannot exist: the error must be reported, redacted, within the timeout.
    let bad = "http://127.0.0.1:9/live/user/secret/1.ts";
    let r = app_engine::probe::probe_stream(EngineOptions::default(), bad, Duration::from_secs(20));
    eprintln!("probe: {}", r.summary());
    assert!(!r.ok);
    assert!(r.error.is_some());
    assert!(!r.url.contains("secret"));
    assert!(!format!("{:?}", r).contains("secret"), "redaction leak: {:?}", r);
}
