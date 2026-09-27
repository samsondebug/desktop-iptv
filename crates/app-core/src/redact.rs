//! Secret redaction. Applied to every log line, diagnostics export, error string and
//! URL that leaves the Rust core (CLAUDE.md §0 non-negotiable 6, §9).
//!
//! Keys: `password`, `pass`, `token`, `mac`, `ticket`, `Authorization`, plus Xtream
//! path-style credentials `/live/{user}/{pass}/…`.

use once_cell::sync::Lazy;
use regex::Regex;

static QUERY_KEYS: Lazy<Regex> = Lazy::new(|| {
    // key=value inside query strings, form bodies, or log lines. Case-insensitive.
    Regex::new(
        r"(?i)\b(password|passwd|pass|pwd|token|access_token|mac|mac_address|ticket|api_key|apikey|key)=([^&\s\x22']*)",
    )
    .expect("valid regex")
});

static AUTH_HEADER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(authorization\s*[:=]\s*)(\S+(?:\s+\S+)?)").expect("valid regex"));

static BASIC_AUTH_URL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(https?://)([^/\s:@]+):([^/\s@]+)@").expect("valid regex"));

// Xtream path credentials: /live/USER/PASS/123.ts , /movie/USER/PASS/..., /series/..., /USER/PASS/123
static XTREAM_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(/(?:live|movie|series|timeshift|streaming/timeshift\.php\?[^/]*)/)([^/\s?]+)/([^/\s?]+)(/)")
        .expect("valid regex")
});

// JSON-ish: "password": "xxx"
static JSON_KEYS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)("(?:password|pass|token|mac|ticket|authorization)"\s*:\s*")([^"]*)(")"#).expect("valid regex")
});

const MASK: &str = "***";

/// Redact secrets from an arbitrary string (URL, log line, JSON blob, header dump).
pub fn redact(input: &str) -> String {
    let s = QUERY_KEYS.replace_all(input, |c: &regex::Captures| format!("{}={}", &c[1], MASK));
    let s = AUTH_HEADER.replace_all(&s, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
    let s = BASIC_AUTH_URL.replace_all(&s, |c: &regex::Captures| format!("{}{}:{}@", &c[1], MASK, MASK));
    let s = XTREAM_PATH.replace_all(&s, |c: &regex::Captures| format!("{}{}/{}{}", &c[1], MASK, MASK, &c[4]));
    let s = JSON_KEYS.replace_all(&s, |c: &regex::Captures| format!("{}{}{}", &c[1], MASK, &c[3]));
    s.into_owned()
}

/// Redact a URL for display in the UI (playlist list, diagnostics). Keeps scheme + host + path shape.
pub fn redact_url(url: &str) -> String {
    redact(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_query_keys() {
        let s = "http://h/get.php?username=bob&password=hunter2&type=m3u&output=ts";
        let r = redact(s);
        assert!(r.contains("password=***"));
        assert!(r.contains("username=bob"));
        assert!(!r.contains("hunter2"));
    }

    #[test]
    fn redacts_xtream_paths() {
        let s = "http://h:8080/live/bob/hunter2/1234.ts";
        let r = redact(s);
        assert_eq!(r, "http://h:8080/live/***/***/1234.ts");
        let s2 = "http://h/movie/bob/hunter2/99.mkv";
        assert_eq!(redact(s2), "http://h/movie/***/***/99.mkv");
    }

    #[test]
    fn redacts_headers_and_json() {
        assert_eq!(redact("Authorization: Bearer abc.def"), "Authorization: ***");
        let j = r#"{"username":"bob","password":"hunter2","token":"t1"}"#;
        let r = redact(j);
        assert!(!r.contains("hunter2") && !r.contains("t1"));
        assert!(r.contains(r#""username":"bob""#));
    }

    #[test]
    fn redacts_mac_and_basic_auth() {
        assert_eq!(redact("mac=00:1A:79:AA:BB:CC"), "mac=***");
        assert_eq!(redact("http://u:p@host/x"), "http://***:***@host/x");
    }

    #[test]
    fn leaves_plain_urls_alone() {
        let s = "http://example.com/playlist.m3u8";
        assert_eq!(redact(s), s);
    }
}
