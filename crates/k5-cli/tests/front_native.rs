//! The binary's own verbs, run as a person runs them, against a logging
//! stub daemon ([`common::stub`]): `k5 --version --check`.
//!
//! Every run gets an empty environment but `PATH` and what the case sets,
//! a temporary `HOME` (its K2 home holds the owner token when a case puts
//! one there) and the stub's port. No real daemon, no real K2 home.

#![cfg(unix)]

mod common;

use common::stub::{Logged, Reply, Stub};
use k5_cli::transport::skew;
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");
const OWNER: &str = "owner-tok-5151";
const OTHER: &str = "0.46.99";

fn ours() -> &'static str {
    k5_cli::embedded::version()
}

static N: AtomicUsize = AtomicUsize::new(0);

struct Rig {
    dir: PathBuf,
    home: PathBuf,
    proj: PathBuf,
    stub: Stub,
}

#[derive(Debug)]
struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k5-native-{tag}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let home = dir.join("home");
        let proj = dir.join("proj");
        // The project is the run's folder: its workspace folder stops the
        // search there.
        for d in [dir.join("bin"), k2_client::paths::k2_home_in(&home), k2_client::workspace_dir::workspace_dot_dir(&proj)] {
            std::fs::create_dir_all(&d).unwrap();
        }
        for name in ["k2", "k5"] {
            std::os::unix::fs::symlink(BIN, dir.join("bin").join(name)).unwrap();
        }
        let stub = Stub::start(&PathBuf::from(format!("/tmp/k5n-{tag}-{}-{n}.sock", std::process::id())));
        Rig { dir, home, proj, stub }
    }

    /// The owner token in the K2 home, as the daemon writes it.
    fn owner_on_disk(&self) {
        std::fs::write(k2_client::paths::k2_home_in(&self.home).join("heartbeat.token"), format!("{OWNER}\n")).unwrap();
    }

    fn port(&self) -> String {
        self.stub.port.to_string()
    }

    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(self.dir.join("bin").join("k5"));
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", &self.home).current_dir(&self.proj).envs(extra.iter().copied()).args(args.iter().map(OsString::from));
        let out = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
        Out {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A port nothing listens on.
fn dead_port() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port().to_string()
}

fn headed(status: u16, version: Option<&str>, body: &str) -> Reply {
    let extra = version.map(|v| vec![("X-K5-Server-Version".to_string(), v.to_string())]).unwrap_or_default();
    Reply::Headed(status, extra, body.to_string())
}

/// The request line and the header lines of a logged request.
fn head(r: &Logged) -> Vec<String> {
    let text = String::from_utf8(r.bytes.clone()).expect("a UTF-8 request");
    let (head, _) = text.split_once("\r\n\r\n").expect("a request head");
    head.split("\r\n").map(str::to_string).collect()
}

// ── --version --check ──────────────────────────────────────────────────

#[test]
fn version_check_same_version() {
    let rig = Rig::new("chk-same");
    rig.stub.reset(&[headed(200, None, &format!(r#"{{"phase":"ready","version":"{}"}}"#, ours()))]);
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &rig.port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("k2 {v}\nserver {v}: same version\n", v = ours()));
    assert_eq!(out.stderr, "");
    // One request: a bare GET, no credential, no query.
    let log = rig.stub.log();
    assert_eq!(log.len(), 1, "{log:?}");
    let h = head(&log[0]);
    assert_eq!(h[0], "GET /boot-status HTTP/1.1");
    assert_eq!(log[0].channel, "tcp");
    let names: Vec<&str> = h[1..].iter().map(|l| l.split_once(':').expect("a header").0).collect();
    assert_eq!(names, ["Host", "User-Agent", "Accept", "X-K5-Cli-Version", "X-K5-Cli-Via"]);
}

#[test]
fn version_check_skewed_is_one_stdout_line_and_exit_0() {
    let rig = Rig::new("chk-skew");
    rig.owner_on_disk();
    // The header names the other version too: still one line.
    rig.stub.reset(&[headed(200, Some(OTHER), &format!(r#"{{"version":"{OTHER}"}}"#))]);
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &rig.port())]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("k2 {}\n{}\n", ours(), skew::line(ours(), OTHER)));
    assert_eq!(out.stderr, "", "the transport's own line is not printed as well");
    // The owner token on disk is not sent to a public read.
    let h = head(&rig.stub.log()[0]);
    assert_eq!(h[0], "GET /boot-status HTTP/1.1");
    assert!(h.iter().all(|l| !l.contains(OWNER) && !l.starts_with("X-K5-Owner-Source")), "{h:?}");
}

#[test]
fn version_check_with_the_daemon_down_is_one_line_and_exit_7() {
    let rig = Rig::new("chk-down");
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &dead_port())]);
    assert_eq!(out.code, Some(7), "{out:?}");
    assert_eq!(out.stdout, format!("k2 {}\n", ours()));
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
    assert!(out.stderr.starts_with("can't reach K2's server for /boot-status: "), "{out:?}");
    // No port anywhere: the same exit, a line that says where it looked.
    let out = rig.run(&["--version", "--check"], &[]);
    assert_eq!(out.code, Some(7), "{out:?}");
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
    assert!(out.stderr.starts_with("can't reach K2's server: no port in K5_PORT or "), "{out:?}");
}

#[test]
fn only_the_exact_pair_checks() {
    let rig = Rig::new("chk-pair");
    // The shell's connection gate wants a credential before its router.
    for args in [&["-v", "--check"][..], &["version", "--check"], &["--version", "--check", "x"]] {
        let out = rig.run(args, &[("K2_PORT", &rig.port()), ("K2_HOOK_TOKEN", "test-token")]);
        assert_eq!((out.code, out.stdout.as_str()), (Some(0), format!("k2 {}\n", ours()).as_str()), "{args:?}: {out:?}");
    }
    assert!(rig.stub.log().iter().all(|r| !head(r)[0].contains("/boot-status")), "the shell's version arm never asks");
}
