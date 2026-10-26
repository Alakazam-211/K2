//! The 0.46.0 App "grace" release (Rosson's cross-over plan, 2026-10-14).
//!
//! 0.46.0 adds the new ways for existing Apps (`k2 publish run --cmd`
//! services such as a customer's Jobs BFF) and removes nothing. The
//! cut-over for EXISTING services is not set (Rosson 2026-10-26: "leave the
//! --cmd services since we're still patching up issues with the runtime";
//! it was planned for 0.46.4, 0.46.2 and 0.46.1 before that): they keep
//! running, and each service's agent moves it to an App itself when ready,
//! with "Build and run Apps" on. Nothing enforces a cut-over: 0.46.1+
//! changes only NEW `--cmd` services (the open path and the agent gate).
//! See [`CUTOVER_RELEASE`]. This module is the daemon-shared part:
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

/// No release removes the old ways for existing `--cmd` services
/// (inherited environment, owner token, unwalled). Rosson 2026-10-26:
/// "leave the --cmd services since we're still patching up issues with the
/// runtime". Nothing ever enforced a cut-over (0.46.1+ gates only NEW
/// agent `--cmd` services), so no text names one: a legacy service keeps
/// running and its agent moves it to an App when ready.
///
/// `None` = no cut-over set. JSON fields that used to carry the release
/// (`cutover`, `envRemovedIn`) are kept for compatibility and are `null`;
/// `cutoverNow` is `false`.
pub const CUTOVER_RELEASE: Option<&str> = None;

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

/// This build is a set cut-over release (or later). Always `false` while
/// [`CUTOVER_RELEASE`] is `None`.
pub fn cutover_now() -> bool {
    CUTOVER_RELEASE.is_some_and(|r| version_reached(env!("CARGO_PKG_VERSION"), r))
}

/// How every legacy `--cmd` service is labelled (status, `k2 publish
/// list`, `k2 app list`, doctor): it keeps running; its agent moves it to
/// an App when ready.
pub const LEGACY_LABEL: &str = "legacy (--cmd): keeps running; move it to an App when ready (k2 study app-move)";

/// [`LEGACY_LABEL`], for callers that want a `String`.
pub fn legacy_label() -> String {
    LEGACY_LABEL.to_string()
}

/// Where an agent reads how to move a legacy service to an App.
pub const LEGACY_HINT: &str = "k2 study app-move";

#[cfg(test)]
mod tests {
    #[test]
    fn no_cutover_is_set_and_the_label_says_it_keeps_running() {
        // Rosson 2026-10-26: no release removes --cmd services; no text
        // names one.
        assert_eq!(super::CUTOVER_RELEASE, None);
        assert!(!super::cutover_now());
        assert_eq!(super::legacy_label(), "legacy (--cmd): keeps running; move it to an App when ready (k2 study app-move)");
        assert!(!super::legacy_label().contains("0.46"), "no release in the label");
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
