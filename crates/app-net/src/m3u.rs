//! Streaming, push-based M3U/M3U8 parser (CLAUDE.md §7.2).
//!
//! Feed it byte chunks as they arrive; it keeps at most one partial line in memory and emits
//! complete entries. Handles `#EXTM3U` header attrs, `#EXTINF` attrs + title (commas inside
//! quoted attribute values are safe), `#EXTGRP`, `#EXTVLCOPT` / `#KODIPROP` per-entry hints,
//! UTF-8 BOM, CRLF, and headerless lists.

use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct M3uHeader {
    /// `url-tvg` / `x-tvg-url` — EPG hint from the playlist itself.
    pub tvg_url: Option<String>,
    pub attrs: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct M3uEntry {
    pub line_no: u64,
    pub title: String,
    pub url: String,
    pub duration: f64,
    pub tvg_id: Option<String>,
    pub tvg_name: Option<String>,
    pub tvg_logo: Option<String>,
    pub group_title: Option<String>,
    pub tvg_chno: Option<String>,
    pub catchup: bool,
    pub catchup_days: i32,
    /// `catchup=` attr value when meaningful: default | append | shift | flussonic | xc | vod…
    pub catchup_kind: Option<String>,
    /// `catchup-source=` template (may embed credentials).
    pub catchup_source: Option<String>,
    /// `#EXTVLCOPT:http-user-agent=` / `#KODIPROP:` hints, keyed by option name.
    pub options: HashMap<String, String>,
    pub extra_attrs: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub struct M3uParser {
    buf: Vec<u8>,
    line_no: u64,
    header: M3uHeader,
    saw_header: bool,
    pending: Option<M3uEntry>,
    pending_group: Option<String>,
    pending_options: HashMap<String, String>,
    bom_checked: bool,
    pub stats: ParseStats,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParseStats {
    pub entries: u64,
    pub entries_without_extinf: u64,
    pub skipped_lines: u64,
    pub bytes: u64,
}

/// Cap on a single line to survive hostile input (a 40 MB "line" is not a playlist).
const MAX_LINE_BYTES: usize = 64 * 1024;

impl M3uParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn header(&self) -> &M3uHeader {
        &self.header
    }

    /// Feed a chunk. Complete entries are pushed to `out`.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<M3uEntry>) {
        self.stats.bytes += chunk.len() as u64;
        let mut chunk = chunk;
        if !self.bom_checked && self.buf.is_empty() {
            // Only the very first bytes can carry a BOM. If the chunk is shorter than 3 bytes
            // we defer the check to the buffered path below.
            if chunk.len() >= 3 {
                chunk = crate::preflight::strip_bom(chunk);
                self.bom_checked = true;
            }
        }
        let mut start = 0usize;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'\n' {
                if self.buf.is_empty() {
                    self.handle_line_bytes(&chunk[start..i], out);
                } else {
                    self.buf.extend_from_slice(&chunk[start..i]);
                    let line = std::mem::take(&mut self.buf);
                    self.handle_line_bytes(&line, out);
                }
                start = i + 1;
            }
        }
        if start < chunk.len() {
            if self.buf.len() + (chunk.len() - start) <= MAX_LINE_BYTES {
                self.buf.extend_from_slice(&chunk[start..]);
            } else {
                // Discard an absurd line; keep the parser alive.
                self.buf.clear();
                self.stats.skipped_lines += 1;
            }
        }
    }

    /// Flush the last unterminated line.
    pub fn finish(&mut self, out: &mut Vec<M3uEntry>) {
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            self.handle_line_bytes(&line, out);
        }
        // A trailing #EXTINF without URL is dropped.
        self.pending = None;
    }

    fn handle_line_bytes(&mut self, raw: &[u8], out: &mut Vec<M3uEntry>) {
        self.line_no += 1;
        let raw = if !self.bom_checked {
            self.bom_checked = true;
            crate::preflight::strip_bom(raw)
        } else {
            raw
        };
        let line = String::from_utf8_lossy(raw);
        let line = line.trim_end_matches(['\r', '\n']).trim();
        if line.is_empty() {
            return;
        }
        if let Some(rest) = line.strip_prefix('#') {
            self.handle_directive(rest, out);
        } else {
            self.handle_url(line, out);
        }
    }

    fn handle_directive(&mut self, rest: &str, _out: &mut Vec<M3uEntry>) {
        // Directive name is the leading [A-Za-z0-9-] run; the value follows a ':' or whitespace.
        let name_end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(rest.len());
        let name = &rest[..name_end];
        let value = rest[name_end..].strip_prefix(':').unwrap_or(&rest[name_end..]);
        let name_upper = name.to_ascii_uppercase();
        match name_upper.as_str() {
            "EXTM3U" => {
                self.saw_header = true;
                // `#EXTM3U url-tvg="..." x-tvg-url="..."` — attrs follow the tag.
                let (attrs, _) = parse_attrs_and_title(value);
                self.header.tvg_url =
                    attrs.get("url-tvg").or_else(|| attrs.get("x-tvg-url")).cloned().filter(|s| !s.is_empty());
                self.header.attrs = attrs;
            }
            "EXTINF" => {
                let (duration, tail) = split_duration(value);
                let (attrs, title) = parse_attrs_and_title(tail);
                let mut entry = M3uEntry { line_no: self.line_no, duration, title, ..Default::default() };
                apply_attrs(&mut entry, attrs);
                self.pending = Some(entry);
            }
            "EXTGRP" => {
                let g = value.trim();
                if !g.is_empty() {
                    self.pending_group = Some(g.to_string());
                }
            }
            "EXTVLCOPT" | "KODIPROP" | "EXTHTTP" => {
                let v = value.trim();
                if let Some(i) = v.find('=') {
                    let k = v[..i].trim().to_ascii_lowercase();
                    let val = v[i + 1..].trim().trim_matches('"').to_string();
                    // normalise the two we actually use
                    let k = match k.as_str() {
                        "http-user-agent" | "inputstream.adaptive.stream_headers" => "user-agent".to_string(),
                        "http-referrer" | "http-referer" => "referrer".to_string(),
                        other => other.to_string(),
                    };
                    self.pending_options.insert(k, val);
                } else if name_upper == "EXTHTTP" {
                    // #EXTHTTP:{"User-Agent":"x"} — JSON headers
                    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(v) {
                        for (k, val) in map {
                            if let Some(s) = val.as_str() {
                                self.pending_options.insert(k.to_ascii_lowercase(), s.to_string());
                            }
                        }
                    }
                }
            }
            _ => {
                // #EXT-X-* HLS tags inside a media playlist, comments, etc.
                self.stats.skipped_lines += 1;
            }
        }
    }

    fn handle_url(&mut self, url: &str, out: &mut Vec<M3uEntry>) {
        let mut entry = match self.pending.take() {
            Some(e) => e,
            None => {
                self.stats.entries_without_extinf += 1;
                M3uEntry { line_no: self.line_no, title: title_from_url(url), duration: -1.0, ..Default::default() }
            }
        };
        entry.url = url.to_string();
        if entry.group_title.is_none() {
            entry.group_title = self.pending_group.clone();
        }
        if !self.pending_options.is_empty() {
            entry.options = std::mem::take(&mut self.pending_options);
        }
        if entry.title.is_empty() {
            entry.title = entry.tvg_name.clone().unwrap_or_else(|| title_from_url(url));
        }
        self.stats.entries += 1;
        out.push(entry);
    }
}

fn split_duration(s: &str) -> (f64, &str) {
    let s = s.trim_start();
    let end = s.find([' ', ',']).unwrap_or(s.len());
    let dur = s[..end].trim().parse::<f64>().unwrap_or(-1.0);
    (dur, &s[end..])
}

/// Parse `key="value" key2=value ,Title` → (attrs, title). Title is everything after the first
/// comma that sits outside quotes. Keys are lowercased.
pub fn parse_attrs_and_title(s: &str) -> (HashMap<String, String>, String) {
    let mut attrs = HashMap::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut title_start: Option<usize> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b' ' || c == b'\t' {
            i += 1;
            continue;
        }
        if c == b',' {
            title_start = Some(i + 1);
            break;
        }
        // key
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b',' && bytes[i] != b' ' {
            i += 1;
        }
        let key = s[key_start..i].trim().to_ascii_lowercase();
        if i >= bytes.len() || bytes[i] != b'=' {
            // bare token without '=' — treat as start of title if it precedes a comma later
            if i < bytes.len() && bytes[i] == b',' {
                title_start = Some(i + 1);
                break;
            }
            continue;
        }
        i += 1; // skip '='
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let vs = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let v = &s[vs..i.min(bytes.len())];
            i += 1; // closing quote
            v.to_string()
        } else {
            let vs = i;
            while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b',' {
                i += 1;
            }
            s[vs..i].to_string()
        };
        if !key.is_empty() {
            attrs.insert(key, value);
        }
    }
    let title = title_start.map(|t| s[t.min(s.len())..].trim().to_string()).unwrap_or_default();
    (attrs, title)
}

fn apply_attrs(e: &mut M3uEntry, mut attrs: HashMap<String, String>) {
    let take = |attrs: &mut HashMap<String, String>, k: &str| attrs.remove(k).filter(|v| !v.trim().is_empty());
    e.tvg_id = take(&mut attrs, "tvg-id");
    e.tvg_name = take(&mut attrs, "tvg-name");
    e.tvg_logo = take(&mut attrs, "tvg-logo");
    e.group_title = take(&mut attrs, "group-title");
    e.tvg_chno = take(&mut attrs, "tvg-chno");
    let catchup_kind = take(&mut attrs, "catchup").or_else(|| take(&mut attrs, "catchup-type"));
    e.catchup_source = take(&mut attrs, "catchup-source");
    let days = ["catchup-days", "timeshift", "tvg-rec"]
        .iter()
        .filter_map(|k| take(&mut attrs, k))
        .filter_map(|v| v.trim().parse::<i32>().ok())
        .max()
        .unwrap_or(0);
    e.catchup_days = days.max(0);
    e.catchup = catchup_kind.as_deref().map(|k| k != "0" && !k.is_empty()).unwrap_or(false)
        || e.catchup_days > 0
        || e.catchup_source.is_some();
    e.catchup_kind = catchup_kind.map(|k| k.trim().to_lowercase()).filter(|k| k != "0" && !k.is_empty());
    e.extra_attrs = attrs;
}

fn title_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or(path);
    let last = last.rsplit_once('.').map(|(a, _)| a).unwrap_or(last);
    if last.is_empty() {
        url.to_string()
    } else {
        last.to_string()
    }
}

/// Convenience for tests / files that fit in memory.
pub fn parse_all(bytes: &[u8]) -> (M3uHeader, Vec<M3uEntry>, ParseStats) {
    let mut p = M3uParser::new();
    let mut out = Vec::new();
    p.feed(bytes, &mut out);
    p.finish(&mut out);
    (p.header.clone(), out, p.stats.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}#EXTM3U url-tvg=\"http://h/xmltv.php?username=u&password=p\"\r\n\
#EXTINF:-1 tvg-id=\"espn.us\" tvg-name=\"ESPN\" tvg-logo=\"http://l/espn.png\" group-title=\"US | Sports, Live\" catchup=\"default\" catchup-days=\"7\",US: ESPN HD, the worldwide leader\r\n\
http://h/live/u/p/1.ts\r\n\
#EXTGRP:Kids\r\n\
#EXTVLCOPT:http-user-agent=Mozilla/5.0\r\n\
#EXTINF:-1,Pokémon TV\r\n\
http://h/live/u/p/2.m3u8\r\n\
#EXTINF:0 tvg-id=x group-title=Movies timeshift=\"3\",Bare attrs\r\n\
http://h/movie/u/p/3.mkv\r\n\
\r\n\
http://h/orphan/4.ts\r\n\
#EXT-X-SOMETHING:ignored\r\n\
#EXTINF:-1,Trailing without url";

    #[test]
    fn parses_sample_whole() {
        let (hdr, entries, stats) = parse_all(SAMPLE.as_bytes());
        assert_eq!(hdr.tvg_url.as_deref(), Some("http://h/xmltv.php?username=u&password=p"));
        assert_eq!(entries.len(), 4);
        let e = &entries[0];
        assert_eq!(e.title, "US: ESPN HD, the worldwide leader");
        assert_eq!(e.tvg_id.as_deref(), Some("espn.us"));
        assert_eq!(e.group_title.as_deref(), Some("US | Sports, Live"));
        assert!(e.catchup);
        assert_eq!(e.catchup_days, 7);
        assert_eq!(e.url, "http://h/live/u/p/1.ts");
        let e = &entries[1];
        assert_eq!(e.title, "Pokémon TV");
        assert_eq!(e.group_title.as_deref(), Some("Kids"), "#EXTGRP applies");
        assert_eq!(e.options.get("user-agent").map(String::as_str), Some("Mozilla/5.0"));
        let e = &entries[2];
        assert_eq!(e.tvg_id.as_deref(), Some("x"));
        assert_eq!(e.group_title.as_deref(), Some("Movies"));
        assert_eq!(e.catchup_days, 3);
        assert!(e.catchup);
        let e = &entries[3];
        assert_eq!(e.title, "4");
        assert_eq!(e.group_title.as_deref(), Some("Kids"), "sticky EXTGRP until replaced");
        assert_eq!(stats.entries, 4);
        assert_eq!(stats.entries_without_extinf, 1);
    }

    #[test]
    fn identical_when_streamed_in_tiny_chunks() {
        let (_, whole, _) = parse_all(SAMPLE.as_bytes());
        for chunk_size in [1usize, 2, 3, 7, 64, 1000] {
            let mut p = M3uParser::new();
            let mut out = Vec::new();
            for c in SAMPLE.as_bytes().chunks(chunk_size) {
                p.feed(c, &mut out);
            }
            p.finish(&mut out);
            assert_eq!(out, whole, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn attrs_edge_cases() {
        let (a, t) = parse_attrs_and_title(r#" tvg-id="a,b" group-title="G",Title, with comma"#);
        assert_eq!(a["tvg-id"], "a,b");
        assert_eq!(a["group-title"], "G");
        assert_eq!(t, "Title, with comma");
        let (a, t) = parse_attrs_and_title(",Just a title");
        assert!(a.is_empty());
        assert_eq!(t, "Just a title");
        let (a, t) = parse_attrs_and_title(r#" tvg-logo="http://x/a.png" ,  Spaced "#);
        assert_eq!(a["tvg-logo"], "http://x/a.png");
        assert_eq!(t, "Spaced");
        let (_, t) = parse_attrs_and_title("");
        assert_eq!(t, "");
    }

    #[test]
    fn absurd_line_is_skipped_not_fatal() {
        let mut p = M3uParser::new();
        let mut out = Vec::new();
        p.feed(b"#EXTM3U\n", &mut out);
        let huge = vec![b'x'; MAX_LINE_BYTES + 10];
        p.feed(&huge, &mut out);
        p.feed(b"\n#EXTINF:-1,ok\nhttp://x/1.ts\n", &mut out);
        p.finish(&mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "ok");
    }
}
