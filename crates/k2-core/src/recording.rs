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
//! | [`CLIENT_CONSENT_SHEET`] | the K5 rooms' "require recording" switch (`POST /cli/voice/recording/settings` `required`): K2's desktop can't answer a consent prompt until its consent sheet ships (K-g, S7), and a required room holds every join for consent (Rosson's answer 6) |
//!
//! Tests flip them through [`set_for_tests`]; nothing else does.

use std::sync::atomic::{AtomicBool, Ordering};

/// `voice:record` is accepted in manifests and roles. On since the Sew 0.9.0
/// pin (K2 0.46.11); every wire change still waits for Sew's own flags, so a
/// box still on Sew 0.8.x is inert (the mint drops the cap without
/// `voice_record_cap`, the rollout skips without it).
pub const RECORDING_IN_APPS: bool = true;

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

/// Tests only: act as if K-g had flipped [`CLIENT_CONSENT_SHEET`] (and, for
/// the record, [`RECORDING_IN_APPS`]; it is on since the pin, so `false` no
/// longer turns it off).
/// Process-wide (nextest gives each test its own process).
#[doc(hidden)]
pub fn set_for_tests(in_apps: bool, consent_sheet: bool) {
    TEST_RECORDING_IN_APPS.store(in_apps, Ordering::Relaxed);
    TEST_CONSENT_SHEET.store(consent_sheet, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    /// Pin day (Sew 0.9.0, K2 0.46.11): `voice:record` is K2's whatever a test
    /// override says; the desktop consent sheet (K-g) is still off.
    #[test]
    fn the_pin_turned_recording_in_apps_on_and_the_consent_sheet_stays_off() {
        assert!(super::RECORDING_IN_APPS);
        super::set_for_tests(false, false);
        assert!(super::in_apps(), "a test can't turn the pin back off");
        assert!(!super::CLIENT_CONSENT_SHEET && !super::consent_sheet());
    }
}
