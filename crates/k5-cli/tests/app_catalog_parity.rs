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
//! - The page, an unknown verb, an unknown flag, a value flag given last;
//!   `check` and `retry` stay the shell's; Python's raise part way (in
//!   each of a job's three steps); a refusal; a large list.

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
        read("catalog list, a word after it", &["catalog", "list", "x"], LIST),
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
        stop("catalog: --id given last", &["catalog", "show", "a", "--id"], 1),
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
        Case {
            compare: Compare::Shell,
            ..stop("catalog retry: the shell's", &["catalog", "retry"], 2)
        },
        Case {
            compare: Compare::Shell,
            ..stop("catalog check: the shell's", &["catalog", "check", "a"], 2)
        },
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
        ends("job: failed, a command", &["catalog", "job", "cj-2"], &failed, 1),
        ends("job: failed, a retry", &["catalog", "jobs", "cj-3"], retry, 1),
        ends("job: waiting", &["catalog", "job", "cj-4"], waiting, 2),
        ends("job: running", &["catalog", "job", "cj-5"], running, 2),
        ends("job --json", &["catalog", "job", "cj-1", "--json"], &done, 0),
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
        read("jobs --json", &["catalog", "jobs", "--json"], r#"{"jobs":[]}"#),
        stop("job: --app given last", &["catalog", "job", "--app"], 1),
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
