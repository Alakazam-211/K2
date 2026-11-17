//! Windows: the `k2.cmd` wrapper Install CLI puts on PATH. The `k2` CLI is
//! a bash script, and Windows has no bash of its own, so the wrapper runs
//! it in Git Bash, or says plainly that Git for Windows is needed.
//!
//! Where Git Bash is looked for is written once, in [`GIT_BASH_PLACES`]
//! and [`GIT_BASH_FROM_GIT`]: the wrapper's cmd.exe code is built from
//! them, and [`find_git_bash`] is the same search for Rust (the k5 command
//! line, when it hands a verb to the bash CLI). The module builds on every
//! system so that search is tested everywhere; only Windows uses it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Where Git for Windows is downloaded.
pub const GIT_FOR_WINDOWS_URL: &str = "https://git-scm.com/download/win";

/// What a person or agent sees when `k2` can't run.
pub const GIT_BASH_NEEDED: &str =
    "K2's command-line tools need Git for Windows (Git Bash). Download it from https://git-scm.com/download/win , then open a new terminal.";

/// Where Git for Windows puts Git Bash, in the order they are tried: the
/// variable that holds a folder, and the path under it. Machine-wide,
/// 32-bit, per user.
pub const GIT_BASH_PLACES: [(&str, &str); 3] = [
    ("ProgramFiles", r"Git\bin\bash.exe"),
    ("ProgramFiles(x86)", r"Git\bin\bash.exe"),
    ("LOCALAPPDATA", r"Programs\Git\bin\bash.exe"),
];

/// Git Bash from the folder of a `git.exe` found on PATH (scoop and custom
/// folders): Git for Windows keeps `git.exe` in `cmd\` and bash in `bin\`.
pub const GIT_BASH_FROM_GIT: &str = r"..\bin\bash.exe";

/// The `k2.cmd` body that runs `script` in Git Bash. Looks where Git for
/// Windows installs ([`GIT_BASH_PLACES`]), then beside whatever `git.exe`
/// is on PATH ([`GIT_BASH_FROM_GIT`]). Never a bare `bash` from PATH:
/// `System32\bash.exe` is WSL, which would run the CLI inside Linux. No
/// Git Bash: [`GIT_BASH_NEEDED`] on stderr, exit 9009 (cmd.exe's "not
/// recognized").
pub fn cmd_body(script: &str) -> String {
    cmd_body_from(script, &cmd_candidates())
}

/// [`GIT_BASH_PLACES`] as cmd.exe's quoted list: `"%ProgramFiles%\Git\bin\bash.exe" ...`.
fn cmd_candidates() -> String {
    GIT_BASH_PLACES.iter().map(|(var, rel)| format!("\"%{var}%\\{rel}\"")).collect::<Vec<_>>().join(" ")
}

/// [`cmd_body`] with its own list of quoted Git Bash places to try first
/// (tests: Windows re-derives `%ProgramFiles%` for every new process, so a
/// test can't hide the machine's Git by setting it).
fn cmd_body_from(script: &str, candidates: &str) -> String {
    // cmd.exe would read a `)` in the echo as the end of its block.
    let msg = GIT_BASH_NEEDED.replace('(', "^(").replace(')', "^)");
    format!(
        "@echo off\r\n\
         setlocal\r\n\
         set \"K2_SCRIPT={script}\"\r\n\
         set \"K2_BASH=\"\r\n\
         for %%B in ({candidates}) do (\r\n\
           if not defined K2_BASH if exist %%B set \"K2_BASH=%%~B\"\r\n\
         )\r\n\
         if not defined K2_BASH for /f \"delims=\" %%G in ('where git.exe 2^>nul') do (\r\n\
           if not defined K2_BASH if exist \"%%~dpG{GIT_BASH_FROM_GIT}\" set \"K2_BASH=%%~dpG{GIT_BASH_FROM_GIT}\"\r\n\
         )\r\n\
         if not defined K2_BASH (\r\n\
           echo {msg} 1>&2\r\n\
           exit /b 9009\r\n\
         )\r\n\
         \"%K2_BASH%\" \"%K2_SCRIPT%\" %*\r\n\
         exit /b %ERRORLEVEL%\r\n"
    )
}

/// Git Bash on this machine, by the wrapper's search: each of
/// [`GIT_BASH_PLACES`] under its variable, then [`GIT_BASH_FROM_GIT`]
/// from the folder of each `git.exe` on `PATH`, in `PATH`'s order. The
/// first that `is_file` accepts wins. Never a bare `bash` from PATH (that
/// is WSL). Unlike `where`, the current folder is not searched for
/// `git.exe`. `env` reads a variable (Windows ignores case in their
/// names); paths are joined with `\` on every system, so the search is
/// the same wherever it is tested.
pub fn find_git_bash(env: impl Fn(&str) -> Option<OsString>, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let under = |base: OsString, rel: &str| {
        let mut p = base;
        p.push("\\");
        p.push(rel);
        PathBuf::from(p)
    };
    for (var, rel) in GIT_BASH_PLACES {
        if let Some(base) = env(var).filter(|b| !b.is_empty()) {
            let bash = under(base, rel);
            if is_file(&bash) {
                return Some(bash);
            }
        }
    }
    let path = env("PATH")?;
    for dir in path.to_string_lossy().split(';') {
        let dir = dir.trim().trim_matches('"').trim_end_matches('\\');
        if dir.is_empty() || dir.contains('\u{fffd}') {
            continue;
        }
        if is_file(&under(dir.into(), "git.exe")) {
            let bash = under(dir.into(), GIT_BASH_FROM_GIT);
            if is_file(&bash) {
                return Some(bash);
            }
        }
    }
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-cmd-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path) -> PathBuf {
        write_with(dir, None)
    }

    /// `candidates`: replace the standard Git Bash places (`None`: keep).
    fn write_with(dir: &Path, candidates: Option<&str>) -> PathBuf {
        let script = dir.join("k2");
        std::fs::write(&script, "#!/usr/bin/env bash\necho \"from-bash:$1:$2\"\nexit 3\n").unwrap();
        let cmd = dir.join("k2.cmd");
        let s = script.display().to_string();
        let body = match candidates {
            Some(c) => cmd_body_from(&s, c),
            None => cmd_body(&s),
        };
        std::fs::write(&cmd, body).unwrap();
        cmd
    }

    #[test]
    fn runs_the_script_in_git_bash_and_passes_args_and_the_exit_code() {
        let d = scratch("run");
        let out = Command::new("cmd").args(["/d", "/c"]).arg(write(&d)).args(["a b", "c"]).output().unwrap();
        assert_eq!(out.status.code(), Some(3), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "from-bash:a b:c");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_git_for_windows_it_says_so_with_the_link() {
        let d = scratch("nogit");
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let sys = std::env::var("SystemRoot").unwrap();
        // None of the usual places has Git Bash, and PATH reaches no git.exe
        // (nor System32's WSL bash, which must never be used).
        let nowhere = format!("\"{}\"", empty.join("bash.exe").display());
        let out = Command::new("cmd")
            .args(["/d", "/c"])
            .arg(write_with(&d, Some(&nowhere)))
            .env("PATH", format!(r"{sys}\System32"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(9009), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), GIT_BASH_NEEDED);
        assert!(out.stdout.is_empty(), "{out:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use std::collections::HashMap;

    /// The cmd.exe list before it was built from [`GIT_BASH_PLACES`]: the
    /// wrapper must not change.
    #[test]
    fn the_wrapper_s_places_are_the_ones_it_always_had() {
        assert_eq!(
            cmd_candidates(),
            "\"%ProgramFiles%\\Git\\bin\\bash.exe\" \"%ProgramFiles(x86)%\\Git\\bin\\bash.exe\" \"%LOCALAPPDATA%\\Programs\\Git\\bin\\bash.exe\""
        );
        let body = cmd_body("C:\\K2\\cli\\k2");
        assert!(body.contains("for %%B in (\"%ProgramFiles%\\Git\\bin\\bash.exe\" "), "{body}");
        assert!(
            body.contains("if not defined K2_BASH if exist \"%%~dpG..\\bin\\bash.exe\" set \"K2_BASH=%%~dpG..\\bin\\bash.exe\"\r\n"),
            "{body}"
        );
        assert!(GIT_BASH_NEEDED.contains(GIT_FOR_WINDOWS_URL));
    }

    fn search(env: &[(&str, &str)], files: &[&str]) -> Option<String> {
        let env: HashMap<String, OsString> = env.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        find_git_bash(|name| env.get(name).cloned(), |p| files.iter().any(|f| f == p))
            .map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn the_places_in_order_then_git_on_path() {
        let env = [
            ("ProgramFiles", "C:\\Program Files"),
            ("ProgramFiles(x86)", "C:\\Program Files (x86)"),
            ("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local"),
            ("PATH", "C:\\Windows\\System32;\"D:\\scoop\\apps\\git\\cmd\";E:\\git\\cmd\\"),
        ];
        let all = [
            "C:\\Program Files\\Git\\bin\\bash.exe",
            "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
            "C:\\Users\\a\\AppData\\Local\\Programs\\Git\\bin\\bash.exe",
            "D:\\scoop\\apps\\git\\cmd\\git.exe",
            "D:\\scoop\\apps\\git\\cmd\\..\\bin\\bash.exe",
            "E:\\git\\cmd\\git.exe",
            "E:\\git\\cmd\\..\\bin\\bash.exe",
        ];
        for skip in 0..all.len() {
            let have: Vec<&str> = all[skip..].to_vec();
            let want = match skip {
                0 => Some(all[0]),
                1 => Some(all[1]),
                2 => Some(all[2]),
                // System32 has no git.exe: the quoted scoop entry is next.
                3 => Some(all[4]),
                // A bash with no git.exe beside it does not count: the
                // next PATH entry (its trailing `\` dropped).
                4 | 5 => Some(all[6]),
                _ => None,
            };
            assert_eq!(search(&env, &have).as_deref(), want, "with only {have:?}");
        }
    }

    #[test]
    fn never_a_bare_bash_from_path_and_nothing_without_git() {
        let env = [("PATH", "C:\\Windows\\System32;C:\\tools")];
        assert_eq!(search(&env, &["C:\\Windows\\System32\\bash.exe", "C:\\tools\\bash.exe"]), None);
        assert_eq!(search(&[], &["C:\\Program Files\\Git\\bin\\bash.exe"]), None);
        // An empty variable is not a folder: no `\Git\bin\bash.exe` at the root.
        assert_eq!(search(&[("ProgramFiles", "")], &["\\Git\\bin\\bash.exe"]), None);
    }
}
