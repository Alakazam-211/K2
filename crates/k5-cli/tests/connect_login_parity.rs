//! `k5-cli connect login` against the shell CLI's `cli/k2 connect login`,
//! on the parity rig ([`common::parity`]): the same stdout, stderr, exit
//! code, daemon requests and account-server requests, and the same
//! account file.
//!
//! The account server is a stub of its own (the daemon's stub is each
//! side's): every side's requests to it are compared byte for byte, its
//! port masked. The tunnel token, the access token and the password are
//! only ever in a request's body or its `Authorization` header: the test
//! checks no stand-in child saw them (no child runs on the binary's side)
//! and that they are in no argv of the binary's.
//!
//! - The token forms: `--token` (its note), `--token-stdin` (`\r` taken
//!   off, empty, with `--token`), the saved session's email.
//! - The picker: the only subdomain, `--subdomain` (any case), a numbered
//!   list and a line of stdin (a number, not a number, out of range), not
//!   in the account, none, a large list.
//! - The account server: a refusal, not there, the claim held by another
//!   device or failing.
//! - The daemon: no port or token (its three lines), `daemon.token`, the
//!   config's refusals, the start's error, an answer that is not JSON or
//!   not an object, no answer (each an error line and exit 1, never the
//!   live line).
//! - The words: usage errors, a password on the command line, air-gap,
//!   the guard and a session with no passport.
//! - A password with no terminal, read from stdin: the named difference
//!   getpass-no-tty-warning, where Python's getpass warns with its own
//!   file and line and the binary with one line of its own; every other
//!   line, the requests and the exit are the same.
//! - A subdomain list above what one environment variable may hold: the
//!   named difference picker-env-size (the shell's picker can't start,
//!   the binary picks).
//! - What the shell keeps: `-h`, which never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::{Reply, Stub};
use std::path::PathBuf;

const VERB: &str = "connect";
const ACCESS: &str = "jwt-acc-7272";
const TUNNEL: &str = "tun-sec-9191";
const DEVICE: &str = r#"{"subdomain":"","token":"","device_id":" dev-5151 "}"#;
const CONFIG_OK: &str = r#"{"subdomain":"acme","tokenSet":true}"#;

fn zone() -> &'static str {
    k2_client::brand::INFRA_ZONE
}

fn one_row() -> String {
    format!(r#"[{{"label":"acme","status":"active","tunnel_token":"{TUNNEL}","claimed_by":null}}]"#)
}

fn two_rows() -> String {
    format!(
        r#"[{{"label":"Acme","status":"active","tunnel_token":"{TUNNEL}"}},{{"label":"globex","status":"paused","tunnel_token":"tun-2"}},"not a row",{{"label":"  "}}]"#
    )
}

fn started() -> String {
    format!(
        r#"{{"running":true,"public_url":"https://acme.{}"}}"#,
        zone()
    )
}

fn put(r: &Rig, name: &str, body: Option<&str>) {
    let p = r.k2home.join(name);
    match body {
        Some(b) => std::fs::write(&p, b).unwrap(),
        None => {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The daemon set up, this device's id in `tunnel.json`, no session.
fn ready(r: &Rig) {
    owner_and_port(r);
    put(r, "tunnel.json", Some(DEVICE));
    put(r, "connect-account.json", None);
    put(r, "daemon.port", None);
    put(r, "settings.json", None);
}

fn saved_session(r: &Rig) {
    ready(r);
    put(
        r,
        "connect-account.json",
        Some(r#"{"access_token":"old","refresh_token":"r","email":" me@acme.example "}"#),
    );
}

fn no_daemon(r: &Rig) {
    ready(r);
    let _ = std::fs::remove_file(r.k2home.join("heartbeat.port"));
}

fn daemon_token_only(r: &Rig) {
    ready(r);
    let _ = std::fs::remove_file(r.k2home.join("heartbeat.token"));
    put(r, "daemon.token", Some("daemon-tok-3131\n"));
}

fn no_device(r: &Rig) {
    ready(r);
    put(r, "tunnel.json", None);
}

fn port_only_ready(r: &Rig) {
    ready(r);
    let _ = std::fs::remove_file(r.k2home.join("heartbeat.token"));
}

/// One row: the case, the account server's answers, and whether the
/// binary answers it (else it hands it to the shell).
struct Row {
    case: Case,
    account: Vec<Reply>,
    native: bool,
    /// The password is read with no terminal: bash's stderr has Python's
    /// getpass warning where the binary's has its own line
    /// ([`no_tty_warning`]).
    getpass: bool,
}

fn row(case: Case, account: Vec<Reply>) -> Row {
    Row {
        case,
        account,
        native: true,
        getpass: false,
    }
}

fn shells(case: Case, account: Vec<Reply>) -> Row {
    Row {
        case,
        account,
        native: false,
        getpass: false,
    }
}

/// A row whose password is read from stdin, with no terminal.
fn no_tty(case: Case, account: Vec<Reply>) -> Row {
    Row {
        case,
        account,
        native: true,
        getpass: true,
    }
}

/// The binary's line for Python's getpass warning.
const NO_TTY_LINE: &str = "Warning: Can not control echo on the terminal.";

/// Bash's stderr with Python's getpass warning (`<file>:<line>:
/// GetPassWarning: Can not control echo on the terminal.`, then the
/// source line Python shows under it when it has one) put as the
/// binary's one line. The warning may follow a prompt on its line (the
/// email's), which stays. It must be there exactly once, and the line
/// after it Python's own `Warning: Password input may be echoed.`, which
/// both print.
fn no_tty_warning(name: &str, stderr: &str) -> String {
    let lines: Vec<&str> = stderr.split_inclusive('\n').collect();
    let at: Vec<usize> = (0..lines.len())
        .filter(|i| {
            lines[*i].ends_with(": GetPassWarning: Can not control echo on the terminal.\n")
                && lines[*i].contains(".py:")
        })
        .collect();
    assert_eq!(
        at.len(),
        1,
        "{name}: Python's getpass warning, once: {stderr:?}"
    );
    let i = at[0];
    let mut next = i + 1;
    if lines.get(next).is_some_and(|l| l.starts_with("  ")) {
        next += 1;
    }
    const SECOND: &str = "Warning: Password input may be echoed.\n";
    let second = lines.get(next).is_some_and(|l| l.starts_with(SECOND));
    assert!(
        second,
        "{name}: Python's second warning line after the first: {stderr:?}"
    );
    let mut out: String = lines[..i].concat();
    let py = lines[i].find(".py:").expect("the warning names its file");
    let path_at = lines[i][..py].rfind(' ').map_or(0, |s| s + 1);
    out.push_str(&lines[i][..path_at]);
    out.push_str(NO_TTY_LINE);
    out.push('\n');
    out.push_str(&lines[next..].concat());
    out
}

/// Fails loudly when this process has a terminal to open: the no-terminal
/// rows read the password from stdin only when there is none (they run
/// on compute, where tests have no terminal).
fn no_terminal_here() {
    assert!(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_err(),
        "the no-terminal rows need a process with no controlling terminal"
    );
}

fn with_stdin(c: Case, input: &'static [u8]) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

/// Each 8-4-4-4-12 hex run (a UUID 4 the CLI made) written `{uuid}`.
fn mask_uuids(s: &str) -> String {
    let b = s.as_bytes();
    let shape = [8, 4, 4, 4, 12];
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < b.len() {
        let mut j = i;
        for (n, len) in shape.into_iter().enumerate() {
            let hex = j + len <= b.len()
                && b[j..j + len]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c));
            if !hex {
                out.push(b[i] as char);
                i += 1;
                continue 'outer;
            }
            j += len;
            if n < 4 {
                if b.get(j) != Some(&b'-') {
                    out.push(b[i] as char);
                    i += 1;
                    continue 'outer;
                }
                j += 1;
            }
        }
        assert_eq!(
            b[i + 14],
            b'4',
            "a UUID that is not version 4: {}",
            &s[i..j]
        );
        out.push_str("{uuid}");
        i = j;
    }
    out
}

/// A side's account-server requests, its port masked.
fn account_log(acct: &Stub) -> Vec<String> {
    acct.take_log()
        .iter()
        .map(|l| {
            mask_uuids(
                &String::from_utf8_lossy(&l.bytes)
                    .replace(&format!("127.0.0.1:{}", acct.port), "127.0.0.1:{acct}"),
            )
        })
        .collect()
}

type Seen = (
    Option<i32>,
    String,
    String,
    Vec<String>,
    Vec<String>,
    Option<String>,
);

/// Runs `row` on the shell, the binary and (for a native row) the binary
/// with no `python3`, and compares them.
fn check(r: &Rig, acct: &Stub, row: &Row) {
    let c = &row.case;
    let session = |r: &Rig| std::fs::read_to_string(r.k2home.join("connect-account.json")).ok();
    r.prepare(Side::Bash, c);
    acct.reset(&row.account);
    let out = r.run(true, c);
    let bash_port = r.stub_of(Side::Bash).port;
    let dlog: Vec<String> = r
        .read(Side::Bash, false, c.name)
        .iter()
        .map(|l| {
            mask_uuids(&normalise(l, false, VERB, c.name).replace(
                &format!("127.0.0.1:{bash_port}"),
                &format!("127.0.0.1:{PORT}"),
            ))
        })
        .collect();
    let stderr = if row.getpass {
        no_tty_warning(c.name, &out.stderr)
    } else {
        out.stderr
    };
    let bash: Seen = (
        out.code,
        out.stdout,
        stderr,
        dlog,
        account_log(acct),
        session(r),
    );
    println!("-- bash: {bash:?}");
    assert_eq!(bash.0, Some(c.code), "{}: bash's exit", c.name);
    assert_eq!(
        bash.3.len(),
        c.requests,
        "{}: bash's daemon requests",
        c.name
    );
    let sides: &[bool] = if row.native { &[false, true] } else { &[false] };
    for nopy in sides {
        acct.reset(&row.account);
        let (out, log) = r.binary(c, *nopy);
        let handed = k5_cli::fallthrough::cache_dir(&r.k2home).exists();
        assert_eq!(handed, !row.native, "{}: handed to the shell", c.name);
        let got: Seen = (
            out.code,
            out.stdout,
            out.stderr,
            log.iter().map(|l| mask_uuids(l.as_str())).collect(),
            account_log(acct),
            session(r),
        );
        assert_eq!(
            got, bash,
            "{}: the binary (no python3: {nopy}) against bash",
            c.name
        );
    }
}

/// The account server's URL, for the life of the test process.
fn db(acct: &Stub) -> &'static str {
    Box::leak(format!("http://127.0.0.1:{}/", acct.port).into_boxed_str())
}

fn account_stub() -> Stub {
    Stub::start(&PathBuf::from(format!(
        "/tmp/k5la-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    )))
}

fn ok_json(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

#[test]
fn connect_login_with_a_token_answers_as_the_shell_does() {
    let r = Rig::new(VERB, "login");
    let acct = account_stub();
    let url = db(&acct);
    let env = |extra: &[(&'static str, &'static str)]| {
        let mut v: Vec<(&'static str, &'static str)> = vec![("K2_CONNECT_DB_URL", url)];
        v.extend_from_slice(extra);
        v
    };
    let started = started();
    let rows = vec![
        // covers: connect login
        row(
            case(
                "--token: the only subdomain, claimed, bound, started",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json(&started)],
                ready,
                0,
                2,
            ),
            vec![ok_json(&one_row()), ok_json(r#"[{"claimed":true}]"#)],
        ),
        row(
            with_stdin(
                case(
                    "--token-stdin and --subdomain in another case",
                    &["login", "--token-stdin", "--subdomain", " ACME "],
                    &env(&[]),
                    vec![ok_json(CONFIG_OK), ok_json("{}")],
                    saved_session,
                    0,
                    2,
                ),
                b"jwt-acc-7272\r\n",
            ),
            vec![ok_json(&two_rows()), ok_json("[]")],
        ),
        row(
            with_stdin(
                case(
                    "the numbered list, the second picked",
                    &["login", "--token-stdin"],
                    &env(&[]),
                    vec![ok_json(r#"{"subdomain":"globex"}"#), ok_json(&started)],
                    ready,
                    0,
                    2,
                ),
                b"jwt-acc-7272\n 2 \n",
            ),
            vec![
                ok_json(&two_rows()),
                ok_json(r#"{"claimed":false,"holder":"dev-5151"}"#),
            ],
        ),
        row(
            with_stdin(
                case(
                    "the numbered list, not a number",
                    &["login", "--token-stdin"],
                    &env(&[]),
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"jwt-acc-7272\nsecond\n",
            ),
            vec![ok_json(&two_rows())],
        ),
        row(
            with_stdin(
                case(
                    "the numbered list, out of range, the input ended",
                    &["login", "--token-stdin"],
                    &env(&[]),
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"jwt-acc-7272\n9",
            ),
            vec![ok_json(&two_rows())],
        ),
        row(
            case(
                "--subdomain not in the account",
                &["login", "--token", ACCESS, "--subdomain=initech"],
                &env(&[]),
                vec![],
                ready,
                1,
                0,
            ),
            vec![ok_json(&two_rows())],
        ),
        row(
            case(
                "no subdomains",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![],
                ready,
                1,
                0,
            ),
            vec![ok_json("[]")],
        ),
        row(
            case(
                "the account server refuses the token",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![],
                ready,
                1,
                0,
            ),
            vec![Reply::Answer(401, r#"{"message":"JWT expired"}"#.into())],
        ),
        row(
            case(
                "the account server refuses with no body",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![],
                ready,
                1,
                0,
            ),
            vec![Reply::Answer(503, String::new())],
        ),
        row(
            case(
                "the account server is not there",
                &["login", "--token", ACCESS],
                &[(
                    "K2_CONNECT_DB_URL",
                    format!("http://127.0.0.1:{}/", dead_port()).as_str(),
                )],
                vec![],
                ready,
                1,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "the subdomain is in use on another device",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![],
                ready,
                1,
                0,
            ),
            vec![
                ok_json(&one_row()),
                ok_json(r#"[{"claimed":false,"holder":"other-dev","holder_label":"laptop"}]"#),
            ],
        ),
        row(
            case(
                "the claim fails: a warning, then the bind; no device id yet",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json(&started)],
                no_device,
                0,
                2,
            ),
            vec![ok_json(&one_row()), Reply::Answer(500, "{}".into())],
        ),
        row(
            case(
                "no daemon: its three lines after the account steps",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![],
                no_daemon,
                1,
                0,
            ),
            vec![ok_json(&one_row())],
        ),
        row(
            case(
                "the daemon's own token from daemon.token",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json(&started)],
                daemon_token_only,
                0,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the config's error",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(
                    r#"{"error":{"code":"locked","hint":"the vault is locked"}}"#,
                )],
                ready,
                1,
                1,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the config did not take the token",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(r#"{"tokenSet":false}"#)],
                ready,
                1,
                1,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the config answer is not JSON",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json("<html>it's down</html>")],
                ready,
                1,
                1,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the start's error",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![
                    ok_json(CONFIG_OK),
                    ok_json(r#"{"error":"frpc is not installed"}"#),
                ],
                ready,
                1,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the start's answer is not JSON",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json("nope")],
                ready,
                1,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "no answer to the config: its line, exit 1",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![Reply::Close],
                ready,
                1,
                1,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        // covers: connect login
        row(
            case(
                "no answer to the start: its line, exit 1, never live",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), Reply::Close],
                ready,
                1,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the start answers an empty body: no answer, exit 1",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json("")],
                ready,
                1,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the start answers a JSON list: exit 1",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json("[]")],
                ready,
                1,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
        row(
            case(
                "the start answers null: exit 1",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![ok_json(CONFIG_OK), ok_json("null")],
                ready,
                1,
                2,
            ),
            vec![ok_json(&one_row()), ok_json("[]")],
        ),
    ];
    for row in &rows {
        check(&r, &acct, row);
    }
}

#[test]
fn connect_login_s_words_and_gates_answer_as_the_shell_does() {
    let r = Rig::new(VERB, "login-words");
    let acct = account_stub();
    let url = db(&acct);
    let env = |extra: &[(&'static str, &'static str)]| {
        let mut v: Vec<(&'static str, &'static str)> = vec![("K2_CONNECT_DB_URL", url)];
        v.extend_from_slice(extra);
        v
    };
    let rows = vec![
        row(
            case(
                "--password",
                &["login", "--password", "x"],
                &env(&[]),
                vec![],
                ready,
                2,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "--email with no value",
                &["login", "--email"],
                &env(&[]),
                vec![],
                ready,
                2,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "an unknown flag: the usage",
                &["login", "-x"],
                &env(&[]),
                vec![],
                ready,
                2,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "a stray word: the usage",
                &["login", "acme"],
                &env(&[]),
                vec![],
                ready,
                2,
                0,
            ),
            vec![],
        ),
        row(
            with_stdin(
                case(
                    "--token-stdin and --token",
                    &["login", "--token-stdin", "--token", ACCESS],
                    &env(&[]),
                    vec![],
                    ready,
                    2,
                    0,
                ),
                b"jwt-acc-7272\n",
            ),
            vec![],
        ),
        row(
            with_stdin(
                case(
                    "--token-stdin with nothing on stdin",
                    &["login", "--token-stdin"],
                    &env(&[]),
                    vec![],
                    ready,
                    2,
                    0,
                ),
                b"",
            ),
            vec![],
        ),
        row(
            case(
                "air-gap: before anything else",
                &["login", "--token", ACCESS],
                &env(&[("K2_AIRGAP", "1")]),
                vec![],
                ready,
                1,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "air-gap comes before the guard",
                &["login", "--token", ACCESS],
                &env(&[("K2_AIRGAP", "1"), ("K2_NO_OWNER_FALLBACK", "1")]),
                vec![],
                port_only_ready,
                1,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "the guard",
                &["login", "--token", ACCESS],
                &env(&[("K2_NO_OWNER_FALLBACK", "1")]),
                vec![],
                port_only_ready,
                3,
                0,
            ),
            vec![],
        ),
        row(
            case(
                "a session with no passport",
                &["login", "--token", ACCESS],
                &env(&[("K2_HOOK_SOCK", "{sock}")]),
                vec![],
                ready,
                3,
                0,
            ),
            vec![],
        ),
        shells(
            case(
                "-h never acts",
                &["login", "--token", ACCESS, "-h"],
                &env(&[]),
                vec![],
                ready,
                0,
                0,
            ),
            vec![],
        ),
    ];
    for row in &rows {
        check(&r, &acct, row);
    }
    // Help sent nothing to the account server.
    assert!(acct.log().is_empty());
}

const SIGNED_IN: &str =
    r#"{"access_token":"jwt-acc-7272","refresh_token":"r","user":{"email":"me@acme.example"}}"#;

fn signed_in() -> Reply {
    ok_json(SIGNED_IN)
}

fn refused() -> Reply {
    Reply::Answer(
        400,
        r#"{"error_description":"Invalid login credentials"}"#.into(),
    )
}

/// The password with no terminal, read from stdin (the named difference
/// getpass-no-tty-warning): the same as the shell's but for the warning's
/// line.
#[test]
fn connect_login_with_a_password_and_no_terminal_answers_as_the_shell_does() {
    no_terminal_here();
    let r = Rig::new(VERB, "login-no-tty");
    let acct = account_stub();
    let url = db(&acct);
    let env: &[(&'static str, &str)] = &[("K2_CONNECT_DB_URL", url)];
    let started = started();
    let words: &'static [&'static str] =
        &["login", "--email", "me@acme.example", "--subdomain", "acme"];
    let rows = vec![
        // covers: connect login
        no_tty(
            with_stdin(
                case(
                    "no terminal: the password from stdin, refused",
                    words,
                    env,
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"hunter2-5353\n",
            ),
            vec![refused()],
        ),
        no_tty(
            with_stdin(
                case(
                    "no terminal: signed in, bound, started; a CR before the newline stays",
                    words,
                    env,
                    vec![ok_json(CONFIG_OK), ok_json(&started)],
                    ready,
                    0,
                    2,
                ),
                b"hunter2-5353\r\n",
            ),
            vec![signed_in(), ok_json(&one_row()), ok_json("[]")],
        ),
        no_tty(
            with_stdin(
                case(
                    "no terminal: email, password, then a picker that finds stdin used up",
                    &["login"],
                    env,
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"me@acme.example\nhunter2-5353\n2\n",
            ),
            vec![signed_in(), ok_json(&two_rows())],
        ),
        no_tty(
            with_stdin(
                case(
                    "no terminal: a password with no newline after it",
                    words,
                    env,
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"hunter2-5353",
            ),
            vec![refused()],
        ),
        no_tty(
            with_stdin(
                case(
                    "no terminal: a CR in the line stays in the password",
                    words,
                    env,
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"hunter2\r5353\n",
            ),
            vec![refused()],
        ),
        no_tty(
            with_stdin(
                case(
                    "no terminal: an empty line for the password",
                    words,
                    env,
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"\n",
            ),
            vec![],
        ),
        no_tty(
            with_stdin(
                case(
                    "no terminal: nothing on stdin, the prompt cancelled",
                    words,
                    env,
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"",
            ),
            vec![],
        ),
    ];
    for row in &rows {
        check(&r, &acct, row);
    }
}

/// A subdomain list above what one environment variable may hold (128
/// KiB on Linux) and above macOS's limit for the whole environment
/// (about 1 MiB): the named difference picker-env-size. The shell's
/// picker gets the list in its environment and can't start ("Argument
/// list too long"): exit 1 after the list, nothing asked of the daemon.
/// The binary picks, claims, binds and starts.
#[test]
fn connect_login_with_a_list_above_the_environment_s_limit_picks() {
    let r = Rig::new(VERB, "login-env-size");
    let acct = account_stub();
    let url = db(&acct);
    let pad = "x".repeat(220);
    let rows: Vec<String> = (0..6000)
        .map(|i| format!(r#"{{"label":"s{i:04}","c":"{pad}","tunnel_token":"t{i:04}"}}"#))
        .collect();
    let body = format!("[{}]", rows.join(","));
    assert!(body.len() > 1024 * 1024 + 256 * 1024);
    // covers: connect login
    let c = case(
        "a list above the environment's limit",
        &["login", "--token", ACCESS, "--subdomain", "s5999"],
        &[("K2_CONNECT_DB_URL", url)],
        vec![ok_json(CONFIG_OK), ok_json(&started())],
        ready,
        1,
        0,
    );
    let account = [ok_json(&body), ok_json("[]")];
    r.prepare(Side::Bash, &c);
    acct.reset(&account);
    let b = r.run(true, &c);
    let bash_daemon = r.read(Side::Bash, false, c.name);
    let bash_account = account_log(&acct);
    assert_eq!(b.code, Some(1), "bash: {b:?}");
    assert!(
        b.stderr.contains("Argument list too long"),
        "bash: {}",
        b.stderr
    );
    assert_eq!(b.stdout, "", "bash's stdout");
    assert!(
        bash_daemon.is_empty(),
        "bash asked the daemon: {bash_daemon:?}"
    );
    assert_eq!(bash_account.len(), 1, "bash: the list only");
    acct.reset(&account);
    let (o, log) = r.binary(&c, false);
    assert_eq!(o.code, Some(0), "the binary: {o:?}");
    assert_eq!(
        o.stdout,
        format!("\u{2713} live at https://acme.{}\n", zone())
    );
    assert!(!o.stderr.contains("Argument list"), "{}", o.stderr);
    assert_eq!(log.len(), 2, "the binary: the config and the start");
    let sent = account_log(&acct);
    assert_eq!(sent.len(), 2, "the binary: the list and the claim");
    assert_eq!(sent[0], bash_account[0], "the list request");
}

/// A list well above 256 KiB: 2000 rows, the last one picked.
#[test]
fn connect_login_with_a_large_list_answers_as_the_shell_does() {
    let r = Rig::new(VERB, "login-large");
    let acct = account_stub();
    let url = db(&acct);
    let pad = "x".repeat(120);
    let rows: Vec<String> = (0..2000)
        .map(|i| {
            format!(r#"{{"label":"s{i:04}","status":"active","tunnel_token":"t{i:04}{pad}"}}"#)
        })
        .collect();
    let body = format!("[{}]", rows.join(","));
    assert!(body.len() > 256 * 1024);
    let c = case(
        "a large list",
        &["login", "--token", ACCESS, "--subdomain", "s1999"],
        &[("K2_CONNECT_DB_URL", url)],
        vec![ok_json(CONFIG_OK), ok_json("{}")],
        ready,
        0,
        2,
    );
    check(&r, &acct, &row(c, vec![ok_json(&body), ok_json("[]")]));
}

/// No secret is on the binary's argv, and the account file holds the
/// access token after a login.
#[test]
fn connect_login_keeps_the_secrets_off_argv() {
    let r = Rig::new(VERB, "login-argv");
    let acct = account_stub();
    let url = db(&acct);
    let c = with_stdin(
        case(
            "secrets",
            &["login", "--token-stdin"],
            &[("K2_CONNECT_DB_URL", url)],
            vec![ok_json(CONFIG_OK), ok_json("{}")],
            ready,
            0,
            2,
        ),
        b"jwt-acc-7272\n",
    );
    for a in r.argv(&c) {
        assert!(!a.contains(ACCESS) && !a.contains(TUNNEL), "{a}");
    }
    acct.reset(&[ok_json(&one_row()), ok_json("[]")]);
    let (out, log) = r.binary(&c, false);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert!(
        log.iter().any(|l| l.contains(TUNNEL)),
        "the tunnel token reached the daemon"
    );
    let sent = account_log(&acct);
    assert!(sent
        .iter()
        .all(|l| l.contains(&format!("Authorization: Bearer {ACCESS}\r\n"))));
    let saved = std::fs::read_to_string(r.k2home.join("connect-account.json")).unwrap();
    assert_eq!(
        saved,
        r#"{"access_token":"jwt-acc-7272","refresh_token":"","email":"(token)"}"#
    );
}

/// The lines name the command by the brand's name.
#[test]
fn connect_login_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let r = Rig::new(VERB, "login-brand");
    let acct = account_stub();
    let url = db(&acct);
    let c = case(
        "brand",
        &["login", "--token", ACCESS],
        &[("K2_CONNECT_DB_URL", url)],
        vec![],
        no_daemon,
        1,
        0,
    );
    acct.reset(&[ok_json(&one_row())]);
    let (out, _) = r.binary(&c, false);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stderr.contains(&format!(
            "Start it first, then re-run: {cli} connect login\n"
        )),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains(&format!("  {cli} daemon start ")));
    if cli != "k2" {
        assert!(!out.stderr.contains("k2 connect"), "{}", out.stderr);
    }
}
