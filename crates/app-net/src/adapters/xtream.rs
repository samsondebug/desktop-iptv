//! Xtream Codes adapter (CLAUDE.md §7.1).
//!
//! `player_api.php` JSON → SQLite. The panel API is a de-facto standard with no schema: numbers
//! arrive as strings, strings as numbers, fields go missing or come back `null`, and a wrong URL
//! returns an HTML login page with HTTP 200. Everything here is written to tolerate that.
//!
//! Secrets: the password travels as a query parameter *and* as a path segment of every media
//! URL. It is never put in [`XtreamAccount`], logs, progress events or error strings — every
//! message that can carry a URL goes through [`app_core::redact::redact`], and
//! [`XtreamClient`]'s `Debug` output masks it.

use super::{AccountInfo, CatalogAdapter};
use crate::http::HttpClient;
use crate::importer::{ImportSource, ProgressSink};
use crate::preflight::{self, PayloadKind};
use crate::xmltv::{import_xmltv, XmltvOptions};
use crate::{NetError, Result};
use app_core::redact::redact;
use app_core::{ImportProgressEvent, SyncStats};
use app_db::channels::ChannelInsert;
use app_db::vod::{EpisodeInsert, VodInsert};
use app_db::Db;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use url::Url;

/// Hard cap on one API response body. A 100k-title VOD list is ~60 MB; anything past this is a
/// broken panel, not a catalog.
pub const MAX_JSON_BYTES: usize = 256 * 1024 * 1024;
/// Group title for streams whose `category_id` does not resolve.
pub const UNCATEGORIZED: &str = "Uncategorized";
/// Rows per SQLite transaction (CLAUDE.md §5).
const BATCH: usize = app_db::MAX_ROWS_PER_TX;
/// Script names people paste as part of the "base URL"; stripped during normalisation.
const STRIP_SCRIPTS: &[&str] = &["player_api.php", "get.php", "xmltv.php", "panel_api.php", "enigma2.php"];
/// Only years in this range are believed; everything else is a placeholder like `0` or `9999`.
const YEAR_RANGE: std::ops::RangeInclusive<i64> = 1888..=2100;

// ---------------------------------------------------------------------------------------------
// account
// ---------------------------------------------------------------------------------------------

/// Login response (`user_info` + `server_info`) minus the password. Safe to store, log and
/// send to the UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct XtreamAccount {
    pub username: String,
    pub status: Option<String>,
    pub exp_date: Option<i64>,
    pub is_trial: bool,
    pub active_cons: Option<u32>,
    pub max_connections: Option<u32>,
    pub created_at: Option<i64>,
    pub allowed_output_formats: Vec<String>,
    pub server_url: Option<String>,
    pub timezone: Option<String>,
    pub server_time: Option<i64>,
    pub auth: bool,
}

impl XtreamAccount {
    pub fn to_account_info(&self) -> AccountInfo {
        AccountInfo {
            status: self.status.clone(),
            exp_date: self.exp_date,
            max_connections: self.max_connections,
            active_connections: self.active_cons,
            server_timezone: self.timezone.clone(),
        }
    }
}

impl From<XtreamAccount> for AccountInfo {
    fn from(a: XtreamAccount) -> Self {
        a.to_account_info()
    }
}

/// Parse a `player_api.php` login response. `auth == 0` or a missing `user_info` is a rejected
/// login; the message carries the account status (if any) but never a credential.
pub fn parse_account(v: &Value) -> Result<XtreamAccount> {
    let Some(ui) = v.get("user_info").filter(|u| u.is_object()) else {
        return Err(login_rejected(None));
    };
    if !truthy(ui, "auth") {
        return Err(login_rejected(s(ui, "status")));
    }
    let si = v.get("server_info").filter(|x| x.is_object());
    let proto = si.and_then(|x| s(x, "server_protocol")).unwrap_or_else(|| "http".into()).to_ascii_lowercase();
    let port =
        si.and_then(|x| if proto == "https" { s(x, "https_port").or_else(|| s(x, "port")) } else { s(x, "port") });
    let server_url = si.and_then(|x| s(x, "url")).map(|host| {
        if host.contains("://") {
            host
        } else {
            match &port {
                Some(p) => format!("{proto}://{host}:{p}"),
                None => format!("{proto}://{host}"),
            }
        }
    });
    let allowed_output_formats = match ui.get("allowed_output_formats") {
        Some(Value::Array(a)) => a.iter().filter_map(value_to_string).collect(),
        Some(Value::String(x)) => x.split(',').map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect(),
        _ => Vec::new(),
    };
    Ok(XtreamAccount {
        username: s(ui, "username").unwrap_or_default(),
        status: s(ui, "status"),
        exp_date: i(ui, "exp_date").filter(|t| *t > 0),
        is_trial: truthy(ui, "is_trial"),
        active_cons: i(ui, "active_cons").and_then(to_u32),
        max_connections: i(ui, "max_connections").and_then(to_u32),
        created_at: i(ui, "created_at").filter(|t| *t > 0),
        allowed_output_formats,
        server_url,
        timezone: si.and_then(|x| s(x, "timezone")),
        server_time: si.and_then(|x| i(x, "timestamp_now")).filter(|t| *t > 0),
        auth: true,
    })
}

fn login_rejected(status: Option<String>) -> NetError {
    let mut msg = String::from("Xtream login rejected (auth=0): check username/password");
    if let Some(st) = status.filter(|st| !st.eq_ignore_ascii_case("active")) {
        msg.push_str(" — account status: ");
        msg.push_str(&redact(&st));
    }
    NetError::Http(msg)
}

// ---------------------------------------------------------------------------------------------
// client
// ---------------------------------------------------------------------------------------------

/// One panel + one set of credentials. Builds every URL the app needs and fetches JSON.
#[derive(Clone)]
pub struct XtreamClient {
    http: HttpClient,
    base: Url,
    user: String,
    pass: String,
}

impl std::fmt::Debug for XtreamClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XtreamClient")
            .field("base", &self.base_url())
            .field("user", &self.user)
            .field("pass", &"***")
            .finish()
    }
}

/// Turn whatever the user pasted into `scheme://host[:port][/prefix]` with no trailing slash:
/// whitespace trimmed, a missing scheme defaults to `http://`, query/fragment/userinfo dropped,
/// and a trailing `player_api.php` / `get.php` / `xmltv.php` / `panel_api.php` removed.
pub fn normalise_base(input: &str) -> Result<Url> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(NetError::InvalidUrl("Xtream server URL is empty".into()));
    }
    let with_scheme = if raw.contains("://") { raw.to_string() } else { format!("http://{raw}") };
    let mut url = Url::parse(&with_scheme).map_err(|e| NetError::InvalidUrl(redact(&format!("{e}: {with_scheme}"))))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(NetError::InvalidUrl(format!("unsupported scheme {} (need http or https)", url.scheme())));
    }
    if url.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(NetError::InvalidUrl("Xtream server URL has no host".into()));
    }
    url.set_query(None);
    url.set_fragment(None);
    // `http://user:pass@host` — credentials belong in the dedicated fields, never in the URL.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let mut path = url.path().trim_end_matches('/').to_string();
    if let Some((dir, last)) = path.rsplit_once('/') {
        if STRIP_SCRIPTS.iter().any(|s| last.eq_ignore_ascii_case(s)) {
            path = dir.to_string();
        }
    }
    let path = path.trim_end_matches('/');
    url.set_path(if path.is_empty() { "/" } else { path });
    Ok(url)
}

/// Append path segments (percent-encoding each one) after the base path.
fn push_segments(u: &mut Url, segs: &[&str]) {
    if let Ok(mut p) = u.path_segments_mut() {
        p.pop_if_empty().extend(segs);
    }
}

impl XtreamClient {
    pub fn new(base_url: &str, user: &str, pass: &str, user_agent: Option<&str>) -> Result<Self> {
        let base = normalise_base(base_url)?;
        let user = user.trim();
        if user.is_empty() || pass.is_empty() {
            return Err(NetError::Other("Xtream username and password are required".into()));
        }
        Ok(Self { http: HttpClient::new(user_agent)?, base, user: user.to_string(), pass: pass.to_string() })
    }

    /// Normalised base, e.g. `http://host:8080` or `https://host/panel`.
    pub fn base_url(&self) -> &str {
        self.base.as_str().trim_end_matches('/')
    }

    pub fn username(&self) -> &str {
        &self.user
    }

    pub fn user_agent(&self) -> &str {
        self.http.user_agent()
    }

    /// `{base}/player_api.php?username=…&password=…[&action=…][&k=v…]`
    pub fn api_url(&self, action: Option<&str>, extra: &[(&str, &str)]) -> String {
        let mut u = self.base.clone();
        push_segments(&mut u, &["player_api.php"]);
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("username", &self.user).append_pair("password", &self.pass);
            if let Some(a) = action {
                q.append_pair("action", a);
            }
            for (k, v) in extra {
                q.append_pair(k, v);
            }
        }
        u.into()
    }

    /// `{base}/live/{user}/{pass}/{stream_id}.{ext}`
    pub fn live_url(&self, stream_id: &str, ext: &str) -> String {
        self.media_url("live", stream_id, ext)
    }

    /// `{base}/movie/{user}/{pass}/{stream_id}.{ext}`
    pub fn movie_url(&self, stream_id: &str, ext: &str) -> String {
        self.media_url("movie", stream_id, ext)
    }

    /// `{base}/series/{user}/{pass}/{episode_id}.{ext}`
    pub fn series_url(&self, episode_id: &str, ext: &str) -> String {
        self.media_url("series", episode_id, ext)
    }

    /// `{base}/xmltv.php?username=…&password=…`
    pub fn xmltv_url(&self) -> String {
        let mut u = self.base.clone();
        push_segments(&mut u, &["xmltv.php"]);
        u.query_pairs_mut().append_pair("username", &self.user).append_pair("password", &self.pass);
        u.into()
    }

    fn media_url(&self, kind: &str, id: &str, ext: &str) -> String {
        let mut u = self.base.clone();
        let ext = ext.trim().trim_start_matches('.');
        let file = if ext.is_empty() { id.to_string() } else { format!("{id}.{ext}") };
        push_segments(&mut u, &[kind, &self.user, &self.pass, &file]);
        u.into()
    }

    /// GET one API action and decode the JSON body. Non-2xx, HTML bodies, empty bodies and
    /// oversized bodies are errors (redacted). A UTF-8 BOM, leading whitespace and PHP notices
    /// printed before the JSON are tolerated.
    pub async fn get_json(&self, action: Option<&str>, extra: &[(&str, &str)]) -> Result<Value> {
        self.get_json_sized(action, extra).await.map(|(v, _)| v)
    }

    /// [`XtreamClient::get_json`] plus the body size in bytes (for progress reporting).
    pub async fn get_json_sized(&self, action: Option<&str>, extra: &[(&str, &str)]) -> Result<(Value, u64)> {
        let url = self.api_url(action, extra);
        let what = action.unwrap_or("login").to_string();
        // Streamed rather than `get_text`: no overall deadline, only the read timeout, so a
        // 15 MB channel list on a slow link is not killed at 30 s.
        let mut resp = self.http.get_stream(&url).await?;
        let hint = resp.content_length.unwrap_or(0).min(4 << 20) as usize;
        let mut buf: Vec<u8> = Vec::with_capacity(hint);
        while let Some(chunk) = resp.body.next().await {
            let chunk = chunk?;
            if buf.len() + chunk.len() > MAX_JSON_BYTES {
                return Err(NetError::Parse(format!("{what}: response exceeds {} MiB", MAX_JSON_BYTES >> 20)));
            }
            buf.extend_from_slice(&chunk);
        }
        let len = buf.len() as u64;
        let content_type = resp.content_type.take();
        // Big panels return 10+ MB documents; keep the parse off the async threads (§5).
        let value = tokio::task::spawn_blocking(move || decode_json(&buf, content_type.as_deref(), &what))
            .await
            .map_err(|e| NetError::Other(format!("JSON decode task failed: {e}")))??;
        tracing::debug!(action = action.unwrap_or("login"), bytes = len, "xtream api call");
        Ok((value, len))
    }

    pub async fn authenticate(&self) -> Result<XtreamAccount> {
        let v = self.get_json(None, &[]).await?;
        parse_account(&v)
    }

    /// `category_id → category_name` for live streams. Ids are normalised to strings.
    pub async fn live_categories(&self) -> Result<HashMap<String, String>> {
        let v = self.get_json(Some("get_live_categories"), &[]).await?;
        Ok(categories_from(&v))
    }

    pub async fn vod_categories(&self) -> Result<HashMap<String, String>> {
        let v = self.get_json(Some("get_vod_categories"), &[]).await?;
        Ok(categories_from(&v))
    }

    pub async fn series_categories(&self) -> Result<HashMap<String, String>> {
        let v = self.get_json(Some("get_series_categories"), &[]).await?;
        Ok(categories_from(&v))
    }

    /// All live streams as channel rows. `ext` is `ts` or `m3u8`.
    pub async fn live_streams(&self, categories: &HashMap<String, String>, ext: &str) -> Result<Vec<ChannelInsert>> {
        let v = self.get_json(Some("get_live_streams"), &[]).await?;
        Ok(expect_list(&v, "get_live_streams")?
            .into_iter()
            .filter_map(|it| self.map_live_stream(it, categories, ext))
            .collect())
    }

    /// All movies as VOD rows (`kind = "movie"`).
    pub async fn vod_streams(&self, categories: &HashMap<String, String>) -> Result<Vec<VodInsert>> {
        let v = self.get_json(Some("get_vod_streams"), &[]).await?;
        Ok(expect_list(&v, "get_vod_streams")?
            .into_iter()
            .filter_map(|it| self.map_vod_stream(it, categories))
            .collect())
    }

    /// All series as VOD rows (`kind = "series"`, no stream URL — episodes carry those).
    pub async fn series_list(&self, categories: &HashMap<String, String>) -> Result<Vec<VodInsert>> {
        let v = self.get_json(Some("get_series"), &[]).await?;
        Ok(expect_list(&v, "get_series")?.into_iter().filter_map(|it| self.map_series(it, categories)).collect())
    }

    /// `get_series_info`: the series row (from `info`, category unresolved → `None`) and its
    /// episodes sorted by (season, episode).
    pub async fn series_info(&self, series_id: &str) -> Result<(Option<VodInsert>, Vec<EpisodeInsert>)> {
        let v = self.get_json(Some("get_series_info"), &[("series_id", series_id)]).await?;
        Ok(self.map_series_info(series_id, &v))
    }

    // ---- JSON → rows ----------------------------------------------------------------------

    /// One `get_live_streams` entry → channel row. `None` when there is no `stream_id`.
    pub fn map_live_stream(&self, v: &Value, categories: &HashMap<String, String>, ext: &str) -> Option<ChannelInsert> {
        let id = s(v, "stream_id")?;
        let name = s(v, "name").unwrap_or_else(|| id.clone());
        let ext = if ext.trim().is_empty() { "ts" } else { ext };
        Some(ChannelInsert {
            source_id: id.clone(),
            name,
            group_title: Some(category_of(v, categories).unwrap_or_else(|| UNCATEGORIZED.to_string())),
            logo: http_url(s(v, "stream_icon")),
            stream_url: self.live_url(&id, ext),
            tvg_id: s(v, "epg_channel_id"),
            tvg_name: None,
            catchup: truthy(v, "tv_archive"),
            catchup_days: i(v, "tv_archive_duration").unwrap_or(0).clamp(0, 3650) as i32,
        })
    }

    /// One `get_vod_streams` entry → movie row. `None` when there is no `stream_id`.
    pub fn map_vod_stream(&self, v: &Value, categories: &HashMap<String, String>) -> Option<VodInsert> {
        let id = s(v, "stream_id")?;
        let title = s(v, "name").or_else(|| s(v, "title")).unwrap_or_else(|| id.clone());
        let ext = container_ext(v).unwrap_or_else(|| "mp4".into());
        Some(VodInsert {
            kind: "movie".into(),
            source_id: id.clone(),
            title: title.clone(),
            poster: http_url(s(v, "stream_icon").or_else(|| s(v, "cover"))),
            backdrop: backdrop_of(v),
            year: year_of(v, &title),
            tmdb_id: tmdb_of(v),
            category: category_of(v, categories),
            description: s(v, "plot").or_else(|| s(v, "description")),
            rating: rating_of(v),
            genre: s(v, "genre"),
            duration_s: duration_of(v),
            stream_url: Some(self.movie_url(&id, &ext)),
            container_ext: Some(ext),
            added: i(v, "added").filter(|t| *t > 0),
        })
    }

    /// One `get_series` entry → series row. `None` when there is no `series_id`.
    pub fn map_series(&self, v: &Value, categories: &HashMap<String, String>) -> Option<VodInsert> {
        let id = s(v, "series_id")?;
        let title = s(v, "name").or_else(|| s(v, "title")).unwrap_or_else(|| id.clone());
        Some(VodInsert {
            kind: "series".into(),
            source_id: id,
            title: title.clone(),
            poster: http_url(s(v, "cover").or_else(|| s(v, "stream_icon"))),
            backdrop: backdrop_of(v),
            year: year_of(v, &title),
            tmdb_id: tmdb_of(v),
            category: category_of(v, categories),
            description: s(v, "plot"),
            rating: rating_of(v),
            genre: s(v, "genre"),
            duration_s: None,
            stream_url: None,
            container_ext: None,
            added: i(v, "last_modified").filter(|t| *t > 0).or_else(|| i(v, "added").filter(|t| *t > 0)),
        })
    }

    /// `get_series_info` document → (series row from `info`, episodes). `episodes` may be an
    /// object keyed by season number, an array of per-season arrays, or a flat array.
    pub fn map_series_info(&self, series_id: &str, v: &Value) -> (Option<VodInsert>, Vec<EpisodeInsert>) {
        let info = v.get("info").filter(|x| x.is_object()).and_then(|info| {
            let mut obj = info.clone();
            obj["series_id"] = Value::String(series_id.to_string());
            self.map_series(&obj, &HashMap::new())
        });
        let mut eps = Vec::new();
        match v.get("episodes") {
            Some(Value::Object(seasons)) => {
                for (key, list) in seasons {
                    self.collect_episodes(list, key.trim().parse::<i32>().ok(), &mut eps);
                }
            }
            Some(Value::Array(items)) if items.iter().all(Value::is_array) => {
                for (idx, list) in items.iter().enumerate() {
                    self.collect_episodes(list, Some(idx as i32 + 1), &mut eps);
                }
            }
            Some(list @ Value::Array(_)) => self.collect_episodes(list, None, &mut eps),
            _ => {}
        }
        eps.sort_by_key(|e| (e.season, e.episode));
        (info, eps)
    }

    fn collect_episodes(&self, list: &Value, season_hint: Option<i32>, out: &mut Vec<EpisodeInsert>) {
        let items: Vec<&Value> = match list {
            Value::Array(a) => a.iter().collect(),
            Value::Object(o) => o.values().collect(),
            _ => return,
        };
        for (pos, ep) in items.into_iter().enumerate() {
            if let Some(e) = self.map_episode(ep, season_hint, pos as i32 + 1) {
                out.push(e);
            }
        }
    }

    fn map_episode(&self, ep: &Value, season_hint: Option<i32>, position: i32) -> Option<EpisodeInsert> {
        let id = s(ep, "id")?;
        let info = ep.get("info").filter(|x| x.is_object());
        let season = i(ep, "season").and_then(to_i32).or(season_hint).unwrap_or(1);
        let episode = i(ep, "episode_num").and_then(to_i32).unwrap_or(position);
        let ext = container_ext(ep).unwrap_or_else(|| "mp4".into());
        Some(EpisodeInsert {
            source_id: Some(id.clone()),
            season,
            episode,
            title: s(ep, "title"),
            stream_url: self.series_url(&id, &ext),
            duration: info.and_then(duration_of),
            poster: info.and_then(|x| http_url(s(x, "movie_image").or_else(|| s(x, "cover_big")))),
            container_ext: Some(ext),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// JSON helpers — every accessor accepts the "wrong" JSON type because panels do
// ---------------------------------------------------------------------------------------------

/// Decode an API body. Distinguishes HTML (wrong URL / blocked) from malformed JSON.
fn decode_json(body: &[u8], content_type: Option<&str>, what: &str) -> Result<Value> {
    let body = preflight::strip_bom(body);
    let start = body.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(body.len());
    let body = &body[start..];
    if body.is_empty() {
        return Err(NetError::Parse(format!("{what}: panel returned an empty response")));
    }
    if matches!(body[0], b'{' | b'[' | b'"' | b'n' | b't' | b'f' | b'-' | b'0'..=b'9') {
        return parse_value(body).map_err(|e| NetError::Parse(format!("{what}: {e}")));
    }
    if preflight::sniff(body, content_type) == PayloadKind::Html {
        return Err(NetError::Http(format!(
            "{what}: panel returned a web page instead of JSON — wrong URL or blocked"
        )));
    }
    // PHP notices/warnings printed before the JSON: skip to the first bracket.
    if let Some(pos) = body.iter().position(|b| matches!(b, b'{' | b'[')) {
        if let Ok(v) = parse_value(&body[pos..]) {
            tracing::warn!(action = what, skipped = pos, "panel printed text before the JSON body");
            return Ok(v);
        }
    }
    Err(NetError::Parse(format!("{what}: panel did not return JSON (starts with: {})", preflight::preview_of(body))))
}

/// `serde_json` parse with a lossy-UTF-8 retry for Latin-1 panels.
fn parse_value(b: &[u8]) -> std::result::Result<Value, String> {
    match serde_json::from_slice::<Value>(b) {
        Ok(v) => Ok(v),
        Err(e) => {
            if std::str::from_utf8(b).is_err() {
                if let Ok(v) = serde_json::from_str::<Value>(&String::from_utf8_lossy(b)) {
                    return Ok(v);
                }
            }
            Err(redact(&format!("invalid JSON from panel: {e}")))
        }
    }
}

/// The items of a list response. Arrays as-is, `null`/`{}` as empty, an object of objects as
/// its values (some panels key lists by index), and the login-rejected shape as an error.
fn expect_list<'a>(v: &'a Value, what: &str) -> Result<Vec<&'a Value>> {
    match v {
        Value::Array(a) => Ok(a.iter().collect()),
        Value::Null => Ok(Vec::new()),
        Value::Object(o) => {
            if let Some(ui) = o.get("user_info") {
                if !truthy(ui, "auth") {
                    return Err(login_rejected(s(ui, "status")));
                }
            }
            if o.values().all(Value::is_object) {
                Ok(o.values().collect())
            } else {
                Err(NetError::Parse(format!("{what}: expected a JSON array, got an object")))
            }
        }
        other => Err(NetError::Parse(format!("{what}: expected a JSON array, got {}", json_type(other)))),
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// `category_id → category_name` from a `get_*_categories` response.
pub fn categories_from(v: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let items: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.values().collect(),
        _ => Vec::new(),
    };
    for c in items {
        if let (Some(id), Some(name)) = (s(c, "category_id"), s(c, "category_name")) {
            out.insert(id, name);
        }
    }
    out
}

/// String value of `key`, accepting a string or a number. Blank and `"null"` are `None`.
pub fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(value_to_string)
}

/// Integer value of `key`, accepting a number, a numeric string (`"7"`, `"7.0"`) or a bool.
pub fn i(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Value::String(x) => {
            let t = x.trim();
            t.parse::<i64>().ok().or_else(|| t.parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| f as i64))
        }
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// Float value of `key`, accepting a number or a numeric string.
pub fn f(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        Value::String(x) => x.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

/// `1`, `"1"`, `true`, `"true"` → true.
fn truthy(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(x)) if x.trim().eq_ignore_ascii_case("true") => true,
        _ => i(v, key).is_some_and(|n| n != 0),
    }
}

fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(x) => {
            let t = x.trim();
            (!t.is_empty() && !t.eq_ignore_ascii_case("null")).then(|| t.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn to_u32(n: i64) -> Option<u32> {
    u32::try_from(n).ok()
}

fn to_i32(n: i64) -> Option<i32> {
    i32::try_from(n).ok()
}

/// Keep only absolute http(s) URLs; panels send `""`, relative paths and junk.
fn http_url(u: Option<String>) -> Option<String> {
    u.filter(|x| {
        let l = x.to_ascii_lowercase();
        l.starts_with("http://") || l.starts_with("https://")
    })
}

/// Resolve `category_id` (or the first resolvable entry of `category_ids`).
fn category_of(v: &Value, categories: &HashMap<String, String>) -> Option<String> {
    let mut candidates: Vec<String> = s(v, "category_id").into_iter().collect();
    if let Some(Value::Array(ids)) = v.get("category_ids") {
        candidates.extend(ids.iter().filter_map(value_to_string));
    }
    candidates.iter().find_map(|id| categories.get(id).cloned())
}

fn container_ext(v: &Value) -> Option<String> {
    s(v, "container_extension")
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn backdrop_of(v: &Value) -> Option<String> {
    match v.get("backdrop_path")? {
        Value::Array(a) => a.iter().find_map(|x| http_url(value_to_string(x))),
        other => http_url(value_to_string(other)),
    }
}

fn tmdb_of(v: &Value) -> Option<i64> {
    i(v, "tmdb_id").or_else(|| i(v, "tmdb")).filter(|t| *t > 0)
}

/// `rating` (0–10) else `rating_5based × 2`; zero means "unrated".
fn rating_of(v: &Value) -> Option<f64> {
    f(v, "rating")
        .filter(|r| *r > 0.0)
        .or_else(|| f(v, "rating_5based").filter(|r| *r > 0.0).map(|r| r * 2.0))
        .map(|r| (r * 10.0).round() / 10.0)
}

/// `duration_secs` else a `HH:MM:SS` `duration`.
fn duration_of(v: &Value) -> Option<i64> {
    i(v, "duration_secs").filter(|d| *d > 0).or_else(|| s(v, "duration").and_then(|d| parse_hms(&d)))
}

/// `year` field, else `(YYYY)` at the end of the title, else the year in a release date.
fn year_of(v: &Value, title: &str) -> Option<i32> {
    i(v, "year").and_then(valid_year).or_else(|| trailing_year(title)).or_else(|| {
        ["releasedate", "release_date", "releaseDate"].iter().find_map(|k| s(v, k)).and_then(|d| year_from_date(&d))
    })
}

fn valid_year(y: i64) -> Option<i32> {
    YEAR_RANGE.contains(&y).then_some(y as i32)
}

/// `"Heat (1995)"` → 1995.
pub fn trailing_year(name: &str) -> Option<i32> {
    let inner = name.trim_end().strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let digits = inner[open + 1..].trim();
    if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()) {
        valid_year(digits.parse().ok()?)
    } else {
        None
    }
}

/// First standalone 4-digit run in a date string (`2002-06-02`, `02/06/2002`, `2019`).
pub fn year_from_date(d: &str) -> Option<i32> {
    let b = d.as_bytes();
    let mut k = 0;
    while k + 4 <= b.len() {
        let run = b[k..k + 4].iter().all(u8::is_ascii_digit);
        let bounded = (k == 0 || !b[k - 1].is_ascii_digit()) && (k + 4 == b.len() || !b[k + 4].is_ascii_digit());
        if run && bounded {
            return valid_year(d[k..k + 4].parse().ok()?);
        }
        k += 1;
    }
    None
}

/// `"1:02:03"` → 3723, `"58:00"` → 3480. A bare number is ambiguous and gives `None`.
pub fn parse_hms(t: &str) -> Option<i64> {
    let parts: Vec<&str> = t.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut total = 0i64;
    for p in parts {
        let n = p.trim().parse::<i64>().ok()?;
        if n < 0 {
            return None;
        }
        total = total * 60 + n;
    }
    (total > 0).then_some(total)
}

// ---------------------------------------------------------------------------------------------
// adapter
// ---------------------------------------------------------------------------------------------

/// [`CatalogAdapter`] for one Xtream playlist row.
pub struct XtreamAdapter {
    pub db: Arc<Db>,
    pub playlist_id: i64,
    pub client: XtreamClient,
    pub sink: Arc<dyn ProgressSink>,
    /// `"ts"` | `"m3u8"` — extension of generated live URLs.
    pub stream_format: String,
}

impl XtreamAdapter {
    /// Build from the playlist row: `base_url`, `user`, `pass`, `ua` and `stream_format`.
    pub fn from_playlist(db: Arc<Db>, playlist_id: i64, sink: Arc<dyn ProgressSink>) -> Result<Self> {
        let src = db.playlist_source(playlist_id)?;
        if src.r#type != "xtream" {
            return Err(NetError::Other(format!("playlist {playlist_id} is a {} source, not Xtream", src.r#type)));
        }
        let client = XtreamClient::new(
            &src.base_url,
            src.user.as_deref().unwrap_or(""),
            src.pass.as_deref().unwrap_or(""),
            src.ua.as_deref(),
        )?;
        let stream_format = db.playlist_meta(playlist_id).map(|m| m.stream_format).unwrap_or_else(|_| "ts".into());
        Ok(Self { db, playlist_id, client, sink, stream_format })
    }

    /// Extension used for live URLs.
    pub fn live_ext(&self) -> &'static str {
        if self.stream_format.trim().eq_ignore_ascii_case("m3u8") {
            "m3u8"
        } else {
            "ts"
        }
    }

    fn emit(&self, playlist_id: i64, stage: &str, rows: u64, bytes: u64, message: Option<String>) {
        self.sink.progress(ImportProgressEvent { playlist_id, stage: stage.into(), channels: rows, bytes, message });
    }

    /// Wrap a sync pass: emit `error` and record the (redacted) outcome on the playlist row.
    async fn record<F>(&self, playlist_id: i64, what: &str, run: F) -> Result<SyncStats>
    where
        F: std::future::Future<Output = Result<SyncStats>>,
    {
        self.emit(playlist_id, "fetching", 0, 0, None);
        match run.await {
            Ok(stats) => {
                if let Err(e) = self.db.set_playlist_sync_result(playlist_id, None) {
                    tracing::warn!(playlist_id, error = %e, "could not record sync result");
                }
                Ok(stats)
            }
            Err(e) => {
                let msg = redact(&e.to_string());
                tracing::warn!(playlist_id, what, error = %msg, "xtream sync failed");
                self.emit(playlist_id, "error", 0, 0, Some(msg.clone()));
                let _ = self.db.set_playlist_sync_result(playlist_id, Some(&msg));
                Err(e)
            }
        }
    }

    async fn run_live(&self, playlist_id: i64) -> Result<SyncStats> {
        let started = Instant::now();
        let mut stats = SyncStats::default();
        let mut bytes = 0u64;

        let (cats, n) = self.client.get_json_sized(Some("get_live_categories"), &[]).await?;
        bytes += n;
        let categories = categories_from(&cats);
        let (doc, n) = self.client.get_json_sized(Some("get_live_streams"), &[]).await?;
        bytes += n;
        self.emit(playlist_id, "parsing", 0, bytes, None);

        let ext = self.live_ext();
        let mut seen: HashSet<String> = HashSet::new();
        let mut groups: HashSet<String> = HashSet::new();
        let mut rows: Vec<ChannelInsert> = Vec::new();
        for item in expect_list(&doc, "get_live_streams")? {
            match self.client.map_live_stream(item, &categories, ext) {
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
        drop(doc);
        let total = rows.len() as u64;

        if rows.is_empty() {
            // A transient empty answer must not wipe a catalog the user relies on.
            stats.warnings.push("panel returned no live streams; existing channels were kept".into());
        } else {
            let sync_gen = self.db.next_sync_gen(playlist_id)?;
            let mut done = 0u64;
            for chunk in rows.chunks(BATCH) {
                let (ins, upd) = self.db.upsert_channels_gen(playlist_id, chunk, Some(sync_gen))?;
                stats.inserted += ins;
                stats.updated += upd;
                done += chunk.len() as u64;
                self.emit(playlist_id, "indexing", done, bytes, Some(format!("{}%", (done * 100 / total).min(99))));
                tokio::task::yield_now().await;
            }
            let removed = self.db.delete_stale_channels(playlist_id, sync_gen)?;
            if removed > 0 {
                stats.warnings.push(format!("removed {removed} channels no longer on the panel"));
            }
        }
        let _ = self.db.checkpoint();

        stats.groups = groups.len() as u64;
        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        if categories.is_empty() {
            stats.warnings.push("panel returned no live categories".into());
        }
        if stats.skipped > 0 {
            stats.warnings.push(format!("{} streams skipped (no stream_id or duplicate)", stats.skipped));
        }
        tracing::info!(playlist_id, ?stats, bytes, "xtream live sync complete");
        self.emit(playlist_id, "done", total, bytes, None);
        Ok(stats)
    }

    async fn run_vod(&self, playlist_id: i64) -> Result<SyncStats> {
        let started = Instant::now();
        let mut stats = SyncStats::default();
        let mut bytes = 0u64;

        let (cats, n) = self.client.get_json_sized(Some("get_vod_categories"), &[]).await?;
        bytes += n;
        let movie_cats = categories_from(&cats);
        let (movies, n) = self.client.get_json_sized(Some("get_vod_streams"), &[]).await?;
        bytes += n;
        let (cats, n) = self.client.get_json_sized(Some("get_series_categories"), &[]).await?;
        bytes += n;
        let series_cats = categories_from(&cats);
        let (series, n) = self.client.get_json_sized(Some("get_series"), &[]).await?;
        bytes += n;
        self.emit(playlist_id, "parsing", 0, bytes, None);

        let mut groups: HashSet<String> = HashSet::new();
        let mut rows: Vec<VodInsert> = Vec::new();
        let mut movie_count = 0u64;
        for item in expect_list(&movies, "get_vod_streams")? {
            match self.client.map_vod_stream(item, &movie_cats) {
                Some(row) => {
                    if let Some(c) = &row.category {
                        if !groups.contains(c) {
                            groups.insert(c.clone());
                        }
                    }
                    rows.push(row);
                    movie_count += 1;
                }
                None => stats.skipped += 1,
            }
        }
        for item in expect_list(&series, "get_series")? {
            match self.client.map_series(item, &series_cats) {
                Some(row) => {
                    if let Some(c) = &row.category {
                        if !groups.contains(c) {
                            groups.insert(c.clone());
                        }
                    }
                    rows.push(row);
                }
                None => stats.skipped += 1,
            }
        }
        drop(movies);
        drop(series);
        let total = rows.len() as u64;
        let series_count = total - movie_count;

        let mut done = 0u64;
        for chunk in rows.chunks(BATCH) {
            let (ins, upd) = self.db.upsert_vod(playlist_id, chunk)?;
            stats.inserted += ins;
            stats.updated += upd;
            done += chunk.len() as u64;
            self.emit(playlist_id, "indexing", done, bytes, Some(format!("{}%", (done * 100 / total.max(1)).min(99))));
            tokio::task::yield_now().await;
        }
        let _ = self.db.checkpoint();

        stats.groups = groups.len() as u64;
        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        if total == 0 {
            stats.warnings.push("panel returned no movies or series".into());
        }
        if stats.skipped > 0 {
            stats.warnings.push(format!("{} VOD entries skipped (no id)", stats.skipped));
        }
        tracing::info!(
            playlist_id,
            ?stats,
            movies = movie_count,
            series = series_count,
            bytes,
            "xtream vod sync complete"
        );
        self.emit(playlist_id, "done", total, bytes, None);
        Ok(stats)
    }
}

#[async_trait]
impl CatalogAdapter for XtreamAdapter {
    async fn authenticate(&self) -> Result<AccountInfo> {
        match self.client.authenticate().await {
            Ok(acct) => {
                let json = serde_json::to_string(&acct).map_err(|e| NetError::Other(e.to_string()))?;
                self.db.set_playlist_account_json(self.playlist_id, Some(&json))?;
                tracing::info!(
                    playlist_id = self.playlist_id,
                    status = ?acct.status,
                    max_connections = ?acct.max_connections,
                    exp_date = ?acct.exp_date,
                    "xtream login ok"
                );
                Ok(acct.to_account_info())
            }
            Err(e) => {
                let msg = redact(&e.to_string());
                tracing::warn!(playlist_id = self.playlist_id, error = %msg, "xtream login failed");
                let _ = self.db.set_playlist_sync_result(self.playlist_id, Some(&msg));
                Err(e)
            }
        }
    }

    async fn sync_live(&self, playlist_id: i64) -> Result<SyncStats> {
        self.record(playlist_id, "live", self.run_live(playlist_id)).await
    }

    async fn sync_vod(&self, playlist_id: i64) -> Result<SyncStats> {
        self.record(playlist_id, "vod", self.run_vod(playlist_id)).await
    }

    async fn sync_epg(&self, playlist_id: i64) -> Result<SyncStats> {
        let source =
            ImportSource::Url { url: self.client.xmltv_url(), user_agent: Some(self.client.user_agent().to_string()) };
        import_xmltv(self.db.clone(), playlist_id, source, self.sink.clone(), XmltvOptions::default()).await
    }
}

// ---------------------------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use app_db::channels::PlaylistInsert;
    use serde_json::json;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn client(base: &str) -> XtreamClient {
        XtreamClient::new(base, "u", "p", None).unwrap()
    }

    fn cats() -> HashMap<String, String> {
        categories_from(&json!([
            {"category_id": "1", "category_name": "UK", "parent_id": 0},
            {"category_id": 2, "category_name": "Sports", "parent_id": 0},
            {"category_id": null, "category_name": "broken"},
            {"category_id": "3", "category_name": ""}
        ]))
    }

    // ---- 1. URL builders ------------------------------------------------------------------

    #[test]
    fn normalises_pasted_base_urls() {
        for (input, want) in [
            ("http://host:8080", "http://host:8080"),
            ("http://host:8080/", "http://host:8080"),
            ("  https://host/path/  ", "https://host/path"),
            ("http://host:8080/player_api.php?username=a&password=b", "http://host:8080"),
            ("http://host/get.php?username=u&password=p&type=m3u_plus&output=ts", "http://host"),
            ("http://host/sub/PLAYER_API.PHP", "http://host/sub"),
            ("http://host/xmltv.php?username=a&password=b", "http://host"),
            ("http://host/panel_api.php", "http://host"),
            ("host:8080", "http://host:8080"),
            ("HTTP://Host/#frag", "http://host"),
            ("http://user:pw@host/", "http://host"),
            ("http://host/player_api.php/", "http://host"),
        ] {
            assert_eq!(client(input).base_url(), want, "{input}");
        }
        for bad in ["", "   ", "ftp://host/", "rtmp://x/y", "http://", "://nope"] {
            assert!(XtreamClient::new(bad, "u", "p", None).is_err(), "{bad}");
        }
        assert!(XtreamClient::new("http://h", "", "p", None).is_err());
        assert!(XtreamClient::new("http://h", "u", "", None).is_err());
        let err = XtreamClient::new("ftp://h/get.php?password=zzz", "u", "p", None).unwrap_err().to_string();
        assert!(!err.contains("zzz"), "{err}");
    }

    #[test]
    fn builds_api_and_media_urls() {
        let c = client("http://host:8080/");
        assert_eq!(c.api_url(None, &[]), "http://host:8080/player_api.php?username=u&password=p");
        assert_eq!(
            c.api_url(Some("get_live_streams"), &[]),
            "http://host:8080/player_api.php?username=u&password=p&action=get_live_streams"
        );
        assert_eq!(
            c.api_url(Some("get_series_info"), &[("series_id", "12")]),
            "http://host:8080/player_api.php?username=u&password=p&action=get_series_info&series_id=12"
        );
        assert_eq!(c.live_url("101", "ts"), "http://host:8080/live/u/p/101.ts");
        assert_eq!(c.live_url("101", ".m3u8"), "http://host:8080/live/u/p/101.m3u8");
        assert_eq!(c.live_url("101", ""), "http://host:8080/live/u/p/101");
        assert_eq!(c.movie_url("5", "mkv"), "http://host:8080/movie/u/p/5.mkv");
        assert_eq!(c.series_url("9", "mp4"), "http://host:8080/series/u/p/9.mp4");
        assert_eq!(c.xmltv_url(), "http://host:8080/xmltv.php?username=u&password=p");

        // A panel under a sub-path keeps its prefix, even when the pasted URL was the API script.
        let c = client("http://host/panel/player_api.php?username=x&password=y");
        assert_eq!(c.api_url(None, &[]), "http://host/panel/player_api.php?username=u&password=p");
        assert_eq!(c.live_url("1", "ts"), "http://host/panel/live/u/p/1.ts");
        assert_eq!(c.xmltv_url(), "http://host/panel/xmltv.php?username=u&password=p");

        // Credentials are encoded (never split the path) and the redactor still masks them.
        let c = XtreamClient::new("http://host", " a b ", "c/d&e", None).unwrap();
        assert_eq!(c.live_url("1", "ts"), "http://host/live/a%20b/c%2Fd&e/1.ts");
        assert_eq!(c.api_url(None, &[]), "http://host/player_api.php?username=a+b&password=c%2Fd%26e");
        assert_eq!(redact(&c.live_url("1", "ts")), "http://host/live/***/***/1.ts");
        assert_eq!(redact(&c.api_url(None, &[])), "http://host/player_api.php?username=a+b&password=***");
        let dbg = format!("{c:?}");
        assert!(
            dbg.contains("a b") && dbg.contains("***") && !dbg.contains("c/d"),
            "Debug must mask the password: {dbg}"
        );
    }

    // ---- 2. mapping -----------------------------------------------------------------------

    #[test]
    fn categories_tolerate_ids_as_strings_or_numbers() {
        let m = cats();
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m["1"], "UK");
        assert_eq!(m["2"], "Sports");
        assert!(categories_from(&json!(null)).is_empty());
        assert!(categories_from(&json!("nope")).is_empty());
        // index-keyed object form
        let m = categories_from(&json!({"0": {"category_id": 7, "category_name": "Kids"}}));
        assert_eq!(m["7"], "Kids");
    }

    #[test]
    fn maps_live_streams_with_mixed_types() {
        let c = client("http://h");
        let m = cats();
        let full = json!({
            "num": 1, "name": " BBC One HD ", "stream_type": "live", "stream_id": 101,
            "stream_icon": "http://img/1.png", "epg_channel_id": "bbc1.uk", "added": "1600000000",
            "category_id": "1", "custom_sid": "", "tv_archive": 1, "direct_source": "", "tv_archive_duration": "7"
        });
        assert_eq!(
            c.map_live_stream(&full, &m, "ts").unwrap(),
            ChannelInsert {
                source_id: "101".into(),
                name: "BBC One HD".into(),
                group_title: Some("UK".into()),
                logo: Some("http://img/1.png".into()),
                stream_url: "http://h/live/u/p/101.ts".into(),
                tvg_id: Some("bbc1.uk".into()),
                tvg_name: None,
                catchup: true,
                catchup_days: 7,
            }
        );
        let sparse = json!({"name": "ITV", "stream_id": "102", "stream_icon": "", "epg_channel_id": "", "category_id": 2, "tv_archive": "0", "tv_archive_duration": null});
        let ch = c.map_live_stream(&sparse, &m, "m3u8").unwrap();
        assert_eq!(ch.source_id, "102");
        assert_eq!(ch.group_title.as_deref(), Some("Sports"));
        assert_eq!(ch.logo, None, "empty icon");
        assert_eq!(ch.tvg_id, None, "empty epg id");
        assert!(!ch.catchup);
        assert_eq!(ch.catchup_days, 0);
        assert_eq!(ch.stream_url, "http://h/live/u/p/102.m3u8");

        // unknown category_id → falls back to category_ids, relative icon dropped, string bool
        let odd = json!({"name": "ESPN", "stream_id": 103, "stream_icon": "/rel.png", "epg_channel_id": null, "category_id": "99", "category_ids": [2], "tv_archive": "true"});
        let ch = c.map_live_stream(&odd, &m, "ts").unwrap();
        assert_eq!(ch.group_title.as_deref(), Some("Sports"));
        assert_eq!(ch.logo, None);
        assert!(ch.catchup);

        let unknown = json!({"name": "Mystery", "stream_id": 104, "category_id": "nope"});
        assert_eq!(c.map_live_stream(&unknown, &m, "ts").unwrap().group_title.as_deref(), Some(UNCATEGORIZED));
        let unnamed = json!({"stream_id": 7});
        assert_eq!(c.map_live_stream(&unnamed, &m, "ts").unwrap().name, "7");
        assert!(c.map_live_stream(&json!({"name": "no id"}), &m, "ts").is_none());
        assert!(c.map_live_stream(&json!({"name": "blank id", "stream_id": ""}), &m, "ts").is_none());
        assert_eq!(c.map_live_stream(&json!({"stream_id": 1}), &m, "").unwrap().stream_url, "http://h/live/u/p/1.ts");
    }

    #[test]
    fn maps_vod_streams() {
        let c = client("http://h");
        let m = categories_from(&json!([{"category_id": "10", "category_name": "Action"}]));
        let heat = json!({
            "num": 1, "name": "Heat (1995)", "stream_type": "movie", "stream_id": 5001,
            "stream_icon": "http://img/heat.jpg", "rating": "8.3", "rating_5based": 4.1, "added": "1690000000",
            "category_id": "10", "container_extension": "MKV", "custom_sid": null, "direct_source": ""
        });
        let v = c.map_vod_stream(&heat, &m).unwrap();
        assert_eq!(v.kind, "movie");
        assert_eq!(v.source_id, "5001");
        assert_eq!(v.title, "Heat (1995)");
        assert_eq!(v.year, Some(1995), "year parsed from the title");
        assert_eq!(v.rating, Some(8.3));
        assert_eq!(v.category.as_deref(), Some("Action"));
        assert_eq!(v.poster.as_deref(), Some("http://img/heat.jpg"));
        assert_eq!(v.container_ext.as_deref(), Some("mkv"));
        assert_eq!(v.stream_url.as_deref(), Some("http://h/movie/u/p/5001.mkv"));
        assert_eq!(v.added, Some(1_690_000_000));
        assert_eq!((v.description, v.genre, v.duration_s, v.backdrop), (None, None, None, None));

        let inception = json!({
            "name": "Inception", "stream_id": "5002", "stream_icon": "", "rating": 0, "rating_5based": 4.4,
            "added": 1690000001, "category_id": 11, "container_extension": null, "year": "2010",
            "plot": "Dreams", "genre": "Sci-Fi", "duration_secs": "8880", "tmdb_id": "27205",
            "backdrop_path": ["http://img/inc_bd.jpg"]
        });
        let v = c.map_vod_stream(&inception, &m).unwrap();
        assert_eq!(v.year, Some(2010), "year field wins");
        assert_eq!(v.rating, Some(8.8), "rating 0 → rating_5based × 2");
        assert_eq!(v.category, None, "unknown category → None for VOD");
        assert_eq!(v.poster, None);
        assert_eq!(v.backdrop.as_deref(), Some("http://img/inc_bd.jpg"));
        assert_eq!(v.container_ext.as_deref(), Some("mp4"), "default container");
        assert_eq!(v.stream_url.as_deref(), Some("http://h/movie/u/p/5002.mp4"));
        assert_eq!(v.description.as_deref(), Some("Dreams"));
        assert_eq!(v.genre.as_deref(), Some("Sci-Fi"));
        assert_eq!(v.duration_s, Some(8880));
        assert_eq!(v.tmdb_id, Some(27205));

        let dated = json!({"name": "Old", "stream_id": 3, "releasedate": "1999-12-31", "rating": "", "rating_5based": "0", "duration": "01:30:00", "year": 0});
        let v = c.map_vod_stream(&dated, &m).unwrap();
        assert_eq!(v.year, Some(1999), "release date fallback");
        assert_eq!(v.rating, None, "unrated");
        assert_eq!(v.duration_s, Some(5400), "HH:MM:SS duration");
        assert!(c.map_vod_stream(&json!({"name": "no id"}), &m).is_none());
        assert_eq!(c.map_vod_stream(&json!({"stream_id": 9, "title": "T"}), &m).unwrap().title, "T");
    }

    #[test]
    fn maps_series_list() {
        let c = client("http://h");
        let m = categories_from(&json!([{"category_id": 20, "category_name": "Drama"}]));
        let wire = json!({
            "num": 1, "name": "The Wire", "series_id": 1, "cover": "http://img/wire.jpg", "plot": "Baltimore",
            "cast": "x", "director": "y", "genre": "Crime", "releaseDate": "2002-06-02", "last_modified": "1680000000",
            "rating": "9", "rating_5based": 4.5, "backdrop_path": ["", "http://img/wire_bd.jpg"], "youtube_trailer": "",
            "episode_run_time": "60", "category_id": "20"
        });
        let v = c.map_series(&wire, &m).unwrap();
        assert_eq!(v.kind, "series");
        assert_eq!(v.source_id, "1");
        assert_eq!(v.title, "The Wire");
        assert_eq!(v.poster.as_deref(), Some("http://img/wire.jpg"));
        assert_eq!(v.backdrop.as_deref(), Some("http://img/wire_bd.jpg"), "first usable entry of the array");
        assert_eq!(v.year, Some(2002));
        assert_eq!(v.rating, Some(9.0));
        assert_eq!(v.category.as_deref(), Some("Drama"));
        assert_eq!(v.description.as_deref(), Some("Baltimore"));
        assert_eq!(v.genre.as_deref(), Some("Crime"));
        assert_eq!(v.added, Some(1_680_000_000));
        assert_eq!((v.stream_url, v.container_ext, v.duration_s), (None, None, None));

        let odd = json!({"name": "Show (2019)", "series_id": "2", "backdrop_path": "http://img/bd.jpg", "release_date": "", "last_modified": "0"});
        let v = c.map_series(&odd, &m).unwrap();
        assert_eq!(v.year, Some(2019));
        assert_eq!(v.backdrop.as_deref(), Some("http://img/bd.jpg"), "backdrop as a plain string");
        assert_eq!(v.added, None);
        assert_eq!(
            c.map_series(&json!({"name": "x", "series_id": 3, "backdrop_path": []}), &m).unwrap().backdrop,
            None
        );
        assert!(c.map_series(&json!({"name": "no id"}), &m).is_none());
    }

    fn series_info_fixture() -> Value {
        json!({
            "seasons": [{"season_number": 1}, {"season_number": 2}],
            "info": {
                "name": "The Wire", "cover": "http://img/wire.jpg", "plot": "Baltimore", "genre": "Crime",
                "releaseDate": "2002-06-02", "rating": "9", "backdrop_path": ["http://img/wire_bd.jpg"], "category_id": "20"
            },
            "episodes": {
                "1": [
                    {"id": "9001", "episode_num": 1, "title": "The Target", "container_extension": "mkv", "season": 1,
                     "info": {"duration_secs": 3600, "duration": "01:00:00", "movie_image": "http://img/e1.jpg", "plot": "p"}},
                    {"id": 9002, "episode_num": "2", "title": "The Detail", "container_extension": "mp4",
                     "info": {"duration": "00:58:00", "movie_image": ""}}
                ],
                "2": [
                    {"id": "9003", "episode_num": 1, "title": "Ebb Tide", "container_extension": "mkv", "season": "2"}
                ]
            }
        })
    }

    #[test]
    fn maps_series_info_episodes_as_object() {
        let c = client("http://h");
        let (info, eps) = c.map_series_info("1", &series_info_fixture());
        let info = info.unwrap();
        assert_eq!((info.kind.as_str(), info.source_id.as_str(), info.title.as_str()), ("series", "1", "The Wire"));
        assert_eq!(info.year, Some(2002));
        assert_eq!(info.category, None, "series_info cannot resolve the category name");
        assert_eq!(eps.len(), 3);
        assert_eq!(
            eps[0],
            EpisodeInsert {
                source_id: Some("9001".into()),
                season: 1,
                episode: 1,
                title: Some("The Target".into()),
                stream_url: "http://h/series/u/p/9001.mkv".into(),
                duration: Some(3600),
                poster: Some("http://img/e1.jpg".into()),
                container_ext: Some("mkv".into()),
            }
        );
        assert_eq!((eps[1].season, eps[1].episode), (1, 2), "season from the object key");
        assert_eq!(eps[1].source_id.as_deref(), Some("9002"), "numeric id");
        assert_eq!(eps[1].duration, Some(3480), "HH:MM:SS fallback");
        assert_eq!(eps[1].poster, None);
        assert_eq!(eps[1].stream_url, "http://h/series/u/p/9002.mp4");
        assert_eq!((eps[2].season, eps[2].episode), (2, 1));
    }

    #[test]
    fn maps_series_info_episodes_as_arrays() {
        let c = client("http://h");
        // array of per-season arrays (season from position), episode_num missing (from position)
        let doc = json!({
            "info": {"name": "S"},
            "episodes": [
                [{"id": 1, "title": "a"}, {"id": 2, "title": "b"}],
                [{"id": 3, "title": "c", "episode_num": 5}]
            ]
        });
        let (info, eps) = c.map_series_info("77", &doc);
        assert_eq!(info.unwrap().source_id, "77");
        let shape: Vec<(i32, i32, String)> = eps.iter().map(|e| (e.season, e.episode, e.stream_url.clone())).collect();
        assert_eq!(
            shape,
            vec![
                (1, 1, "http://h/series/u/p/1.mp4".to_string()),
                (1, 2, "http://h/series/u/p/2.mp4".to_string()),
                (2, 5, "http://h/series/u/p/3.mp4".to_string()),
            ]
        );
        // flat array with explicit seasons, out of order → sorted; entries without id dropped
        let doc = json!({"episodes": [
            {"id": "b", "season": 2, "episode_num": 1}, {"id": "a", "season": 1, "episode_num": 3}, {"title": "no id"}
        ]});
        let (info, eps) = c.map_series_info("1", &doc);
        assert!(info.is_none());
        assert_eq!(eps.iter().map(|e| (e.season, e.episode)).collect::<Vec<_>>(), vec![(1, 3), (2, 1)]);
        // object keyed by season whose values are index-keyed objects
        let doc = json!({"episodes": {"3": {"0": {"id": 10, "episode_num": 1}}}});
        let (_, eps) = c.map_series_info("1", &doc);
        assert_eq!((eps[0].season, eps[0].episode, eps[0].source_id.as_deref()), (3, 1, Some("10")));
        assert!(c.map_series_info("1", &json!({"episodes": null})).1.is_empty());
        assert!(c.map_series_info("1", &json!([])).1.is_empty());
    }

    #[test]
    fn parses_account_variants() {
        let full = json!({
            "user_info": {
                "username": "bob", "password": "hunter2", "message": "hi", "auth": 1, "status": "Active",
                "exp_date": "1893456000", "is_trial": "0", "active_cons": "1", "created_at": "1600000000",
                "max_connections": "2", "allowed_output_formats": ["m3u8", "ts", "rtmp"]
            },
            "server_info": {
                "url": "panel.example", "port": "8080", "https_port": "443", "server_protocol": "http",
                "rtmp_port": "1935", "timezone": "Europe/London", "timestamp_now": 1700000000, "time_now": "2023-11-14 22:13:20"
            }
        });
        let a = parse_account(&full).unwrap();
        assert_eq!(a.username, "bob");
        assert_eq!(a.status.as_deref(), Some("Active"));
        assert_eq!(a.exp_date, Some(1_893_456_000));
        assert!(!a.is_trial);
        assert_eq!((a.active_cons, a.max_connections), (Some(1), Some(2)));
        assert_eq!(a.created_at, Some(1_600_000_000));
        assert_eq!(a.allowed_output_formats, vec!["m3u8", "ts", "rtmp"]);
        assert_eq!(a.server_url.as_deref(), Some("http://panel.example:8080"));
        assert_eq!(a.timezone.as_deref(), Some("Europe/London"));
        assert_eq!(a.server_time, Some(1_700_000_000));
        assert!(a.auth);
        let js = serde_json::to_string(&a).unwrap();
        assert!(!js.contains("hunter2") && !js.contains("\"password\""), "{js}");

        let minimal = json!({"user_info": {"auth": "1", "is_trial": 1, "exp_date": null, "max_connections": "unlimited"}, "server_info": {"url": "h", "server_protocol": "https", "https_port": 443}});
        let a = parse_account(&minimal).unwrap();
        assert!(a.auth && a.is_trial);
        assert_eq!((a.exp_date, a.max_connections, a.status), (None, None, None));
        assert_eq!(a.server_url.as_deref(), Some("https://h:443"));
        assert_eq!(parse_account(&json!({"user_info": {"auth": true}})).unwrap().server_url, None);
        assert_eq!(
            parse_account(&json!({"user_info": {"auth": 1, "allowed_output_formats": "ts, m3u8"}}))
                .unwrap()
                .allowed_output_formats,
            vec!["ts", "m3u8"]
        );

        for rejected in [
            json!({"user_info": {"auth": 0, "status": "Disabled", "password": "hunter2"}}),
            json!({"user_info": {"auth": "0"}}),
            json!({"user_info": {"status": "Expired"}}),
            json!({"user_info": null}),
            json!({}),
            json!([]),
        ] {
            let err = parse_account(&rejected).unwrap_err();
            let msg = err.to_string();
            assert!(matches!(err, NetError::Http(_)), "{rejected}");
            assert!(msg.contains("login rejected"), "{msg}");
            assert!(!msg.contains("hunter2"), "{msg}");
        }
        let msg = parse_account(&json!({"user_info": {"auth": 0, "status": "Expired"}})).unwrap_err().to_string();
        assert!(msg.contains("Expired"), "{msg}");
    }

    #[test]
    fn scalar_helpers() {
        let v = json!({"a": "7", "b": 7, "c": "7.9", "d": 7.9, "e": "", "f": "null", "g": null, "h": true, "k": " x "});
        assert_eq!((i(&v, "a"), i(&v, "b"), i(&v, "c"), i(&v, "d")), (Some(7), Some(7), Some(7), Some(7)));
        assert_eq!((i(&v, "e"), i(&v, "f"), i(&v, "g"), i(&v, "h"), i(&v, "zz")), (None, None, None, Some(1), None));
        assert_eq!((f(&v, "c"), f(&v, "d"), f(&v, "a"), f(&v, "e")), (Some(7.9), Some(7.9), Some(7.0), None));
        assert_eq!((s(&v, "a"), s(&v, "b"), s(&v, "d")), (Some("7".into()), Some("7".into()), Some("7.9".into())));
        assert_eq!(
            (s(&v, "e"), s(&v, "f"), s(&v, "g"), s(&v, "h"), s(&v, "k")),
            (None, None, None, None, Some("x".into()))
        );
        assert_eq!(trailing_year("Heat (1995)"), Some(1995));
        assert_eq!(trailing_year("Heat (1995) "), Some(1995));
        assert_eq!(trailing_year("Heat (1995) HD"), None);
        assert_eq!(trailing_year("(1995)"), Some(1995));
        assert_eq!(trailing_year("Room (104)"), None);
        assert_eq!(trailing_year("Blade Runner (2049)"), Some(2049));
        assert_eq!(trailing_year("Year (9999)"), None);
        assert_eq!(year_from_date("2002-06-02"), Some(2002));
        assert_eq!(year_from_date("02/06/2002"), Some(2002));
        assert_eq!(year_from_date("20020602"), None, "8 digits is not a year");
        assert_eq!(year_from_date(""), None);
        assert_eq!(parse_hms("1:02:03"), Some(3723));
        assert_eq!(parse_hms("00:58:00"), Some(3480));
        assert_eq!(parse_hms("45"), None);
        assert_eq!(parse_hms("00:00:00"), None);
        assert_eq!(parse_hms("a:b"), None);
    }

    #[test]
    fn decodes_bodies_defensively() {
        assert_eq!(decode_json(b"\xEF\xBB\xBF \n[1]", None, "x").unwrap(), json!([1]));
        assert_eq!(decode_json(b"null", None, "x").unwrap(), Value::Null);
        assert_eq!(
            decode_json(b"<br /><b>Warning</b>: foo in bar.php on line 1<br />{\"a\":1}", None, "x").unwrap(),
            json!({"a": 1})
        );
        assert_eq!(
            decode_json(b"{\"n\":\"caf\xE9\"}", None, "x").unwrap()["n"],
            json!("caf\u{FFFD}"),
            "Latin-1 retried lossily"
        );
        let err = decode_json(
            b"<!DOCTYPE html><html><body>password=zzz</body></html>",
            Some("text/html"),
            "get_live_streams",
        )
        .unwrap_err();
        assert!(matches!(err, NetError::Http(ref m) if m.contains("web page")), "{err:?}");
        let err = decode_json(b"", None, "login").unwrap_err();
        assert!(matches!(err, NetError::Parse(ref m) if m.contains("empty")), "{err:?}");
        let err = decode_json(b"garbage password=zzz", None, "login").unwrap_err();
        assert!(
            matches!(err, NetError::Parse(ref m) if m.contains("did not return JSON") && !m.contains("zzz")),
            "{err:?}"
        );
        let err = decode_json(b"{\"a\": [1, 2", None, "login").unwrap_err();
        assert!(matches!(err, NetError::Parse(ref m) if m.contains("invalid JSON")), "{err:?}");
        assert!(expect_list(&json!({"user_info": {"auth": 0}}), "x").is_err());
        assert!(expect_list(&json!("str"), "x").is_err());
        assert_eq!(expect_list(&json!({}), "x").unwrap().len(), 0);
        assert_eq!(expect_list(&json!({"0": {"a": 1}, "1": {"b": 2}}), "x").unwrap().len(), 2);
    }

    // ---- 3. end-to-end against a mock panel ----------------------------------------------

    const USER: &str = "u";
    const PASS: &str = "s3cret";

    #[derive(Default)]
    struct MockState {
        live: Vec<Value>,
        html: bool,
        hits: Vec<String>,
        /// Start of the XMLTV fixture's first programme (unix); fixed once per test so the
        /// assertions and `XmltvOptions::default()`'s now-relative window cannot race an hour boundary.
        epg_hour: i64,
    }

    fn fresh_state() -> Arc<Mutex<MockState>> {
        let now = now_unix();
        Arc::new(Mutex::new(MockState { live: live_fixture(), epg_hour: now - now % 3600, ..Default::default() }))
    }

    struct Collect(Mutex<Vec<ImportProgressEvent>>);
    impl ProgressSink for Collect {
        fn progress(&self, ev: ImportProgressEvent) {
            self.0.lock().unwrap().push(ev);
        }
    }

    fn live_fixture() -> Vec<Value> {
        vec![
            json!({"num": 1, "name": "BBC One HD", "stream_type": "live", "stream_id": 101, "stream_icon": "http://img/bbc1.png",
                   "epg_channel_id": "bbc1.uk", "added": "1600000000", "category_id": "1", "tv_archive": 1, "tv_archive_duration": 7}),
            json!({"num": 2, "name": "ITV", "stream_type": "live", "stream_id": "102", "stream_icon": "", "epg_channel_id": "",
                   "added": "1600000000", "category_id": 1, "tv_archive": "0", "tv_archive_duration": "0"}),
            json!({"num": 3, "name": "ESPN", "stream_id": 103, "stream_icon": "/relative.png", "epg_channel_id": null,
                   "category_id": "2", "tv_archive": 0}),
        ]
    }

    /// Unix → `YYYYMMDDHHMMSS` (UTC) for the XMLTV fixture.
    fn fmt_ts(t: i64) -> String {
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
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

    fn now_unix() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn xmltv_fixture(hour: i64) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tv>\n<channel id=\"bbc1.uk\"><display-name>BBC One</display-name></channel>\n\
             <programme start=\"{} +0000\" stop=\"{} +0000\" channel=\"bbc1.uk\"><title>Now Show</title></programme>\n\
             <programme start=\"{} +0000\" stop=\"{} +0000\" channel=\"bbc1.uk\"><title>Next Show</title></programme>\n</tv>\n",
            fmt_ts(hour),
            fmt_ts(hour + 3600),
            fmt_ts(hour + 3600),
            fmt_ts(hour + 7200)
        )
    }

    fn route(state: &Mutex<MockState>, target: &str) -> (&'static str, &'static str, String) {
        let url = Url::parse(&format!("http://mock{target}")).unwrap();
        let q: HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let mut st = state.lock().unwrap();
        st.hits.push(redact(target));
        if st.html {
            return (
                "200 OK",
                "text/html",
                "<!DOCTYPE html><html><head><title>Login</title></head><body>nope</body></html>".into(),
            );
        }
        let json = |v: Value| ("200 OK", "application/json", v.to_string());
        match url.path() {
            "/player_api.php" => {
                if q.get("username").map(String::as_str) != Some(USER)
                    || q.get("password").map(String::as_str) != Some(PASS)
                {
                    // Real panels echo the credentials back on a failed login.
                    return json(
                        json!({"user_info": {"auth": 0, "status": "Disabled", "username": q.get("username"), "password": q.get("password")}, "server_info": {}}),
                    );
                }
                match q.get("action").map(String::as_str) {
                    None => json(json!({
                        "user_info": {"username": USER, "password": PASS, "auth": 1, "status": "Active", "exp_date": "1893456000",
                                      "is_trial": "0", "active_cons": "0", "created_at": "1600000000", "max_connections": "2",
                                      "allowed_output_formats": ["m3u8", "ts"]},
                        "server_info": {"url": "127.0.0.1", "port": "80", "https_port": "443", "server_protocol": "http",
                                        "timezone": "UTC", "timestamp_now": 1700000000}
                    })),
                    Some("get_live_categories") => json(json!([
                        {"category_id": "1", "category_name": "UK", "parent_id": 0},
                        {"category_id": 2, "category_name": "Sports", "parent_id": 0}
                    ])),
                    Some("get_live_streams") => json(Value::Array(st.live.clone())),
                    Some("get_vod_categories") => json(json!([
                        {"category_id": "10", "category_name": "Action"}, {"category_id": "11", "category_name": "Sci-Fi"}
                    ])),
                    Some("get_vod_streams") => json(json!([
                        {"num": 1, "name": "Heat (1995)", "stream_id": 5001, "stream_icon": "http://img/heat.jpg", "rating": "8.3",
                         "rating_5based": 4.1, "added": "1690000000", "category_id": "10", "container_extension": "mkv"},
                        {"num": 2, "name": "Inception", "stream_id": "5002", "stream_icon": "http://img/inc.jpg", "rating": 0,
                         "rating_5based": 0, "added": 1690000001, "category_id": 11, "container_extension": "mp4", "year": "2010",
                         "plot": "Dreams", "genre": "Sci-Fi", "duration_secs": "8880"}
                    ])),
                    Some("get_series_categories") => json(json!([{"category_id": 20, "category_name": "Drama"}])),
                    Some("get_series") => json(json!([
                        {"num": 1, "name": "The Wire", "series_id": 1, "cover": "http://img/wire.jpg", "plot": "Baltimore",
                         "genre": "Crime", "releaseDate": "2002-06-02", "last_modified": "1680000000", "rating": "9",
                         "rating_5based": 4.5, "backdrop_path": ["http://img/wire_bd.jpg"], "category_id": "20"}
                    ])),
                    Some("get_series_info") if q.get("series_id").map(String::as_str) == Some("1") => {
                        json(series_info_fixture())
                    }
                    Some(_) => json(json!([])),
                }
            }
            "/xmltv.php" => ("200 OK", "application/xml", xmltv_fixture(st.epg_hour)),
            _ => ("404 Not Found", "text/plain", "not here".into()),
        }
    }

    /// Minimal HTTP/1.1 responder on 127.0.0.1: reads the request head, routes on path + query,
    /// answers with `Connection: close`.
    async fn spawn_mock(state: Arc<Mutex<MockState>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let state = state.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    loop {
                        let n = match sock.read(&mut tmp).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
                            break;
                        }
                    }
                    let head = String::from_utf8_lossy(&buf);
                    let target =
                        head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
                    let (status, ctype, body) = route(&state, &target);
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.write_all(body.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn new_xtream_playlist(db: &Db, base: &str, pass: &str) -> i64 {
        db.insert_playlist(&PlaylistInsert {
            r#type: "xtream".into(),
            name: "mock".into(),
            base_url: base.into(),
            user: Some(USER.into()),
            pass: Some(pass.into()),
            mac: None,
            ua: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn end_to_end_against_mock_panel() {
        let state = fresh_state();
        let base = spawn_mock(state.clone()).await;
        let db = Arc::new(Db::open_in_memory().unwrap());
        // Pasted with a trailing script + query to prove normalisation end to end.
        let p = new_xtream_playlist(&db, &format!("{base}/player_api.php?username=x&password=y"), PASS);
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let adapter = XtreamAdapter::from_playlist(db.clone(), p, sink.clone()).unwrap();
        assert_eq!(adapter.client.base_url(), base);
        assert_eq!(adapter.live_ext(), "ts");

        // ---- authenticate ----
        let info = adapter.authenticate().await.unwrap();
        assert_eq!(info.status.as_deref(), Some("Active"));
        assert_eq!(info.max_connections, Some(2));
        assert_eq!(info.active_connections, Some(0));
        assert_eq!(info.exp_date, Some(1_893_456_000));
        assert_eq!(info.server_timezone.as_deref(), Some("UTC"));
        let meta = db.playlist_meta(p).unwrap();
        let json = meta.account_json.expect("account_json stored");
        assert!(json.contains("\"username\":\"u\""), "{json}");
        assert!(json.contains("\"status\":\"Active\""), "{json}");
        assert!(json.contains("\"server_url\":\"http://127.0.0.1:80\""), "{json}");
        assert!(!json.contains("\"password\""), "{json}");
        assert!(!json.contains(PASS), "{json}");
        let acct: XtreamAccount = serde_json::from_str(&json).unwrap();
        assert_eq!(acct.allowed_output_formats, vec!["m3u8", "ts"]);

        // ---- live ----
        let stats = adapter.sync_live(p).await.unwrap();
        assert_eq!((stats.inserted, stats.updated, stats.skipped, stats.groups), (3, 0, 0, 2), "{stats:?}");
        assert!(stats.warnings.is_empty(), "{:?}", stats.warnings);
        let rows = db.list_channels(p, None, 10, 0).unwrap();
        assert_eq!(rows.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["BBC One HD", "ITV", "ESPN"]);
        assert_eq!(
            rows.iter().map(|c| c.group_title.as_deref()).collect::<Vec<_>>(),
            [Some("UK"), Some("UK"), Some("Sports")]
        );
        assert_eq!(rows[0].stream_url, format!("{base}/live/{USER}/{PASS}/101.ts"));
        assert_eq!(rows[1].stream_url, format!("{base}/live/{USER}/{PASS}/102.ts"));
        assert_eq!(rows.iter().map(|c| c.source_id.as_str()).collect::<Vec<_>>(), ["101", "102", "103"]);
        assert_eq!(rows.iter().map(|c| c.tvg_id.as_deref()).collect::<Vec<_>>(), [Some("bbc1.uk"), None, None]);
        assert_eq!(rows.iter().map(|c| c.catchup_days).collect::<Vec<_>>(), [7, 0, 0]);
        assert_eq!(
            rows.iter().map(|c| c.logo.as_deref()).collect::<Vec<_>>(),
            [Some("http://img/bbc1.png"), None, None]
        );
        assert_eq!(db.channel_count(p, Some("UK")).unwrap(), 2);
        let meta = db.playlist_meta(p).unwrap();
        assert!(meta.last_synced.is_some() && meta.last_error.is_none(), "{meta:?}");
        {
            let events = sink.0.lock().unwrap();
            let stages: Vec<&str> = events.iter().map(|e| e.stage.as_str()).collect();
            assert_eq!(stages, ["fetching", "parsing", "indexing", "done"], "{stages:?}");
            assert!(events.iter().all(|e| e.playlist_id == p));
            assert_eq!(events.last().unwrap().channels, 3);
            assert!(events.last().unwrap().bytes > 0);
            assert!(!events.iter().any(|e| e.message.as_deref().is_some_and(|m| m.contains(PASS))));
        }

        // Switching the stream format changes generated URLs on the next sync (rows updated in place).
        db.set_playlist_stream_format(p, "m3u8").unwrap();
        let adapter = XtreamAdapter::from_playlist(db.clone(), p, sink.clone()).unwrap();
        assert_eq!(adapter.live_ext(), "m3u8");
        let stats = adapter.sync_live(p).await.unwrap();
        assert_eq!((stats.inserted, stats.updated), (0, 3), "{stats:?}");
        let rows = db.list_channels(p, None, 10, 0).unwrap();
        assert!(rows.iter().all(|c| c.stream_url.ends_with(".m3u8")), "{rows:?}");
        assert_eq!(db.channel_count(p, None).unwrap(), 3);

        // ---- vod ----
        let stats = adapter.sync_vod(p).await.unwrap();
        assert_eq!((stats.inserted, stats.updated, stats.skipped, stats.groups), (3, 0, 0, 3), "{stats:?}");
        assert_eq!(db.vod_count(p, "movie", None).unwrap(), 2);
        assert_eq!(db.vod_count(p, "series", None).unwrap(), 1);
        let movies = db.list_vod(p, "movie", None, "title", 10, 0).unwrap();
        let heat = &movies[0];
        assert_eq!(heat.title, "Heat (1995)");
        assert_eq!(heat.year, Some(1995));
        assert_eq!(heat.rating, Some(8.3));
        assert_eq!(heat.category.as_deref(), Some("Action"));
        assert_eq!(heat.container_ext.as_deref(), Some("mkv"));
        assert_eq!(heat.stream_url.as_deref(), Some(format!("{base}/movie/{USER}/{PASS}/5001.mkv").as_str()));
        let inc = &movies[1];
        assert_eq!((inc.title.as_str(), inc.year, inc.rating), ("Inception", Some(2010), None));
        assert_eq!(inc.category.as_deref(), Some("Sci-Fi"));
        assert_eq!(inc.description.as_deref(), Some("Dreams"));
        assert_eq!(inc.duration_s, Some(8880));
        assert_eq!(inc.stream_url.as_deref(), Some(format!("{base}/movie/{USER}/{PASS}/5002.mp4").as_str()));
        let series = db.list_vod(p, "series", None, "title", 10, 0).unwrap();
        let wire = &series[0];
        assert_eq!((wire.title.as_str(), wire.source_id.as_str(), wire.year), ("The Wire", "1", Some(2002)));
        assert_eq!(wire.category.as_deref(), Some("Drama"));
        assert_eq!(wire.poster.as_deref(), Some("http://img/wire.jpg"));
        assert_eq!(wire.backdrop.as_deref(), Some("http://img/wire_bd.jpg"));
        assert_eq!(wire.added, Some(1_680_000_000));
        assert_eq!(wire.stream_url, None);
        assert!(wire.episodes_synced.is_none());
        assert_eq!(db.search_vod("wire", p, Some("series"), 10).unwrap().len(), 1, "FTS row present");
        // VOD sync is idempotent.
        let stats = adapter.sync_vod(p).await.unwrap();
        assert_eq!((stats.inserted, stats.updated), (0, 3), "{stats:?}");

        // ---- episodes (lazy, via the client) ----
        let (info, eps) = adapter.client.series_info(&wire.source_id).await.unwrap();
        assert_eq!(info.unwrap().title, "The Wire");
        assert_eq!(eps.len(), 3);
        assert_eq!(eps[0].stream_url, format!("{base}/series/{USER}/{PASS}/9001.mkv"));
        assert_eq!(db.upsert_episodes(wire.id, &eps).unwrap(), 3);
        let stored = db.episodes(wire.id).unwrap();
        assert_eq!(stored.iter().map(|e| (e.season, e.episode)).collect::<Vec<_>>(), [(1, 1), (1, 2), (2, 1)]);
        assert_eq!(stored[1].duration, Some(3480));
        assert!(db.get_vod(wire.id).unwrap().episodes_synced.is_some());
        assert!(adapter.client.series_info("404").await.unwrap().1.is_empty(), "unknown series → empty");

        // ---- epg ----
        let stats = adapter.sync_epg(p).await.unwrap();
        assert_eq!(stats.inserted, 2, "{stats:?}");
        assert_eq!(db.epg_stats(p).unwrap().programmes, 2);
        let hour = state.lock().unwrap().epg_hour;
        let (now, next) = db.now_next(p, "bbc1.uk", hour + 1800).unwrap();
        assert_eq!(now.unwrap().title, "Now Show");
        assert_eq!(next.unwrap().title, "Next Show");

        // ---- stale channel removal ----
        state.lock().unwrap().live.retain(|v| v["name"] != "ITV");
        let stats = adapter.sync_live(p).await.unwrap();
        assert_eq!((stats.inserted, stats.updated, stats.groups), (0, 2, 2), "{stats:?}");
        assert!(
            stats.warnings.iter().any(|w| w == "removed 1 channels no longer on the panel"),
            "{:?}",
            stats.warnings
        );
        let rows = db.list_channels(p, None, 10, 0).unwrap();
        assert_eq!(rows.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["BBC One HD", "ESPN"]);
        assert_eq!(db.search_count("itv", p).unwrap(), 0, "FTS row removed too");

        // An empty panel answer keeps the catalog instead of wiping it.
        state.lock().unwrap().live.clear();
        let stats = adapter.sync_live(p).await.unwrap();
        assert_eq!(stats.inserted + stats.updated, 0);
        assert!(stats.warnings.iter().any(|w| w.contains("existing channels were kept")), "{:?}", stats.warnings);
        assert_eq!(db.channel_count(p, None).unwrap(), 2);

        // Every request the mock saw carried the credentials, and every one redacts cleanly.
        let hits = state.lock().unwrap().hits.clone();
        assert!(hits.len() >= 12, "{hits:?}");
        assert!(hits.iter().all(|h| h.contains("password=***") && !h.contains(PASS)), "{hits:?}");
        assert!(hits.iter().any(|h| h.starts_with("/xmltv.php?")), "{hits:?}");
        assert!(hits.iter().any(|h| h.contains("action=get_series_info&series_id=1")), "{hits:?}");
    }

    /// `sync_epg` drives the (non-`Send`) XMLTV future through `Handle::block_on` on a blocking
    /// thread; the app runs a multi-thread runtime, so prove the pattern there too (the other
    /// tests use tokio's current-thread test runtime).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_epg_runs_on_multi_thread_runtime() {
        let state = fresh_state();
        let base = spawn_mock(state.clone()).await;
        let db = Arc::new(Db::open_in_memory().unwrap());
        let p = new_xtream_playlist(&db, &base, PASS);
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let adapter = XtreamAdapter::from_playlist(db.clone(), p, sink.clone()).unwrap();
        // Run it from a spawned task as the Tauri command layer would.
        let stats = tokio::spawn(async move { adapter.sync_epg(p).await }).await.unwrap().unwrap();
        assert_eq!(stats.inserted, 2, "{stats:?}");
        assert_eq!(db.epg_stats(p).unwrap().programmes, 2);
        let stages: Vec<String> = sink.0.lock().unwrap().iter().map(|e| e.stage.clone()).collect();
        assert_eq!(stages.first().map(String::as_str), Some("fetching"));
        assert_eq!(stages.last().map(String::as_str), Some("done"));
        let hits = state.lock().unwrap().hits.clone();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].starts_with("/xmltv.php?username=u&password=***"), "{hits:?}");
    }

    #[tokio::test]
    async fn rejected_login_and_bad_panels_never_leak_the_password() {
        let state = fresh_state();
        let base = spawn_mock(state.clone()).await;
        let db = Arc::new(Db::open_in_memory().unwrap());

        // auth=0 (the mock echoes the wrong password back in its JSON, like real panels do)
        let p = new_xtream_playlist(&db, &base, "wrongpass");
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let adapter = XtreamAdapter::from_playlist(db.clone(), p, sink.clone()).unwrap();
        let err = adapter.authenticate().await.unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, NetError::Http(_)), "{err:?}");
        assert!(msg.contains("login rejected") && msg.contains("Disabled"), "{msg}");
        assert!(!msg.contains("wrongpass"), "{msg}");
        let meta = db.playlist_meta(p).unwrap();
        assert!(meta.account_json.is_none());
        assert!(
            meta.last_error.as_deref().is_some_and(|e| e.contains("login rejected") && !e.contains("wrongpass")),
            "{meta:?}"
        );
        // list actions with bad credentials surface the same rejection, and the error event is clean
        let err = adapter.sync_live(p).await.unwrap_err().to_string();
        assert!(err.contains("login rejected") && !err.contains("wrongpass"), "{err}");
        let last = sink.0.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.stage, "error");
        assert!(!last.message.unwrap().contains("wrongpass"));
        assert_eq!(db.channel_count(p, None).unwrap(), 0);

        // wrong path → HTTP 404 with a redacted URL
        let c = XtreamClient::new(&format!("{base}/nope"), USER, "wrongpass", None).unwrap();
        let err = c.authenticate().await.unwrap_err().to_string();
        assert!(err.contains("HTTP 404"), "{err}");
        assert!(err.contains("password=***") && !err.contains("wrongpass"), "{err}");

        // HTML instead of JSON (login page / blocked)
        state.lock().unwrap().html = true;
        let c = XtreamClient::new(&base, USER, PASS, None).unwrap();
        let err = c.live_categories().await.unwrap_err();
        assert!(matches!(err, NetError::Http(ref m) if m.contains("web page instead of JSON")), "{err:?}");
        assert!(!err.to_string().contains(PASS));
        let err = adapter.sync_vod(p).await.unwrap_err().to_string();
        assert!(err.contains("web page") && !err.contains("wrongpass"), "{err}");

        // wrong playlist type / missing credentials are refused before any HTTP
        let m3u = db
            .insert_playlist(&PlaylistInsert {
                r#type: "m3u".into(),
                name: "x".into(),
                base_url: "http://h/list.m3u".into(),
                user: None,
                pass: None,
                mac: None,
                ua: None,
            })
            .unwrap();
        assert!(XtreamAdapter::from_playlist(db.clone(), m3u, Arc::new(crate::importer::NoopSink)).is_err());
        let nocreds = db
            .insert_playlist(&PlaylistInsert {
                r#type: "xtream".into(),
                name: "x".into(),
                base_url: base.clone(),
                user: Some(USER.into()),
                pass: None,
                mac: None,
                ua: None,
            })
            .unwrap();
        assert!(XtreamAdapter::from_playlist(db.clone(), nocreds, Arc::new(crate::importer::NoopSink)).is_err());
        assert!(XtreamAdapter::from_playlist(db, 9_999, Arc::new(crate::importer::NoopSink)).is_err());
    }
}
