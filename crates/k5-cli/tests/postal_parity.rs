//! `k5-cli postal ...` (Beta) against the shell CLI's `cli/k2 postal ...`
//! on the parity rig ([`common::parity`]): the same requests, stdout,
//! stderr and exit code.
//!
//! - The usage (no daemon needed), an unknown verb, `adopt`, help
//!   anywhere on a line (help, nothing sent).
//! - The flag loop's lines and each verb's usage line.
//! - Every verb's request and `_postal_print`'s lines for each view:
//!   asked (exit 2), settings, status, friends (one answer above 256 KiB),
//!   code, route, owner-link, send, a friend's state, the JSON fallback.
//! - The daemon's errors (each exit), a refusal's closing line, an answer
//!   that is not JSON, no answer, no daemon.
//! - The owner token in a session (COMPAT-58, as the shell sends it).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "postal";
const ADDR: &str = "scout::acme.postal.bot";
const ASKED: &str = r#"{"ok":true,"asked":true,"card":"c-1","request":"pr_7","waiting":true,"hint":"a person answers it"}"#;

fn check(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

fn leak(args: &[&str]) -> &'static [&'static str] {
    let owned: Vec<&'static str> = args
        .iter()
        .map(|a| &*Box::leak(a.to_string().into_boxed_str()))
        .collect();
    Box::leak(owned.into_boxed_slice())
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn usage_help_and_words() {
    let rig = Rig::new(VERB, "usage");
    let mut v = vec![
        // covers: postal
        case("postal alone", &[], &[], vec![], owner_and_port, 0, 0),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case("help with no daemon", &["help"], &[], vec![], nothing, 0, 0),
        case("adopt", &["adopt"], &[], vec![], nothing, 1, 0),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no daemon: the gate",
            &["status"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            nothing,
            3,
            0,
        ),
    ];
    for args in [
        &["claim", ADDR, "-h"][..],
        &["invite", ADDR, "--why", "x", "--help"],
        &["accept", "pr_1", "--code", "123456", "-h"],
        &["route", "inbox", "--help"],
        &["enable", "-h"],
        &["send", ADDR, "hi", "-h"],
    ] {
        v.push(case(
            "help, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    for args in [
        &["status", "--workspace"][..],
        &["claim", "--why"],
        &["accept", "x", "--code"],
        &["route", "--friend"],
        &["status", "-q"],
        &["claim"],
        &["invite"],
        &["accept"],
        &["reject"],
        &["code"],
        &["remove"],
        &["block"],
        &["send"],
        &["send", "not-an-address", "hi"],
        &["send", ADDR],
        &["route"],
        &["route", "--no-wake"],
        &["admins-may-accept"],
        &["admins-may-accept", "maybe"],
        &["owner-link", "--token-stdin"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn each_view() {
    let rig = Rig::new(VERB, "views");
    let status = r#"{"ok":true,"handle":{"address":"scout::acme.postal.bot","status":"local"},"counts":{"accepted":2,"pendingIn":1,"pendingOut":0},"ownerLinked":false,"plane":{"note":"Postal network: connected."}}"#;
    let friends = r#"{"ok":true,"handle":{"address":"scout::acme.postal.bot"},"friends":[{"peer":"a::b.postal.bot","status":"accepted","updatedAt":1700000000,"ownerName":"Ada"},{"peer":"c::d.postal.bot","status":"pending_in","id":"pr_1","updatedAt":0,"codeWaiting":true},{"peer":"e::f.postal.bot","status":"pending_out","id":"pr_2","decidedBy":"owner","updatedAt":1700000060.5},{"peer":"g::h.postal.bot","status":"rekey_pending","id":"pr_3"},{"peer":"i::j.postal.bot","status":"blocked"},{"peer":"k::l.postal.bot","status":"revoked"},{"peer":"m","status":"odd"}]}"#;
    check(
        &rig,
        vec![
            case(
                "status",
                &["status"],
                &[],
                vec![ok(status)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "status, claimed, no counts",
                &["status", "--workspace", "acme"],
                &[],
                vec![ok(
                    r#"{"ok":true,"handle":{"address":"x::y.postal.bot","status":"claimed"},"already":true}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "status, no address",
                &["status"],
                &[],
                vec![ok(r#"{"ok":true,"workspace":{"handle":"acme"}}"#)],
                owner_and_port,
                0,
                1,
            ),
            // covers: postal --json
            case(
                "status --json",
                &["status", "--json"],
                &[],
                vec![ok(status)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "friends",
                &["friends"],
                &[],
                vec![ok(friends)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "friends, no address",
                &["friends"],
                &[],
                vec![ok(r#"{"ok":true,"friends":[]}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "friends, none",
                &["friends"],
                &[],
                vec![ok(r#"{"ok":true,"handle":{"address":"x::y.postal.bot"}}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "claim: asked",
                &["claim", ADDR, "--why", "to talk é"],
                &[],
                vec![ok(ASKED)],
                owner_and_port,
                2,
                1,
            ),
            case(
                "claim: asked, the default hint",
                &["claim", ADDR],
                &[],
                vec![ok(r#"{"ok":true,"asked":true,"card":"c-2"}"#)],
                owner_and_port,
                2,
                1,
            ),
            case(
                "invite: recorded",
                &["invite", "x::y.postal.bot", "--why", "work"],
                &[],
                vec![ok(
                    r#"{"ok":true,"friend":{"peer":"x::y.postal.bot","status":"pending_out"}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "invite: sent",
                &["invite", "x::y.postal.bot"],
                &[],
                vec![ok(
                    r#"{"ok":true,"sent":true,"friend":{"peer":"x::y.postal.bot","status":"pending_out"}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "accept with a code: friends",
                &["accept", "pr_1", "--code", "123456", "--workspace", "acme"],
                &[],
                vec![ok(
                    r#"{"ok":true,"friends":true,"friend":{"peer":"c::d.postal.bot","status":"accepted"}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "reject: a hint",
                &["reject", "pr_1"],
                &[],
                vec![ok(
                    r#"{"ok":true,"hint":"they are told","friend":{"peer":"c::d.postal.bot","status":"rejected"}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "code",
                &["code", "pr_1"],
                &[],
                vec![ok(r#"{"ok":true,"id":"pr_1","code":"482913"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "route",
                &[
                    "route",
                    "inbox",
                    "thread:ada",
                    "--friend",
                    "a::b.postal.bot",
                    "--no-wake",
                ],
                &[],
                vec![ok(
                    r#"{"ok":true,"friend":"a::b.postal.bot","route":{"targets":["inbox","thread:ada"],"wake":false}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "route --clear",
                &["route", "--clear"],
                &[],
                vec![ok(r#"{"ok":true,"route":null}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "owner-link: a page",
                &["owner-link"],
                &[],
                vec![ok(
                    r#"{"ok":true,"label":"acme","url":"https://example.test/approve","userCode":"ABCD-EFGH"}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            Case {
                stdin: Some(b"k2c_pd_secret\nmore\n"),
                ..case(
                    "owner-link: a token on stdin",
                    &["owner-link", "--token-stdin"],
                    &[],
                    vec![ok(r#"{"ok":true,"linked":true,"label":"acme"}"#)],
                    owner_and_port,
                    0,
                    1,
                )
            },
            case(
                "send: held",
                &["send", ADDR, "hello", "there", "--no-wake"],
                &[],
                vec![ok(
                    r#"{"ok":true,"status":"held","to":"scout::acme.postal.bot","badge":"b1","deliveryId":"p_abc"}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "send: queued",
                &["send", "Scout::Acme.postal.bot", "hi"],
                &[],
                vec![ok(r#"{"ok":true,"status":"queued","to":"x"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "send: failed",
                &["send", ADDR, "hi"],
                &[],
                vec![ok(
                    r#"{"ok":true,"status":"failed","to":"x","reason":"no route"}"#,
                )],
                owner_and_port,
                1,
                1,
            ),
            case(
                "send: delivered, --json",
                &["send", ADDR, "hi", "--json"],
                &[],
                vec![ok(r#"{"ok":true,"status":"delivered","to":"x"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "remove",
                &["remove", "a::b.postal.bot"],
                &[],
                vec![ok(
                    r#"{"ok":true,"friend":{"peer":"a::b.postal.bot","status":"revoked"}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "block",
                &["block", "a::b.postal.bot", "--json"],
                &[],
                vec![ok(
                    r#"{"ok":true,"friend":{"peer":"a::b.postal.bot","status":"blocked"}}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "enable: asked",
                &["enable", "--why", "friends"],
                &[],
                vec![ok(ASKED)],
                owner_and_port,
                2,
                1,
            ),
            case(
                "disable",
                &["disable"],
                &[],
                vec![ok(r#"{"ok":true,"enabled":false,"adminsMayAccept":true}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "admins may accept",
                &["admins-may-accept", "on"],
                &[],
                vec![ok(r#"{"ok":true,"enabled":true}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "admins may not",
                &["admins-may-accept", "off"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "settings",
                &["settings"],
                &[],
                vec![ok(r#"{"ok":true,"enabled":true,"adminsMayAccept":false}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "link-p5: the answer as JSON",
                &["link-p5"],
                &[],
                vec![ok(r#"{"ok":true,"key":"é"}"#)],
                owner_and_port,
                0,
                1,
            ),
        ],
    );
}

#[test]
fn a_friends_list_above_256_kib() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"peer":"agent{i}::label{i}.postal.bot","status":"{}","id":"pr_{i}","updatedAt":{},"ownerName":"Owner number {i}"}}"#,
                ["accepted", "pending_in", "pending_out", "rejected"][i % 4],
                1_700_000_000 + i * 61
            )
        })
        .collect();
    let body = format!(
        r#"{{"ok":true,"handle":{{"address":"scout::acme.postal.bot"}},"friends":[{}]}}"#,
        rows.join(",")
    );
    assert!(
        body.len() > 256 * 1024,
        "the answer is {} bytes",
        body.len()
    );
    rig.check(&case(
        "a large friends list",
        &["friends"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn the_daemon_s_errors() {
    let rig = Rig::new(VERB, "errors");
    let off = r#"{"error":{"code":"postal_off","hint":"Postal is off on this server."},"permission":{"command":"k2 permission request postal.enable --why \"...\"","requestable":true}}"#;
    let mut v = vec![
        case(
            "postal is off: refused",
            &["status"],
            &[],
            vec![Reply::Answer(403, off.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not found",
            &["code", "pr_9"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"postal_no_code","hint":"no code yet"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "a bad address",
            &["claim", "x"],
            &[],
            vec![ok(r#"{"error":{"code":"bad_postal_address","hint":"x"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error as text",
            &["status"],
            &[],
            vec![ok(r#"{"error":"boom"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "no code in the error",
            &["status"],
            &[],
            vec![ok(r#"{"error":{"hint":"something"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["status"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty 200",
            &["settings"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a code that is a number",
            &["status"],
            &[],
            vec![ok(r#"{"error":{"code":5}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a list",
            &["status"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
    ];
    if cfg!(target_os = "linux") {
        v.push(Case {
            compare: Compare::Waited,
            ..case(
                "no answer: the line, then SIGPIPE",
                &["status"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            )
        });
    }
    check(&rig, v);
    let port = dead_port();
    rig.check(&case(
        "no daemon",
        &["status"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// An open tool: in a session with the owner token on disk the shell sends
/// it (COMPAT-58), a passport or not, and so does the binary.
#[test]
fn the_owner_token_in_a_session() {
    let rig = Rig::new(VERB, "session");
    check(
        &rig,
        vec![
            Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a passport and the owner token",
                    &["settings"],
                    &[],
                    vec![ok(r#"{"ok":true}"#)],
                    owner_and_port,
                    0,
                    1,
                )
            },
            Case {
                env: with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                ..case(
                    "a passport, no owner token",
                    &["settings"],
                    &[],
                    vec![ok(r#"{"ok":true}"#)],
                    port_only,
                    0,
                    1,
                )
            },
        ],
    );
}
