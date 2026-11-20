//! `k5-cli fed` against the shell CLI's `cli/k2 fed`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code
//! for every verb of `cmd_fed`.
//!
//! - Each verb: `enable`, `disable`, `status`, `peers` (in order, none,
//!   the daemon's error, one answer above 256 KiB), `pubkey` (each form,
//!   no key, the tunnel key before the identity), `send` (its usage, no
//!   owner token, a refusal), `outbox`, `pair-start` (its flags, a word it
//!   doesn't take, a flag with no value), `pair-join`, `pair-finish` and
//!   `pair-verify` (the token on argv and on stdin; a wrong, damaged or
//!   missing token), `pair-request` (a key file, none), `confirm`, no
//!   verb and an unknown one.
//! - The answers a reader can't read and those of a shape it doesn't
//!   expect (Python's traceback).
//! - The owner token from `daemon.token` only; a passport (the owner's
//!   verbs refused); the gates.
//! - No answer: an empty reply and nothing listening.
//! - Help with `-h` or `--help` anywhere: the shell answers it (the
//!   binary hands it over) with the usage, exit 0, and sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "fed";

const PEERS: &str = r#"{"peers":[{"label":"zeta","subdomain":"z","fingerprint":"0123456789abcdefXYZ","trust":"pinned"},{"subdomain":"Alpha","base_url":"https://a.example/x","repair_recommended":true,"address_notice":{"status":"pending","target":{"subdomain":"new"},"attempts":2,"last_error":"down é"}},{"fingerprint":"ffff","address_notice":{"status":"failed","target":{"base_url":"http://b"}}},{"label":"  ","subdomain":"host.ts.net","trust":"seen"},{"label":"Straße","subdomain":"10.0.0.1:4100"}]}"#;

const OUTBOX: &str = r#"{"peers":[{"label":"acme","queued":2,"oldest_age_secs":30,"dead":1,"dead_letters":[{"msg_uuid":"0123456789","reason":"gone"},{"msg_uuid":"x"},{},{},{},{"msg_uuid":"sixth"}]},{"fingerprint":"abcdefabcdefabcdef","oldest_age_secs":null}],"total_queued":2}"#;

fn reply(body: &str) -> Reply {
    ok(body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

/// The shell hands the line on (help with `-h`): both sides are the
/// shell's.
fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

/// The owner token as `cmd_fed` reads it: `daemon.token`, with spaces
/// `tr` takes out; and the port.
fn fed_owner(r: &Rig) {
    r.port_on_disk();
    std::fs::write(r.k2home.join("daemon.token"), format!(" {OWNER}\n\t")).unwrap();
    r.owner_on_disk();
}

/// A key file in the workspace folder, for `pair-request`.
fn key_file(r: &Rig) {
    fed_owner(r);
    std::fs::write(
        r.proj.join("peer.pem"),
        "-----BEGIN PUBLIC KEY-----\r\nMFkw é\r\n-----END PUBLIC KEY-----\n",
    )
    .unwrap();
}

/// A key file that is not UTF-8.
fn key_file_bytes(r: &Rig) {
    fed_owner(r);
    std::fs::write(r.proj.join("peer.pem"), b"ok\xff").unwrap();
}

/// The identity key: one made once per rig ([`make_key`]), copied in.
fn identity(r: &Rig) {
    fed_owner(r);
    let dir = r.k2home.join("federation");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(r.dir.join("made.pem"), dir.join("identity.pem")).unwrap();
}

/// No key at all (the rig's K2 home outlives a case).
fn no_key(r: &Rig) {
    fed_owner(r);
    for f in ["federation/identity.pem", "tunnel-key.pem"] {
        let p = r.k2home.join(f);
        if p.exists() {
            std::fs::remove_file(p).unwrap();
        }
    }
}

/// Before the daemon's first 0.45.2 boot: only the tunnel key.
fn tunnel_key(r: &Rig) {
    no_key(r);
    std::fs::copy(r.dir.join("made.pem"), r.k2home.join("tunnel-key.pem")).unwrap();
}

/// A P-256 key, made with the system's openssl (the command needs it
/// too).
fn make_key(r: &Rig) {
    let out = std::process::Command::new("openssl")
        .args([
            "ecparam",
            "-name",
            "prime256v1",
            "-genkey",
            "-noout",
            "-out",
        ])
        .arg(r.dir.join("made.pem"))
        .output()
        .expect("openssl, to make a test key");
    assert!(out.status.success(), "openssl: {out:?}");
}

/// Peers enough that the answer is above 256 KiB.
fn many_peers() -> String {
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"label":"peer-{:05}-{}","subdomain":"s{i}","fingerprint":"{:064x}","trust":"pinned"}}"#,
                (i * 7919) % 3000,
                "é".repeat(i % 20),
                i
            )
        })
        .collect();
    format!(r#"{{"peers":[{}]}}"#, rows.join(","))
}

#[test]
fn fed_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: fed
        case("no verb: the usage", &[], &[], vec![], fed_owner, 2, 0),
        case(
            "an unknown verb",
            &["nope", "x"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        // covers: fed enable
        case(
            "enable",
            &["enable"],
            &[],
            vec![reply(r#"{"federationEnabled":true}"#)],
            fed_owner,
            0,
            1,
        ),
        // covers: fed disable
        case(
            "disable",
            &["disable", "x"],
            &[],
            vec![reply(r#"{"federationEnabled":false,"other":1}"#)],
            fed_owner,
            0,
            1,
        ),
        case(
            "enable: not JSON",
            &["enable"],
            &[],
            vec![reply("<html>")],
            fed_owner,
            1,
            1,
        ),
        case(
            "disable: a list",
            &["disable"],
            &[],
            vec![reply("[]")],
            fed_owner,
            1,
            1,
        ),
        // covers: fed status
        case(
            "status",
            &["status"],
            &[],
            vec![reply(r#"{"federationEnabled":1}"#)],
            fed_owner,
            0,
            1,
        ),
        case(
            "status: a refusal",
            &["status"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            fed_owner,
            0,
            1,
        ),
        case(
            "status: a string",
            &["status"],
            &[],
            vec![reply("\"on\"")],
            fed_owner,
            1,
            1,
        ),
        // covers: fed peers
        case(
            "peers",
            &["peers"],
            &[],
            vec![reply(PEERS)],
            fed_owner,
            0,
            1,
        ),
        case(
            "peers: none",
            &["peers"],
            &[],
            vec![reply(r#"{"peers":[]}"#)],
            fed_owner,
            0,
            1,
        ),
        case(
            "peers: a list answer is none",
            &["peers"],
            &[],
            vec![reply("[1]")],
            fed_owner,
            0,
            1,
        ),
        case(
            "peers: the daemon's error, an object",
            &["peers"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"federation_off"}}"#.into(),
            )],
            fed_owner,
            1,
            1,
        ),
        case(
            "peers: the daemon's error, a string",
            &["peers"],
            &[],
            vec![reply(r#"{"error":"off"}"#)],
            fed_owner,
            1,
            1,
        ),
        case(
            "peers: not JSON",
            &["peers"],
            &[],
            vec![reply("")],
            fed_owner,
            1,
            1,
        ),
        // covers: fed outbox
        case(
            "outbox",
            &["outbox"],
            &[],
            vec![reply(OUTBOX)],
            fed_owner,
            0,
            1,
        ),
        case(
            "outbox: empty",
            &["outbox"],
            &[],
            vec![reply(r#"{"peers":[],"total_queued":0}"#)],
            fed_owner,
            0,
            1,
        ),
        case(
            "outbox: the daemon's error",
            &["outbox"],
            &[],
            vec![Reply::Answer(404, r#"{"error":{"code":"off"}}"#.into())],
            fed_owner,
            1,
            1,
        ),
        case(
            "outbox: not JSON",
            &["outbox"],
            &[],
            vec![reply("nope")],
            fed_owner,
            1,
            1,
        ),
        case(
            "outbox: no owner token",
            &["outbox"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: fed send
        case(
            "send",
            &["send", "acme::ws::bot", "hi é \"there\""],
            &[],
            vec![reply("{\"ok\":true,\"id\":\"m1\"}\n\n")],
            fed_owner,
            0,
            1,
        ),
        case(
            "send: a refusal with a permission card",
            &["send", "a::b::c", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            fed_owner,
            0,
            1,
        ),
        case(
            "send: no message",
            &["send", "a::b::c"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "send: no owner token",
            &["send", "a::b::c", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: fed pair-start
        case(
            "pair-start",
            &[
                "pair-start",
                "--label",
                "Acme é",
                "--subdomain=acme",
                "--base-url",
                "http://h:1",
            ],
            &[],
            vec![reply(r#"{"offer":{"id":"acmé","x":[1,2.5,null]}}"#)],
            fed_owner,
            0,
            1,
        ),
        case(
            "pair-start: a word it doesn't take",
            &["pair-start", "--pubkey", "x"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "pair-start: a flag with no value",
            &["pair-start", "--label"],
            &[],
            vec![],
            fed_owner,
            1,
            0,
        ),
        case(
            "pair-start: the daemon's error",
            &["pair-start"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"federation is off"}"#.into(),
            )],
            fed_owner,
            1,
            1,
        ),
        case(
            "pair-start: not JSON",
            &["pair-start"],
            &[],
            vec![reply("")],
            fed_owner,
            1,
            1,
        ),
        case(
            "pair-start: no owner token",
            &["pair-start", "--nope"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: fed pair-join
        case(
            "pair-join: the token on argv",
            &["pair-join", "k2pair1:eyJuIjoxfQ"],
            &[],
            vec![reply(r#"{"answer":{"m":"é"}}"#)],
            fed_owner,
            0,
            1,
        ),
        // covers: fed pair-finish
        stdin(
            b"  k2pair2:eyJpZCI6ImFjbVx1MDBlOSIsIngiOlsxLDIuNSxudWxsXX0\nnext line\n",
            case(
                "pair-finish: the token on stdin",
                &["pair-finish", "--token-stdin"],
                &[],
                vec![reply(
                    r#"{"reveal":{"r":1},"sas":"12-34","peer_fingerprint":"ab"}"#,
                )],
                fed_owner,
                0,
                1,
            ),
        ),
        // covers: fed pair-verify
        stdin(
            b"k2pair3:eyJuIjoxfQ",
            case(
                "pair-verify: - reads stdin",
                &["pair-verify", "-"],
                &[],
                vec![reply(r#"{"sas":7,"peer_fingerprint":"f"}"#)],
                fed_owner,
                0,
                1,
            ),
        ),
        case(
            "pair-join: a token of another step",
            &["pair-join", "k2pair2:eyJuIjoxfQ"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "pair-join: a token with a bad length",
            &["pair-join", "k2pair1:abcde"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "pair-join: a token that is no JSON",
            &["pair-join", "k2pair1:WzEs"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "pair-join: a token that is no UTF-8",
            &["pair-join", "k2pair1:__4="],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "pair-join: no token",
            &["pair-join"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        stdin(
            b"\n",
            case(
                "pair-verify: an empty line on stdin",
                &["pair-verify", "--token-stdin"],
                &[],
                vec![],
                fed_owner,
                2,
                0,
            ),
        ),
        case(
            "pair-finish: no owner token",
            &["pair-finish", "x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: fed pair-request
        case(
            "pair-request",
            &[
                "pair-request",
                "--pubkey",
                "peer.pem",
                "--label=A",
                "--url",
                "http://h:2",
            ],
            &[],
            vec![reply("{\"ok\":true}\n")],
            key_file,
            0,
            1,
        ),
        case(
            "pair-request: no key file",
            &["pair-request", "--pubkey", "missing.pem"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "pair-request: a word it doesn't take",
            &["pair-request", "--token", "x"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        traceback(case(
            "pair-request: a key file that is not UTF-8",
            &["pair-request", "--pubkey", "peer.pem"],
            &[],
            vec![],
            key_file_bytes,
            1,
            0,
        )),
        // covers: fed confirm
        case(
            "confirm",
            &["confirm", "abcd", "12-34"],
            &[],
            vec![Reply::Answer(400, "{\"error\":\"no such peer\"}".into())],
            fed_owner,
            0,
            1,
        ),
        case(
            "confirm: no code",
            &["confirm", "abcd"],
            &[],
            vec![],
            fed_owner,
            2,
            0,
        ),
        case(
            "confirm: no owner token",
            &["confirm", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn fed_pubkey() {
    let rig = Rig::new(VERB, "key");
    make_key(&rig);
    let v = vec![
        // covers: fed pubkey
        case("pubkey", &["pubkey"], &[], vec![], identity, 0, 0),
        // covers: fed pubkey --json
        case(
            "pubkey --json",
            &["pubkey", "--json"],
            &[],
            vec![],
            identity,
            0,
            0,
        ),
        case(
            "pubkey --fp",
            &["pubkey", "--fp"],
            &[],
            vec![],
            identity,
            0,
            0,
        ),
        case(
            "pubkey --pem from the tunnel key",
            &["pubkey", "--pem"],
            &[],
            vec![],
            tunnel_key,
            0,
            0,
        ),
        case(
            "pubkey: a word it doesn't take",
            &["pubkey", "-x"],
            &[],
            vec![],
            identity,
            2,
            0,
        ),
        case("pubkey: no key", &["pubkey"], &[], vec![], no_key, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Answers a reader can't read, and shapes its Python doesn't expect.
#[test]
fn fed_reads_what_it_can() {
    let rig = Rig::new(VERB, "odd");
    let v = vec![
        traceback(case(
            "peers: a peer that is a string",
            &["peers"],
            &[],
            vec![reply(r#"{"peers":["x"]}"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "peers: peers that are a number",
            &["peers"],
            &[],
            vec![reply(r#"{"peers":7}"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "peers: a label that is a number",
            &["peers"],
            &[],
            vec![reply(r#"{"peers":[{"label":5}]}"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "peers: a notice that is a list",
            &["peers"],
            &[],
            vec![reply(r#"{"peers":[{"label":"a","address_notice":[1]}]}"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "outbox: a number",
            &["outbox"],
            &[],
            vec![reply("7")],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "outbox: a list naming error",
            &["outbox"],
            &[],
            vec![reply(r#"["error"]"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "outbox: dead letters that are a number",
            &["outbox"],
            &[],
            vec![reply(r#"{"peers":[{"label":"a","dead_letters":3}]}"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "pair-finish: no code in the answer",
            &["pair-finish", "k2pair2:eyJuIjoxfQ"],
            &[],
            vec![reply(r#"{"reveal":{}}"#)],
            fed_owner,
            1,
            1,
        )),
        traceback(case(
            "pair-start: no offer in the answer",
            &["pair-start"],
            &[],
            vec![reply("{}")],
            fed_owner,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer above 256 KiB.
#[test]
fn fed_peers_large() {
    let rig = Rig::new(VERB, "big");
    let body = many_peers();
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "peers: thousands",
        &["peers"],
        &[],
        vec![reply(&body)],
        fed_owner,
        0,
        1,
    ));
}

#[test]
fn fed_with_a_passport_and_the_gates() {
    let rig = Rig::new(VERB, "pass");
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "send with a passport: the passport, not the owner token",
                &["send", "a::b::c", "x"],
                &[],
                vec![reply("{}")],
                fed_owner,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "pair-start with a passport: the owner's",
                &["pair-start"],
                &[],
                vec![],
                fed_owner,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "confirm with a passport: the owner's",
                &["confirm", "a", "b"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "outbox with a passport: the owner's",
                &["outbox"],
                &[],
                vec![],
                port_only,
                3,
                0,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "peers on the cell socket",
                &["peers"],
                &[],
                vec![reply(PEERS)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "no port: the gate",
            &["status"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "no token: the gate",
            &["peers"],
            &[],
            vec![],
            port_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn fed_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let v = vec![
        case(
            "send: an empty reply is an empty line",
            &["send", "a::b::c", "x"],
            &[],
            vec![Reply::Close],
            fed_owner,
            0,
            1,
        ),
        case(
            "outbox: an empty reply",
            &["outbox"],
            &[],
            vec![Reply::Close],
            fed_owner,
            1,
            1,
        ),
        case(
            "pair-start: an empty reply is curl's code",
            &["pair-start"],
            &[],
            vec![Reply::Close],
            fed_owner,
            52,
            1,
        ),
        case(
            "confirm: an empty reply is curl's code",
            &["confirm", "a", "b"],
            &[],
            vec![Reply::Close],
            fed_owner,
            52,
            1,
        ),
        case(
            "pair-request: an empty reply is curl's code",
            &["pair-request", "--pubkey", "peer.pem"],
            &[],
            vec![Reply::Close],
            key_file,
            52,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let dead = vec![("K2_PORT", dead_port())];
    for c in [
        case(
            "confirm: nothing listening",
            &["confirm", "a", "b"],
            &[],
            vec![],
            fed_owner,
            7,
            0,
        ),
        case(
            "send: nothing listening",
            &["send", "a::b::c", "x"],
            &[],
            vec![],
            fed_owner,
            0,
            0,
        ),
    ] {
        rig.check(&with(dead.clone(), c));
    }
}

/// Help never acts: `-h` or `--help` anywhere prints the fed usage on
/// stdout, exit 0, and sends nothing (`cmd_fed`'s `_wants_help`). The
/// binary hands the line to the shell.
#[test]
fn fed_hands_help_to_the_shell() {
    let rig = Rig::new(VERB, "help");
    let v = vec![
        shell(case(
            "send with -h: the usage, nothing sent",
            &["send", "a::b::c", "x", "-h"],
            &[],
            vec![],
            fed_owner,
            0,
            0,
        )),
        shell(case(
            "enable --help: the usage, nothing sent",
            &["enable", "--help"],
            &[],
            vec![],
            fed_owner,
            0,
            0,
        )),
        shell(case(
            "pair-start -h: the usage, nothing sent",
            &["pair-start", "-h"],
            &[],
            vec![],
            fed_owner,
            0,
            0,
        )),
        shell(case(
            "fed --help: the usage, nothing sent",
            &["--help"],
            &[],
            vec![],
            fed_owner,
            0,
            0,
        )),
        shell(case(
            "disable -h: the usage, nothing sent",
            &["disable", "-h"],
            &[],
            vec![],
            fed_owner,
            0,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
