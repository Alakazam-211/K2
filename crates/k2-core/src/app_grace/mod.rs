//! The 0.46.0 App "grace" release (Rosson's cross-over plan, 2026-10-14).
//!
//! 0.46.0 adds the new ways for existing Apps (`k2 publish run --cmd`
//! services such as a customer's Jobs BFF) and removes nothing. The
//! cut-over for EXISTING services (the inherited environment ends, the
//! owner token is out of reach, the wall goes on) is 0.46.4 (Rosson
//! 2026-10-20: in 0.46.3 each service's agent moves it to an App itself,
//! with "Build and run Apps" on; it was 0.46.2, and 0.46.1 before that).
//! Until then nothing enforces it: 0.46.1+ changes only NEW `--cmd`
//! services (the open path and the agent gate). This module is the
//! daemon-shared part:
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

/// The release that removes the old ways for existing `--cmd` services
/// (inherited env, owner token, unwalled). Warnings, notices and the legacy
/// label name it. Nothing before it removes them.
pub const CUTOVER_RELEASE: &str = "0.46.4";

/// How every legacy `--cmd` service is labelled (status, `k2 publish
/// list`, `k2 app list`, doctor) until its agent moves it to an App.
pub fn legacy_label() -> String {
    format!("legacy (--cmd) — move it to an App before {CUTOVER_RELEASE}")
}

/// Where an agent reads how to move a legacy service to an App.
pub const LEGACY_HINT: &str = "k2 study app-move";

#[cfg(test)]
mod tests {
    #[test]
    fn the_cutover_is_0_46_4_and_the_label_names_it() {
        assert_eq!(super::CUTOVER_RELEASE, "0.46.4");
        assert_eq!(super::legacy_label(), "legacy (--cmd) — move it to an App before 0.46.4");
        assert!(!super::legacy_label().contains("adopt"));
    }
}
