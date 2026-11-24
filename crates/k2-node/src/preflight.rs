//! What a job's start needs, checked right before the spawn (0.46.18).
//!
//! A spawn that can't `chdir` into its working directory fails with the
//! same `ENOENT` as one whose program isn't there, and Rust's
//! `Command::spawn` names only the program: jobs 9f22e22f (k2n16) and
//! e1f2a581 (k2n21) died at once with `k2-node job-exec: run bash: No such
//! file or directory (os error 2)`, even as `/bin/bash`, which was there.
//! So the `job-exec` shim, the node's own spawn and the node's last step
//! before a start check both first and say which one is missing.
//!
//! The checks only ever turn a spawn that would fail with `ENOENT` into a
//! message that says why: a folder or program that exists but can't be
//! used (no permission, not executable) is left to the spawn, which says
//! so itself.

use std::path::{Path, PathBuf};

/// The `PATH` an `execvp` uses when the env has none (glibc's and the
/// BSDs' default).
pub const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// The job's working directory must be a folder. `Err` (the whole reason,
/// no prefix) only when it is missing or isn't a folder.
pub fn cwd_ready(cwd: &Path) -> Result<(), String> {
    match std::fs::metadata(cwd) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(format!("working folder {} is not a folder", cwd.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("working folder {} is missing", cwd.display())),
        // Anything else (a folder on the way it may not search) is the
        // spawn's to report.
        Err(_) => Ok(()),
    }
}

/// Where `program` resolves for a spawn in `cwd` with `path` (the job's
/// own `PATH`; `None` = [`DEFAULT_PATH`]), as `execvp` looks: a name with a
/// `/` as it is (relative to `cwd`), any other name in each `PATH` entry
/// in turn (an empty entry is `cwd`). `Err` only when nothing of that
/// name (but a folder) is anywhere it looks.
pub fn program_ready(program: &str, cwd: &Path, path: Option<&str>) -> Result<PathBuf, String> {
    if program.is_empty() {
        return Err("empty program name".into());
    }
    // Gone for an exec: nothing there (a dangling link too), or a part of
    // the way that isn't a folder. Anything else (a file, a folder it may
    // not search) is the spawn's to judge.
    let absent = |p: &Path| match std::fs::metadata(p) {
        Ok(m) => m.is_dir(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ENOTDIR),
    };
    if program.contains('/') {
        let p = cwd.join(program);
        return if absent(&p) { Err(format!("program {program} not found")) } else { Ok(p) };
    }
    let path = path.unwrap_or(DEFAULT_PATH);
    for dir in path.split(':') {
        let p = if dir.is_empty() { cwd.join(program) } else { cwd.join(dir).join(program) };
        if !absent(&p) {
            return Ok(p);
        }
    }
    Err(format!("program {program} not found on PATH={path}"))
}

/// Both checks, the working directory first (a missing one fails any
/// program, `/bin/bash` too).
pub fn spawn_ready(program: &str, cwd: &Path, path: Option<&str>) -> Result<(), String> {
    cwd_ready(cwd)?;
    program_ready(program, cwd, path).map(|_| ())
}

/// The `PATH` in a job's env pairs (the last one wins, as in `Command`).
pub fn path_of(env: &[(String, String)]) -> Option<&str> {
    env.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| v.as_str())
}

/// A spawn failed: when it is `ENOENT`, what is missing now (the folder
/// may have gone between the check and the spawn); else, or when both
/// are there again, the spawn's own error.
pub fn spawn_error(program: &str, cwd: &Path, path: Option<&str>, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        if let Err(why) = spawn_ready(program, cwd, path) {
            return why;
        }
    }
    format!("run {program}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_working_directory_is_named_not_the_program() {
        let d = crate::util::temp_dir("preflight-cwd");
        let gone = d.join("src");
        assert_eq!(cwd_ready(&gone), Err(format!("working folder {} is missing", gone.display())));
        // `/bin/sh` is there; the folder isn't: the folder is what's named.
        assert_eq!(spawn_ready("/bin/sh", &gone, Some("/usr/bin:/bin")), Err(format!("working folder {} is missing", gone.display())));
        assert_eq!(spawn_ready("sh", &gone, None), Err(format!("working folder {} is missing", gone.display())));
        let file = d.join("f");
        std::fs::write(&file, "").unwrap();
        assert_eq!(cwd_ready(&file), Err(format!("working folder {} is not a folder", file.display())));
        std::fs::create_dir_all(&gone).unwrap();
        assert_eq!(cwd_ready(&gone), Ok(()));
        assert_eq!(spawn_ready("/bin/sh", &gone, None), Ok(()));
    }

    #[test]
    fn a_program_resolves_as_execvp_looks_or_is_named_with_the_path() {
        let d = crate::util::temp_dir("preflight-prog");
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool"), "#!/bin/sh\n").unwrap();
        assert_eq!(program_ready("tool", &d, Some("/nowhere:bin")), Ok(d.join("bin").join("tool")), "a relative PATH entry is the cwd's");
        assert_eq!(program_ready("tool", &bin, Some("")), Ok(bin.join("tool")), "an empty entry is the cwd");
        assert_eq!(program_ready("tool", &d, Some("/nowhere:/also-not")), Err("program tool not found on PATH=/nowhere:/also-not".into()));
        assert_eq!(program_ready("sh", &d, None), Ok(PathBuf::from("/bin/sh")), "no PATH: execvp's default");
        assert_eq!(program_ready("./bin/tool", &d, None), Ok(d.join("./bin/tool")));
        assert_eq!(program_ready("/no/such/bash", &d, None), Err("program /no/such/bash not found".into()));
        assert!(program_ready("", &d, None).is_err());
        // The folder first: a missing one is the reason even when the
        // program is missing too.
        assert_eq!(spawn_ready("/no/such/bash", &d.join("gone"), None), Err(format!("working folder {} is missing", d.join("gone").display())));
        // A file that isn't executable is the spawn's to report (EACCES).
        std::fs::write(d.join("plain"), "").unwrap();
        assert!(program_ready("plain", &d, Some(&d.display().to_string())).is_ok());
    }

    #[test]
    fn a_spawn_error_names_what_is_missing_now() {
        let d = crate::util::temp_dir("preflight-err");
        let enoent = std::io::Error::from_raw_os_error(libc::ENOENT);
        let gone = d.join("src");
        assert_eq!(spawn_error("/bin/sh", &gone, None, &enoent), format!("working folder {} is missing", gone.display()));
        // Both there: the spawn's own words.
        assert_eq!(spawn_error("/bin/sh", &d, None, &enoent), "run /bin/sh: No such file or directory (os error 2)");
        let eacces = std::io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(spawn_error("/bin/sh", &gone, None, &eacces), "run /bin/sh: Permission denied (os error 13)");
        assert_eq!(path_of(&[("PATH".into(), "/a".into()), ("X".into(), "1".into()), ("PATH".into(), "/b".into())]), Some("/b"));
        assert_eq!(path_of(&[]), None);
    }
}
