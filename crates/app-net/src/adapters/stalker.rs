//! Stalker / Ministra "MAC portal" adapter (CLAUDE.md §7.3) — **feature-flagged, live TV only**.
//!
//! The portal speaks a JSON RPC over GET (`portal.php?type=…&action=…`) and identifies the box by
//! a MAC address sent as a cookie plus a bearer token obtained from a `handshake`. Channel `cmd`
//! values are not playable URLs: they must be exchanged for a short-lived tokenised link with
//! `create_link` **at play time**. We store them as `stalker://<cmd>` in `channels.stream_url`
//! and the command layer resolves them just before `loadfile`.
//!
//! Every hop is logged redacted (the MAC and token never appear in logs or the HTTP trace) —
//! the MAC is an identity, so `redact` masks `mac=` and `Authorization` everywhere.
//!
//! Out of scope for the flagged v1: VOD, series, EPG (`get_epg_info` differs per portal; add an
//! XMLTV source manually), timeshift/catch-up.

use super::{AccountInfo, CatalogAdapter};
use crate::http::HttpClient;
use crate::{NetError, Result};
use app_core::redact::redact;
use app_core::{ImportProgressEvent, SyncStats};
use app_db::channels::ChannelInsert;
use app_db::Db;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

pub const STALKER_SCHEME: &str = "stalker://";
/// What most portals expect a MAG box to send. Sent instead of our product UA on purpose:
/// portals whitelist STB user agents.
pub const STB_USER_AGENT: &str =
    "Mozilla/5.0 (QtEmbedded; U; Linux; C) AppleWebKit/533.3 (KHTML, like Gecko) MAG200 stbapp ver: 2 rev: 250 Safari/533.3";
const X_USER_AGENT: &str = "Model: MAG250; Link: WiFi";
const BATCH: usize = app_db::MAX_ROWS_PER_TX;

/// Normalise a MAC to `AA:BB:CC:DD:EE:FF`. Accepts `aabbccddeeff`, dashes, dots.
pub fn normalize_mac(raw: &str) -> Option<String> {
    let hex: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 12 {
        return None;
    }
    let up = hex.to_ascii_uppercase();
    Some((0..6).map(|i| &up[i * 2..i * 2 + 2]).collect::<Vec<_>>().join(":"))
}

/// Portal endpoint candidates for a user-supplied portal URL. Users paste anything from
/// `http://host` to `http://host/c/` to `http://host/stalker_portal/c/index.html`.
pub fn endpoint_candidates(portal: &str) -> Result<Vec<String>> {
    let mut u = url::Url::parse(portal.trim()).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(NetError::InvalidUrl(format!("unsupported scheme {}", u.scheme())));
    }
    u.set_query(None);
    u.set_fragment(None);
    let mut path = u.path().trim_end_matches('/').to_string();
    for suffix in ["/index.html", "/c", "/server/load.php", "/portal.php"] {
        if path.ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
        }
    }
    let path = path.trim_end_matches('/').to_string();
    u.set_path("");
    let origin = u.as_str().trim_end_matches('/').to_string();
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if !path.is_empty() {
        push(format!("{origin}{path}/server/load.php"));
        push(format!("{origin}{path}/portal.php"));
    }
    push(format!("{origin}/portal.php"));
    push(format!("{origin}/stalker_portal/server/load.php"));
    push(format!("{origin}/server/load.php"));
    push(format!("{origin}/c/portal.php"));
    Ok(out)
}

pub struct StalkerClient {
    http: HttpClient,
    origin: String,
    candidates: Vec<String>,
    endpoint: Mutex<Option<String>>,
    mac: String,
    token: Mutex<Option<String>>,
    /// Pseudo serial / device ids derived from the MAC so the portal sees a stable "box".
    sn: String,
    device_id: String,
}

impl StalkerClient {
    pub fn new(portal: &str, mac: &str, user_agent: Option<&str>) -> Result<Self> {
        let mac = normalize_mac(mac)
            .ok_or_else(|| NetError::InvalidUrl("MAC must be 12 hex digits, e.g. 00:1A:79:12:34:56".into()))?;
        let candidates = endpoint_candidates(portal)?;
        let origin = {
            let u = url::Url::parse(&candidates[0]).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
            format!(
                "{}://{}{}",
                u.scheme(),
                u.host_str().unwrap_or(""),
                u.port().map(|p| format!(":{p}")).unwrap_or_default()
            )
        };
        let ua = user_agent.filter(|s| !s.trim().is_empty()).unwrap_or(STB_USER_AGENT);
        let http = HttpClient::new(Some(ua))?.with_kind("stalker");
        let digest = fnv64(&mac);
        Ok(Self {
            http,
            origin,
            candidates,
            endpoint: Mutex::new(None),
            mac,
            token: Mutex::new(None),
            sn: format!("{digest:013X}")[..13].to_string(),
            device_id: format!("{digest:016X}{digest:016X}{digest:016X}{digest:016X}"),
        })
    }

    pub fn mac(&self) -> &str {
        &self.mac
    }

    fn cookie(&self) -> String {
        format!("mac={}; stb_lang=en; timezone=UTC", url_encode(&self.mac))
    }

    fn build_url(&self, endpoint: &str, kind: &str, action: &str, extra: &[(&str, &str)]) -> String {
        let mut s = format!("{endpoint}?type={kind}&action={action}");
        for (k, v) in extra {
            s.push('&');
            s.push_str(k);
            s.push('=');
            s.push_str(&url_encode(v));
        }
        s.push_str("&JsHttpRequest=1-xml");
        s
    }

    /// One portal call. Returns the `js` member of the response.
    async fn call_at(&self, endpoint: &str, kind: &str, action: &str, extra: &[(&str, &str)]) -> Result<Value> {
        let url = self.build_url(endpoint, kind, action, extra);
        let mut req = self
            .http
            .raw()
            .get(&url)
            .timeout(crate::http::REQUEST_TIMEOUT)
            .header("X-User-Agent", X_USER_AGENT)
            .header("Referer", format!("{}/c/", self.origin))
            .header("Cookie", self.cookie())
            .header("Accept", "*/*");
        if let Some(t) = self.token.lock().unwrap().as_deref() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let pending = crate::trace::start("stalker", "GET", &url);
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                pending.fail(&e.to_string());
                return Err(e.into());
            }
        };
        let status = resp.status().as_u16();
        let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let id = pending.finish(status, ct.as_deref(), resp.content_length());
        let text = resp.text().await?;
        crate::trace::set_preview(id, text.as_bytes());
        tracing::info!(action, status, bytes = text.len(), url = %redact(&url), "stalker call");
        if status == 401 || status == 403 {
            crate::trace::set_error(id, &format!("HTTP {status}"));
            return Err(NetError::Http(format!(
                "portal refused the request (HTTP {status}) — MAC not registered or expired?"
            )));
        }
        if !(200..300).contains(&status) {
            crate::trace::set_error(id, &format!("HTTP {status}"));
            return Err(NetError::Http(format!("HTTP {status} from portal")));
        }
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case("authorization failed") || trimmed.is_empty() {
            crate::trace::set_error(id, "authorization failed / empty body");
            return Err(NetError::Http(
                "portal answered 'Authorization failed' — MAC not registered, expired, or wrong portal URL".into(),
            ));
        }
        let v: Value = serde_json::from_str(trimmed).map_err(|_| {
            NetError::Parse(format!(
                "portal did not return JSON for {action} (first bytes: {})",
                crate::trace::preview_of(trimmed.as_bytes()).chars().take(80).collect::<String>()
            ))
        })?;
        Ok(v.get("js").cloned().unwrap_or(v))
    }

    async fn call(&self, kind: &str, action: &str, extra: &[(&str, &str)]) -> Result<Value> {
        let ep = self
            .endpoint
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| NetError::Other("portal not authenticated (call handshake first)".into()))?;
        self.call_at(&ep, kind, action, extra).await
    }

    /// Find the endpoint and obtain a bearer token. Idempotent; re-run on 401/403.
    pub async fn handshake(&self) -> Result<String> {
        let mut last: Option<NetError> = None;
        for ep in &self.candidates {
            match self.call_at(ep, "stb", "handshake", &[("token", ""), ("prehash", "")]).await {
                Ok(js) => {
                    if let Some(t) = js.get("token").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                        *self.token.lock().unwrap() = Some(t.to_string());
                        *self.endpoint.lock().unwrap() = Some(ep.clone());
                        tracing::info!(endpoint = %redact(ep), "stalker handshake ok");
                        return Ok(t.to_string());
                    }
                    last = Some(NetError::Parse("handshake response had no token".into()));
                }
                Err(e) => {
                    tracing::info!(endpoint = %redact(ep), error = %e, "stalker endpoint candidate failed");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| NetError::Other("no portal endpoint answered".into())))
    }

    /// `get_profile` registers the box for this session (some portals require it before itv calls).
    pub async fn get_profile(&self) -> Result<Value> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .to_string();
        let extra: Vec<(&str, &str)> = vec![
            ("hd", "1"),
            ("ver", "ImageDescription: 0.2.18-r23-250; ImageDate: Wed Aug 29 10:49:53 EEST 2018; PORTAL version: 5.6.2; API Version: JS API version: 343; STB API version: 146; Player Engine version: 0x58c"),
            ("num_banks", "2"),
            ("sn", &self.sn),
            ("stb_type", "MAG250"),
            ("client_type", "STB"),
            ("image_version", "218"),
            ("video_out", "hdmi"),
            ("device_id", &self.device_id),
            ("device_id2", &self.device_id),
            ("signature", ""),
            ("auth_second_step", "1"),
            ("hw_version", "1.7-BD-00"),
            ("not_valid_token", "0"),
            ("metrics", "{\"mac\":\"\",\"sn\":\"\",\"model\":\"MAG250\",\"type\":\"STB\",\"uid\":\"\",\"random\":\"\"}"),
            ("hw_version_2", "b3dc2e4b7f6a3c1d"),
            ("timestamp", &ts),
            ("api_signature", "262"),
            ("prehash", ""),
        ];
        self.call("stb", "get_profile", &extra).await
    }

    /// Account expiry etc. Best effort: not every portal exposes it.
    pub async fn get_main_info(&self) -> Result<Value> {
        self.call("account_info", "get_main_info", &[]).await
    }

    pub async fn get_genres(&self) -> Result<HashMap<String, String>> {
        let js = self.call("itv", "get_genres", &[]).await?;
        let mut map = HashMap::new();
        if let Some(list) = js.as_array() {
            for g in list {
                if let (Some(id), Some(title)) = (str_of(g.get("id")), str_of(g.get("title"))) {
                    map.insert(id, title);
                }
            }
        }
        Ok(map)
    }

    /// All live channels. Tries the single-shot `get_all_channels`, then the paged list.
    pub async fn get_all_channels(&self) -> Result<Vec<Value>> {
        let js = self.call("itv", "get_all_channels", &[]).await?;
        let mut items: Vec<Value> = js.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
        if !items.is_empty() {
            return Ok(items);
        }
        // Paged fallback: get_ordered_list&genre=*&p=N
        let mut page = 1u32;
        loop {
            let p = page.to_string();
            let js = self
                .call(
                    "itv",
                    "get_ordered_list",
                    &[
                        ("genre", "*"),
                        ("force_ch_link_check", ""),
                        ("fav", "0"),
                        ("sortby", "number"),
                        ("hd", "0"),
                        ("p", &p),
                    ],
                )
                .await?;
            let data = js.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
            let total = js.get("total_items").and_then(as_u64).unwrap_or(0);
            let per_page = js.get("max_page_items").and_then(as_u64).unwrap_or(14).max(1);
            let got = data.len();
            items.extend(data);
            if got == 0 || (items.len() as u64) >= total || page as u64 > total.div_ceil(per_page) + 1 || page > 5000 {
                break;
            }
            page += 1;
        }
        Ok(items)
    }

    /// Exchange a channel `cmd` for a playable URL. Re-handshakes once on an auth failure.
    pub async fn create_link(&self, cmd: &str) -> Result<String> {
        let cmd = cmd.strip_prefix(STALKER_SCHEME).unwrap_or(cmd);
        if self.endpoint.lock().unwrap().is_none() {
            self.handshake().await?;
        }
        let extra = [
            ("cmd", cmd),
            ("series", ""),
            ("forced_storage", ""),
            ("disable_ad", "0"),
            ("download", "0"),
            ("force_ch_link_check", "0"),
        ];
        let js = match self.call("itv", "create_link", &extra).await {
            Ok(js) => js,
            Err(NetError::Http(msg)) if msg.contains("refused") || msg.contains("Authorization failed") => {
                self.handshake().await?;
                self.call("itv", "create_link", &extra).await?
            }
            Err(e) => return Err(e),
        };
        let link = js
            .get("cmd")
            .and_then(|c| c.as_str())
            .map(str::to_string)
            .or_else(|| js.as_str().map(str::to_string))
            .ok_or_else(|| NetError::Parse("create_link returned no cmd".into()))?;
        let url = strip_cmd_prefix(&link);
        if !(url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("rtsp://")
            || url.starts_with("rtmp://")
            || url.starts_with("udp://"))
        {
            return Err(NetError::Parse(format!("create_link returned something that is not a URL: {}", redact(&url))));
        }
        Ok(url)
    }

    fn map_channel(&self, item: &Value, genres: &HashMap<String, String>) -> Option<ChannelInsert> {
        let id = str_of(item.get("id"))?;
        let name = str_of(item.get("name")).filter(|s| !s.trim().is_empty())?;
        let cmd = str_of(item.get("cmd")).filter(|s| !s.trim().is_empty())?;
        let group = str_of(item.get("tv_genre_id")).and_then(|g| genres.get(&g).cloned());
        let logo = str_of(item.get("logo")).filter(|l| !l.is_empty()).map(|l| {
            if l.starts_with("http://") || l.starts_with("https://") {
                l
            } else {
                format!("{}/stalker_portal/misc/logos/320/{}", self.origin, l.trim_start_matches('/'))
            }
        });
        let archive = item.get("archive").and_then(as_u64).unwrap_or(0) == 1
            || item.get("allow_local_timeshift").and_then(as_u64).unwrap_or(0) == 1;
        let days = item.get("tv_archive_duration").and_then(as_u64).unwrap_or(0);
        Some(ChannelInsert {
            source_id: id,
            name: name.trim().to_string(),
            group_title: group,
            logo,
            stream_url: format!("{STALKER_SCHEME}{}", cmd.trim()),
            tvg_id: str_of(item.get("xmltv_id")).filter(|s| !s.is_empty()),
            tvg_name: None,
            catchup: archive,
            catchup_days: if archive { (days.clamp(0, 30) as i32).max(1) } else { 0 },
        })
    }
}

/// `ffmpeg http://…` / `auto http://…` → `http://…`
pub fn strip_cmd_prefix(cmd: &str) -> String {
    let t = cmd.trim();
    if let Some(idx) = t.find("://") {
        // Walk back to the start of the scheme token.
        let start = t[..idx].rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        return t[start..].trim().to_string();
    }
    t.to_string()
}

fn str_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(u64::from(*b)),
        _ => None,
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Account info from `get_main_info` / `get_profile` (portal-dependent, all optional).
fn account_from(profile: &Value, main: Option<&Value>) -> AccountInfo {
    let mut a = AccountInfo::default();
    if let Some(m) = main {
        // "phone" holds the expiry text on Ministra ("Expires: 2027-01-01" / "Unlimited").
        let exp = str_of(m.get("phone"))
            .or_else(|| str_of(m.get("end_date")))
            .or_else(|| str_of(m.get("expire_billing_date")));
        a.status = Some(match exp.as_deref() {
            Some(s) if !s.trim().is_empty() => format!("registered ({})", s.trim()),
            _ => "registered".into(),
        });
        a.exp_date = exp.as_deref().and_then(parse_expiry);
    }
    if a.status.is_none() {
        a.status = Some(if profile.get("id").and_then(as_u64).unwrap_or(0) > 0 {
            "registered".into()
        } else {
            "unknown".into()
        });
    }
    a.server_timezone = str_of(profile.get("default_timezone")).or_else(|| str_of(profile.get("timezone")));
    a
}

/// Best-effort `YYYY-MM-DD` inside an expiry string → unix seconds (UTC midnight).
fn parse_expiry(s: &str) -> Option<i64> {
    let digits: Vec<&str> = s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).collect();
    for w in digits.windows(3) {
        if w[0].len() == 4 {
            let (y, m, d) = (w[0].parse::<i64>().ok()?, w[1].parse::<i64>().ok()?, w[2].parse::<i64>().ok()?);
            if (1..=12).contains(&m) && (1..=31).contains(&d) {
                return Some(days_from_civil(y, m, d) * 86_400);
            }
        }
    }
    None
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------- adapter ----------

pub struct StalkerAdapter {
    pub db: Arc<Db>,
    pub playlist_id: i64,
    pub client: StalkerClient,
    pub sink: Arc<dyn crate::importer::ProgressSink>,
}

impl StalkerAdapter {
    pub fn from_playlist(db: Arc<Db>, playlist_id: i64, sink: Arc<dyn crate::importer::ProgressSink>) -> Result<Self> {
        let src = db.playlist_source(playlist_id)?;
        if src.r#type != "stalker" {
            return Err(NetError::Other(format!("playlist {playlist_id} is a {} source, not Stalker", src.r#type)));
        }
        let mac = src.mac.as_deref().ok_or_else(|| NetError::Other("stalker playlist has no MAC".into()))?;
        let client = StalkerClient::new(&src.base_url, mac, src.ua.as_deref())?;
        Ok(Self { db, playlist_id, client, sink })
    }

    fn emit(&self, stage: &str, rows: u64, bytes: u64, message: Option<String>) {
        self.sink.progress(ImportProgressEvent {
            playlist_id: self.playlist_id,
            stage: stage.into(),
            channels: rows,
            bytes,
            message,
        });
    }

    async fn ensure_session(&self) -> Result<Value> {
        if self.client.endpoint.lock().unwrap().is_none() {
            self.client.handshake().await?;
        }
        // Tolerate portals that reject get_profile but serve itv calls anyway.
        Ok(self.client.get_profile().await.unwrap_or(Value::Null))
    }

    async fn run_live(&self) -> Result<SyncStats> {
        let started = Instant::now();
        let mut stats = SyncStats::default();
        self.ensure_session().await?;
        let genres = self.client.get_genres().await.unwrap_or_default();
        let items = self.client.get_all_channels().await?;
        self.emit("parsing", 0, 0, None);
        let mut seen: HashSet<String> = HashSet::new();
        let mut groups: HashSet<String> = HashSet::new();
        let mut rows: Vec<ChannelInsert> = Vec::new();
        for item in &items {
            match self.client.map_channel(item, &genres) {
                Some(row) if seen.insert(row.source_id.clone()) => {
                    if let Some(g) = &row.group_title {
                        if !groups.contains(g) {
                            groups.insert(g.clone());
                        }
                    }
                    rows.push(row);
                }
                _ => stats.skipped += 1,
            }
        }
        let total = rows.len() as u64;
        if rows.is_empty() {
            stats.warnings.push("portal returned no channels; existing channels were kept".into());
        } else {
            let sync_gen = self.db.next_sync_gen(self.playlist_id)?;
            let mut done = 0u64;
            for chunk in rows.chunks(BATCH) {
                let (ins, upd) = self.db.upsert_channels_gen(self.playlist_id, chunk, Some(sync_gen))?;
                stats.inserted += ins;
                stats.updated += upd;
                done += chunk.len() as u64;
                self.emit("indexing", done, 0, Some(format!("{}%", (done * 100 / total).min(99))));
                tokio::task::yield_now().await;
            }
            let removed = self.db.delete_stale_channels(self.playlist_id, sync_gen)?;
            if removed > 0 {
                stats.warnings.push(format!("removed {removed} channels no longer on the portal"));
            }
        }
        let _ = self.db.checkpoint();
        stats.groups = groups.len() as u64;
        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        if genres.is_empty() {
            stats.warnings.push("portal returned no genres (channels imported without groups)".into());
        }
        stats
            .warnings
            .push("Stalker support is experimental: live TV only (no VOD/EPG); links are resolved at play time".into());
        tracing::info!(playlist_id = self.playlist_id, ?stats, "stalker live sync complete");
        self.emit("done", total, 0, None);
        Ok(stats)
    }
}

#[async_trait]
impl CatalogAdapter for StalkerAdapter {
    async fn authenticate(&self) -> Result<AccountInfo> {
        self.client.handshake().await?;
        let profile = self.client.get_profile().await.unwrap_or(Value::Null);
        let main = self.client.get_main_info().await.ok();
        let info = account_from(&profile, main.as_ref());
        // Store what the diagnostics report may show — never the MAC or token.
        let json = serde_json::json!({
            "status": info.status,
            "exp_date": info.exp_date,
            "max_connections": info.max_connections,
            "portal_kind": "stalker",
            "timezone": info.server_timezone,
        });
        let _ = self.db.set_playlist_account_json(self.playlist_id, Some(&json.to_string()));
        Ok(info)
    }

    async fn sync_live(&self, _playlist_id: i64) -> Result<SyncStats> {
        self.emit("fetching", 0, 0, None);
        match self.run_live().await {
            Ok(s) => {
                let _ = self.db.set_playlist_sync_result(self.playlist_id, None);
                Ok(s)
            }
            Err(e) => {
                let msg = redact(&e.to_string());
                self.emit("error", 0, 0, Some(msg.clone()));
                let _ = self.db.set_playlist_sync_result(self.playlist_id, Some(&msg));
                Err(e)
            }
        }
    }

    async fn sync_vod(&self, _playlist_id: i64) -> Result<SyncStats> {
        Ok(SyncStats { warnings: vec!["Stalker VOD is not supported yet".into()], ..Default::default() })
    }

    async fn sync_epg(&self, _playlist_id: i64) -> Result<SyncStats> {
        Err(NetError::Other("Stalker EPG is not supported yet — add an XMLTV source for this playlist".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_db::channels::PlaylistInsert;
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn mac_and_endpoints_and_prefix() {
        assert_eq!(normalize_mac("00:1a:79:12:34:56").as_deref(), Some("00:1A:79:12:34:56"));
        assert_eq!(normalize_mac("001A79123456").as_deref(), Some("00:1A:79:12:34:56"));
        assert!(normalize_mac("nope").is_none());
        let c = endpoint_candidates("http://host:8080/c/").unwrap();
        assert_eq!(c[0], "http://host:8080/portal.php");
        assert!(c.contains(&"http://host:8080/stalker_portal/server/load.php".to_string()));
        let c = endpoint_candidates("http://host/stalker_portal/c/index.html").unwrap();
        assert_eq!(c[0], "http://host/stalker_portal/server/load.php");
        assert_eq!(strip_cmd_prefix("ffmpeg http://h/ch/1_?token=x"), "http://h/ch/1_?token=x");
        assert_eq!(strip_cmd_prefix("auto   rtsp://h/x"), "rtsp://h/x");
        assert_eq!(parse_expiry("Expires: 2027-01-15"), Some(1_799_971_200));
        assert_eq!(parse_expiry("Unlimited"), None);
    }

    /// Minimal portal: handshake → token; itv calls require the bearer; create_link tokenises.
    async fn serve(seen: Arc<StdMutex<Vec<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let mut head = Vec::new();
                    loop {
                        let n = match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        head.extend_from_slice(&buf[..n]);
                        if head.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&head).to_string();
                    let line = text.lines().next().unwrap_or("").to_string();
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let has_bearer =
                        text.lines().any(|l| l.to_ascii_lowercase().starts_with("authorization: bearer tok123"));
                    let has_mac = text.lines().any(|l| {
                        l.to_ascii_lowercase().starts_with("cookie:") && l.contains("mac=00%3A1A%3A79%3A12%3A34%3A56")
                    });
                    seen.lock().unwrap().push(line.clone());
                    let (status, body) = if !path.starts_with("/portal.php") {
                        ("404 Not Found", "<html>no</html>".to_string())
                    } else if !has_mac {
                        ("200 OK", "Authorization failed".to_string())
                    } else if path.contains("action=handshake") {
                        ("200 OK", r#"{"js":{"token":"tok123","random":"r"}}"#.to_string())
                    } else if !has_bearer {
                        ("403 Forbidden", "".to_string())
                    } else if path.contains("action=get_profile") {
                        ("200 OK", r#"{"js":{"id":42,"default_timezone":"Europe/London"}}"#.to_string())
                    } else if path.contains("type=account_info") {
                        ("200 OK", r#"{"js":{"mac":"00:1A:79:12:34:56","phone":"Expires: 2027-01-15"}}"#.to_string())
                    } else if path.contains("action=get_genres") {
                        ("200 OK", r#"{"js":[{"id":"1","title":"News"},{"id":"2","title":"Sport"}]}"#.to_string())
                    } else if path.contains("action=get_all_channels") {
                        (
                            "200 OK",
                            r#"{"js":{"data":[
                              {"id":"10","name":"One","cmd":"ffmpeg http://localhost/ch/10_","tv_genre_id":"1","logo":"one.png","xmltv_id":"one.uk","archive":"1","tv_archive_duration":"7"},
                              {"id":"11","name":"Two","cmd":"ffmpeg http://localhost/ch/11_","tv_genre_id":"2","logo":"","archive":"0"},
                              {"id":"12","name":"","cmd":"x"}
                            ]}}"#
                                .to_string(),
                        )
                    } else if path.contains("action=create_link") {
                        (
                            "200 OK",
                            r#"{"js":{"id":10,"cmd":"ffmpeg http://cdn.example/live/10.ts?token=abc&play_token=zzz"}}"#
                                .to_string(),
                        )
                    } else {
                        ("200 OK", r#"{"js":{}}"#.to_string())
                    };
                    let resp = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}/c/")
    }

    #[tokio::test]
    async fn stalker_end_to_end_against_mock_portal() {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let portal = serve(seen.clone()).await;
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = db
            .insert_playlist(&PlaylistInsert {
                r#type: "stalker".into(),
                name: "portal".into(),
                base_url: portal.clone(),
                user: None,
                pass: None,
                mac: Some("00:1a:79:12:34:56".into()),
                ua: None,
            })
            .unwrap();
        let adapter = StalkerAdapter::from_playlist(db.clone(), p, Arc::new(crate::importer::NoopSink)).unwrap();
        let info = adapter.authenticate().await.unwrap();
        assert!(info.status.as_deref().unwrap().contains("Expires: 2027-01-15"));
        assert_eq!(info.exp_date, Some(1_799_971_200));
        assert_eq!(info.server_timezone.as_deref(), Some("Europe/London"));
        let meta = db.playlist_meta(p).unwrap();
        let json = meta.account_json.unwrap();
        assert!(
            !json.contains("00:1A") && !json.contains("tok123"),
            "account json must not hold the MAC/token: {json}"
        );

        let stats = adapter.sync_live(p).await.unwrap();
        assert_eq!(stats.inserted, 2);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.groups, 2);
        let chans = db.list_channels(p, None, 10, 0).unwrap();
        let one = chans.iter().find(|c| c.name == "One").unwrap();
        assert_eq!(one.stream_url, "stalker://ffmpeg http://localhost/ch/10_");
        assert_eq!(one.group_title.as_deref(), Some("News"));
        assert_eq!(one.tvg_id.as_deref(), Some("one.uk"));
        assert_eq!(one.catchup_days, 7);
        assert!(one.logo.as_deref().unwrap().ends_with("/stalker_portal/misc/logos/320/one.png"));

        // Play-time resolution.
        let url = adapter.client.create_link(&one.stream_url).await.unwrap();
        assert_eq!(url, "http://cdn.example/live/10.ts?token=abc&play_token=zzz");

        // Every hop was traced and redacted.
        let traces = crate::trace::snapshot();
        let mine: Vec<_> = traces.iter().filter(|t| t.kind == "stalker").collect();
        assert!(mine.len() >= 6, "{}", mine.len());
        for t in &mine {
            let dump = format!("{t:?}");
            assert!(!dump.contains("tok123"), "token leaked: {dump}");
            assert!(!dump.contains("00:1A:79") && !dump.contains("00%3A1A"), "mac leaked: {dump}");
        }
        // The portal really saw the cookie and the bearer.
        let lines = seen.lock().unwrap();
        assert!(lines.iter().any(|l| l.contains("action=create_link") && l.contains("cmd=ffmpeg%20http")));
    }
}
