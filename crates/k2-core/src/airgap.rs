//! Air-gap / offline flag (MasterControl). Opt-in, default **off**.
//!
//! Env `K2_AIRGAP` wins over the typed `AppSettings.airgap` field. Polarity
//! (fail closed = **block** outbound):
//!   - truthy `1` / `true` / `on` / `yes` (case-insensitive) → on
//!   - falsy `0` / `false` / `off` / `no` → off
//!   - set but garbage → **on**
//!   - unset → persisted setting (default off)
//!
//! Opposite polarity from [`crate::listen`] (`K2_LISTEN` garbage → loopback).
//! Read at boot and every refuse site — never renderer-only.
//!
//! Custom/enterprise builds can set `--features airgap` ([`baked`] is true:
//! air-gap cannot be turned off, and k2-daemon omits the GitHub
//! update-availability URL). That binary is **not** a GitHub Release asset.
//! Cloud / Linux ship builds leave this feature OFF.

use std::sync::atomic::{AtomicBool, Ordering};

/// Teaching copy for CLI/HTTP refuse. Names the env; does not dump internals.
pub const TEACHING: &str = "Air-gap is on (K2_AIRGAP=1). This daemon will not start a tunnel or phone Connect, cert, GitHub, or other hosted services.";

/// Env name. Images set this on the unit **before first start**.
pub const ENV_VAR: &str = "K2_AIRGAP";

/// Runtime mirror of the persisted `airgap` setting. Env still wins.
static SETTING_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `AppSettings.airgap` value into this process.
pub fn set_setting_enabled(on: bool) {
    SETTING_ENABLED.store(on, Ordering::Relaxed);
}

/// True iff this binary was compiled with the `airgap` cargo feature.
///
/// Custom/enterprise `--features airgap` builds set this. Runtime
/// `K2_AIRGAP=0` cannot turn it off. Not a GitHub Release asset.
pub fn baked() -> bool {
    cfg!(feature = "airgap")
}

/// True iff air-gap is on. Env wins; garbage env → on; unset → setting.
/// A `--features airgap` build is always on (see [`baked`]).
pub fn enabled() -> bool {
    if baked() {
        return true;
    }
    match std::env::var(ENV_VAR) {
        Ok(v) => parse_env(&v),
        Err(_) => SETTING_ENABLED.load(Ordering::Relaxed) || crate::app_settings::load().airgap,
    }
}

/// Refuse with [`TEACHING`] when air-gap is on.
pub fn refuse() -> Result<(), String> {
    if enabled() {
        Err(TEACHING.to_string())
    } else {
        Ok(())
    }
}

/// JSON `{error}` body for a 403 refuse.
pub fn error_json() -> String {
    serde_json::json!({ "error": TEACHING }).to_string()
}

/// Parse `K2_AIRGAP`. Garbage (including empty) → on.
fn parse_env(raw: &str) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => true,
        "0" | "false" | "off" | "no" => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `K2_AIRGAP` under the ONE env lock (restored on drop, even on
    /// panic); also resets the in-memory setting.
    struct EnvGuard {
        _var: crate::test_env::EnvVar,
    }

    impl EnvGuard {
        fn set(val: Option<&str>) -> Self {
            let _var = match val {
                Some(v) => crate::test_env::EnvVar::set(ENV_VAR, v),
                None => crate::test_env::EnvVar::remove(ENV_VAR),
            };
            Self { _var }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            set_setting_enabled(false);
        }
    }

    fn isolated_home() -> crate::test_env::TempHome {
        crate::test_env::TempHome::new()
    }

    #[test]
    fn baked_matches_cargo_feature() {
        assert_eq!(
            baked(),
            cfg!(feature = "airgap"),
            "baked() must track the airgap cargo feature"
        );
    }

    #[cfg(feature = "airgap")]
    #[test]
    fn baked_build_cannot_be_disabled_with_env() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        set_setting_enabled(false);
        for v in ["0", "false", "OFF", "no"] {
            let _env = EnvGuard::set(Some(v));
            assert!(enabled(), "baked airgap must stay on when K2_AIRGAP={v}");
        }
        let _unset = EnvGuard::set(None);
        assert!(enabled(), "baked airgap must stay on when env is unset");
    }

    #[cfg(not(feature = "airgap"))]
    #[test]
    fn defaults_off_when_env_and_setting_unset() {
        let _lock = crate::test_env::lock();
        let _env = EnvGuard::set(None);
        set_setting_enabled(false);
        let _home = isolated_home();
        assert!(!enabled(), "air-gap must default OFF");
    }

    #[test]
    fn env_truthy_enables() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        set_setting_enabled(false);
        for v in ["1", "true", "TRUE", "on", "Yes"] {
            let _env = EnvGuard::set(Some(v));
            assert!(enabled(), "K2_AIRGAP={v} must enable");
        }
    }

    #[cfg(not(feature = "airgap"))]
    #[test]
    fn env_falsy_disables_even_if_setting_on() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        set_setting_enabled(true);
        for v in ["0", "false", "OFF", "no"] {
            let _env = EnvGuard::set(Some(v));
            assert!(!enabled(), "K2_AIRGAP={v} must disable (env wins)");
        }
    }

    #[test]
    fn env_garbage_enables_fail_closed() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        set_setting_enabled(false);
        for v in ["garbage", "maybe", "", "2", "lan"] {
            let _env = EnvGuard::set(Some(v));
            assert!(enabled(), "K2_AIRGAP={v:?} must enable (fail closed)");
        }
    }

    #[cfg(not(feature = "airgap"))]
    #[test]
    fn setting_enables_when_env_unset() {
        let _lock = crate::test_env::lock();
        let _env = EnvGuard::set(None);
        let _home = isolated_home();
        set_setting_enabled(true);
        assert!(enabled(), "persisted airgap setting must enable");
        set_setting_enabled(false);
        assert!(!enabled());
    }

    #[test]
    fn refuse_err_names_env() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        let _env = EnvGuard::set(Some("1"));
        let err = refuse().expect_err("air-gap must refuse");
        assert!(
            err.contains("K2_AIRGAP=1"),
            "teaching error must name the env; got {err}"
        );
    }
}
