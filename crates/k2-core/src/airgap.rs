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

/// True iff air-gap is on. Env wins; garbage env → on; unset → setting.
pub fn enabled() -> bool {
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
    use crate::themes::HOME_LOCK;

    struct EnvGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(val: Option<&str>) -> Self {
            let prev = std::env::var_os(ENV_VAR);
            match val {
                Some(v) => std::env::set_var(ENV_VAR, v),
                None => std::env::remove_var(ENV_VAR),
            }
            Self { prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var(ENV_VAR, p),
                None => std::env::remove_var(ENV_VAR),
            }
            set_setting_enabled(false);
        }
    }

    fn isolated_home() -> (std::path::PathBuf, Option<std::ffi::OsString>) {
        let prev = std::env::var_os("HOME");
        let dir = std::env::temp_dir().join(format!(
            "k2-airgap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp HOME");
        std::env::set_var("HOME", &dir);
        (dir, prev)
    }

    fn restore_home(dir: &std::path::Path, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn defaults_off_when_env_and_setting_unset() {
        let _lock = HOME_LOCK.lock();
        let _env = EnvGuard::set(None);
        set_setting_enabled(false);
        let (dir, prev) = isolated_home();
        assert!(!enabled(), "air-gap must default OFF");
        restore_home(&dir, prev);
    }

    #[test]
    fn env_truthy_enables() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        set_setting_enabled(false);
        for v in ["1", "true", "TRUE", "on", "Yes"] {
            let _env = EnvGuard::set(Some(v));
            assert!(enabled(), "K2_AIRGAP={v} must enable");
        }
        restore_home(&dir, prev);
    }

    #[test]
    fn env_falsy_disables_even_if_setting_on() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        set_setting_enabled(true);
        for v in ["0", "false", "OFF", "no"] {
            let _env = EnvGuard::set(Some(v));
            assert!(!enabled(), "K2_AIRGAP={v} must disable (env wins)");
        }
        restore_home(&dir, prev);
    }

    #[test]
    fn env_garbage_enables_fail_closed() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        set_setting_enabled(false);
        for v in ["garbage", "maybe", "", "2", "lan"] {
            let _env = EnvGuard::set(Some(v));
            assert!(enabled(), "K2_AIRGAP={v:?} must enable (fail closed)");
        }
        restore_home(&dir, prev);
    }

    #[test]
    fn setting_enables_when_env_unset() {
        let _lock = HOME_LOCK.lock();
        let _env = EnvGuard::set(None);
        let (dir, prev) = isolated_home();
        set_setting_enabled(true);
        assert!(enabled(), "persisted airgap setting must enable");
        set_setting_enabled(false);
        assert!(!enabled());
        restore_home(&dir, prev);
    }

    #[test]
    fn refuse_err_names_env() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        let _env = EnvGuard::set(Some("1"));
        let err = refuse().expect_err("air-gap must refuse");
        assert!(
            err.contains("K2_AIRGAP=1"),
            "teaching error must name the env; got {err}"
        );
        restore_home(&dir, prev);
    }
}
