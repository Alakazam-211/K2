//! The writes of `k5-cli app links|pairing|push|email` against the shell
//! CLI's (`cli/src/app-links.sh`, `cli/src/app-auth.sh`), on the parity
//! rig ([`common::parity`]): the same requests (bodies too), stdout,
//! stderr and exit code.
//!
//! - links `set|off|check`: the body the Python builds (spaces, an
//!   empty value, Android fingerprints), the status after, a failed
//!   check (exit 1); a flag of `set` elsewhere, a bad Android spec.
//! - pairing `setup|set|off|check`: phones, desktop only, the stores;
//!   the usage pairs; the `Next:` line.
//! - push `setup` (key files read, CRLF as Python reads it),
//!   `creds rm`, `check`, `on|off`, `set`, `send|test` (202 too),
//!   `devices rm`; a number that isn't, `--data` without `=`.
//! - email `on|off|test|send`: the lists, a header, a text file, held
//!   (exit 2).
//! - The confirm step: in a session the 403 is the refusal; outside one,
//!   no service id is the refusal, and with one the confirm is asked over
//!   the vault socket (none here: its failure, exit 3).
//! - Help never acts: `-h` and `--help` on every write send nothing,
//!   but `push devices rm` (the shell's bug: its two words are taken as
//!   they come, so `-h` is a device id and it sends the remove).
//! - A refusal; no daemon; a value flag given last; Python's raise.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

const LINKS: &str = r#"{"app":"acme","name":"Acme","state":"live","served":true,"ios":["T.app"],"android":[{"package":"com.acme"}],"paths":[{"spec":"/open/*","since":1700000000}],"hosts":["acme.example"]}"#;
const PAIRING: &str =
    r#"{"app":"acme","state":"on","ios":["T.app"],"hosts":[{"host":"acme.example","pairs":true}]}"#;
const PUSH: &str = r#"{"app":"acme","state":"on","apns":{"keyId":"K","teamId":"T","bundleId":"com.acme","keyFingerprint":"f1"},"devices":{"total":1}}"#;
const EMAIL: &str = r#"{"app":"acme","on":true,"boxSwitch":"on","sender":{"address":"no-reply@acme.example","fromName":"Acme","path":"relay","state":"ok"},"recipientScope":"users"}"#;
const CONFIRM: &str = r#"{"error":{"code":"vault_confirm_required","hint":"confirm","serviceId":"svc-1","action":"app-email-on","secretId":"sec-1"}}"#;

fn one(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn refused(name: &'static str, args: &'static [&'static str]) -> Case {
    case(
        name,
        args,
        &[],
        vec![Reply::Answer(
            403,
            r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.into(),
        )],
        owner_and_port,
        3,
        1,
    )
}

#[test]
fn links_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "lset");
    let check = r#"{"hosts":[{"host":"acme.example","aasa":"ok","assetlinks":"stale","appleCdn":"weird"},{"host":"b.example","aasa":"none","assetlinks":"ok","appleCdn":"not_served"}]}"#;
    let v = vec![
        one(
            "links set",
            &[
                "links",
                "set",
                "acme",
                "--ios",
                " T.app ",
                "--ios",
                "",
                "--android",
                "com.acme:AB:CD, EF ,",
                "--path",
                "/open/*",
                "--drop",
                "/old é",
            ],
            LINKS,
            0,
        ),
        one(
            "links set --json, Android only",
            &["links", "set", "--json", "acme", "--android", "com.b:11"],
            LINKS,
            0,
        ),
        one("links off", &["links", "off", "acme"], LINKS, 0),
        one(
            "links check, a stale file",
            &["links", "check", "acme"],
            check,
            1,
        ),
        one(
            "links check, all well",
            &["links", "check", "acme"],
            r#"{"hosts":[{"host":"h","aasa":"ok","assetlinks":"none"}]}"#,
            0,
        ),
        one(
            "links check --json",
            &["links", "check", "acme", "--json"],
            check,
            0,
        ),
        stop(
            "links set: no phone app",
            &["links", "set", "acme", "--path", "/x"],
            2,
        ),
        stop(
            "links set: a bad Android spec",
            &["links", "set", "acme", "--android", "com.b"],
            2,
        ),
        stop(
            "links off: a flag of set",
            &["links", "off", "acme", "--drop", "/x"],
            2,
        ),
        stop("links set: no App", &["links", "set"], 2),
        stop(
            "links set: --ios given last",
            &["links", "set", "acme", "--ios"],
            2,
        ),
        stop(
            "links check: a second App",
            &["links", "check", "a", "b"],
            2,
        ),
        refused("links off: a refusal", &["links", "off", "acme"]),
        traceback(one(
            "links check: a host that is a list",
            &["links", "check", "acme"],
            r#"{"hosts":[{"host":"h","aasa":[1]}]}"#,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn pairing_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "pset");
    let v = vec![
        one(
            "pairing setup",
            &[
                "pairing",
                "setup",
                "acme",
                "--ios",
                "T.app",
                "--android",
                "com.a:AA",
                "--host",
                "acme.example",
                "--native-origin",
                "tauri://localhost",
                "--link-path",
                "/pair",
                "--app-store",
                "https://apps.example/1",
                "--play-store",
                "https://play.example/a",
            ],
            PAIRING,
            0,
        ),
        one(
            "pairing set, desktop only",
            &["pairing", "set", "acme", "--desktop-only"],
            PAIRING,
            0,
        ),
        one(
            "pairing setup --json",
            &["pairing", "setup", "acme", "--ios", "T.app", "--json"],
            PAIRING,
            0,
        ),
        one("pairing off", &["pairing", "off", "acme"], PAIRING, 0),
        one(
            "pairing check",
            &["pairing", "check", "acme"],
            r#"{"hosts":[{"host":"h","aasa":"tls_error","assetlinks":"ok","appleCdn":"ok"}]}"#,
            1,
        ),
        stop(
            "pairing setup: no App",
            &["pairing", "setup", "--ios", "x"],
            2,
        ),
        stop("pairing setup: no phone", &["pairing", "setup", "acme"], 2),
        stop(
            "pairing setup: desktop and a phone",
            &["pairing", "setup", "acme", "--desktop-only", "--ios", "x"],
            2,
        ),
        stop(
            "pairing setup: an unknown flag",
            &["pairing", "setup", "a", "--x"],
            2,
        ),
        stop("pairing off: no App", &["pairing", "off"], 2),
        stop(
            "pairing check: an unknown flag",
            &["pairing", "check", "a", "--x"],
            2,
        ),
        stop(
            "pairing setup: --host given last",
            &["pairing", "setup", "a", "--host"],
            2,
        ),
        refused(
            "pairing setup: a refusal",
            &["pairing", "setup", "acme", "--ios", "T"],
        ),
        case(
            "pairing setup: a confirm with no service id",
            &["pairing", "setup", "acme", "--ios", "T"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"vault_confirm_required","hint":"confirm"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "pairing setup: a confirm, no vault socket",
            &["pairing", "setup", "acme", "--ios", "T"],
            &[],
            vec![Reply::Answer(403, CONFIRM.into())],
            owner_and_port,
            3,
            1,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "pairing setup: in a session, no confirm",
                &["pairing", "setup", "acme", "--ios", "T"],
                &[],
                vec![Reply::Answer(403, CONFIRM.into())],
                nothing,
                3,
                1,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

fn key_files(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(
        rig.proj.join("key.p8"),
        "-----BEGIN KEY-----\r\nabc é\r\n-----END KEY-----\r\n",
    )
    .unwrap();
    std::fs::write(
        rig.proj.join("sa.json"),
        "{\"type\": \"service_account\"}\n",
    )
    .unwrap();
}

#[test]
fn push_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "pushset");
    let sent = r#"{"queued":3,"sendId":"s-1","skipped":{"muted_guest":1,"off":0,"no_device":2}}"#;
    let mut v = vec![
        case(
            "push setup, both keys",
            &[
                "push",
                "setup",
                "acme",
                "--apns-key",
                "key.p8",
                "--key-id",
                "K",
                "--team-id",
                "T",
                "--bundle-id",
                "com.acme",
                "--fcm",
                "sa.json",
            ],
            &[],
            vec![ok(PUSH)],
            key_files,
            0,
            1,
        ),
        case(
            "push setup --json, Firebase only",
            &[
                "push",
                "setup",
                "acme",
                "--fcm-service-account",
                "sa.json",
                "--json",
            ],
            &[],
            vec![ok(PUSH)],
            key_files,
            0,
            1,
        ),
        stop("push setup: no key", &["push", "setup", "acme"], 2),
        stop(
            "push setup: a key without its ids",
            &["push", "setup", "acme", "--apns-key", "key.p8"],
            2,
        ),
        stop(
            "push setup: a file that isn't there",
            &["push", "setup", "acme", "--fcm", "nope.json"],
            2,
        ),
        one(
            "push creds rm",
            &["push", "creds", "rm", "acme", "--apns"],
            PUSH,
            0,
        ),
        one(
            "push credentials remove --json",
            &["push", "credentials", "remove", "acme", "--all", "--json"],
            PUSH,
            0,
        ),
        stop("push creds: the page", &["push", "creds"], 0),
        stop("push creds: an unknown verb", &["push", "creds", "zap"], 2),
        stop(
            "push creds rm: no platform",
            &["push", "creds", "rm", "acme"],
            2,
        ),
        one(
            "push check",
            &["push", "check", "acme"],
            r#"{"results":{"apns":{"configured":true,"ok":false,"code":"bad_key","limits":"10 a second"},"fcm":{"configured":true,"ok":true}}}"#,
            1,
        ),
        one(
            "push check, no keys",
            &["push", "check", "acme"],
            r#"{}"#,
            0,
        ),
        one("push on", &["push", "on", "acme"], PUSH, 0),
        one(
            "push off --json",
            &["push", "off", "acme", "--json"],
            PUSH,
            0,
        ),
        stop("push on: no App", &["push", "on"], 2),
        one(
            "push set",
            &[
                "push",
                "set",
                "acme",
                "--agent-replies",
                "off",
                "--chat",
                "on",
                "--previews",
                "on",
                "--max-devices",
                "05",
                "--register",
                "server",
            ],
            PUSH,
            0,
        ),
        stop("push set: nothing to set", &["push", "set", "acme"], 2),
        stop(
            "push set: a bad switch",
            &["push", "set", "acme", "--chat", "yes"],
            2,
        ),
        stop(
            "push set: a bad number",
            &["push", "set", "acme", "--max-devices", "5x"],
            2,
        ),
        stop(
            "push set: --chat given last",
            &["push", "set", "acme", "--chat"],
            2,
        ),
        case(
            "push send, queued (202)",
            &[
                "push",
                "send",
                "acme",
                "--to",
                "ann",
                "--to",
                "bo",
                "--title",
                "Hi é",
                "--body",
                "b",
                "--topic",
                "t",
                "--collapse-id",
                "c",
                "--thread-id",
                "th",
                "--sound",
                "default",
                "--badge",
                "3",
                "--priority",
                "high",
                "--ttl",
                " 60 ",
                "--android-channel",
                "ch",
                "--silent",
                "--data",
                "k=v",
                "--data",
                "x=1=2",
            ],
            &[],
            vec![Reply::Answer(202, sent.into())],
            owner_and_port,
            0,
            1,
        ),
        one(
            "push send to a room",
            &["push", "send", "acme", "--room", "lobby"],
            r#"{"queued":0}"#,
            0,
        ),
        one(
            "push test",
            &["push", "test", "acme", "--to", "ann", "--title", "dropped"],
            sent,
            0,
        ),
        stop("push send: no one", &["push", "send", "acme"], 2),
        stop(
            "push send: both",
            &["push", "send", "acme", "--to", "a", "--room", "r"],
            2,
        ),
        stop(
            "push send: a badge that isn't a number",
            &["push", "send", "acme", "--to", "a", "--badge", "x"],
            2,
        ),
        stop(
            "push send: data without =",
            &["push", "send", "acme", "--to", "a", "--data", "k"],
            2,
        ),
        stop("push test: no one", &["push", "test", "acme"], 2),
        one(
            "push devices rm",
            &["push", "devices", "rm", "acme", "apd_1"],
            r#"{"ok":true}"#,
            0,
        ),
        stop(
            "push devices rm: no id",
            &["push", "devices", "remove", "acme"],
            2,
        ),
        refused("push on: a refusal", &["push", "on", "acme"]),
        traceback(one(
            "push send: skips that are a list",
            &["push", "send", "acme", "--to", "a"],
            r#"{"skipped":[1]}"#,
            1,
        )),
    ];
    // `push devices rm acme -h`: the shell takes -h as the device id and
    // sends the remove (a bash bug; the row flips when the fix lands).
    v.push(shell(case(
        "push devices rm: -h is taken as the id (bash bug)",
        &["push", "devices", "rm", "acme", "-h"],
        &[],
        vec![ok(r#"{"ok":true}"#)],
        owner_and_port,
        0,
        1,
    )));
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn email_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "emset");
    fn text_file(rig: &Rig) {
        owner_and_port(rig);
        std::fs::write(rig.proj.join("body.txt"), "line one\r\nline two é\n").unwrap();
    }
    let v = vec![
        one(
            "email on",
            &[
                "email",
                "on",
                "acme",
                "--sender",
                "no-reply@acme.example",
                "--scope",
                "any",
            ],
            EMAIL,
            0,
        ),
        one(
            "email on --from --json",
            &["email", "on", "acme", "--from", "s-1", "--json"],
            EMAIL,
            0,
        ),
        one(
            "email off",
            &["email", "off", "acme"],
            r#"{"app":"acme","on":false}"#,
            0,
        ),
        one(
            "email test",
            &["email", "test", "acme"],
            r#"{"id":"em_9","result":"sent","to":"owner@acme.example"}"#,
            0,
        ),
        case(
            "email send, held (202)",
            &[
                "email",
                "send",
                "acme",
                "--to",
                "a@acme.example",
                "--to",
                "--cc",
                "--cc",
                "c@acme.example",
                "--bcc",
                "b@acme.example",
                "--reply-to",
                "r@acme.example",
                "--header",
                "X-A=1=2",
                "--tag",
                "t1",
                "--subject",
                "Hello é",
                "--text",
                "plain",
                "--idempotency-key",
                "k-1",
            ],
            &[],
            vec![Reply::Answer(
                202,
                r#"{"id":"em_1","status":"held","recipients":1}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "email send, a text file",
            &[
                "email",
                "send",
                "acme",
                "--to",
                "a@acme.example",
                "--subject",
                "S",
                "--text-file",
                "body.txt",
            ],
            &[],
            vec![ok(r#"{"id":"em_2","status":"queued","recipients":2}"#)],
            text_file,
            0,
            1,
        ),
        stop("email on: no sender", &["email", "on", "acme"], 2),
        stop(
            "email on: a bad scope",
            &["email", "on", "acme", "--sender", "s", "--scope", "all"],
            2,
        ),
        stop("email off: no App", &["email", "off"], 2),
        stop(
            "email send: no subject",
            &["email", "send", "acme", "--to", "a", "--text", "t"],
            2,
        ),
        stop(
            "email send: no text",
            &["email", "send", "acme", "--to", "a", "--subject", "s"],
            2,
        ),
        stop(
            "email send: a header without =",
            &[
                "email",
                "send",
                "acme",
                "--to",
                "a",
                "--subject",
                "s",
                "--text",
                "t",
                "--header",
                "X",
            ],
            2,
        ),
        stop(
            "email send: a file that isn't there",
            &[
                "email",
                "send",
                "acme",
                "--to",
                "a",
                "--subject",
                "s",
                "--html-file",
                "nope.html",
            ],
            2,
        ),
        stop(
            "email send: --to given last",
            &["email", "send", "acme", "--to"],
            2,
        ),
        refused("email test: a refusal", &["email", "test", "acme"]),
        case(
            "email on: a confirm, no vault socket",
            &["email", "on", "acme", "--sender", "s"],
            &[],
            vec![Reply::Answer(403, CONFIRM.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "email on: a confirm with no action",
            &["email", "on", "acme", "--sender", "s"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"vault_confirm_required","secretId":"x"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: `-h` or `--help` anywhere on a write sends nothing
/// (a page, exit 0, where the verb's loop knows the flag; its unknown
/// flag line, exit 2, where it doesn't).
#[test]
fn help_never_acts_on_a_write() {
    let rig = Rig::new(VERB, "pshelp");
    let lines: &[(&'static [&'static str], i32)] = &[
        (&["links", "set", "acme", "--ios", "T", "-h"], 0),
        (&["links", "off", "--help", "acme"], 0),
        (&["links", "check", "acme", "-h"], 0),
        (&["pairing", "setup", "acme", "--ios", "T", "--help"], 0),
        (&["pairing", "off", "acme", "-h"], 2),
        (&["pairing", "check", "-h", "acme"], 2),
        (&["push", "setup", "acme", "--fcm", "x", "-h"], 0),
        (&["push", "creds", "rm", "acme", "--all", "--help"], 2),
        (&["push", "check", "acme", "-h"], 2),
        (&["push", "on", "acme", "--help"], 2),
        (&["push", "off", "-h", "acme"], 2),
        (&["push", "set", "acme", "--chat", "on", "-h"], 2),
        (&["push", "send", "acme", "--to", "a", "--help"], 0),
        (&["push", "test", "acme", "--to", "a", "-h"], 0),
        (&["email", "on", "acme", "--sender", "s", "-h"], 0),
        (&["email", "off", "acme", "--help"], 0),
        (&["email", "test", "-h", "acme"], 0),
        (
            &[
                "email",
                "send",
                "acme",
                "--to",
                "a",
                "--subject",
                "s",
                "--text",
                "t",
                "--help",
            ],
            0,
        ),
    ];
    for (args, code) in lines {
        rig.check(&shell(stop("help never acts", args, *code)));
    }
    // A value flag takes the next word as it comes, so `-h` there is a
    // value and the write is sent (bash bugs, app-links.sh:21 and
    // app-auth.sh:176; the rows flip when the fixes land). `email` is
    // safe: the router answers its -h with the family's usage first.
    for args in [
        &["links", "set", "acme", "--ios", "-h"][..],
        &["pairing", "setup", "acme", "--ios", "T", "--host", "-h"],
    ] {
        rig.check(&shell(case(
            "help as a value acts (bash bug)",
            args,
            &[],
            vec![ok(r#"{"app":"acme"}"#)],
            owner_and_port,
            0,
            1,
        )));
    }
}

#[test]
fn no_daemon() {
    let rig = Rig::new(VERB, "psdead");
    let dead = dead_port();
    rig.check(&case(
        "push on: no daemon",
        &["push", "on", "acme"],
        &[("K2_PORT", dead.as_str())],
        vec![],
        owner_only,
        3,
        0,
    ));
}

/// Bash bugs the binary copies until K2's fixes land; each row flips then.
#[test]
fn bash_bugs_copied() {
    let rig = Rig::new(VERB, "psbugs");
    let v = vec![
        // `app push send` takes no bare `--` (it is not in
        // `_k2_takes_dashdash`, 7a07949682): a `--to --` value is the
        // shell's refusal, "usage: app takes no '--'", exit 2, before
        // anything runs. The front gives the same line first. (It used to
        // reach app-auth.sh:605's `args.index("--")`, a bash bug.)
        stop(
            "push send: --to -- is a stray --, refused",
            &[
                "push", "send", "acme", "--to", "--", "--to", "b", "--data", "k=v",
            ],
            2,
        ),
        // app-auth.sh:881-885: a list value that names a list (`--to
        // --cc`) switches which list the Python fills, so the To list is
        // sent empty and the address goes to Cc. Flips when the fix lands.
        one(
            "email send: a list value switches the list (bash bug)",
            &[
                "email",
                "send",
                "acme",
                "--to",
                "--cc",
                "--cc",
                "b@acme.example",
                "--subject",
                "s",
                "--text",
                "t",
            ],
            r#"{"id":"em_3","status":"queued","recipients":1}"#,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}
