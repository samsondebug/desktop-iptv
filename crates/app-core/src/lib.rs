//! `app-core` — lifecycle, config, license and the typed IPC contracts shared by
//! every other crate and mirrored 1:1 in `apps/desktop/src/lib/ipc.ts`.
//!
//! Rules (CLAUDE.md §0, §4, §10):
//! - These are the only shapes that cross the IPC boundary. Do not invent parallel ones.
//! - Secrets never leave Rust unredacted (see [`redact`]).
//! - License validation lives here, in Rust, never in JS.

pub mod backup;
pub mod config;
pub mod ipc;
pub mod license;
pub mod redact;

pub use config::*;
pub use ipc::*;
pub use license::*;

/// The legal block. Shown on first run, in Settings, on the website and in store copy.
pub const LEGAL_BLOCK: &str = "This app does not provide channels, playlists, or stream URLs. \
You bring your own source. We do not support illegal services. \
We support the player, not the reseller.";

/// Product identifiers. Never "IPTV Player Zero", "Zero" or "Built By Board".
/// The product was called `desktop-iptv` up to 0.1.0 (still the crate, bundle-identifier and
/// repository name); `SKTV` is what users see.
pub const PRODUCT_NAME: &str = "SKTV";
/// Salt for the machine identifier. Deliberately the old product name: changing it would give
/// every existing install a new machine GUID, restarting trials and orphaning issued licences.
pub const MACHINE_ID_SALT: &str = "desktop-iptv";
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
