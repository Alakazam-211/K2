//! Where the built `k5-cli` writes, against `bash cli/k2`: a broken pipe
//! on stdout ends both by SIGPIPE, and the notice for callers of `k2` is
//! not printed (it is off until the switch).
//!
//! Every run gets an empty environment but `PATH`, a temporary `HOME` and
//! a dead port with a dummy token, as tests/front_binary.rs does.

#![cfg(unix)]

mod common;

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");

static N: AtomicUsize = AtomicUsize::new(0);

/// A temporary folder with `bin/k2` and `bin/k5` linked to the binary and
/// an empty `home`.
struct Rig {
    dir: PathBuf,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!(
            "k5-out-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        for name in ["k2", "k5"] {
            std::os::unix::fs::symlink(BIN, dir.join("bin").join(name)).unwrap();
        }
        Rig { dir }
    }

    fn cmd(&self, program: PathBuf) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", self.dir.join("home"))
            .env("K2_PORT", "1")
            .env("K2_HOOK_TOKEN", "test-token");
        cmd
    }

    fn front(&self, name: &str) -> Command {
        self.cmd(self.dir.join("bin").join(name))
    }

    fn bash(&self) -> Command {
        let mut cmd = self.cmd(PathBuf::from("bash"));
        cmd.arg(common::cli_src().join("../k2"));
        cmd
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Runs `cmd` with stdout a pipe whose reader is already gone. The child
/// starts with SIGPIPE at its default (std resets it for children).
fn with_no_reader(cmd: &mut Command) -> Output {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("stderr is piped")
        .read_to_end(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();
    Output {
        status,
        stdout: Vec::new(),
        stderr,
    }
}

#[test]
fn a_broken_pipe_on_version_ends_by_sigpipe_as_bash_does() {
    let rig = Rig::new("pipe");
    for one in ["--version", "-v", "version"] {
        let bash = with_no_reader(rig.bash().arg(one));
        assert_eq!(
            bash.status.signal(),
            Some(libc::SIGPIPE),
            "bash cli/k2 {one}: {bash:?}"
        );
        // Linux's bash says nothing. macOS's /bin/bash 3.2 lets its exit
        // trap catch the signal, so its echo first says why, with the
        // script's path and line. The front says nothing anywhere.
        let said = String::from_utf8_lossy(&bash.stderr);
        assert!(
            said.is_empty() || said.ends_with("echo: write error: Broken pipe\n"),
            "bash cli/k2 {one}: {said}"
        );
        for name in ["k2", "k5"] {
            let front = with_no_reader(rig.front(name).arg(one));
            assert_eq!(
                front.status.signal(),
                Some(libc::SIGPIPE),
                "{name} {one}: {front:?}"
            );
            assert_eq!(String::from_utf8_lossy(&front.stderr), "", "{name} {one}");
        }
    }
}

/// The notice is off: called as `k2`, nothing on stderr for `--version`,
/// and no notice when a verb falls through.
#[test]
fn the_notice_is_not_printed_today() {
    let rig = Rig::new("notice");
    for name in ["k2", "k5"] {
        let out = rig.front(name).arg("--version").output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{name}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "",
            "{name} --version wrote to stderr"
        );
        let out = rig.front(name).arg("nosuchverb").output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{name}: {out:?}");
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(!said.is_empty(), "{name} nosuchverb said nothing on stderr");
        assert!(
            !said.contains(k5_cli::notice::TEXT),
            "{name} nosuchverb: {said}"
        );
        assert!(!said.contains("k2 is now k5"), "{name} nosuchverb: {said}");
    }
}
