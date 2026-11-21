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
//!   config's refusals, the start's error and an answer that is not JSON,
//!   no answer.
//! - The words: usage errors, a password on the command line, air-gap,
//!   the guard and a session with no passport.
//! - What the shell keeps: a password with no terminal (Python's getpass
//!   warning names its own file), and `-h`, which never acts.

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
    format!(r#"{{"running":true,"public_url":"https://acme.{}"}}"#, zone())
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
}

fn row(case: Case, account: Vec<Reply>) -> Row {
    Row {
        case,
        account,
        native: true,
    }
}

fn shells(case: Case, account: Vec<Reply>) -> Row {
    Row {
        case,
        account,
        native: false,
    }
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
        assert_eq!(b[i + 14], b'4', "a UUID that is not version 4: {}", &s[i..j]);
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
            mask_uuids(
                &normalise(l, false, VERB, c.name)
                    .replace(&format!("127.0.0.1:{bash_port}"), &format!("127.0.0.1:{PORT}")),
            )
        })
        .collect();
    let bash: Seen = (
        out.code,
        out.stdout,
        out.stderr,
        dlog,
        account_log(acct),
        session(r),
    );
    println!("-- bash: {bash:?}");
    assert_eq!(bash.0, Some(c.code), "{}: bash's exit", c.name);
    assert_eq!(bash.3.len(), c.requests, "{}: bash's daemon requests", c.name);
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
        assert_eq!(got, bash, "{}: the binary (no python3: {nopy}) against bash", c.name);
    }
}

fn db(acct: &Stub) -> String {
    format!("http://127.0.0.1:{}/", acct.port)
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
    let env = |extra: &[(&'static str, &str)]| {
        let mut v = vec![("K2_CONNECT_DB_URL", url.as_str())];
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
            vec![ok_json(&two_rows()), ok_json(r#"{"claimed":false,"holder":"dev-5151"}"#)],
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
                &[("K2_CONNECT_DB_URL", format!("http://127.0.0.1:{}/", dead_port()).as_str())],
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
                vec![ok_json(r#"{"error":{"code":"locked","hint":"the vault is locked"}}"#)],
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
                vec![ok_json(CONFIG_OK), ok_json(r#"{"error":"frpc is not installed"}"#)],
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
                "the daemon closes on the config and the start: live all the same",
                &["login", "--token", ACCESS],
                &env(&[]),
                vec![Reply::Close, Reply::Close],
                ready,
                0,
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
    let env = |extra: &[(&'static str, &str)]| {
        let mut v = vec![("K2_CONNECT_DB_URL", url.as_str())];
        v.extend_from_slice(extra);
        v
    };
    let rows = vec![
        row(
            case("--password", &["login", "--password", "x"], &env(&[]), vec![], ready, 2, 0),
            vec![],
        ),
        row(
            case("--email with no value", &["login", "--email"], &env(&[]), vec![], ready, 2, 0),
            vec![],
        ),
        row(
            case("an unknown flag: the usage", &["login", "-x"], &env(&[]), vec![], ready, 2, 0),
            vec![],
        ),
        row(
            case("a stray word: the usage", &["login", "acme"], &env(&[]), vec![], ready, 2, 0),
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
                "air-gap: after the account server's line",
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
            with_stdin(
                case(
                    "a password with no terminal is the shell's",
                    &["login", "--email", "me@acme.example", "--subdomain", "acme"],
                    &env(&[]),
                    vec![],
                    ready,
                    1,
                    0,
                ),
                b"hunter2-5353\n",
            ),
            vec![Reply::Answer(400, r#"{"error_description":"Invalid login credentials"}"#.into())],
        ),
        shells(
            case("-h never acts", &["login", "--token", ACCESS, "-h"], &env(&[]), vec![], ready, 0, 0),
            vec![],
        ),
    ];
    for row in &rows {
        check(&r, &acct, row);
    }
    // Help sent nothing to the account server.
    assert!(acct.log().is_empty());
}

/// A list well above 256 KiB: 2000 rows, the last one picked.
#[test]
fn connect_login_with_a_large_list_answers_as_the_shell_does() {
    let r = Rig::new(VERB, "login-large");
    let acct = account_stub();
    let url = db(&acct);
    let pad = "x".repeat(120);
    let rows: Vec<String> = (0..2000)
        .map(|i| format!(r#"{{"label":"s{i:04}","status":"active","tunnel_token":"t{i:04}{pad}"}}"#))
        .collect();
    let body = format!("[{}]", rows.join(","));
    assert!(body.len() > 256 * 1024);
    let c = case(
        "a large list",
        &["login", "--token", ACCESS, "--subdomain", "s1999"],
        &[("K2_CONNECT_DB_URL", url.as_str())],
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
            &[("K2_CONNECT_DB_URL", url.as_str())],
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
    assert!(log.iter().any(|l| l.contains(TUNNEL)), "the tunnel token reached the daemon");
    let sent = account_log(&acct);
    assert!(sent.iter().all(|l| l.contains(&format!("Authorization: Bearer {ACCESS}\r\n"))));
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
        &[("K2_CONNECT_DB_URL", url.as_str())],
        vec![],
        no_daemon,
        1,
        0,
    );
    acct.reset(&[ok_json(&one_row())]);
    let (out, _) = r.binary(&c, false);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stderr
            .contains(&format!("Start it first, then re-run: {cli} connect login\n")),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains(&format!("  {cli} daemon start ")));
    if cli != "k2" {
        assert!(!out.stderr.contains("k2 connect"), "{}", out.stderr);
    }
}
