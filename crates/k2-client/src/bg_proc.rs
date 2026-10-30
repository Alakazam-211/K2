//! Background child processes: the one way K2 starts a helper program
//! (git, powercfg, a CLI `--version` probe, frpc, a shell, …).
//!
//! On Windows the release daemon and the desktop app are GUI-subsystem
//! programs with no console. A console program they start (powercfg.exe,
//! git.exe, cmd.exe for a `.cmd` shim) gets a brand-new console window,
//! which pops up and steals focus. [`command`] sets `CREATE_NO_WINDOW`, so
//! the child gets a hidden console instead, and its own console children
//! share that hidden console. GUI programs (an editor, Explorer) ignore
//! the flag and open normally. On macOS and Linux it is a plain
//! `Command::new`.
//!
//! A person's terminal (a PTY tab) is not started here: ConPTY gives it
//! its own pseudo console. Starting any other process with a raw
//! `Command::new` fails the source guard in `k2_core::bg_proc`.

use std::ffi::OsStr;
use std::process::Command;

/// `CREATE_NO_WINDOW`: a console child gets a hidden console (Windows).
/// Callers that add flags (a new process group, a suspended start) OR
/// this in, because `creation_flags` replaces what was set before.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `Command::new(program)` that never opens a console window on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program); // bg_proc: allow (the helper itself)
    hide(&mut c);
    c
}

/// Mark an already-built `Command` windowless (Windows); a no-op elsewhere.
pub fn hide(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_background_command_runs_and_returns_its_output() {
        #[cfg(windows)]
        let out = super::command("cmd").args(["/C", "echo", "k2-bg"]).output().expect("cmd runs");
        #[cfg(not(windows))]
        let out = super::command("sh").args(["-c", "echo k2-bg"]).output().expect("sh runs");
        assert!(out.status.success(), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "k2-bg");
    }
}
