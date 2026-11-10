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
//! | [`RECORDING_IN_APPS`] | `voice:record` in a manifest's caps and in role room lines ([`crate::skin::parse_caps`], `deploy::validate`), and with it Meet's `requires.k2 >= 0.46.11`, and the grandfather step of the cap rollout. The pin slice (Sew 0.9.0 pin day, S9) flips it |
//! | [`CLIENT_CONSENT_SHEET`] | the K5 rooms' "require recording" switch (`POST /cli/voice/recording/settings` `required`): K2's desktop can't answer a consent prompt until its consent sheet ships (K-g, S7), and a required room holds every join for consent (Rosson's answer 6) |
//!
//! Tests flip them through [`set_for_tests`]; nothing else does.

use std::sync::atomic::{AtomicBool, Ordering};

/// `voice:record` is accepted in manifests and roles. Off until the
/// Sew 0.9.0 pin slice.
pub const RECORDING_IN_APPS: bool = false;

/// K2's desktop answers recording consent prompts (K-g). Off until it ships.
pub const CLIENT_CONSENT_SHEET: bool = false;

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

/// Tests only: act as if the pin slice (and K-g) had flipped the consts.
/// Process-wide (nextest gives each test its own process).
#[doc(hidden)]
pub fn set_for_tests(in_apps: bool, consent_sheet: bool) {
    TEST_RECORDING_IN_APPS.store(in_apps, Ordering::Relaxed);
    TEST_CONSENT_SHEET.store(consent_sheet, Ordering::Relaxed);
}
