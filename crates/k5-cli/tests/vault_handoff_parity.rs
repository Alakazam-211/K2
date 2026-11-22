//! `k5-cli vault send|incoming|sent|ask|asks|unsend|accept-from|send-allow`
//! (`families::vault::handoff`) against `cli/k2`, on the vault rig
//! ([`common::vault`]): each verb's request on the vault socket from a
//! person's terminal and through the daemon from a K2 terminal (or with
//! no vault socket), its printer, its flag checks and refusals, the
//! handoff exit codes, and the confirm a person gives to send a secret.
//! 0.46.17: an agent's `accept-from add` is refused with the one card to
//! ask for (`vault.accept-from`, its "Ask with:" line), nothing sent; a
//! person's has no such line; an agent's `remove` is the person guard's.

#![cfg(unix)]

mod common;

use common::parity::{PASSPORT, PERMISSION_ASK};
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};

/// The daemon's refusal of an accept-from edit by a login under Admin.
const ROLE_REQUIRED: &str = r#"{"ok":false,"error":{"code":"role_required","hint":"editing who an agent accepts secrets from takes an Owner or Admin login (or the owner)"}}"#;

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

fn traceback(mut c: VCase) -> VCase {
    c.compare = VCompare::Traceback;
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
        // covers: vault send
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
            // covers: vault unsend
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
        // covers: vault incoming
        person("incoming", &["incoming"], vec![ok(incoming)], 0),
        person(
            "incoming --workspace",
            &["incoming", "--workspace", "a b"],
            vec![ok(r#"{"items":[]}"#)],
            0,
        ),
        // covers: vault sent
        // covers: vault sent --json
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
            // covers: vault incoming decline
            &["incoming", "decline", "d1"],
            vec![ok(
                r#"{"delivery":{"status":"declined","name":"API","deliveryId":"d1"}}"#,
            )],
            0,
        ),
        // covers: vault incoming accept
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
        // covers: vault ask
        refused("ask, no name", &["ask", "bob"], 2),
        owner(
            "asks",
            // covers: vault asks
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
            // covers: vault accept-from
            &["accept-from"],
            vec![ok(
                r#"{"entries":[{"sender":"bob::h","senderLabel":"Bob","addedBy":"me","addedAt":"t"},{"sender":"c"}]}"#,
            )],
            0,
        ),
        person(
            "accept-from add",
            // covers: vault accept-from add
            &["accept-from", "add", "bob::h", "--workspace", "proj"],
            vec![ok(r#"{"entries":[]}"#)],
            0,
        ),
        // 0.46.17: editing the list takes an Owner or Admin login; any
        // other login gets the vault's role refusal, exit 3.
        person(
            "accept-from add, not an owner or admin login",
            &["accept-from", "add", "bob::h", "--workspace", "proj"],
            vec![Reply::Answer(403, ROLE_REQUIRED.into())],
            3,
        ),
        person(
            "accept-from remove, not an owner or admin login",
            &["accept-from", "remove", "bob::h", "--workspace", "proj"],
            vec![Reply::Answer(403, ROLE_REQUIRED.into())],
            3,
        ),
        refused(
            "accept-from add, no workspace",
            &["accept-from", "add", "bob"],
            2,
        ),
        refused("accept-from, a verb it lacks", &["accept-from", "nope"], 2),
        person(
            "send-allow",
            // covers: vault send-allow
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

#[test]
fn send_a_typed_value_and_send_allow_add() {
    let rig = VRig::new();
    let with = |mut c: VCase, stdin: &[u8], secrets: &[&'static str]| {
        c.stdin = stdin.to_vec();
        c.secrets = secrets.to_vec();
        c
    };
    let mut stdin_in_k2 = refused(
        "send --stdin in a K2 terminal",
        &["send", "API", "--to", "bob::h", "--stdin"],
        3,
    );
    stdin_in_k2.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    stdin_in_k2.stdin = b"typed-in-k2\n".to_vec();
    let mut no_sock = refused(
        "send --stdin with no vault socket",
        &["send", "API", "--to", "bob::h", "--stdin"],
        1,
    );
    no_sock.vault = None;
    let allowed =
        ok(r#"{"secrets":[{"name":"OTHER","id":"s0"},{"name":"API","id":"sec-55"}],"entries":[]}"#);
    let added = ok(
        r#"{"workspace":"proj","entries":[{"name":"API","to":"bob::h","addedBy":"amy","addedAt":"t1"}]}"#,
    );
    for c in [
        with(
            person(
                "send --stdin",
                &[
                    "send",
                    "API",
                    "--to",
                    "bob::h",
                    "--stdin",
                    "--for",
                    "a.com, b.com",
                    "--kind",
                    "token",
                    "--ttl",
                    "2h",
                ],
                vec![ok(LANDED)],
                0,
            ),
            b"typed-value-123\n",
            &["typed-value-123", "dHlwZWQtdmFsdWUtMTIz"],
        ),
        with(
            // covers: vault send --json
            person(
                "send --stdin, held for the other side",
                &["send", "API", "--to", "bob::h", "--stdin", "--json"],
                vec![ok(
                    r#"{"status":"held","name":"API","to":"bob::h","deliveryId":"d2"}"#,
                )],
                3,
            ),
            b"typed-value-456",
            &["typed-value-456"],
        ),
        with(
            person(
                "send --stdin, a person's confirm",
                &["send", "API", "--to", "bob::h", "--stdin"],
                vec![
                    Reply::Answer(
                        403,
                        r#"{"error":{"code":"confirm_required","hint":"confirm"},"secretId":"s-9"}"#
                            .into(),
                    ),
                    ok(r#"{"methods":["vault-password"]}"#),
                    ok(r#"{"token":"tk-st-1"}"#),
                    ok(LANDED),
                ],
                0,
            ),
            b"typed-value-789\npw-st-1\n",
            &["typed-value-789", "tk-st-1"],
        ),
        with(
            person(
                "send --stdin, refused",
                &["send", "API", "--to", "bob::h", "--stdin"],
                vec![Reply::Answer(
                    400,
                    r#"{"error":{"code":"value_too_large","hint":"64 KiB at most"}}"#.into(),
                )],
                2,
            ),
            b"v\n",
            &[],
        ),
        person(
            "send --stdin, nothing typed",
            &["send", "API", "--to", "bob::h", "--stdin"],
            vec![],
            2,
        ),
        refused(
            "send --stdin --scope",
            &[
                "send", "API", "--to", "bob::h", "--stdin", "--scope", "server",
            ],
            2,
        ),
        refused(
            "send --stdin --workspace",
            &[
                "send",
                "API",
                "--to",
                "bob::h",
                "--stdin",
                "--workspace",
                "w",
            ],
            2,
        ),
        stdin_in_k2,
        no_sock,
        // covers: vault send-allow add
        person(
            "send-allow add",
            &[
                "send-allow",
                "add",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![
                allowed.clone(),
                ok(r#"{"methods":["local-auth"],"confirmText":"let proj send API"}"#),
                ok(r#"{"token":"t"}"#),
                added.clone(),
            ],
            0,
        ),
        with(
            person(
                "send-allow add --json, the vault password",
                // covers: vault send-allow add --json
                &[
                    "send-allow",
                    "add",
                    "API",
                    "--to",
                    "bob::h",
                    "--workspace",
                    "proj",
                    "--json",
                ],
                vec![
                    allowed.clone(),
                    ok(r#"{"methods":["vault-password"]}"#),
                    ok(r#"{"token":"tk-sa-1"}"#),
                    added.clone(),
                ],
                0,
            ),
            b"pw-sa-1\n",
            &["pw-sa-1", "tk-sa-1"],
        ),
        person(
            "send-allow add, no such secret",
            &[
                "send-allow",
                "add",
                "NOPE",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![allowed.clone()],
            4,
        ),
        traceback(person(
            "send-allow add, a list Python can't read",
            &[
                "send-allow",
                "add",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![ok(r#"{"secrets":[{"name":"API"}]}"#)],
            1,
        )),
        person(
            "send-allow add, the list refused",
            &[
                "send-allow",
                "add",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"an Owner or Admin"}}"#.into(),
            )],
            3,
        ),
        person(
            "send-allow add, confirm cancelled",
            &[
                "send-allow",
                "add",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![allowed.clone(), ok(r#"{"methods":[]}"#)],
            3,
        ),
        // covers: vault send-allow remove
        person(
            "send-allow remove, from a person's terminal",
            &[
                "send-allow",
                "remove",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
            ],
            vec![ok(r#"{"workspace":"proj","entries":[]}"#)],
            0,
        ),
        traceback(person(
            "send, an answer Python can't read",
            &["send", "API", "--to", "bob::h"],
            vec![ok("not json")],
            1,
        )),
    ] {
        rig.check(&c);
    }
}

/// Each verb with `--json`: the answer as it came, then its exit; and a
/// refusal with `--json`, which is the plain refusal.
#[test]
fn the_json_forms() {
    let rig = VRig::new();
    let entries = r#"{"workspace":"proj","entries":[{"sender":"bob::h","name":"API","to":"bob::h","addedBy":"me","addedAt":"t"}]}"#;
    let items = r#"{"items":[{"name":"API","status":"pending","peer":"bob::h","deliveryId":"d1","direction":"out","askId":"a1"}]}"#;
    let decided =
        r#"{"delivery":{"status":"landed","name":"API","deliveryId":"d1"},"ref":"k2v:API"}"#;
    for c in [
        // covers: vault incoming --json
        person(
            "incoming --json",
            &["incoming", "--json"],
            vec![ok(items)],
            0,
        ),
        owner(
            "incoming --json --workspace, no vault socket",
            &["incoming", "--workspace", "proj", "--json"],
            ok(r#"{"items":[]}"#),
            0,
        ),
        // covers: vault incoming accept --json
        person(
            "incoming accept --json",
            &["incoming", "accept", "d1", "--as", "API2", "--json"],
            vec![ok(decided)],
            0,
        ),
        // covers: vault incoming decline --json
        person(
            "incoming decline --json",
            &["incoming", "decline", "d1", "--json"],
            vec![ok(decided)],
            0,
        ),
        person(
            "incoming decline --json, refused",
            &["incoming", "decline", "d1", "--json"],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"not_pending","hint":"it was decided"}}"#.into(),
            )],
            2,
        ),
        // covers: vault accept-from --json
        person(
            "accept-from --json",
            &["accept-from", "--json"],
            vec![ok(entries)],
            0,
        ),
        agent(
            "accept-from list --json in a K2 terminal",
            &["accept-from", "list", "--json"],
            ok(entries),
            0,
        ),
        // covers: vault accept-from add --json
        person(
            "accept-from add --json",
            &[
                "accept-from",
                "add",
                "bob::h",
                "--workspace",
                "proj",
                "--json",
            ],
            vec![ok(entries)],
            0,
        ),
        // covers: vault accept-from remove
        person(
            "accept-from remove",
            &["accept-from", "remove", "bob::h", "--workspace", "proj"],
            vec![ok(r#"{"entries":[]}"#)],
            0,
        ),
        // covers: vault accept-from remove --json
        person(
            "accept-from remove --json",
            &[
                "accept-from",
                "remove",
                "bob::h",
                "--workspace",
                "proj",
                "--json",
            ],
            vec![ok(r#"{"entries":[]}"#)],
            0,
        ),
        refused(
            "accept-from remove, no workspace",
            &["accept-from", "remove", "bob::h", "--json"],
            2,
        ),
        // covers: vault send-allow --json
        person(
            "send-allow --json",
            &["send-allow", "--workspace", "proj", "--json"],
            vec![ok(entries)],
            0,
        ),
        // covers: vault send-allow remove --json
        person(
            "send-allow remove --json",
            &[
                "send-allow",
                "remove",
                "API",
                "--to",
                "bob::h",
                "--workspace",
                "proj",
                "--json",
            ],
            vec![ok(r#"{"workspace":"proj","entries":[]}"#)],
            0,
        ),
        // covers: vault ask --json
        person(
            "ask --json",
            &["ask", "bob::h", "API", "--note", "please", "--json"],
            vec![ok(
                r#"{"to":"bob::h","name":"API","askId":"a1","delivered":true}"#,
            )],
            0,
        ),
        // covers: vault asks --json
        owner("asks --json", &["asks", "--json"], ok(items), 0),
        agent(
            "asks --json, refused",
            &["asks", "--json"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        // covers: vault unsend --json
        person(
            "unsend --json",
            &["unsend", "d1", "--json"],
            vec![ok(r#"{"deliveryId":"d1","disabled":true}"#)],
            0,
        ),
    ] {
        rig.check(&c);
    }
}

/// 0.46.17: an agent asks a person on one card (`vault.accept-from`).
#[test]
fn accept_from_add_asks_on_one_card() {
    let rig = VRig::new();
    let in_terminal = |name: &'static str, args: &[&'static str], env: (&'static str, String)| {
        let mut c = refused(name, args, 3);
        c.env = vec![env];
        c
    };
    for c in [
        in_terminal(
            "accept-from add in a K2 terminal: the Ask with: line",
            &["accept-from", "add", "bob::h", "--workspace", "proj"],
            ("K2_HOOK_TOKEN", PASSPORT.to_string()),
        ),
        in_terminal(
            "accept-from add, several receivers, a sender with a space",
            &[
                "accept-from",
                "add",
                "bo b",
                "--workspace",
                "c,d é",
                "--json",
            ],
            ("K5_HOOK_TOKEN", PASSPORT.to_string()),
        ),
        in_terminal(
            "accept-from remove in a K2 terminal: the person guard",
            &["accept-from", "remove", "bob", "--workspace", "proj"],
            ("K2_HOOK_TOKEN", PASSPORT.to_string()),
        ),
        person(
            "accept-from add from a person: no Ask line",
            &["accept-from", "add", "bob::h", "--workspace", "proj"],
            vec![ok(r#"{"entries":[{"sender":"bob::h"}]}"#)],
            0,
        ),
    ] {
        rig.check(&c);
    }
    let mut c = refused(
        "accept-from add in a K2 terminal, no workspace: usage first",
        &["accept-from", "add", "bob"],
        2,
    );
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    rig.check(&c);
}

/// The Ask with: line names the brand's command.
#[test]
fn the_accept_from_ask_line_says_the_brand_s_name() {
    let rig = VRig::new();
    let cli = k2_client::brand::CLI_NAME;
    let mut c = refused(
        "brand",
        &["accept-from", "add", "bob", "--workspace", "p"],
        3,
    );
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    let ran = rig.run(false, false, &c);
    assert_eq!(
        ran.out.stderr,
        format!("handoff_person_only: a person decides who an agent takes secrets from without a card.\nAsk with: {cli} permission request vault.accept-from sender=bob receivers=p --why \"<what you need it for>\" (several receivers: receivers=a,b; one person: --assign <login>)\n")
    );
    assert_eq!(ran.out.code, Some(3));
}
