//! The air-gap env switch and its teaching copy, shared by the daemon
//! (`k2_core::airgap`, which adds the persisted setting and the baked
//! `--features airgap` build) and the thin client's updater refusals.
//!
//! Env `K2_AIRGAP` polarity (fail closed = **block** outbound):
//!   - truthy `1` / `true` / `on` / `yes` (case-insensitive) → on
//!   - falsy `0` / `false` / `off` / `no` → off
//!   - set but garbage (including empty) → **on**
//!   - unset → the persisted `airgap` setting decides (default off)

/// Teaching copy for CLI/HTTP refuse. Names the env; does not dump internals.
pub const TEACHING: &str = "Air-gap is on (K2_AIRGAP=1). This daemon will not start a tunnel or phone Connect, cert, GitHub, or other hosted services.";

/// Env name. Images set this on the unit **before first start**.
pub const ENV_VAR: &str = "K2_AIRGAP";

/// Parse `K2_AIRGAP`. Garbage (including empty) → on.
pub fn parse_env(raw: &str) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => true,
        "0" | "false" | "off" | "no" => false,
        _ => true,
    }
}

/// `Some(on)` when `K2_AIRGAP` is set (it wins over the setting), `None`
/// when unset.
pub fn env_override() -> Option<bool> {
    std::env::var(ENV_VAR).ok().map(|v| parse_env(&v))
}

/// The thin client's air-gap check: env wins, else `settings.json`'s
/// `airgap` (read through [`crate::app_settings_view`]). The app is never a
/// `--features airgap` build and never syncs the daemon's in-process
/// setting mirror, so this is exactly what `k2_core::airgap::enabled()`
/// returned inside the app process.
pub fn enabled_for_client() -> bool {
    match env_override() {
        Some(on) => on,
        None => crate::app_settings_view::load().airgap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_polarity_fails_closed() {
        for v in ["1", "true", "ON", " yes ", "", "garbage"] {
            assert!(parse_env(v), "{v:?} must read as on");
        }
        for v in ["0", "false", "OFF", " no "] {
            assert!(!parse_env(v), "{v:?} must read as off");
        }
    }
}
