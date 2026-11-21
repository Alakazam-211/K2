//! A Codex call's conversation headers on the binary's own requests, as
//! the shell's `_k2_codex_headers` sends them (`cli/src/cli-tools-policy.sh`,
//! from 0.46.17): `X-K2-Conversation` (`CODEX_SESSION_ID`) and
//! `X-K2-Thread` (`CODEX_THREAD_ID`) on every daemon request, on the cell
//! socket and over TCP alike, only while `CODEX_THREAD_ID` is set and
//! `K2_CODEX_PASSPORT_BIND` is not off, each id only when it is 1 to 128
//! of letters, digits, `-` and `_`; never to a server that is not the
//! daemon (the Connect account server).
//!
//! The rows run the binary alone against the rig's stub daemon
//! ([`common::parity::Rig::binary`]) and read the headers off every
//! request it logged: `permission request` and `permission status` (a
//! GET) on the cell socket and over TCP, `msg`, `thread` (a write and a
//! read, the read a GET) over TCP. With a cell socket `msg` and `thread`
//! hand a Codex call to the shell, whose own headers are the shell's
//! (tests/cli/codex_conversation_headers.sh holds both sides).

#![cfg(unix)]

mod common;

use common::parity::*;

const KEY: &str = "parity-key-0001";
const ROOT: &str = "root-1";
const THREAD: &str = "thread-1";
const ON: &str = r#"{"enabled":true}"#;
const SEND_ONCE: &str = r#"{"version":"0","features":["send-once"]}"#;
const SENT: &str = r#"{"success":true,"target_session_id":"s1","attempts":1}"#;
const POSTED: &str = r#"{"ok":true,"id":"m1","seq":4,"from":"me"}"#;
const READ: &str =
    r#"{"ok":true,"items":[{"seq":1,"doc":{"from":"ada","body":"hi"}}],"has_more":false}"#;
const ACCOUNT: &str = r#"{"access_token":"a.b.c","refresh_token":"r1","email":"me@acme.example"}"#;

/// A Codex tool call: both ids, and the marker of a tab K2 started
/// (`K2_CODEX_TAB_ISOLATED`, the passport's session), which the shell
/// asks for before it sends anything.
fn codex(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("CODEX_THREAD_ID", THREAD.to_string()),
        ("CODEX_SESSION_ID", ROOT.to_string()),
        ("K2_CODEX_TAB_ISOLATED", "sessA".to_string()),
    ];
    env.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    env
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// One logged request's channel and its header lines.
fn parts(request: &str) -> (&str, Vec<&str>) {
    let (head, _) = request
        .split_once("\n\n")
        .unwrap_or_else(|| panic!("a request with no head: {request:?}"));
    let mut lines = head.lines();
    let channel = lines.next().unwrap_or_default();
    // The request line, then the headers.
    lines.next();
    (channel, lines.collect())
}

/// The values of `name` in `headers`.
fn values<'a>(headers: &[&'a str], name: &str) -> Vec<&'a str> {
    headers
        .iter()
        .filter_map(|h| h.strip_prefix(name)?.strip_prefix(": "))
        .collect()
}

/// Runs `c` on the binary: its exit and request count as the row says,
/// every request on `channel`, and each carrying exactly `want` (none
/// for `None`) as its conversation headers, the conversation first.
fn check(rig: &Rig, c: &Case, channel: &str, want: Option<(&str, &str)>) {
    let (out, log) = rig.binary(c, false);
    assert_eq!(out.code, Some(c.code), "{}: {out:?}", c.name);
    assert_eq!(log.len(), c.requests, "{}: {log:#?}", c.name);
    for r in &log {
        let (on, headers) = parts(r);
        assert_eq!(on, format!("[{channel}]"), "{}: {r}", c.name);
        let got: Vec<&str> = headers
            .iter()
            .copied()
            .filter(|h| h.starts_with("X-K2-Conversation:") || h.starts_with("X-K2-Thread:"))
            .collect();
        let want: Vec<String> = match want {
            Some(("", thread)) => vec![format!("X-K2-Thread: {thread}")],
            Some((root, thread)) => vec![
                format!("X-K2-Conversation: {root}"),
                format!("X-K2-Thread: {thread}"),
            ],
            None => vec![],
        };
        assert_eq!(got, want, "{}: {r}", c.name);
        assert_eq!(values(&headers, "X-K5-Cli-Via"), ["<via>"], "{}", c.name);
    }
}

#[test]
fn both_headers_on_the_cell_socket() {
    let rig = Rig::new("permission", "codexsock");
    let sock = codex(&[("K2_HOOK_SOCK", "{sock}")]);
    let rows = [
        case(
            "permission request: the probe and the request",
            &["request", "x", "--why", "w"],
            &[],
            vec![ok(ON), ok(r#"{"id":"pr_1"}"#)],
            port_only,
            0,
            2,
        ),
        case(
            "permission status: two GETs",
            &["status", "pr_1"],
            &[],
            vec![ok(ON), ok(r#"{"status":"pending"}"#)],
            port_only,
            0,
            2,
        ),
    ];
    for c in rows {
        let env: Vec<_> = with_passport(&[]).into_iter().chain(sock.clone()).collect();
        check(&rig, &with(env, c), "uds", Some((ROOT, THREAD)));
    }
}

#[test]
fn both_headers_over_tcp() {
    let rig = Rig::new("permission", "codextcp");
    let env = || -> Vec<(&'static str, String)> {
        with_passport(&[]).into_iter().chain(codex(&[])).collect()
    };
    for c in [
        case(
            "permission request with the passport over TCP",
            &["request", "x", "--why", "w"],
            &[],
            vec![ok(ON), ok(r#"{"id":"pr_1"}"#)],
            port_only,
            0,
            2,
        ),
        case(
            "permission status over TCP: two GETs",
            &["status", "pr_1"],
            &[],
            vec![ok(ON), ok(r#"{"status":"pending"}"#)],
            port_only,
            0,
            2,
        ),
    ] {
        check(&rig, &with(env(), c), "tcp", Some((ROOT, THREAD)));
    }
    let msg = Rig::new("msg", "codextcp");
    let c = case(
        "msg: the send-once probe and the send",
        &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
        &[],
        vec![ok(SEND_ONCE), ok(SENT)],
        nothing,
        0,
        2,
    );
    check(&msg, &with(env(), c), "tcp", Some((ROOT, THREAD)));
    let thread = Rig::new("thread", "codextcp");
    for c in [
        case(
            "thread: a write",
            &["bob", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![ok(SEND_ONCE), ok(POSTED)],
            nothing,
            0,
            2,
        ),
        case(
            "thread: a read, a GET",
            &["bob", "--from", "me"],
            &[],
            vec![ok(READ)],
            nothing,
            0,
            1,
        ),
    ] {
        check(&thread, &with(env(), c), "tcp", Some((ROOT, THREAD)));
    }
    // The owner's token, no passport: the headers all the same.
    let c = case(
        "thread: a read as the owner",
        &["bob", "--from", "me"],
        &[],
        vec![ok(READ)],
        owner_only,
        0,
        1,
    );
    let owner = [("K2_PORT", PORT.to_string())]
        .into_iter()
        .chain(codex(&[]));
    check(
        &thread,
        &with(owner.collect(), c),
        "tcp",
        Some((ROOT, THREAD)),
    );
}

#[test]
fn no_headers_unless_codex_set_them() {
    let rig = Rig::new("permission", "codexnone");
    let request = |name| {
        case(
            name,
            &["request", "x", "--why", "w"],
            &[],
            vec![ok(ON), ok(r#"{"id":"pr_1"}"#)],
            port_only,
            0,
            2,
        )
    };
    let sock = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let plus = |extra: &[(&'static str, &str)]| -> Vec<(&'static str, String)> {
        sock()
            .into_iter()
            .chain(extra.iter().map(|(k, v)| (*k, v.to_string())))
            .collect()
    };
    // No Codex setting at all.
    check(
        &rig,
        &with(sock(), request("no CODEX_ setting")),
        "uds",
        None,
    );
    // A root and no thread: Codex's own mark is the thread.
    let c = with(
        plus(&[("CODEX_SESSION_ID", ROOT)]),
        request("CODEX_SESSION_ID alone"),
    );
    check(&rig, &c, "uds", None);
    let c = with(
        plus(&[("CODEX_THREAD_ID", ""), ("CODEX_SESSION_ID", ROOT)]),
        request("an empty CODEX_THREAD_ID"),
    );
    check(&rig, &c, "uds", None);
    // The switch off.
    for off in ["0", "off", "false", "no"] {
        let mut env = plus(&[("K2_CODEX_PASSPORT_BIND", off)]);
        env.extend(codex(&[]));
        check(&rig, &with(env, request("the switch off")), "uds", None);
    }
    // A root id the shell won't send: the thread's alone.
    let c = with(
        plus(&[
            ("CODEX_THREAD_ID", THREAD),
            ("CODEX_SESSION_ID", "root 1"),
            ("K2_CODEX_TAB_ISOLATED", "sessA"),
        ]),
        request("a root id with a space"),
    );
    check(&rig, &c, "uds", Some(("", THREAD)));
}

/// `connect logout`'s best-effort POST goes to the account server, not
/// the daemon (the shell's Python gets `_K5_PY_HEADERS=""` there): no
/// conversation headers, Codex or not.
#[test]
fn none_to_a_server_that_is_not_the_daemon() {
    let rig = Rig::new("connect", "codexacct");
    let account = |r: &Rig| {
        owner_and_port(r);
        std::fs::write(r.k2home.join("connect-account.json"), ACCOUNT).unwrap();
        let _ = std::fs::remove_file(r.k2home.join("daemon.port"));
    };
    let c = case(
        "connect logout to the account server",
        &["logout"],
        &[],
        vec![ok("{}")],
        account,
        0,
        1,
    );
    let env = codex(&[("K2_CONNECT_DB_URL", "http://127.0.0.1:{port}")]);
    let c = with(env, c);
    rig.prepare(Side::Rust, &c);
    let out = rig.run(false, &c);
    assert_eq!(out.code, Some(0), "{out:?}");
    let log = rig.stub_of(Side::Rust).take_log();
    assert_eq!(log.len(), 1, "{log:?}");
    let text = String::from_utf8_lossy(&log[0].bytes);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    assert!(
        head.starts_with("POST /auth/v1/logout HTTP/1.1\r\n"),
        "{head}"
    );
    assert!(
        head.contains("\r\nAuthorization: Bearer a.b.c\r\n"),
        "{head}"
    );
    for name in ["X-K2-Conversation", "X-K2-Thread", "X-K5-"] {
        assert!(
            !head.contains(name),
            "{name} went to the account server: {head}"
        );
    }
}
