//! `k5-cli msg <target> --signal <kind>` against the shell CLI's
//! `cli/k2 msg … --signal …` (`cmd_msg_signal_form`, `cmd_signal`), on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The signal's body: each kind (`msg`, `status`, `presence`, a custom
//!   one), a payload that is JSON or not, an empty one, `--from` or
//!   `*_AGENT_NAME` or `cli`, the tray flags' `inbox` delivery, and values
//!   holding quotes, a backslash and a newline, which reach the body as
//!   given (bash's fix, 1fb5f1fb6a).
//! - The workspace's id from K2's home database through the `sqlite3` on
//!   `PATH` (a stub program here that logs what it was run with), else the
//!   project's path.
//! - The answer as it came and a newline; a refusal's closing line; the
//!   cell socket in a session (its answer's trailing newlines dropped).
//! - The stops: an empty target, a value flag last (K2's D6), a payload
//!   the shell's Python raises on.
//!
//! Each body has a new UUID and the time to the microsecond, so
//! [`masked`] compares the requests with those two masked, after checking
//! their shape.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "msg";
const OK: &str = r#"{"ok":true}"#;

fn owner(c: Case) -> Case {
    Case {
        env: vec![("K2_PORT", PORT.to_string())],
        setup: owner_only,
        ..c
    }
}

fn plain(
    name: &'static str,
    args: &'static [&'static str],
    replies: Vec<Reply>,
    code: i32,
    requests: usize,
) -> Case {
    owner(case(name, args, &[], replies, nothing, code, requests))
}

fn env(mut c: Case, extra: &[(&'static str, &str)]) -> Case {
    c.env
        .extend(extra.iter().map(|(k, v)| (*k, (*v).to_string())));
    c
}

/// `s` with each signal's `"id": "<uuid>"` and `"at": "<time>"` masked,
/// after checking each is the shape Python prints.
fn mask(name: &str, s: &str) -> String {
    let mut t = s.to_string();
    for (key, shown, ok) in [
        (
            "\"id\": \"",
            "<id>",
            (|v: &str| {
                v.len() == 36
                    && v.bytes().enumerate().all(|(i, b)| {
                        if [8, 13, 18, 23].contains(&i) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
                        }
                    })
                    && v.as_bytes()[14] == b'4'
            }) as fn(&str) -> bool,
        ),
        (
            "\"at\": \"",
            "<at>",
            (|v: &str| {
                let b = v.as_bytes();
                v.len() == 32
                    && v.ends_with("+00:00")
                    && b[4] == b'-'
                    && b[10] == b'T'
                    && b[19] == b'.'
                    && v[20..26].bytes().all(|c| c.is_ascii_digit())
            }) as fn(&str) -> bool,
        ),
    ] {
        let mut from = 0;
        while let Some(at) = t[from..].find(key) {
            let start = from + at + key.len();
            let end = start + t[start..].find('"').expect("a closing quote");
            let v = t[start..end].to_string();
            assert!(ok(&v), "{name}: {shown} is {v:?}");
            t.replace_range(start..end, shown);
            from = start + shown.len();
        }
    }
    t
}

/// The `Host` line's port written `{port}`, as the rig writes the
/// binary's.
fn host_port(request: &str, port: u16) -> String {
    let tail = format!(":{port}");
    request
        .split('\n')
        .map(|l| match l.strip_suffix(&tail) {
            Some(host) if l.starts_with("Host: ") => format!("{host}:{PORT}"),
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `scripts/cli-parity.sh`'s named difference `bash32-empty-brace`: an
/// empty payload takes cmd_signal's default, `"${3:-{\}}"`, which bash
/// before 4 (macOS's `/bin/bash` 3.2) keeps as `{\}`, so bash sends the
/// text `{\}` where the binary, as bash 4 and later, sends "". Only there,
/// bash's request is compared with that text read as "" (and its length
/// four bytes shorter). The shell's fix makes bash 3.2 send "" too: the
/// row then fails, saying the mark goes.
const EMPTY_BRACE: &str = "presence, an empty payload and kind, --inbox-silent";

fn bash32_empty_brace(name: &str, requests: Vec<String>) -> Vec<String> {
    const SENT: &str = r#""text": "{\\}""#;
    if name != EMPTY_BRACE || bash_version() >= (4, 0) {
        return requests;
    }
    requests
        .into_iter()
        .map(|r| {
            assert!(
                r.contains(SENT),
                "{name}: bash {:?} sends no text {{\\}}: the bash32-empty-brace mark goes\n{r}",
                bash_version()
            );
            let r = r.replacen(SENT, r#""text": """#, 1);
            let (head, body) = r.split_once("\n\n").expect("a head and a body");
            let head: Vec<String> = head
                .split('\n')
                .map(|l| match l.strip_prefix("Content-Length: ") {
                    Some(n) => format!("Content-Length: {}", n.parse::<usize>().unwrap() - 4),
                    None => l.to_string(),
                })
                .collect();
            format!("{}\n\n{body}", head.join("\n"))
        })
        .collect()
}

/// The binary's runs (with and without `python3`) and bash's, with the
/// signal's id and time masked: the requests, the programs run, the exit,
/// stdout and stderr.
fn masked(rig: &Rig, c: &Case) {
    let name = c.name;
    rig.prepare(Side::Bash, c);
    let b = rig.run(true, c);
    let bt = rig.take_tool_log();
    assert_eq!(
        rig.stub_of(Side::Bash).left(),
        0,
        "{name}: bash left answers: {b:?}"
    );
    let bl: Vec<String> = rig
        .read(Side::Bash, false, name)
        .iter()
        .map(|r| {
            mask(
                name,
                &host_port(
                    &normalise(r, false, VERB, name),
                    rig.stub_of(Side::Bash).port,
                ),
            )
        })
        .collect();
    assert_eq!(b.code, Some(c.code), "{name}: bash's exit: {b:?}");
    assert_eq!(bl.len(), c.requests, "{name}: bash's requests: {bl:#?}");
    let bl = bash32_empty_brace(name, bl);
    for without in [false, true] {
        let side = if without { Side::NoPython3 } else { Side::Rust };
        let (r, rl) = rig.binary(c, without);
        let rt = rig.take_tool_log();
        assert_eq!(rig.stub_of(side).left(), 0, "{name}: left answers: {r:?}");
        let rl: Vec<String> = rl.iter().map(|l| mask(name, l)).collect();
        assert_eq!(rl, bl, "{name} ({side:?}): the requests");
        assert_eq!(rt, bt, "{name} ({side:?}): the programs run");
        assert_eq!(
            (r.code, r.signal),
            (b.code, b.signal),
            "{name} ({side:?}): the exit\n{r:?}\n{b:?}"
        );
        assert_eq!(r.stdout, b.stdout, "{name} ({side:?}): stdout");
        assert_eq!(r.stderr, b.stderr, "{name} ({side:?}): stderr");
    }
}

/// A rig whose `PATH` starts with a `sqlite3` that logs its argv and
/// answers the home database's two questions: the live workspaces'
/// count, and the project's id.
fn sqlite_rig(tag: &str) -> Rig {
    use std::os::unix::fs::PermissionsExt;
    let mut rig = Rig::new(VERB, tag);
    let shim = rig.dir.join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    let log = rig.dir.join("sqlite.log");
    let script = format!(
        r#"#!/bin/sh
for a in "$@"; do printf 'argv=%s\n' "$a" >> '{log}'; done
case "$2" in
  *count*) case "$1" in *k2so.db*) echo 3 ;; *) echo 1 ;; esac ;;
  *"SELECT id"*) printf 'ws-0001\n\n' ;;
esac
"#,
        log = log.display()
    );
    let bin = shim.join("sqlite3");
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    rig.path_first = Some(shim);
    rig.tool_log = Some(log);
    rig
}

fn db_new(r: &Rig) {
    r.owner_on_disk();
    std::fs::write(r.k2home.join("k2.db"), b"db").unwrap();
}

fn db_both(r: &Rig) {
    db_new(r);
    std::fs::write(r.k2home.join("k2so.db"), b"old db").unwrap();
}

#[test]
fn msg_signal_sends_as_the_shell_does() {
    let rig = Rig::new(VERB, "signal");
    let cases = vec![
        // covers: msg
        plain(
            "a custom kind, a text payload, --from, quotes and a newline",
            &[
                "ta\"r\"\"\"g'e\\t\nz",
                "--signal",
                "k\"i\"\"\"n'd\\x\ny",
                "--payload",
                "pa\"y\"\"\"l'o\\a\nd",
                "--from",
                "fr\"o\"\"\"m'\\n\nw",
            ],
            vec![ok(OK)],
            0,
            1,
        ),
        plain(
            "kind msg, a JSON payload, --inbox-wake",
            &[
                "ws",
                "--signal",
                "msg",
                "--payload",
                r#"{"text": "h\u00e9 \"there\"", "n": 1.50}"#,
                "--inbox-wake",
                "--from",
                "ada",
            ],
            vec![ok(OK)],
            0,
            1,
        ),
        env(
            plain(
                "status from *_AGENT_NAME, the payload no JSON, other words skipped",
                &[
                    "ws",
                    "extra",
                    "--signal",
                    "status",
                    "--payload",
                    "{not",
                    "--x",
                ],
                vec![ok("answer with no newline")],
                0,
                1,
            ),
            &[("K2_AGENT_NAME", "bob")],
        ),
        plain(
            "presence, an empty payload and kind, --inbox-silent",
            &["ws", "--signal", "", "--payload", "", "--inbox-silent"],
            vec![ok("{}\n\n")],
            0,
            1,
        ),
        plain(
            "presence's own state",
            &[
                "ws",
                "--signal",
                "presence",
                "--payload",
                r#"{"state":"away"}"#,
            ],
            vec![ok(OK)],
            0,
            1,
        ),
        plain(
            "a custom kind with a list payload, bare --inbox",
            &[
                "ws",
                "--signal",
                "ping",
                "--payload",
                "[1, NaN, 2e400]",
                "--inbox",
            ],
            vec![ok(OK)],
            0,
            1,
        ),
        plain(
            "a refusal prints its answer and the closing line",
            &["ws", "--signal", "msg"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            0,
            1,
        ),
        plain(
            "a server error's answer as it came",
            &["ws", "--signal", "msg"],
            vec![Reply::Answer(500, "boom\n".into())],
            0,
            1,
        ),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            ..case(
                "in a session, on the cell socket",
                &["ws", "--signal", "msg", "--payload", "{\"text\":\"hi\"}"],
                &[],
                vec![ok("{\"ok\":true}\n\n")],
                nothing,
                0,
                1,
            )
        },
    ];
    for c in &cases {
        masked(&rig, c);
    }
}

#[test]
fn msg_signal_reads_the_workspace_id_from_the_home_database() {
    let rig = sqlite_rig("signal-db");
    for c in [
        case(
            "k2.db alone: the project's id",
            &["ws", "--signal", "msg"],
            &[],
            vec![ok(OK)],
            db_new,
            0,
            1,
        ),
        case(
            "k2.db and k2so.db: the one with more workspaces",
            &["ws", "--signal", "msg"],
            &[],
            vec![ok(OK)],
            db_both,
            0,
            1,
        ),
        case(
            "no database: the project's path, sqlite3 not run",
            &["ws", "--signal", "msg"],
            &[],
            vec![ok(OK)],
            owner_only,
            0,
            1,
        ),
    ] {
        masked(&rig, &owner_env(c));
    }
}

fn owner_env(c: Case) -> Case {
    Case {
        env: vec![("K2_PORT", PORT.to_string())],
        ..c
    }
}

#[test]
fn msg_signal_stops_before_sending() {
    let rig = Rig::new(VERB, "signal-stops");
    for c in [
        plain("an empty target", &["", "--signal", "msg"], vec![], 1, 0),
        Case {
            compare: Compare::Traceback,
            ..plain(
                "status with a payload that is a list",
                &["ws", "--signal", "status", "--payload", "[1]"],
                vec![],
                1,
                0,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..plain(
                "msg with a payload that is a number",
                &["ws", "--signal", "msg", "--payload", "7"],
                vec![],
                1,
                0,
            )
        },
        // Help never acts: the shell's page, nothing sent.
        Case {
            compare: Compare::Shell,
            ..plain(
                "--help after the signal",
                &["ws", "--signal", "msg", "--help"],
                vec![],
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..plain("-h first", &["-h", "ws", "--signal", "msg"], vec![], 0, 0)
        },
    ] {
        rig.check(&c);
    }
}

/// A value flag as the last word: bash stops on its own `$2: unbound
/// variable` line and exits 1; the binary prints one plain usage line and
/// exits 2 (K2's D6, the named difference `edge-usage-exit-2`). Neither
/// sends anything.
#[test]
fn msg_signal_value_flag_last_is_the_d6_line() {
    let rig = Rig::new(VERB, "signal-d6");
    let cli = k2_client::brand::CLI_NAME;
    for (name, args, flag) in [
        ("--signal last", &["ws", "--signal"][..], "--signal"),
        (
            "--payload last",
            &["ws", "--signal", "k", "--payload"],
            "--payload",
        ),
        ("--from last", &["ws", "--signal", "k", "--from"], "--from"),
    ] {
        let c = plain(name, args, vec![], 1, 0);
        rig.prepare(Side::Bash, &c);
        let b = rig.run(true, &c);
        if edge_usage_exit_2_proven() {
            assert_eq!(b.code, Some(1), "{name}: bash's exit: {b:?}");
            assert!(b.stdout.is_empty(), "{name}: bash's stdout: {b:?}");
            assert_eq!(b.stderr.lines().count(), 1, "{name}: bash's stderr: {b:?}");
            assert!(
                b.stderr.ends_with(": $2: unbound variable\n"),
                "{name}: bash's stderr: {b:?}"
            );
            assert!(
                rig.read(Side::Bash, false, name).is_empty(),
                "{name}: bash sent"
            );
        }
        let (r, sent) = rig.binary(&c, false);
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{name}: {r:?}");
        assert!(r.stdout.is_empty(), "{name}: {r:?}");
        assert_eq!(
            r.stderr,
            format!("Error: {flag} needs a value (try '{cli} msg --help')\n"),
            "{name}"
        );
    }
}
