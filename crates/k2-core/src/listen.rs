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
            set_setting_lan(false);
            set_lan_bound(false);
        }
    }

    fn isolated_home() -> (std::path::PathBuf, Option<std::ffi::OsString>) {
        let prev = std::env::var_os("HOME");
        let dir = std::env::temp_dir().join(format!(
            "k2-listen-{}-{}",
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
    fn defaults_loopback() {
        let _lock = HOME_LOCK.lock();
        let _env = EnvGuard::set(None);
        set_setting_lan(false);
        let (dir, prev) = isolated_home();
        assert!(!lan_requested(), "LAN listen must default OFF");
        assert_eq!(bind_ip(), Ipv4Addr::LOCALHOST);
        restore_home(&dir, prev);
    }

    #[test]
    fn env_lan_wins() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        set_setting_lan(false);
        let _env = EnvGuard::set(Some("lan"));
        assert!(lan_requested());
        assert_eq!(bind_ip(), Ipv4Addr::UNSPECIFIED);
        restore_home(&dir, prev);
    }

    #[test]
    fn env_lan_is_case_insensitive() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        let _env = EnvGuard::set(Some("LAN"));
        assert!(lan_requested());
        restore_home(&dir, prev);
    }

    #[test]
    fn env_garbage_is_loopback() {
        let _lock = HOME_LOCK.lock();
        let (dir, prev) = isolated_home();
        set_setting_lan(true);
        for v in ["garbage", "1", "true", "yes", "0.0.0.0", ""] {
            let _env = EnvGuard::set(Some(v));
            assert!(
                !lan_requested(),
                "K2_LISTEN={v:?} must stay loopback (fail closed)"
            );
            assert_eq!(bind_ip(), Ipv4Addr::LOCALHOST);
        }
        restore_home(&dir, prev);
    }

    #[test]
    fn setting_enables_when_env_unset() {
        let _lock = HOME_LOCK.lock();
        let _env = EnvGuard::set(None);
        let (dir, prev) = isolated_home();
        set_setting_lan(true);
        assert!(lan_requested());
        assert_eq!(bind_ip(), Ipv4Addr::UNSPECIFIED);
        restore_home(&dir, prev);
    }
}
