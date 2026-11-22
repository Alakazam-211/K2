//! The reads of `k5-cli app catalog` against the shell CLI's
//! (`cli/src/app-catalog.sh`), on the parity rig ([`common::parity`]):
//! the same requests, stdout, stderr and exit code.
//!
//! - `catalog [list]`: entries deployed, ready, blocked with reasons,
//!   installed; `--json`; words after `list` are dropped.
//! - `catalog show <entry>`: what it will do and approve, why it can't;
//!   `--json`; usage.
//! - `catalog job <id>`: the steps done, then the end: done (exit 0),
//!   failed with a command or a retry (exit 1), waiting (exit 2), still
//!   running (exit 2); `--json` as it came.
//! - `catalog jobs [--app A]`: the list.
//! - `catalog check <entry> <address> [--id ID]`: free (exit 0), not free
//!   with a suggestion (exit 2), the pro hint; the workspace as
//!   `_compute_ws` names it (this checkout, none in a session); `--json`
//!   (exit 2 when not free); usage; a raise in the ok reader and in the
//!   printer.
//! - `catalog retry <job>`: sent once; `--json`; usage; a refusal; no
//!   daemon.
//! - The page, an unknown verb, an unknown flag, a value flag given last;
//!   Python's raise part way (in each of a job's three steps); a refusal;
//!   a large list.
//! - Help never acts: `-h` or `--help` anywhere prints the page and sends
//!   nothing, but as a value flag's value (`--id -h`), where the shell
//!   sends (a bash bug; the rows flip when the fix lands).
//! - `app install --catalog` ([`install_catalog_answers_as_the_shell_does`]):
//!   the dry run's plan, `--no-wait`, the watch (steps printed once as
//!   they come, then done, failed or waiting), `--json` of each, an
//!   address the daemon won't give (exit 2), a refusal, its loop's
//!   mistakes, and the Python raising.

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

/// One answer, and the exit it ends with.
fn ends(name: &'static str, args: &'static [&'static str], body: &str, code: i32) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, code, 1)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

const LIST: &str = r#"{"entries":[{"entry":"forgejo","name":"Forgejo","version":"9.0.1","defaultAddress":{"url":"https://git.acme.example"},"verified":"signed","license":"MIT","summary":"git hosting é","alreadyDeployed":"git-1"},{"entry":"mealie","name":"Mealie","defaultAddress":{"label":"food"},"license":"AGPL","summary":"recipes","canDeploy":true,"installed":[{"app":"m-1","version":"1.0","variant":"k2"}]},{"entry":"x","name":"X","license":null,"reasons":[{"text":"needs Linux","code":"os","hint":"run it on Linux"},{"code":"mem"}]}],"trust":"pinned by sha256"}"#;

const SHOW: &str = r#"{"name":"Forgejo","summary":"git hosting","entry":"forgejo","publisher":{"name":"Forgejo e.V."},"license":"MIT","installId":"forgejo","defaultAddress":{"label":"git"},"onePerBox":true,"versions":["9.0.1","9.0.0"],"willDo":["pull the image","make a database"],"approves":[{"kind":"boxService","name":"postgres","why":"its data"},{"kind":"cap","name":"net"}],"reasons":[{"text":"no space","code":"disk","hint":"free some"}]}"#;

#[test]
fn list_and_show_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "catalog");
    let v = vec![
        read("catalog", &["catalog"], LIST),
        read(
            "catalog list, a word after it",
            &["catalog", "list", "x"],
            LIST,
        ),
        read("catalog --json", &["catalog", "--json"], LIST),
        read("catalog list --json", &["catalog", "list", "--json"], LIST),
        read("catalog, empty", &["catalog"], r#"{}"#),
        read("catalog show", &["catalog", "show", "forgejo"], SHOW),
        read(
            "catalog show, the least",
            &["catalog", "show", "a b"],
            r#"{"name":"A","summary":"s","entry":"a b","publisher":{"name":"p"},"license":"l","installId":"a","defaultAddress":{"url":"https://a.example"},"version":"1"}"#,
        ),
        read(
            "catalog show --json",
            &["catalog", "show", "--json", "forgejo"],
            SHOW,
        ),
        stop("catalog help: the page", &["catalog", "help"], 0),
        stop("catalog: an unknown verb", &["catalog", "zap"], 2),
        stop("catalog show: no entry", &["catalog", "show"], 2),
        stop("catalog show: two", &["catalog", "show", "a", "b"], 2),
        stop("catalog: an unknown flag", &["catalog", "list", "--x"], 2),
        stop(
            "catalog: --id given last",
            &["catalog", "show", "a", "--id"],
            2,
        ),
        traceback(ends(
            "catalog: a label that is a number",
            &["catalog"],
            r#"{"entries":[{"entry":"e","name":"n","defaultAddress":{"label":5}}]}"#,
            1,
        )),
        traceback(ends(
            "catalog show: no publisher",
            &["catalog", "show", "a"],
            r#"{"name":"A","summary":"s","entry":"a"}"#,
            1,
        )),
        case(
            "catalog: a refusal with its fix line",
            &["catalog"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.into(),
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

#[test]
fn jobs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "catjob");
    let steps = r#""steps":[{"step":"pull","state":"done","note":"image 1.2 GB"},{"step":"db","state":"done"},{"step":"start","state":"running"}]"#;
    let done = format!(
        r#"{{"job":{{"id":"cj-1","entry":"forgejo","app":"git-1","variant":"k2","version":"9.0.1","state":"done","address":{{"url":"https://git.acme.example"}},"next":["open it","add people"],{steps}}}}}"#
    );
    let failed = format!(
        r#"{{"job":{{"id":"cj-2","state":"failed","step":"db","error":{{"code":"db_down","text":"the database did not start","hint":"k2 app db status git-1"}},{steps}}}}}"#
    );
    let retry = r#"{"id":"cj-3","state":"failed","step":"pull","error":{"code":"net","hint":"no network","retry":true}}"#;
    let waiting = r#"{"job":{"id":"cj-4","state":"waiting","step":"approve","waiting":{"reason":"owner","requests":["r-1","r-2"],"text":"the owner approves","hint":"k2 app approve r-1"}}}"#;
    let running = r#"{"job":{"id":"cj-5","state":"running","step":"pull"}}"#;
    let v = vec![
        ends("job: done", &["catalog", "job", "cj-1"], &done, 0),
        ends(
            "job: failed, a command",
            &["catalog", "job", "cj-2"],
            &failed,
            1,
        ),
        ends(
            "job: failed, a retry",
            &["catalog", "jobs", "cj-3"],
            retry,
            1,
        ),
        ends("job: waiting", &["catalog", "job", "cj-4"], waiting, 2),
        ends("job: running", &["catalog", "job", "cj-5"], running, 2),
        ends(
            "job --json",
            &["catalog", "job", "cj-1", "--json"],
            &done,
            0,
        ),
        read(
            "jobs",
            &["catalog", "jobs"],
            r#"{"jobs":[{"id":"cj-1","entry":"forgejo","app":"git-1","state":"done","step":"start"},{"id":"cj-4","entry":"mealie","app":"m-1","state":"waiting","step":"approve","waiting":{"reason":"owner"}},{"id":"cj-2","entry":"x","app":"x-1","state":"failed","step":"db","error":{"code":"db_down"}}]}"#,
        ),
        read(
            "jobs --app wins over an id",
            &["catalog", "job", "cj-1", "--app", "git 1", "--id", "z"],
            r#"{"jobs":[]}"#,
        ),
        read(
            "jobs --json",
            &["catalog", "jobs", "--json"],
            r#"{"jobs":[]}"#,
        ),
        stop("job: --app given last", &["catalog", "job", "--app"], 2),
        traceback(ends(
            "job: a step that is a list",
            &["catalog", "job", "cj-1"],
            r#"{"steps":[{"state":"done","step":[1]}]}"#,
            1,
        )),
        traceback(ends(
            "job: a job that is a list",
            &["catalog", "job", "cj-1"],
            r#"{"job":[1]}"#,
            1,
        )),
        traceback(ends(
            "job: requests that are numbers",
            &["catalog", "job", "cj-1"],
            r#"{"state":"waiting","waiting":{"requests":[1]}}"#,
            1,
        )),
        traceback(ends(
            "jobs: a job with no id",
            &["catalog", "jobs"],
            r#"{"jobs":[{"entry":"e"}]}"#,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// One answer above 256 KiB: thousands of entries.
#[test]
fn catalog_a_large_list() {
    let rig = Rig::new(VERB, "catlarge");
    let rows: Vec<String> = (0..2500)
        .map(|n| {
            format!(
                r#"{{"entry":"entry-{n}","name":"Name {n}","version":"1.{n}","defaultAddress":{{"label":"app-{n}"}},"license":"MIT","summary":"the {n}th App é","canDeploy":true}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"entries":[{}],"trust":"t"}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    for args in [&["catalog"][..], &["catalog", "--json"]] {
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

const FREE: &str = r#"{"ok":true,"app":"forgejo","address":{"label":"git","kind":"label"},"tier":{"hint":"a custom domain é with Pro"}}"#;

const TAKEN: &str =
    r#"{"ok":false,"error":{"code":"address_taken","hint":"git is taken","suggestion":"git-2"}}"#;

#[test]
fn check_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "catcheck");
    let v = vec![
        read("check: free", &["catalog", "check", "forgejo", "git"], FREE),
        read(
            "check: free at a URL, --id",
            &[
                "catalog",
                "check",
                "forgejo",
                "https://git.acme.example",
                "--id",
                "g 1",
            ],
            r#"{"ok":1,"app":"g 1","address":{"url":"https://git.acme.example","label":"x","kind":"domain"}}"#,
        ),
        ends(
            "check: not free, a suggestion",
            &["catalog", "check", "forgejo", "git"],
            TAKEN,
            2,
        ),
        ends(
            "check: not free, no suggestion, ok missing",
            &["catalog", "check", "forgejo", "git"],
            r#"{"error":{"code":"address_reserved"}}"#,
            2,
        ),
        read(
            "check --json: free",
            &["catalog", "check", "--json", "forgejo", "git"],
            FREE,
        ),
        ends(
            "check --json: not free",
            &["catalog", "check", "forgejo", "git", "--json"],
            TAKEN,
            2,
        ),
        Case {
            env: with_passport(&[]),
            ..case(
                "check: in a session, no workspace named",
                &["catalog", "check", "forgejo", "git"],
                &[],
                vec![ok(FREE)],
                port_only,
                0,
                1,
            )
        },
        stop("check: one word", &["catalog", "check", "forgejo"], 2),
        stop(
            "check: three words",
            &["catalog", "check", "a", "b", "c"],
            2,
        ),
        stop(
            "check: --id given last",
            &["catalog", "check", "a", "b", "--id"],
            2,
        ),
        traceback(ends(
            "check: an answer that is a list",
            &["catalog", "check", "a", "b"],
            "[1]",
            1,
        )),
        traceback(ends(
            "check: free with no address",
            &["catalog", "check", "a", "b"],
            r#"{"ok":true}"#,
            1,
        )),
        case(
            "check: a refusal",
            &["catalog", "check", "a", "b", "--json"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.into(),
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

#[test]
fn retry_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "catretry");
    let dead = dead_port();
    let v = vec![
        read("retry", &["catalog", "retry", "cj-3"], r#"{"ok":true}"#),
        read(
            "retry --json, --app dropped",
            &["catalog", "retry", "--json", "cj é", "--app", "x"],
            r#"{"job":{"id":"cj-3","state":"running"}}"#,
        ),
        read("retry: an empty id", &["catalog", "retry", ""], r#"{}"#),
        stop("retry: no id", &["catalog", "retry"], 2),
        stop("retry: two ids", &["catalog", "retry", "a", "b"], 2),
        stop(
            "retry: an unknown flag",
            &["catalog", "retry", "a", "--x"],
            2,
        ),
        case(
            "retry: a 400",
            &["catalog", "retry", "cj-9"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"not_retryable","hint":"it ran"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "retry: no daemon",
            &["catalog", "retry", "cj-3"],
            &[("K2_PORT", dead.as_str())],
            vec![],
            owner_only,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "cathelp");
    for args in [
        &["catalog", "retry", "cj-3", "-h"][..],
        &["catalog", "retry", "--help", "cj-3"],
        &["catalog", "check", "a", "b", "-h"],
        &["catalog", "check", "--help"],
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..stop("help never acts", args, 0)
        });
    }
    // Right after `--id` or `--app` too: help, nothing sent (the shell's
    // fix; the write was sent with -h as the value).
    for args in [
        &["catalog", "retry", "cj-3", "--id", "-h"][..],
        &["catalog", "retry", "cj-3", "--app", "--help"],
        &["catalog", "job", "--app", "-h"],
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..stop("help right after a value flag", args, 0)
        });
    }
}

const PLAN: &str = r#"{"plan":{"name":"Forgejo","version":"9.0.1","variant":"k2","app":"git-1","address":{"label":"git"},"willDo":["pull the image","make a database é"],"approves":[{"kind":"boxService","name":"postgres","why":"its data"},{"kind":"cap","name":"net"}],"setup":["admin","mail"],"steps":["pull","db","deploy"],"waits":[{"reason":"owner","text":"the owner approves","hint":"k2 app approve r1"},{"reason":"dns","hint":"point the name"}],"files":{"tag":"v9","file":"forgejo.tar","sha256":"ab12"}},"tier":"pro"}"#;
const STARTED: &str = r#"{"job":{"id":"cj-7","state":"running"}}"#;
const RUNNING: &str = r#"{"job":{"id":"cj-7","state":"running","step":"db","steps":[{"step":"pull","state":"done","note":"9.0.1 é"},{"step":"db","state":"running"}]}}"#;
const DONE: &str = r#"{"job":{"id":"cj-7","state":"done","entry":"forgejo","app":"git-1","variant":"k2","version":"9.0.1","address":{"url":"https://git.acme.example"},"steps":[{"step":"pull","state":"done","note":"9.0.1 é"},{"step":"db","state":"done"},{"step":"deploy","state":"done","note":"live"}],"next":["open it"]}}"#;
const FAILED: &str = r#"{"job":{"id":"cj-7","state":"failed","step":"db","error":{"code":"db_down","text":"the database is down","retry":true},"steps":[{"step":"pull","state":"done"}]}}"#;
const WAITING: &str = r#"{"job":{"id":"cj-7","state":"waiting","step":"approve","waiting":{"reason":"owner","requests":["r1"],"text":"the owner approves","hint":"k2 app approve r1"},"steps":[]}}"#;
const TAKEN_FULL: &str = r#"{"error":{"code":"address_taken","hint":"git is taken","suggestion":"git-2","holder":"","existing":{"app":"x"}}}"#;

#[test]
fn install_catalog_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "catinst");
    let v = vec![
        // native: app install
        read(
            "dry run: the plan",
            &[
                "install",
                "--catalog",
                "forgejo",
                "--dry-run",
                "--address",
                "git",
                "--variant",
                "k2",
            ],
            PLAN,
        ),
        // native: app install --json
        read(
            "dry run --json",
            &[
                "install",
                "--catalog=forgejo",
                "--dry-run",
                "--json",
                "git-1",
            ],
            PLAN,
        ),
        // native: app install
        read(
            "--no-wait: the job's name",
            &[
                "install",
                "--catalog",
                "forgejo",
                "--no-wait",
                "--id=git-1",
                "--version",
                "9.0.1",
                "--domain",
                "acme.example",
                "--rebind-meet",
            ],
            STARTED,
        ),
        // native: app install --json
        read(
            "--no-wait --json",
            &[
                "install",
                "--catalog",
                "forgejo",
                "--no-wait",
                "--json",
                "--workspace",
                "ws-b",
            ],
            STARTED,
        ),
        // native: app install
        case(
            "the watch: done at once",
            &["install", "acme", "--catalog", "forgejo"],
            &[],
            vec![ok(STARTED), ok(DONE)],
            owner_and_port,
            0,
            2,
        ),
        // native: app install
        case(
            "the watch: running, then done; each step once",
            &["install", "--catalog", "forgejo"],
            &[],
            vec![ok(STARTED), ok(RUNNING), ok(DONE)],
            owner_and_port,
            0,
            3,
        ),
        // native: app install
        case(
            "the watch: failed",
            &["install", "--catalog", "forgejo"],
            &[],
            vec![ok(STARTED), ok(FAILED)],
            owner_and_port,
            1,
            2,
        ),
        // native: app install --json
        case(
            "the watch --json: waiting",
            &["install", "--catalog", "forgejo", "--json"],
            &[],
            vec![ok(STARTED), ok(WAITING)],
            owner_and_port,
            2,
            2,
        ),
        // native: app install
        case(
            "the watch: a read refused",
            &["install", "--catalog", "forgejo"],
            &[],
            vec![
                ok(STARTED),
                Reply::Answer(
                    404,
                    r#"{"error":{"code":"no_such_job","hint":"gone"}}"#.to_string(),
                ),
            ],
            owner_and_port,
            3,
            2,
        ),
        // native: app install
        case(
            "an address that is taken",
            &["install", "--catalog", "forgejo", "--address", "git"],
            &[],
            vec![Reply::Answer(409, TAKEN_FULL.to_string())],
            owner_and_port,
            2,
            1,
        ),
        // native: app install
        case(
            "refused",
            &["install", "--catalog", "forgejo"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner."}}"#.to_string(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // native: app install
        case(
            "an error that is a string",
            &["install", "--catalog", "forgejo"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":"address_taken"}"#.to_string(),
            )],
            owner_and_port,
            2,
            1,
        ),
        // native: app install
        stop("no entry", &["install", "acme", "--catalog="], 2),
        // native: app install
        stop(
            "an unknown flag",
            &["install", "--catalog", "x", "--zap"],
            2,
        ),
        // native: app install
        stop("a second id", &["install", "--catalog", "x", "a", "b"], 2),
        // native: app install
        stop(
            "a variant it doesn't know",
            &["install", "--catalog", "x", "--variant", "y"],
            2,
        ),
        // native: app install
        stop(
            "--version last",
            &["install", "--catalog", "x", "--version"],
            2,
        ),
        // native: app install
        traceback(ends(
            "no job in the answer",
            &["install", "--catalog", "forgejo"],
            r#"{"ok":true}"#,
            1,
        )),
        // native: app install
        traceback(case(
            "a step that is not a string",
            &["install", "--catalog", "forgejo"],
            &[],
            vec![
                ok(STARTED),
                ok(
                    r#"{"job":{"id":"cj-7","state":"running","steps":[{"step":5,"state":"done"}]}}"#,
                ),
            ],
            owner_and_port,
            1,
            2,
        )),
        // native: app install
        traceback(ends(
            "a plan with no address",
            &["install", "--catalog", "forgejo", "--dry-run"],
            r#"{"plan":{"name":"F"}}"#,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
    // native: app install
    rig.check(&Case {
        compare: Compare::Shell,
        ..stop(
            "help never acts",
            &["install", "--catalog", "x", "--dry-run", "-h"],
            0,
        )
    });
    // A help word where `--address` wants its value is help: the shell's
    // fix (-h or --help anywhere on app catalog prints help and sends
    // nothing) holds here too, so nothing is sent.
    // native: app install
    rig.check(&Case {
        compare: Compare::Shell,
        ..stop(
            "help as a value is help: nothing sent",
            &[
                "install",
                "--catalog",
                "x",
                "--no-wait",
                "--address",
                "--help",
            ],
            0,
        )
    });
}

/// `install --catalog`'s lines name the command by the brand's name (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2`
/// or `k5`; the variant's `k2` is a wire value and stays.
#[test]
fn install_catalog_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "catbrand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want) in [
        (
            &["install", "--catalog="][..],
            format!("usage: {cli} app install --catalog <entry> [--address LABEL|auto] [--dry-run] (entries: {cli} app catalog)\n"),
        ),
        (
            &["install", "--catalog", "x", "a", "b"],
            format!("usage: {cli} app install --catalog <entry> [--id ID] [--address LABEL|auto] ({cli} app catalog --help)\n"),
        ),
        (
            &["install", "--catalog", "x", "--zap"],
            format!("Unknown flag: --zap ({cli} app catalog --help)\n"),
        ),
        (
            &["install", "--catalog", "x", "--variant", "y"],
            "--variant is k2 or standalone\n".to_string(),
        ),
    ] {
        let c = stop("brand", args, 2);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
    let c = read(
        "brand: started",
        &["install", "--catalog", "x", "--no-wait"],
        r#"{"job":{"id":"cj-1"}}"#,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stdout,
        format!(
            "Started job cj-1 (it runs in the daemon). Follow it: {cli} app catalog job cj-1\n"
        )
    );
}
