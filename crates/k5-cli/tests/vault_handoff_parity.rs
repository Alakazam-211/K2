//! `k5-cli vault send|incoming|sent|ask|asks|unsend|accept-from|send-allow`
//! (`families::vault::handoff`) against `cli/k2`, on the vault rig
//! ([`common::vault`]): each verb's request on the vault socket from a
//! person's terminal and through the daemon from a K2 terminal (or with
//! no vault socket), its printer, its flag checks and refusals, the
//! handoff exit codes, and the confirm a person gives to send a secret.

#![cfg(unix)]

mod common;

use common::parity::{PASSPORT, PERMISSION_ASK};
use common::stub::Reply;
use common::vault::{VCase, VRig};

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

/// From a person's terminal: the vault socket answers.
fn person(name: &'static str, args: &[&'static str], replies: Vec<Reply>, code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.vault_requests = replies.len();
    c.vault = Some(replies);
    c.code = code;
    c
}

/// No vault socket: the daemon answers, with the owner token.
fn owner(name: &'static str, args: &[&'static str], reply: Reply, code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.vault = None;
    c.daemon = vec![reply];
    c.daemon_requests = 1;
    c.code = code;
    c
}

/// A K2 terminal: the passport, through the daemon.
fn agent(name: &'static str, args: &[&'static str], reply: Reply, code: i32) -> VCase {
    let mut c = owner(name, args, reply, code);
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    c
}

fn refused(name: &'static str, args: &[&'static str], code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.code = code;
    c
}

const LANDED: &str = r#"{"status":"landed","name":"API","to":"bob::h","deliveryId":"d1","ref":"k2v:API","landedName":"API2","landsAs":{"delivery":"env","ceiling":"file"},"who":"bob on h","addressNote":"resolved"}"#;

#[test]
fn send_and_its_answers() {
    let rig = VRig::new();
    let mut confirmed = person(
        "send: a person's confirm",
        &["send", "API", "--to", "bob::h"],
        vec![
            Reply::Answer(
                403,
                r#"{"error":{"code":"confirm_required"},"secretId":"s1"}"#.into(),
            ),
            ok(r#"{"methods":["vault-password"]}"#),
            ok(r#"{"token":"tok-3141"}"#),
            ok(LANDED),
        ],
        0,
    );
    confirmed.stdin = b"pw-2718\n".to_vec();
    confirmed.secrets = vec!["pw-2718", "tok-3141"];
    for c in [
        person(
            "send, landed",
            &[
                "send", "API", "--to", "bob::h", "--note", "n", "--ttl", "7d", "--as", "API2",
                "--scope", "ws:proj",
            ],
            vec![ok(LANDED)],
            0,
        ),
        person(
            "send, held",
            &["send", "API", "--to", "bob::h"],
            vec![ok(
                r#"{"status":"held","name":"API","to":"bob","deliveryId":"d2"}"#,
            )],
            3,
        ),
        person(
            "send --json",
            &[
                "send",
                "API",
                "--to",
                "bob",
                "--json",
                "--workspace",
                "proj",
            ],
            vec![ok(LANDED)],
            0,
        ),
        owner(
            "send, no vault socket",
            &[
                "send",
                "API",
                "--to",
                "bob",
                "--reply",
                "a1",
                "--kind",
                "token",
                "--delivery",
                "env",
            ],
            ok(LANDED),
            0,
        ),
        agent(
            "send from a K2 terminal",
            &["send", "API", "--to", "bob::h", "--via", "relay"],
            ok(LANDED),
            0,
        ),
        agent(
            "send: a permission card",
            &["send", "API", "--to", "bob::h"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        agent(
            "send: send_not_allowed on a 200 code",
            &["send", "API", "--to", "x"],
            Reply::Answer(
                409,
                r#"{"error":{"code":"send_not_allowed","hint":"h"}}"#.into(),
            ),
            3,
        ),
        agent(
            "send: bad_ttl",
            &["send", "API", "--to", "x"],
            Reply::Answer(500, r#"{"code":"bad_ttl","error":"bad ttl"}"#.into()),
            2,
        ),
        agent(
            "send: not JSON",
            &["send", "API", "--to", "x"],
            Reply::Answer(502, "<html>".into()),
            1,
        ),
        confirmed,
        refused("send, no --to", &["send", "API"], 2),
        refused(
            "send, a value on argv",
            &["send", "API", "hunter2", "--to", "x"],
            2,
        ),
        refused(
            "send, a bad ttl",
            &["send", "API", "--to", "x", "--ttl", "1.5h"],
            2,
        ),
        refused(
            "send, --for without --stdin",
            &["send", "API", "--to", "x", "--for", "a.com"],
            2,
        ),
        refused(
            "send, --workspace and --scope differ",
            &[
                "send",
                "API",
                "--to",
                "x",
                "--workspace",
                "a",
                "--scope",
                "ws:b",
            ],
            2,
        ),
        refused("send --value", &["send", "API", "--value", "x"], 2),
        refused("send, an unknown flag", &["send", "API", "--nope"], 2),
        refused(
            "send, a flag ask takes only",
            &["unsend", "d1", "--note", "x"],
            2,
        ),
        refused("send, a flag with no value", &["send", "API", "--to"], 2),
    ] {
        rig.check(&c);
    }
    let mut c = refused(
        "send --scope in a K2 terminal",
        &["send", "API", "--to", "x", "--scope", "server"],
        2,
    );
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    rig.check(&c);
}

#[test]
fn the_lists_and_the_small_verbs() {
    let rig = VRig::new();
    let incoming = r#"{"items":[{"name":"API","status":"pending","peer":"bob::h","createdAt":"t1","deliveryId":"d1","card":{"card":"c1","addr":"proj"},"note":"for you","landsAs":{"delivery":"env","ceiling":"file","expiresAt":"t9"}},{"name":"B","landedName":"B2","status":"pending","deliveryId":"d2"},{"name":"C","status":"landed","peer":"x","deliveryId":"d3","ref":"k2v:C","secret":{"delivery":"file","expiresAt":"t8","use":"k2 vault file C"}}]}"#;
    let big = format!(
        r#"{{"items":[{}]}}"#,
        (0..2500)
            .map(|i| format!(r#"{{"name":"SECRET_NUMBER_{i:05}","status":"landed","peer":"agent-{i}::host.example.com","createdAt":"2026-11-20T10:00:00Z","deliveryId":"dlv_{i:08}","ref":"k2v:SECRET_{i}"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(big.len() > 256 * 1024);
    for c in [
        person("incoming", &["incoming"], vec![ok(incoming)], 0),
        person(
            "incoming --workspace",
            &["incoming", "--workspace", "a b"],
            vec![ok(r#"{"items":[]}"#)],
            0,
        ),
        owner("sent", &["sent", "--json"], ok(incoming), 0),
        owner("sent, none", &["sent"], ok("{}"), 0),
        person("sent: above 256 KiB", &["sent"], vec![ok(&big)], 0),
        refused("incoming, a word", &["incoming", "x"], 2),
        person(
            "incoming accept",
            &[
                "incoming",
                "accept",
                "d1",
                "--as",
                "API2",
                "--replace",
                "--always",
            ],
            vec![ok(
                r#"{"delivery":{"status":"landed","name":"API","landedName":"API2","peer":"bob","deliveryId":"d1"},"ref":"k2v:API2","secret":{"delivery":"env","ceiling":"file","expiresAt":"t","use":"u"},"more":"m","alwaysAdded":"bob"}"#,
            )],
            0,
        ),
        person(
            "incoming decline",
            &["incoming", "decline", "d1"],
            vec![ok(
                r#"{"delivery":{"status":"declined","name":"API","deliveryId":"d1"}}"#,
            )],
            0,
        ),
        refused("incoming accept, no id", &["incoming", "accept"], 2),
        refused(
            "incoming decline, a flag it doesn't take",
            &["incoming", "decline", "d1", "--as", "x"],
            2,
        ),
        person(
            "ask",
            &[
                "ask",
                "bob::h",
                "API",
                "--note",
                "please",
                "--workspace",
                "proj",
            ],
            vec![ok(
                r#"{"to":"bob::h","name":"API","askId":"a1","delivered":false,"expiresInDays":7}"#,
            )],
            0,
        ),
        refused("ask, no name", &["ask", "bob"], 2),
        owner(
            "asks",
            &["asks"],
            ok(
                r#"{"items":[{"name":"API","status":"open","direction":"out","peer":"bob","askId":"a1"},{"name":"B","direction":"in","peer":"c"}]}"#,
            ),
            0,
        ),
        owner("asks, none", &["asks"], ok("{}"), 0),
        person(
            "unsend",
            &["unsend", "d1"],
            vec![ok(
                r#"{"deliveryId":"d1","disabled":true,"honest":"they may have copied it"}"#,
            )],
            0,
        ),
        person(
            "unsend, kept",
            &["unsend", "d1"],
            vec![ok(
                r#"{"deliveryId":"d1","keptReplaced":true,"honest":"h"}"#,
            )],
            0,
        ),
        refused("unsend, no id", &["unsend"], 2),
        person(
            "accept-from",
            &["accept-from"],
            vec![ok(
                r#"{"entries":[{"sender":"bob::h","senderLabel":"Bob","addedBy":"me","addedAt":"t"},{"sender":"c"}]}"#,
            )],
            0,
        ),
        person(
            "accept-from add",
            &["accept-from", "add", "bob::h", "--workspace", "proj"],
            vec![ok(r#"{"entries":[]}"#)],
            0,
        ),
        refused(
            "accept-from add, no workspace",
            &["accept-from", "add", "bob"],
            2,
        ),
        refused("accept-from, a verb it lacks", &["accept-from", "nope"], 2),
        person(
            "send-allow",
            &["send-allow", "--workspace", "proj"],
            vec![ok(
                r#"{"workspace":"proj","entries":[{"name":"API","to":"bob::h","addedBy":"me","addedAt":"t"}]}"#,
            )],
            0,
        ),
        person(
            "send-allow remove",
            &[
                "send-allow",
                "remove",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![ok(r#"{"entries":[]}"#)],
            0,
        ),
        person(
            "send-allow, refused",
            &["send-allow"],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"not_yours","hint":"h"}}"#.into(),
            )],
            3,
        ),
    ] {
        rig.check(&c);
    }
    let mut c = refused(
        "incoming accept in a K2 terminal",
        &["incoming", "accept", "d1"],
        3,
    );
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    rig.check(&c);
    let mut c = refused(
        "send-allow add in a K2 terminal",
        &[
            "send-allow",
            "add",
            "API",
            "--to",
            "b::h",
            "--workspace",
            "p",
        ],
        3,
    );
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    rig.check(&c);
    let mut c = refused(
        "accept-from add, no vault socket",
        &["accept-from", "add", "bob", "--workspace", "p"],
        1,
    );
    c.vault = None;
    rig.check(&c);
}
