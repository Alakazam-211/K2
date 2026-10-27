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

/// Whether a workspace token is a filesystem path (vs a handle, name or
/// id): it starts with `/`, or, on Windows, it is an absolute Windows path
/// (`D:\ws`, `\\server\share\ws`). Exactly `starts_with('/')` on Unix.
pub fn is_path_token(token: &str) -> bool {
    token.starts_with('/') || (cfg!(windows) && std::path::Path::new(token).is_absolute())
}

/// `path` is `root` or inside it, compared as strings: the byte after
/// `root` must be a separator (`/`; `\` too on Windows), so `/a/bc` is not
/// under `/a/b`. Exactly the old `starts_with(root + "/")` test on Unix.
pub fn is_under(path: &str, root: &str) -> bool {
    path == root
        || (path.starts_with(root)
            && path.as_bytes().get(root.len()).is_some_and(|b| *b == b'/' || (cfg!(windows) && *b == b'\\')))
}

/// A relative path as a `/`-joined string on every OS: the portable form
/// K2 keeps in bundles, manifests, rule lists and reports (Windows'
/// `to_string_lossy` would give `a\b`). Same string as `to_string_lossy`
/// for a clean relative Unix path.
pub fn slash_rel(rel: &std::path::Path) -> String {
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
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
    fn path_tokens_are_absolute_paths_only() {
        assert!(is_path_token("/Users/a/ws"));
        for t in ["sales", "sales/reviewer", "rosson.k2.dev::sales", "", "ws\\x"] {
            assert!(!is_path_token(t), "{t:?}");
        }
        assert_eq!(is_path_token(r"D:\ws"), cfg!(windows));
        assert_eq!(is_path_token(r"\\server\share\ws"), cfg!(windows));
    }

    #[test]
    fn slash_rel_is_slash_joined_on_every_os() {
        let p = PathBuf::from(".k2").join("vault").join("vault.db");
        assert_eq!(slash_rel(&p), ".k2/vault/vault.db");
        assert_eq!(slash_rel(std::path::Path::new("one")), "one");
    }

    #[test]
    fn helpers_derive_from_k2_home() {
        assert_eq!(k2_bin(), k2_home().join("bin"));
        assert_eq!(k2_hooks(), k2_home().join("hooks"));
    }
}
