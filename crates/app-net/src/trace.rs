//! HTTP trace ring buffer for the diagnostics workbench (CLAUDE.md §9).
//!
//! Every request the app makes — playlist fetches, Xtream API calls, XMLTV, recorder and
//! downloader GETs, probes — leaves one [`HttpTrace`] here: method, **redacted** URL, status,
//! timing, content type, the first 200 bytes (redacted, printable) and the redirect chain.
//! The buffer is process-global, bounded (`CAPACITY`) and cheap to append to; the Tauri layer
//! exposes it read-only for the "Copy sanitized report" button.
//!
//! Redirect hops come from a custom reqwest redirect policy: the policy closure has no request
//! context, so hops are parked keyed by the *first* URL of the chain and picked up when the
//! response for that URL arrives.

use app_core::redact::redact;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const CAPACITY: usize = 250;
pub const PREVIEW_BYTES: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpTrace {
    pub id: u64,
    /// Unix milliseconds when the request was sent.
    pub at_ms: u64,
    /// What the request was for: `playlist`, `xtream-api`, `epg`, `record`, `download`, `probe`, `stalker`.
    pub kind: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub elapsed_ms: Option<u64>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    /// First bytes of the body, redacted and escaped (non-printables as `\xNN`).
    pub preview: Option<String>,
    /// Every hop after the first URL, redacted.
    pub redirects: Vec<String>,
    pub error: Option<String>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static TRACES: Lazy<Mutex<VecDeque<HttpTrace>>> = Lazy::new(|| Mutex::new(VecDeque::with_capacity(CAPACITY)));
/// Redirect hops parked by the redirect policy, keyed by the chain's first (unredacted) URL.
static PENDING_REDIRECTS: Lazy<Mutex<HashMap<String, Vec<String>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A request in flight. Dropping it without [`Pending::finish`] / [`Pending::fail`] records the
/// entry as an error ("dropped before completion") so cancelled tasks still leave a mark.
pub struct Pending {
    id: u64,
    started: Instant,
    raw_url: String,
    done: bool,
}

/// Record the start of a request. Returns a handle used to fill in the response.
pub fn start(kind: &str, method: &str, url: &str) -> Pending {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    push(HttpTrace {
        id,
        at_ms: now_ms(),
        kind: kind.to_string(),
        method: method.to_string(),
        url: redact(url),
        status: None,
        elapsed_ms: None,
        content_type: None,
        content_length: None,
        preview: None,
        redirects: Vec::new(),
        error: None,
    });
    Pending { id, started: Instant::now(), raw_url: url.to_string(), done: false }
}

impl Pending {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Response headers arrived.
    pub fn finish(mut self, status: u16, content_type: Option<&str>, content_length: Option<u64>) -> u64 {
        self.done = true;
        let elapsed = self.started.elapsed().as_millis() as u64;
        let hops = take_redirects(&self.raw_url);
        update(self.id, |t| {
            t.status = Some(status);
            t.elapsed_ms = Some(elapsed);
            t.content_type = content_type.map(|s| s.chars().take(80).collect());
            t.content_length = content_length;
            t.redirects = hops;
        });
        self.id
    }

    /// Transport-level failure (DNS, TLS, timeout…). The message is redacted.
    pub fn fail(mut self, error: &str) -> u64 {
        self.done = true;
        let elapsed = self.started.elapsed().as_millis() as u64;
        let hops = take_redirects(&self.raw_url);
        let msg = redact(error);
        update(self.id, |t| {
            t.elapsed_ms = Some(elapsed);
            t.error = Some(msg);
            t.redirects = hops;
        });
        self.id
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.done {
            let elapsed = self.started.elapsed().as_millis() as u64;
            let _ = take_redirects(&self.raw_url);
            update(self.id, |t| {
                t.elapsed_ms = Some(elapsed);
                if t.error.is_none() {
                    t.error = Some("dropped before completion".into());
                }
            });
        }
    }
}

/// Attach the first body bytes to an entry (call once, with the first chunk).
pub fn set_preview(id: u64, first_bytes: &[u8]) {
    let preview = preview_of(first_bytes);
    update(id, |t| {
        if t.preview.is_none() {
            t.preview = Some(preview);
        }
    });
}

/// Mark an entry as failed after the headers were already recorded (mid-body error).
pub fn set_error(id: u64, error: &str) {
    let msg = redact(error);
    update(id, |t| t.error = Some(msg));
}

/// One-shot record for calls that are not streamed through [`start`] (tests, probes).
#[allow(clippy::too_many_arguments)]
pub fn record(
    kind: &str,
    method: &str,
    url: &str,
    status: Option<u16>,
    elapsed_ms: u64,
    content_type: Option<&str>,
    preview: Option<&[u8]>,
    error: Option<&str>,
) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    push(HttpTrace {
        id,
        at_ms: now_ms(),
        kind: kind.to_string(),
        method: method.to_string(),
        url: redact(url),
        status,
        elapsed_ms: Some(elapsed_ms),
        content_type: content_type.map(|s| s.chars().take(80).collect()),
        content_length: None,
        preview: preview.map(preview_of),
        redirects: take_redirects(url),
        error: error.map(redact),
    });
    id
}

/// Newest last.
pub fn snapshot() -> Vec<HttpTrace> {
    TRACES.lock().map(|t| t.iter().cloned().collect()).unwrap_or_default()
}

pub fn clear() {
    if let Ok(mut t) = TRACES.lock() {
        t.clear();
    }
    if let Ok(mut p) = PENDING_REDIRECTS.lock() {
        p.clear();
    }
}

/// Called by the redirect policy: `previous` is the chain so far (first element = original URL),
/// `next` the hop about to be followed.
pub fn note_redirect(previous: &[url::Url], next: &url::Url) {
    let Some(first) = previous.first() else { return };
    if let Ok(mut p) = PENDING_REDIRECTS.lock() {
        // Bound the side table too: a runaway client must not grow it forever.
        if p.len() > 256 {
            p.clear();
        }
        p.entry(first.to_string()).or_default().push(redact(next.as_str()));
    }
}

fn take_redirects(raw_url: &str) -> Vec<String> {
    PENDING_REDIRECTS.lock().ok().and_then(|mut p| p.remove(raw_url)).unwrap_or_default()
}

fn push(t: HttpTrace) {
    if let Ok(mut buf) = TRACES.lock() {
        if buf.len() >= CAPACITY {
            buf.pop_front();
        }
        buf.push_back(t);
    }
}

fn update(id: u64, f: impl FnOnce(&mut HttpTrace)) {
    if let Ok(mut buf) = TRACES.lock() {
        if let Some(t) = buf.iter_mut().rev().find(|t| t.id == id) {
            f(t);
        }
    }
}

/// Printable, redacted preview of the first bytes of a body.
pub fn preview_of(bytes: &[u8]) -> String {
    let head = &bytes[..bytes.len().min(PREVIEW_BYTES)];
    let text = String::from_utf8_lossy(head);
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || c == '\u{fffd}' => out.push_str(&format!("\\x{:02x}", c as u32 & 0xff)),
            c => out.push(c),
        }
    }
    redact(&out)
}

/// Render the buffer as text for the sanitized report.
pub fn render(traces: &[HttpTrace]) -> String {
    let mut s = String::new();
    for t in traces {
        let status = t.status.map(|c| c.to_string()).unwrap_or_else(|| "---".into());
        let ms = t.elapsed_ms.map(|m| format!("{m} ms")).unwrap_or_else(|| "…".into());
        s.push_str(&format!(
            "{} {:<11} {} {} → {} in {} [{}{}]\n",
            fmt_ts(t.at_ms),
            t.kind,
            t.method,
            t.url,
            status,
            ms,
            t.content_type.as_deref().unwrap_or("-"),
            t.content_length.map(|n| format!(", {n} B")).unwrap_or_default()
        ));
        for (i, hop) in t.redirects.iter().enumerate() {
            s.push_str(&format!("    ↳ redirect {}: {}\n", i + 1, hop));
        }
        if let Some(e) = &t.error {
            s.push_str(&format!("    ✗ {e}\n"));
        }
        if let Some(p) = &t.preview {
            s.push_str(&format!("    ≡ {p}\n"));
        }
    }
    s
}

fn fmt_ts(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}.{:03}Z", ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_is_bounded_and_redacted() {
        clear();
        for i in 0..(CAPACITY + 20) {
            let p = start("playlist", "GET", &format!("http://h/get.php?username=u&password=s{i}"));
            p.finish(200, Some("audio/mpegurl"), Some(10));
        }
        let snap = snapshot();
        assert_eq!(snap.len(), CAPACITY);
        assert!(snap.iter().all(|t| t.url.contains("password=***")));
        assert!(snap.iter().all(|t| t.status == Some(200)));
    }

    #[test]
    fn preview_is_printable_and_redacted() {
        let p = preview_of(b"#EXTM3U\n#EXTINF:-1,X\nhttp://h/live/user/secret/1.ts\n\x00\xff");
        assert!(p.contains("\\n"));
        assert!(p.contains("/live/***/***/1.ts"));
        assert!(!p.contains("secret"));
        assert!(p.contains("\\x00"));
    }

    #[test]
    fn dropped_pending_is_recorded_as_error() {
        clear();
        {
            let _p = start("probe", "GET", "http://h/x");
        }
        let snap = snapshot();
        assert_eq!(snap.last().unwrap().error.as_deref(), Some("dropped before completion"));
    }

    #[test]
    fn redirects_are_attached_to_the_right_entry() {
        clear();
        let a = url::Url::parse("http://a/first").unwrap();
        let b = url::Url::parse("http://b/second?token=t").unwrap();
        let p = start("playlist", "GET", a.as_str());
        note_redirect(std::slice::from_ref(&a), &b);
        p.finish(200, None, None);
        let t = snapshot().pop().unwrap();
        assert_eq!(t.redirects, vec!["http://b/second?token=***".to_string()]);
        let text = render(&[t]);
        assert!(text.contains("redirect 1: http://b/second?token=***"));
    }
}
