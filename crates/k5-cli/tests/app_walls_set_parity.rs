//! The writes of `k5-cli app egress|box-service|db` against the shell
//! CLI's (`cli/src/app-db.sh`, `cli/src/app-box.sh`), on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code.
//!
//! - `egress add <app> <host>...`: allowed and already allowed (0), held
//!   for the owner (2), not applied (3); `--json` (2 when held); usage;
//!   a refusal; Python's raise.
//! - `egress rm|remove`, `egress mode` (set, held, `--json` 202 and 200,
//!   an answer that isn't a dict: two tracebacks and exit 0).
//! - `box-service request` (allowed by name, held, `--json`) and
//!   `box-service revoke|rm`.
//! - `db connect`: PASS, FAIL (1), skipped; held (2) with its card;
//!   `--json`; `--wait` until approved, failed (1), a card that ended
//!   (3) or the time ran out (2); a 400 (1), a 409 (3); usage (1).
//! - `db check` and `db disconnect` (an error in the answer is exit 1).
//! - Help never acts; no daemon. stdin is not a terminal here, so a held
//!   request is never approved on the spot.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "app";

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

/// One answer with `status`, and the exit it ends with.
fn one(name: &'static str, args: &'static [&'static str], status: u16, body: &str, code: i32) -> Case {
    case(
        name,
        args,
        &[],
        vec![Reply::Answer(status, body.to_string())],
        owner_and_port,
        code,
        1,
    )
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

const ALLOWED: &str = r#"{"status":"allowed","added":["api.example:443","b é:443"],"alreadyAllowed":["c.example:443"],"result":{"note":"live at once"}}"#;

const HELD: &str = r#"{"status":"held","request":{"id":"req-1","summary":"reach api.example:443 é"},"alreadyAllowed":["c.example:443"],"reused":true,"askedAgo":"5m","card":{"where":"Settings, App Approvals"},"hint":"the owner approves it"}"#;

const REFUSED: &str = r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#;

#[test]
fn egress_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "egset");
    let v = vec![
        one(
            "egress add: allowed",
            &["egress", "add", "acme", "api.example", "b é", "--reason", "sync é"],
            200,
            ALLOWED,
            0,
        ),
        one(
            "egress add: already allowed, no note",
            &["egress", "add", "acme", "c.example"],
            200,
            r#"{"status":"allowed","alreadyAllowed":["c.example:443"],"hint":"nothing new"}"#,
            0,
        ),
        one("egress add: held", &["egress", "add", "acme", "api.example"], 202, HELD, 2),
        one(
            "egress add: not applied",
            &["egress", "add", "acme", "api.example"],
            200,
            r#"{"status":"failed","result":{"code":"nft_failed"}}"#,
            3,
        ),
        one(
            "egress add --json: held",
            &["egress", "add", "--json", "acme", "api.example"],
            202,
            HELD,
            2,
        ),
        one(
            "egress add --json: allowed",
            &["egress", "add", "acme", "api.example", "--json"],
            200,
            ALLOWED,
            0,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "egress add: held in a session",
                &["egress", "add", "acme", "api.example"],
                &[],
                vec![Reply::Answer(202, HELD.into())],
                port_only,
                2,
                1,
            )
        },
        one(
            "egress add: a refusal",
            &["egress", "add", "acme", "api.example"],
            403,
            REFUSED,
            3,
        ),
        traceback(one(
            "egress add: held with no request",
            &["egress", "add", "acme", "api.example"],
            202,
            r#"{"status":"held"}"#,
            1,
        )),
        traceback(one(
            "egress add --json: an answer that is a list",
            &["egress", "add", "acme", "h", "--json"],
            200,
            "[1]",
            1,
        )),
        stop("egress add: no App", &["egress", "add"], 2),
        stop("egress add: --reason given last", &["egress", "add", "a", "h", "--reason"], 1),
        stop("egress add: an unknown flag", &["egress", "add", "a", "h", "--app-port", "1"], 2),
        one(
            "egress rm: removed",
            &["egress", "rm", "acme", "api.example:443"],
            200,
            r#"{"removed":true,"host":"api.example:443"}"#,
            0,
        ),
        one(
            "egress remove: not on the list",
            &["egress", "remove", "acme", "x"],
            200,
            r#"{"removed":false}"#,
            0,
        ),
        one(
            "egress rm: its hint",
            &["egress", "rm", "acme", "x"],
            200,
            r#"{"hint":"x is the manifest's é"}"#,
            0,
        ),
        one(
            "egress rm --json",
            &["egress", "rm", "acme", "x", "--json"],
            200,
            r#"{"removed":true,"host":"x"}"#,
            0,
        ),
        stop("egress rm: two hosts", &["egress", "rm", "acme", "a", "b"], 2),
        one(
            "egress mode: set",
            &["egress", "mode", "acme", "broad", "--reason", "r"],
            200,
            r#"{"mode":"broad"}"#,
            0,
        ),
        one(
            "egress mode: held",
            &["egress", "mode", "acme", "broad"],
            202,
            HELD,
            2,
        ),
        one(
            "egress mode --json: held",
            &["egress", "mode", "--json", "acme", "broad"],
            202,
            r#"{"status":"held"}"#,
            2,
        ),
        one(
            "egress mode --json: set",
            &["egress", "mode", "acme", "allowlist", "--json"],
            200,
            r#"{"mode":"allowlist"}"#,
            0,
        ),
        traceback(one(
            "egress mode: an answer that is a list",
            &["egress", "mode", "acme", "broad"],
            200,
            "[]",
            0,
        )),
        stop("egress mode: no mode", &["egress", "mode", "acme"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn box_service_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "bsset");
    let v = vec![
        one(
            "box-service request: allowed",
            &[
                "box-service",
                "request",
                "acme",
                "postgres",
                "5432",
                "--app-port",
                "15432",
                "--reason",
                "its data é",
            ],
            200,
            r#"{"status":"allowed","services":[{"name":"postgres"},"x",{"name":""}],"result":{"note":"restart it"}}"#,
            0,
        ),
        one(
            "box-service request: held",
            &["box-services", "request", "acme", "redis"],
            202,
            HELD,
            2,
        ),
        one(
            "box-service request --json: held",
            &["box-service", "request", "acme", "redis", "--json"],
            202,
            HELD,
            2,
        ),
        one(
            "box-service request: a refusal",
            &["box-service", "request", "acme", "redis"],
            403,
            REFUSED,
            3,
        ),
        stop("box-service request: no name", &["box-service", "request", "acme"], 2),
        stop(
            "box-service request: three words",
            &["box-service", "request", "acme", "a", "1", "2"],
            2,
        ),
        stop("box-service request: no App", &["box-service", "request"], 2),
        one(
            "box-service revoke: revoked",
            &["box-service", "revoke", "acme", "postgres"],
            200,
            r#"{"revoked":true,"name":"postgres"}"#,
            0,
        ),
        one(
            "box-service rm: not one",
            &["box-service", "rm", "acme", "x"],
            200,
            r#"{}"#,
            0,
        ),
        one(
            "box-service revoke --json",
            &["box-service", "revoke", "acme", "x", "--json"],
            200,
            r#"{"revoked":false,"hint":"x é"}"#,
            0,
        ),
        traceback(one(
            "box-service revoke: a name that is missing",
            &["box-service", "revoke", "acme", "x"],
            200,
            r#"{"revoked":true}"#,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

const PASS: &str = r#"{"app":"acme","created":true,"db":"acme_main","major":17,"cluster":"main","roles":["acme_login","acme_owner"],"secrets":["app-db.acme.DATABASE_URL","app-db.acme.PGPASSWORD"],"kind":"container","doorOpen":true,"replaced":["app-db.acme.OLD_URL"],"note":"migrations ran é","rehost":{"result":"done","deployment":"dep-2"},"switchesTurnedOn":["app.db"],"check":{"ok":true,"version":"17.2","latencyMs":4,"from":"app"}}"#;

const FAIL: &str = r#"{"app":"acme","db":"reports","major":16,"cluster":"old","roles":[],"kind":"process-exception","rehost":{"result":"failed","error":"no image"},"check":{"ok":false,"step":"login","error":"password refused","hint":"rotate it","appOutput":["boot é","FATAL: auth"]}}"#;

const DB_HELD: &str = r#"{"request":{"id":"card-1"},"diff":{"lines":["connect acme to reports","reports belongs to workspace ops é","x"]},"switches":[{"name":"Apps databases","where":"Settings","on":false},{"name":"y","on":true}],"reused":true,"hint":"the owner approves it"}"#;

#[test]
fn db_connect_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "dbcon");
    let v = vec![
        // covers: app db connect
        one("db connect --new: PASS", &["db", "connect", "acme", "--new", "--major", "17"], 200, PASS, 0),
        one(
            "db connect --db: FAIL",
            &[
                "db",
                "connect",
                "acme",
                "--db=reports",
                "--cluster",
                "old",
                "--name",
                "n",
                "--as",
                "REPORTS_URL",
                "--no-migrate-url",
                "--reason",
                "r é",
            ],
            200,
            FAIL,
            1,
        ),
        one(
            "db connect: skipped, plain",
            &["db", "connect", "acme", "--db", "d"],
            200,
            r#"{"app":"acme","db":"d","kind":"process","check":{"skipped":"not running"}}"#,
            0,
        ),
        // covers: app db connect --json
        one("db connect --json: PASS", &["db", "connect", "--json", "acme", "--new"], 200, PASS, 0),
        one("db connect --json: FAIL", &["db", "connect", "acme", "--new", "--json"], 200, FAIL, 1),
        one("db connect: held", &["db", "connect", "acme", "--new"], 202, DB_HELD, 2),
        one(
            "db connect: held, the diff's first line",
            &["db", "connect", "acme", "--new"],
            202,
            r#"{"request":{"id":"card-2"},"diff":{"lines":["connect é"]}}"#,
            2,
        ),
        one("db connect --json: held", &["db", "connect", "acme", "--new", "--json"], 202, DB_HELD, 2),
        traceback(one(
            "db connect: held with no request",
            &["db", "connect", "acme", "--new"],
            202,
            r#"{"diff":{}}"#,
            1,
        )),
        traceback(one(
            "db connect: secrets that are numbers",
            &["db", "connect", "acme", "--new"],
            200,
            r#"{"app":"a","created":false,"secrets":[5]}"#,
            1,
        )),
        stop("db connect: which database", &["db", "connect", "acme"], 1),
        stop("db connect: no App", &["db", "connect", "--new"], 1),
        stop("db connect: a bad major", &["db", "connect", "acme", "--new", "--major=9"], 1),
        one(
            "db connect: a 400",
            &["db", "connect", "acme", "--new"],
            400,
            r#"{"error":{"code":"no_postgres","hint":"Linux only"}}"#,
            1,
        ),
        one(
            "db connect: a 409",
            &["db", "connect", "acme", "--new"],
            409,
            r#"{"error":{"code":"app_busy","hint":"x"}}"#,
            3,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `--wait --timeout 5`: one read of the status, five seconds on.
#[test]
fn db_connect_waits_for_the_owner() {
    let rig = Rig::new(VERB, "dbwait");
    let status = |s: &str| ok(&format!(r#"{{"request":{{"id":"card-1",{s}}}}}"#));
    let wait = |name: &'static str, args: &'static [&'static str], then: Reply, code: i32| {
        case(
            name,
            args,
            &[],
            vec![Reply::Answer(202, DB_HELD.into()), then],
            owner_and_port,
            code,
            2,
        )
    };
    let v = vec![
        wait(
            "--wait: approved",
            &["db", "connect", "acme", "--new", "--wait", "--timeout", "5"],
            status(&format!(r#""status":"approved","result":{PASS}"#)),
            0,
        ),
        wait(
            "--wait --json: approved, FAIL",
            &["db", "connect", "acme", "--new", "--wait", "--timeout", "5s", "--json"],
            status(&format!(r#""status":"approved","result":{FAIL}"#)),
            1,
        ),
        wait(
            "--wait: failed",
            &["db", "connect", "acme", "--new", "--wait", "--timeout", "5"],
            status(r#""status":"failed","result":{"code":"pg_down","hint":"the cluster é"}"#),
            1,
        ),
        wait(
            "--wait: denied",
            &["db", "connect", "acme", "--new", "--wait", "--timeout", "5"],
            status(r#""status":"denied""#),
            3,
        ),
        wait(
            "--wait: still pending, the time runs out",
            &["db", "connect", "acme", "--new", "--wait", "--timeout", "5"],
            status(r#""status":"pending""#),
            2,
        ),
        wait(
            "--wait: a status read refused, the time runs out",
            &["db", "connect", "acme", "--new", "--wait", "--timeout", "5"],
            Reply::Answer(403, REFUSED.into()),
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn db_check_and_disconnect_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "dbchk");
    let v = vec![
        // covers: app db check
        one("db check: PASS", &["db", "check", "acme", "--as", "DATABASE_URL"], 200, PASS, 0),
        one("db check: FAIL", &["db", "check", "acme"], 200, FAIL, 1),
        // covers: app db check --json
        one("db check --json: FAIL", &["db", "check", "acme", "--json"], 200, FAIL, 1),
        one(
            "db check: a 400",
            &["db", "check", "acme"],
            400,
            r#"{"error":{"code":"not_connected","hint":"connect it first"}}"#,
            1,
        ),
        stop("db check: two Apps", &["db", "check", "a", "b"], 1),
        // covers: app db disconnect
        one(
            "db disconnect",
            &["db", "disconnect", "acme", "--as", "REPORTS_URL", "--drop-roles"],
            200,
            r#"{"ok":true,"app":"acme","db":"reports","as":"REPORTS_URL","roles":["r_login"],"sessionsCut":2,"rolesDropped":true,"doorOpen":true,"secrets":["app-db.acme.REPORTS_URL"],"note":"done é"}"#,
            0,
        ),
        one(
            "db disconnect: an error in the answer",
            &["db", "disconnect", "acme"],
            200,
            r#"{"app":"acme","db":"d","as":"A","error":{"hint":"the role is in use"}}"#,
            1,
        ),
        // covers: app db disconnect --json
        one(
            "db disconnect --json: not ok",
            &["db", "disconnect", "acme", "--json"],
            200,
            r#"{"ok":false}"#,
            1,
        ),
        traceback(one(
            "db disconnect: secrets that are numbers",
            &["db", "disconnect", "acme"],
            200,
            r#"{"app":"a","db":"d","as":"A","secrets":[1]}"#,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "wshelp");
    for args in [
        &["egress", "add", "acme", "h", "-h"][..],
        &["egress", "mode", "--help", "acme", "broad"],
        &["egress", "rm", "acme", "h", "--help"],
        &["box-service", "request", "acme", "pg", "-h"],
        &["box-service", "revoke", "acme", "pg", "--help"],
        &["box-service", "request", "acme", "pg", "--reason", "-h"],
        &["db", "connect", "acme", "--new", "-h"],
        &["db", "check", "--help", "acme"],
        &["db", "disconnect", "acme", "-h"],
    ] {
        rig.check(&shell(stop("help never acts", args, 0)));
    }
    // A value flag takes the next word as it comes, so `-h` there is a
    // value and the write is sent (bash bugs, app-db.sh:313 and
    // app-db.sh:101-106; the rows flip when the fixes land).
    for args in [
        &["egress", "add", "acme", "h", "--reason", "-h"][..],
        &["egress", "mode", "acme", "broad", "--reason", "--help"],
        &["db", "check", "acme", "--as", "-h"],
        &["db", "disconnect", "acme", "--reason", "--help"],
    ] {
        rig.check(&shell(one(
            "help as a value acts (bash bug)",
            args,
            200,
            r#"{"status":"allowed","mode":"broad","app":"acme","db":"d","as":"A","ok":true}"#,
            0,
        )));
    }
}

#[test]
fn no_daemon() {
    let rig = Rig::new(VERB, "wsdead");
    let dead = dead_port();
    for args in [
        &["egress", "add", "acme", "h"][..],
        &["box-service", "revoke", "acme", "pg"],
        &["db", "check", "acme"],
    ] {
        rig.check(&case(
            "no daemon",
            args,
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ));
    }
}
