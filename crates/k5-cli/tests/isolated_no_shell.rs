//! An isolated agent session's passport call, end to end, in this build.
//! The session is marked by `AGENT_CGROUP` (here under its `K2_` name)
//! and carries a session passport and a live session socket (the stub's).
//!
//! Off Windows a verb the binary answers runs natively in any build: its
//! passport goes over the session socket, marked `X-K2-Via-Tcp: 1`, and
//! nothing reaches TCP (`agent context list`). A verb that still goes to
//! the shell (`agent whois`, on the native port allowance) is refused in a
//! build with no shell CLI (the brand file's `cli.no_bash`,
//! `K2_BRAND_DIR=brand/xy`): one line on stderr that starts with the
//! brand's command name, exit 70, and the stub records no request. With
//! the shell CLI it goes to the hand-off as before: with no `python3` on
//! `PATH` that is the hand-off's first check, exit 69 and its line, and no
//! request either. On Windows every such passport call still goes to the
//! shell (`passport::identity_hand_off`, its unit test).
//!
//! Every run gets an empty environment but `PATH` (with no `python3` on
//! it), a temporary `HOME`, and the stub's port and socket with a session
//! passport, so nothing reads or writes a real K2 home or reaches a real
//! daemon.

#![cfg(unix)]

mod common;

use common::stub::Stub;
use k5_cli::fallthrough::{noun, FrontError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_k5-cli");

/// Every file and folder under `dir`, as paths below it, sorted.
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let entries = std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|e| panic!("read {}: {e}", d.display()))
                .path();
            found.push(
                path.strip_prefix(dir)
                    .expect("under the folder")
                    .to_path_buf(),
            );
            if path.is_dir() {
                todo.push(path);
            }
        }
    }
    found.sort();
    found
}

const PASSPORT: &str = "sessA.secret7272";

struct Run {
    code: Option<i32>,
    stderr: String,
    stdout: Vec<u8>,
}

fn run(stub: &Stub, home: &Path, proj: &Path, path: &std::ffi::OsStr, args: &[OsString]) -> Run {
    let out = Command::new(BIN)
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("K2_PORT", stub.port.to_string())
        .env("K2_HOOK_SOCK", &stub.sock)
        .env("K2_HOOK_TOKEN", PASSPORT)
        .env("K2_AGENT_CGROUP", "/k2agents.slice/a.scope")
        .current_dir(proj)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("run {BIN}: {e}"));
    Run {
        code: out.status.code(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        stdout: out.stdout,
    }
}

fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// A native verb runs over the session socket in any build; a verb that
/// still goes to the shell is refused with no shell CLI (exit 70) and
/// handed off with one. No request ever reaches TCP.
#[test]
fn an_isolated_session_s_passport_call_runs_natively_or_is_refused_with_no_shell_cli() {
    let dir = std::env::temp_dir().join(format!("k5-isolated-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let home = dir.join("home");
    let proj = dir.join("proj");
    for d in [&home, &proj] {
        std::fs::create_dir_all(d).unwrap();
    }
    let stub = Stub::start(&PathBuf::from(format!(
        "/tmp/k5iso-{}.sock",
        std::process::id()
    )));
    let path = common::path_without_python3(&dir.join("path"));
    let isolated_line = |noun: &str| {
        format!(
            "{} {noun}: this command isn't available inside an isolated agent session yet",
            k2_client::brand::CLI_NAME
        )
    };

    // A native verb: over the session socket, marked, in any build.
    let native = words(&["agent", "context", "list"]);
    stub.reset(&[]);
    let r = run(&stub, &home, &proj, path.as_os_str(), &native);
    let log = stub.take_log();
    let over_tcp: Vec<String> = log
        .iter()
        .filter(|l| l.channel == "tcp")
        .map(|l| String::from_utf8_lossy(&l.bytes).into_owned())
        .collect();
    assert_eq!(over_tcp, Vec::<String>::new(), "a request reached TCP");
    let marked = log.iter().any(|l| {
        let head = String::from_utf8_lossy(&l.bytes);
        l.channel == "uds"
            && head.starts_with("GET /cli/context/layers")
            && head.contains("\r\nX-K2-Via-Tcp: 1\r\n")
            && head.contains(&format!("\r\nAuthorization: Bearer {PASSPORT}\r\n"))
    });
    assert!(marked, "no marked passport request on the session socket: {log:?}");
    assert_ne!(r.code, Some(70), "{}", r.stderr);
    assert_ne!(r.code, Some(69), "{}", r.stderr);
    assert!(!r.stderr.contains("isolated agent session"), "{}", r.stderr);

    // A verb that still goes to the shell.
    let shell = words(&["agent", "whois"]);
    stub.reset(&[]);
    let r = run(&stub, &home, &proj, path.as_os_str(), &shell);
    let noun = noun(&shell);
    let (line, code) = if k2_client::brand::CLI_NO_BASH {
        (FrontError::IsolatedNoShell { noun }.line(), 70)
    } else {
        (FrontError::NoPython { noun }.line(), 69)
    };
    assert_eq!(
        (r.code, r.stderr.as_str(), r.stdout.as_slice()),
        (Some(code), format!("{line}\n").as_str(), &b""[..]),
        "no_bash = {}",
        k2_client::brand::CLI_NO_BASH
    );
    if k2_client::brand::CLI_NO_BASH {
        assert_eq!(line, isolated_line("agent"));
    }
    assert_eq!(stub.take_log(), Vec::new(), "the hand-off's call reached the daemon");
    let _ = std::fs::remove_dir_all(home.join(".k2"));
    assert_eq!(
        tree(&proj),
        Vec::<PathBuf>::new(),
        "the call wrote in the folder"
    );
    drop(stub);
    std::fs::remove_dir_all(&dir).unwrap();
}
