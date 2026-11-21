//! The reads of `k5-cli app egress|box-service|db status` against the
//! shell CLI's (`cli/src/app-db.sh`, `cli/src/app-box.sh`), on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - `egress [show] <app>`: hosts (a v6 one, a request), pending, the
//!   proxy listening or not, not enforced, none; `--json`; the page, the
//!   page on stderr with no App, a host word, an unknown flag, a value
//!   flag given last.
//! - `egress log <app> [--tail N]`: ok and refused lines.
//! - `box-service <app>`: services forwarded, direct, missing, pending,
//!   presets; none; `--json`; the page, a second word, a flag.
//! - `db status <app>`: connections checked, failed, not checked; no
//!   Postgres; pending cards; `--json`; its JSON usage lines (exit 1); a
//!   bad `--timeout` (exit 2); a 400 (exit 1) and a 409 (exit 3).
//! - A large list; Python's raise part way; no daemon. The writes are
//!   app_walls_set_parity.rs'.

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

fn read(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

/// Python raises: exit 1, after the one request.
fn fail(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    traceback(case(name, args, &[], vec![ok(body)], owner_and_port, 1, 1))
}

/// Nothing sent: the line and its exit.
fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

const EGRESS: &str = r#"{"app":"acme","mode":"allowlist","enforced":false,"note":"new hosts wait for the owner","hosts":[{"status":"allowed","host":"api.example","port":443,"source":"manifest","hits":12,"refused":0},{"status":"held","host":"2001:db8::1","port":8443,"source":"agent é","hits":0,"refused":3,"requestId":"req-1"}],"pending":[{"id":"req-1","summary":"reach 2001:db8::1","requestedBy":"ann"}],"proxy":{"listening":true,"port":40001,"active":1,"connections":9,"refused":3}}"#;

const BOX: &str = r#"{"app":"acme","wall":"container","note":"forwards into the wall","services":[{"status":"allowed","name":"postgres","hostPort":5432,"inWall":"127.0.0.1:5432","env":"PGHOST","forwarded":true},{"status":"allowed","name":"redis","hostPort":6379,"inWall":"127.0.0.1:6379","env":"REDIS_URL","forwarded":false,"requestId":"r-2"},{"status":"held","name":"mail","hostPort":25,"inWall":"-","env":"-","forwarded":"direct"},{"status":"x","name":"y","hostPort":1,"inWall":"z","env":"e"}],"pending":[{"id":"r-3","summary":"reach mail","requestedBy":"bo"}],"presets":[{"name":"ollama","hostPort":11434,"from":"preset","what":"local models"},{"name":"bad","error":"not running"}],"refused":"the daemon's own ports"}"#;

const DB: &str = r#"{"app":"acme","kind":"container","running":true,"postgres":true,"connections":[{"as":"DATABASE_URL","db":"acme_main","major":17,"cluster":"main","roles":["acme_login","acme_owner"],"doorOpen":true,"check":{"ok":true,"latencyMs":3}},{"as":"REPORTS_URL","db":"reports","major":16,"cluster":"old","doorOpen":false,"check":{"ok":false,"step":"login"}},{"as":"X","db":"y","roles":[]}],"pending":[{"id":"card-1","summary":"connect reports é"}],"majors":[16,17]}"#;

#[test]
fn egress_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "egress");
    let v = vec![
        read("egress", &["egress", "acme"], EGRESS),
        read(
            "egress, none, enforced, no proxy",
            &["egress", "acme"],
            r#"{"app":"acme","mode":"broad","enforced":true}"#,
        ),
        read("egress --json", &["egress", "--json", "acme"], EGRESS),
        read(
            "egress log",
            &["egress", "log", "acme", "--tail", "50", "--reason", "r"],
            r#"{"lines":[{"at":1700000000,"result":"ok","kind":"https","host":"api.example","port":443,"up":10,"down":2000,"ms":41,"end":"closed"},{"at":1700000001.5,"result":"refused","kind":"http","host":"evil.example","code":"not_allowed"},{"at":2,"result":"error"}]}"#,
        ),
        read(
            "egress log, none",
            &["egress", "log", "acme", "extra"],
            r#"{}"#,
        ),
        read(
            "egress log --json",
            &["egress", "log", "--json", "acme"],
            r#"{"lines":[]}"#,
        ),
        stop("egress: the page", &["egress"], 0),
        stop("egress help: the page", &["egress", "help"], 0),
        stop(
            "egress: no App, the page on stderr",
            &["egress", "--json"],
            2,
        ),
        stop("egress log: no App", &["egress", "log"], 2),
        stop("egress: a host word", &["egress", "acme", "api.example"], 2),
        stop("egress: an unknown flag", &["egress", "acme", "--zap"], 2),
        stop(
            "egress: --tail given last",
            &["egress", "acme", "--tail"],
            2,
        ),
        fail(
            "egress: a host that is a number",
            &["egress", "acme"],
            r#"{"app":"a","mode":"m","hosts":[{"host":7}]}"#,
        ),
        fail(
            "egress log: a time that is words",
            &["egress", "log", "acme"],
            r#"{"lines":[{"at":"x"}]}"#,
        ),
        case(
            "egress: a refusal with its fix line",
            &["egress", "acme"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        stop("egress add: no host", &["egress", "add", "acme"], 2),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn box_service_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "boxsvc");
    let v = vec![
        read("box-service", &["box-service", "acme"], BOX),
        read(
            "box-services, none",
            &["box-services", "acme", "--reason", "r", "--app-port", "1"],
            r#"{"app":"acme","wall":"process"}"#,
        ),
        read(
            "box-service --json",
            &["box-service", "acme", "--json"],
            BOX,
        ),
        stop("box-service: the page", &["box-service"], 0),
        stop("box-service: no App", &["box-service", "--json"], 2),
        stop(
            "box-service: a second word",
            &["box-service", "acme", "pg"],
            2,
        ),
        stop(
            "box-service: an unknown flag",
            &["box-service", "acme", "--x"],
            2,
        ),
        stop(
            "box-service: --reason given last",
            &["box-service", "acme", "--reason"],
            2,
        ),
        fail(
            "box-service: a note that is a number",
            &["box-service", "acme"],
            r#"{"app":"a","wall":"w","note":5}"#,
        ),
        stop(
            "box-service revoke: no name",
            &["box-service", "revoke", "acme"],
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn db_status_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "dbst");
    let dead = dead_port();
    let v = vec![
        // covers: app db status
        read("db status", &["db", "status", "acme"], DB),
        read(
            "database status, no Postgres, none",
            &[
                "database",
                "status",
                "acme",
                "--timeout",
                "1h30m",
                "--major",
                "17",
                "--as=X",
                "--wait",
            ],
            r#"{"app":"acme","kind":"process"}"#,
        ),
        // covers: app db status --json
        read("db status --json", &["db", "status", "--json", "acme"], DB),
        stop("db: the page", &["db"], 0),
        stop("db: an unknown verb", &["db", "zap"], 1),
        stop("db status: no App", &["db", "status"], 1),
        stop("db status: two Apps", &["db", "status", "a", "b é"], 1),
        stop(
            "db status: a value missing",
            &["db", "status", "a", "--db"],
            1,
        ),
        stop(
            "db status: an empty value",
            &["db", "status", "a", "--as", ""],
            1,
        ),
        stop(
            "db status: an unknown flag",
            &["db", "status", "a", "--name=x"],
            1,
        ),
        stop(
            "db status: a bad major",
            &["db", "status", "a", "--major", "7"],
            1,
        ),
        stop(
            "db status: a bad duration",
            &["db", "status", "a", "--timeout", "5x"],
            2,
        ),
        case(
            "db status: a 400",
            &["db", "status", "acme"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_app","hint":"no App é"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "db status: a 400 that is not JSON",
            &["db", "status", "acme"],
            &[],
            vec![Reply::Answer(400, "oops".into())],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "db status: a 400 whose error is words",
            &["db", "status", "acme"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad"}"#.into())],
            owner_and_port,
            1,
            1,
        )),
        case(
            "db status: a 409",
            &["db", "status", "acme", "--json"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"app_exists","hint":"x"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        fail(
            "db status: roles that are a number",
            &["db", "status", "acme"],
            r#"{"app":"a","connections":[{"as":"A","db":"d","roles":5}]}"#,
        ),
        case(
            "db status: no daemon",
            &["db", "status", "acme"],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
        stop("db connect: no App", &["db", "connect"], 1),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer above 256 KiB: thousands of egress log lines.
#[test]
fn egress_log_a_large_list() {
    let rig = Rig::new(VERB, "eglarge");
    let rows: Vec<String> = (0..4000)
        .map(|n| {
            format!(
                r#"{{"at":{},"result":"ok","kind":"https","host":"host-{n}.example","port":443,"up":{n},"down":{},"ms":7,"end":"closed é"}}"#,
                1_700_000_000 + n,
                n * 3
            )
        })
        .collect();
    let body = format!(r#"{{"lines":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [
        &["egress", "log", "acme"][..],
        &["egress", "log", "acme", "--json"],
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
