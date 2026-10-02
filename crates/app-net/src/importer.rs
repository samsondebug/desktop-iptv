//! Streaming M3U importer: URL or file → preflight → parser → 5k-row SQLite chunks.
//!
//! Memory profile is O(chunk) regardless of playlist size. Progress is reported through a
//! [`ProgressSink`] so the UI can show "Importing 34%" without blocking first paint.

use crate::http::{map_body, HttpClient};
use crate::m3u::{M3uEntry, M3uParser};
use crate::preflight::{self, SNIFF_BYTES};
use crate::{NetError, Result};
use app_core::{ImportProgressEvent, SyncStats};
use app_db::channels::ChannelInsert;
use app_db::Db;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncReadExt;

/// Where a playlist comes from.
#[derive(Debug, Clone)]
pub enum ImportSource {
    Url { url: String, user_agent: Option<String> },
    File { path: std::path::PathBuf },
}

/// Progress callback. Implementations must be cheap and non-blocking (they run on the
/// import task). The Tauri layer forwards these as `import_progress` events.
pub trait ProgressSink: Send + Sync {
    fn progress(&self, ev: ImportProgressEvent);
}

impl<F: Fn(ImportProgressEvent) + Send + Sync> ProgressSink for F {
    fn progress(&self, ev: ImportProgressEvent) {
        self(ev)
    }
}

pub struct NoopSink;
impl ProgressSink for NoopSink {
    fn progress(&self, _: ImportProgressEvent) {}
}

/// Rows buffered before handing to SQLite. Equals the max-rows-per-transaction rule.
const BATCH: usize = app_db::MAX_ROWS_PER_TX;

pub type ByteStream = std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<bytes::Bytes>> + Send>>;

/// An opened source: a byte stream plus what we know about it.
pub struct OpenedSource {
    pub stream: ByteStream,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub final_url_redacted: Option<String>,
    pub status: Option<u16>,
}

/// Open a URL (streamed GET, custom UA) or a local file as a byte stream. Shared by the M3U and
/// XMLTV importers so every source goes through the same client, timeouts and redaction.
pub async fn open_source(source: &ImportSource) -> Result<OpenedSource> {
    match source {
        ImportSource::Url { url, user_agent } => {
            let client = HttpClient::new(user_agent.as_deref())?.with_kind("playlist");
            let resp = client.get_stream(url).await?;
            tracing::info!(
                status = resp.status,
                content_type = ?resp.content_type,
                len = ?resp.content_length,
                hops = resp.redirect_hops,
                url = %app_core::redact::redact(&resp.final_url),
                "source GET"
            );
            Ok(OpenedSource {
                stream: Box::pin(map_body(resp.body)),
                content_type: resp.content_type,
                content_length: resp.content_length,
                final_url_redacted: Some(app_core::redact::redact(&resp.final_url)),
                status: Some(resp.status),
            })
        }
        ImportSource::File { path } => {
            let file = tokio::fs::File::open(path).await?;
            let len = file.metadata().await.ok().map(|m| m.len());
            Ok(OpenedSource {
                stream: Box::pin(tokio_file_stream(file)),
                content_type: None,
                content_length: len,
                final_url_redacted: None,
                status: None,
            })
        }
    }
}

/// Import (or refresh) an M3U playlist into `playlist_id`.
pub async fn import_m3u(
    db: Arc<Db>,
    playlist_id: i64,
    source: ImportSource,
    sink: Arc<dyn ProgressSink>,
) -> Result<SyncStats> {
    let started = Instant::now();
    let mut stats = SyncStats::default();
    let emit = |stage: &str, channels: u64, bytes: u64, message: Option<String>| {
        sink.progress(ImportProgressEvent { playlist_id, stage: stage.into(), channels, bytes, message });
    };
    emit("fetching", 0, 0, None);

    // ---- open the byte stream --------------------------------------------------------
    let opened = open_source(&source).await?;
    let (mut stream, content_type, total_len) = (opened.stream, opened.content_type, opened.content_length);

    // ---- preflight on the first 512 bytes --------------------------------------------
    let mut head: Vec<u8> = Vec::with_capacity(SNIFF_BYTES);
    let mut first_chunks: Vec<bytes::Bytes> = Vec::new();
    while head.len() < SNIFF_BYTES {
        match stream.next().await {
            Some(chunk) => {
                let chunk = chunk?;
                head.extend_from_slice(&chunk[..chunk.len().min(SNIFF_BYTES - head.len())]);
                first_chunks.push(chunk);
            }
            None => break,
        }
    }
    if let Err(e) = preflight::check(&head, content_type.as_deref()) {
        emit("error", 0, head.len() as u64, Some(e.message.clone()));
        return Err(NetError::Preflight(e));
    }

    // ---- stream → parser → db --------------------------------------------------------
    emit("parsing", 0, 0, None);
    let mut parser = M3uParser::new();
    let mut entries: Vec<M3uEntry> = Vec::with_capacity(BATCH);
    let mut ingest = Ingest::new(&db, playlist_id);
    let mut bytes_seen = 0u64;
    let mut last_emit = Instant::now();

    for chunk in first_chunks.drain(..) {
        bytes_seen += chunk.len() as u64;
        parser.feed(&chunk, &mut entries);
        ingest.handle(&mut entries, &mut stats)?;
    }
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes_seen += chunk.len() as u64;
        parser.feed(&chunk, &mut entries);
        ingest.handle(&mut entries, &mut stats)?;
        if last_emit.elapsed().as_millis() > 150 {
            let pct = total_len.map(|t| format!("{}%", (bytes_seen * 100 / t.max(1)).min(99)));
            emit("indexing", ingest.channels_seen, bytes_seen, pct);
            last_emit = Instant::now();
        }
        // Give the runtime a chance to breathe on huge lists.
        tokio::task::yield_now().await;
    }
    parser.finish(&mut entries);
    ingest.handle(&mut entries, &mut stats)?;
    ingest.flush(&mut stats)?;
    let _ = db.checkpoint();

    stats.groups = ingest.groups.len() as u64;
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    if parser.stats.entries_without_extinf > 0 {
        stats
            .warnings
            .push(format!("{} entries had no #EXTINF line (named from URL)", parser.stats.entries_without_extinf));
    }
    if let Some(tvg) = &parser.header().tvg_url {
        stats.warnings.push(format!("playlist advertises an EPG at {}", app_core::redact::redact(tvg)));
        stats.epg_url = Some(tvg.clone());
    }
    if ingest.channels_seen == 0 {
        stats.warnings.push("no channels found".into());
    }
    tracing::info!(?stats, "import complete");
    emit("done", ingest.channels_seen, bytes_seen, None);
    Ok(stats)
}

/// Entry → row conversion, dedupe and batched upserts.
struct Ingest<'a> {
    db: &'a Db,
    playlist_id: i64,
    batch: Vec<ChannelInsert>,
    seen_ids: HashSet<String>,
    groups: HashSet<String>,
    channels_seen: u64,
}

impl<'a> Ingest<'a> {
    fn new(db: &'a Db, playlist_id: i64) -> Self {
        Self { db, playlist_id, batch: Vec::new(), seen_ids: HashSet::new(), groups: HashSet::new(), channels_seen: 0 }
    }

    fn flush(&mut self, stats: &mut SyncStats) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let (ins, upd) = self.db.upsert_channels(self.playlist_id, &self.batch)?;
        stats.inserted += ins;
        stats.updated += upd;
        self.batch.clear();
        Ok(())
    }

    fn handle(&mut self, entries: &mut Vec<M3uEntry>, stats: &mut SyncStats) -> Result<()> {
        for e in entries.drain(..) {
            if e.url.is_empty() {
                stats.skipped += 1;
                continue;
            }
            let mut source_id = derive_source_id(&e);
            // Guarantee uniqueness within this playlist without losing rows.
            if !self.seen_ids.insert(source_id.clone()) {
                let mut n = 2;
                loop {
                    let candidate = format!("{source_id}#{n}");
                    if self.seen_ids.insert(candidate.clone()) {
                        source_id = candidate;
                        break;
                    }
                    n += 1;
                }
            }
            if let Some(g) = &e.group_title {
                if !self.groups.contains(g) {
                    self.groups.insert(g.clone());
                }
            }
            self.batch.push(ChannelInsert {
                source_id,
                name: e.title,
                group_title: e.group_title,
                logo: e.tvg_logo,
                stream_url: e.url,
                tvg_id: e.tvg_id,
                tvg_name: e.tvg_name,
                catchup: e.catchup,
                catchup_days: e.catchup_days,
                catchup_kind: e.catchup_kind,
                catchup_source: e.catchup_source,
            });
            self.channels_seen += 1;
            if self.batch.len() >= BATCH {
                self.flush(stats)?;
            }
        }
        Ok(())
    }
}

/// Stable per-playlist identity for an M3U entry. Prefer explicit ids; fall back to the URL.
fn derive_source_id(e: &M3uEntry) -> String {
    if let Some(id) = &e.tvg_id {
        // tvg-id is an EPG key and is often shared by SD/HD variants → combine with URL hash.
        return format!("{}|{}", id, short_hash(&e.url));
    }
    format!("url|{}", short_hash(&e.url))
}

fn short_hash(s: &str) -> String {
    // FNV-1a 64 — plenty for uniqueness within one playlist, no extra dependency.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn tokio_file_stream(file: tokio::fs::File) -> impl futures_util::Stream<Item = Result<bytes::Bytes>> + Send {
    futures_util::stream::unfold(file, |mut f| async move {
        let mut buf = vec![0u8; 256 * 1024];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(bytes::Bytes::from(buf)), f))
            }
            Err(e) => Some((Err(NetError::from(e)), f)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_db::channels::PlaylistInsert;

    fn new_pl(db: &Db) -> i64 {
        db.insert_playlist(&PlaylistInsert {
            r#type: "m3u".into(),
            name: "t".into(),
            base_url: "file".into(),
            user: None,
            pass: None,
            mac: None,
            ua: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn imports_file_and_refreshes_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("list.m3u");
        let mut s = String::from("#EXTM3U\n");
        for i in 0..12_345 {
            s.push_str(&format!(
                "#EXTINF:-1 tvg-id=\"c{i}\" group-title=\"G{}\",Channel {i}\nhttp://h/live/u/p/{i}.ts\n",
                i % 7
            ));
        }
        // duplicate URL + same tvg-id → must still import as a distinct row
        s.push_str("#EXTINF:-1 tvg-id=\"c1\",Dup of 1\nhttp://h/live/u/p/1.ts\n");
        std::fs::write(&path, s).unwrap();

        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        let stats =
            import_m3u(db.clone(), p, ImportSource::File { path: path.clone() }, Arc::new(NoopSink)).await.unwrap();
        assert_eq!(stats.inserted, 12_346);
        assert_eq!(stats.groups, 7);
        assert_eq!(db.channel_count(p, None).unwrap(), 12_346);

        let stats = import_m3u(db.clone(), p, ImportSource::File { path }, Arc::new(NoopSink)).await.unwrap();
        assert_eq!(stats.inserted, 0);
        assert_eq!(stats.updated, 12_346);
        assert_eq!(db.channel_count(p, None).unwrap(), 12_346, "refresh is idempotent");
    }

    #[tokio::test]
    async fn rejects_html() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("login.m3u");
        std::fs::write(&path, "<!DOCTYPE html><html><body>Login password=secret</body></html>").unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        let err = import_m3u(db.clone(), p, ImportSource::File { path }, Arc::new(NoopSink)).await.unwrap_err();
        match err {
            NetError::Preflight(e) => {
                assert_eq!(e.kind, preflight::PayloadKind::Html);
                assert!(!e.preview.contains("secret"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(db.channel_count(p, None).unwrap(), 0);
    }
}
