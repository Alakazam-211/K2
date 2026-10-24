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

/// Is `running` (`major.minor.patch`, a `-pre` suffix ignored) at or past
/// `release`? Unreadable versions count as not reached.
pub fn version_reached(running: &str, release: &str) -> bool {
    fn parts(v: &str) -> Option<(u64, u64, u64)> {
        let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
        Some((it.next()??, it.next()??, it.next().unwrap_or(Some(0))?))
    }
    matches!((parts(running), parts(release)), (Some(a), Some(b)) if a >= b)
}

/// This build is the cut-over release (or later): text that names it says
/// "now", never "before 0.46.4" while running 0.46.4 (NSI 0.46.4 #4: the
/// notice said "ends 0.46.2" on a box running 0.46.2).
pub fn cutover_now() -> bool {
    version_reached(env!("CARGO_PKG_VERSION"), CUTOVER_RELEASE)
}

/// When the old ways end, for a sentence: "in K2 0.46.4", or "with this
/// release (K2 0.46.4)" once this build is it.
pub fn cutover_phrase() -> String {
    if cutover_now() {
        format!("with this release (K2 {CUTOVER_RELEASE})")
    } else {
        format!("in K2 {CUTOVER_RELEASE}")
    }
}

/// How every legacy `--cmd` service is labelled (status, `k2 publish
/// list`, `k2 app list`, doctor) until its agent moves it to an App.
pub fn legacy_label() -> String {
    if cutover_now() {
        format!("legacy (--cmd) — K2 {CUTOVER_RELEASE} is the cut-over: move it to an App now")
    } else {
        format!("legacy (--cmd) — move it to an App before {CUTOVER_RELEASE}")
    }
}

/// Where an agent reads how to move a legacy service to an App.
pub const LEGACY_HINT: &str = "k2 study app-move";

#[cfg(test)]
mod tests {
    #[test]
    fn the_cutover_is_0_46_4_and_the_label_names_it() {
        assert_eq!(super::CUTOVER_RELEASE, "0.46.4");
        // The label follows the build: "before" until this build is the
        // cut-over, "now" from it on (never "before 0.46.4" on 0.46.4).
        let want = if super::version_reached(env!("CARGO_PKG_VERSION"), "0.46.4") {
            "legacy (--cmd) — K2 0.46.4 is the cut-over: move it to an App now"
        } else {
            "legacy (--cmd) — move it to an App before 0.46.4"
        };
        assert_eq!(super::legacy_label(), want);
        assert_eq!(super::cutover_now(), super::version_reached(env!("CARGO_PKG_VERSION"), "0.46.4"));
        assert!(!super::legacy_label().contains("adopt"));
    }

    #[test]
    fn version_reached_compares_numerically_and_says_now_at_the_release() {
        use super::version_reached as r;
        assert!(!r("0.46.3", "0.46.4"));
        assert!(r("0.46.4", "0.46.4"), "the cut-over release itself has reached it");
        assert!(r("0.46.10", "0.46.4"), "numeric, not lexical");
        assert!(r("0.47.0", "0.46.4"));
        assert!(r("v0.46.4-rc1", "0.46.4"));
        assert!(!r("0.45.99", "0.46.4"));
        assert!(!r("garbage", "0.46.4"), "unreadable: not reached");
        assert!(!r("0.46.4", ""), "no release: not reached");
    }
}
