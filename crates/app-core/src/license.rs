//! License + trial state (CLAUDE.md §10). Validated in Rust only — the frontend only ever
//! receives [`LicenseStateResponse`].
//!
//! Token format: `DIPTV1.<base64url(payload_json)>.<hex(hmac_sha256(secret, payload))>`
//! payload = `{"tier":"pro_lifetime","machine":"<sha256(machine_guid)[..32]>","issued":<unix>}`
//!
//! The signing secret is baked in at build time via `APP_LICENSE_SECRET` (falls back to a
//! dev secret so debug builds work). Rotate before shipping.

use crate::ipc::LicenseStateResponse;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const TRIAL_SECONDS: i64 = 72 * 60 * 60;
const TOKEN_PREFIX: &str = "DIPTV1";

fn secret() -> &'static [u8] {
    option_env!("APP_LICENSE_SECRET").unwrap_or("dev-secret-rotate-before-ship").as_bytes()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Free,
    Trial,
    ProLifetime,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Free => "free",
            Tier::Trial => "trial",
            Tier::ProLifetime => "pro_lifetime",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenPayload {
    tier: String,
    machine: String,
    issued: i64,
}

/// Stable, non-reversible machine identifier. Falls back to a fixed string when the OS
/// refuses (VMs, sandboxes) so the app still runs.
pub fn machine_guid() -> String {
    let raw = machine_uid::get().unwrap_or_else(|_| "unknown-machine".to_string());
    let hash = Sha256::digest(format!("{}|{}", crate::PRODUCT_NAME, raw).as_bytes());
    hex::encode(&hash[..16])
}

fn sign(payload_b64: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret()).expect("hmac accepts any key length");
    mac.update(payload_b64.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Issue a lifetime token bound to `machine_guid`. Used by the (offline) key generator / store
/// fulfilment; never called from the UI.
pub fn issue_lifetime_token(machine_guid: &str, issued_unix: i64) -> String {
    let payload = TokenPayload {
        tier: Tier::ProLifetime.as_str().into(),
        machine: machine_guid.to_string(),
        issued: issued_unix,
    };
    let json = serde_json::to_string(&payload).expect("payload serializes");
    let b64 = base64url_encode(json.as_bytes());
    format!("{}.{}.{}", TOKEN_PREFIX, b64, sign(&b64))
}

/// Verify a token against this machine. Returns the tier it grants.
pub fn verify_token(token: &str, machine_guid: &str) -> Option<Tier> {
    let mut parts = token.trim().split('.');
    let prefix = parts.next()?;
    let b64 = parts.next()?;
    let sig = parts.next()?;
    if prefix != TOKEN_PREFIX || parts.next().is_some() {
        return None;
    }
    let expected = sign(b64);
    if !constant_time_eq(expected.as_bytes(), sig.as_bytes()) {
        return None;
    }
    let json = base64url_decode(b64)?;
    let payload: TokenPayload = serde_json::from_slice(&json).ok()?;
    if payload.machine != machine_guid {
        return None;
    }
    match payload.tier.as_str() {
        "pro_lifetime" => Some(Tier::ProLifetime),
        _ => None,
    }
}

/// Compute the effective license state.
///
/// * `now` — unix seconds
/// * `trial_started_at` — unix seconds of the first successful playlist import, if any
/// * `stored_token` — the token in `settings`, if any
pub fn compute_state(
    now: i64,
    trial_started_at: Option<i64>,
    stored_token: Option<&str>,
    machine_guid: &str,
) -> LicenseStateResponse {
    if let Some(tok) = stored_token {
        if verify_token(tok, machine_guid) == Some(Tier::ProLifetime) {
            return LicenseStateResponse { is_valid: true, expires_at: None, tier: Tier::ProLifetime.as_str().into() };
        }
    }
    if let Some(started) = trial_started_at {
        let expires = started + TRIAL_SECONDS;
        if now < expires {
            return LicenseStateResponse {
                is_valid: true,
                expires_at: Some(expires),
                tier: Tier::Trial.as_str().into(),
            };
        }
        return LicenseStateResponse { is_valid: true, expires_at: Some(expires), tier: Tier::Free.as_str().into() };
    }
    // No import yet: everything is allowed until the trial clock starts.
    LicenseStateResponse { is_valid: true, expires_at: None, tier: Tier::Trial.as_str().into() }
}

/// Feature gates. Keep in one place so the UI and the commands agree.
pub fn allows(tier: &str, feature: Feature) -> bool {
    match tier {
        "pro_lifetime" | "trial" => true,
        _ => matches!(feature, Feature::LiveTv | Feature::SinglePlaylist | Feature::TrimmedEpg),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    LiveTv,
    SinglePlaylist,
    MultiplePlaylists,
    TrimmedEpg,
    FullEpg,
    Recording,
    Downloads,
    UnlimitedVod,
    Backup,
    Multiscreen,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn base64url_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip_and_binding() {
        let tok = issue_lifetime_token("machine-a", 1_700_000_000);
        assert_eq!(verify_token(&tok, "machine-a"), Some(Tier::ProLifetime));
        assert_eq!(verify_token(&tok, "machine-b"), None, "bound to machine");
        let mut tampered = tok.clone();
        tampered.pop();
        tampered.push('0');
        assert_eq!(verify_token(&tampered, "machine-a"), None, "signature mismatch");
        assert_eq!(verify_token("garbage", "machine-a"), None);
    }

    #[test]
    fn trial_clock() {
        let m = "m";
        let s = compute_state(1000, None, None, m);
        assert_eq!(s.tier, "trial");
        let s = compute_state(1000, Some(500), None, m);
        assert_eq!(s.tier, "trial");
        assert_eq!(s.expires_at, Some(500 + TRIAL_SECONDS));
        let s = compute_state(500 + TRIAL_SECONDS + 1, Some(500), None, m);
        assert_eq!(s.tier, "free");
        let tok = issue_lifetime_token(m, 1);
        let s = compute_state(999_999_999, Some(500), Some(&tok), m);
        assert_eq!(s.tier, "pro_lifetime");
    }

    #[test]
    fn gates() {
        assert!(allows("free", Feature::LiveTv));
        assert!(!allows("free", Feature::Recording));
        assert!(allows("trial", Feature::Recording));
        assert!(allows("pro_lifetime", Feature::Multiscreen));
    }

    #[test]
    fn b64() {
        for s in ["", "a", "ab", "abc", "abcd", "{\"x\":1}"] {
            assert_eq!(base64url_decode(&base64url_encode(s.as_bytes())).unwrap(), s.as_bytes());
        }
    }
}
