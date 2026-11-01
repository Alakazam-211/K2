//! The voice rooms preview switch (prd-k2-voice-rooms-v1 vs-live §3):
//! `voice_rooms_preview` (an owner-only setting) or `K2_VOICE_ROOMS=1`,
//! default **off**, the `compute_preview` pattern
//! ([`crate::compute::enabled`]).
//!
//! It gates the voice rooms work that lands dark in 0.46.8: the
//! `/cli/voice/*` routes, the Admin Space, the `voice-rooms-v1` feature key,
//! the headset button and S6's Sew set-up on every Linux box. With it off,
//! nothing changes on any box.
//!
//! The persisted setting is synced in with [`set_enabled`] (daemon boot and
//! every settings update/reset), like `compute::set_enabled`.

use std::sync::atomic::{AtomicBool, Ordering};

/// The env var that turns the preview on (`1`/`true`/`yes`/`on`).
pub const ENV: &str = "K2_VOICE_ROOMS";

static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `voice_rooms_preview` setting into this process.
pub fn set_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// True when the preview is on: [`ENV`] or the persisted switch.
pub fn enabled() -> bool {
    if let Ok(v) = std::env::var(ENV) {
        if matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
    }
    SETTING_ENABLED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_on_by_env_or_setting() {
        let _g = crate::themes::HOME_LOCK.lock();
        let env = crate::test_env::EnvVar::remove(ENV);
        set_enabled(false);
        assert!(!enabled(), "off by default");
        for on in ["1", "true", "YES", " on "] {
            let _e = crate::test_env::EnvVar::set(ENV, on);
            assert!(enabled(), "{on}");
        }
        for off in ["0", "false", "", "maybe"] {
            let _e = crate::test_env::EnvVar::set(ENV, off);
            assert!(!enabled(), "{off:?}");
        }
        set_enabled(true);
        assert!(enabled(), "the setting alone");
        set_enabled(false);
        drop(env);
    }
}
