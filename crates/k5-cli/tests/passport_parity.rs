//! The passport rule against the shell CLI: the one offline list is the
//! shell's (the virtual-desktop guard's `_k2_offline_verb`), the prelude
//! reads `K5_CLI_WOULD_REFUSE` as the front sets it and takes every
//! hand-over variable out of the environment, the markers are the ones the
//! shell names, and a session process found by the live walk is named by
//! the kernel (a copy of bash named `k2-ptyd` as the ancestor).

mod common;

use k5_cli::called_as::{CalledAs, WOULD_REFUSE_VAR};
use k5_cli::passport::{self, Verdict, OFFLINE_ANYWHERE, OFFLINE_FIRST};
#[cfg(unix)]
use std::ffi::OsString;

fn policy() -> String {
    let path = common::cli_src().join("cli-tools-policy.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The guard's three lines that set `_k2_offline_verb`.
fn offline_block() -> String {
    common::block(
        &policy(),
        r#"_k2_offline_verb="""#,
        r#"for _a in "$@"; do case "$_a" in -h|--help) _k2_offline_verb=1 ;; esac; done"#,
    )
}

#[test]
fn the_offline_list_is_the_shell_s_word_for_word() {
    let block = offline_block();
    let line = block.lines().nth(1).expect("the case line");
    let list = line
        .strip_prefix(r#"case "${1:-}" in "#)
        .and_then(|l| l.strip_suffix(") _k2_offline_verb=1 ;; esac"))
        .unwrap_or_else(|| panic!("the guard's case line changed shape: {line}"));
    let shell: Vec<&str> = list.split('|').map(|v| if v == r#""""# { "" } else { v }).collect();
    assert_eq!(shell, OFFLINE_FIRST);
    let anywhere = block.lines().nth(2).expect("the loop line");
    assert!(anywhere.contains(&format!("case \"$_a\" in {})", OFFLINE_ANYWHERE.join("|"))), "{anywhere}");
}

#[cfg(unix)]
#[test]
fn every_command_line_is_offline_as_the_shell_says() {
    let script = format!("{}\nprintf '%s' \"$_k2_offline_verb\"", offline_block());
    let rows: &[&[&str]] = &[
        &[],
        &[""],
        &["help"],
        &["--help"],
        &["-h"],
        &["study"],
        &["study", "cli"],
        &["--schema"],
        &["schema"],
        &["version"],
        &["--version"],
        &["-v"],
        &["explain"],
        &["glossary"],
        &["msg"],
        &["msg", "x", "--help"],
        &["msg", "--", "-h"],
        &["msg", "-help"],
        &["msg", "help"],
        &["Help"],
        &[" help"],
        &["help", "msg"],
        &["heartbeat", "list", "--json"],
    ];
    for row in rows {
        let shell = common::bash_args(&script, row) == "1";
        let args: Vec<OsString> = row.iter().map(OsString::from).collect();
        assert_eq!(passport::is_offline(&args), shell, "{row:?}");
    }
}

/// The prelude's lines that read `K5_CLI_WOULD_REFUSE`.
fn would_refuse_block() -> String {
    common::block(
        &common::prelude(),
        r#"_K5_WOULD_REFUSE="""#,
        r#"if [ "${K5_CLI_WOULD_REFUSE:-}" = "1" ]; then _K5_WOULD_REFUSE=1; fi"#,
    )
}

#[cfg(unix)]
#[test]
fn the_shell_reads_the_hand_off_as_the_front_sets_it() {
    let script = format!("{}\nprintf '%s' \"$_K5_WOULD_REFUSE\"", would_refuse_block());
    let (name, value) = passport::hand_off_env(Verdict::WouldRefuse);
    assert_eq!(name, WOULD_REFUSE_VAR);
    assert_eq!(common::bash(&script, "k2", &[(name, value.expect("a value"))]), "1");
    assert_eq!(common::bash(&script, "k2", &[]), "");
    for other in ["", "0", "yes", " 1", "11", "true"] {
        assert_eq!(common::bash(&script, "k2", &[(name, other)]), "", "{other:?}");
    }
}

/// No hand-over variable reaches a process the shell starts: the prelude
/// unsets each one the front sets.
#[test]
fn the_prelude_unsets_every_hand_over_variable() {
    let prelude = common::prelude();
    let unset: Vec<&str> = prelude
        .lines()
        .filter_map(|l| l.strip_prefix("unset "))
        .flat_map(str::split_whitespace)
        .collect();
    let mut names: Vec<&str> = CalledAs::K2Name.fallthrough_env().iter().map(|(n, _)| *n).collect();
    names.push(WOULD_REFUSE_VAR);
    for name in names {
        assert!(unset.contains(&name), "the prelude never unsets {name}");
    }
}

/// The markers the shell names: `PANE_ID` for a session (`hooks emit`)
/// and `CODEX_THREAD_ID` for a Codex tool run.
#[test]
fn the_markers_are_the_ones_the_shell_names() {
    let sources = common::shell_sources();
    let text = |name: &str| sources.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone()).expect(name);
    assert!(text("remind.sh").contains("_envn_into pane PANE_ID"));
    assert!(policy().contains("Codex sets CODEX_THREAD_ID on tool runs"));
    let all: Vec<&str> = passport::SESSION_MARKERS.iter().flat_map(|n| n.iter().copied()).collect();
    for want in ["K5_PANE_ID", "K2_PANE_ID", "K5_CELL", "K2_CELL", "CLAUDECODE", "CODEX_THREAD_ID"] {
        assert!(all.contains(&want), "{want} is not a marker");
    }
}

/// A real walk: a copy of bash named `k2-ptyd` starts `sleep`, and the
/// walk up from the sleep finds it. A copy under another name is not found
/// as the pty daemon.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_live_walk_finds_a_process_named_like_the_pty_daemon() {
    use k2_client::proc_ancestry::LiveProcessTable;
    use passport::{Ancestry, SessionProcess};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let dir = std::env::temp_dir().join(format!("k5-passport-walk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bash = ["/bin/bash", "/usr/bin/bash"].into_iter().find(|p| std::path::Path::new(p).is_file()).expect("a bash");
    let table = LiveProcessTable;
    let mut seen = Vec::new();
    for (name, is_ptyd) in [("k2-ptyd", true), ("k2-ptyx", false)] {
        let fake = dir.join(name);
        std::fs::copy(bash, &fake).unwrap();
        let mut child = Command::new(&fake)
            .arg("-c")
            .arg("sleep 30 & echo $!; wait")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("run {}: {e}", fake.display()));
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let sleep: i32 = line.trim().parse().unwrap_or_else(|e| panic!("sleep pid {line:?}: {e}"));
        let got = passport::session_ancestor(&table, |p| table.command_name(p), sleep);
        let sleep_name = table.command_name(sleep);
        let fake_name = table.command_name(child.id() as i32);
        let _ = Command::new("kill").arg(sleep.to_string()).status();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(sleep_name.as_deref(), Some("sleep"));
        assert_eq!(fake_name.as_deref(), Some(name));
        if is_ptyd {
            assert_eq!(got, Ancestry::Found(SessionProcess::Ptyd), "under {name}");
        } else {
            assert_ne!(got, Ancestry::Found(SessionProcess::Ptyd), "under {name}");
        }
        seen.push(got);
    }
    let _ = std::fs::remove_dir_all(&dir);
    // This test's own process is named by its file name, cut as the
    // kernel cuts it.
    let me = table.command_name(std::process::id() as i32).expect("my own name");
    let exe = std::env::current_exe().unwrap();
    let file = exe.file_name().unwrap().to_string_lossy().into_owned();
    assert!(!me.is_empty() && file.starts_with(&me), "{me:?} is not the start of {file:?}");
    assert_eq!(seen.len(), 2);
}
