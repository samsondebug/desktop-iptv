//! Shared reqwest client: custom UA, timeouts, redirect limit, rustls (ring provider).

use crate::{NetError, Result};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use std::sync::Once;
use std::time::Duration;

pub const DEFAULT_USER_AGENT: &str = concat!("desktop-iptv/", env!("CARGO_PKG_VERSION"), " (libmpv)");
pub const MAX_REDIRECTS: usize = 5;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-request overall timeout for metadata calls (Xtream JSON etc.). Streaming bodies use
/// a read-timeout instead so a slow 40 MB playlist is not killed mid-download.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const READ_TIMEOUT: Duration = Duration::from_secs(20);

static INSTALL_PROVIDER: Once = Once::new();

fn install_crypto_provider() {
    INSTALL_PROVIDER.call_once(|| {
        // reqwest is built with `rustls-no-provider`; pick ring explicitly so Windows builds
        // never need CMake/NASM (aws-lc-rs).
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[derive(Clone, Debug)]
pub struct HttpClient {
    inner: reqwest::Client,
    user_agent: String,
    kind: &'static str,
}

pub struct StreamedResponse {
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub redirect_hops: usize,
    pub body: std::pin::Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
}

impl HttpClient {
    pub fn new(user_agent: Option<&str>) -> Result<Self> {
        install_crypto_provider();
        let ua = user_agent.filter(|s| !s.trim().is_empty()).unwrap_or(DEFAULT_USER_AGENT).to_string();
        let inner = reqwest::Client::builder()
            .user_agent(ua.clone())
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // Record every hop for the diagnostics trace, then apply the usual limit.
                crate::trace::note_redirect(attempt.previous(), attempt.url());
                if attempt.previous().len() > MAX_REDIRECTS {
                    attempt.error(format!("too many redirects (>{MAX_REDIRECTS})"))
                } else {
                    attempt.follow()
                }
            }))
            .gzip(true)
            .use_rustls_tls()
            .build()?;
        Ok(Self { inner, user_agent: ua, kind: "http" })
    }

    /// Label requests from this client in the diagnostics trace (`playlist`, `xtream-api`, …).
    pub fn with_kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn raw(&self) -> &reqwest::Client {
        &self.inner
    }

    /// GET with a streamed body. Redirect count is approximated by comparing URLs
    /// (reqwest does not expose the hop list directly).
    pub async fn get_stream(&self, url: &str) -> Result<StreamedResponse> {
        let parsed = url::Url::parse(url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(NetError::InvalidUrl(format!("unsupported scheme {}", parsed.scheme())));
        }
        let pending = crate::trace::start(self.kind, "GET", url);
        let resp = match self.inner.get(parsed.clone()).send().await {
            Ok(r) => r,
            Err(e) => {
                pending.fail(&e.to_string());
                return Err(e.into());
            }
        };
        let status = resp.status().as_u16();
        let final_url = resp.url().to_string();
        let redirect_hops = usize::from(resp.url() != &parsed);
        let content_type =
            resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let content_length = resp.content_length();
        let trace_id = pending.finish(status, content_type.as_deref(), content_length);
        if !resp.status().is_success() {
            // Keep the first bytes of the error page: "got HTML not M3U" is the diagnosis.
            let body = resp.bytes().await.unwrap_or_default();
            crate::trace::set_preview(trace_id, &body);
            crate::trace::set_error(trace_id, &format!("HTTP {status}"));
            return Err(NetError::Http(format!("HTTP {status} from {}", app_core::redact::redact(&final_url))));
        }
        Ok(StreamedResponse {
            final_url,
            status,
            content_type,
            content_length,
            redirect_hops,
            body: Box::pin(traced_body(trace_id, resp.bytes_stream())),
        })
    }

    /// Small JSON/text GET with an overall timeout (Xtream API calls).
    pub async fn get_text(&self, url: &str) -> Result<(u16, String)> {
        let pending = crate::trace::start(self.kind, "GET", url);
        let resp = match self.inner.get(url).timeout(REQUEST_TIMEOUT).send().await {
            Ok(r) => r,
            Err(e) => {
                pending.fail(&e.to_string());
                return Err(e.into());
            }
        };
        let status = resp.status().as_u16();
        let content_type =
            resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
        let id = pending.finish(status, content_type.as_deref(), resp.content_length());
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                crate::trace::set_error(id, &e.to_string());
                return Err(e.into());
            }
        };
        crate::trace::set_preview(id, text.as_bytes());
        Ok((status, text))
    }
}

/// Wrap a body stream so its first chunk (and any mid-body error) lands in the trace entry.
fn traced_body(
    id: u64,
    body: impl Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send {
    let mut first = true;
    body.map(move |item| {
        match &item {
            Ok(chunk) if first => {
                first = false;
                crate::trace::set_preview(id, chunk);
            }
            Err(e) => crate::trace::set_error(id, &e.to_string()),
            _ => {}
        }
        item
    })
}

/// Adapter so a streamed body can be consumed as a plain `Stream<Item = Result<Bytes>>`.
pub fn map_body(
    body: std::pin::Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
) -> impl Stream<Item = Result<Bytes>> + Send {
    body.map(|r| r.map_err(NetError::from))
}
