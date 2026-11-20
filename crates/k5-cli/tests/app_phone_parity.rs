//! The reads of `k5-cli app links|pairing|push|email` against the shell
//! CLI's (`cli/src/app-links.sh`, `cli/src/app-auth.sh`), on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - `links <app>` and `links status <app>`: link files, apps, paths,
//!   hosts, the last check, notes; not served; `--json`; the page; a
//!   flag that goes with `set`, a second App, an unknown flag, usage.
//! - `pairing status|log`: on, off with its reason and not served; the
//!   phones paired; the page, an unknown verb, usage.
//! - `push status|log|devices`: keys, devices, chat pushes; sends and
//!   skips; phones with `--guest`; the page, an unknown verb, usage.
//! - `email show|log|status`: sender or none, off; the log's recipients;
//!   one email's status; the page, an unknown verb, usage.
//! - The writes stay the shell's (`push devices rm`).
//! - Python's raise part way.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "app";

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn read(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

/// Python raises: exit 1, after the one request.
fn fail(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1)
}

fn usage(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

fn page(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 0, 0)
}

#[test]
fn links_and_pairing_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "links");
    let links = r#"{"app":"acme","name":"Acme é","state":"live","served":true,"pairing":true,"ios":["TEAM.app"],"android":[{"package":"com.acme"},{"package":"com.acme.beta"}],"paths":[{"spec":"/open/*","since":1700000000},{"spec":"/x"}],"hosts":["acme.example"],"lastCheck":{"at":1700000000,"hosts":[{"host":"acme.example","aasa":"ok","assetlinks":"none","appleCdn":"stale"}]},"warnings":["one"]}"#;
    let pairing = r#"{"app":"acme","state":"off","off":"not_set_up","served":false,"notServed":"no host","ios":["TEAM.app"],"android":[{"package":"com.acme","sha256":["a"]},{"package":"b","sha256":["a","b"]}],"hosts":[{"host":"acme.example","pairs":true},{"host":"b.example"}],"nativeOrigins":["tauri://localhost"],"linkPaths":["/pair"],"store":{"ios":"https://apps.example/1"},"pairedPhones":2,"staySignedInDays":30,"lastCheck":{"hosts":[{"host":"acme.example","aasa":"ok"}]},"warnings":["w"]}"#;
    let v = vec![
        // covers: app links
        read("links", &["links", "acme"], links),
        read(
            "links status, not served",
            &["links", "status", "a b"],
            r#"{"app":"a b","state":"draft","served":false}"#,
        ),
        // covers: app links --json
        read("links --json", &["links", "acme", "--json"], links),
        page("links: the page", &["links"]),
        usage("links: a flag of set", &["links", "acme", "--ios", "x"]),
        usage("links: a second App", &["links", "a", "b"]),
        usage("links: an unknown flag", &["links", "acme", "--zap"]),
        usage("links: a flag first", &["links", "--zap"]),
        usage("links status: no App", &["links", "status"]),
        traceback(fail(
            "links: a note that is not words",
            &["links", "acme"],
            r#"{"app":"a","warnings":[7]}"#,
        )),
        read(
            "pairing status, off",
            &["pairing", "status", "acme"],
            pairing,
        ),
        read(
            "pairing status, on",
            &["pairing", "status", "acme"],
            r#"{"app":"acme","state":"on","desktopOnly":true}"#,
        ),
        read(
            "pairing status --json",
            &["pairing", "status", "--json", "acme"],
            pairing,
        ),
        read(
            "pairing log",
            &["pairing", "log", "acme"],
            r#"{"app":"acme","pairings":[{"pairedAt":1700000000,"username":"ann","deviceName":"a phone with a very long name indeed","host":"h","live":true},{"username":"bo","host":"h","ended":"signed out"},{"pairedAt":1,"username":"cy"}]}"#,
        ),
        read(
            "pairing log, none",
            &["pairing", "log", "acme"],
            r#"{"name":"Acme"}"#,
        ),
        page("pairing: the page", &["pairing"]),
        usage("pairing: an unknown verb", &["pairing", "zap"]),
        usage("pairing status: no App", &["pairing", "status"]),
        usage(
            "pairing log: an unknown flag",
            &["pairing", "log", "a", "--x"],
        ),
        traceback(fail(
            "pairing: a reason that is a list",
            &["pairing", "status", "acme"],
            r#"{"state":"off","off":[1]}"#,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn push_and_email_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "push");
    let push = r#"{"app":"acme","state":"off","off":"not_approved","apns":{"keyId":"K","teamId":"T","bundleId":"com.acme","keyFingerprint":"f1"},"credError":"expired","devices":{"total":3,"guests":2,"apns":2,"fcm":1},"serverOnly":true,"agentReplies":true,"maxDevices":5,"chatPushes":true,"set":["previews"],"removed":["fcm"]}"#;
    let sends = r#"{"sends":[{"at":1700000000.9999996,"sendId":"s1","queued":3,"sent":2,"skipped":{"muted":1,"off":2}},{"sendId":"s2","skipped":{"a":0.5}}],"pending":4}"#;
    let devices = r#"{"devices":[{"id":"apd_1","username":"a-very-long-user-name-here","platform":"ios","environment":"sandbox","deviceId":"0123456789abcdef0123456789abcdef","lastSeenAt":1700000000},{"id":"apd_2"}]}"#;
    let email = r#"{"app":"acme","on":false,"boxSwitch":"on","sender":{"address":"no-reply@acme.example","fromName":"Acme","path":"relay","state":"ok"},"recipientScope":"members","offReason":"the owner turned it off"}"#;
    let log = r#"{"items":[{"createdAt":1700000000,"id":"em_1","status":"sent","to":["a@acme.example","b@acme.example","c@acme.example"],"subject":"A subject that is longer than sixty characters, to see the cut é"},{"id":"em_2","status":"failed","error":"bounced","to":"x@acme.example","purpose":"reset"},{"id":"em_3","status":"queued"}]}"#;
    let v = vec![
        read("push status", &["push", "status", "acme"], push),
        read(
            "push status, on, chat pushes waiting",
            &["push", "status", "acme"],
            r#"{"name":"Acme","state":"on","fcm":{"projectId":"p","clientEmail":"c@p.example","keyFingerprint":"f2"},"chatPushes":true,"chatPushesBox":false,"previews":1}"#,
        ),
        read(
            "push status --json",
            &["push", "status", "acme", "--json"],
            push,
        ),
        read("push log", &["push", "log", "acme"], sends),
        read(
            "push log, none",
            &["push", "log", "acme"],
            r#"{"sends":[]}"#,
        ),
        read("push devices", &["push", "devices", "acme"], devices),
        read(
            "push devices --guest",
            &["push", "devices", "--guest", "ann é", "acme"],
            r#"{"devices":[]}"#,
        ),
        page("push: the page", &["push"]),
        usage("push: an unknown verb", &["push", "zap"]),
        usage("push devices: no App", &["push", "devices", "--json"]),
        usage("push status: no App", &["push", "status"]),
        Case {
            compare: Compare::Shell,
            ..usage(
                "push devices rm: the shell's",
                &["push", "devices", "rm", "acme"],
            )
        },
        traceback(fail(
            "push log: a skip that is not a number",
            &["push", "log", "acme"],
            r#"{"sends":[{"skipped":{"a":"x"}}]}"#,
        )),
        read("email show", &["email", "show", "acme"], email),
        read(
            "email status-of, no sender, the box off",
            &["email", "status-of", "acme"],
            r#"{"name":"Acme","on":true,"recipientScope":"any"}"#,
        ),
        read(
            "email show --json",
            &["email", "show", "acme", "--json"],
            email,
        ),
        read(
            "email log",
            &[
                "email", "log", "acme", "--limit", "5", "--status", "sent", "--tag", "t é",
            ],
            log,
        ),
        read("email log, none", &["email", "log", "acme"], r#"{}"#),
        read(
            "email status",
            &["email", "status", "acme", "em_0", "em_1"],
            r#"{"id":"em_1","status":"failed","error":"bounced","recipients":2}"#,
        ),
        page("email: the page", &["email"]),
        usage("email: an unknown verb", &["email", "zap"]),
        usage("email status: no id", &["email", "status", "acme"]),
        usage(
            "email log: an unknown flag",
            &["email", "log", "acme", "--x"],
        ),
        traceback(fail(
            "email log: a recipient that is not words",
            &["email", "log", "acme"],
            r#"{"items":[{"to":[7]}]}"#,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
