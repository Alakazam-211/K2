//! A command line the front hands to the shell CLI, in this build. With
//! no shell CLI (the brand file's `cli.no_bash`, `K2_BRAND_DIR=brand/xy`)
//! it is refused: one line on stderr that starts with the brand's command
//! name, exit 70, and nothing written under the home. With the shell CLI
//! the hand-off goes on to its first check, Python 3, as before: with no
//! `python3` on `PATH` that is exit 69 and its line, and nothing written
//! either.
//!
//! The native binary hands nothing to the shell (the native bar's
//! ceiling), so the hand-off runs through `k5-spec-fixture`: a command
//! line no row of its table starts goes to the front's hand-off
//! ([`k5_cli::fallthrough::hand_off`]).
//!
//! Every run gets an empty environment but `PATH` (with no `python3` on
//! it), a temporary `HOME` and a dead port with a dummy token, so nothing
//! reads or writes a real K2 home or reaches a daemon.

#![cfg(unix)]

mod common;

use k5_cli::fallthrough::{noun, FrontError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FIXTURE: &str = env!("CARGO_BIN_EXE_k5-spec-fixture");

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
            found.push(path.strip_prefix(dir).expect("under the folder").to_path_buf());
            if path.is_dir() {
                todo.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_hand_off_is_refused_only_in_a_build_with_no_shell_cli() {
    let dir = std::env::temp_dir().join(format!("k5-no-shell-cli-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let home = dir.join("home");
    let proj = dir.join("proj");
    for d in [&home, &proj] {
        std::fs::create_dir_all(d).unwrap();
    }
    let path = common::path_without_python3(&dir.join("path"));
    let args: Vec<OsString> = ["zz-no-such-verb", "x"].iter().map(OsString::from).collect();
    let out = Command::new(FIXTURE)
        .env_clear()
        .env("PATH", &path)
        .env("HOME", &home)
        .env("K2_PORT", "1")
        .env("K2_HOOK_TOKEN", "test-token")
        .current_dir(&proj)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("run {FIXTURE}: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let noun = noun(&args);
    let (line, code) = if k2_client::brand::CLI_NO_BASH {
        (FrontError::NoShellCli { noun }.line(), 70)
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
        assert!(
            line.starts_with(&format!("{} zz-no-such-verb: ", k2_client::brand::CLI_NAME)),
            "{line}"
        );
    }
    assert_eq!(
        tree(&home),
        Vec::<PathBuf>::new(),
        "the hand-off wrote under the home"
    );
    assert_eq!(tree(&proj), Vec::<PathBuf>::new(), "the hand-off wrote in the folder");
    std::fs::remove_dir_all(&dir).unwrap();
}
