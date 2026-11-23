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

/// Every name a workspace's private folder has had, newest first: `.k5`
/// (the coming name), `.k2`, and `.k2so` (before 0.40.4). A workspace can
/// still hold an old `.k2so/` beside its `.k2/`, and links at its root may
/// still point into it.
pub const WORKSPACE_DOT_DIR_NAMES: &[&str] = &[".k5", ".k2", ".k2so"];

/// Whether one path segment names a workspace's private folder, under any
/// of its names ([`WORKSPACE_DOT_DIR_NAMES`]). Letter case is ignored
/// (`.K2` on a case-insensitive disk is the same folder) and so are
/// trailing dots and spaces (Windows strips them: `.k2.` and `.k2 ` open
/// `.k2`).
///
/// For checks that keep someone OUT of that folder. Ask this, never
/// `== ".k2"`: a guard that knows one name leaves the others open.
pub fn is_workspace_dot_dir(segment: &str) -> bool {
    let name = segment.trim_end_matches(['.', ' ']);
    WORKSPACE_DOT_DIR_NAMES.iter().any(|n| name.eq_ignore_ascii_case(n))
}

/// The K2 home folder's name under the user's home: `.k2`, the brand
/// file's `[machine] home` (every brand has today's name for now; the
/// reader refuses another). Name it only through here; a path into the
/// home comes from [`k2_home`] (or [`k2_home_in`] for a given user home).
pub const HOME_DIR_NAME: &str = k2_brand::machine::HOME_DIR_NAME;

/// The K2 home directory: `$HOME/.k2`, unless the home override names
/// another folder (see [`home_override`]).
///
/// Falls back to a relative `.k2` when no home directory can be resolved
/// — the same degrade `db::db_dir` has always used, so a pathological
/// environment behaves identically across subsystems instead of panicking
/// in some and degrading in others.
pub fn k2_home() -> PathBuf {
    k2_home_opt().unwrap_or_else(|| PathBuf::from(HOME_DIR_NAME))
}

/// [`k2_home`], or `None` when there is no override and no user home: for
/// a caller with its own fallback (an error, a temp folder).
pub fn k2_home_opt() -> Option<PathBuf> {
    k2_home_from(crate::env_names::first_set_os(crate::env_names::HOME_OVERRIDE), home_dir).unwrap_or_else(|e| panic!("{e}"))
}

/// The rule of [`k2_home_opt`] over the override as already read and the
/// user home: the override when one is set, else `<user home>/.k2`, else
/// `None`. An override that is set but not absolute is an error naming
/// its variable ([`home_override_from`]). `user_home` is asked only when
/// there is no override. For a caller that reports the error instead of
/// panicking (the Rust CLI).
pub fn k2_home_from(
    read: Option<(&'static str, std::ffi::OsString)>,
    user_home: impl FnOnce() -> Option<PathBuf>,
) -> Result<Option<PathBuf>, crate::env_names::Malformed> {
    Ok(match home_override_from(read)? {
        Some(over) => Some(over),
        None => user_home().map(k2_home_in),
    })
}

/// The K2 home inside the user home `home`: `<home>/.k2`. For code that is
/// handed a home (another user's, a test's, a guest's); never follows the
/// override, because the caller chose the home.
pub fn k2_home_in(home: impl AsRef<std::path::Path>) -> PathBuf {
    home.as_ref().join(HOME_DIR_NAME)
}

/// The home override: the K2 home folder itself (what `~/.k2` is
/// otherwise), for rigs and tests. Read as `K5_HOME_OVERRIDE`, then
/// `K2_HOME_OVERRIDE` (`env_names::HOME_OVERRIDE`). Unset means no
/// override.
///
/// A value that is set must be an absolute path: an empty or relative one
/// panics with the variable's name, because a home that silently moves
/// with the working directory would scatter a database, a vault and keys.
pub fn home_override() -> Option<PathBuf> {
    home_override_from(crate::env_names::first_set_os(crate::env_names::HOME_OVERRIDE)).unwrap_or_else(|e| panic!("{e}"))
}

/// The rule of [`home_override`] over a value already read.
pub fn home_override_from(
    read: Option<(&'static str, std::ffi::OsString)>,
) -> Result<Option<PathBuf>, crate::env_names::Malformed> {
    let Some((name, value)) = read else { return Ok(None) };
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        return Ok(Some(path));
    }
    Err(crate::env_names::Malformed {
        name,
        value: value.to_string_lossy().into_owned(),
        why: format!("the {} home override must be an absolute path", crate::brand::PRODUCT_NAME),
    })
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

    /// Neither override name set: the home is `<user home>/.k2`, as always.
    #[test]
    fn with_no_override_the_home_is_dot_k2_under_the_user_home() {
        assert_eq!(home_override_from(None), Ok(None));
        let home = PathBuf::from("/Users/someone");
        assert_eq!(k2_home_in(&home), PathBuf::from("/Users/someone/.k2"));
        assert_eq!(HOME_DIR_NAME, ".k2");
    }

    #[test]
    fn an_absolute_override_is_the_home_itself() {
        let abs = if cfg!(windows) { r"D:\rig\k2-home" } else { "/rig/k2-home" };
        assert_eq!(home_override_from(Some(("K5_HOME_OVERRIDE", abs.into()))), Ok(Some(PathBuf::from(abs))));
        assert_eq!(home_override_from(Some(("K2_HOME_OVERRIDE", abs.into()))), Ok(Some(PathBuf::from(abs))));
    }

    #[test]
    fn an_empty_or_relative_override_fails_loudly() {
        for bad in ["", "rel/home", ".k2"] {
            let err = home_override_from(Some(("K2_HOME_OVERRIDE", bad.into()))).expect_err(bad);
            assert_eq!(err.name, "K2_HOME_OVERRIDE");
            assert!(err.to_string().contains("absolute path"), "{err}");
        }
    }

    #[test]
    fn k2_home_from_takes_the_override_then_the_user_home() {
        let abs = if cfg!(windows) { r"D:\rig\k2-home" } else { "/rig/k2-home" };
        let user = || Some(PathBuf::from("/Users/someone"));
        assert_eq!(k2_home_from(None, user), Ok(Some(PathBuf::from("/Users/someone").join(HOME_DIR_NAME))));
        assert_eq!(k2_home_from(None, || None), Ok(None));
        let asked = std::cell::Cell::new(false);
        let got = k2_home_from(Some(("K5_HOME_OVERRIDE", abs.into())), || {
            asked.set(true);
            None
        });
        assert_eq!(got, Ok(Some(PathBuf::from(abs))));
        assert!(!asked.get(), "the user home is not asked for when the override is set");
        let err = k2_home_from(Some(("K2_HOME_OVERRIDE", "".into())), user).expect_err("empty is not absolute");
        assert_eq!(err.name, "K2_HOME_OVERRIDE");
    }

    #[test]
    fn the_override_is_read_k5_name_first() {
        assert_eq!(crate::env_names::HOME_OVERRIDE, &["K5_HOME_OVERRIDE", "K2_HOME_OVERRIDE"]);
    }

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
    fn a_workspace_dot_dir_is_any_of_its_three_names_in_any_case() {
        assert_eq!(WORKSPACE_DOT_DIR_NAMES, &[".k5", ".k2", ".k2so"]);
        for s in [
            ".k5", ".k2", ".k2so", ".K5", ".K2", ".K2SO", ".K2so", ".k2So",
            // Windows opens these as the folder itself.
            ".k2.", ".k2 ", ".k2. .", ".k2so..", ".K5 ",
        ] {
            assert!(is_workspace_dot_dir(s), "{s:?}");
        }
        for s in [
            "", ".", "..", "k2", "k2so", ".k", ".k22", ".k2s", ".k2sox", ".k3", ".k2so-old", "x.k2", " .k2", ".k2/",
            ".k2\\", ".git", ".claude", "\u{212a}2", ".\u{212a}2",
        ] {
            assert!(!is_workspace_dot_dir(s), "{s:?}");
        }
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
