//! The two day-one playback profiles (CLAUDE.md §6.2) and the startup option set.
//!
//! Everything here is injected as mpv options from Rust. There is no user-visible mpv.conf;
//! the Advanced panel lists the same keys (`profile_options` is what it shows).

use app_core::ProfileMode;

/// VO fallback chain. `gpu-next` → `gpu` (both d3d11 on Windows); Linux also gets the software
/// `x11` VO so VMs / remote desktops without GL still show video.
#[cfg(all(unix, not(target_os = "macos")))]
pub const VO_CHAIN: &str = "gpu-next,gpu,x11";
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub const VO_CHAIN: &str = "gpu-next,gpu";

/// Startup-only options (set before `mpv_initialize`). Embedding, no OSC, no config files.
pub fn startup_options(wid: Option<i64>, hwdec: &str, log_level: &str) -> Vec<(String, String)> {
    let mut o: Vec<(String, String)> = vec![
        ("config", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("terminal", "no"),
        ("osc", "no"),
        ("osd-level", "0"),
        ("osd-bar", "no"),
        ("input-default-bindings", "no"),
        ("input-vo-keyboard", "no"),
        ("input-cursor", "no"),
        ("cursor-autohide", "no"),
        ("idle", "yes"),
        ("keep-open", "no"),
        // Fallback chain: gpu-next → gpu. Both use d3d11 on Windows / Metal-via-MoltenVK or OpenGL on
        // macOS; `gpu` rescues machines where gpu-next's context setup fails.
        ("vo", VO_CHAIN),
        ("hwdec", hwdec),
        ("volume-max", "130"),
        // Live TV streams carry EIA-608/708 captions that render as garbage over the picture
        // (and providers' teletext). Subtitles stay off unless the user turns them on.
        ("sid", "no"),
        ("secondary-sid", "no"),
        ("sub-auto", "no"),
        ("audio-client-name", app_core::PRODUCT_NAME),
        ("title", app_core::PRODUCT_NAME),
        ("msg-level", &format!("all={log_level}")),
        // Live TV: do not seek on load, do not loop, never pause when window unfocused.
        ("loop-file", "no"),
        ("pause", "no"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();

    if let Some(w) = wid {
        o.push(("wid".into(), w.to_string()));
        // Create the (black) video child window immediately so the transparent webview never
        // reveals the desktop behind the app before the first stream starts.
        o.push(("force-window".into(), "yes".into()));
    } else {
        // Headless (tests, probes): decode but draw nothing.
        o.push(("force-window".into(), "no".into()));
        o.push(("vo".into(), "null".into()));
        o.push(("ao".into(), "null".into()));
    }
    o
}

/// Profile matrix. `stable_cache_secs` is the user's 20–60 s setting for the stable profile.
pub fn profile_options(profile: ProfileMode, stable_cache_secs: u16) -> Vec<(String, String)> {
    let common = [
        ("cache", "yes".to_string()),
        ("hwdec-codecs", "all".to_string()),
        ("deband", "no".to_string()),
        ("interpolation", "no".to_string()),
        ("scale", "bilinear".to_string()),
        ("untimed", "no".to_string()),
    ];
    // `stream-lavf-o` is a key=value list option; the `-add` suffix only exists for the CLI/conf
    // parser, so the client API gets the full list in one go.
    // Same recovery set the established players ship: reconnect on network errors, on 4xx/5xx
    // (panels answer 5xx while a stream restarts), at EOF (live streams "end" when a node is
    // rotated), and never reuse a persistent HTTP connection across those retries. The value
    // "4xx,5xx" contains the list separator, hence mpv's `%len%` escape.
    let lavf = |delay_max: u32| {
        format!(
            "reconnect=1,reconnect_streamed=1,reconnect_on_network_error=1,reconnect_on_http_error=%7%4xx,5xx,reconnect_at_eof=1,reconnect_delay_max={delay_max},http_persistent=0,http_multiple=0"
        )
    };
    let specific: Vec<(&str, String)> = match profile {
        ProfileMode::LowLatency => vec![
            ("cache-secs", "3".into()),
            ("demuxer-readahead-secs", "3".into()),
            ("demuxer-max-bytes", "32MiB".into()),
            ("demuxer-max-back-bytes", "8MiB".into()),
            ("video-latency-hacks", "yes".into()),
            ("network-timeout", "8".into()),
            ("stream-lavf-o", lavf(5)),
        ],
        ProfileMode::Stable => {
            let secs = stable_cache_secs.clamp(20, 60);
            vec![
                ("cache-secs", secs.to_string()),
                ("demuxer-readahead-secs", secs.to_string()),
                ("demuxer-max-bytes", "400MiB".into()),
                ("demuxer-max-back-bytes", "50MiB".into()),
                ("video-latency-hacks", "no".into()),
                ("network-timeout", "15".into()),
                ("stream-lavf-o", lavf(10)),
            ]
        }
    };
    common.into_iter().chain(specific).map(|(k, v)| (k.to_string(), v)).collect()
}

/// Options that take effect on the *running* stream when changed via `set_property`.
/// Everything else in `profile_options` needs a reload (`loadfile … replace`).
pub const LIVE_SWITCHABLE: &[&str] = &[
    "cache-secs",
    "demuxer-readahead-secs",
    "demuxer-max-bytes",
    "demuxer-max-back-bytes",
    "deband",
    "interpolation",
    "scale",
    "network-timeout",
];

/// Map the user-facing `hw_decoding` setting to what this OS actually accepts.
pub fn hwdec_for_platform(setting: &str) -> String {
    let s = setting.trim();
    #[cfg(target_os = "windows")]
    {
        match s {
            "videotoolbox" | "videotoolbox-copy" | "vaapi" | "vaapi-copy" => "auto-safe".into(),
            other => other.into(),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match s {
            "d3d11va" | "d3d11va-copy" | "vaapi" | "vaapi-copy" => "auto-safe".into(),
            other => other.into(),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match s {
            "d3d11va" | "d3d11va-copy" | "videotoolbox" | "videotoolbox-copy" => "auto-safe".into(),
            other => other.into(),
        }
    }
}

/// Copy-back fallback chain (CLAUDE.md §6.1 step 5). Returns the next hwdec to try.
pub fn hwdec_fallback(current: &str) -> Option<&'static str> {
    match current {
        "d3d11va" => Some("d3d11va-copy"),
        "videotoolbox" => Some("videotoolbox-copy"),
        "vaapi" => Some("vaapi-copy"),
        "auto-safe" | "auto" => Some("auto-copy-safe"),
        "auto-copy-safe" | "d3d11va-copy" | "videotoolbox-copy" | "vaapi-copy" => Some("no"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_matches_handoff() {
        let ll = profile_options(ProfileMode::LowLatency, 20);
        let get = |o: &Vec<(String, String)>, k: &str| o.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        assert_eq!(get(&ll, "cache-secs").as_deref(), Some("3"));
        assert_eq!(get(&ll, "video-latency-hacks").as_deref(), Some("yes"));
        assert_eq!(get(&ll, "demuxer-max-bytes").as_deref(), Some("32MiB"));
        assert_eq!(get(&ll, "cache").as_deref(), Some("yes"), "cache=no is forbidden");
        let st = profile_options(ProfileMode::Stable, 45);
        assert_eq!(get(&st, "cache-secs").as_deref(), Some("45"));
        assert_eq!(get(&st, "video-latency-hacks").as_deref(), Some("no"));
        let st = profile_options(ProfileMode::Stable, 5);
        assert_eq!(get(&st, "cache-secs").as_deref(), Some("20"), "clamped");
        assert!(startup_options(None, "auto-safe", "warn").iter().any(|(k, v)| k == "vo" && v == "null"));
        assert!(startup_options(Some(1234), "auto-safe", "warn").iter().any(|(k, v)| k == "wid" && v == "1234"));
    }

    #[test]
    fn fallback_chain_terminates() {
        let mut h = "auto-safe";
        let mut steps = 0;
        while let Some(next) = hwdec_fallback(h) {
            h = next;
            steps += 1;
            assert!(steps < 5);
        }
        assert_eq!(h, "no");
    }
}
