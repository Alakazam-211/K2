//! Background child processes (see [`k2_client::bg_proc`]): every helper
//! program the daemon or the desktop app starts goes through [`command`]
//! or [`tokio_command`], so on Windows no console window ever pops up and
//! steals focus. The source guard below fails on a raw `Command::new` in
//! k2-core, k2-daemon, k2-client or src-tauri.

use std::ffi::OsStr;

pub use k2_client::bg_proc::{command, hide, CREATE_NO_WINDOW};

/// [`command`] for tokio: windowless on Windows, plain elsewhere.
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
