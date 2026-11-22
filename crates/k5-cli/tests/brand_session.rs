//! Another brand's server on this machine (K2's rulings 2 and 4): the
//! binary says so on stderr, once per process, and runs the command as it
//! would anyway. Two ways to know: the session's environment (help and
//! study warn too, with no request) and the brand headers of an answer
//! from a daemon on this machine ([`common::stub`]).
//!
//! Every run gets an empty environment but `PATH` and what the case sets,
//! a temporary `HOME` and the stub's port. No real daemon, no real K2
//! home.

#![cfg(unix)]

mod common;

use common::stub::{Reply, Stub};
use k2_client::brand::session::mismatch_line;
use k2_client::brand::{BRAND_ID, CLI_NAME};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");

static N: AtomicUsize = AtomicUsize::new(0);

/// A brand id that is not this build's.
fn other() -> &'static str {
    if BRAND_ID == "zz" {
        "zy"
    } else {
        "zz"
    }
}

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

impl Out {
    fn lines(&self, theirs: &str) -> usize {
        let want = mismatch_line(theirs);
        self.stderr.lines().filter(|l| *l == want).count()
    }
}

impl Rig {
    fn new(tag: &str) -> Rig {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("k5-brand-{tag}-{}-{n}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let home = dir.join("home");
        let proj = dir.join("proj");
        for d in [
            dir.join("bin"),
            k2_client::paths::k2_home_in(&home),
            k2_client::workspace_dir::workspace_dot_dir(&proj),
        ] {
            std::fs::create_dir_all(&d).unwrap();
        }
        std::os::unix::fs::symlink(BIN, dir.join("bin").join(CLI_NAME)).unwrap();
        let stub = Stub::start(&PathBuf::from(format!(
            "/tmp/k5b-{tag}-{}-{n}.sock",
            std::process::id()
        )));
        Rig {
            dir,
            home,
            proj,
            stub,
        }
    }

    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(self.dir.join("bin").join(CLI_NAME));
        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", &self.home)
            .current_dir(&self.proj)
            .envs(extra.iter().copied())
            .args(args.iter().map(OsString::from))
            .stdin(std::process::Stdio::null());
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

fn ours() -> &'static str {
    k5_cli::embedded::version()
}

/// `/boot-status` as a daemon of brand `brand` with command `cli` answers.
fn boot(brand: &str, cli: &str) -> Reply {
    Reply::Headed(
        200,
        vec![
            ("X-K5-Server-Brand".to_string(), brand.to_string()),
            ("X-K5-Server-Cli".to_string(), cli.to_string()),
        ],
        format!(r#"{{"phase":"ready","version":"{}"}}"#, ours()),
    )
}

/// The line, then the version, as ever: in another brand's session the
/// call that never asks the daemon warns too.
#[test]
fn version_in_another_brand_s_session_warns_once() {
    let rig = Rig::new("env-version");
    let out = rig.run(
        &["--version"],
        &[("K2_SESSION_BRAND", other()), ("K2_SESSION_CLI", "zq")],
    );
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("{CLI_NAME} {}\n", ours()));
    assert_eq!(out.stderr, format!("{}\n", mismatch_line("zq")));
    assert!(
        rig.stub.log().is_empty(),
        "no request: {:?}",
        rig.stub.log()
    );
}

/// Help and study in another brand's session: the line once, before
/// whatever the page prints.
#[test]
fn help_and_study_in_another_brand_s_session_warn_once() {
    let rig = Rig::new("env-help");
    for args in [&["--help"][..], &["help"], &["study"], &["msg", "--help"]] {
        let out = rig.run(
            args,
            &[("K2_SESSION_BRAND", other()), ("K2_SESSION_CLI", "zq")],
        );
        assert_eq!(out.lines("zq"), 1, "{args:?}: {out:?}");
        assert!(
            out.stderr.starts_with(&mismatch_line("zq")),
            "{args:?}: the line comes first: {out:?}"
        );
    }
}

/// This brand's session, half a session, and values that are not names
/// give no line.
#[test]
fn this_brand_or_a_bad_value_is_no_line() {
    let rig = Rig::new("env-none");
    for env in [
        &[("K2_SESSION_BRAND", BRAND_ID), ("K2_SESSION_CLI", CLI_NAME)][..],
        &[("K2_SESSION_BRAND", BRAND_ID), ("K2_SESSION_CLI", "zq")],
        &[("K2_SESSION_BRAND", other())],
        &[("K2_SESSION_CLI", "zq")],
        &[("K2_SESSION_BRAND", other()), ("K2_SESSION_CLI", "Z Q")],
        &[
            ("K2_SESSION_BRAND", "\u{1b}[31mzz"),
            ("K2_SESSION_CLI", "zq"),
        ],
        &[
            ("K2_SESSION_BRAND", other()),
            ("K2_SESSION_CLI", "abcdefghijklm"),
        ],
        &[],
    ] {
        let out = rig.run(&["--version"], env);
        assert_eq!(out.code, Some(0), "{env:?}: {out:?}");
        assert_eq!(out.stderr, "", "{env:?}");
    }
}

/// An answer from a daemon of another brand on this machine: the line
/// once, and the call goes on (the version check still answers).
#[test]
fn an_answer_from_another_brand_s_daemon_warns_once() {
    let rig = Rig::new("hdr");
    rig.stub.reset(&[boot(other(), "zq")]);
    let port = rig.stub.port.to_string();
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &port)]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(
        out.stdout,
        format!("{CLI_NAME} {v}\nserver {v}: same version\n", v = ours())
    );
    assert_eq!(out.stderr, format!("{}\n", mismatch_line("zq")));
    assert_eq!(rig.stub.log().len(), 1);
}

/// Seen both ways in one process: still one line.
#[test]
fn seen_both_ways_it_is_still_one_line() {
    let rig = Rig::new("both");
    rig.stub.reset(&[boot(other(), "zq")]);
    let port = rig.stub.port.to_string();
    let out = rig.run(
        &["--version", "--check"],
        &[
            ("K2_PORT", &port),
            ("K2_SESSION_BRAND", other()),
            ("K2_SESSION_CLI", "zq"),
        ],
    );
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.lines("zq"), 1, "{out:?}");
    assert_eq!(out.stderr.lines().count(), 1, "{out:?}");
}

/// This brand's daemon, or headers that are not names, or only one of
/// the two: no line.
#[test]
fn an_answer_from_this_brand_or_with_bad_headers_is_no_line() {
    for (brand, cli) in [
        (BRAND_ID, CLI_NAME),
        (BRAND_ID, "zq"),
        (other(), "Z Q"),
        (other(), ""),
    ] {
        let rig = Rig::new("hdr-none");
        rig.stub.reset(&[boot(brand, cli)]);
        let port = rig.stub.port.to_string();
        let out = rig.run(&["--version", "--check"], &[("K2_PORT", &port)]);
        assert_eq!(out.code, Some(0), "{brand} {cli}: {out:?}");
        assert_eq!(out.stderr, "", "{brand} {cli}");
    }
    let rig = Rig::new("hdr-half");
    rig.stub.reset(&[Reply::Headed(
        200,
        vec![("X-K5-Server-Brand".to_string(), other().to_string())],
        format!(r#"{{"version":"{}"}}"#, ours()),
    )]);
    let port = rig.stub.port.to_string();
    let out = rig.run(&["--version", "--check"], &[("K2_PORT", &port)]);
    assert_eq!(out.stderr, "", "the id alone: {out:?}");
}
