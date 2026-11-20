//! `k5-cli msg` against the shell CLI's `cli/k2 msg`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - The live send: the send-once probe, the sender lookups (handle,
//!   display name, folder; a refusal there never closes the command; no
//!   answer there ends it by SIGPIPE; a failure ends it with curl's code,
//!   76 with TMPDIR unusable), the form, the answer and its notes, exit 0,
//!   2 or 3, the closing line, the bulletin.
//! - Sent once: an unanswered send (an empty 200, 52, 56, 409
//!   `in_progress`) asked again with the same key, then the line that it
//!   may have been delivered; a daemon without send-once gets one try.
//!   On the cell socket, a POST goes to TCP only after a refused connect
//!   or "not served"; a re-minted passport.
//! - The words: every usage stop, `--session`, the wire form, a bad key,
//!   the summary hint, a 100 KiB text.
//! - `msg who` and `msg verify`: their words, readers, refusals and no
//!   answer.
//!
//! Most cases name their key (`--idempotency-key`) so the requests match
//! byte for byte; [`masked`] runs the ones with a new key, or with seconds
//! in a line, with those masked.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "msg";
const KEY: &str = "parity-key-0001";
const ON: &str = r#"{"version":"0","features":["send-once"]}"#;
const OFF: &str = r#"{"version":"0","features":[]}"#;
const SENT: &str = r#"{"success":true,"target_session_id":"s1","attempts":1}"#;
const NOT_SENT: &str = r#"{"success":false,"reason":"no_agent","hint":"nobody home"}"#;
const POSTAL_SENT: &str =
    r#"{"status":"delivered","to":"a::abc.postal.bot","badge":"B","deliveryId":"m_1"}"#;

fn owner(c: Case) -> Case {
    Case {
        env: vec![("K2_PORT", PORT.to_string())],
        setup: owner_only,
        ..c
    }
}

fn session(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: nothing,
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

fn leak(words: Vec<String>) -> &'static [&'static str] {
    let v: Vec<&'static str> = words
        .into_iter()
        .map(|w| &*Box::leak(w.into_boxed_str()))
        .collect();
    Box::leak(v.into_boxed_slice())
}

#[test]
fn msg_sends_as_the_shell_does() {
    let rig = Rig::new(VERB, "send");
    let big = "word ".repeat(20 * 1024);
    let long = "x".repeat(700);
    let v = vec![
        // covers: msg
        plain(
            "a live send",
            &["ws", "hello", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), ok(SENT)],
            0,
            2,
        ),
        plain(
            "every flag",
            &[
                "ws",
                "a",
                "b",
                "--no-wake",
                "--from",
                "me",
                "--command",
                "/x",
                "--wake-timeout",
                "9",
                "--via",
                "talk",
                "--summary",
                "s",
                "--wake",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(SENT)],
            0,
            2,
        ),
        plain(
            "not delivered",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), ok(NOT_SENT)],
            2,
            2,
        ),
        plain(
            "gated",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"gated","hint":"turn it on"}}"#.into(),
                ),
            ],
            3,
            2,
        ),
        plain(
            "a refusal's closing line",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Answer(403, PERMISSION_ASK.into())],
            2,
            2,
        ),
        plain(
            "the rename and replay notes",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                ok(r#"{"success":true,"movedFrom":"old","addr":"new name","replayed":true}"#),
            ],
            0,
            2,
        ),
        plain(
            "the woke notes stay quiet",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                ok(r#"{"success":true,"woke":true,"wake_ms":1234,"revived":true,"addr":"a"}"#),
            ],
            0,
            2,
        ),
        plain(
            "an answer echo takes as a flag",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), ok("-n")],
            2,
            2,
        ),
        plain(
            "not JSON",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), ok("plain text\n\n")],
            2,
            2,
        ),
        plain(
            "the bulletin",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                ok(
                    r#"{"success":true,"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
                ),
            ],
            0,
            2,
        ),
        // The sender.
        plain(
            "the folder's handle",
            &["ws", "hi", "--idempotency-key", KEY],
            vec![ok(ON), ok(r#"{"handle":" acme "}"#), ok(SENT)],
            0,
            3,
        ),
        plain(
            "the display name, then the folder",
            &["ws", "hi", "--idempotency-key", KEY],
            vec![
                ok(ON),
                ok("{}"),
                ok(r#"{"handle":5}"#),
                ok(r#"{"path":"/a/b/proj/"}"#),
                ok(SENT),
            ],
            0,
            5,
        ),
        plain(
            "external, and a lookup's refusal is quiet",
            &["ws", "hi", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(403, PERMISSION_ASK.into()),
                ok("[]"),
                ok("{}"),
                ok(NOT_SENT),
            ],
            2,
            5,
        ),
        plain(
            "a lookup with no answer",
            &["ws", "hi", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Close],
            141,
            2,
        ),
        plain(
            "a lookup not HTTP",
            &["ws", "hi", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Raw(GARBAGE.to_vec())],
            1,
            2,
        ),
        Case {
            no_tmpdir: true,
            ..plain(
                "a lookup reset with TMPDIR unusable",
                &["ws", "hi", "--idempotency-key", KEY],
                vec![ok(ON), Reply::Reset],
                76,
                2,
            )
        },
        Case {
            env: vec![
                ("K2_PORT", PORT.to_string()),
                ("K2_CELL", "sidecar".to_string()),
                ("K2_PRIMARY", "lead".to_string()),
                ("K5_SIDECAR_NAME", "sales".to_string()),
            ],
            ..plain(
                "a sidecar's sender",
                &["ws", "hi", "--idempotency-key", KEY],
                vec![ok(ON), ok(SENT)],
                0,
                2,
            )
        },
        Case {
            env: vec![
                ("K2_PORT", PORT.to_string()),
                ("K2_PROJECT_PATH", "/else/where".to_string()),
            ],
            ..plain(
                "PROJECT_PATH names the sender",
                &["ws", "hi", "--idempotency-key", KEY],
                vec![ok(ON), ok(r#"{"handle":"w"}"#), ok(SENT)],
                0,
                3,
            )
        },
        // Postal mail.
        plain(
            "Postal mail",
            &["--no-wake", "a::abc.postal.bot", "hi", "there"],
            vec![ok(
                r#"{"status":"delivered","to":"a::abc.postal.bot","badge":"B","deliveryId":"m_1"}"#,
            )],
            0,
            1,
        ),
        plain(
            "Postal mail, --json",
            &["a::abc.postal.bot", "hi", "--json"],
            vec![ok(r#"{"status":"held"}"#)],
            0,
            1,
        ),
        plain(
            "Postal mail, failed",
            &["a::abc.postal.bot", "hi"],
            vec![ok(r#"{"status":"failed","to":"x","reason":"gone"}"#)],
            1,
            1,
        ),
        plain(
            "Postal mail, not friends",
            &["a::abc.postal.bot", "hi"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"postal_not_friends","hint":"ask first"}}"#.into(),
            )],
            3,
            1,
        ),
        plain(
            "Postal mail, asked",
            &["a::abc.postal.bot", "hi"],
            vec![ok(r#"{"asked":true,"card":"c1"}"#)],
            2,
            1,
        ),
        plain("Postal mail, no text", &["a::abc.postal.bot"], vec![], 2, 0),
        plain(
            "Postal mail, a K2 flag",
            &["a::abc.postal.bot", "hi", "--summary", "s"],
            vec![],
            2,
            0,
        ),
        plain(
            "Postal mail, not HTTP",
            &["a::abc.postal.bot", "hi"],
            vec![Reply::Raw(GARBAGE.to_vec())],
            1,
            1,
        ),
        // The words.
        plain(
            "--session",
            &[
                "--session",
                "s1",
                "hi",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(SENT)],
            0,
            2,
        ),
        plain(
            "the wire form",
            &["a::b::c", "t", "--idempotency-key", KEY],
            vec![ok(ON)],
            2,
            1,
        ),
        plain(
            "a key it won't take",
            &["ws", "t", "--idempotency-key", "short"],
            vec![],
            2,
            0,
        ),
        plain("no address", &[], vec![], 1, 0),
        plain("no text", &["ws"], vec![], 1, 0),
        plain("--agent", &["--agent", "x", "ws", "t"], vec![], 1, 0),
        plain("an unknown flag", &["ws", "t", "--bogus"], vec![], 1, 0),
        plain("a bare --inbox", &["ws", "--inbox", "f"], vec![], 2, 0),
        plain(
            "both tray modes",
            &["ws", "--inbox-silent", "--inbox-wake", "f"],
            vec![],
            2,
            0,
        ),
        plain(
            "--session and two words",
            &["--session", "s", "a", "b"],
            vec![],
            2,
            0,
        ),
        plain(
            "--idempotency-key alone",
            &["ws", "t", "--idempotency-key"],
            vec![],
            2,
            0,
        ),
        plain("--summary alone", &["ws", "t", "--summary"], vec![], 2, 0),
        plain("--secret empty", &["ws", "t", "--secret", ""], vec![], 2, 0),
        plain(
            "--secret with --session",
            leak(vec![
                "--session".into(),
                "s".into(),
                long.clone(),
                "--secret".into(),
                "N".into(),
            ]),
            vec![],
            2,
            0,
        ),
        plain(
            "the summary hint",
            leak(vec![
                "ws".into(),
                long,
                "--from".into(),
                "me".into(),
                "--idempotency-key".into(),
                KEY.into(),
            ]),
            vec![ok(ON), ok(SENT)],
            0,
            2,
        ),
        plain(
            "a 100 KiB text",
            leak(vec![
                "ws".into(),
                big,
                "--from".into(),
                "me".into(),
                "--summary".into(),
                "s".into(),
                "--idempotency-key".into(),
                KEY.into(),
            ]),
            vec![ok(ON), ok(SENT)],
            0,
            2,
        ),
        // Sent once.
        plain(
            "an empty answer, again",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), ok(""), ok(SENT)],
            0,
            3,
        ),
        plain(
            "52, again",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Close, ok(SENT)],
            0,
            3,
        ),
        plain(
            "56, again",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Reset, ok(SENT)],
            0,
            3,
        ),
        plain(
            "in_progress, again",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(409, r#"{"error":{"code":"in_progress"}}"#.into()),
                ok(SENT),
            ],
            0,
            3,
        ),
        plain(
            "not HTTP is an answer",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Raw(GARBAGE.to_vec())],
            1,
            2,
        ),
        plain(
            "without send-once, once",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(OFF), ok(SENT)],
            0,
            2,
        ),
        plain(
            "the probe not HTTP",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![Reply::Raw(GARBAGE.to_vec()), ok(SENT)],
            0,
            2,
        ),
        Case {
            env: vec![("K2_PORT", dead_port())],
            ..plain(
                "nothing listening",
                &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
                vec![],
                7,
                0,
            )
        },
        // A session: the cell socket.
        session(case(
            "over the cell socket",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![ok(ON), ok(SENT)],
            nothing,
            0,
            2,
        )),
        session(case(
            "the sender in a session",
            &["ws", "hi", "--idempotency-key", KEY],
            &[],
            vec![ok(ON), ok(r#"{"handle":"me"}"#), ok(SENT)],
            nothing,
            0,
            3,
        )),
        session(case(
            "not served, then TCP",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![
                ok(ON),
                Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into()),
                ok(SENT),
            ],
            nothing,
            0,
            3,
        )),
        session(case(
            "52 on the socket, again there",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![ok(ON), Reply::Close, ok(SENT)],
            nothing,
            0,
            3,
        )),
        session(case(
            "an empty socket answer, again",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![ok(ON), ok("\n"), ok(SENT)],
            nothing,
            0,
            3,
        )),
        session(case(
            "the socket not HTTP",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![ok(ON), Reply::Raw(GARBAGE.to_vec())],
            nothing,
            1,
            2,
        )),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            ..session(case(
                "a dead socket, then TCP",
                &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
                &[],
                vec![ok(ON), ok(SENT)],
                nothing,
                0,
                2,
            ))
        },
        Case {
            env: vec![
                ("K2_PORT", PORT.to_string()),
                ("K2_HOOK_SOCK", "{sock}".to_string()),
            ],
            setup: owner_only,
            ..case(
                "a session with no passport",
                &["ws", "hi"],
                &[],
                vec![],
                nothing,
                3,
                0,
            )
        },
        // Postal mail in a session: the passport, the cell socket first
        // (`cli_send`), then TCP by its rules; a person's step ends with
        // where a person takes it.
        session(case(
            "Postal mail over the cell socket",
            &["a::abc.postal.bot", "hi"],
            &[],
            vec![ok(POSTAL_SENT)],
            nothing,
            0,
            1,
        )),
        session(case(
            "Postal mail: not served, then TCP",
            &["a::abc.postal.bot", "hi"],
            &[],
            vec![
                Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into()),
                ok(POSTAL_SENT),
            ],
            nothing,
            0,
            2,
        )),
        Case {
            env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            ..session(case(
                "Postal mail: no socket, TCP with the passport",
                &["a::abc.postal.bot", "hi"],
                &[],
                vec![ok(POSTAL_SENT)],
                nothing,
                0,
                1,
            ))
        },
        session(case(
            "Postal mail: a person's step in a session",
            &["a::abc.postal.bot", "hi"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"postal_people_only","hint":"a person sends this"}}"#.into(),
            )],
            nothing,
            3,
            1,
        )),
        session(case(
            "Postal mail: a hint that says where",
            &["a::abc.postal.bot", "hi"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"in Settings"}}"#.into(),
            )],
            nothing,
            3,
            1,
        )),
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "",
            },
            ..session(case(
                "Postal mail the cell socket may have run",
                &["a::abc.postal.bot", "hi"],
                &[],
                vec![Reply::Close],
                nothing,
                141,
                1,
            ))
        },
        case("no daemon", &["ws", "hi"], &[], vec![], nothing, 1, 0),
    ];
    for c in &v {
        rig.check(c);
    }
    // Help never acts: -h or --help anywhere is the shell's, and sends
    // nothing. (`--signal … --help` is left out: the shell sends the
    // signal there, a bash bug reported to K2.)
    for (name, args, code) in [
        ("--help", &["--help"][..], 0),
        ("-h after the text", &["ws", "hi", "--no-wake", "-h"], 0),
        (
            "--help with a key",
            &["ws", "hi", "--idempotency-key", KEY, "--help"],
            0,
        ),
        ("who -h", &["who", "m_1", "-h"], 0),
        (
            "verify --help",
            &["verify", "m_1", "--text", "x", "--help"],
            0,
        ),
        (
            "Postal mail --help",
            &["a::abc.postal.bot", "hi", "--help"],
            0,
        ),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..plain(
                name,
                leak(args.iter().map(|w| w.to_string()).collect()),
                vec![],
                code,
                0,
            )
        });
    }
    let healed = session(case(
        "a re-minted passport",
        &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
        &[],
        vec![ok(ON), heal(&rig), ok(SENT)],
        port_only,
        0,
        3,
    ));
    rig.check(&healed);
}

/// The binary's run and bash's, with a new send key and the seconds in a
/// line masked (bash counts whole seconds of `$SECONDS`).
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
            let uuid = key.len() == 36 && key.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
            assert!(uuid || key == KEY, "{name}: a new key is a UUID: {t}");
            let shown = if uuid { "<key>" } else { KEY };
            t.replace_range(at..end, &format!("idem={shown}"));
        }
        // "(3 tries, 2 s)" and "(0 s)".
        let mut out = String::new();
        let mut rest = t.as_str();
        while let Some(at) = rest.find(" s)") {
            let head = &rest[..at];
            let digits = head.len() - head.trim_end_matches(|ch: char| ch.is_ascii_digit()).len();
            out.push_str(&head[..head.len() - digits]);
            out.push_str("<n> s)");
            rest = &rest[at + 3..];
        }
        out.push_str(rest);
        out
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
                &normalise(r, false, "msg", name),
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
        assert_eq!(
            mask(&r.stderr, port),
            mask(&b.stderr, rig.stub_of(Side::Bash).port),
            "{name} ({side:?}): stderr"
        );
    }
}

#[test]
fn msg_with_a_new_key_or_no_answer() {
    let rig = Rig::new(VERB, "once");
    for c in [
        plain(
            "a new key",
            &["ws", "hi", "--from", "me"],
            vec![ok(ON), ok(SENT)],
            0,
            2,
        ),
        plain(
            "a new key, no send-once",
            &["ws", "hi", "--from", "me"],
            vec![ok(OFF), ok(SENT)],
            0,
            2,
        ),
        plain(
            "no answer, no send-once",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(OFF), Reply::Close],
            1,
            2,
        ),
        plain(
            "three tries, no answer",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Close, ok(""), Reply::Reset],
            1,
            4,
        ),
        Case {
            env: vec![
                ("K2_PORT", PORT.to_string()),
                ("_K2_IDEM_TRIES", "1".to_string()),
            ],
            ..plain(
                "one try",
                &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
                vec![ok(ON), Reply::Close],
                1,
                2,
            )
        },
        session(case(
            "three tries on the socket",
            &["ws", "hi", "--from", "me", "--idempotency-key", KEY],
            &[],
            vec![ok(ON), Reply::Close, Reply::Close, Reply::Close],
            nothing,
            1,
            4,
        )),
    ] {
        masked(&rig, &c);
    }
}

const WHO: &str = r#"{"id":"m_1","via":"passport","sender":{"label":"acme"},"actor":"owner","onBehalfOf":{"value":"x@y"},"linked":true,"links":[{"id":"l1","scope":{"a":"é"}}]}"#;

#[test]
fn msg_who_and_verify_as_the_shell_does() {
    let rig = Rig::new(VERB, "who");
    let verified = r#"{"code":"m_1","kind":"msg","sender":{"address":"a@b","handle":"acme","fingerprint":"ff"},"via":"passport","actor":"owner","target":{"workspaceId":"w1"},"at":1700000000,"delivered":true,"badge":"b2","landed":{"x":[1]},"content":"match","contentHint":"ok"}"#;
    let v = vec![
        plain("who", &["who", "m_1"], vec![ok(WHO)], 0, 1),
        plain("who --json", &["who", "--json", "m_1"], vec![ok(WHO)], 0, 1),
        plain("who, a bare answer", &["who", "m_1"], vec![ok("{}")], 0, 1),
        plain(
            "who, not found",
            &["who", "m_x"],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such delivery"}}"#.into(),
            )],
            4,
            1,
        ),
        plain(
            "who, a token hint",
            &["who", "m_x"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"hint":"Invalid token"}}"#.into(),
            )],
            3,
            1,
        ),
        plain("who, not JSON", &["who", "m_1"], vec![ok("oops")], 1, 1),
        plain(
            "who, not HTTP",
            &["who", "m_1"],
            vec![Reply::Raw(GARBAGE.to_vec())],
            1,
            1,
        ),
        plain("who, no id", &["who"], vec![], 2, 0),
        plain("who, two ids", &["who", "a", "b"], vec![], 2, 0),
        plain("who, a flag", &["who", "a", "-x"], vec![], 2, 0),
        Case {
            compare: Compare::Traceback,
            ..plain(
                "who, a link with no id",
                &["who", "m_1"],
                vec![ok(r#"{"linked":1,"links":[{"x":1}]}"#)],
                1,
                1,
            )
        },
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "Error: daemon answered: \n",
            },
            ..plain(
                "who, no answer",
                &["who", "m_1"],
                vec![Reply::Close],
                141,
                1,
            )
        },
        // covers: msg verify
        plain(
            "verify",
            &["verify", "m_1", "--text", "[from acme · m_1] hi\n> q"],
            vec![ok(verified)],
            0,
            1,
        ),
        plain(
            "verify, no text",
            &["verify", "m_1"],
            vec![ok(r#"{"content":"not_checked"}"#)],
            0,
            1,
        ),
        // covers: msg verify --json
        plain(
            "verify --json, a mismatch",
            &["verify", "--json", "m_1", "--text", "x"],
            vec![ok(r#"{"content":"mismatch"}"#)],
            1,
            1,
        ),
        plain(
            "verify, stdin",
            &["verify", "m_1", "--text-file", "-"],
            vec![ok(r#"{"content":"match"}"#)],
            0,
            1,
        ),
        Case {
            setup: |r: &Rig| {
                r.owner_on_disk();
                std::fs::write(r.proj.join("seen.txt"), "the text\r\n\n\n").unwrap();
            },
            ..plain(
                "verify, a file",
                &["verify", "m_1", "--text-file", "seen.txt"],
                vec![ok(r#"{"content":"match"}"#)],
                0,
                1,
            )
        },
        plain(
            "verify, refused",
            &["verify", "m_1"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"not yours"}}"#.into(),
            )],
            3,
            1,
        ),
        plain(
            "verify, no file",
            &["verify", "m_1", "--text-file", "nope.txt"],
            vec![],
            2,
            0,
        ),
        plain(
            "verify, --text alone",
            &["verify", "m_1", "--text"],
            vec![],
            2,
            0,
        ),
        plain("verify, no code", &["verify", "--json"], vec![], 2, 0),
        plain("verify, two codes", &["verify", "a", "b"], vec![], 2, 0),
        plain("verify, a flag", &["verify", "a", "--x"], vec![], 2, 0),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// A value flag as the last word: bash stops on its own `$2: unbound
/// variable` line (the script's path and line in it) and exits 1; the
/// binary prints one plain usage line and exits 2 (K2's D6,
/// `scripts/cli-parity.sh`'s named difference `edge-usage-exit-2`).
/// Neither sends anything.
#[test]
fn msg_value_flag_last_is_the_d6_line() {
    let rig = Rig::new(VERB, "d6");
    let cli = k2_client::brand::CLI_NAME;
    for (args, flag) in [
        (&["ws", "hi", "--from"][..], "--from"),
        (&["ws", "hi", "--command"], "--command"),
        (&["ws", "hi", "--wake-timeout"], "--wake-timeout"),
        (&["ws", "hi", "--via"], "--via"),
        (&["--from"], "--from"),
    ] {
        let words = leak(args.iter().map(|s| s.to_string()).collect());
        let c = plain(leak_name(flag, args), words, vec![], 1, 0);
        let name = c.name;
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

fn leak_name(flag: &str, args: &[&str]) -> &'static str {
    Box::leak(format!("{flag} last in {}", args.join(" ")).into_boxed_str())
}

/// `msg <agent>::<host>` and `<agent>@<host>`: the peers, the roster and
/// the federation send, each reader's lines and exits, the `@` hint, a
/// chat, a reader that raises.
#[test]
fn msg_to_another_server() {
    let rig = Rig::new(VERB, "remote");
    const PEERS: &str = r#"{"peers":[{"subdomain":"box","fingerprint":"fp1","trust":"trusted"}]}"#;
    const ROSTER: &str =
        r#"{"roster":{"agents":[{"agent":"ada","workspace_id":"w1","aliases":["lovelace"]}]}}"#;
    let to = |a: &'static str| -> &'static [&'static str] {
        leak(vec![
            a.into(),
            "hi".into(),
            "--idempotency-key".into(),
            KEY.into(),
        ])
    };
    let v = vec![
        // covers: msg
        plain(
            "delivered",
            to("ada::box.k2.dev"),
            vec![ok(ON), ok(PEERS), ok(ROSTER), ok(r#"{"status":"sent"}"#)],
            0,
            4,
        ),
        plain(
            "delivered by another name",
            to("lovelace::box.k2.dev"),
            vec![
                ok(ON),
                ok(PEERS),
                ok(ROSTER),
                ok(r#"{"status":"sent","replayed":true}"#),
            ],
            0,
            4,
        ),
        plain(
            "the @ form and a chat",
            to("ada/Field Notes@box"),
            vec![
                ok(ON),
                ok(PEERS),
                ok(ROSTER),
                ok(r#"{"status":"queued","reason":"delivery deferred to retry: refused"}"#),
            ],
            0,
            4,
        ),
        plain("an empty agent", to("@box"), vec![ok(ON)], 2, 1),
        plain("a bad chat", to("ada/a/b::box.x"), vec![ok(ON)], 2, 1),
        plain(
            "federation off",
            to("ada::box.x"),
            vec![
                ok(ON),
                Reply::Answer(404, r#"{"error":"federation is off"}"#.into()),
            ],
            2,
            2,
        ),
        plain(
            "the peers refused",
            to("ada::box.x"),
            vec![
                ok(ON),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"forbidden","hint":"not yours"}}"#.into(),
                ),
            ],
            3,
            2,
        ),
        plain(
            "no such peer",
            to("ada::nope.x"),
            vec![ok(ON), ok(PEERS)],
            2,
            2,
        ),
        plain(
            "not trusted",
            to("ada::box.k2.dev"),
            vec![
                ok(ON),
                ok(r#"{"peers":[{"subdomain":"box","fingerprint":"fp1","trust":"Pending"}]}"#),
            ],
            2,
            2,
        ),
        plain(
            "no such agent",
            to("carl::box.k2.dev"),
            vec![ok(ON), ok(PEERS), ok(ROSTER)],
            2,
            3,
        ),
        plain(
            "the gate",
            to("ada::box.k2.dev"),
            vec![
                ok(ON),
                ok(PEERS),
                ok(ROSTER),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"not_connected","hint":"connect first"}}"#.into(),
                ),
            ],
            3,
            4,
        ),
        plain(
            "declined",
            to("ada::box.k2.dev"),
            vec![
                ok(ON),
                ok(PEERS),
                ok(ROSTER),
                ok(r#"{"status":"declined","hint":"-n"}"#),
            ],
            2,
            4,
        ),
        plain(
            "unexpected",
            to("ada::box.k2.dev"),
            vec![ok(ON), ok(PEERS), ok(ROSTER), ok("[1]")],
            1,
            4,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
    let raises = Case {
        compare: Compare::Traceback,
        ..plain(
            "a reader that raises",
            to("ada::box.k2.dev"),
            vec![ok(ON), ok(r#"{"peers":[7]}"#)],
            1,
            2,
        )
    };
    rig.check(&raises);
}
