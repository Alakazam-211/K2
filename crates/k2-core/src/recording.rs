//! Recording in Apps (Sew 0.9.0, K2 PRD prd-k2-bobbin-in-apps-v1): the
//! release switches.
//!
//! Every wire change waits for Sew's own flags
//! ([`crate::tool_hook::features::RecordingFeatures`]); Sew 0.8.x reports
//! none, so K2 is inert there. Two changes are K2's alone and wait for a
//! const each instead:
//!
//! | Const | What it holds back |
//! |---|---|
//! | [`RECORDING_IN_APPS`] | `voice:record` in a manifest's caps and in role room lines ([`crate::skin::parse_caps`], `deploy::validate`), and with it Meet's `requires.k2 >= 0.46.11`, and the grandfather step of the cap rollout. **On** since the pin slice (Sew 0.9.0 pin day, S9, K2 0.46.11) |
//! | [`CLIENT_CONSENT_SHEET`] | the K5 rooms' "require recording" switch (`POST /cli/voice/recording/settings` `required`): a required room holds every join for the notice, so it waited for K2's desktop to answer one (K-g, S7; Rosson's answer 6). **On** since K-g (K2 0.46.11) |
//!
//! Tests flip them through [`set_for_tests`]; nothing else does.

use std::sync::atomic::{AtomicBool, Ordering};

/// `voice:record` is accepted in manifests and roles. On since the Sew 0.9.0
/// pin (K2 0.46.11); every wire change still waits for Sew's own flags, so a
/// box still on Sew 0.8.x is inert (the mint drops the cap without
/// `voice_record_cap`, the rollout skips without it).
pub const RECORDING_IN_APPS: bool = true;

/// K2's desktop answers recording notices (K-g). On since K2 0.46.11: in
/// K2's own rooms the notice is "OK" (the person's grant) or "Leave call"
/// (Rosson 2026-11-10), so the K5 rooms' "require recording" switch is
/// available to the owner.
pub const CLIENT_CONSENT_SHEET: bool = true;

static TEST_RECORDING_IN_APPS: AtomicBool = AtomicBool::new(false);
static TEST_CONSENT_SHEET: AtomicBool = AtomicBool::new(false);

/// [`RECORDING_IN_APPS`], or a test that turned it on.
pub fn in_apps() -> bool {
    RECORDING_IN_APPS || TEST_RECORDING_IN_APPS.load(Ordering::Relaxed)
}

/// [`CLIENT_CONSENT_SHEET`], or a test that turned it on.
pub fn consent_sheet() -> bool {
    CLIENT_CONSENT_SHEET || TEST_CONSENT_SHEET.load(Ordering::Relaxed)
}

/// Tests only: the overrides (both consts are on since 0.46.11, so `false`
/// no longer turns either off).
/// Process-wide (nextest gives each test its own process).
#[doc(hidden)]
pub fn set_for_tests(in_apps: bool, consent_sheet: bool) {
    TEST_RECORDING_IN_APPS.store(in_apps, Ordering::Relaxed);
    TEST_CONSENT_SHEET.store(consent_sheet, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    /// K2 0.46.11: `voice:record` is K2's (the pin) and K2's desktop answers
    /// recording notices (K-g), whatever a test override says.
    #[test]
    fn recording_in_apps_and_the_consent_sheet_are_on() {
        super::set_for_tests(false, false);
        assert!(super::in_apps(), "on with no test override: a test can't turn the pin back off");
        assert!(super::consent_sheet(), "K-g shipped: the desktop answers recording notices");
    }
}
