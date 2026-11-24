//! `k5-cli talk` against the shell CLI's `cli/k2 talk`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The words: no workspace, an unknown flag, `--token-stdin` beside
//!   `--token` or with nothing on stdin, `--answer` with `--respond`, an
//!   empty `--answers`, no text.
//! - `--read-only`: the read and the judging notes; a 300 KiB screen.
//! - The send: the read, the classify, then `msg`'s send (`--via talk`,
//!   `--wake` or `--no-wake`; a new send key, masked); a human-in-the-loop
//!   verdict holds the message and scouts the form; `--session` writes
//!   straight to the session and reads it back.
//! - `--scout` by the tab bar and by the screen settling; `--answers`
//!   with the total warning; `--answer` and `--respond`; the screen read's
//!   raw second request; no live session.
//! - `<workspace>@http://…` with the token from stdin and from the
//!   environment; no token for a host.
//! - No answer: on the read's pipe (SIGPIPE), on a write (exit 1).
//! - Help never acts: `-h` and `--help` anywhere send nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "talk";
const SID: &str = "0123abcd-0000-4000-8000-00000000beef";
const ON: &str = r#"{"version":"0","features":["send-once"]}"#;
const SENT: &str = r#"{"success":true,"target_session_id":"s1","attempts":1}"#;
const SCREEN: &str = r#"{"lines":["$ build","ok",""]}"#;
const CLEAR: &str = r#"{"is_hitl":false,"source":"fastpath"}"#;
const HITL: &str = r#"{"is_hitl":true,"source":"fastpath"}"#;
const FORM: &str = r#"{"lines":["←  ☐ Colour  ☒ Size  ✔ Submit  →","","Which colour?","❯ 1. Red","  2. Blue","3) Green","Question 1 of 3"]}"#;
const SESSIONS: &str = r#"[{"sessionId":"s-old","isV2":false},{"sessionId":"s-ada","isV2":true,"agentName":"ada"},{"sessionId":"s-bob","isV2":true,"agentName":"bob"}]"#;

fn owner(c: Case) -> Case {
    Case {
        env: vec![
            ("K2_PORT", PORT.to_string()),
            ("K2_TALK_NAV_SETTLE", "0".into()),
            ("K2_TALK_ANSWER_SETTLE", "0".into()),
        ],
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

fn stdin(c: Case, input: &'static [u8]) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn talk_words_as_the_shell_reads_them() {
    let rig = Rig::new(VERB, "words");
    let v = vec![
        // covers: talk
        plain("no workspace", &[], vec![], 1, 0),
        plain("no workspace, flags only", &["--read-only"], vec![], 1, 0),
        plain("an unknown flag", &["ws", "hi", "--bogus"], vec![], 1, 0),
        plain(
            "--token-stdin and --token",
            &["ws@box.example", "hi", "--token", "t", "--token-stdin"],
            vec![],
            2,
            0,
        ),
        stdin(
            plain(
                "--token-stdin with nothing on stdin",
                &["ws@box.example", "hi", "--token-stdin"],
                vec![],
                2,
                0,
            ),
            b"",
        ),
        plain(
            "--answer and --respond",
            &["ws", "--answer", "1", "--respond", "y"],
            vec![],
            1,
            0,
        ),
        plain("no text", &["ws"], vec![], 1, 0),
        plain(
            "no token for another server",
            &["ws@box.example", "hi"],
            vec![],
            1,
            0,
        ),
        plain(
            "an empty --answers list",
            &["ws", "--answers", " , ,", "--session", SID],
            vec![],
            1,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn talk_reads_and_holds() {
    let rig = Rig::new(VERB, "read");
    let big = format!(
        r#"{{"lines":[{}]}}"#,
        (0..6000)
            .map(|i| format!("\"line {i} {}\"", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join(",")
    );
    let v = vec![
        plain(
            "--read-only",
            &["ws", "--read-only", "--lines", "3", "--agent", "ada"],
            vec![ok(SCREEN)],
            0,
            1,
        ),
        plain(
            "--peek with a held message",
            &["ws", "later", "--peek"],
            vec![ok(SCREEN)],
            0,
            1,
        ),
        plain(
            "--dry-run on a session",
            &["ws", "--dry-run", "--session", SID],
            vec![ok(SCREEN)],
            0,
            1,
        ),
        plain(
            "the read refused",
            &["ws", "--read-only"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"gated","hint":"turn it on"}}"#.into(),
            )],
            0,
            1,
        ),
        plain(
            "a 300 KiB screen",
            &["ws", "--read-only"],
            vec![ok(leak(big))],
            0,
            1,
        ),
        plain(
            "a human-in-the-loop prompt holds the message and scouts",
            &["ws", "hello", "--agent", "ada"],
            vec![
                ok(SCREEN),
                ok(HITL),
                ok(r#"{"path":"/w/ws"}"#),
                ok(SESSIONS),
                ok(FORM),
                ok(FORM),
                ok("{}"),
                ok(FORM),
                ok("{}"),
            ],
            0,
            9,
        ),
        plain(
            "a hold whose session is asleep",
            &["ws", "hello"],
            vec![ok(SCREEN), ok(HITL), ok("{}"), ok("[]")],
            0,
            4,
        ),
        plain(
            "--session writes and reads back",
            &["ws", "hello", "--session", SID],
            vec![ok(SCREEN), ok(CLEAR), ok("{}"), ok(SCREEN)],
            0,
            4,
        ),
        // The token from stdin reaches the other server, and the requests
        // go there.
        stdin(
            plain(
                "another server, token from stdin",
                &["ws@http://127.0.0.1:{port}", "--read-only", "--token-stdin"],
                vec![ok(SCREEN)],
                0,
                1,
            ),
            b"remote-tok-1\n",
        ),
        env(
            plain(
                "another server, token from the environment",
                &[
                    "ws@http://127.0.0.1:{port}/",
                    "--answer",
                    "2",
                    "--session",
                    SID,
                ],
                vec![ok("{}"), ok(SCREEN)],
                0,
                2,
            ),
            &[("K2_REMOTE_TOKEN", "remote-tok-2")],
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn talk_drives_forms() {
    let rig = Rig::new(VERB, "drive");
    let v = vec![
        plain(
            "--scout by the tab bar",
            &["ws", "--scout", "--session", SID],
            vec![ok(FORM), ok(FORM), ok("{}"), ok(FORM), ok("{}")],
            0,
            5,
        ),
        plain(
            "--scout until the screen settles",
            &["ws", "--scout", "--agent", "bob"],
            vec![
                ok(r#"{"path":7}"#),
                ok(SESSIONS),
                ok(SCREEN),
                ok(SCREEN),
                ok("{}"),
                ok(SCREEN),
                ok("{}"),
            ],
            0,
            7,
        ),
        env(
            plain(
                "--scout capped",
                &["ws", "--scout", "--session", SID],
                vec![
                    ok(r#"{"lines":[]}"#),
                    ok(r#"{"lines":[]}"#),
                    ok("{}"),
                    ok(r#"{"lines":[]}"#),
                    ok("{}"),
                ],
                0,
                5,
            ),
            &[("K5_TALK_SCOUT_MAX", "2")],
        ),
        env(
            plain(
                "--answers with a total that differs",
                &["ws", "--answers", "1, 2,,3,", "--session", SID],
                vec![
                    ok(r#"{"lines":["Question 1 of 2"]}"#),
                    ok("{}"),
                    ok("{}"),
                    ok("{}"),
                    ok("{}"),
                    ok(SCREEN),
                ],
                0,
                6,
            ),
            &[("K2_TALK_SUBMIT_KEY", "9")],
        ),
        plain(
            "--answer",
            &["ws", "--answer", "2", "--session", SID],
            vec![ok("{}"), ok(SCREEN)],
            0,
            2,
        ),
        plain(
            "--respond yes, the raw second read",
            &["ws", "--respond", "yes", "--session", SID],
            vec![ok("{}"), ok("not json"), ok("raw answer\n")],
            0,
            3,
        ),
        plain(
            "no live session",
            &["ws", "--answer", "1", "--agent", "zed"],
            vec![ok(r#"{"path":"/w/ws"}"#), ok("[]")],
            1,
            2,
        ),
        plain(
            "a write refused ends with the closing line",
            &["ws", "--respond", "n", "--session", SID],
            vec![Reply::Answer(403, PERMISSION_ASK.into()), ok(SCREEN)],
            0,
            2,
        ),
        plain(
            "a write with no answer",
            &["ws", "--answer", "1", "--session", SID],
            vec![Reply::Close],
            1,
            1,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn talk_read_with_no_answer_ends_in_the_pipe() {
    let rig = Rig::new(VERB, "pipe");
    rig.check(&Case {
        compare: Compare::Waited,
        ..plain(
            "no answer to the read",
            &["ws", "--read-only"],
            vec![Reply::Close],
            141,
            1,
        )
    });
}

#[test]
fn talk_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for (name, args) in [
        ("--help", &["--help"][..]),
        ("-h after the text", &["ws", "hi", "-h"]),
        ("--help with --answer", &["ws", "--answer", "1", "--help"]),
        ("-h with --scout", &["ws", "--scout", "-h"]),
        (
            "--help with --session",
            &["ws", "hi", "--session", SID, "--help"],
        ),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..plain(name, args, vec![], 0, 0)
        });
    }
}

/// A case whose send goes with a new key: the key masked on both sides.
fn masked(rig: &Rig, c: &Case) {
    let name = c.name;
    let mask = |text: &str, port: u16| -> String {
        let mut t = text.replace(&port.to_string(), "{port}");
        while let Some(at) = t.find("idempotency_key=") {
            let start = at + "idempotency_key=".len();
            let end = t[start..]
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
                .map_or(t.len(), |e| start + e);
            let key = t[start..end].to_string();
            assert!(
                key.len() == 36 && key.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
                "{name}: a new key is a UUID: {t}"
            );
            t.replace_range(at..end, "idem=<key>");
        }
        t
    };
    rig.prepare(Side::Bash, c);
    let b = rig.run(true, c);
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
                &normalise(r, false, VERB, name),
                rig.stub_of(Side::Bash).port,
            )
        })
        .collect();
    assert_eq!(b.code, Some(c.code), "{name}: bash's exit: {b:?}");
    assert_eq!(bl.len(), c.requests, "{name}: bash's requests: {bl:#?}");
    for without in [false, true] {
        let side = if without { Side::NoPython3 } else { Side::Rust };
        let (r, rl) = rig.binary(c, without);
        assert_eq!(rig.stub_of(side).left(), 0, "{name}: left answers: {r:?}");
        let port = rig.stub_of(side).port;
        let rl: Vec<String> = rl
            .iter()
            .map(|l| mask(&l.replace("{port}", &port.to_string()), port))
            .collect();
        assert_eq!(rl, bl, "{name} ({side:?}): the requests");
        assert_eq!(
            (r.code, r.signal),
            (b.code, b.signal),
            "{name} ({side:?}): the exit\n{r:?}\n{b:?}"
        );
        assert_eq!(r.stdout, b.stdout, "{name} ({side:?}): stdout");
        assert_eq!(r.stderr, b.stderr, "{name} ({side:?}): stderr");
    }
}

#[test]
fn talk_delivers_through_msg() {
    let rig = Rig::new(VERB, "send");
    for c in [
        plain(
            "read, classify, send",
            &[
                "ws",
                "hello",
                "--from",
                "me",
                "--command",
                "/x",
                "--wake-timeout",
                "9",
            ],
            vec![ok(SCREEN), ok(CLEAR), ok(ON), ok(SENT)],
            0,
            4,
        ),
        plain(
            "--no-wake, a classify that is not JSON",
            &["ws", "hello", "--from", "me", "--no-wake"],
            vec![ok(SCREEN), ok("nope"), ok(ON), ok(SENT)],
            0,
            4,
        ),
        plain(
            "the send not delivered",
            &["ws", "hello", "--from", "me", "--lines", "5"],
            vec![
                ok(SCREEN),
                ok(r#"{"is_hitl":null,"source":"unavailable"}"#),
                ok(ON),
                ok(r#"{"success":false,"reason":"no_agent","hint":"nobody home"}"#),
            ],
            2,
            4,
        ),
        stdin(
            plain(
                "another server: the send goes there too",
                &[
                    "ws@http://127.0.0.1:{port}",
                    "hi",
                    "--from",
                    "me",
                    "--token-stdin",
                ],
                vec![ok(SCREEN), ok(CLEAR), ok(ON), ok(SENT)],
                0,
                4,
            ),
            b"remote-tok-3\n",
        ),
    ] {
        masked(&rig, &c);
    }
}

/// After a bare `--` the words are the workspace and the text, as typed
/// (`cmd_talk`'s `--)` arm), and `talk` hands msg the text after its own
/// `--`: a text that starts with a dash or is `--help` is sent.
#[test]
fn talk_after_dashes_sends_the_text_as_typed() {
    let rig = Rig::new(VERB, "dashes");
    // covers: talk
    let c = plain(
        "a text that starts with a dash",
        &["--from", "me", "ws", "--", "-x --json"],
        vec![ok(SCREEN), ok(CLEAR), ok(ON), ok(SENT)],
        0,
        4,
    );
    masked(&rig, &c);
    // A help word after `--` is the text: the front's help scan stops at
    // `--`, so the family sends it.
    for c in [
        plain(
            "the text --help",
            &["--from", "me", "--no-wake", "--", "ws", "--help"],
            vec![ok(SCREEN), ok(CLEAR), ok(ON), ok(SENT)],
            0,
            4,
        ),
        plain(
            "the text -h, a flag-looking word before it",
            &["--from", "me", "--", "ws", "--read-only", "-h"],
            vec![ok(SCREEN), ok(CLEAR), ok(ON), ok(SENT)],
            0,
            4,
        ),
    ] {
        masked(&rig, &c);
    }
    for c in [
        plain("-- alone", &["--"], vec![], 1, 0),
        plain("help before --", &["ws", "-h", "--", "hi"], vec![], 0, 0),
        plain(
            "--read-only, then --help as the text",
            &["ws", "--read-only", "--", "--help"],
            vec![ok(SCREEN)],
            0,
            1,
        ),
    ] {
        rig.check(&as_the_front_routes(VERB, c));
    }
}

/// A value flag as the last word: bash's `_need_value` and the binary
/// print the same line, "Error: <flag> takes a value", and exit 2.
/// Neither sends anything.
#[test]
fn talk_value_flag_last_is_the_d6_line() {
    let rig = Rig::new(VERB, "d6");
    for flag in [
        "--from",
        "--command",
        "--lines",
        "--agent",
        "--session",
        "--token",
        "--answer",
        "--respond",
        "--answers",
        "--wake-timeout",
    ] {
        let args: &'static [&'static str] = Box::leak(vec!["ws", "hi", flag].into_boxed_slice());
        let c = plain(leak(format!("{flag} last")), args, vec![], 2, 0);
        let name = c.name;
        rig.prepare(Side::Bash, &c);
        let b = rig.run(true, &c);
        assert_eq!(b.code, Some(2), "{name}: bash's exit: {b:?}");
        assert!(b.stdout.is_empty(), "{name}: bash's stdout: {b:?}");
        assert_eq!(
            b.stderr,
            format!("Error: {flag} takes a value\n"),
            "{name}: bash's stderr: {b:?}"
        );
        assert!(
            rig.read(Side::Bash, false, name).is_empty(),
            "{name}: bash sent"
        );
        let (r, sent) = rig.binary(&c, false);
        assert!(sent.is_empty(), "{name}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{name}: {r:?}");
        assert!(r.stdout.is_empty(), "{name}: {r:?}");
        assert_eq!(r.stderr, format!("Error: {flag} takes a value\n"), "{name}");
    }
}

/// Two forms the binary handed to the shell until bash's fixes reached
/// this line, now answered by both alike:
/// - an `--agent` reaches the session pick's Python as data (argv), so a
///   backslash or quotes in it pick the session of the agent named
///   exactly that, past a near miss;
/// - a `*_TALK_SCOUT_MAX` that is not plain digits is the default cap,
///   20, never arithmetic: with `1+1` the walk goes past two questions.
#[test]
fn talk_agent_and_scout_cap_as_fixed_in_bash() {
    let rig = Rig::new(VERB, "fixed");
    let lines = |l: &str| ok(&format!(r#"{{"lines":["{l}"]}}"#));
    for c in [
        plain(
            "an --agent with a backslash and quotes picks its own session",
            &["ws", "--answer", "1", "--agent", "a\\b'''\""],
            vec![
                ok(r#"{"path":"/w/ws"}"#),
                ok(
                    r#"[{"sessionId":"s-near","isV2":true,"agentName":"a\\\\b"},{"sessionId":"s-it","isV2":true,"agentName":"a\\b'''\""}]"#,
                ),
                ok("{}"),
                ok(SCREEN),
            ],
            0,
            4,
        ),
        plain(
            "an --agent with a backslash and no live session",
            &["ws", "--answer", "1", "--agent", "a\\b"],
            vec![ok("{}"), ok("[]")],
            1,
            2,
        ),
        env(
            plain(
                "a scout cap that is not digits walks to the default cap",
                &["ws", "--scout", "--session", SID],
                vec![
                    ok(r#"{"lines":[]}"#),
                    lines("one"),
                    ok("{}"),
                    lines("two"),
                    ok("{}"),
                    lines("two"),
                    ok("{}"),
                    ok("{}"),
                ],
                0,
                8,
            ),
            &[("K2_TALK_SCOUT_MAX", "1+1")],
        ),
    ] {
        rig.check(&c);
    }
}

fn words(w: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(w.into_boxed_slice())
}

/// `talk ws@https://<host>` through the person's proxy (the shared
/// transport's rule): Basic in the CONNECT, the host's token only
/// inside the tunnel, the same screen on both sides.
#[test]
fn talk_at_a_remote_host_through_the_proxy() {
    use common::tunnel::{self, ProxyMode, ProxyStub, TlsStub, HOST};
    let rig = Rig::new(VERB, "proxy");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let remote = TlsStub::start(&pki, 200, SCREEN);
    let proxy = ProxyStub::start(ProxyMode::Tunnel);
    let target = format!("{HOST}:{}", remote.port);
    let addr = leak(format!("ws@https://{target}"));
    let authed = proxy.url(Some(("acme-proxy", "pr0xy-s3cret")));
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let c = env(
        plain(
            "ws@https://host --read-only through https_proxy",
            words(vec![addr, "--read-only"]),
            vec![],
            0,
            0,
        ),
        &[
            ("K2_REMOTE_TOKEN", "remote-tok-9"),
            ("https_proxy", authed.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &remote);
    let basic = tunnel::basic_line("acme-proxy", "pr0xy-s3cret");
    for run in [&r, &b] {
        tunnel::assert_tunnel_only(c.name, run, &target, Some(&basic), "remote-tok-9");
        assert!(
            run.daemon.is_empty(),
            "never this machine's daemon: {run:?}"
        );
    }
    tunnel::assert_same(c.name, &b, &r);
    assert_eq!(r.out.code, Some(0), "{r:?}");
    assert!(r.out.stdout.contains("$ build"), "{r:?}");
    tunnel::assert_unprinted(c.name, &[b, r], &["pr0xy-s3cret", "remote-tok-9"]);
}

/// `talk ws@https://<host>` through an `https://` proxy with a user part:
/// TLS to the proxy (its certificate checked, `localhost` in the
/// throwaway CA's certificate), the CONNECT and its Basic inside that
/// TLS, the host's token only inside the tunnel's own TLS, the same
/// screen on both sides. It used to be the shell's alone.
#[test]
fn talk_through_an_https_proxy() {
    use common::tunnel::{self, ProxyMode, ProxyStub, TlsStub, HOST};
    let rig = Rig::new(VERB, "proxytls");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let remote = TlsStub::start(&pki, 200, SCREEN);
    let proxy = ProxyStub::start_tls(&pki, ProxyMode::Tunnel);
    let target = format!("{HOST}:{}", remote.port);
    let addr = leak(format!("ws@https://{target}"));
    let url = format!("https://acme-proxy:pr0xy-s3cret@localhost:{}", proxy.port);
    let ca = pki.ca_file.to_string_lossy().into_owned();
    let c = env(
        plain(
            "ws@https://host --read-only through an https:// proxy",
            words(vec![addr, "--read-only"]),
            vec![],
            0,
            0,
        ),
        &[
            ("K2_REMOTE_TOKEN", "remote-tok-9"),
            ("https_proxy", url.as_str()),
            ("SSL_CERT_FILE", ca.as_str()),
        ],
    );
    let [b, r] = tunnel::both(&rig, &c, &proxy, &remote);
    let basic = tunnel::basic_line("acme-proxy", "pr0xy-s3cret");
    for run in [&r, &b] {
        tunnel::assert_tunnel_only(c.name, run, &target, Some(&basic), "remote-tok-9");
        assert!(
            run.daemon.is_empty(),
            "never this machine's daemon: {run:?}"
        );
    }
    tunnel::assert_same(c.name, &b, &r);
    assert_eq!(r.out.code, Some(0), "{r:?}");
    assert!(r.out.stdout.contains("$ build"), "{r:?}");
    tunnel::assert_unprinted(c.name, &[b, r], &["pr0xy-s3cret", "remote-tok-9"]);
}

/// `talk ws@https://<host>` through a SOCKS5 proxy: `socks5h://` with no
/// authentication and with a user and password (RFC 1929), both sides
/// against the same recording stub, the same handshake (curl's GSS-API
/// method left out: the named difference curl-socks5-gssapi), the same
/// requests inside the tunnel and the same screen; `socks5://` resolves
/// the target's name here, and a name that doesn't resolve fails the same
/// way on both, before any proxy is reached. A proxy scheme curl refuses
/// (`ftp://`) fails the same on both, and nothing is sent anywhere.
#[test]
fn talk_through_a_socks5_proxy() {
    use common::tunnel::{self, SocksStub, TlsStub, HOST};
    let rig = Rig::new(VERB, "socks");
    let pki = tunnel::Pki::new(&rig.dir.join("pki"));
    let remote = TlsStub::start(&pki, 200, SCREEN);
    let open = SocksStub::start(None);
    let locked = SocksStub::start(Some(("acme", "pw")));
    let target = format!("{HOST}:{}", remote.port);
    let addr = leak(format!("ws@https://{target}"));
    let ca = pki.ca_file.to_string_lossy().into_owned();
    for (name, stub, url, line) in [
        (
            "through socks5h://",
            &open,
            open.url("socks5h", None),
            Some(format!("socks5 methods [0] connect {target}")),
        ),
        (
            "through socks5h:// with a user and password",
            &locked,
            locked.url("socks5h", Some(("acme", "pw"))),
            Some(format!(
                "socks5 methods [0, 2] user \"acme\" pass \"pw\" connect {target}"
            )),
        ),
        (
            "through socks5:// (a name that doesn't resolve here)",
            &open,
            open.url("socks5", None),
            None,
        ),
        (
            "through ftp:// (a scheme curl refuses)",
            &open,
            format!("ftp://127.0.0.1:{}", open.port),
            None,
        ),
    ] {
        let name = leak(format!("ws@https://host --read-only {name}"));
        let c = env(
            plain(name, words(vec![addr, "--read-only"]), vec![], 0, 0),
            &[
                ("K2_REMOTE_TOKEN", "remote-tok-9"),
                ("https_proxy", url.as_str()),
                ("SSL_CERT_FILE", ca.as_str()),
            ],
        );
        let [b, r] = tunnel::both_logged(&rig, &c, &stub.log, &remote);
        assert_eq!(
            (b.out.code, &b.out.stdout, &b.out.stderr),
            (r.out.code, &r.out.stdout, &r.out.stderr),
            "{name}: bash, then the binary"
        );
        assert!(
            b.daemon.is_empty() && r.daemon.is_empty(),
            "{name}: {b:?} {r:?}"
        );
        assert_eq!(b.proxy, r.proxy, "{name}: the SOCKS handshakes");
        let only = |t: &[(String, bool)]| t.iter().map(|(q, _)| q.clone()).collect::<Vec<_>>();
        assert_eq!(
            only(&b.tls),
            only(&r.tls),
            "{name}: the requests in the tunnel"
        );
        assert!(
            r.tls.iter().all(|(_, native)| *native),
            "{name}: the binary's own"
        );
        match &line {
            Some(l) => {
                assert_eq!(r.proxy, [l.clone()], "{name}");
                assert_eq!(r.out.code, Some(0), "{name}: {r:?}");
                assert!(r.out.stdout.contains("$ build"), "{name}: {r:?}");
                for (q, _) in &r.tls {
                    assert!(
                        q.contains("Authorization: Bearer remote-tok-9"),
                        "{name}: {q}"
                    );
                }
            }
            None => {
                assert!(
                    r.proxy.is_empty(),
                    "{name}: nothing reached the proxy: {r:?}"
                );
                assert!(r.tls.is_empty(), "{name}: nothing sent: {r:?}");
                assert_ne!(r.out.code, Some(0), "{name}: {r:?}");
            }
        }
        for run in [&b, &r] {
            assert!(
                run.proxy.iter().all(|l| !l.contains("remote-tok-9")),
                "{name}: the token reached the proxy: {run:?}"
            );
        }
        tunnel::assert_unprinted(name, &[b, r], &["remote-tok-9"]);
    }
}

/// K2's shared transport rule: a token goes to another machine over
/// https only, and "this computer" is judged by the URL as written (a
/// 127.0.0.0/8 or ::1 literal, or exactly `localhost`), never a lookup.
/// `ws@http://<anything else>` is one line, exit 2, with nothing sent;
/// `x.localhost` and a name that only starts with `127.` are refused
/// too. `localhost` goes. The refusal rows' bash side flips by itself
/// once cli/k2 carries the fix (pending-bash-fix until then).
#[test]
fn talk_refuses_plain_http_to_another_machine() {
    let rig = Rig::new(VERB, "plain");
    // Allowed: the name `localhost` as written, to this side's stub.
    rig.check(&stdin(
        plain(
            "ws@http://localhost, token from stdin",
            &["ws@http://localhost:{port}", "--read-only", "--token-stdin"],
            vec![ok(SCREEN)],
            0,
            1,
        ),
        b"remote-tok-1\n",
    ));
    let fixed =
        common::tunnel::bash_has("Error: plain http:// only to this computer; use https://");
    for (name, addr) in [
        (
            "ws@http://another machine",
            "ws@http://cp.example.invalid:9",
        ),
        ("ws@http://x.localhost", "ws@http://x.localhost:9"),
        ("ws@http://127.<a name>", "ws@http://127.example.invalid:9"),
    ] {
        let url = addr.strip_prefix("ws@").unwrap();
        let line = format!("Error: plain http:// only to this computer; use https:// (got: {url})");
        let c = env(
            plain(name, words(vec![addr, "hi"]), vec![], 2, 0),
            &[("K2_REMOTE_TOKEN", "remote-tok-9")],
        );
        if fixed {
            rig.check(&c);
            continue;
        }
        println!("PENDING bash fix: {name}: the binary's side alone");
        rig.prepare(Side::Rust, &c);
        let out = rig.run(false, &c);
        assert_eq!(
            (out.code, out.stdout.as_str(), out.stderr.as_str()),
            (Some(2), "", format!("{line}\n").as_str()),
            "{name}"
        );
        assert_eq!(rig.read(Side::Rust, true, name).len(), 0, "{name}: sent");
        rig.prepare(Side::Bash, &c);
        let out = rig.run(true, &c);
        assert!(
            !out.stderr.contains(&line),
            "{name}: bash half-fixed: {out:?}"
        );
        rig.read(Side::Bash, false, name);
    }
}
