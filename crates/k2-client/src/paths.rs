//! Canonical K2 home-directory paths (PRD `prd-k2so-cleanup-v1.md` §3.1).
//!
//! `~/.k2` is the ONE home dir. The legacy `~/.k2so` is a compatibility
//! SYMLINK maintained by `migration_home` for external tools that
//! memorized the old path — no K2 code may construct a `.k2so` path
//! (enforced by `scripts/k2so-gate.sh` in CI). Every subsystem that needs
//! "the K2 home" resolves it here so the answer can never drift again.

use std::path::PathBuf;

/// The user's home directory: the `~` every K2 path hangs off.
///
/// Unix: `dirs::home_dir()` (`$HOME`, then the password database).
///
/// Windows: `HOME` when it holds an absolute Windows path, else
/// `USERPROFILE`, else the profile folder. `dirs::home_dir()` alone asks
/// the shell for the profile folder and ignores both variables, but the
/// `k2` CLI runs in Git Bash, which resolves `~` from `HOME` and then
/// `USERPROFILE`; this order keeps the daemon, the app and the CLI on the
/// same `~/.k2`. An MSYS-style `HOME` (`/c/Users/x`) is not absolute on
/// Windows and is skipped.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        windows_home_from(std::env::var_os("HOME"), std::env::var_os("USERPROFILE")).or_else(dirs::home_dir)
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir()
    }
}

/// The Windows rule of [`home_dir`] over explicit `HOME` / `USERPROFILE`
/// values: the first that is an absolute path.
#[cfg(windows)]
fn windows_home_from(home: Option<std::ffi::OsString>, profile: Option<std::ffi::OsString>) -> Option<PathBuf> {
    [home, profile].into_iter().flatten().map(PathBuf::from).find(|p| p.is_absolute())
}

/// The K2 home directory: `$HOME/.k2`.
///
/// Falls back to a relative `.k2` when no home directory can be resolved
/// — the same degrade `db::db_dir` has always used, so a pathological
/// environment behaves identically across subsystems instead of panicking
/// in some and degrading in others.
pub fn k2_home() -> PathBuf {
    home_dir().unwrap_or_default().join(".k2")
}

/// `~/.k2/bin` — staged sidecar binaries (frpc, k2-open, xdg-open).
pub fn k2_bin() -> PathBuf {
    k2_home().join("bin")
}

/// `~/.k2/hooks` — agent lifecycle hook scripts (notify.sh).
pub fn k2_hooks() -> PathBuf {
    k2_home().join("hooks")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k2_home_ends_with_dot_k2_never_k2so() {
        let p = k2_home();
        assert!(p.ends_with(".k2"), "canonical home must be ~/.k2: {p:?}");
        assert!(
            !p.to_string_lossy().contains(".k2so"),
            "legacy name must never appear in the canonical path"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_home_prefers_an_absolute_home_then_the_profile() {
        use std::ffi::OsString;
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(windows_home_from(os(r"D:\h"), os(r"C:\Users\u")), Some(PathBuf::from(r"D:\h")));
        // Git Bash's own spelling of HOME is not a Windows path.
        assert_eq!(windows_home_from(os("/c/Users/u"), os(r"C:\Users\u")), Some(PathBuf::from(r"C:\Users\u")));
        assert_eq!(windows_home_from(None, os(r"C:\Users\u")), Some(PathBuf::from(r"C:\Users\u")));
        assert_eq!(windows_home_from(os("rel"), None), None);
    }

    #[test]
    fn helpers_derive_from_k2_home() {
        assert_eq!(k2_bin(), k2_home().join("bin"));
        assert_eq!(k2_hooks(), k2_home().join("hooks"));
    }
}
