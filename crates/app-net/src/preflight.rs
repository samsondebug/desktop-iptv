//! Sniff the first 512 bytes of a "playlist" before parsing it (CLAUDE.md §0B, §7.2).
//!
//! The #1 support ticket is a provider URL that returns an HTML login page, a JSON error
//! blob, or the XMLTV guide instead of an M3U. We refuse those with a clear diagnostic.

use serde::{Deserialize, Serialize};

pub const SNIFF_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadKind {
    M3u,
    /// Looks like a plain list of URLs (no `#EXTM3U` header). Parsed leniently.
    BareUrlList,
    Html,
    Xmltv,
    Json,
    Empty,
    Unknown,
}

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct PreflightError {
    pub kind: PayloadKind,
    pub message: String,
    /// Printable, redacted preview of the first bytes for the diagnostics panel.
    pub preview: String,
}

/// Classify the first bytes of a response body. Strips a UTF-8 BOM first.
pub fn sniff(head: &[u8], content_type: Option<&str>) -> PayloadKind {
    let head = strip_bom(head);
    let text = String::from_utf8_lossy(&head[..head.len().min(SNIFF_BYTES)]);
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();

    if trimmed.is_empty() {
        return PayloadKind::Empty;
    }
    if lower.starts_with("#extm3u") {
        return PayloadKind::M3u;
    }
    if lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.contains("<head")
        || lower.contains("<body")
    {
        return PayloadKind::Html;
    }
    if lower.starts_with("<?xml") || lower.starts_with("<tv") {
        if lower.contains("<tv") || lower.contains("<programme") || lower.contains("<channel") {
            return PayloadKind::Xmltv;
        }
        return PayloadKind::Unknown;
    }
    if lower.starts_with('{') || lower.starts_with('[') {
        return PayloadKind::Json;
    }
    // Some providers omit the header but still emit #EXTINF lines or bare URLs.
    if lower.starts_with("#extinf") {
        return PayloadKind::M3u;
    }
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("rtmp://")
        || lower.starts_with("rtsp://")
    {
        return PayloadKind::BareUrlList;
    }
    // Content-Type is a weak hint (providers lie), used only when bytes are inconclusive.
    if let Some(ct) = content_type.map(|c| c.to_ascii_lowercase()) {
        if ct.contains("text/html") {
            return PayloadKind::Html;
        }
        if ct.contains("mpegurl") || ct.contains("x-mpegurl") || ct.contains("audio/mpegurl") {
            return PayloadKind::M3u;
        }
        if ct.contains("xml") {
            return PayloadKind::Xmltv;
        }
        if ct.contains("json") {
            return PayloadKind::Json;
        }
    }
    PayloadKind::Unknown
}

/// Turn a sniff result into Ok (parse it) or a user-facing error.
pub fn check(head: &[u8], content_type: Option<&str>) -> std::result::Result<PayloadKind, PreflightError> {
    let kind = sniff(head, content_type);
    let preview = preview_of(head);
    let msg = match kind {
        PayloadKind::M3u | PayloadKind::BareUrlList => return Ok(kind),
        PayloadKind::Html => {
            "The URL returned an HTML page, not an M3U playlist. Usually a login/portal page, an expired \
             account, or a wrong path. Open the URL in a browser to see what the provider is saying."
        }
        PayloadKind::Xmltv => {
            "The URL returned an XMLTV guide (EPG), not an M3U playlist. Add it as the EPG source instead."
        }
        PayloadKind::Json => {
            "The URL returned JSON, not an M3U playlist. If this is an Xtream panel, add it as an \
             Xtream source with username/password instead of a .m3u link."
        }
        PayloadKind::Empty => "The URL returned an empty body.",
        PayloadKind::Unknown => "The URL did not return a recognizable M3U playlist.",
    };
    Err(PreflightError { kind, message: msg.to_string(), preview })
}

pub fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

/// First 200 bytes, printable, redacted.
pub fn preview_of(head: &[u8]) -> String {
    let head = strip_bom(head);
    let s = String::from_utf8_lossy(&head[..head.len().min(200)]);
    let cleaned: String = s.chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
    app_core::redact::redact(&cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        assert_eq!(sniff(b"#EXTM3U\n#EXTINF:-1,X\nhttp://x", None), PayloadKind::M3u);
        assert_eq!(sniff(b"\xEF\xBB\xBF#EXTM3U url-tvg=\"x\"\n", None), PayloadKind::M3u);
        assert_eq!(sniff(b"  \n#extinf:-1,X\nhttp://x", None), PayloadKind::M3u);
        assert_eq!(sniff(b"<!DOCTYPE html><html><head>", None), PayloadKind::Html);
        assert_eq!(sniff(b"<html lang=en>", None), PayloadKind::Html);
        assert_eq!(sniff(b"<?xml version=\"1.0\"?>\n<tv generator-info-name=\"x\">", None), PayloadKind::Xmltv);
        assert_eq!(sniff(b"{\"user_info\":{\"auth\":0}}", None), PayloadKind::Json);
        assert_eq!(sniff(b"http://a/1.ts\nhttp://a/2.ts", None), PayloadKind::BareUrlList);
        assert_eq!(sniff(b"", None), PayloadKind::Empty);
        assert_eq!(sniff(b"garbage", Some("text/html; charset=utf-8")), PayloadKind::Html);
        assert_eq!(sniff(b"garbage", Some("application/x-mpegurl")), PayloadKind::M3u);
        assert_eq!(sniff(b"garbage", None), PayloadKind::Unknown);
    }

    #[test]
    fn check_messages_are_redacted() {
        let err = check(b"<html><body>bad password=hunter2</body>", None).unwrap_err();
        assert_eq!(err.kind, PayloadKind::Html);
        assert!(err.message.contains("HTML"));
        assert!(!err.preview.contains("hunter2"));
        assert!(check(b"#EXTM3U\n", None).is_ok());
    }
}
