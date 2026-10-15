//! The 0.46.0 App "grace" release (Rosson's cross-over plan, 2026-10-14).
//!
//! 0.46.0 adds the new ways for existing Apps (`k2 publish run --cmd`
//! services such as NSI's Jobs BFF) and removes nothing; 0.46.1 is the
//! cut-over (the inherited environment ends, the owner token is out of
//! reach, the wall goes on). This module is the daemon-shared part:
//!
//! - [`keys`]: the App-scoped key (vs-live 2 §6, S45): one App, a fixed
//!   scope vocabulary, a room/role ceiling.
//! - [`requests`]: held requests (owners widen, agents propose; §5.6).
//! - [`config`]: the §4.4 App config document and the narrowing/widening
//!   classifier behind `/cli/app/config` (S13).
//! - [`scan`]: the per-service warnings: env names the code reads, K2
//!   paths it touches, programs it finds through `PATH`.
//! - [`token_uses`]: owner-token calls attributed to a service.

pub mod config;
pub mod keys;
pub mod requests;
pub mod scan;
pub mod token_uses;

/// The release that removes the old ways (inherited env, owner token,
/// unwalled services). Warnings and notices name it.
pub const CUTOVER_RELEASE: &str = "0.46.1";

