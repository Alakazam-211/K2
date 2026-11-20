//! `k5-cli app updates ls|list|stats|fleet` against the shell CLI's
//! (`cli/src/app-updates.sh`), on the parity rig ([`common::parity`]):
//! the same requests, stdout, stderr and exit code.
//!
//! - `ls`: a key, channels, releases with files and an error, the
//!   endpoint; a bare feed; `--channel`, `--release`, `--json`; a list
//!   above 256 KiB; the usage lines (no App, two), an unknown flag,
//!   `--channel` last (exit 1).
//! - `stats`: downloads and active users by version, unavailable
//!   downloads, `--days`, `--json`; a second App.
//! - `fleet`: the Apps, none, `--json`; a word it doesn't take.
//! - The page (`updates`, `updates help`) and an unknown verb's usage
//!   line; a verb that is still the shell's (`pull`).
//! - Python's raise part way; a refusal.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";
const LS: &str = r#"{"app":"acme","dlId":"dl-1","key":{"keyId":"K1"},"channels":[{"channel":"stable","version":"1.2.0"},{"channel":"beta"}],"activeUsers":{"today":3,"last7Days":9,"counting":"devices"},"releases":[{"id":"r1","version":"1.2.0","state":"live","channels":["stable"],"size":12582912,"activeUsers":3,"files":[{"platform":"darwin-aarch64","size":10485760,"url":"https://dl.example/a"},{"platform":"windows-x86_64","size":2097152.5,"upload":"pending"}]},{"id":"r2","version":"1.3.0-rc.1 é","state":"failed","error":"bad sig"}],"endpoints":{"dynamic":"https://dl.example/{{target}}"}}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn ls_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "upls");
    let v = vec![
        // covers: app updates ls
        case(
            "a feed",
            &["updates", "ls", "acme"],
            &[],
            vec![ok(LS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a bare feed",
            &["updates", "list", "a b"],
            &[],
            vec![ok(r#"{"app":"a b"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--channel and --release",
            &[
                "updates",
                "ls",
                "--channel",
                "beta é",
                "acme",
                "--release",
                "1.2.0",
            ],
            &[],
            vec![ok(LS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app updates ls --json
        case(
            "--json",
            &["updates", "ls", "acme", "--json"],
            &[],
            vec![ok(LS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no App",
            &["updates", "ls"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two Apps",
            &["updates", "ls", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["updates", "ls", "acme", "--zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--channel last: set -e, exit 1",
            &["updates", "ls", "acme", "--channel"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        traceback(case(
            "a size that is not a number",
            &["updates", "ls", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","releases":[{"version":"1","state":"s","size":"big"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a refusal with its fix line",
            &["updates", "ls", "acme"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"x"}}"#.into(),
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

/// One answer above 256 KiB: thousands of releases.
#[test]
fn ls_a_large_list() {
    let rig = Rig::new(VERB, "uplsbig");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"id":"r{n}","version":"1.0.{n}","state":"live","channels":["c{n}"],"size":{},"activeUsers":{n},"files":[{{"platform":"linux-x86_64","size":{},"url":"https://dl.example/{n}"}}]}}"#,
                n * 1000,
                n * 7
            )
        })
        .collect();
    let body = format!(r#"{{"app":"acme","releases":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [
        &["updates", "ls", "acme"][..],
        &["updates", "ls", "acme", "--json"],
    ] {
        rig.check(&case(
            "a large list",
            args,
            &[],
            vec![ok(&body)],
            owner_and_port,
            0,
            1,
        ));
    }
}

#[test]
fn stats_fleet_and_the_page_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "upst");
    let stats = r#"{"app":"acme","from":"2026-10-01","to":"2026-10-31","downloads":{"available":true,"total":42,"byVersion":[{"version":"1.2.0","downloads":40},{"version":"1.1.0","downloads":2.9}]},"activeUsers":{"today":3,"last7Days":9,"counting":"checks","checks":120,"byVersion":{"1.2.0":8,"1.1.0":1,"é":0}}}"#;
    let fleet = r#"{"apps":[{"app":"acme","dlId":"dl-1","channels":[{"channel":"stable","version":"1.2.0"}],"releases":2,"activeUsers":{"last7Days":9}},{"app":"globex","channels":[],"releases":0,"activeUsers":{"last7Days":0}}]}"#;
    let v = vec![
        // covers: app updates stats
        case(
            "stats",
            &["updates", "stats", "acme"],
            &[],
            vec![ok(stats)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stats --days, unavailable",
            &["updates", "stats", "--days", "7", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","from":"f","to":"t","downloads":{"why":"no host"},"activeUsers":{"today":0,"last7Days":0,"counting":"checks","checks":0}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: app updates stats --json
        case(
            "stats --json",
            &["updates", "stats", "acme", "--json"],
            &[],
            vec![ok(stats)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stats: two Apps",
            &["updates", "stats", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "stats: a total that is not a number",
            &["updates", "stats", "acme"],
            &[],
            vec![ok(
                r#"{"app":"a","from":"f","to":"t","downloads":{"available":true,"total":"many"}}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        // covers: app updates fleet
        case(
            "fleet",
            &["updates", "fleet"],
            &[],
            vec![ok(fleet)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "fleet, none",
            &["updates", "fleet"],
            &[],
            vec![ok(r#"{"apps":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: app updates fleet --json
        case(
            "fleet --json",
            &["updates", "fleet", "--json"],
            &[],
            vec![ok(fleet)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "fleet: a word",
            &["updates", "fleet", "acme"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("the page", &["updates"], &[], vec![], owner_and_port, 0, 0),
        case(
            "the page by help",
            &["updates", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown verb",
            &["updates", "zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            compare: Compare::Shell,
            ..case(
                "pull is the shell's",
                &["updates", "pull", "acme"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}
