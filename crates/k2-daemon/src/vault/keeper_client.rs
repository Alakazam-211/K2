// Linux-only at runtime; compiled everywhere so the code stays checked.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! Transport to the root helper's `keep` verbs (KV23). Dormant until a
//! `k2-root-helper` answering protocol ≥ 4 is installed (root-helper PRD
//! §7.5); until then Linux boxes run on the file key and status says so.
//!
//! Calls are `/usr/bin/sudo -n <helper> keep …` with an empty stdin; the
//! helper reads nothing secret from us and returns the KEK on stdout only
//! to the daemon process (its caller check).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use k2_vault::keystore::keeper::KeeperTransport;
use k2_vault::VaultError;

pub const HELPER_PATHS: &[&str] = &["/usr/local/libexec/k2-root-helper", "/usr/lib/k2/k2-root-helper"];
const SUDO: &str = "/usr/bin/sudo";

pub struct SudoHelper {
    path: PathBuf,
}

impl SudoHelper {
    /// The installed helper, if any (no call is made here).
    pub fn detect() -> Option<SudoHelper> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        HELPER_PATHS
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .map(|path| SudoHelper { path })
    }
}

impl KeeperTransport for SudoHelper {
    fn call(&self, args: &[&str]) -> Result<Vec<u8>, VaultError> {
        // The installed helper holds the box's real key. A test daemon
        // never gets here (`vault::select_key_store` gives a test build
        // the file key); if one ever does, stop it at once, by name,
        // instead of a `keeper_error` deep inside a vault route.
        assert!(
            !k2_core::TEST_BUILD,
            "test daemon reached the installed root helper ({} {}): a test build keeps the vault key in a file \
             under its own home (vault::select_key_store)",
            self.path.display(),
            args.join(" ")
        );
        let out = Command::new(SUDO)
            .arg("-n")
            .arg(&self.path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|e| VaultError::internal(format!("root helper: {}", e.kind())))?;
        if !out.status.success() {
            return Err(VaultError::new(
                "keeper_error",
                500,
                format!("root helper refused `{}`", args.join(" ")),
            ));
        }
        Ok(out.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The belt behind `select_key_store`: whatever path leads a test
    /// build to the installed helper, the first call fails by name (and
    /// before `sudo` runs).
    #[test]
    #[should_panic(expected = "test daemon reached the installed root helper")]
    fn a_test_build_never_calls_the_installed_helper() {
        let h = SudoHelper { path: PathBuf::from(HELPER_PATHS[0]) };
        let _ = h.call(&["keep", "status"]);
    }
}
