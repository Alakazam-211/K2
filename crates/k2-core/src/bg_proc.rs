//! Background child processes (see [`k2_client::bg_proc`]): every helper
//! program the daemon or the desktop app starts goes through [`command`]
//! or [`tokio_command`], so on Windows no console window ever pops up and
//! steals focus. The source guard below fails on a raw `Command::new` in
//! k2-core, k2-daemon, k2-client or src-tauri.

//!
//! On Unix every such child also runs in a session of its own (`setsid`),
//! except the daemon's own `sudo` calls ([`SUDO_PATHS`]) and what asks for
//! [`command_keeping_session`]. The root helper takes a `sudo` in the
//! daemon's session, whose parent is the daemon, for the daemon itself; a
//! child that stayed in that session and then ran `sudo` (a `k2 job`'s
//! argv, a program on PATH the daemon runs that another user of this
//! account can replace) would pass that check too.

use std::ffi::OsStr;
use std::process::Command;

pub use k2_client::bg_proc::{hide, CREATE_NO_WINDOW};

/// The `sudo` the daemon reaches the root helper (and the older helpers)
/// through. Only these keep the daemon's session.
pub const SUDO_PATHS: &[&str] = &["/usr/bin/sudo", "/usr/bin/sudo-rs", "/usr/lib/cargo/bin/sudo"];

/// A background child: windowless on Windows; on Unix in its own session
/// unless it is one of [`SUDO_PATHS`].
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let keep = keeps_session(program.as_ref());
    let mut c = k2_client::bg_proc::command(program);
    if !keep {
        in_own_session(&mut c);
    }
    c
}

/// Only the daemon's own `sudo`, by its absolute path.
pub fn keeps_session(program: &OsStr) -> bool {
    SUDO_PATHS.iter().any(|s| program == OsStr::new(s))
}

/// A background child that stays in this process's session: the daemon's
/// own `sudo` calls (also kept by [`command`] by path), and a helper that
/// runs inside a terminal session rather than under the daemon (`k2 vault
/// run`, whose program reads the terminal).
pub fn command_keeping_session(program: impl AsRef<OsStr>) -> Command {
    k2_client::bg_proc::command(program)
}

/// Make `c`'s child lead a session of its own (Unix; nothing elsewhere).
/// Idempotent: a later `setsid` of the caller's own is harmless.
pub fn in_own_session(c: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `own_session` only calls async-signal-safe functions
        // (getsid, getpid, setsid) and allocates nothing.
        unsafe {
            c.pre_exec(own_session);
        }
    }
    #[cfg(not(unix))]
    let _ = c;
}

/// In a forked child before exec: lead a new session, unless it already
/// does. Fails (and so fails the spawn) when the child can't: it is a
/// process-group leader already (`process_group(0)` asks for that; use
/// the session instead, whose group has the same id).
#[cfg(unix)]
pub fn own_session() -> std::io::Result<()> {
    // SAFETY: plain syscalls on this process.
    unsafe {
        if libc::getsid(0) == libc::getpid() {
            return Ok(());
        }
        if libc::setsid() == -1 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// [`command`] for tokio: windowless on Windows, in its own session on
/// Unix.
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    tokio::process::Command::from(command(program))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::git_safe::tests::{is_test_file, non_test_lines, rs_files};

    const RAW: &str = concat!("Command", "::new");
    const MARK: &str = concat!("bg_proc", ": allow (");

    /// Every non-test `Command::new` the guard flags, as `file:line: code`.
    fn raw_spawns(roots: &[PathBuf]) -> (Vec<String>, Vec<String>) {
        let mut files = Vec::new();
        for r in roots {
            rs_files(r, &mut files);
        }
        assert!(files.len() > 300, "the scan found the sources ({} files)", files.len());
        let (mut bad, mut allowed) = (Vec::new(), Vec::new());
        for f in &files {
            if is_test_file(f) {
                continue;
            }
            let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
            let kept = non_test_lines(f, &src);
            for (idx, (n, line)) in kept.iter().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") || !code.contains(RAW) {
                    continue;
                }
                let before_comment = code.split("//").next().unwrap_or(code);
                if !before_comment.contains(RAW) {
                    continue;
                }
                let prev = if idx > 0 { kept[idx - 1].1.as_str() } else { "" };
                let at = format!("{}:{n}", f.display());
                if line.contains(MARK) || prev.contains(MARK) {
                    allowed.push(at);
                } else {
                    bad.push(format!("{at}: {}", code.trim_end()));
                }
            }
        }
        (bad, allowed)
    }

    /// The daemon (release build) and the desktop app are GUI-subsystem on
    /// Windows, so a raw `Command::new` of a console program opens a
    /// visible console window. Route it through `bg_proc::command` /
    /// `bg_proc::tokio_command`; a deliberate exception carries
    /// `bg_proc: allow (<reason>)` on the line or the line above.
    #[test]
    fn no_raw_command_new_outside_bg_proc() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let roots: Vec<PathBuf> = ["k2-core/src", "k2-daemon/src", "k2-client/src", "../src-tauri/src"]
            .iter()
            .map(|r| crates.join(r))
            .collect();
        let (bad, allowed) = raw_spawns(&roots);
        assert!(
            bad.is_empty(),
            "raw Command::new outside bg_proc (use k2_core::bg_proc::command / tokio_command, \
             k2_client::bg_proc::command in src-tauri, or mark it `bg_proc: allow (<reason>)`):\n{}",
            bad.join("\n")
        );
        // The one allowed line is the helper itself.
        assert_eq!(allowed.len(), 1, "allow-listed raw Command::new lines: {allowed:?}");
        assert!(allowed[0].replace('\\', "/").contains("k2-client/src/bg_proc.rs:"), "{allowed:?}");
    }

    /// A child of the daemon leads its own session: a program it runs
    /// (a `k2 job`'s argv, a tool on PATH) that then runs `sudo` is not in
    /// the daemon's session, which the root helper requires of the daemon.
    #[cfg(unix)]
    #[test]
    fn a_background_child_leads_its_own_session_and_sudo_keeps_the_daemons() {
        use std::ffi::OsStr;
        let mut own = super::command("sleep").arg("5").spawn().expect("spawn sleep");
        let mut kept = super::command_keeping_session("sleep").arg("5").spawn().expect("spawn sleep");
        let mut tk_cmd = super::tokio_command("sleep");
        let mut tk = tk_cmd.as_std_mut().arg("5").spawn().expect("spawn sleep (tokio_command's)");
        // SAFETY: getsid reads session ids.
        let (s_own, s_kept, s_tk, s_me) =
            unsafe { (libc::getsid(own.id() as i32), libc::getsid(kept.id() as i32), libc::getsid(tk.id() as i32), libc::getsid(0)) };
        let own_pid = own.id() as i32;
        let tk_pid = tk.id() as i32;
        for c in [&mut own, &mut kept, &mut tk] {
            let _ = c.kill();
            let _ = c.wait();
        }
        assert_eq!(s_own, own_pid, "bg_proc::command: the child leads its own session");
        assert_ne!(s_own, s_me);
        assert_eq!(s_tk, tk_pid, "bg_proc::tokio_command: the same");
        assert_eq!(s_kept, s_me, "command_keeping_session stays in this session");
        // The daemon's own sudo (the root helper's caller check) keeps it.
        for s in super::SUDO_PATHS {
            assert!(super::keeps_session(OsStr::new(s)), "{s}");
        }
        for p in ["sudo", "/usr/local/bin/sudo", "sh", "/bin/sh", "claude", "/home/k2/.local/bin/claude"] {
            assert!(!super::keeps_session(OsStr::new(p)), "{p} must get its own session");
        }
    }

    /// No daemon spawn stays in the daemon's session except the two kinds
    /// `bg_proc` allows: its own `sudo` and `command_keeping_session`, used
    /// only by `k2 vault run` (which runs in a terminal's session, not under
    /// the daemon). And no `process_group(0)` (it would make the session
    /// impossible: a group leader can't `setsid`).
    ///
    /// Every spawn path of the daemon and k2-core is one of:
    /// - `bg_proc::command` / `tokio_command` (own session, tested above;
    ///   `no_raw_command_new_outside_bg_proc` keeps every `Command::new`
    ///   here);
    /// - a terminal's pty (the pty makes its child a session leader);
    /// - the separate `k2-vmm-worker` binary's own fork/exec (the worker
    ///   is a sandboxed terminal's program, started on that terminal's pty,
    ///   so already outside the daemon's session).
    /// A raw `fork` / `exec*` / `posix_spawn` anywhere else fails here.
    #[test]
    fn no_daemon_spawn_keeps_the_daemons_session() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for r in ["k2-core/src", "k2-daemon/src"] {
            rs_files(&crates.join(r), &mut files);
        }
        assert!(files.len() > 300, "the scan found the sources ({} files)", files.len());
        let (mut keeping, mut groups, mut raw) = (Vec::new(), Vec::new(), Vec::new());
        let raw_spawns = [concat!("libc::", "fork("), concat!("libc::", "vfork("), concat!("libc::", "execv"), concat!("libc::", "execl"), concat!("posix", "_spawn"), concat!("nix::unistd::", "fork")];
        for f in &files {
            if is_test_file(f) || f.ends_with("bg_proc.rs") {
                continue;
            }
            let src = std::fs::read_to_string(f).unwrap();
            let rel = f.display().to_string().replace('\\', "/");
            for (n, line) in non_test_lines(f, &src) {
                let code = line.split("//").next().unwrap_or("");
                let at = format!("{rel}:{n}");
                if code.contains(concat!("command_keeping", "_session(")) {
                    keeping.push(at.clone());
                }
                if code.contains(concat!(".process", "_group(")) {
                    groups.push(at.clone());
                }
                if raw_spawns.iter().any(|r| code.contains(r)) && !rel.contains("k2-daemon/src/bin/k2-vmm-worker.rs") {
                    raw.push(at);
                }
            }
        }
        assert!(raw.is_empty(), "a raw fork/exec outside bg_proc (it would keep the daemon's session): {raw:?}");
        assert_eq!(keeping.len(), 1, "spawns that keep the daemon's session: {keeping:?}");
        assert!(keeping[0].contains("k2-daemon/src/vault/run.rs:"), "{keeping:?}");
        assert!(groups.iter().all(|g| g.contains("k2-daemon/src/vault/run.rs:")), "process_group outside k2 vault run: {groups:?}");
    }

    #[test]
    fn the_guard_catches_a_raw_spawn_and_honours_the_mark() {
        let d = std::env::temp_dir().join(format!("k2-bg-proc-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..301 {
            std::fs::write(d.join(format!("f{i}.rs")), "fn x() {}\n").unwrap();
        }
        let raw = format!("fn a() {{ let _ = std::process::{RAW}(\"powercfg\"); }}\n");
        std::fs::write(d.join("raw.rs"), &raw).unwrap();
        std::fs::write(d.join("marked.rs"), format!("// {MARK}a visible terminal)\n{raw}")).unwrap();
        std::fs::write(d.join("commented.rs"), format!("// {RAW}(\"x\") in a comment\n")).unwrap();
        std::fs::write(d.join("in_tests.rs"), &raw).unwrap();
        std::fs::write(d.join("test_mod.rs"), format!("#[cfg(test)]\nmod tests {{\n{raw}}}\n")).unwrap();
        let (bad, allowed) = raw_spawns(&[d.clone()]);
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(bad[0].contains("raw.rs:1"), "{bad:?}");
        assert_eq!(allowed.len(), 1, "{allowed:?}");
        assert!(allowed[0].contains("marked.rs:2"), "{allowed:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The bug this module fixes: a parent with no console (the
    /// GUI-subsystem daemon) starts a console program. A raw spawn gets a
    /// console window of its own; a `bg_proc` spawn gets none. The test re-runs itself as that parent,
    /// started DETACHED_PROCESS (no console), and the console program
    /// (PowerShell) reports its own console window.
    #[cfg(windows)]
    #[test]
    fn a_console_child_of_a_windowless_parent_shows_no_window() {
        use std::os::windows::process::CommandExt;
        const NAME: &str = "bg_proc::tests::a_console_child_of_a_windowless_parent_shows_no_window";
        const PROBE: &str = "$t = Add-Type -Name W -Namespace K2Probe -PassThru -MemberDefinition '\
            [DllImport(\"kernel32.dll\")] public static extern System.IntPtr GetConsoleWindow(); \
            [DllImport(\"user32.dll\")] public static extern bool IsWindowVisible(System.IntPtr h);'; \
            $h = $t::GetConsoleWindow(); Write-Output (\"hwnd=\" + [int64]$h + \" visible=\" + $t::IsWindowVisible($h))";
        // Child mode: be the windowless parent and start the probe.
        if let (Ok(mode), Ok(out)) = (std::env::var("BG_PROC_PROBE_MODE"), std::env::var("BG_PROC_PROBE_OUT")) {
            let mut c = if mode == "raw" { std::process::Command::new("powershell") } else { super::command("powershell") };
            let o = c.args(["-NoProfile", "-NonInteractive", "-Command", PROBE]).output().expect("powershell runs");
            std::fs::write(&out, String::from_utf8_lossy(&o.stdout).trim()).expect("write the probe result");
            return;
        }
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let dir = std::env::temp_dir().join(format!("k2-bg-proc-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut seen = Vec::new();
        for mode in ["bg", "raw"] {
            let out = dir.join(format!("{mode}.txt"));
            let st = std::process::Command::new(std::env::current_exe().unwrap())
                .args([NAME, "--exact", "--test-threads=1"])
                .env("BG_PROC_PROBE_MODE", mode)
                .env("BG_PROC_PROBE_OUT", &out)
                .creation_flags(DETACHED_PROCESS)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("re-run this test as the windowless parent");
            assert!(st.success(), "{mode}: {st:?}");
            seen.push((mode, std::fs::read_to_string(&out).unwrap_or_else(|e| panic!("{mode}: no probe result: {e}"))));
        }
        let _ = std::fs::remove_dir_all(&dir);
        let line = |m: &str| seen.iter().find(|(k, _)| *k == m).map(|(_, v)| v.clone()).unwrap();
        // bg_proc: the console has no window at all (nothing can show).
        assert_eq!(line("bg"), "hwnd=0 visible=False", "{seen:?}");
        // The raw spawn this module replaces gets a console window of its
        // own: on a person's desktop that is the window that pops up and
        // takes focus (on a service desktop, such as an ssh session on the
        // test box, it exists but stays unshown).
        assert!(!line("raw").starts_with("hwnd=0 "), "{seen:?}");
    }

    #[tokio::test]
    async fn a_tokio_background_command_runs() {
        #[cfg(windows)]
        let out = super::tokio_command("cmd").args(["/C", "echo", "k2-bg"]).output().await.expect("cmd runs");
        #[cfg(not(windows))]
        let out = super::tokio_command("sh").args(["-c", "echo k2-bg"]).output().await.expect("sh runs");
        assert!(out.status.success(), "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "k2-bg");
    }
}
