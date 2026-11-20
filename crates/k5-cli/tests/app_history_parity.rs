//! `k5-cli app history|errors|logs` against the shell CLI's
//! (`cli/src/app-doctor.sh`), on the parity rig ([`common::parity`]): the
//! same requests, stdout, stderr and exit code.
//!
//! - `history`: deployments (live, can roll back, across a migration, a
//!   Seed freed, why and error lines), what K2 keeps and what it can roll
//!   back to, a broken chain; `--users`, `--widget`, `--limit`, `--json`;
//!   a list above 256 KiB; the usage line.
//! - `errors`: failures (exit, signal, restarts, rolled back), events,
//!   the framed lines, `--kind` (no events), `--since`, `--json`; a bad
//!   kind, `--follow`, usage.
//! - `logs`: one read, `--deployment`, `--tail`; `--client` (bundles,
//!   none, `--user`, `--kind`, `--limit`, `--json`) and its usage lines;
//!   the `--client`-only flags without it. (`--follow` is the shell's:
//!   the family's unit tests check the hand-off.)
//! - `logs --client --since`: days, a duration, plain seconds and a bad
//!   one (exit 2). The time sent is now less the window: the shell reads
//!   now from a stub `date` first on PATH, the binary from the fixed time
//!   its debug build takes for tests (`APP_LOGS_TEST_NOW`).
//! - Python's raise part way.

#![cfg(unix)]

mod common;

use common::parity::*;

const VERB: &str = "app";
const HISTORY: &str = r#"{"app":"acme","liveSeed":"seed-b-0123456789","deployments":[{"startedAt":1700000000,"kind":"deploy","result":"live","fromSeed":"seed-a-0123456789","toSeed":"seed-b-0123456789","actor":"ann","commitRef":"abcdef0123456789","rollbackable":false},{"startedAt":1699990000,"kind":"deploy","result":"replaced","toSeed":"seed-a-0123456789","actor":"bo é","rollbackable":true,"freedSoon":true,"reason":"a fix"},{"startedAt":0,"kind":"rollback","result":"failed","actor":"cy","crossesMigration":true,"error":"boom"},{"startedAt":1699980000,"kind":"deploy","result":"replaced","toSeed":"s","actor":"d","liveAt":1,"seedStored":false}],"keepDeployments":5,"keepUnusedDays":30,"rollbackTo":[{"seed":"seed-a-0123456789","startedAt":1699990000}],"auditVerified":true}"#;
const ERRORS: &str = r#"{"failures":[{"atMs":1700000000123,"kind":"crash","deployment":"d-1","exitCode":0,"signal":9,"restartCount":2,"rolledBack":true,"reason":"oom é"},{"atMs":1700000001000,"kind":"health","reason":"timeout"}],"events":[{"at":1700000002,"kind":"deploy","deployment":"d-2","result":"failed","error":"bad"}],"framed":"--- untrusted ---\nline\n--- end ---"}"#;

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn history_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "hist");
    let v = vec![
        case(
            "deployments",
            &["history", "acme"],
            &[],
            vec![ok(HISTORY)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nothing kept, a broken chain",
            &["history", "acme", "--limit", "5"],
            &[],
            vec![ok(
                r#"{"app":"acme","keepDeployments":5,"keepUnusedDays":30.5}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--users",
            &["history", "a b", "--users"],
            &[],
            vec![ok(
                r#"{"users":true,"audit":[{"at":1700000000,"event":"login","actor":"ann","payload":{"z":1,"a":"é"}}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--users, none",
            &["history", "acme", "--users"],
            &[],
            vec![ok(r#"{"users":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--widget",
            &["history", "--widget", "w é"],
            &[],
            vec![ok(
                r#"{"widget":"w","audit":[{"at":0,"event":"e","actor":"a","payload":null}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--widget, none",
            &["history", "--widget", "w"],
            &[],
            vec![ok(r#"{"widget":"w","audit":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["history", "acme", "--json"],
            &[],
            vec![ok(HISTORY)],
            owner_and_port,
            0,
            1,
        ),
        case("usage", &["history"], &[], vec![], owner_and_port, 2, 0),
        case(
            "--limit last: set -e, exit 1",
            &["history", "acme", "--limit"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        traceback(case(
            "a deployment with no kind",
            &["history", "acme"],
            &[],
            vec![ok(
                r#"{"deployments":[{"startedAt":1,"result":"live","actor":"a"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer above 256 KiB: thousands of deployments.
#[test]
fn history_a_large_list() {
    let rig = Rig::new(VERB, "histbig");
    let rows: Vec<String> = (0..3000)
        .map(|n| {
            format!(
                r#"{{"startedAt":{},"kind":"deploy","result":"replaced","fromSeed":"seed-{n:020}","toSeed":"seed-{n:020}","actor":"agent {n} é","commitRef":"c{n:030}"}}"#,
                1_700_000_000 + n
            )
        })
        .collect();
    let body = format!(r#"{{"app":"acme","deployments":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["history", "acme"][..], &["history", "acme", "--json"]] {
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
fn errors_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "errs");
    let v = vec![
        case(
            "failures and events",
            &["errors", "acme"],
            &[],
            vec![ok(ERRORS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nothing",
            &["errors", "acme", "--since", "2h"],
            &[],
            vec![ok(r#"{"framed":""}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--kind: no events",
            &["errors", "--kind", "crash", "a b"],
            &[],
            vec![ok(ERRORS
                .replace(r#""failures""#, r#""kind":"crash","failures""#)
                .as_str())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json",
            &["errors", "acme", "--json"],
            &[],
            vec![ok(ERRORS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a bad kind",
            &["errors", "acme", "--kind", "oops"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--follow",
            &["errors", "acme", "--follow"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "usage",
            &["errors", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "a time that is not a number",
            &["errors", "acme"],
            &[],
            vec![ok(
                r#"{"failures":[{"atMs":"now","kind":"crash","reason":"r"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "no framed lines",
            &["errors", "acme"],
            &[],
            vec![ok(r#"{"events":[]}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn logs_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "logs");
    let framed = r#"{"framed":"--- untrusted ---\nhello é\n--- end ---"}"#;
    let client = r#"{"app":"acme","count":2,"keepDays":14,"framed":"--- untrusted ---\nbundle\n--- end ---"}"#;
    let v = vec![
        case(
            "one read",
            &["logs", "acme"],
            &[],
            vec![ok(framed)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--deployment and --tail",
            &["logs", "--tail", "50", "acme", "--deployment", "d 1"],
            &[],
            vec![ok(framed)],
            owner_and_port,
            0,
            1,
        ),
        case("usage", &["logs"], &[], vec![], owner_and_port, 2, 0),
        case(
            "--client flags without --client",
            &["logs", "acme", "--kind", "crash"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--json without --client",
            &["logs", "acme", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--client",
            &["logs", "acme", "--client"],
            &[],
            vec![ok(client)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--client, every filter",
            &[
                "logs", "--client", "acme", "--user", "ann é", "--kind", "log", "--limit", "5",
            ],
            &[],
            vec![ok(r#"{"app":"acme","count":0,"keepDays":14}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--client --json",
            &["logs", "acme", "--client", "--json"],
            &[],
            vec![ok(client)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--client: no App",
            &["logs", "--client"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--client: a bad kind",
            &["logs", "acme", "--client", "--kind", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--client: a bad limit",
            &["logs", "acme", "--client", "--limit", "5x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "no framed lines",
            &["logs", "acme"],
            &[],
            vec![ok(r#"{}"#)],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `date +%s` and the binary's clock, both fixed at [`NOW`].
const NOW: &str = "1800000000";

#[test]
fn logs_client_since_answers_as_the_shell_does() {
    use std::os::unix::fs::PermissionsExt;
    let mut rig = Rig::new(VERB, "since");
    let shim = rig.dir.join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    let date = shim.join("date");
    std::fs::write(
        &date,
        format!(
            "#!/bin/sh\nif [ \"$*\" = +%s ]; then echo {NOW}; exit 0; fi\nPATH=/usr/bin:/bin exec date \"$@\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&date, std::fs::Permissions::from_mode(0o755)).unwrap();
    rig.path_first = Some(shim);
    let client = r#"{"app":"acme","count":1,"keepDays":14,"framed":"--- untrusted ---\nbundle\n--- end ---"}"#;
    let since = |name: &'static str, args: &'static [&'static str], code: i32, requests: usize| {
        case(
            name,
            args,
            &[("APP_LOGS_TEST_NOW", NOW)],
            if requests == 0 {
                vec![]
            } else {
                vec![ok(client)]
            },
            owner_and_port,
            code,
            requests,
        )
    };
    let v = vec![
        since(
            "--since days",
            &["logs", "acme", "--client", "--since", "2d"],
            0,
            1,
        ),
        since(
            "--since a duration, every filter",
            &[
                "logs", "acme", "--client", "--since", "1h30m", "--user", "ann", "--kind", "crash",
                "--limit", "9",
            ],
            0,
            1,
        ),
        since(
            "--since seconds, --json",
            &["logs", "acme", "--client", "--since", "0300", "--json"],
            0,
            1,
        ),
        since(
            "--since a bad duration",
            &["logs", "acme", "--client", "--since", "2w"],
            2,
            0,
        ),
        since(
            "--since days in a bad form",
            &["logs", "acme", "--client", "--since", "d"],
            2,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}
