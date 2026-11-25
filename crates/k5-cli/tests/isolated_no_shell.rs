//! An isolated agent session's passport call, end to end, in this build.
//! The session is marked by `AGENT_CGROUP` (here under its
//! `K2_` name) and carries a session passport, so the front hands the call
//! to the shell, which sends it over the session socket.
//!
//! With no shell CLI (the brand file's `cli.no_bash`,
//! `K2_BRAND_DIR=brand/xy`) that call is refused: one line on stderr that
//! starts with the brand's command name, exit 70, and the stub daemon
//! records no request. With the shell CLI it goes to the hand-off as
//! before: with no `python3` on `PATH` that is the hand-off's first check,
//! exit 69 and its line, and no request either (the native client never
//! ran).
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

#[test]
fn an_isolated_session_s_passport_call_is_refused_only_with_no_shell_cli() {
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
    let args: Vec<OsString> = ["agent", "context", "list"]
        .iter()
        .map(OsString::from)
        .collect();
    let out = Command::new(BIN)
        .env_clear()
        .env("PATH", &path)
        .env("HOME", &home)
        .env("K2_PORT", stub.port.to_string())
        .env("K2_HOOK_SOCK", &stub.sock)
        .env("K2_HOOK_TOKEN", "sessA.secret7272")
        .env("K2_AGENT_CGROUP", "/k2agents.slice/a.scope")
        .current_dir(&proj)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("run {BIN}: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let noun = noun(&args);
    let (line, code) = if k2_client::brand::CLI_NO_BASH {
        (FrontError::IsolatedNoShell { noun }.line(), 70)
    } else {
        (FrontError::NoPython { noun }.line(), 69)
    };
    assert_eq!(
        (out.status.code(), stderr.as_str(), out.stdout.as_slice()),
        (Some(code), format!("{line}\n").as_str(), &b""[..]),
        "no_bash = {}",
        k2_client::brand::CLI_NO_BASH
    );
    if k2_client::brand::CLI_NO_BASH {
        assert_eq!(
            line,
            format!(
                "{} agent: this command isn't available inside an isolated agent session yet",
                k2_client::brand::CLI_NAME
            )
        );
    }
    assert_eq!(stub.log(), Vec::new(), "the call reached the daemon");
    assert_eq!(
        tree(&home),
        Vec::<PathBuf>::new(),
        "the call wrote under the home"
    );
    assert_eq!(
        tree(&proj),
        Vec::<PathBuf>::new(),
        "the call wrote in the folder"
    );
    drop(stub);
    std::fs::remove_dir_all(&dir).unwrap();
}
