//! 0.40.41 — self-stage the `k2` CLI on headless Linux servers.
//!
//! The `k2` CLI (`cli/k2`, a bash client that talks to the daemon over
//! HTTP via `~/.k2/heartbeat.{port,token}`) reaches a box one of two
//! ways: the DESKTOP APP symlinks `/usr/local/bin/k2` into its bundle
//! (macOS — see `commands/settings.rs::cli_symlink_needs_heal`), or the
//! provision script `install`s it (Linux). A headless Linux SERVER has no
//! app, so if the provision/migration step is skipped the CLI silently
//! never appears — daemon + data present, `k2` command absent. That bit
//! the nsi migration (see `prd-cloud-server-upgrade-v1.md` §9 L-cli).
//!
//! Fix: the daemon self-stages the CLI at boot, exactly like the
//! `k2-open` shim ([`crate::open_shim`]) — embed `cli/k2` via
//! `include_str!` and, on Linux, write it to `/usr/local/bin/k2` when it
//! is ABSENT or an out-of-date regular file. Guardrails:
//!   - **Linux only.** macOS keeps the app-managed symlink; we never race
//!     it.
//!   - **Never clobber a SYMLINK** at the target. That respects both the
//!     macOS app's symlink and a developer's repo symlink — we only ever
//!     manage a regular-file install.
//!   - **Non-fatal.** A failed write (not root, read-only `/usr/local/bin`)
//!     is logged by the caller, never crashes boot.

use std::path::{Path, PathBuf};

/// The CLI script, embedded verbatim from `cli/k2` at the repo root so
/// the daemon binary is self-contained (same reasoning as the SQL
/// migrations' and the k2-open shim's `include_str!`).
pub const CLI_CONTENT: &str = include_str!("../../../cli/k2");

/// Current version marker inside `cli/k2` (bumped by `release.sh`).
const VERSION_PREFIX: &str = "K2_CLI_VERSION=";
/// Pre-rename marker — lets an on-disk k2so-era CLI read as outdated so
/// it gets refreshed forward.
const LEGACY_VERSION_PREFIX: &str = "K2SO_CLI_VERSION=";

/// Canonical install location (on PATH; matches `provision-k2-server.sh`).
const INSTALL_PATH: &str = "/usr/local/bin/k2";

/// Extract the CLI version value, quote-trimmed. `None` when neither
/// marker is present (treated as outdated so a marker-less / hand-edited
/// file is refreshed toward the embedded copy).
fn cli_version(content: &str) -> Option<String> {
    for line in content.lines().take(30) {
        for prefix in [VERSION_PREFIX, LEGACY_VERSION_PREFIX] {
            if let Some(rest) = line.strip_prefix(prefix) {
                return Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

/// Write `content` at `path` with mode 0755 (world-executable script; it
/// carries no secrets — tokens come from `~/.k2` at runtime).
fn write_exec(path: &Path, content: &str) -> Result<(), String> {
    std::fs::write(path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod 0755 {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Testable core — decide + act at an explicit `target`. Returns
/// `Ok(Some(path))` when the CLI is now staged (freshly written or
/// already current), `Ok(None)` when deliberately skipped because the
/// target is a SYMLINK (owned by the app / a dev), and `Err` only on a
/// real write/permission failure.
pub fn stage_cli_at(target: &Path) -> Result<Option<PathBuf>, String> {
    // Never clobber a symlink: the macOS app points it into its bundle,
    // and a developer may link it at their repo checkout. We manage only
    // a regular-file install.
    if std::fs::symlink_metadata(target)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let embedded = cli_version(CLI_CONTENT);
    debug_assert!(embedded.is_some(), "cli/k2 lost its K2_CLI_VERSION line");

    let current = std::fs::read_to_string(target)
        .ok()
        .and_then(|d| cli_version(&d));
    if embedded.is_some() && current == embedded {
        return Ok(Some(target.to_path_buf())); // already current — leave it
    }

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    write_exec(target, CLI_CONTENT)?;
    Ok(Some(target.to_path_buf()))
}

/// Real boot entry — Linux ONLY (macOS keeps the app-managed symlink).
/// No-op on other platforms. Staging into `/usr/local/bin/k2`.
pub fn stage_cli() -> Result<Option<PathBuf>, String> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    stage_cli_at(Path::new(INSTALL_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-cli-stage-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn embedded_cli_has_version_and_bash_shebang() {
        // The staging cycle keys off K2_CLI_VERSION; the CLI is bash.
        assert_eq!(CLI_CONTENT.lines().next(), Some("#!/bin/bash"));
        let v = cli_version(CLI_CONTENT).expect("K2_CLI_VERSION present");
        assert!(!v.is_empty(), "version value must be non-empty");
    }

    #[test]
    fn version_parse_trims_quotes_and_accepts_legacy() {
        assert_eq!(cli_version("K2_CLI_VERSION=\"1.2.3\"\n").as_deref(), Some("1.2.3"));
        assert_eq!(cli_version("K2SO_CLI_VERSION=\"0.9\"\n").as_deref(), Some("0.9"));
        assert_eq!(cli_version("# no version here\n"), None);
    }

    #[test]
    fn stages_when_absent_0755() {
        let dir = tmp_dir("absent");
        let target = dir.join("k2");
        let out = stage_cli_at(&target).expect("stage ok");
        assert_eq!(out, Some(target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), CLI_CONTENT);
        #[cfg(unix)]
        assert_eq!(mode_of(&target), 0o755, "CLI must be executable");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn idempotent_when_current() {
        let dir = tmp_dir("current");
        let target = dir.join("k2");
        stage_cli_at(&target).expect("first stage");
        // Same-version file is left untouched on the second pass.
        let again = stage_cli_at(&target).expect("second stage");
        assert_eq!(again, Some(target.clone()));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), CLI_CONTENT);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refreshes_an_outdated_regular_file() {
        let dir = tmp_dir("stale");
        let target = dir.join("k2");
        std::fs::write(&target, "#!/bin/bash\nK2_CLI_VERSION=\"0.0.1\"\necho old\n").unwrap();
        stage_cli_at(&target).expect("stage ok");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            CLI_CONTENT,
            "an older on-disk version must be refreshed"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn never_clobbers_a_symlink() {
        // Respect the macOS app's symlink / a dev's repo symlink: if the
        // target is a symlink we skip entirely and leave it in place.
        let dir = tmp_dir("symlink");
        let real = dir.join("elsewhere-k2");
        std::fs::write(&real, "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\n").unwrap();
        let target = dir.join("k2");
        std::os::unix::fs::symlink(&real, &target).unwrap();

        let out = stage_cli_at(&target).expect("stage ok");
        assert_eq!(out, None, "a symlink target must be skipped");
        assert!(
            std::fs::symlink_metadata(&target).unwrap().file_type().is_symlink(),
            "symlink must be left intact"
        );
        assert_eq!(
            std::fs::read_to_string(&real).unwrap(),
            "#!/bin/bash\nK2_CLI_VERSION=\"9.9.9\"\n",
            "the symlink's real target must be untouched"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
