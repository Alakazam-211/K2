//! `k5-cli app updates key|token|promote|rollback|pull|push` against the
//! shell CLI's (`cli/src/app-updates.sh`), on the parity rig
//! ([`common::parity`]): the same requests (bodies too), stdout, stderr
//! and exit code.
//!
//! - `key`: the read (none, set), `--set`, `--set-file`, held (exit 2),
//!   unchanged; an empty key, a missing file.
//! - `token`: the read (set, none), `--file`, `--stdin` (the token on
//!   stdin, into the body only).
//! - `promote`, `rollback`, `pull`: their bodies and lines, usage.
//! - `push`: `--no-wait` (exit 2), a wait that ends live (exit 0) or
//!   failed (exit 1); artifacts and sigs paired, a sig with no artifact,
//!   a bad artifact word, a missing file, notes from a file.
//! - Help never acts; a refusal; a value flag given last.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

fn one(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn files(rig: &Rig) {
    owner_and_port(rig);
    for (f, text) in [
        ("app.tar.gz", "bytes"),
        ("app.msi", "bytes"),
        ("app.sig", "sig"),
        ("notes.md", "Fixes é\n\n"),
        ("key.pub", "untrusted comment: k\nRWQ\n\n"),
        ("tok.txt", "  dl-token-4455 \n"),
    ] {
        std::fs::write(rig.proj.join(f), text).unwrap();
    }
}

#[test]
fn key_and_token_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "updkey");
    let v = vec![
        // covers: app updates key
        one(
            "key: none",
            &["updates", "key", "acme"],
            r#"{"app":"acme"}"#,
            0,
        ),
        // covers: app updates key --json
        one(
            "key: set, --show --json",
            &["updates", "key", "acme", "--show", "--json"],
            r#"{"app":"acme","pubkey":"RWQ","keyId":"k1"}"#,
            0,
        ),
        one(
            "key: set",
            &["updates", "key", "acme"],
            r#"{"app":"acme","pubkey":"RWQ é","keyId":"k1","setBy":"ann"}"#,
            0,
        ),
        one(
            "key --set",
            &["updates", "key", "acme", "--set", "RWQ key"],
            r#"{"app":"acme","keyId":"k2","changed":true}"#,
            0,
        ),
        case(
            "key --set-file, unchanged",
            &["updates", "key", "acme", "--set-file", "key.pub"],
            &[],
            vec![ok(r#"{"app":"acme","keyId":"k2","unchanged":true}"#)],
            files,
            0,
            1,
        ),
        case(
            "key --set: held",
            &["updates", "key", "acme", "--set", "RWQ"],
            &[],
            vec![Reply::Answer(
                202,
                r#"{"status":"held","request":{"id":"req-1","summary":"change the key"},"reused":true,"card":{"where":"the Org Chart"},"hint":"the owner decides"}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "key --set --json: held",
            &["updates", "key", "acme", "--set", "RWQ", "--json"],
            &[],
            vec![Reply::Answer(202, r#"{"status":"held"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        stop("key --set: empty", &["updates", "key", "acme", "--set", ""], 2),
        stop("key --set-file: not a file", &["updates", "key", "acme", "--set-file", "nope.pub"], 2),
        stop("key --set given last", &["updates", "key", "acme", "--set"], 2),
        stop("key: two Apps", &["updates", "key", "a", "b"], 2),
        stop("key: no App", &["updates", "key"], 2),
        // covers: app updates token
        one(
            "token: set",
            &["updates", "token", "acme"],
            r#"{"app":"acme","set":true,"fingerprint":"fp","dlId":"dl-1","host":"dl.example"}"#,
            0,
        ),
        one(
            "token: none",
            &["updates", "token", "acme"],
            r#"{"app":"acme","dlId":"dl-1"}"#,
            0,
        ),
        case(
            "token --file",
            &["updates", "token", "acme", "--file", "tok.txt"],
            &[],
            vec![ok(r#"{"app":"acme","fingerprint":"fp","dlId":"dl-1"}"#)],
            files,
            0,
            1,
        ),
        // covers: app updates token --json
        Case {
            stdin: Some(b"  dl-token-7788\r\n\n".as_slice()),
            ..one(
                "token --stdin --json",
                &["updates", "token", "acme", "--stdin", "--json"],
                r#"{"app":"acme","fingerprint":"fp","dlId":"dl-1"}"#,
                0,
            )
        },
        stop("token: the token as a word", &["updates", "token", "acme", "dl-token"], 2),
        stop("token: an unknown flag", &["updates", "token", "acme", "--token"], 2),
        stop("token --file: not a file", &["updates", "token", "acme", "--file", "nope"], 2),
        case(
            "token --file: a refusal",
            &["updates", "token", "acme", "--file", "tok.txt"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.into(),
            )],
            files,
            3,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn promote_rollback_and_pull_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "updmove");
    let v = vec![
        // covers: app updates promote
        one(
            "promote",
            &[
                "updates",
                "promote",
                "acme",
                "--from",
                "beta",
                "--to",
                "stable",
                "--drop-platform",
            ],
            r#"{"app":"acme","channel":"stable","version":"1.2.0","fallback":"kept"}"#,
            0,
        ),
        // covers: app updates promote --json
        one(
            "promote --version, unchanged --json",
            &[
                "updates",
                "promote",
                "acme",
                "--to",
                "stable",
                "--version",
                "1.2.0",
                "--json",
            ],
            r#"{"app":"acme","unchanged":true}"#,
            0,
        ),
        one(
            "promote: unchanged",
            &[
                "updates",
                "promote",
                "acme",
                "--to",
                "stable",
                "--version",
                "1.2.0",
            ],
            r#"{"app":"acme","channel":"stable","version":"1.2.0","unchanged":true}"#,
            0,
        ),
        stop(
            "promote: no --to",
            &["updates", "promote", "acme", "--from", "beta"],
            2,
        ),
        stop("promote: two Apps", &["updates", "promote", "a", "b"], 2),
        // covers: app updates rollback
        one(
            "rollback",
            &[
                "updates",
                "rollback",
                "acme",
                "--channel",
                "stable",
                "--to",
                "1.1.0",
            ],
            r#"{"app":"acme","channel":"stable","from":"1.2.0","version":"1.1.0","warning":"phones on 1.2.0 stay there"}"#,
            0,
        ),
        // covers: app updates rollback --json
        one(
            "rollback --json",
            &[
                "updates",
                "rollback",
                "acme",
                "--channel",
                "s",
                "--to",
                "1",
                "--json",
            ],
            r#"{"app":"acme"}"#,
            0,
        ),
        stop(
            "rollback: no --to",
            &["updates", "rollback", "acme", "--channel", "s"],
            2,
        ),
        stop(
            "rollback: --to given last",
            &["updates", "rollback", "acme", "--to"],
            1,
        ),
        // covers: app updates pull
        one(
            "pull",
            &["updates", "pull", "acme", "--version", "1.2.0"],
            r#"{"app":"acme","version":"1.2.0","channels":[{"channel":"stable","now":"1.1.0"},{"channel":"beta"}],"note":"phones keep what they have"}"#,
            0,
        ),
        one(
            "pull: already",
            &["updates", "pull", "acme", "--version", "1.2.0"],
            r#"{"app":"acme","version":"1.2.0","unchanged":true}"#,
            0,
        ),
        // covers: app updates pull --json
        one(
            "pull --json",
            &["updates", "pull", "acme", "--version", "1", "--json"],
            r#"{"app":"acme"}"#,
            0,
        ),
        case(
            "pull: a refusal",
            &["updates", "pull", "acme", "--version", "1"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"x"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..one(
                "rollback: no warning",
                &["updates", "rollback", "acme", "--channel", "s", "--to", "1"],
                r#"{"app":"a","channel":"s","from":"2","version":"1"}"#,
                1,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn push_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "updpush");
    let live = r#"{"app":"acme","releases":[{"id":"rel-1","version":"1.2.0","state":"live","channels":["beta"],"files":[{"platform":"linux-x86_64","size":5242880,"url":"https://dl.example/a"},{"platform":"windows-x86_64","size":1048,"upload":"pending"}]}]}"#;
    let failed = r#"{"app":"acme","releases":[{"id":"rel-1","version":"1.2.0","state":"failed","error":"upload refused"}]}"#;
    let v = vec![
        // covers: app updates push
        case(
            "push, the wait ends live",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "beta",
                "--version",
                "1.2.0",
                "--artifact",
                "linux-x86_64=app.tar.gz",
                "--artifact",
                "windows-x86_64=app.msi",
                "--sig",
                "linux-x86_64=app.sig",
                "--notes-file",
                "notes.md",
                "--drop-platform",
                "mac",
                "--evidence",
                "run-9",
            ],
            &[],
            vec![ok(r#"{"release":"rel-1"}"#), ok(live)],
            files,
            0,
            2,
        ),
        case(
            "push, the wait ends failed",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "beta",
                "--version",
                "1.2.0",
                "--artifact",
                "linux-x86_64=app.tar.gz",
            ],
            &[],
            vec![ok(r#"{"release":"rel-1"}"#), ok(failed)],
            files,
            1,
            2,
        ),
        // covers: app updates push --json
        case(
            "push --no-wait --json",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "beta",
                "--version",
                "1",
                "--artifact",
                "linux=app.tar.gz",
                "--no-wait",
                "--json",
            ],
            &[],
            vec![ok(r#"{"release":"rel-2"}"#)],
            files,
            2,
            1,
        ),
        case(
            "push --no-wait",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "beta",
                "--version",
                "1",
                "--artifact",
                "linux=app.tar.gz",
                "--no-wait",
            ],
            &[],
            vec![ok(r#"{"release":"rel-2"}"#)],
            files,
            2,
            1,
        ),
        case(
            "push: a sig with no artifact",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "b",
                "--version",
                "1",
                "--artifact",
                "linux=app.tar.gz",
                "--sig",
                "mac=app.sig",
            ],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "push: an artifact word with no =",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "b",
                "--version",
                "1",
                "--artifact",
                "app.tar.gz",
            ],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        stop(
            "push: a missing file",
            &[
                "updates",
                "push",
                "acme",
                "--channel",
                "b",
                "--version",
                "1",
                "--artifact",
                "l=nope",
            ],
            2,
        ),
        stop(
            "push: no channel",
            &[
                "updates",
                "push",
                "acme",
                "--version",
                "1",
                "--artifact",
                "l=f",
            ],
            2,
        ),
        stop(
            "push: --evidence empty",
            &["updates", "push", "acme", "--evidence", ""],
            2,
        ),
        stop(
            "push: --channel given last",
            &["updates", "push", "acme", "--channel"],
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts on a write.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "updhelp");
    let lines: &[&'static [&'static str]] = &[
        &["updates", "key", "acme", "--set", "k", "-h"],
        &["updates", "token", "acme", "--stdin", "--help"],
        &[
            "updates",
            "promote",
            "acme",
            "--to",
            "s",
            "--version",
            "1",
            "-h",
        ],
        &[
            "updates",
            "rollback",
            "acme",
            "--channel",
            "s",
            "--to",
            "1",
            "--help",
        ],
        &["updates", "pull", "acme", "--version", "1", "-h"],
        &[
            "updates",
            "push",
            "acme",
            "--channel",
            "b",
            "--version",
            "1",
            "--artifact",
            "l=f",
            "-h",
        ],
    ];
    for args in lines {
        rig.check(&Case {
            compare: Compare::Shell,
            ..stop("help never acts", args, 0)
        });
    }
}
