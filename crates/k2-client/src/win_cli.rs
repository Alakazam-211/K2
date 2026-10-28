//! Windows: the `k2.cmd` wrapper Install CLI puts on PATH. The `k2` CLI is
//! a bash script, and Windows has no bash of its own, so the wrapper runs
//! it in Git Bash, or says plainly that Git for Windows is needed.

/// What a person or agent sees when `k2` can't run.
pub const GIT_BASH_NEEDED: &str =
    "K2's command-line tools need Git for Windows (Git Bash). Download it from https://git-scm.com/download/win , then open a new terminal.";

/// The `k2.cmd` body that runs `script` in Git Bash. Looks where Git for
/// Windows installs (machine, 32-bit, per-user), then beside whatever
/// `git.exe` is on PATH (scoop and custom folders). Never a bare `bash`
/// from PATH: `System32\bash.exe` is WSL, which would run the CLI inside
/// Linux. No Git Bash: [`GIT_BASH_NEEDED`] on stderr, exit 9009 (cmd.exe's
/// "not recognized").
pub fn cmd_body(script: &str) -> String {
    cmd_body_from(
        script,
        "\"%ProgramFiles%\\Git\\bin\\bash.exe\" \"%ProgramFiles(x86)%\\Git\\bin\\bash.exe\" \"%LOCALAPPDATA%\\Programs\\Git\\bin\\bash.exe\"",
    )
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
           if not defined K2_BASH if exist \"%%~dpG..\\bin\\bash.exe\" set \"K2_BASH=%%~dpG..\\bin\\bash.exe\"\r\n\
         )\r\n\
         if not defined K2_BASH (\r\n\
           echo {msg} 1>&2\r\n\
           exit /b 9009\r\n\
         )\r\n\
         \"%K2_BASH%\" \"%K2_SCRIPT%\" %*\r\n\
         exit /b %ERRORLEVEL%\r\n"
    )
}

#[cfg(test)]
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
