//! LAN listen flag. Opt-in, default **off** (loopback).
//!
//! Env `K2_LISTEN=lan` wins over the typed `AppSettings.listen_lan` field.
//! Unknown/garbage env → **loopback** (fail closed = do not expose).
//! Opposite polarity from [`crate::airgap`].

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};

/// Env name. Images that want a second client on the VPC set `K2_LISTEN=lan`
/// on the unit **before first start**.
pub const ENV_VAR: &str = "K2_LISTEN";

/// Runtime mirror of the persisted `listenLan` setting. Env still wins.
static SETTING_LAN: AtomicBool = AtomicBool::new(false);

/// Whether THIS process actually bound `0.0.0.0` (set at HTTP claim time).
static LAN_BOUND: AtomicBool = AtomicBool::new(false);

/// Sync the persisted `AppSettings.listen_lan` value into this process.
pub fn set_setting_lan(on: bool) {
    SETTING_LAN.store(on, Ordering::Relaxed);
}

/// Record the bind decision after a successful `claim_port_on`.
pub fn set_lan_bound(on: bool) {
    LAN_BOUND.store(on, Ordering::Relaxed);
}

/// True iff this process's HTTP listener is on `0.0.0.0`.
pub fn lan_bound() -> bool {
    LAN_BOUND.load(Ordering::Relaxed)
}

/// True iff LAN listen is requested (env `lan` or persisted setting).
pub fn lan_requested() -> bool {
    match std::env::var(ENV_VAR) {
        Ok(v) => v.trim().eq_ignore_ascii_case("lan"),
        Err(_) => SETTING_LAN.load(Ordering::Relaxed) || crate::app_settings::load().listen_lan,
    }
}

/// IPv4 bind address for the HTTP listener.
pub fn bind_ip() -> Ipv4Addr {
    if lan_requested() {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `K2_LISTEN` under the ONE env lock (restored on drop, even on
    /// panic); also resets the in-memory LAN flags.
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
            set_setting_lan(false);
            set_lan_bound(false);
        }
    }

    fn isolated_home() -> crate::test_env::TempHome {
        crate::test_env::TempHome::new()
    }

    #[test]
    fn defaults_loopback() {
        let _lock = crate::test_env::lock();
        let _env = EnvGuard::set(None);
        set_setting_lan(false);
        let _home = isolated_home();
        assert!(!lan_requested(), "LAN listen must default OFF");
        assert_eq!(bind_ip(), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn env_lan_wins() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        set_setting_lan(false);
        let _env = EnvGuard::set(Some("lan"));
        assert!(lan_requested());
        assert_eq!(bind_ip(), Ipv4Addr::UNSPECIFIED);
    }

    #[test]
    fn env_lan_is_case_insensitive() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        let _env = EnvGuard::set(Some("LAN"));
        assert!(lan_requested());
    }

    #[test]
    fn env_garbage_is_loopback() {
        let _lock = crate::test_env::lock();
        let _home = isolated_home();
        set_setting_lan(true);
        for v in ["garbage", "1", "true", "yes", "0.0.0.0", ""] {
            let _env = EnvGuard::set(Some(v));
            assert!(
                !lan_requested(),
                "K2_LISTEN={v:?} must stay loopback (fail closed)"
            );
            assert_eq!(bind_ip(), Ipv4Addr::LOCALHOST);
        }
    }

    #[test]
    fn setting_enables_when_env_unset() {
        let _lock = crate::test_env::lock();
        let _env = EnvGuard::set(None);
        let _home = isolated_home();
        set_setting_lan(true);
        assert!(lan_requested());
        assert_eq!(bind_ip(), Ipv4Addr::UNSPECIFIED);
    }
}
