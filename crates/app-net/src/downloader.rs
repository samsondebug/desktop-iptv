//! VOD download with HTTP Range resume (CLAUDE.md §6.4).
//!
//! A VOD download is a *separate* GET (never the playback connection). Bytes go to `<path>.part`;
//! a later call resumes from that file's size with `Range: bytes=N-` and the `.part` is renamed
//! to `path` once complete. The optional sidecar `.srt` lands next to the media file.
//!
//! Every error string that can reach the UI goes through [`app_core::redact::redact`] — VOD URLs
//! carry credentials in the path (`/movie/{user}/{pass}/…`) or the query.

use crate::http::{HttpClient, REQUEST_TIMEOUT};
use crate::recorder::progress_due;
use crate::{NetError, Result};
use app_core::redact::redact;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use reqwest::StatusCode;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::watch;
use tokio::time::Instant;

/// Suffix of the in-progress file next to the target path.
pub const PART_SUFFIX: &str = ".part";

/// Progress callback: `(bytes on disk so far, total if known)`.
pub type DownloadProgress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

const PROGRESS_EVERY: Duration = Duration::from_secs(1);
const RECONNECTS: u32 = 1;
const RECONNECT_DELAY: Duration = Duration::from_millis(500);
const WRITE_BUF: usize = 256 * 1024;
/// Sidecar subtitles are read into memory; anything past this is not a subtitle file.
const MAX_SRT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct DownloadOutcome {
    /// Bytes on disk at the end of this call (the resumed prefix included).
    pub bytes: u64,
    /// Full size when the server told us (Content-Range total, else Content-Length).
    pub total: Option<u64>,
    /// Offset this call continued from; 0 when it started (or was forced to restart) from zero.
    pub resumed_from: u64,
    /// `true` once the `.part` was renamed to the target path.
    pub completed: bool,
}

/// `<path>.part` — the in-progress file a later call resumes from.
pub fn part_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(PART_SUFFIX);
    PathBuf::from(os)
}

/// Download `url` to `path`.
///
/// Writes to `<path>.part` and resumes from its size with `Range: bytes=N-`: a `206` continues,
/// a `200` restarts from zero (the server ignored the range), a `416` whose `Content-Range`
/// total equals the existing part size means the part is already complete. On success the part
/// is renamed to `path`. `on_progress(done, total)` fires at most every ~1 s. When `cancel`
/// flips to `true` the transfer stops and returns `Ok(completed = false)`, leaving the `.part`
/// for a later resume. One mid-transfer read error is retried (resuming from the part size)
/// before the call fails.
pub async fn download_with_resume(
    url: &str,
    user_agent: Option<&str>,
    path: &Path,
    on_progress: DownloadProgress,
    cancel: watch::Receiver<bool>,
) -> Result<DownloadOutcome> {
    download_inner(DownloadOptions::default(), url, user_agent, path, on_progress, cancel).await
}

/// Free bytes on the volume holding `path` (or its nearest existing ancestor). `None` when the
/// volume cannot be probed.
pub fn free_space(path: &Path) -> Option<u64> {
    let probe = path
        .ancestors()
        .find(|p| !p.as_os_str().is_empty() && p.exists())
        .map(Path::to_path_buf)
        .or_else(|| path.is_relative().then(|| PathBuf::from(".")))?;
    fs4::available_space(&probe).ok()
}

/// Sidecar subtitles: download `srt_url` next to `media_path` as `<stem>.srt` (small, no resume).
/// Returns `Ok(true)` when a file was written; `Ok(false)` when the server has no subtitle file
/// (`404`/`410`) or sent an empty body. Other failures are errors.
pub async fn download_sidecar_srt(srt_url: &str, user_agent: Option<&str>, media_path: &Path) -> Result<bool> {
    let redacted = redact(srt_url);
    validate_url(srt_url)?;
    let client = HttpClient::new(user_agent)?;
    let pending = crate::trace::start("download", "GET", srt_url);
    let resp = match client.raw().get(srt_url).timeout(REQUEST_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => {
            pending.fail(&e.to_string());
            return Err(e.into());
        }
    };
    let status = resp.status();
    pending.finish(
        status.as_u16(),
        resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
        resp.content_length(),
    );
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        tracing::info!(status = status.as_u16(), url = %redacted, "sidecar srt: none available");
        return Ok(false);
    }
    if !status.is_success() {
        return Err(NetError::Http(format!("HTTP {} from {redacted}", status.as_u16())));
    }
    if resp.content_length().is_some_and(|l| l > MAX_SRT_BYTES as u64) {
        return Err(NetError::Other(format!("subtitle file too large ({redacted})")));
    }
    let mut body = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        if buf.len() + chunk.len() > MAX_SRT_BYTES {
            return Err(NetError::Other(format!("subtitle file too large ({redacted})")));
        }
        buf.extend_from_slice(&chunk);
    }
    if buf.is_empty() {
        tracing::info!(url = %redacted, "sidecar srt: empty body, nothing written");
        return Ok(false);
    }
    let target = media_path.with_extension("srt");
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&target, &buf).await?;
    tracing::info!(bytes = buf.len(), path = %target.display(), "sidecar srt: written");
    Ok(true)
}

/// Tunables kept private so tests can shrink the throttle/backoff; production uses the defaults.
#[derive(Clone, Copy)]
struct DownloadOptions {
    progress_every: Duration,
    reconnects: u32,
    reconnect_delay: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self { progress_every: PROGRESS_EVERY, reconnects: RECONNECTS, reconnect_delay: RECONNECT_DELAY }
    }
}

async fn download_inner(
    opts: DownloadOptions,
    url: &str,
    user_agent: Option<&str>,
    path: &Path,
    on_progress: DownloadProgress,
    mut cancel: watch::Receiver<bool>,
) -> Result<DownloadOutcome> {
    let redacted = redact(url);
    validate_url(url)?;
    let client = HttpClient::new(user_agent)?;
    let part = part_path(path);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).await?;
    }

    let mut offset = file_len(&part).await?;
    let mut resumed_from = offset;
    let mut total: Option<u64> = None;
    let mut reconnects_left = opts.reconnects;
    let mut restarted = false;
    let mut last_progress: Option<Instant> = None;
    tracing::info!(url = %redacted, path = %path.display(), offset, "download: start");

    loop {
        if *cancel.borrow() {
            return Ok(DownloadOutcome { bytes: offset, total, resumed_from, completed: false });
        }
        let resp = tokio::select! {
            biased;
            _ = cancelled(&mut cancel) => {
                return Ok(DownloadOutcome { bytes: offset, total, resumed_from, completed: false });
            }
            r = send_get(&client, url, offset) => r?,
        };
        let status = resp.status();
        let content_length = resp.content_length();
        let content_range =
            resp.headers().get(CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(parse_content_range);
        tracing::info!(status = status.as_u16(), ?content_range, ?content_length, offset, "download: response");

        let mut writer = match status {
            StatusCode::PARTIAL_CONTENT => match content_range {
                Some(ContentRange::Range { start, total: t, .. }) if start == offset => {
                    total = t.or_else(|| content_length.map(|l| l + offset));
                    open_part(&part, true).await?
                }
                Some(ContentRange::Range { start, .. }) => {
                    return Err(NetError::Http(format!(
                        "HTTP 206 resumed at byte {start}, expected {offset} ({redacted})"
                    )));
                }
                _ => return Err(NetError::Http(format!("HTTP 206 without a usable Content-Range ({redacted})"))),
            },
            StatusCode::RANGE_NOT_SATISFIABLE => {
                let t = match content_range {
                    Some(ContentRange::Unsatisfied { total }) => total,
                    _ => None,
                };
                if offset > 0 && t == Some(offset) {
                    // The .part already holds the whole file.
                    total = t;
                    fs::rename(&part, path).await?;
                    on_progress(offset, total);
                    tracing::info!(bytes = offset, "download: part was already complete");
                    return Ok(DownloadOutcome { bytes: offset, total, resumed_from, completed: true });
                }
                if offset == 0 || restarted {
                    return Err(NetError::Http(format!("HTTP 416 from {redacted}")));
                }
                tracing::warn!(offset, ?t, "download: range unsatisfiable, restarting from zero");
                restarted = true;
                offset = 0;
                resumed_from = 0;
                continue;
            }
            s if s.is_success() => {
                if offset > 0 {
                    tracing::info!(offset, "download: server ignored Range, restarting from zero");
                }
                total = content_length;
                offset = 0;
                resumed_from = 0;
                open_part(&part, false).await?
            }
            s => return Err(NetError::Http(format!("HTTP {} from {redacted}", s.as_u16()))),
        };

        let mut done = offset;
        let mut body = resp.bytes_stream();
        let mut read_err: Option<NetError> = None;
        loop {
            tokio::select! {
                biased;
                _ = cancelled(&mut cancel) => {
                    writer.flush().await?;
                    tracing::info!(bytes = done, ?total, "download: cancelled, part kept");
                    return Ok(DownloadOutcome { bytes: done, total, resumed_from, completed: false });
                }
                next = body.next() => match next {
                    Some(Ok(chunk)) => {
                        writer.write_all(&chunk).await?;
                        done += chunk.len() as u64;
                        if progress_due(last_progress, opts.progress_every) {
                            last_progress = Some(Instant::now());
                            on_progress(done, total);
                        }
                    }
                    Some(Err(e)) => {
                        read_err = Some(NetError::from(e));
                        break;
                    }
                    None => break,
                }
            }
        }
        writer.flush().await?;
        drop(writer);
        offset = done;

        let err = match (read_err, total) {
            (Some(e), _) => e,
            (None, Some(t)) if done < t => {
                NetError::Http(format!("connection closed after {done} of {t} bytes ({redacted})"))
            }
            (None, _) => {
                fs::rename(&part, path).await?;
                on_progress(done, total);
                tracing::info!(bytes = done, ?total, resumed_from, path = %path.display(), "download: complete");
                return Ok(DownloadOutcome { bytes: done, total, resumed_from, completed: true });
            }
        };
        if reconnects_left == 0 {
            tracing::warn!(err = %err, bytes = done, "download: failed, part kept for a later resume");
            return Err(err);
        }
        reconnects_left -= 1;
        tracing::warn!(err = %err, offset, "download: interrupted, reconnecting");
        tokio::select! {
            biased;
            _ = cancelled(&mut cancel) => {
                return Ok(DownloadOutcome { bytes: offset, total, resumed_from, completed: false });
            }
            _ = tokio::time::sleep(opts.reconnect_delay) => {}
        }
    }
}

/// Resolves when `cancel` is `true`. A dropped sender never cancels.
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn validate_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(NetError::InvalidUrl(format!("unsupported scheme {}", parsed.scheme())));
    }
    Ok(())
}

/// GET with `Range: bytes=offset-` when resuming. `Accept-Encoding: identity` keeps
/// Content-Length and byte offsets meaningful (reqwest would otherwise ask for gzip).
async fn send_get(client: &HttpClient, url: &str, offset: u64) -> Result<reqwest::Response> {
    let mut req = client.raw().get(url).header(ACCEPT_ENCODING, "identity");
    if offset > 0 {
        req = req.header(RANGE, format!("bytes={offset}-"));
    }
    let pending = crate::trace::start("download", if offset > 0 { "GET+Range" } else { "GET" }, url);
    match req.send().await {
        Ok(resp) => {
            pending.finish(
                resp.status().as_u16(),
                resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
                resp.content_length(),
            );
            Ok(resp)
        }
        Err(e) => {
            pending.fail(&e.to_string());
            Err(e.into())
        }
    }
}

async fn file_len(path: &Path) -> Result<u64> {
    match fs::metadata(path).await {
        Ok(m) => Ok(m.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e.into()),
    }
}

async fn open_part(part: &Path, append: bool) -> Result<BufWriter<File>> {
    let file = if append {
        OpenOptions::new().create(true).append(true).open(part).await?
    } else {
        File::create(part).await?
    };
    Ok(BufWriter::with_capacity(WRITE_BUF, file))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentRange {
    /// `bytes START-END/TOTAL` (`TOTAL` may be `*`).
    Range { start: u64, end: u64, total: Option<u64> },
    /// `bytes */TOTAL` — sent with `416`.
    Unsatisfied { total: Option<u64> },
}

fn parse_content_range(v: &str) -> Option<ContentRange> {
    let v = v.trim();
    if !v.get(..5)?.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = v[5..].trim_start().split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    let range = range.trim();
    if range == "*" {
        return Some(ContentRange::Unsatisfied { total });
    }
    let (start, end) = range.split_once('-')?;
    Some(ContentRange::Range { start: start.trim().parse().ok()?, end: end.trim().parse().ok()?, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const SIZE: usize = 300 * 1024;
    const KIB: usize = 1024;

    fn body() -> Vec<u8> {
        (0..SIZE).map(|i| (i % 253) as u8).collect()
    }

    struct Mock {
        body: Vec<u8>,
        honour_range: bool,
        /// Force this status (tiny text body) for every request.
        status: Option<u16>,
        /// Close the socket after this many body bytes (first streamed response only unless
        /// `drop_every`).
        drop_after: Option<usize>,
        drop_every: bool,
        /// First streamed response only: pause for 300 ms after this many body bytes.
        stall_after: Option<usize>,
        first_done: AtomicBool,
        statuses: Mutex<Vec<u16>>,
        ranges: Mutex<Vec<Option<u64>>>,
    }

    impl Mock {
        fn new(body: Vec<u8>) -> Self {
            Self {
                body,
                honour_range: true,
                status: None,
                drop_after: None,
                drop_every: false,
                stall_after: None,
                first_done: AtomicBool::new(false),
                statuses: Mutex::new(Vec::new()),
                ranges: Mutex::new(Vec::new()),
            }
        }
        fn statuses(&self) -> Vec<u16> {
            self.statuses.lock().unwrap().clone()
        }
        fn ranges(&self) -> Vec<Option<u64>> {
            self.ranges.lock().unwrap().clone()
        }
    }

    async fn read_head(sock: &mut TcpStream) -> Option<String> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => n,
            };
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
        }
    }

    fn range_start(head: &str) -> Option<u64> {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if !k.trim().eq_ignore_ascii_case("range") {
                return None;
            }
            v.trim().strip_prefix("bytes=")?.split('-').next()?.parse().ok()
        })
    }

    /// Minimal HTTP/1.1 responder that honours (or deliberately ignores) `Range`, answering with
    /// Content-Length bodies in 16 KiB writes.
    async fn serve(mock: Arc<Mock>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mock = mock.clone();
                tokio::spawn(async move {
                    let Some(head) = read_head(&mut sock).await else { return };
                    let start = range_start(&head);
                    mock.ranges.lock().unwrap().push(start);
                    if let Some(code) = mock.status {
                        mock.statuses.lock().unwrap().push(code);
                        let body = "nope";
                        let head = format!(
                            "HTTP/1.1 {code} Forced\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(body.as_bytes()).await;
                        let _ = sock.shutdown().await;
                        return;
                    }
                    let len = mock.body.len();
                    let (code, extra, from) = match start {
                        Some(n) if mock.honour_range && n >= len as u64 => {
                            (416, format!("Content-Range: bytes */{len}\r\nContent-Length: 0\r\n"), len)
                        }
                        Some(n) if mock.honour_range => {
                            let n = n as usize;
                            (
                                206,
                                format!(
                                    "Content-Range: bytes {n}-{}/{len}\r\nContent-Length: {}\r\n",
                                    len - 1,
                                    len - n
                                ),
                                n,
                            )
                        }
                        _ => (200, format!("Content-Length: {len}\r\n"), 0),
                    };
                    mock.statuses.lock().unwrap().push(code);
                    let head = format!(
                        "HTTP/1.1 {code} X\r\nContent-Type: video/x-matroska\r\nAccept-Ranges: bytes\r\n{extra}Connection: close\r\n\r\n"
                    );
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let first = code != 416 && !mock.first_done.swap(true, Ordering::SeqCst);
                    let drop_after = if first || mock.drop_every { mock.drop_after } else { None };
                    let stall_after = if first { mock.stall_after } else { None };
                    let mut pos = from;
                    let mut sent = 0usize;
                    let mut stalled = false;
                    while pos < len {
                        let n = (len - pos).min(16 * KIB);
                        if sock.write_all(&mock.body[pos..pos + n]).await.is_err() {
                            return;
                        }
                        pos += n;
                        sent += n;
                        if drop_after.is_some_and(|d| sent >= d) {
                            return; // abrupt close mid-body
                        }
                        if !stalled && stall_after.is_some_and(|s| sent >= s) {
                            stalled = true;
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn fast() -> DownloadOptions {
        DownloadOptions { progress_every: Duration::ZERO, reconnects: 1, reconnect_delay: Duration::from_millis(20) }
    }

    fn never() -> watch::Receiver<bool> {
        watch::channel(false).1
    }

    type Seen = Arc<Mutex<Vec<(u64, Option<u64>)>>>;

    fn recorder() -> (DownloadProgress, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        (Arc::new(move |d, t| sink.lock().unwrap().push((d, t))), seen)
    }

    /// Cancels once `done >= at`.
    fn cancel_at(at: u64) -> (DownloadProgress, watch::Receiver<bool>) {
        let (tx, rx) = watch::channel(false);
        let tx = Arc::new(tx);
        let progress: DownloadProgress = Arc::new(move |done, _| {
            if done >= at {
                let _ = tx.send(true);
            }
        });
        (progress, rx)
    }

    #[tokio::test]
    async fn cancel_then_resume_produces_identical_file() {
        let mut mock = Mock::new(body());
        mock.stall_after = Some(128 * KIB);
        let mock = Arc::new(mock);
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vod").join("movie.mkv");
        let url = format!("{base}/movie/u/p/1.mkv");
        let part = part_path(&path);

        let (progress, cancel) = cancel_at(100 * KIB as u64);
        let first = download_inner(fast(), &url, None, &path, progress, cancel).await.unwrap();
        assert!(!first.completed, "{first:?}");
        assert!(first.bytes >= 100 * KIB as u64 && first.bytes < SIZE as u64, "{first:?}");
        assert_eq!(first.resumed_from, 0);
        assert_eq!(first.total, Some(SIZE as u64));
        assert_eq!(std::fs::metadata(&part).unwrap().len(), first.bytes, "part flushed on cancel");
        assert!(!path.exists());

        let (progress, seen) = recorder();
        let second = download_inner(fast(), &url, None, &path, progress, never()).await.unwrap();
        assert!(second.completed, "{second:?}");
        assert_eq!(second.resumed_from, first.bytes);
        assert!(second.resumed_from > 0);
        assert_eq!(second.bytes, SIZE as u64);
        assert_eq!(second.total, Some(SIZE as u64));
        assert!(!part.exists(), "part renamed away");
        assert_eq!(std::fs::read(&path).unwrap(), body(), "byte-identical after resume");
        assert_eq!(mock.statuses(), vec![200, 206]);
        assert_eq!(mock.ranges(), vec![None, Some(first.bytes)]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.last().copied(), Some((SIZE as u64, Some(SIZE as u64))));
        assert!(seen.iter().all(|(d, _)| *d >= first.bytes), "progress reports include the resumed prefix");
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_from_zero() {
        let mut mock = Mock::new(body());
        mock.honour_range = false;
        mock.stall_after = Some(128 * KIB);
        let mock = Arc::new(mock);
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");
        let url = format!("{base}/movie/u/p/2.mkv");

        let (progress, cancel) = cancel_at(100 * KIB as u64);
        let first = download_inner(fast(), &url, None, &path, progress, cancel).await.unwrap();
        assert!(!first.completed && first.bytes >= 100 * KIB as u64 && first.bytes < SIZE as u64, "{first:?}");

        let (progress, _) = recorder();
        let second = download_inner(fast(), &url, None, &path, progress, never()).await.unwrap();
        assert!(second.completed, "{second:?}");
        assert_eq!(second.resumed_from, 0, "a 200 means the server restarted us");
        assert_eq!(second.bytes, SIZE as u64);
        assert_eq!(std::fs::read(&path).unwrap(), body());
        assert!(!part_path(&path).exists());
        assert_eq!(mock.statuses(), vec![200, 200]);
        assert_eq!(mock.ranges(), vec![None, Some(first.bytes)], "we did ask for a range");
    }

    #[tokio::test]
    async fn full_size_part_completes_via_416() {
        let mock = Arc::new(Mock::new(body()));
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");
        let part = part_path(&path);
        std::fs::write(&part, body()).unwrap();

        let (progress, seen) = recorder();
        let out =
            download_inner(fast(), &format!("{base}/movie/u/p/3.mkv"), None, &path, progress, never()).await.unwrap();
        assert!(out.completed, "{out:?}");
        assert_eq!((out.bytes, out.total, out.resumed_from), (SIZE as u64, Some(SIZE as u64), SIZE as u64));
        assert_eq!(mock.statuses(), vec![416]);
        assert!(!part.exists());
        assert_eq!(std::fs::read(&path).unwrap(), body());
        assert_eq!(seen.lock().unwrap().as_slice(), &[(SIZE as u64, Some(SIZE as u64))]);
    }

    #[tokio::test]
    async fn reconnects_once_after_mid_transfer_drop() {
        let mut mock = Mock::new(body());
        mock.drop_after = Some(128 * KIB);
        let mock = Arc::new(mock);
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");

        let (progress, _) = recorder();
        let out =
            download_inner(fast(), &format!("{base}/movie/u/p/4.mkv"), None, &path, progress, never()).await.unwrap();
        assert!(out.completed, "{out:?}");
        assert_eq!(out.bytes, SIZE as u64);
        assert_eq!(out.resumed_from, 0, "resumed_from describes the start of the call");
        assert_eq!(std::fs::read(&path).unwrap(), body());
        assert_eq!(mock.statuses(), vec![200, 206]);
        let ranges = mock.ranges();
        assert_eq!(ranges.len(), 2);
        let resumed = ranges[1].expect("second request carried a Range");
        assert!(resumed > 0 && resumed <= 128 * KIB as u64, "{resumed}");
    }

    #[tokio::test]
    async fn repeated_drops_fail_after_one_reconnect_and_keep_the_part() {
        let mut mock = Mock::new(body());
        mock.drop_after = Some(64 * KIB);
        mock.drop_every = true;
        let mock = Arc::new(mock);
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");
        let (progress, _) = recorder();
        let url = format!("{base}/movie/u/p/5.mkv?password=secret");
        let err = download_inner(fast(), &url, None, &path, progress, never()).await.unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("secret"), "credentials leaked: {msg}");
        assert_eq!(mock.statuses(), vec![200, 206], "one reconnect, then failure");
        let part = part_path(&path);
        assert!(part.exists(), "part kept for a later resume");
        assert!(std::fs::metadata(&part).unwrap().len() > 0);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn not_found_error_is_redacted() {
        let mut mock = Mock::new(body());
        mock.status = Some(404);
        let mock = Arc::new(mock);
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");
        let url = format!("{base}/get.php?username=bob&password=secret&type=m3u");
        let (progress, seen) = recorder();
        let err = download_inner(fast(), &url, None, &path, progress, never()).await.unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, NetError::Http(_)), "{err:?}");
        assert!(msg.contains("404"), "{msg}");
        assert!(!msg.contains("secret"), "credentials leaked: {msg}");
        assert!(msg.contains("password=***"), "{msg}");
        assert!(!part_path(&path).exists(), "nothing opened before the status check");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn already_cancelled_returns_without_a_request() {
        let mock = Arc::new(Mock::new(body()));
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mkv");
        let (_tx, rx) = watch::channel(true);
        let (progress, _) = recorder();
        let out = download_inner(fast(), &format!("{base}/movie/u/p/6.mkv"), None, &path, progress, rx).await.unwrap();
        assert!(!out.completed && out.bytes == 0, "{out:?}");
        assert!(mock.statuses().is_empty());
    }

    #[tokio::test]
    async fn sidecar_srt_is_written_next_to_the_media() {
        let srt = b"1\n00:00:01,000 --> 00:00:02,000\nHello\n\n".to_vec();
        let mock = Arc::new(Mock::new(srt.clone()));
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("season.1").join("episode.01.mkv");
        let written = download_sidecar_srt(&format!("{base}/movie/u/p/7.srt"), None, &media).await.unwrap();
        assert!(written);
        assert_eq!(std::fs::read(dir.path().join("season.1").join("episode.01.srt")).unwrap(), srt);

        let mut missing = Mock::new(Vec::new());
        missing.status = Some(404);
        let base = serve(Arc::new(missing)).await;
        let media = dir.path().join("other.mkv");
        let written = download_sidecar_srt(&format!("{base}/movie/u/p/8.srt?token=t"), None, &media).await.unwrap();
        assert!(!written);
        assert!(!dir.path().join("other.srt").exists());
    }

    #[test]
    fn free_space_probes_the_nearest_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does").join("not").join("exist.mkv");
        let a = free_space(&missing).expect("volume probed via ancestor");
        let b = free_space(dir.path()).expect("volume probed directly");
        assert!(a > 0 && b > 0);
        assert!(free_space(Path::new("relative/does/not/exist.mkv")).is_some());
    }

    #[test]
    fn part_path_appends_suffix_to_the_full_name() {
        assert_eq!(part_path(Path::new("/x/movie.mkv")), PathBuf::from("/x/movie.mkv.part"));
        assert_eq!(part_path(Path::new("movie")), PathBuf::from("movie.part"));
    }

    #[test]
    fn content_range_parsing() {
        use ContentRange::*;
        assert_eq!(parse_content_range("bytes 100-299/300"), Some(Range { start: 100, end: 299, total: Some(300) }));
        assert_eq!(parse_content_range("BYTES 0-9/*"), Some(Range { start: 0, end: 9, total: None }));
        assert_eq!(parse_content_range("bytes */300"), Some(Unsatisfied { total: Some(300) }));
        assert_eq!(parse_content_range(" bytes */* "), Some(Unsatisfied { total: None }));
        assert_eq!(parse_content_range("items 0-1/2"), None);
        assert_eq!(parse_content_range("bytes 0-1"), None);
        assert_eq!(parse_content_range("bytes x-1/2"), None);
        assert_eq!(parse_content_range(""), None);
    }
}
