//! The front's Codex tab gate ([`k5_cli::codex_gate::decide`]) decides as
//! the shell's own `_cli_codex_tab_gate` does: bash runs the gate itself,
//! taken from `cli/k2` with the helpers it calls (`_envn_into`,
//! `_cli_codex_bind_on`), over the same table, with the process walk
//! (`_cli_codex_ancestry`) standing in for each state, and `cmd_help`
//! standing in for the help page. Every family's first word in the route
//! table runs, with and without the socket, with no marker, another
//! tab's marker and the matching one.
//!
//! Also here: the binary's copy of the walk's Python is the shell's, and
//! its readers of `ps` rows ([`k5_cli::codex_gate::ps_chain`],
//! [`k5_cli::codex_gate::has_shared_server`]) agree with the shell's awk
//! on the same rows.

#![cfg(unix)]

mod common;

use k5_cli::codex_gate::{self, Anc, Decision};
use std::cell::Cell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

/// The definition of the shell function `name` in `text`: from its
/// `name() {` line to the first `}` alone on a line.
fn function(text: &str, name: &str) -> String {
    let head = format!("\n{name}() {{\n");
    let at = text
        .find(&head)
        .unwrap_or_else(|| panic!("no {name} in cli/k2"));
    let body = &text[at + 1..];
    let end = body
        .find("\n}\n")
        .unwrap_or_else(|| panic!("{name} has no end"));
    body[..end + 3].to_string()
}

/// Lines `from` to `to` of `text`, both included.
fn block(text: &str, from: &str, to: &str) -> String {
    let a = text
        .find(from)
        .unwrap_or_else(|| panic!("no {from} in cli/k2"));
    let b = text[a..]
        .find(to)
        .unwrap_or_else(|| panic!("no {to} in cli/k2"));
    text[a..a + b + to.len()].to_string()
}

fn cli_text() -> String {
    let p = common::parity::cli_k2();
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// The harness: the shell's gate, then one line naming what it left.
fn harness(dir: &std::path::Path) -> PathBuf {
    let text = cli_text();
    let script = format!(
        "set -euo pipefail\n{}\n{}\n{}\n\
         _cli_codex_ancestry() {{ _K2_ANC_STATE=\"$GATE_TEST_ANC\"; }}\n\
         cmd_help() {{ echo \"HELP $1\"; }}\n\
         SCOPED_TOKEN=\"\"\n_envn_into SCOPED_TOKEN HOOK_TOKEN\n\
         _K2_ANC_STATE=\"\"; _K2_PERMISSION_ON=\"\"; _K2_SF_AGENTS_OFF=\"\"\n\
         DISK_OWNER_TOKEN=owner; TOKEN=\"${{SCOPED_TOKEN:-owner}}\"\n\
         _cli_codex_tab_gate \"${{1:-help}}\" \"${{@:2}}\"\n\
         echo \"PASS anc=$_K2_ANC_STATE held=$_K2_PERMISSION_ON/$_K2_SF_AGENTS_OFF\"\n",
        function(&text, "_envn_into"),
        function(&text, "_cli_codex_bind_on"),
        block(
            &text,
            "# BEGIN_CLI_CODEX_TAB_BIND",
            "# END_CLI_CODEX_TAB_BIND"
        ),
    );
    let path = dir.join("gate.sh");
    std::fs::write(&path, script).unwrap();
    path
}

fn anc_name(a: Anc) -> &'static str {
    match a {
        Anc::Clear => "clear",
        Anc::Blind => "blind",
        Anc::Shared => "shared",
        Anc::Broken => "broken",
    }
}

/// What bash must print and exit for the binary's decision.
fn expected(d: &Decision, walked: Option<Anc>) -> (i32, String, Vec<u8>) {
    let anc = walked.map_or("", anc_name);
    match d {
        Decision::Off | Decision::Confirmed => (0, format!("PASS anc={anc} held=/\n"), vec![]),
        Decision::Blind => (
            0,
            format!("PASS anc={anc} held=/\n"),
            format!("{}\n", codex_gate::BLIND_NOTE).into_bytes(),
        ),
        Decision::Allowed => (0, format!("PASS anc={anc} held=0/0\n"), vec![]),
        Decision::HelpOf(v) => (0, format!("HELP {}\n", v.to_string_lossy()), vec![]),
        Decision::Refuse { verb, anc } => (3, String::new(), codex_gate::refusal(verb, *anc)),
        Decision::Unknown => panic!("the stand-in walk always answers"),
    }
}

struct Row {
    env: Vec<(&'static str, String)>,
    anc: Anc,
    args: Vec<String>,
}

fn run_row(gate: &std::path::Path, row: &Row) {
    let walked = Cell::new(None);
    let lookup = |n: &str| {
        row.env
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| OsString::from(v))
    };
    let args: Vec<OsString> = row.args.iter().map(OsString::from).collect();
    let d = codex_gate::decide(&args, lookup, || {
        walked.set(Some(row.anc));
        Some(row.anc)
    });
    let (code, stdout, stderr) = expected(&d, walked.get());
    let mut cmd = Command::new("bash");
    cmd.arg(gate).args(&row.args).env_clear();
    if let Some(p) = std::env::var_os("PATH") {
        cmd.env("PATH", p);
    }
    cmd.env("GATE_TEST_ANC", anc_name(row.anc));
    for (k, v) in &row.env {
        cmd.env(k, v);
    }
    let o = cmd.output().expect("run bash");
    let got = (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    );
    let want = (
        Some(code),
        stdout,
        String::from_utf8_lossy(&stderr).into_owned(),
    );
    assert_eq!(
        got, want,
        "{:?} {:?} (walk {:?}): the binary decided {d:?}",
        row.env, row.args, row.anc
    );
}

/// The settings each row's environment is built from.
fn envs() -> Vec<Vec<(&'static str, String)>> {
    let s = |v: &str| v.to_string();
    let thread = [None, Some(""), Some("t1")];
    let bind = [None, Some("0"), Some("OFF")];
    let session: [&[(&'static str, &str)]; 6] = [
        &[],
        &[("K2_HOOK_SOCK", "/s")],
        &[("K5_HOOK_SOCK", ""), ("K2SO_HOOK_SOCK", "/s")],
        &[("K2_PANE_ID", "p1"), ("K2_HOOK_TOKEN", "sessA.x")],
        &[("K2_PANE_ID", "p1")],
        &[("K2_HOOK_TOKEN", "sessA.x")],
    ];
    let marker = [None, Some(""), Some("sessA"), Some("sessB")];
    let token = [None, Some("sessA.x"), Some("sessA")];
    let mut out = Vec::new();
    for t in thread {
        for b in bind {
            for sess in session {
                for m in marker {
                    for tok in token {
                        let mut env: Vec<(&'static str, String)> =
                            sess.iter().map(|(k, v)| (*k, s(v))).collect();
                        if let Some(t) = t {
                            env.push(("CODEX_THREAD_ID", s(t)));
                        }
                        if let Some(b) = b {
                            env.push(("K2_CODEX_PASSPORT_BIND", s(b)));
                        }
                        if let Some(m) = m {
                            env.push(("K2_CODEX_TAB_ISOLATED", s(m)));
                        }
                        if let Some(tok) = tok {
                            if !env.iter().any(|(k, _)| *k == "K2_HOOK_TOKEN") {
                                env.push(("K5_HOOK_TOKEN", s(tok)));
                            }
                        }
                        out.push(env);
                    }
                }
            }
        }
    }
    out
}

const ANCS: [Anc; 4] = [Anc::Clear, Anc::Blind, Anc::Shared, Anc::Broken];

/// The settings grid, every walk state, a few command lines.
#[test]
fn the_settings_and_the_walk_decide_as_the_shell() {
    let dir = tempdir("grid");
    let gate = harness(&dir);
    let lines: &[&[&str]] = &[
        &["whoami"],
        &[],
        &[""],
        &["msg", "--", "--help"],
        &["explain", "MPC-1"],
    ];
    let mut n = 0;
    for env in envs() {
        // The walk runs only for the matching marker: every state there.
        let matching = env
            .iter()
            .any(|(k, v)| *k == "K2_CODEX_TAB_ISOLATED" && v == "sessA");
        let ancs: &[Anc] = if matching { &ANCS } else { &ANCS[..1] };
        for &anc in ancs {
            for line in lines {
                run_row(
                    &gate,
                    &Row {
                        env: env.clone(),
                        anc,
                        args: line.iter().map(|w| w.to_string()).collect(),
                    },
                );
                n += 1;
            }
        }
    }
    println!("{n} rows");
}

/// Every family's words, and the words the gate reads, under a Codex call
/// with no marker, another tab's and the matching one, with the socket
/// and with a pane id alone.
#[test]
fn every_family_decides_as_the_shell() {
    let dir = tempdir("families");
    let gate = harness(&dir);
    let mut words: Vec<&str> = k5_cli::route::TABLE
        .iter()
        .flat_map(|n| n.words.iter().copied())
        .collect();
    words.extend([
        "help",
        "--help",
        "-h",
        "schema",
        "--schema",
        "version",
        "--version",
        "-v",
        "explain",
        "help-advanced",
        "help-internal",
        "help-deprecated",
        "cli",
        "status",
        "study",
        "no-such-verb",
    ]);
    let sessions: [&[(&'static str, &str)]; 2] =
        [&[("K2_HOOK_SOCK", "/s")], &[("K2_PANE_ID", "p1")]];
    let mut n = 0;
    for w in &words {
        for sess in sessions {
            for marker in [None, Some("sessB"), Some("sessA")] {
                let ancs: &[Anc] = if marker == Some("sessA") {
                    &[Anc::Clear, Anc::Shared]
                } else {
                    &[Anc::Clear]
                };
                for &anc in ancs {
                    let mut env: Vec<(&'static str, String)> =
                        sess.iter().map(|(k, v)| (*k, v.to_string())).collect();
                    env.push(("CODEX_THREAD_ID", "t1".into()));
                    env.push(("K2_HOOK_TOKEN", "sessA.x".into()));
                    if let Some(m) = marker {
                        env.push(("K2_CODEX_TAB_ISOLATED", m.into()));
                    }
                    for line in [vec![*w], vec![*w, "x", "--help"]] {
                        run_row(
                            &gate,
                            &Row {
                                env: env.clone(),
                                anc,
                                args: line.iter().map(|s| s.to_string()).collect(),
                            },
                        );
                        n += 1;
                    }
                }
            }
        }
    }
    println!("{n} rows over {} words", words.len());
}

fn tempdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k5-codex-gate-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The binary's copy of the walk's Python is the shell's `_K2_ANCESTRY_PY`.
#[test]
fn the_walk_s_python_is_the_shell_s() {
    let src = common::cli_src().join("cli-tools-policy.sh");
    let text = std::fs::read_to_string(&src).unwrap();
    let head = "\n_K2_ANCESTRY_PY='";
    let at = text.find(head).expect("_K2_ANCESTRY_PY in the policy") + head.len();
    let end = text[at..].find("\n'\n").expect("its closing quote") + 1;
    assert_eq!(codex_gate::ANCESTRY_PY, &text[at..at + end]);
}

/// `ps` rows read by the shell's awk (`_cli_ps_chain`, then
/// `_cli_ancestry_has_shared_codex_server` on the chain) and by the
/// binary: the same chain, the same verdict.
#[test]
fn the_ps_readers_agree_with_the_shell_s_awk() {
    let text = cli_text();
    let dir = tempdir("awk");
    let script = dir.join("awk.sh");
    std::fs::write(
        &script,
        format!(
            "{}\n{}\nset +e\nrows=\"$(printf '%s\\n' \"$ROWS\" | _cli_ps_chain \"$START\")\"; rc=$?\n\
             if [ $rc -ne 0 ]; then echo \"bad $rc\"; exit 0; fi\n\
             printf '%s\\n' \"$rows\"\n\
             if printf '%s\\n' \"$rows\" | _cli_ancestry_has_shared_codex_server \"$START\"; then echo shared; else echo clear; fi\n",
            function(&text, "_cli_ps_chain"),
            function(&text, "_cli_ancestry_has_shared_codex_server"),
        ),
    )
    .unwrap();
    let tables: &[(&str, &str)] = &[
        ("500", "    1     0 /sbin/launchd\n  200     1 /x/bin/codex app-server daemon pid-update-loop\n  300   200 /x/bin/codex app-server --listen unix://\n  400   300 /bin/zsh -lc k2 thread k2/c hi\n  500   400 /bin/bash /usr/local/bin/k2 thread k2/c hi"),
        ("500", "    1     0 init\n  300     1 node /x/codex app-server --listen=unix:///tmp/s\n  500   300 bash"),
        ("500", "    1     0 init\n  300     1 codex app-server --managed-daemon\n  500   300 bash"),
        ("500", "    1     0 init\n  300     1 codex   app-server\n  500   300 bash"),
        ("500", "    1     0 init\n  290     1 codex\n  300   290 codex app-server\n  500   300 bash"),
        ("500", "    1     0 init\n  290     1 codex --no-daemon\n  500   290 bash k2"),
        ("500", "    1     0 init\n\t400\t1\t/bin/zsh\n  500   400 bash"),
        ("500", "  500   400 bash"),
        ("500", "garbage\n  500   1 bash"),
        ("500", ""),
        ("500", "    1     0 init\n  500   500 bash"),
        ("0500", "    1     0 init\n  500     1 bash"),
        ("500", "  1 0 init\n  500 1 bash x"),
        ("500", "    1     0 init\n  500     1"),
    ];
    for (start, rows) in tables {
        let o = Command::new("bash")
            .arg(&script)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("ROWS", rows)
            .env("START", start)
            .output()
            .unwrap();
        let bash = String::from_utf8_lossy(&o.stdout).into_owned();
        let ours = match codex_gate::ps_chain(rows, start) {
            None => "bad 3\n".to_string(),
            Some(chain) => {
                let chain = chain.trim_end_matches('\n');
                let shared = codex_gate::has_shared_server(chain, start);
                format!("{chain}\n{}\n", if shared { "shared" } else { "clear" })
            }
        };
        assert_eq!(ours, bash, "start {start}, rows:\n{rows}");
    }
}
