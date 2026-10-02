//! Catch-up / archive replay URL construction (player-side only: we *consume* the provider's
//! own archive endpoints, never host or relay anything).
//!
//! Modes follow the de-facto M3U `catchup=` attribute plus the Xtream API:
//! * `xc` — Xtream Codes `timeshift` endpoint, derived from the live URL (which carries
//!   the credentials; they never leave Rust).
//! * `flussonic` — Flussonic archive playlist next to the live `index.m3u8`.
//! * `default`   — the provider's `catchup-source` template with `{...}` placeholders.
//! * `append`    — `catchup-source` (or its query) appended to the live URL.
//! * `shift`     — `?utc=<start>&lutc=<now>` appended to the live URL.
//!
//! All times are unix seconds (UTC).

use app_db::channels::CatchupInfo;

#[derive(Debug, thiserror::Error)]
pub enum CatchupError {
    #[error("this channel has no catch-up/archive")]
    NotSupported,
    #[error("programme is outside the {0}-day archive window")]
    OutsideWindow(i32),
    #[error("could not derive a replay URL from the live URL for mode '{0}'")]
    BadLiveUrl(String),
}

/// Local civil-time pieces of a unix timestamp, UTC. (Xtream panels expect the *panel's* zone,
/// which for the common panels is UTC; an EPG offset is already applied upstream.)
fn utc_parts(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (y, m as u32, d as u32, (secs / 3600) as u32, ((secs % 3600) / 60) as u32, (secs % 60) as u32)
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Fill a `catchup-source` style template. Supported placeholders (the common set across
/// TiviMate/Kodi/OTT Navigator playlists): `{utc}`, `${start}`, `{start}`, `{lutc}`, `${end}`,
/// `{end}`, `{duration}` (seconds), `{durmin}` (minutes), `{offset}` (now-start, seconds), and
/// `{Y}/{m}/{d}/{H}/{M}/{S}` of the start time.
fn fill_template(t: &str, start: i64, stop: i64, now: i64) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(start);
    let dur = (stop - start).max(60);
    t.replace("${start}", &start.to_string())
        .replace("{start}", &start.to_string())
        .replace("{utc}", &start.to_string())
        .replace("${end}", &stop.to_string())
        .replace("{end}", &stop.to_string())
        .replace("{utcend}", &stop.to_string())
        .replace("{lutc}", &now.to_string())
        .replace("{now}", &now.to_string())
        .replace("{duration}", &dur.to_string())
        .replace("{durmin}", &((dur + 59) / 60).to_string())
        .replace("{offset}", &(now - start).max(0).to_string())
        .replace("{Y}", &format!("{y:04}"))
        .replace("{m}", &format!("{mo:02}"))
        .replace("{d}", &format!("{d:02}"))
        .replace("{H}", &format!("{h:02}"))
        .replace("{M}", &format!("{mi:02}"))
        .replace("{S}", &format!("{s:02}"))
}

/// Xtream live URL → timeshift URL. Handles both common live forms
/// `{base}/live/{user}/{pass}/{id}.{ext}` and `{base}/{user}/{pass}/{id}`:
/// replay is `{base}/timeshift/{user}/{pass}/{durmin}/{YYYY-MM-DD:HH-MM}/{id}.ts`.
fn xtream_url(live: &str, start: i64, stop: i64) -> Option<String> {
    let (scheme_host, path) = {
        let after = live.find("://")? + 3;
        let slash = live[after..].find('/')? + after;
        (&live[..slash], &live[slash + 1..])
    };
    let path = path.split('?').next().unwrap_or(path);
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.first() == Some(&"live") {
        parts.remove(0);
    }
    if parts.len() != 3 {
        return None;
    }
    let (user, pass, last) = (parts[0], parts[1], parts[2]);
    let id = last.split('.').next().unwrap_or(last);
    if id.is_empty() {
        return None;
    }
    let durmin = ((stop - start).max(60) + 59) / 60;
    let (y, mo, d, h, mi, _) = utc_parts(start);
    Some(format!("{scheme_host}/timeshift/{user}/{pass}/{durmin}/{y:04}-{mo:02}-{d:02}:{h:02}-{mi:02}/{id}.ts"))
}

/// Flussonic: `.../stream/index.m3u8` (or `mono.m3u8` / `video.m3u8` / `stream.m3u8`) →
/// `.../stream/archive-{start}-{duration}.m3u8`.
fn flussonic_url(live: &str, start: i64, stop: i64) -> Option<String> {
    let dur = (stop - start).max(60);
    let (path, query) = match live.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (live, None),
    };
    let replaced = if let Some(pos) = path.rfind('/') {
        let (dir, file) = (&path[..pos], &path[pos + 1..]);
        if matches!(file, "index.m3u8" | "mono.m3u8" | "video.m3u8" | "playlist.m3u8") {
            format!("{dir}/archive-{start}-{dur}.m3u8")
        } else if let Some(stem) = file.strip_suffix(".m3u8") {
            // `.../streamname.m3u8` → the stream name is the last path segment.
            format!("{dir}/{stem}/archive-{start}-{dur}.m3u8")
        } else if file.contains('.') {
            return None; // .ts etc. — not a flussonic playlist URL
        } else {
            format!("{path}/archive-{start}-{dur}.m3u8")
        }
    } else {
        return None;
    };
    Some(match query {
        Some(q) => format!("{replaced}?{q}"),
        None => replaced,
    })
}

/// Build the replay URL for `[start, stop]` on this channel, or say why it can't be done.
pub fn replay_url(info: &CatchupInfo, start: i64, stop: i64, now: i64) -> Result<String, CatchupError> {
    if !info.catchup && info.catchup_days <= 0 && info.catchup_source.is_none() {
        return Err(CatchupError::NotSupported);
    }
    let days = if info.catchup_days > 0 { info.catchup_days } else { 1 };
    if start < now - days as i64 * 86_400 {
        return Err(CatchupError::OutsideWindow(days));
    }
    let kind =
        info.catchup_kind.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(str::to_lowercase).unwrap_or_else(
            || {
                if info.catchup_source.is_some() {
                    "default".into()
                } else if info.playlist_type == "xtream" {
                    "xc".into()
                } else {
                    "shift".into()
                }
            },
        );
    let live = info.stream_url.as_str();
    match kind.as_str() {
        "xc" | "xtream" => xtream_url(live, start, stop).ok_or_else(|| CatchupError::BadLiveUrl(kind.clone())),
        "flussonic" | "flussonic-hls" | "fs" => {
            flussonic_url(live, start, stop).ok_or_else(|| CatchupError::BadLiveUrl(kind.clone()))
        }
        "append" => {
            let t = info.catchup_source.as_deref().ok_or(CatchupError::NotSupported)?;
            Ok(format!("{live}{}", fill_template(t, start, stop, now)))
        }
        "shift" | "timeshift" => {
            let sep = if live.contains('?') { '&' } else { '?' };
            Ok(format!("{live}{sep}utc={start}&lutc={now}"))
        }
        // "default" and anything unrecognized: a template wins; otherwise fall back by source type.
        _ => {
            if let Some(t) = info.catchup_source.as_deref() {
                if t.starts_with('?') || t.starts_with('&') {
                    let sep = if live.contains('?') { "&" } else { "?" };
                    Ok(format!("{live}{sep}{}", fill_template(t.trim_start_matches(['?', '&']), start, stop, now)))
                } else {
                    Ok(fill_template(t, start, stop, now))
                }
            } else if info.playlist_type == "xtream" {
                xtream_url(live, start, stop).ok_or_else(|| CatchupError::BadLiveUrl(kind.clone()))
            } else {
                let sep = if live.contains('?') { '&' } else { '?' };
                Ok(format!("{live}{sep}utc={start}&lutc={now}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(kind: Option<&str>, source: Option<&str>, ptype: &str, url: &str) -> CatchupInfo {
        CatchupInfo {
            channel_id: 1,
            name: "Ch".into(),
            stream_url: url.into(),
            catchup: true,
            catchup_days: 7,
            catchup_kind: kind.map(Into::into),
            catchup_source: source.map(Into::into),
            playlist_type: ptype.into(),
        }
    }

    // 2026-10-01 18:30:00 UTC, one-hour show, "now" an hour after it ended.
    const START: i64 = 1790879400;
    const STOP: i64 = START + 3600;
    const NOW: i64 = STOP + 3600;

    #[test]
    fn xtream_timeshift_from_live_url() {
        let i = info(Some("xc"), None, "xtream", "http://host.tv:8080/live/alice/s3cret/4242.ts");
        assert_eq!(
            replay_url(&i, START, STOP, NOW).unwrap(),
            "http://host.tv:8080/timeshift/alice/s3cret/60/2026-10-01:18-30/4242.ts"
        );
        // No /live/ prefix, m3u8 ext.
        let i = info(None, None, "xtream", "http://host.tv/alice/s3cret/4242.m3u8");
        assert_eq!(
            replay_url(&i, START, STOP, NOW).unwrap(),
            "http://host.tv/timeshift/alice/s3cret/60/2026-10-01:18-30/4242.ts"
        );
    }

    #[test]
    fn flussonic_archive() {
        let i = info(Some("flussonic"), None, "m3u", "http://fl.example/bbc/index.m3u8");
        assert_eq!(replay_url(&i, START, STOP, NOW).unwrap(), "http://fl.example/bbc/archive-1790879400-3600.m3u8");
        let i = info(Some("flussonic"), None, "m3u", "http://fl.example/bbc.m3u8?token=t");
        assert_eq!(
            replay_url(&i, START, STOP, NOW).unwrap(),
            "http://fl.example/bbc/archive-1790879400-3600.m3u8?token=t"
        );
    }

    #[test]
    fn default_template_and_append_and_shift() {
        let i = info(
            Some("default"),
            Some("http://a.example/replay?start=${start}&end=${end}&d={duration}"),
            "m3u",
            "http://a.example/live.m3u8",
        );
        assert_eq!(
            replay_url(&i, START, STOP, NOW).unwrap(),
            format!("http://a.example/replay?start={START}&end={STOP}&d=3600")
        );
        let i = info(Some("append"), Some("&utc={utc}&lutc={lutc}"), "m3u", "http://a.example/live?x=1");
        assert_eq!(
            replay_url(&i, START, STOP, NOW).unwrap(),
            format!("http://a.example/live?x=1&utc={START}&lutc={NOW}")
        );
        let i = info(Some("shift"), None, "m3u", "http://a.example/live");
        assert_eq!(replay_url(&i, START, STOP, NOW).unwrap(), format!("http://a.example/live?utc={START}&lutc={NOW}"));
    }

    #[test]
    fn window_and_support_checks() {
        let mut i = info(Some("xc"), None, "xtream", "http://h/live/u/p/1.ts");
        assert!(matches!(
            replay_url(&i, NOW - 9 * 86_400, NOW - 9 * 86_400 + 3600, NOW),
            Err(CatchupError::OutsideWindow(7))
        ));
        i.catchup = false;
        i.catchup_days = 0;
        assert!(matches!(replay_url(&i, START, STOP, NOW), Err(CatchupError::NotSupported)));
    }

    #[test]
    fn template_date_parts() {
        let i = info(Some("default"), Some("http://x/{Y}-{m}-{d}/{H}:{M}:{S}/go"), "m3u", "http://x/l");
        assert_eq!(replay_url(&i, START, STOP, NOW).unwrap(), "http://x/2026-10-01/18:30:00/go");
    }
}
