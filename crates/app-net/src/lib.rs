//! `app-net` — HTTP + source adapters + streaming importer (CLAUDE.md §7).
//!
//! Rules:
//! * The UI never calls HTTP. Everything goes through here, behind typed IPC.
//! * Custom UA, explicit timeouts, redirect limit, rustls.
//! * Playlists are **streamed**: bytes → lines → entries → 5k-row SQLite chunks. A 40 MB M3U
//!   never lives in memory as a whole, on any thread.
//! * The first 512 bytes are sniffed. HTML login pages and XMLTV files are rejected with a
//!   diagnostic instead of being parsed as playlists.
//! * Every error string that can reach the UI is passed through [`app_core::redact::redact`].

pub mod adapters;
pub mod http;
pub mod importer;
pub mod m3u;
pub mod preflight;
pub mod xmltv;

pub use http::HttpClient;
pub use importer::{import_m3u, open_source, ImportSource, ProgressSink};

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("http: {0}")]
    Http(String),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("{0}")]
    Preflight(#[from] preflight::PreflightError),
    #[error("parse: {0}")]
    Parse(String),
    #[error("db: {0}")]
    Db(#[from] app_db::DbError),
    #[error("io: {0}")]
    Io(String),
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for NetError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors embed the URL (with query string) — redact before it can be logged.
        NetError::Http(app_core::redact::redact(&e.to_string()))
    }
}

impl From<std::io::Error> for NetError {
    fn from(e: std::io::Error) -> Self {
        NetError::Io(app_core::redact::redact(&e.to_string()))
    }
}

pub type Result<T> = std::result::Result<T, NetError>;
