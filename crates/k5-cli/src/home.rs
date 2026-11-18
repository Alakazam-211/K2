//! The K2 home and a workspace's private folder, as the CLI finds them.
//!
//! Both come from k2-client, the code the daemon and the app use, so the
//! three agree; nothing here is a second copy of either rule:
//!
//! - the K2 home is `k2_client::paths::k2_home_from`: the home override
//!   (`K5_HOME_OVERRIDE`, then `K2_HOME_OVERRIDE`, read as the daemon reads
//!   it) when one is set, else the K2 home folder in the user's home.
//! - a workspace's folder is `k2_client::workspace_dir::workspace_dot_dir`.
//!
//! The shell CLI finds the home its own way (`cli/src/prelude.sh`): it
//! ignores the override and falls back to the old folder name when the
//! new one is not a folder. Those cases differ from this module today and
//! wait for a ruling; `tests/home_parity.rs` lists each one. For a
//! workspace's folder the two agree.

use k2_client::{env_names, paths};
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

pub use k2_client::workspace_dir::workspace_dot_dir;

/// Why the K2 home can't be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeError {
    /// The override is set but is not an absolute path.
    Override(env_names::Malformed),
    /// No override, and no user home to put the K2 home in.
    NoUserHome,
}

impl fmt::Display for HomeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HomeError::Override(e) => fmt::Display::fmt(e, f),
            HomeError::NoUserHome => f.write_str("no home folder: set HOME"),
        }
    }
}

impl std::error::Error for HomeError {}

/// The K2 home for this process.
pub fn k2_home() -> Result<PathBuf, HomeError> {
    k2_home_with(|name| std::env::var_os(name), paths::home_dir)
}

/// [`k2_home`] over a lookup of the environment and the user home.
pub fn k2_home_with(
    lookup: impl Fn(&str) -> Option<OsString>,
    user_home: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf, HomeError> {
    let read = env_names::first_set_os_in(env_names::HOME_OVERRIDE, lookup);
    paths::k2_home_from(read, user_home)
        .map_err(HomeError::Override)?
        .ok_or(HomeError::NoUserHome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn abs(p: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!("D:{}", p.replace('/', "\\")))
        } else {
            PathBuf::from(p)
        }
    }

    #[test]
    fn the_errors_name_what_to_fix() {
        let none = |_: &str| None;
        assert_eq!(k2_home_with(none, || None), Err(HomeError::NoUserHome));
        assert_eq!(
            HomeError::NoUserHome.to_string(),
            "no home folder: set HOME"
        );
        let rel = |name: &str| (name == "K2_HOME_OVERRIDE").then(|| OsString::from("rel/home"));
        let err =
            k2_home_with(rel, || Some(abs("/u"))).expect_err("a relative override is an error");
        let HomeError::Override(m) = &err else {
            panic!("{err:?}")
        };
        assert_eq!(m.name, "K2_HOME_OVERRIDE");
        assert!(
            err.to_string()
                .starts_with("K2_HOME_OVERRIDE=\"rel/home\" is not valid: "),
            "{err}"
        );
    }

    #[test]
    fn the_home_is_k2_client_s_answer() {
        let none = |_: &str| None;
        assert_eq!(
            k2_home_with(none, || Some(abs("/u"))),
            Ok(paths::k2_home_in(abs("/u")))
        );
        let over = abs("/rig/home");
        let set = |name: &str| (name == "K5_HOME_OVERRIDE").then(|| over.clone().into_os_string());
        assert_eq!(k2_home_with(set, || None), Ok(over.clone()));
        assert!(Path::new(&over).is_absolute());
    }
}
