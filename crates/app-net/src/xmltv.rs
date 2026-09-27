//! Streaming XMLTV (EPG) importer: URL or file → sniff → (gunzip) → quick-xml events → 5k-row
//! SQLite chunks.
//!
//! Memory profile is O(chunk) regardless of guide size: the byte stream is pulled through a
//! bounded [`BufReader`], quick-xml reuses one event buffer, and programmes are flushed to
//! `epg_programmes` every [`BATCH`] rows. Nothing about the document is ever held whole.
//!
//! Progress is reported through a [`ProgressSink`] using the same stages as the M3U importer
//! (`fetching` → `parsing` → `indexing` → `done` | `error`); `channels` carries the number of
//! programmes seen so far and `message` a percent string when the source length is known.

use crate::importer::{open_source, ImportSource, ProgressSink};
use crate::preflight::{self, PayloadKind, PreflightError, SNIFF_BYTES};
use crate::{NetError, Result};
use app_core::redact::redact;
use app_core::{ImportProgressEvent, SyncStats};
use app_db::epg::ProgrammeInsert;
use app_db::Db;
use async_compression::tokio::bufread::GzipDecoder;
use futures_util::TryStreamExt;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use std::collections::HashSet;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncReadExt, BufReader, Chain};
use tokio_util::io::StreamReader;

/// Rows buffered before handing to SQLite. Equals the max-rows-per-transaction rule.
const BATCH: usize = app_db::MAX_ROWS_PER_TX;
/// Upper bound on the `<channel>` list returned to the caller (guides with 100k+ channels exist).
pub const MAX_CHANNELS: usize = 50_000;
/// Display names kept per channel.
const MAX_DISPLAY_NAMES: usize = 8;
/// Longest text we keep for one title/desc — anything beyond is truncated at a char boundary.
const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Read-side buffer between the byte stream / gzip decoder and the XML parser.
const READ_BUF: usize = 256 * 1024;

/// Options for one XMLTV import pass.
#[derive(Debug, Clone)]
pub struct XmltvOptions {
    /// Programmes that ended before this unix time are skipped and pruned (default: now − 12 h).
    pub prune_before: i64,
    /// Programmes starting after this unix time are skipped (default: now + 14 d).
    pub ignore_after: i64,
}

impl Default for XmltvOptions {
    fn default() -> Self {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        Self { prune_before: now - 12 * 3600, ignore_after: now + 14 * 86_400 }
    }
}

/// A `<channel>` declaration from the guide (id → names/icon). Used for tvg-id matching UIs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XmltvChannel {
    pub id: String,
    pub display_names: Vec<String>,
    pub icon: Option<String>,
}

/// Stream an XMLTV document (plain or gzip) into `epg_programmes` for `playlist_id`.
pub async fn import_xmltv(
    db: Arc<Db>,
    playlist_id: i64,
    source: ImportSource,
    sink: Arc<dyn ProgressSink>,
    opts: XmltvOptions,
) -> Result<SyncStats> {
    import_xmltv_with_channels(db, playlist_id, source, sink, opts).await.map(|(stats, _)| stats)
}

/// Same as [`import_xmltv`] but also returns the `<channel>` declarations seen (capped at
/// [`MAX_CHANNELS`]).
pub async fn import_xmltv_with_channels(
    db: Arc<Db>,
    playlist_id: i64,
    source: ImportSource,
    sink: Arc<dyn ProgressSink>,
    opts: XmltvOptions,
) -> Result<(SyncStats, Vec<XmltvChannel>)> {
    let emit = |stage: &str, programmes: u64, bytes: u64, message: Option<String>| {
        sink.progress(ImportProgressEvent { playlist_id, stage: stage.into(), channels: programmes, bytes, message });
    };
    emit("fetching", 0, 0, None);
    match run(&db, playlist_id, &source, &emit, &opts).await {
        Ok(out) => Ok(out),
        Err(e) => {
            let msg = redact(&e.to_string());
            tracing::warn!(playlist_id, error = %msg, "xmltv import failed");
            emit("error", 0, 0, Some(msg));
            Err(e)
        }
    }
}

type Emit<'a> = dyn Fn(&str, u64, u64, Option<String>) + Sync + 'a;

async fn run(
    db: &Db,
    playlist_id: i64,
    source: &ImportSource,
    emit: &Emit<'_>,
    opts: &XmltvOptions,
) -> Result<(SyncStats, Vec<XmltvChannel>)> {
    let started = Instant::now();
    let mut stats = SyncStats::default();

    // ---- open the byte stream, count raw bytes for the percent display ------------------
    let opened = open_source(source).await?;
    let content_type = opened.content_type;
    let total_len = opened.content_length;
    let raw_bytes = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&raw_bytes);
    let stream = opened
        .stream
        .inspect_ok(move |b| {
            counter.fetch_add(b.len() as u64, Ordering::Relaxed);
        })
        .map_err(std::io::Error::other);
    let raw = StreamReader::new(stream);

    // ---- sniff: gzip magic first, then HTML / XML on the (decoded) head -----------------
    let (head, raw) = read_head(raw, SNIFF_BYTES).await?;
    let gzip = is_gzip(&head);
    let (head, body): (Vec<u8>, Box<dyn AsyncRead + Send + Unpin>) = if gzip {
        let mut dec = GzipDecoder::new(BufReader::with_capacity(READ_BUF, raw));
        dec.multiple_members(true);
        let (head, dec) = read_head(dec, SNIFF_BYTES).await?;
        (head, Box::new(dec))
    } else {
        (head, Box::new(raw))
    };
    classify(&head, content_type.as_deref())?;
    if gzip {
        stats.warnings.push("source was gzip-compressed (decoded on the fly)".into());
    }
    emit("parsing", 0, raw_bytes.load(Ordering::Relaxed), None);

    // ---- stream → events → rows ----------------------------------------------------------
    let mut reader = Reader::from_reader(BufReader::with_capacity(READ_BUF, body));
    {
        let cfg = reader.config_mut();
        // Guides with a raw "&" in a title are common; do not abort the whole import on them.
        cfg.allow_dangling_amp = true;
        cfg.check_comments = false;
    }
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut parser = XmltvParser::default();
    let mut ingest = Ingest::new(db, playlist_id, opts);
    let mut channels: Vec<XmltvChannel> = Vec::new();
    let mut channels_declared = 0u64;
    let mut last_emit = Instant::now();
    // Programmes since the last progress check; avoids an `Instant::now()` per event.
    let mut since_check = 0u32;

    loop {
        let ev = reader.read_event_into_async(&mut buf).await.map_err(|e| map_xml_error(e, reader.error_position()))?;
        let emitted = match ev {
            Event::Eof => break,
            Event::Start(e) => parser.start(&e, false),
            Event::Empty(e) => parser.start(&e, true),
            Event::End(_) => parser.end(),
            Event::Text(t) => {
                parser.text(&t);
                None
            }
            Event::CData(c) => {
                parser.text(&c);
                None
            }
            Event::GeneralRef(r) => {
                parser.general_ref(&r);
                None
            }
            _ => None,
        };
        buf.clear();
        match emitted {
            Some(Emitted::Programme(p)) => {
                ingest.handle(p, &mut stats)?;
                since_check += 1;
                if since_check >= 512 {
                    since_check = 0;
                    if last_emit.elapsed().as_millis() <= 150 {
                        continue;
                    }
                    let bytes = raw_bytes.load(Ordering::Relaxed);
                    let pct = total_len.map(|t| format!("{}%", (bytes * 100 / t.max(1)).min(99)));
                    emit("indexing", ingest.seen, bytes, pct);
                    last_emit = Instant::now();
                    // Give the runtime a chance to breathe on huge guides.
                    tokio::task::yield_now().await;
                }
            }
            Some(Emitted::Channel(c)) => {
                channels_declared += 1;
                if channels.len() < MAX_CHANNELS {
                    channels.push(c);
                }
            }
            None => {}
        }
    }
    ingest.flush(&mut stats)?;
    let pruned = db.prune_programmes(playlist_id, opts.prune_before)?;
    let _ = db.checkpoint();

    // ---- stats / warnings ------------------------------------------------------------------
    stats.groups = ingest.channel_ids.len() as u64;
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    if ingest.bad_times > 0 {
        stats.warnings.push(format!("{} programmes had unparsable times", ingest.bad_times));
    }
    if ingest.inverted > 0 {
        stats.warnings.push(format!("{} programmes had stop <= start", ingest.inverted));
    }
    if ingest.incomplete > 0 {
        stats.warnings.push(format!("{} programmes had no channel or title", ingest.incomplete));
    }
    if ingest.outside_window > 0 {
        stats.warnings.push(format!("{} programmes were outside the import window", ingest.outside_window));
    }
    let silent = channels.iter().filter(|c| !ingest.channel_ids.contains(&c.id)).count();
    if silent > 0 {
        stats.warnings.push(format!("{silent} channels declared but no programmes"));
    }
    if channels_declared as usize > MAX_CHANNELS {
        stats.warnings.push(format!("channel list truncated to {MAX_CHANNELS} of {channels_declared} declared"));
    }
    if pruned > 0 {
        stats.warnings.push(format!("pruned {pruned} programmes that ended before the retention window"));
    }
    if ingest.seen == 0 {
        stats.warnings.push("no programmes found".into());
    }
    let bytes = raw_bytes.load(Ordering::Relaxed);
    tracing::info!(?stats, channels = channels.len(), bytes, "xmltv import complete");
    emit("done", ingest.seen, bytes, None);
    Ok((stats, channels))
}

// ---------------------------------------------------------------------------------------------
// source sniffing
// ---------------------------------------------------------------------------------------------

/// Pull up to `n` bytes off `r` without losing them: returns the head and a reader that replays
/// it in front of the rest.
async fn read_head<R: AsyncRead + Unpin>(mut r: R, n: usize) -> Result<(Vec<u8>, Chain<Cursor<Vec<u8>>, R>)> {
    let mut head = Vec::with_capacity(n);
    let mut scratch = vec![0u8; n];
    while head.len() < n {
        let got = r.read(&mut scratch[..n - head.len()]).await?;
        if got == 0 {
            break;
        }
        head.extend_from_slice(&scratch[..got]);
    }
    let replay = Cursor::new(head.clone()).chain(r);
    Ok((head, replay))
}

fn is_gzip(head: &[u8]) -> bool {
    head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b
}

/// Accept XML, refuse HTML with a preflight diagnostic and anything else with a parse error.
fn classify(head: &[u8], content_type: Option<&str>) -> Result<()> {
    let kind = preflight::sniff(head, content_type);
    let preview = preflight::preview_of(head);
    match kind {
        PayloadKind::Xmltv => Ok(()),
        PayloadKind::Html => Err(NetError::Preflight(PreflightError {
            kind,
            message: "The URL returned a web page (HTML), not an XMLTV guide. Usually a login/portal page, an \
                      expired account, or a wrong path. Open the URL in a browser to see what the provider is saying."
                .into(),
            preview,
        })),
        PayloadKind::Empty => Err(NetError::Parse("the source is empty, not an XMLTV guide".into())),
        PayloadKind::M3u | PayloadKind::BareUrlList => {
            Err(NetError::Parse(format!("the source is an M3U playlist, not an XMLTV guide (starts with: {preview})")))
        }
        PayloadKind::Json => {
            Err(NetError::Parse(format!("the source is JSON, not an XMLTV guide (starts with: {preview})")))
        }
        PayloadKind::Unknown => {
            let lower = String::from_utf8_lossy(preflight::strip_bom(head)).trim_start().to_ascii_lowercase();
            if lower.starts_with("<?xml") || lower.starts_with("<tv") {
                Ok(())
            } else {
                Err(NetError::Parse(format!("the source is not an XMLTV guide (starts with: {preview})")))
            }
        }
    }
}

fn map_xml_error(e: quick_xml::Error, position: u64) -> NetError {
    match e {
        quick_xml::Error::Io(io) => NetError::Io(redact(&io.to_string())),
        other => NetError::Parse(redact(&format!("XMLTV syntax error at byte {position}: {other}"))),
    }
}

// ---------------------------------------------------------------------------------------------
// event → element state machine
// ---------------------------------------------------------------------------------------------

/// A `<programme>` as read from the document, before validation.
#[derive(Debug, Default)]
struct RawProgramme {
    channel: Option<String>,
    start: Option<String>,
    stop: Option<String>,
    title: Option<String>,
    sub_title: Option<String>,
    desc: Option<String>,
}

enum Emitted {
    Programme(RawProgramme),
    Channel(XmltvChannel),
}

#[derive(Default)]
enum Ctx {
    #[default]
    Outside,
    Channel(XmltvChannel),
    Programme(RawProgramme),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Field {
    #[default]
    None,
    Title,
    SubTitle,
    Desc,
    DisplayName,
}

/// Tracks where we are in the document. Only `<channel>` and `<programme>` (and the handful of
/// children we care about) are interpreted; everything else is skipped by depth counting.
#[derive(Default)]
struct XmltvParser {
    ctx: Ctx,
    /// Open elements inside the current `<channel>`/`<programme>` (0 = directly inside it).
    depth: u32,
    /// Which text field, if any, we are accumulating into `text`.
    field: Field,
    text: String,
    truncated: bool,
}

impl XmltvParser {
    fn start(&mut self, e: &BytesStart<'_>, empty: bool) -> Option<Emitted> {
        let name = e.local_name();
        let name = name.as_ref();
        match &mut self.ctx {
            Ctx::Outside => match name {
                "channel" => {
                    let ch = XmltvChannel { id: attr(e, "id").unwrap_or_default(), ..Default::default() };
                    if empty {
                        return Some(Emitted::Channel(ch));
                    }
                    self.enter(Ctx::Channel(ch));
                }
                "programme" => {
                    let p = RawProgramme {
                        channel: attr(e, "channel"),
                        start: attr(e, "start"),
                        stop: attr(e, "stop"),
                        ..Default::default()
                    };
                    if empty {
                        return Some(Emitted::Programme(p));
                    }
                    self.enter(Ctx::Programme(p));
                }
                _ => {}
            },
            Ctx::Channel(ch) => {
                if self.depth == 0 {
                    match name {
                        "display-name" if !empty => self.capture(Field::DisplayName),
                        "icon" if ch.icon.is_none() => ch.icon = attr(e, "src"),
                        _ => {}
                    }
                }
                if !empty {
                    self.depth += 1;
                }
            }
            Ctx::Programme(p) => {
                if self.depth == 0 && !empty {
                    match name {
                        "title" if p.title.is_none() => self.capture(Field::Title),
                        "sub-title" if p.sub_title.is_none() => self.capture(Field::SubTitle),
                        "desc" if p.desc.is_none() => self.capture(Field::Desc),
                        _ => {}
                    }
                }
                if !empty {
                    self.depth += 1;
                }
            }
        }
        None
    }

    fn end(&mut self) -> Option<Emitted> {
        if matches!(self.ctx, Ctx::Outside) {
            return None;
        }
        if self.depth == 0 {
            // Closing the <channel>/<programme> itself.
            self.field = Field::None;
            self.text.clear();
            return match std::mem::take(&mut self.ctx) {
                Ctx::Channel(ch) => Some(Emitted::Channel(ch)),
                Ctx::Programme(p) => Some(Emitted::Programme(p)),
                Ctx::Outside => None,
            };
        }
        self.depth -= 1;
        if self.depth == 0 && self.field != Field::None {
            let value = self.text.trim().to_string();
            let field = std::mem::take(&mut self.field);
            self.text.clear();
            self.truncated = false;
            if !value.is_empty() {
                match (&mut self.ctx, field) {
                    (Ctx::Channel(ch), Field::DisplayName) if ch.display_names.len() < MAX_DISPLAY_NAMES => {
                        ch.display_names.push(value);
                    }
                    (Ctx::Programme(p), Field::Title) => p.title = Some(value),
                    (Ctx::Programme(p), Field::SubTitle) => p.sub_title = Some(value),
                    (Ctx::Programme(p), Field::Desc) => p.desc = Some(value),
                    _ => {}
                }
            }
        }
        None
    }

    fn text(&mut self, s: &str) {
        if self.field == Field::None || self.truncated {
            return;
        }
        let room = MAX_TEXT_BYTES.saturating_sub(self.text.len());
        if s.len() <= room {
            self.text.push_str(s);
        } else {
            let mut cut = room;
            while cut > 0 && !s.is_char_boundary(cut) {
                cut -= 1;
            }
            self.text.push_str(&s[..cut]);
            self.truncated = true;
        }
    }

    fn general_ref(&mut self, r: &BytesRef<'_>) {
        if self.field == Field::None {
            return;
        }
        if let Ok(Some(c)) = r.resolve_char_ref() {
            let mut tmp = [0u8; 4];
            self.text(c.encode_utf8(&mut tmp));
            return;
        }
        let name: &str = r;
        match quick_xml::escape::resolve_predefined_entity(name).or_else(|| resolve_html_entity(name)) {
            Some(s) => self.text(s),
            // Unknown entity: keep it literally rather than silently dropping characters.
            None => {
                let literal = format!("&{name};");
                self.text(&literal);
            }
        }
    }

    fn enter(&mut self, ctx: Ctx) {
        self.ctx = ctx;
        self.depth = 0;
        self.field = Field::None;
        self.text.clear();
        self.truncated = false;
    }

    fn capture(&mut self, field: Field) {
        self.field = field;
        self.text.clear();
        self.truncated = false;
    }
}

/// The few HTML entities that show up in real-world guides beyond the XML predefined five.
fn resolve_html_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "nbsp" => "\u{a0}",
        "hellip" => "\u{2026}",
        "ndash" => "\u{2013}",
        "mdash" => "\u{2014}",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201c}",
        "rdquo" => "\u{201d}",
        "copy" => "\u{a9}",
        "reg" => "\u{ae}",
        "trade" => "\u{2122}",
        "eacute" => "\u{e9}",
        _ => return None,
    })
}

/// Unescaped, trimmed attribute value; `None` when absent or blank.
fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------------------------------------
// row conversion + batched upserts
// ---------------------------------------------------------------------------------------------

struct Ingest<'a> {
    db: &'a Db,
    playlist_id: i64,
    opts: &'a XmltvOptions,
    batch: Vec<ProgrammeInsert>,
    channel_ids: HashSet<String>,
    seen: u64,
    bad_times: u64,
    inverted: u64,
    incomplete: u64,
    outside_window: u64,
}

impl<'a> Ingest<'a> {
    fn new(db: &'a Db, playlist_id: i64, opts: &'a XmltvOptions) -> Self {
        Self {
            db,
            playlist_id,
            opts,
            batch: Vec::with_capacity(BATCH),
            channel_ids: HashSet::new(),
            seen: 0,
            bad_times: 0,
            inverted: 0,
            incomplete: 0,
            outside_window: 0,
        }
    }

    fn handle(&mut self, p: RawProgramme, stats: &mut SyncStats) -> Result<()> {
        self.seen += 1;
        let (Some(channel), Some(title)) = (p.channel, p.title) else {
            self.incomplete += 1;
            stats.skipped += 1;
            return Ok(());
        };
        let (Some(start), Some(stop)) =
            (p.start.as_deref().and_then(parse_xmltv_time), p.stop.as_deref().and_then(parse_xmltv_time))
        else {
            self.bad_times += 1;
            stats.skipped += 1;
            return Ok(());
        };
        if stop <= start {
            self.inverted += 1;
            stats.skipped += 1;
            return Ok(());
        }
        if stop < self.opts.prune_before || start > self.opts.ignore_after {
            self.outside_window += 1;
            stats.skipped += 1;
            return Ok(());
        }
        let title = match p.sub_title {
            Some(sub) => format!("{title} — {sub}"),
            None => title,
        };
        if !self.channel_ids.contains(&channel) {
            self.channel_ids.insert(channel.clone());
        }
        self.batch.push(ProgrammeInsert { channel_tvg_id: channel, start, stop, title, desc: p.desc });
        if self.batch.len() >= BATCH {
            self.flush(stats)?;
        }
        Ok(())
    }

    fn flush(&mut self, stats: &mut SyncStats) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        stats.inserted += self.db.upsert_programmes(self.playlist_id, &self.batch)?;
        self.batch.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// time parsing (dependency-free)
// ---------------------------------------------------------------------------------------------

/// Parse an XMLTV timestamp (`YYYYMMDDHHMMSS [+|-]HHMM`) to unix seconds.
///
/// Tolerates missing seconds / minutes / hours (padded with zeros), a missing zone (UTC),
/// `Z`/`UTC`/`GMT` zones, a `+HH:MM` colon, surrounding whitespace, and a zone glued to the
/// digits. Anything shorter than a full date, or with out-of-range fields, is `None`.
pub fn parse_xmltv_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let digits_len = s.bytes().take_while(u8::is_ascii_digit).count();
    if !(8..=14).contains(&digits_len) {
        return None;
    }
    let (digits, rest) = s.split_at(digits_len);
    let field = |from: usize, to: usize, default: i64| -> Option<i64> {
        if digits.len() < to {
            // Partial field (odd digit count) is padded with zeros to the right.
            if digits.len() > from {
                let mut v = digits[from..].parse::<i64>().ok()?;
                for _ in digits.len()..to {
                    v *= 10;
                }
                return Some(v);
            }
            return Some(default);
        }
        digits[from..to].parse::<i64>().ok()
    };
    let year = field(0, 4, 0)?;
    let month = field(4, 6, 1)?;
    let day = field(6, 8, 1)?;
    let hour = field(8, 10, 0)?;
    let minute = field(10, 12, 0)?;
    let second = field(12, 14, 0)?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month as u32) as i64
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let offset = parse_zone(rest.trim())?;
    let days = days_from_civil(year, month as u32, day as u32);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

/// Zone suffix → offset in seconds east of UTC. Empty means UTC.
fn parse_zone(z: &str) -> Option<i64> {
    if z.is_empty() || z.eq_ignore_ascii_case("z") || z.eq_ignore_ascii_case("utc") || z.eq_ignore_ascii_case("gmt") {
        return Some(0);
    }
    if !z.is_ascii() {
        return None;
    }
    let (sign, body) = match z.as_bytes()[0] {
        b'+' => (1, &z[1..]),
        b'-' => (-1, &z[1..]),
        _ => return None,
    };
    let body = body.trim();
    let (hh, mm) = match body.split_once(':') {
        Some((h, m)) => (h, m),
        None => match body.len() {
            4 => body.split_at(2),
            2 => (body, "00"),
            _ => return None,
        },
    };
    if hh.len() != 2
        || mm.len() != 2
        || !hh.bytes().all(|b| b.is_ascii_digit())
        || !mm.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let (h, m) = (hh.parse::<i64>().ok()?, mm.parse::<i64>().ok()?);
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * 3_600 + m * 60))
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importer::NoopSink;
    use app_db::channels::PlaylistInsert;
    use std::io::Write;
    use std::sync::Mutex;

    // 2026-09-27T12:00:00Z
    const NOON: i64 = 1_790_510_400;

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

    fn all_time() -> XmltvOptions {
        XmltvOptions { prune_before: 0, ignore_after: i64::MAX }
    }

    fn write_temp(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="test" source-info-url="http://h/xmltv.php?username=u&amp;password=hunter2">
  <channel id="bbc1.uk">
    <display-name lang="en">BBC One</display-name>
    <display-name>BBC 1</display-name>
    <icon src="http://x/bbc1.png"/>
  </channel>
  <channel id="itv.uk">
    <display-name>ITV</display-name>
  </channel>
  <channel id="ghost.uk"><display-name>Ghost</display-name></channel>
  <programme start="20260927120000 +0000" stop="20260927130000 +0000" channel="bbc1.uk">
    <title lang="en">Tom &amp; Jerry</title>
    <desc>Cat &lt;3 mouse &#169; 2026</desc>
    <category>Kids</category>
    <credits><actor>Tom</actor></credits>
  </programme>
  <programme start="20260927130000 +0000" stop="20260927140000 +0000" channel="bbc1.uk">
    <title>News</title>
    <sub-title>Lunchtime edition</sub-title>
    <desc lang="en">First desc</desc>
    <desc lang="fr">Second desc</desc>
  </programme>
  <programme start="20260927150000 +0100" stop="20260927160000 +0100" channel="bbc1.uk">
    <title>Afternoon <![CDATA[<Show>]]></title>
  </programme>
  <programme start="20260927120000 +0000" stop="20260927123000 +0000" channel="itv.uk">
    <title>ITV Morning</title>
  </programme>
  <programme start="not-a-time" stop="20260927130000 +0000" channel="itv.uk">
    <title>Broken</title>
  </programme>
  <programme start="20260927123000 +0000" stop="20260927120000 +0000" channel="itv.uk">
    <title>Inverted</title>
  </programme>
  <programme start="20260927123000 +0000" stop="20260927130000 +0000" channel="itv.uk">
    <title></title>
  </programme>
</tv>
"#;

    struct Collect(Mutex<Vec<ImportProgressEvent>>);
    impl ProgressSink for Collect {
        fn progress(&self, ev: ImportProgressEvent) {
            self.0.lock().unwrap().push(ev);
        }
    }

    async fn assert_fixture_import(db: &Arc<Db>, p: i64, path: std::path::PathBuf) -> (SyncStats, Vec<XmltvChannel>) {
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let (stats, channels) =
            import_xmltv_with_channels(db.clone(), p, ImportSource::File { path }, sink.clone(), all_time())
                .await
                .unwrap();
        assert_eq!(stats.inserted, 4, "{stats:?}");
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.skipped, 3);
        assert_eq!(stats.groups, 2);
        assert!(stats.warnings.iter().any(|w| w == "1 programmes had unparsable times"), "{:?}", stats.warnings);
        assert!(stats.warnings.iter().any(|w| w == "1 programmes had stop <= start"), "{:?}", stats.warnings);
        assert!(stats.warnings.iter().any(|w| w == "1 channels declared but no programmes"), "{:?}", stats.warnings);
        assert!(stats.warnings.iter().all(|w| !w.contains("hunter2")));

        assert_eq!(channels.len(), 3);
        assert_eq!(channels[0].id, "bbc1.uk");
        assert_eq!(channels[0].display_names, vec!["BBC One", "BBC 1"]);
        assert_eq!(channels[0].icon.as_deref(), Some("http://x/bbc1.png"));
        assert_eq!(channels[1].display_names, vec!["ITV"]);
        assert_eq!(channels[1].icon, None);

        let (now, next) = db.now_next(p, "bbc1.uk", NOON + 1800).unwrap();
        let now = now.unwrap();
        assert_eq!(now.title, "Tom & Jerry");
        assert_eq!(now.desc.as_deref(), Some("Cat <3 mouse © 2026"));
        assert_eq!((now.start, now.stop), (NOON, NOON + 3600));
        let next = next.unwrap();
        assert_eq!(next.title, "News — Lunchtime edition");
        assert_eq!(next.desc.as_deref(), Some("First desc"), "first <desc> wins");
        let (now, next) = db.now_next(p, "bbc1.uk", NOON + 2 * 3600 + 60).unwrap();
        assert_eq!(now.unwrap().title, "Afternoon <Show>", "+0100 zone normalised to UTC; CDATA kept");
        assert!(next.is_none());
        let (now, _) = db.now_next(p, "itv.uk", NOON + 60).unwrap();
        assert_eq!(now.unwrap().title, "ITV Morning");
        assert_eq!(db.epg_stats(p).unwrap().programmes, 4);

        let stages: Vec<String> = sink.0.lock().unwrap().iter().map(|e| e.stage.clone()).collect();
        assert_eq!(stages.first().map(String::as_str), Some("fetching"));
        assert!(stages.contains(&"parsing".to_string()));
        let last = sink.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.stage, "done");
        assert_eq!(last.channels, 7, "programmes seen, including skipped");
        assert!(last.bytes > 0);
        (stats, channels)
    }

    #[test]
    fn parses_xmltv_times() {
        assert_eq!(parse_xmltv_time("20260927120000 +0000"), Some(NOON));
        assert_eq!(parse_xmltv_time("20260927120000"), Some(NOON), "missing zone is UTC");
        assert_eq!(parse_xmltv_time("  20260927120000 +0000  "), Some(NOON));
        assert_eq!(parse_xmltv_time("20260927130000 +0100"), Some(NOON));
        assert_eq!(parse_xmltv_time("20260927070000 -0500"), Some(NOON));
        assert_eq!(parse_xmltv_time("20260927133000 +01:30"), Some(NOON));
        assert_eq!(parse_xmltv_time("20260927120000+0000"), Some(NOON), "zone glued to digits");
        assert_eq!(parse_xmltv_time("20260927120000 Z"), Some(NOON));
        assert_eq!(parse_xmltv_time("20260927120000 UTC"), Some(NOON));
        assert_eq!(parse_xmltv_time("202609271200"), Some(NOON), "missing seconds");
        assert_eq!(parse_xmltv_time("2026092712"), Some(NOON), "missing minutes");
        assert_eq!(parse_xmltv_time("20260927"), Some(NOON - 12 * 3600), "date only = midnight");
        assert_eq!(parse_xmltv_time("2026092712000"), Some(NOON), "odd digit count pads right");
        assert_eq!(parse_xmltv_time("19700101000000 +0000"), Some(0));
        assert_eq!(parse_xmltv_time("19691231230000 -0100"), Some(0));
        assert_eq!(parse_xmltv_time("20000229120000"), Some(951_825_600), "leap day");
        assert_eq!(parse_xmltv_time("20240101000000 +0000"), Some(1_704_067_200));
        // invalid
        assert_eq!(parse_xmltv_time(""), None);
        assert_eq!(parse_xmltv_time("not-a-time"), None);
        assert_eq!(parse_xmltv_time("2026"), None, "no date");
        assert_eq!(parse_xmltv_time("20261327120000"), None, "month 13");
        assert_eq!(parse_xmltv_time("20230229120000"), None, "no leap day in 2023");
        assert_eq!(parse_xmltv_time("20260927250000"), None, "hour 25");
        assert_eq!(parse_xmltv_time("20260927120000 +2500"), None, "bogus zone");
        assert_eq!(parse_xmltv_time("20260927120000 PST"), None, "named zones unsupported");
        assert_eq!(parse_xmltv_time("202609271200001"), None, "too many digits");
    }

    #[test]
    fn days_from_civil_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 1, 1), 19_723);
        assert_eq!(days_from_civil(2026, 9, 27), 20_723);
    }

    #[tokio::test]
    async fn imports_plain_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "guide.xml", FIXTURE.as_bytes());
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        let (stats, _) = assert_fixture_import(&db, p, path.clone()).await;
        assert!(!stats.warnings.iter().any(|w| w.contains("gzip")));

        // Re-import is idempotent (PK replace) and reports the same counts.
        let stats =
            import_xmltv(db.clone(), p, ImportSource::File { path }, Arc::new(NoopSink), all_time()).await.unwrap();
        assert_eq!(stats.inserted, 4);
        assert_eq!(db.epg_stats(p).unwrap().programmes, 4);
    }

    #[tokio::test]
    async fn imports_gzipped_fixture_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "guide.xml.gz", &gzip(FIXTURE.as_bytes()));
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        let (stats, _) = assert_fixture_import(&db, p, path).await;
        assert!(stats.warnings.iter().any(|w| w.contains("gzip")), "{:?}", stats.warnings);
    }

    #[tokio::test]
    async fn rejects_html_with_redacted_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path =
            write_temp(&dir, "login.xml", b"<!DOCTYPE html><html><body>Login password=secret token=abc</body></html>");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let err = import_xmltv(db.clone(), p, ImportSource::File { path }, sink.clone(), all_time()).await.unwrap_err();
        match err {
            NetError::Preflight(e) => {
                assert_eq!(e.kind, PayloadKind::Html);
                assert!(e.message.contains("web page"), "{}", e.message);
                assert!(e.message.contains("XMLTV"), "{}", e.message);
                assert!(!e.preview.contains("secret") && !e.preview.contains("abc"), "{}", e.preview);
                assert!(e.preview.contains("password=***"), "{}", e.preview);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(db.epg_stats(p).unwrap().programmes, 0);
        let last = sink.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.stage, "error");
        assert!(!last.message.unwrap().contains("secret"));
    }

    #[tokio::test]
    async fn rejects_non_xml_with_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        for (name, body) in [
            ("list.m3u", "#EXTM3U\n#EXTINF:-1,X\nhttp://h/live/u/p/1.ts?token=zzz\n"),
            ("blob.json", "{\"user_info\":{\"auth\":0,\"password\":\"pw\"}}"),
            ("junk.bin", "garbage password=leak"),
            ("empty.xml", ""),
        ] {
            let path = write_temp(&dir, name, body.as_bytes());
            let err = import_xmltv(db.clone(), p, ImportSource::File { path }, Arc::new(NoopSink), all_time())
                .await
                .unwrap_err();
            match err {
                NetError::Parse(msg) => {
                    assert!(msg.contains("not an XMLTV guide"), "{name}: {msg}");
                    assert!(!msg.contains("zzz") && !msg.contains("leak"), "{name}: {msg}");
                }
                other => panic!("{name}: unexpected {other:?}"),
            }
        }
        // Malformed XML past the sniff window surfaces as a parse error with a position.
        let path = write_temp(&dir, "broken.xml", b"<?xml version=\"1.0\"?>\n<tv>\n<programme start=\"x\">\n</tv>\n");
        let err =
            import_xmltv(db.clone(), p, ImportSource::File { path }, Arc::new(NoopSink), all_time()).await.unwrap_err();
        assert!(matches!(err, NetError::Parse(ref m) if m.contains("syntax error")), "{err:?}");
    }

    #[tokio::test]
    async fn window_skips_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_temp(&dir, "guide.xml", FIXTURE.as_bytes());
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_pl(&db);
        // Seed a stale row that the prune must remove.
        db.upsert_programmes(
            p,
            &[ProgrammeInsert {
                channel_tvg_id: "bbc1.uk".into(),
                start: 0,
                stop: 60,
                title: "old".into(),
                desc: None,
            }],
        )
        .unwrap();
        let opts = XmltvOptions { prune_before: NOON + 3600 + 1, ignore_after: NOON + 3600 };
        let stats = import_xmltv(db.clone(), p, ImportSource::File { path }, Arc::new(NoopSink), opts).await.unwrap();
        // Kept: News (13:00–14:00). Dropped: Tom & Jerry (stops 13:00 < prune), Afternoon (starts
        // 14:00 > horizon), ITV Morning (stops 12:30 < prune) + the 3 always-invalid ones.
        assert_eq!(stats.inserted, 1, "{stats:?}");
        assert_eq!(stats.skipped, 6);
        assert!(
            stats.warnings.iter().any(|w| w == "3 programmes were outside the import window"),
            "{:?}",
            stats.warnings
        );
        assert!(stats.warnings.iter().any(|w| w.starts_with("pruned 1 programmes")), "{:?}", stats.warnings);
        let st = db.epg_stats(p).unwrap();
        assert_eq!(st.programmes, 1);
        assert_eq!(st.min_start, Some(NOON + 3600));
    }

    fn big_fixture(n: usize) -> String {
        let mut s = String::with_capacity(n * 160);
        s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n");
        for c in 0..50 {
            s.push_str(&format!("<channel id=\"ch{c}\"><display-name>Channel {c}</display-name></channel>\n"));
        }
        for i in 0..n {
            let c = i % 50;
            let slot = i / 50;
            let start = NOON + slot as i64 * 1800;
            // Explicit zone on odd rows to exercise both parser paths.
            let zone = if i % 2 == 0 { "" } else { " +0000" };
            s.push_str(&format!(
                "<programme start=\"{}{zone}\" stop=\"{}{zone}\" channel=\"ch{c}\"><title>Prog &amp; {i}</title><desc>D{i}</desc></programme>\n",
                fmt_ts(start),
                fmt_ts(start + 1800)
            ));
        }
        s.push_str("</tv>\n");
        s
    }

    /// Unix → `YYYYMMDDHHMMSS` (UTC), the inverse of `parse_xmltv_time` for round-trip fixtures.
    fn fmt_ts(t: i64) -> String {
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        // civil_from_days (Hinnant)
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    }

    #[tokio::test]
    async fn large_guide_crosses_chunk_and_batch_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let body = big_fixture(20_000);
        assert!(body.len() > 300 * 1024, "fixture must span several 256 KiB file chunks: {} bytes", body.len());
        let plain = write_temp(&dir, "big.xml", body.as_bytes());
        let gz = write_temp(&dir, "big.xml.gz", &gzip(body.as_bytes()));

        for (path, label) in [(plain, "plain"), (gz, "gzip")] {
            let db = Arc::new(Db::open_in_memory().unwrap());
            let p = new_pl(&db);
            let sink = Arc::new(Collect(Mutex::new(Vec::new())));
            let stats =
                import_xmltv(db.clone(), p, ImportSource::File { path }, sink.clone(), all_time()).await.unwrap();
            assert_eq!(stats.inserted, 20_000, "{label}: {stats:?}");
            assert_eq!(stats.skipped, 0, "{label}");
            assert_eq!(stats.groups, 50, "{label}");
            let st = db.epg_stats(p).unwrap();
            assert_eq!((st.programmes, st.channels_with_epg), (20_000, 50), "{label}");
            assert_eq!(st.min_start, Some(NOON), "{label}");
            assert_eq!(st.max_stop, Some(NOON + 400 * 1800), "{label}");
            // Spot-check a row deep in the file (past every chunk boundary).
            let (now, next) = db.now_next(p, "ch49", NOON + 399 * 1800 + 10).unwrap();
            assert_eq!(now.unwrap().title, "Prog & 19999", "{label}");
            assert!(next.is_none(), "{label}");
            let events = sink.0.lock().unwrap();
            assert!(
                events.iter().any(|e| e.stage == "indexing" && e.message.as_deref().is_some_and(|m| m.ends_with('%'))),
                "{label}: percent progress expected"
            );
            assert_eq!(events.last().unwrap().stage, "done", "{label}");
            assert_eq!(events.last().unwrap().channels, 20_000, "{label}");
        }
    }

    #[test]
    fn parser_handles_nested_and_empty_elements() {
        let doc = r#"<tv><channel id="a"/><channel id="b"><display-name>B</display-name><icon src="s"/></channel>
<programme start="1" stop="2" channel="a"/>
<programme start="3" stop="4" channel="b"><title>T<i>nested</i>!</title><title>second</title><sub-title></sub-title><desc>D</desc></programme></tv>"#;
        let mut reader = Reader::from_str(doc);
        let mut parser = XmltvParser::default();
        let mut out = Vec::new();
        loop {
            let emitted = match reader.read_event().unwrap() {
                Event::Eof => break,
                Event::Start(e) => parser.start(&e, false),
                Event::Empty(e) => parser.start(&e, true),
                Event::End(_) => parser.end(),
                Event::Text(t) => {
                    parser.text(&t);
                    None
                }
                _ => None,
            };
            if let Some(e) = emitted {
                out.push(e);
            }
        }
        assert_eq!(out.len(), 4);
        match &out[0] {
            Emitted::Channel(c) => assert_eq!((c.id.as_str(), c.display_names.len(), c.icon.is_none()), ("a", 0, true)),
            _ => panic!(),
        }
        match &out[1] {
            Emitted::Channel(c) => {
                assert_eq!((c.id.as_str(), c.display_names[0].as_str(), c.icon.as_deref()), ("b", "B", Some("s")))
            }
            _ => panic!(),
        }
        match &out[2] {
            Emitted::Programme(p) => assert!(p.title.is_none() && p.channel.as_deref() == Some("a")),
            _ => panic!(),
        }
        match &out[3] {
            Emitted::Programme(p) => {
                assert_eq!(p.title.as_deref(), Some("Tnested!"), "nested markup inside title is flattened");
                assert_eq!(p.sub_title, None, "empty sub-title is not appended");
                assert_eq!(p.desc.as_deref(), Some("D"));
            }
            _ => panic!(),
        }
    }
}
