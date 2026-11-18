//! Whether the shell CLI has a Python 3 to run. Its readers pipe the
//! daemon's answers through `python3`, the one `command -v python3` finds.
//!
//! The binary's native commands read those answers themselves, so they
//! need no Python. Where the shell has one, they print what its Python
//! prints, byte for byte; [`NATIVE_WITHOUT_PYTHON3`] says what they do
//! where it has none.
//!
//! [`Python3::Missing`]: no executable `python3` on `PATH`, or the one
//! found is macOS's `/usr/bin/python3` with no developer tools behind it.
//! That file is Apple's stand-in: run, it opens the dialog that offers to
//! install the command line tools and runs no Python. It is never run
//! here: `xcode-select -p` must name a folder with `usr/bin/python3` in
//! it (the rule [`crate::daemon::usable_git`] keeps for `/usr/bin/git`).
//!
//! [`Python3::Unknown`]: the binary can't tell what the shell's `python3`
//! is. On Windows the shell's prelude tries `py -3` and `python` too
//! (`cli/src/prelude.sh`); anywhere, an exported shell function named
//! `python3` runs in its place. What depends on the shell's Python goes
//! to the shell then.

use crate::daemon;
use crate::offline::Env;
use std::path::{Path, PathBuf};

/// A native command where the shell has no `python3` ([`Python3::Missing`]).
/// True (K2's ruling): it runs, and prints what it prints where there is
/// one; that is a named difference (`native-without-python3` in
/// `scripts/cli-parity.sh`). False: it goes to the shell, as when the
/// binary can't tell.
pub const NATIVE_WITHOUT_PYTHON3: bool = true;

/// macOS's stand-in for `python3`.
pub const MACOS_STAND_IN: &str = "/usr/bin/python3";

/// The shell's `python3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Python3 {
    /// There is one, and it runs Python.
    Present,
    /// There is none (or only macOS's stand-in with no developer tools).
    Missing,
    /// The binary can't tell (Windows, or a `python3` shell function).
    Unknown,
}

/// The shell's `python3` in `env`.
pub fn in_env(env: &Env) -> Python3 {
    if !cfg!(unix) {
        return Python3::Unknown;
    }
    // Exported as `BASH_FUNC_python3%%` (bash 4.3 and later) or
    // `BASH_FUNC_python3()` (before).
    if env.any_named("BASH_FUNC_python3%") || env.any_named("BASH_FUNC_python3(") {
        return Python3::Unknown;
    }
    let found = env
        .get("PATH")
        .and_then(|p| daemon::find_on_path("python3", p.to_os_string()));
    judge(
        found.as_deref(),
        cfg!(target_os = "macos"),
        Path::new(MACOS_STAND_IN),
        || daemon::developer_dir().map(Path::to_path_buf),
    )
}

/// The shell's `python3` in this process's environment.
pub fn here() -> Python3 {
    in_env(&Env::current())
}

/// `found`: the `python3` on `PATH`, if any. On macOS (`macos`), when it
/// is `stand_in` (links followed), `developer_dir` must name a folder
/// with an executable `usr/bin/python3` in it. `found` is never run.
pub fn judge(
    found: Option<&Path>,
    macos: bool,
    stand_in: &Path,
    developer_dir: impl FnOnce() -> Option<PathBuf>,
) -> Python3 {
    let Some(found) = found else {
        return Python3::Missing;
    };
    if macos && same_file(found, stand_in) {
        let real =
            developer_dir().is_some_and(|d| daemon::is_executable(&d.join("usr/bin/python3")));
        if !real {
            return Python3::Missing;
        }
    }
    Python3::Present
}

/// `a` and `b` name the same file, links followed (as written when one
/// can't be followed).
fn same_file(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(a) == real(b)
}

/// Whether a native family answers for the shell's `python3` `p`: with
/// one, always (it prints what the shell's Python prints); with none, by
/// [`NATIVE_WITHOUT_PYTHON3`]; when the binary can't tell, never.
pub fn family_native(p: Python3) -> bool {
    match p {
        Python3::Present => true,
        Python3::Missing => NATIVE_WITHOUT_PYTHON3,
        Python3::Unknown => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    /// A fresh folder under the system temp folder.
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k5-python-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if d.exists() {
            std::fs::remove_dir_all(&d).unwrap();
        }
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn executable(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A stand-in that leaves `ran` behind if anything runs it.
    fn stand_in(d: &Path) -> (PathBuf, PathBuf) {
        let ran = d.join("ran");
        let stub = d.join("stub").join("python3");
        executable(
            &stub,
            &format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display()),
        );
        (stub, ran)
    }

    #[test]
    fn macos_s_stand_in_without_developer_tools_is_missing_and_never_runs() {
        let d = scratch("stand-in");
        let (stub, ran) = stand_in(&d);
        let tools = d.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        // No developer tools, or a folder without Python in it.
        assert_eq!(judge(Some(&stub), true, &stub, || None), Python3::Missing);
        assert_eq!(
            judge(Some(&stub), true, &stub, || Some(tools.clone())),
            Python3::Missing
        );
        // A link to the stand-in is the stand-in.
        let link = d.join("bin").join("python3");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&stub, &link).unwrap();
        assert_eq!(judge(Some(&link), true, &stub, || None), Python3::Missing);
        // The tools' own Python: the stand-in runs it.
        executable(&tools.join("usr/bin/python3"), "#!/bin/sh\nexit 0\n");
        assert_eq!(
            judge(Some(&stub), true, &stub, || Some(tools.clone())),
            Python3::Present
        );
        // Off macOS, or another python3: there is one, nothing asked.
        let never = || -> Option<PathBuf> { panic!("the developer tools are not asked") };
        assert_eq!(judge(Some(&stub), false, &stub, never), Python3::Present);
        let other = d.join("other").join("python3");
        executable(&other, "#!/bin/sh\nexit 0\n");
        assert_eq!(judge(Some(&other), true, &stub, never), Python3::Present);
        assert_eq!(judge(None, true, &stub, never), Python3::Missing);
        assert!(!ran.exists(), "the stand-in ran");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn python3_as_the_shell_finds_it() {
        let d = scratch("path");
        let with = d.join("with");
        executable(&with.join("python3"), "#!/bin/sh\nexit 0\n");
        let without = d.join("without");
        std::fs::create_dir_all(&without).unwrap();
        // Not executable: the shell doesn't run it.
        std::fs::write(without.join("python3"), "").unwrap();
        let at = |path: &str| in_env(&Env::of(&[("PATH", path)]));
        let (with, without) = (with.to_str().unwrap(), without.to_str().unwrap());
        assert_eq!(at(with), Python3::Present);
        assert_eq!(at(&format!("{without}:{with}")), Python3::Present);
        assert_eq!(at(without), Python3::Missing);
        assert_eq!(at(""), Python3::Missing);
        assert_eq!(in_env(&Env::of(&[])), Python3::Missing);
        for name in ["BASH_FUNC_python3%%", "BASH_FUNC_python3()"] {
            assert_eq!(
                in_env(&Env::of(&[("PATH", without), (name, "() {  :\n}")])),
                Python3::Unknown,
                "{name}"
            );
        }
        // Another function is no python3.
        assert_eq!(
            in_env(&Env::of(&[
                ("PATH", without),
                ("BASH_FUNC_python3x%%", "() { :\n}")
            ])),
            Python3::Missing
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_family_answers_by_the_switch_only_without_python3() {
        assert!(family_native(Python3::Present));
        assert!(!family_native(Python3::Unknown));
        assert_eq!(family_native(Python3::Missing), NATIVE_WITHOUT_PYTHON3);
    }
}
