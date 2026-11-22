//! The passport rule against the shell CLI: the one offline list is the
//! shell's (the virtual-desktop guard's `_k2_offline_verb`), the prelude
//! reads `K5_CLI_WOULD_REFUSE` as the front sets it and takes every
//! hand-over variable out of the environment, the markers are the ones the
//! shell names, and a session process found by the live walk is named by
//! the kernel (a copy of this test binary named `k2-ptyd` as the ancestor).

mod common;

use k5_cli::called_as::{fallthrough_env, CALLED_AS_VAR, WOULD_REFUSE_VAR};
use k5_cli::passport::{self, Verdict, OFFLINE_ANYWHERE, OFFLINE_FIRST};
#[cfg(unix)]
use std::ffi::OsString;

fn policy() -> String {
    let path = common::cli_src().join("cli-tools-policy.sh");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The guard's lines that set `_k2_offline_verb`.
fn offline_block() -> String {
    common::block(&policy(), r#"_k2_offline_verb="""#, "esac")
}

/// The guard's block with the function it calls for `app exec` and `app
/// shell`, ready for bash.
fn offline_script() -> String {
    let says_help = common::block(&policy(), "_app_cmd_says_help() {", "}");
    format!("{says_help}\n{}", offline_block())
}

#[test]
fn the_offline_list_is_the_shell_s_word_for_word() {
    let block = offline_block();
    let line = block.lines().nth(1).expect("the case line");
    let list = line
        .strip_prefix(r#"case "${1:-}" in "#)
        .and_then(|l| l.strip_suffix(") _k2_offline_verb=1 ;; esac"))
        .unwrap_or_else(|| panic!("the guard's case line changed shape: {line}"));
    let shell: Vec<&str> = list
        .split('|')
        .map(|v| if v == r#""""# { "" } else { v })
        .collect();
    assert_eq!(shell, OFFLINE_FIRST);
    assert!(
        block.contains("\n    \"app exec\"|\"app shell\") if _app_cmd_says_help \"${@:3}\"; then _k2_offline_verb=1; fi ;;\n"),
        "{block}"
    );
    let anywhere = block
        .lines()
        .find(|l| l.starts_with("    *) for _a in "))
        .expect("the loop line, for every other command");
    assert!(
        anywhere.contains(&format!("case \"$_a\" in {})", OFFLINE_ANYWHERE.join("|"))),
        "{anywhere}"
    );
}

#[cfg(unix)]
#[test]
fn every_command_line_is_offline_as_the_shell_says() {
    let script = format!("{}\nprintf '%s' \"$_k2_offline_verb\"", offline_script());
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
        &["msg", "-h", "--", "x"],
        &["msg", "-help"],
        &["msg", "help"],
        &["Help"],
        &[" help"],
        &["help", "msg"],
        &["heartbeat", "list", "--json"],
        // `app exec` and `app shell`: everything after the App is the
        // command's own.
        &["app", "exec", "-h"],
        &["app", "exec", "help"],
        &["app", "exec", "web", "--help"],
        &["app", "exec", "web", "help"],
        &["app", "exec", "web", "ls", "-h"],
        &["app", "exec", "web", "ls", "help"],
        &["app", "shell", "web", "-h"],
        &["app", "shell", "web", "bash", "--help"],
        &["app", "exec", "--timeout", "5", "web", "-h"],
        &["app", "exec", "--timeout", "-h"],
        &["app", "exec", "--timeout"],
        &["app", "exec", "-x", "web", "-h"],
        &["app", "exec", "--", "-h"],
        &["app", "exec", "web", "--", "-h"],
        &["app", "exec", "", "-h"],
        &["app", "exec", "", "web", "ls", "-h"],
        &["app", "exec", "-", "web", "-h"],
        &["app", "exec"],
        &["app", "exec", "web"],
        &["app", "list", "-h"],
        &["app", "list", "--", "-h"],
        &["app exec", "web", "ls", "-h"],
        &["app", "exec x", "-h"],
        &["app", "Exec", "web", "ls", "-h"],
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
    let script = format!(
        "{}\nprintf '%s' \"$_K5_WOULD_REFUSE\"",
        would_refuse_block()
    );
    let (name, value) = passport::hand_off_env(Verdict::WouldRefuse);
    assert_eq!(name, WOULD_REFUSE_VAR);
    assert_eq!(
        common::bash(&script, "k2", &[(name, value.expect("a value"))]),
        "1"
    );
    assert_eq!(common::bash(&script, "k2", &[]), "");
    for other in ["", "0", "yes", " 1", "11", "true"] {
        assert_eq!(
            common::bash(&script, "k2", &[(name, other)]),
            "",
            "{other:?}"
        );
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
    let mut names: Vec<&str> = fallthrough_env().iter().map(|(n, _)| *n).collect();
    names.push(CALLED_AS_VAR);
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
    let text = |name: &str| {
        sources
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .expect(name)
    };
    assert!(text("remind.sh").contains("_envn_into pane PANE_ID"));
    assert!(policy().contains("Codex sets CODEX_THREAD_ID on tool runs"));
    let all: Vec<&str> = passport::SESSION_MARKERS
        .iter()
        .flat_map(|n| n.iter().copied())
        .collect();
    for want in [
        "K5_PANE_ID",
        "K2_PANE_ID",
        "K5_CELL",
        "K2_CELL",
        "CLAUDECODE",
        "CODEX_THREAD_ID",
    ] {
        assert!(all.contains(&want), "{want} is not a marker");
    }
}

/// Set on a copy of this test binary run as the stand-in parent; any
/// other run of [`stand_in_parent`] does nothing.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const STAND_IN_VAR: &str = "PASSPORT_PARITY_STAND_IN";

/// What the stand-in parent prints before the pid of the sleep it started,
/// so the pid stands apart from the test harness's own output.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const SLEEP_PID: &str = "stand-in sleep pid ";

/// Not a check: the body of the stand-in parent for the live walk. A copy
/// of this test binary, run as this one test with [`STAND_IN_VAR`] set,
/// starts `sleep`, prints its pid and waits on it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn stand_in_parent() {
    use std::io::Write;
    if std::env::var_os(STAND_IN_VAR).is_none() {
        return;
    }
    let mut sleep = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("run sleep");
    let mut out = std::io::stdout().lock();
    writeln!(out, "{SLEEP_PID}{}", sleep.id()).expect("print the pid");
    out.flush().expect("flush the pid");
    drop(out);
    let _ = sleep.wait();
}

/// A real walk: a copy of this test binary named `k2-ptyd` starts `sleep`,
/// and the walk up from the sleep finds it. A copy under another name is
/// not found as the pty daemon. The copy is our own build (signed ad hoc
/// by the linker on macOS), never a system binary: macOS kills a copied
/// platform binary at launch.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_live_walk_finds_a_process_named_like_the_pty_daemon() {
    use k2_client::proc_ancestry::LiveProcessTable;
    use passport::{Ancestry, SessionProcess};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let dir = std::env::temp_dir().join(format!("k5-passport-walk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = std::env::current_exe().expect("this test binary");
    let table = LiveProcessTable;
    let mut seen = Vec::new();
    for (name, is_ptyd) in [("k2-ptyd", true), ("k2-ptyx", false)] {
        let fake = dir.join(name);
        std::fs::copy(&exe, &fake)
            .unwrap_or_else(|e| panic!("copy {} to {}: {e}", exe.display(), fake.display()));
        let mut child = Command::new(&fake)
            .args([
                "stand_in_parent",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(STAND_IN_VAR, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("run {}: {e}", fake.display()));
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut printed = Vec::new();
        let sleep: i32 = loop {
            match lines.next() {
                // The harness prints `test stand_in_parent ... ` with no
                // line end before the test runs, so the mark is mid-line.
                Some(Ok(l)) => match l.split_once(SLEEP_PID) {
                    Some((_, pid)) => {
                        break pid
                            .trim()
                            .parse()
                            .unwrap_or_else(|e| panic!("{name} printed {l:?}: {e}"))
                    }
                    None => printed.push(l),
                },
                end => {
                    let out = child.wait_with_output().expect("wait");
                    panic!(
                        "{name} printed no pid ({end:?} after {printed:?}); {}: {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
            }
        };
        // Wait until the kernel names the new process sleep.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while table.command_name(sleep).as_deref() != Some("sleep")
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let sleep_name = table.command_name(sleep);
        let got = passport::session_ancestor(&table, |p| table.command_name(p), sleep);
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
    let me = table
        .command_name(std::process::id() as i32)
        .expect("my own name");
    let file = exe.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        !me.is_empty() && file.starts_with(&me),
        "{me:?} is not the start of {file:?}"
    );
    assert_eq!(seen.len(), 2);
}
