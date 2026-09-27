//! Raw stream tee to disk for **scheduled** recordings (CLAUDE.md §6.4).
//!
//! Scope: this records a channel that is *not* the one being watched. It opens its own single
//! HTTP(S) GET for the live URL and appends the raw bytes (MPEG-TS as served; no remux) to the
//! target file until the deadline passes or [`RecordControl::stop`] is called. The channel that
//! *is* being watched is recorded by mpv's own `stream-record` on the playback connection —
//! never by this module — because providers ban a second connection to the same stream.
//!
//! * A drop or read error before the deadline is retried with 1 s, 2 s, 5 s, 10 s, 10 s… backoff.
//!   The file is opened in append mode, so a reconnect (or a resumed job) continues the same file.
//! * Six consecutive attempts that deliver nothing (non-2xx, connect error, empty body) end the
//!   job: the handle resolves to `Err(last_error)` and the caller records [`RecordEnd::Failed`].
//! * HLS playlists (`.m3u8`) are rejected up front — a playlist is not a byte stream; those go
//!   through the player-side recorder.
//! * The URL carries credentials: every log line and error string goes through
//!   [`app_core::redact::redact`].

use crate::http::{map_body, HttpClient};
use crate::{NetError, Result};
use app_core::redact::redact;
use futures_util::StreamExt;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Error text returned (as [`NetError::Other`]) for `.m3u8` URLs.
pub const HLS_UNSUPPORTED: &str = "HLS playlists need the player-side recorder";

/// Progress callback: total bytes written in this session.
pub type RecordProgress = Arc<dyn Fn(u64) + Send + Sync>;

const BACKOFF: [Duration; 4] =
    [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(5), Duration::from_secs(10)];
const MAX_CONSECUTIVE_FAILURES: u32 = 6;
const FLUSH_EVERY: u64 = 1024 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_secs(2);
const WRITE_BUF: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct RecordOutcome {
    /// Bytes written in this session (the file may hold more if the job was resumed).
    pub bytes: u64,
    /// Connection attempts beyond the first (successful or not).
    pub reconnects: u32,
    pub ended: RecordEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordEnd {
    /// The deadline passed; the file holds everything received up to it.
    Deadline,
    /// [`RecordControl::stop`] was called; the file is flushed up to that point.
    Stopped,
    /// The job gave up after repeated failures. The join handle resolves to `Err(last_error)` in
    /// that case (the outcome is not returned); callers persist this variant for it.
    Failed,
}

/// Handle to a running recording: stop it, or read the live byte counter.
pub struct RecordControl {
    stop: watch::Sender<bool>,
    pub bytes: Arc<AtomicU64>,
}

impl RecordControl {
    /// Ask the job to stop. Idempotent; a no-op once the job has finished. Dropping the control
    /// without calling this does *not* stop the job — it runs to its deadline.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Bytes written so far in this session.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}

/// Start recording `url` into `path` until `deadline` or [`RecordControl::stop`].
///
/// Returns the control handle and a `JoinHandle` resolving to the outcome. The file is opened in
/// append mode (an interrupted job can be resumed into the same file) and only once the first
/// 2xx response arrives, so a job that never connects leaves no empty file behind.
/// `on_progress(bytes_total)` fires at most every ~2 s. Must be called from within a Tokio runtime.
pub fn start_recording(
    url: String,
    user_agent: Option<String>,
    path: PathBuf,
    deadline: Instant,
    on_progress: RecordProgress,
) -> (RecordControl, JoinHandle<Result<RecordOutcome>>) {
    spawn_recording(RecordOptions::default(), url, user_agent, path, deadline, on_progress)
}

/// Tunables kept private so tests can shrink the backoff; production always uses the defaults.
#[derive(Clone, Copy)]
struct RecordOptions {
    backoff: [Duration; 4],
    max_consecutive_failures: u32,
    flush_every: u64,
    progress_every: Duration,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self {
            backoff: BACKOFF,
            max_consecutive_failures: MAX_CONSECUTIVE_FAILURES,
            flush_every: FLUSH_EVERY,
            progress_every: PROGRESS_EVERY,
        }
    }
}

fn spawn_recording(
    opts: RecordOptions,
    url: String,
    user_agent: Option<String>,
    path: PathBuf,
    deadline: Instant,
    on_progress: RecordProgress,
) -> (RecordControl, JoinHandle<Result<RecordOutcome>>) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let bytes = Arc::new(AtomicU64::new(0));
    let control = RecordControl { stop: stop_tx, bytes: bytes.clone() };
    if is_hls(&url) {
        tracing::warn!(url = %redact(&url), "recorder: HLS playlist rejected");
        let handle: JoinHandle<Result<RecordOutcome>> =
            tokio::spawn(async { Err(NetError::Other(HLS_UNSUPPORTED.into())) });
        return (control, handle);
    }
    let job = RecordJob { opts, url, user_agent, path, deadline, on_progress, stop: stop_rx, bytes };
    (control, tokio::spawn(job.run()))
}

fn is_hls(url: &str) -> bool {
    url::Url::parse(url).map(|u| u.path().to_ascii_lowercase().ends_with(".m3u8")).unwrap_or(false)
}

/// Resolves when a stop is requested. If the control handle was dropped without a stop, the job
/// keeps running to its deadline, so this never resolves in that case.
async fn stopped(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|s| *s).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Throttle helper: the first report always goes out, later ones only after `every`.
pub(crate) fn progress_due(last: Option<Instant>, every: Duration) -> bool {
    match last {
        None => true,
        Some(t) => t.elapsed() >= every,
    }
}

async fn open_append(path: &Path) -> Result<BufWriter<File>> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path).await?;
    Ok(BufWriter::with_capacity(WRITE_BUF, file))
}

struct RecordJob {
    opts: RecordOptions,
    url: String,
    user_agent: Option<String>,
    path: PathBuf,
    deadline: Instant,
    on_progress: RecordProgress,
    stop: watch::Receiver<bool>,
    bytes: Arc<AtomicU64>,
}

impl RecordJob {
    async fn run(self) -> Result<RecordOutcome> {
        let RecordJob { opts, url, user_agent, path, deadline, on_progress, mut stop, bytes } = self;
        let redacted = redact(&url);
        let client = HttpClient::new(user_agent.as_deref())?;
        let deadline_sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(deadline_sleep);

        let mut writer: Option<BufWriter<File>> = None;
        let mut total: u64 = 0;
        let mut unflushed: u64 = 0;
        let mut attempts: u32 = 0;
        let mut reconnects: u32 = 0;
        // Consecutive attempts that delivered no data (the give-up counter).
        let mut failures: u32 = 0;
        // Consecutive connection ends since data last flowed (the backoff step).
        let mut backoff_step: u32 = 0;
        let mut last_err: Option<NetError> = None;
        let mut last_progress: Option<Instant> = None;

        tracing::info!(url = %redacted, path = %path.display(), "recorder: start");

        let ended = 'job: loop {
            if Instant::now() >= deadline {
                break RecordEnd::Deadline;
            }
            if *stop.borrow() {
                break RecordEnd::Stopped;
            }

            if attempts > 0 {
                let idx = (backoff_step.saturating_sub(1) as usize).min(opts.backoff.len() - 1);
                let wait = opts.backoff[idx];
                tracing::info!(wait_ms = wait.as_millis() as u64, attempt = attempts + 1, "recorder: reconnecting");
                tokio::select! {
                    biased;
                    _ = stopped(&mut stop) => break 'job RecordEnd::Stopped,
                    _ = &mut deadline_sleep => break 'job RecordEnd::Deadline,
                    _ = tokio::time::sleep(wait) => {}
                }
                reconnects += 1;
            }
            attempts += 1;

            let resp = tokio::select! {
                biased;
                _ = stopped(&mut stop) => break 'job RecordEnd::Stopped,
                _ = &mut deadline_sleep => break 'job RecordEnd::Deadline,
                r = client.get_stream(&url) => r,
            };
            let mut body = match resp {
                Ok(r) => {
                    tracing::info!(
                        status = r.status,
                        content_type = ?r.content_type,
                        hops = r.redirect_hops,
                        url = %redact(&r.final_url),
                        "recorder: connected"
                    );
                    Box::pin(map_body(r.body))
                }
                Err(e) => {
                    failures += 1;
                    backoff_step += 1;
                    tracing::warn!(attempt = attempts, failures, err = %e, "recorder: connect failed");
                    last_err = Some(e);
                    if failures >= opts.max_consecutive_failures {
                        break RecordEnd::Failed;
                    }
                    continue;
                }
            };

            let mut got_data = false;
            let mut conn_err: Option<NetError> = None;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped(&mut stop) => break 'job RecordEnd::Stopped,
                    _ = &mut deadline_sleep => break 'job RecordEnd::Deadline,
                    next = body.next() => match next {
                        Some(Ok(chunk)) => {
                            if chunk.is_empty() {
                                continue;
                            }
                            let w = match writer.as_mut() {
                                Some(w) => w,
                                None => writer.insert(open_append(&path).await?),
                            };
                            w.write_all(&chunk).await?;
                            let n = chunk.len() as u64;
                            total += n;
                            unflushed += n;
                            bytes.store(total, Ordering::Relaxed);
                            if !got_data {
                                got_data = true;
                                failures = 0;
                                backoff_step = 0;
                            }
                            if unflushed >= opts.flush_every {
                                w.flush().await?;
                                unflushed = 0;
                            }
                            if progress_due(last_progress, opts.progress_every) {
                                last_progress = Some(Instant::now());
                                on_progress(total);
                            }
                        }
                        Some(Err(e)) => {
                            conn_err = Some(e);
                            break;
                        }
                        None => break,
                    }
                }
            }

            backoff_step += 1;
            if got_data {
                match conn_err {
                    Some(e) => {
                        tracing::warn!(err = %e, bytes = total, "recorder: stream error");
                        last_err = Some(e);
                    }
                    None => tracing::info!(bytes = total, "recorder: stream ended before deadline"),
                }
            } else {
                failures += 1;
                let e =
                    conn_err.unwrap_or_else(|| NetError::Http(format!("connection ended without data ({redacted})")));
                tracing::warn!(attempt = attempts, failures, err = %e, "recorder: attempt delivered nothing");
                last_err = Some(e);
                if failures >= opts.max_consecutive_failures {
                    break RecordEnd::Failed;
                }
            }
        };

        if let Some(w) = writer.as_mut() {
            w.flush().await?;
        }

        match ended {
            RecordEnd::Failed => {
                let last = last_err.map(|e| e.to_string()).unwrap_or_else(|| "unknown error".into());
                let msg = redact(&format!(
                    "recording gave up after {failures} consecutive failures \
                     ({total} bytes written, {reconnects} reconnects): {last}"
                ));
                tracing::error!(url = %redacted, "recorder: {msg}");
                Err(NetError::Other(msg))
            }
            ended => {
                tracing::info!(url = %redacted, bytes = total, reconnects, ?ended, "recorder: finished");
                Ok(RecordOutcome { bytes: total, reconnects, ended })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// What the mock does with one accepted connection.
    #[derive(Clone, Copy)]
    enum Act {
        /// `200` and `bytes` of the pattern stream in `chunk`-sized writes (`delay_ms` apart);
        /// then either close, or hold the socket open until the client hangs up.
        Stream { bytes: usize, chunk: usize, delay_ms: u64, hold: bool },
        /// A non-2xx status with a tiny body, then close.
        Status(u16),
    }

    struct Mock {
        queue: Mutex<VecDeque<Act>>,
        fallback: Act,
        /// Pattern position = bytes sent across all connections, so the concatenation of what
        /// every connection streamed is `pattern(0, total)`.
        sent: AtomicU64,
        conns: AtomicU32,
    }

    impl Mock {
        fn new(queue: Vec<Act>, fallback: Act) -> Arc<Self> {
            Arc::new(Self {
                queue: Mutex::new(queue.into()),
                fallback,
                sent: AtomicU64::new(0),
                conns: AtomicU32::new(0),
            })
        }
        fn conns(&self) -> u32 {
            self.conns.load(Ordering::SeqCst)
        }
    }

    fn pattern(from: u64, len: usize) -> Vec<u8> {
        (0..len as u64).map(|i| ((from + i) % 251) as u8).collect()
    }

    async fn read_head(sock: &mut TcpStream) -> bool {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let n = match sock.read(&mut tmp).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => n,
            };
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
                return true;
            }
        }
    }

    /// Minimal HTTP/1.1 responder on 127.0.0.1. Bodies are close-delimited (no Content-Length,
    /// `Connection: close`), like a live MPEG-TS stream.
    async fn serve(mock: Arc<Mock>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mock = mock.clone();
                tokio::spawn(async move {
                    if !read_head(&mut sock).await {
                        return;
                    }
                    mock.conns.fetch_add(1, Ordering::SeqCst);
                    let act = mock.queue.lock().unwrap().pop_front().unwrap_or(mock.fallback);
                    match act {
                        Act::Status(code) => {
                            let body = "nope";
                            let head = format!(
                                "HTTP/1.1 {code} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(body.as_bytes()).await;
                            let _ = sock.shutdown().await;
                        }
                        Act::Stream { bytes, chunk, delay_ms, hold } => {
                            let head = "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nConnection: close\r\n\r\n";
                            if sock.write_all(head.as_bytes()).await.is_err() {
                                return;
                            }
                            let mut left = bytes;
                            while left > 0 {
                                let n = left.min(chunk);
                                let from = mock.sent.fetch_add(n as u64, Ordering::SeqCst);
                                if sock.write_all(&pattern(from, n)).await.is_err() {
                                    return;
                                }
                                left -= n;
                                if delay_ms > 0 {
                                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                                }
                            }
                            if hold {
                                // Stay connected until the client hangs up (bounded for safety).
                                let mut buf = [0u8; 64];
                                let _ = tokio::time::timeout(Duration::from_secs(30), async {
                                    while !matches!(sock.read(&mut buf).await, Ok(0) | Err(_)) {}
                                })
                                .await;
                            } else {
                                let _ = sock.shutdown().await;
                            }
                        }
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    fn fast() -> RecordOptions {
        RecordOptions {
            backoff: [Duration::from_millis(20); 4],
            max_consecutive_failures: 6,
            flush_every: 64 * 1024,
            progress_every: Duration::ZERO,
        }
    }

    type Progress = Arc<Mutex<Vec<u64>>>;

    fn start(
        opts: RecordOptions,
        url: String,
        path: PathBuf,
        deadline: Instant,
    ) -> (RecordControl, JoinHandle<Result<RecordOutcome>>, Progress) {
        let progress: Progress = Arc::new(Mutex::new(Vec::new()));
        let sink = progress.clone();
        let on_progress: RecordProgress = Arc::new(move |b| sink.lock().unwrap().push(b));
        let (control, handle) = spawn_recording(opts, url, None, path, deadline, on_progress);
        (control, handle, progress)
    }

    async fn wait_for_bytes(control: &RecordControl, want: u64) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while control.bytes() < want {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bytes arrived in time");
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[tokio::test]
    async fn reconnects_after_early_close_and_stops_promptly() {
        let mock = Mock::new(
            vec![Act::Stream { bytes: 100_000, chunk: 8192, delay_ms: 0, hold: false }],
            Act::Stream { bytes: 50_000, chunk: 8192, delay_ms: 0, hold: true },
        );
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec").join("job.ts"); // nested: parent dir is created
        let url = format!("{base}/live/u/p/1.ts?password=secret");
        let (control, handle, progress) = start(fast(), url, path.clone(), far());

        wait_for_bytes(&control, 150_000).await;
        let t0 = std::time::Instant::now();
        control.stop();
        let out = handle.await.unwrap().unwrap();
        assert!(t0.elapsed() < Duration::from_secs(1), "stop took {:?}", t0.elapsed());

        assert_eq!(out.ended, RecordEnd::Stopped);
        assert_eq!(out.bytes, 150_000);
        assert_eq!(out.reconnects, 1);
        assert_eq!(control.bytes(), 150_000);
        assert_eq!(mock.conns(), 2);
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len(), 150_000, "file size == streamed bytes");
        assert_eq!(data, pattern(0, 150_000), "chunks appended in order across the reconnect");
        let p = progress.lock().unwrap();
        assert_eq!(p.last().copied(), Some(150_000));
        assert!(p.windows(2).all(|w| w[0] < w[1]), "progress is monotonic");
    }

    #[tokio::test]
    async fn deadline_ends_recording_and_flushes() {
        let mock = Mock::new(vec![], Act::Stream { bytes: 64 << 20, chunk: 4096, delay_ms: 5, hold: true });
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.ts");
        let deadline = Instant::now() + Duration::from_millis(400);
        let t0 = std::time::Instant::now();
        let (_control, handle, _) = start(fast(), format!("{base}/live/u/p/2.ts"), path.clone(), deadline);
        let out = handle.await.unwrap().unwrap();
        assert!(t0.elapsed() < Duration::from_secs(3), "deadline honoured: {:?}", t0.elapsed());

        assert_eq!(out.ended, RecordEnd::Deadline);
        assert_eq!(out.reconnects, 0);
        assert!(out.bytes > 0);
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len() as u64, out.bytes, "everything counted was flushed");
        assert_eq!(data, pattern(0, data.len()));
        assert!(mock.sent.load(Ordering::SeqCst) >= out.bytes);
    }

    #[tokio::test]
    async fn past_deadline_returns_immediately_without_writing() {
        let mock = Mock::new(vec![], Act::Stream { bytes: 1000, chunk: 100, delay_ms: 0, hold: true });
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.ts");
        let deadline = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now);
        let (control, handle, progress) = start(fast(), format!("{base}/live/u/p/3.ts"), path.clone(), deadline);
        let out = tokio::time::timeout(Duration::from_secs(1), handle).await.expect("immediate").unwrap().unwrap();
        assert_eq!((out.bytes, out.reconnects, out.ended), (0, 0, RecordEnd::Deadline));
        assert_eq!(control.bytes(), 0);
        assert!(!path.exists(), "no file is created");
        assert_eq!(mock.conns(), 0, "no connection is made");
        assert!(progress.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn gives_up_after_six_consecutive_failures_with_redacted_error() {
        let mock = Mock::new(vec![], Act::Status(500));
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.ts");
        let url = format!("{base}/get.php?username=bob&password=secret&type=ts");
        let (control, handle, _) = start(fast(), url, path.clone(), far());
        let err = handle.await.unwrap().unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, NetError::Other(_)), "{err:?}");
        assert!(msg.contains("HTTP 500"), "{msg}");
        assert!(msg.contains("6 consecutive failures"), "{msg}");
        assert!(!msg.contains("secret"), "credentials leaked: {msg}");
        assert!(msg.contains("password=***"), "{msg}");
        assert_eq!(mock.conns(), 6);
        assert_eq!(control.bytes(), 0);
        assert!(!path.exists(), "no file is created when nothing was received");
    }

    #[tokio::test]
    async fn failures_reset_once_data_flows() {
        let mock = Mock::new(
            vec![Act::Status(500), Act::Status(503)],
            Act::Stream { bytes: 40_000, chunk: 8192, delay_ms: 0, hold: true },
        );
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.ts");
        let (control, handle, _) = start(fast(), format!("{base}/live/u/p/4.ts"), path.clone(), far());
        wait_for_bytes(&control, 40_000).await;
        control.stop();
        let out = handle.await.unwrap().unwrap();
        assert_eq!(out.ended, RecordEnd::Stopped);
        assert_eq!(out.bytes, 40_000);
        assert_eq!(out.reconnects, 2);
        assert_eq!(mock.conns(), 3);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 40_000);
    }

    #[tokio::test]
    async fn appends_to_an_existing_file() {
        let mock = Mock::new(vec![], Act::Stream { bytes: 20_000, chunk: 8192, delay_ms: 0, hold: true });
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.ts");
        std::fs::write(&path, b"0123456789").unwrap();
        let (control, handle, _) = start(fast(), format!("{base}/live/u/p/5.ts"), path.clone(), far());
        wait_for_bytes(&control, 20_000).await;
        control.stop();
        let out = handle.await.unwrap().unwrap();
        assert_eq!(out.bytes, 20_000, "session bytes exclude the pre-existing prefix");
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len(), 20_010);
        assert_eq!(&data[..10], b"0123456789");
        assert_eq!(&data[10..], &pattern(0, 20_000)[..]);
    }

    #[tokio::test]
    async fn hls_playlists_are_rejected_up_front() {
        let mock = Mock::new(vec![], Act::Stream { bytes: 1000, chunk: 100, delay_ms: 0, hold: true });
        let base = serve(mock.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.ts");
        for url in [format!("{base}/live/u/p/6.M3U8"), format!("{base}/hls/playlist.m3u8?token=abc")] {
            let (_control, handle, _) = start(fast(), url, path.clone(), far());
            let err =
                tokio::time::timeout(Duration::from_secs(1), handle).await.expect("immediate").unwrap().unwrap_err();
            assert!(matches!(&err, NetError::Other(m) if m == HLS_UNSUPPORTED), "{err:?}");
        }
        assert_eq!(mock.conns(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn hls_detection_looks_at_the_path_only() {
        assert!(is_hls("http://h/live/u/p/1.m3u8"));
        assert!(is_hls("http://h/x.M3U8?password=p"));
        assert!(!is_hls("http://h/live/u/p/1.ts?next=a.m3u8"));
        assert!(!is_hls("not a url"));
    }

    #[test]
    fn record_end_serialises_snake_case() {
        let out = RecordOutcome { bytes: 1, reconnects: 2, ended: RecordEnd::Deadline };
        assert_eq!(serde_json::to_string(&out).unwrap(), r#"{"bytes":1,"reconnects":2,"ended":"deadline"}"#);
    }
}
