//! Exercises the real libmpv FFI path headlessly (`vo=null`, `ao=null`).
//! Skips (passes) when libmpv or ffmpeg is not installed, so CI without media libs stays green.

use app_core::ProfileMode;
use app_engine::{create_engine, EngineEvent, EngineOptions};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_ts(path: &std::path::Path) -> bool {
    // 12 s of MPEG-TS / H.264 test pattern — the same container as a typical IPTV live stream.
    let st = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=12:size=320x240:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=12",
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

    let (tx, rx) = mpsc::channel::<EngineEvent>();
    engine.set_listener(Arc::new(move |ev| {
        let _ = tx.send(ev);
    }));

    let t0 = Instant::now();
    engine.load(ts.to_str().unwrap(), ProfileMode::LowLatency, 100).unwrap();

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
    engine.load(ts.to_str().unwrap(), ProfileMode::Stable, 120).unwrap();
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
    assert_eq!(engine.get_property("volume").unwrap().unwrap().parse::<f64>().unwrap().round() as i64, 120);
    engine.shutdown();
}

struct ProfineModeCompat;
impl ProfineModeCompat {
    fn stable() -> ProfileMode {
        ProfileMode::Stable
    }
}
