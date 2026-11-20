//! `k5-cli msg <address> "text" --secret NAME` against `cli/k2`, on the
//! vault rig ([`common::vault`]): one vault handoff whose note is the
//! text (`_vault_handoff send NAME --to <address> --note <text> --via
//! msg`), and no message. From a person's terminal it goes to the vault
//! socket (with the confirm typed in on stdin), in a K2 terminal or with
//! no vault socket through the daemon. Each case's typed-in password and
//! confirm token reach no child of the binary (the rig's spies).

#![cfg(unix)]

mod common;

use common::parity::{PASSPORT, PERMISSION_ASK};
use common::stub::Reply;
use common::vault::{VCase, VRig};

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

fn msg(name: &'static str, args: &[&'static str], code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.noun = "msg";
    c.code = code;
    c
}

/// From a person's terminal: the vault socket answers.
fn person(name: &'static str, args: &[&'static str], replies: Vec<Reply>, code: i32) -> VCase {
    let mut c = msg(name, args, code);
    c.vault_requests = replies.len();
    c.vault = Some(replies);
    c
}

/// No vault socket: the daemon answers, with the owner token.
fn owner(name: &'static str, args: &[&'static str], reply: Reply, code: i32) -> VCase {
    let mut c = msg(name, args, code);
    c.vault = None;
    c.daemon = vec![reply];
    c.daemon_requests = 1;
    c
}

/// A K2 terminal: the passport, through the daemon.
fn agent(name: &'static str, args: &[&'static str], reply: Reply, code: i32) -> VCase {
    let mut c = owner(name, args, reply, code);
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    c
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

const LANDED: &str = r#"{"status":"landed","name":"API","to":"bob::h","deliveryId":"d1","ref":"k2v:API","landedName":"API","landsAs":{"delivery":"env","ceiling":"file"},"who":"bob on h","addressNote":"resolved"}"#;

#[test]
fn msg_secret_is_one_handoff() {
    let rig = VRig::new();
    let mut confirmed = person(
        "a person's confirm",
        // covers: msg
        &["bob::h", "here is the key", "--secret", "API"],
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
    let long = leak("x".repeat(700));
    let big = leak("word ".repeat(20 * 1024));
    for c in [
        confirmed,
        person(
            "landed, on the vault socket",
            &["bob::h", "the note", "--secret", "API", "--from", "me"],
            vec![ok(LANDED)],
            0,
        ),
        person(
            "held",
            &["--secret", "API", "bob", "the note"],
            vec![ok(
                r#"{"status":"held","name":"API","to":"bob","deliveryId":"d2"}"#,
            )],
            3,
        ),
        person(
            "the summary hint first",
            &["bob", long, "--secret", "API"],
            vec![ok(LANDED)],
            0,
        ),
        person(
            "a 100 KiB note, refused",
            &["bob", big, "--secret", "API"],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"note_too_long","hint":"keep it short"}}"#.into(),
            )],
            2,
        ),
        owner(
            "no vault socket: the owner token",
            &["bob", "the note", "--secret", "API", "--no-wake"],
            ok(LANDED),
            0,
        ),
        agent(
            "from a K2 terminal",
            &["bob::h", "the note", "--secret", "API"],
            ok(LANDED),
            0,
        ),
        agent(
            "a permission card",
            &["bob::h", "the note", "--secret", "API"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        agent(
            "send_not_allowed",
            &["bob", "the note", "--secret", "API"],
            Reply::Answer(
                409,
                r#"{"error":{"code":"send_not_allowed","hint":"h"}}"#.into(),
            ),
            3,
        ),
        agent(
            "not JSON",
            &["bob", "the note", "--secret", "API"],
            Reply::Answer(502, "<html>".into()),
            1,
        ),
        agent(
            "no answer",
            &["bob", "the note", "--secret", "API"],
            Reply::Close,
            1,
        ),
    ] {
        rig.check(&c);
    }
}

/// The name is the handoff's first word: a flag there is the handoff's
/// own usage line, and nothing is sent.
#[test]
fn msg_secret_name_that_is_a_flag() {
    let rig = VRig::new();
    for (name, args) in [
        ("--json: no name", &["bob", "t", "--secret", "--json"][..]),
        ("--value", &["bob", "t", "--secret", "--value"]),
        ("--system", &["bob", "t", "--secret", "--system"]),
        ("an unknown flag", &["bob", "t", "--secret", "--nope"]),
        ("--ttl eats --to", &["bob", "t", "--secret", "--ttl"]),
        ("--for without --stdin", &["bob", "t", "--secret", "--for"]),
    ] {
        rig.check(&msg(name, args, 2));
    }
    // In a K2 terminal, --scope (here the name) is for people.
    let mut c = msg(
        "--scope in a K2 terminal",
        &["bob", "t", "--secret", "--scope"],
        2,
    );
    c.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    c.vault = None;
    rig.check(&c);
}

/// Help never acts: `-h` or `--help` anywhere on a `--secret` line prints
/// help and sends nothing (the binary hands these lines to the shell).
#[test]
fn msg_secret_help_never_acts() {
    let rig = VRig::new();
    for args in [
        &["bob", "t", "--secret", "API", "-h"][..],
        &["bob", "t", "--help", "--secret", "API"],
        &["-h", "bob", "t", "--secret", "API"],
    ] {
        let mut c = msg("help", args, 0);
        c.handed = true;
        rig.check(&c);
    }
}
