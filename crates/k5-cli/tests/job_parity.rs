//! `k5-cli job` against the shell CLI's `cli/k2 job`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code
//! for every form.
//!
//! - The help: `job` alone, `help`, `--help`, `-h` anywhere before `--`,
//!   with no daemon set up, behind the virtual desktop's guard, and in a
//!   session with no passport.
//! - The words: a value flag with no value or an empty one, an unknown
//!   flag, a verb with no name, an unknown verb, `add` with no command,
//!   `pause` with neither a name nor `--all`; and the gates in front of
//!   them.
//! - Every verb and alias, with `--json` and without: the bodies (every
//!   flag, the command after `--`, unicode), the queries; many rows (an
//!   answer above 256 KiB); the readers' odd answers (not JSON, a field
//!   of another type).
//! - A refusal (400, 401, 403 with a permission card: no closing line,
//!   404, 413, 500 with no body or not JSON).
//! - The owner from disk, a passport over TCP, a live cell socket (a GET
//!   goes on to TCP after no answer there, a POST only after "not
//!   served"; a POST that may have run is never sent again), a dead
//!   socket file, a bulletin (no line: `_job_call` notes nothing).
//! - No answer (an empty reply, a reset, not HTTP) and no daemon.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "job";

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn list() -> String {
    r#"{"paused":true,"pausedBy":"owner é","jobs":[
        {"workspace":"acme","name":"backup","schedule":"every 15m","enabled":true,
         "lastRun":{"outcome":"ok","startedAt":"2026-11-19T10:15:00Z"},"command":"sh -c 'tar czf /tmp/x.tgz . && echo done with a long tail of words'"},
        {"workspace":"globex","name":"n日本","schedule":"*/5 * * * *","running":"r1","command":"x"},
        {"workspace":"acme","name":"off","schedule":null,"enabled":false,"lastRun":null}]}"#
        .to_string()
}

/// Thousands of rows, above 256 KiB.
fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"workspace":"ws{}","name":"job-{i}","schedule":"every {}m","enabled":{},"lastRun":{{"outcome":"ok","startedAt":"2026-11-19T10:{:02}:00Z"}},"command":"{}"}}"#,
                i % 3,
                i % 60 + 1,
                i % 2 == 0,
                i % 60,
                "c".repeat(i % 80)
            )
        })
        .collect();
    let body = format!(r#"{{"jobs":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    body
}

fn show() -> String {
    r#"{"job":{"name":"backup","workspace":"acme","schedule":"every 1h","running":"r9","next":"2026-11-20T00:00:00Z","command":"ls -la","cwd":"sub","timeoutSecs":600,
        "createdBy":"owner","createdAt":"t1","updatedBy":"acme","updatedAt":"t2",
        "grants":[{"secret":"API_KEY","delivery":"env","envName":"API_KEY_V"},{"secret":"CERT","delivery":"file"}],
        "runs":[{"id":"r9","outcome":"running","trigger":"schedule","exitCode":null,"startedAt":"t3"},
                {"id":"r8","outcome":"failed","trigger":"manual","exitCode":2,"startedAt":"t0"}]}}"#
        .to_string()
}

fn logs() -> String {
    r#"{"run":{"id":"r8","startedAt":"t0","outcome":"failed","exitCode":2,"bytes":20},"truncated":true,"log":"line one é\n[k2v:API_KEY]\n"}"#.to_string()
}

#[test]
fn job_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        // covers: job
        ("the help: job alone", &[][..]),
        ("help", &["help"]),
        ("--help", &["--help"]),
        ("-h after a verb", &["list", "-h"]),
        ("--help before --", &["add", "n", "--help", "--", "x"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(case(
        "the help with no daemon set up",
        &["show", "--help"],
        &[],
        vec![],
        nothing,
        0,
        0,
    ));
    v.push(case(
        "the help behind the virtual desktop's guard",
        &[],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "--help passes the guard",
        &["--help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        0,
        0,
    ));
    v.push(case(
        "the help in a session with no passport",
        &["help"],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    for (name, args) in [
        // covers: job --json
        ("job --json: a verb with no name", &["--json"][..]),
        ("add with no name", &["add"]),
        ("add with no command", &["add", "n"]),
        (
            "add with an empty command",
            &["add", "n", "--every", "1m", "--"],
        ),
        ("a value flag at the end", &["list", "--every"]),
        ("an empty value", &["show", "n", "--cwd", ""]),
        ("an unknown flag", &["list", "--bogus"]),
        ("--json=1 is no flag", &["list", "--json=1"]),
        ("an unknown verb", &["nosuch"]),
        ("an unknown verb with a name", &["nosuch", "x"]),
        ("show with no name", &["show"]),
        ("logs with only a flag", &["logs", "--json"]),
        ("pause with nothing", &["pause"]),
        ("resume with nothing", &["resume", "--json"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    v.push(case(
        "a usage error with no daemon set up: the gate first",
        &["add"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "the guard",
        &["list"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    // After `--` a help word is the command's: the guard stops the line
    // (`_k2_offline_verb` stops at `--`, 75dfa4fc07).
    v.push(case(
        "-h after -- is the command's: the guard",
        &["add", "n", "--every", "1m", "--", "ls", "-h"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "a session with no passport",
        &["list"],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn job_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let done = |body: &str| ok(body);
    let add = r#"{"job":{"name":"backup","schedule":"every 15m","command":"sh -c x","next":"2026-11-20T00:15:00Z"}}"#;
    let v = vec![
        // covers: job list
        case(
            "list",
            &["list"],
            &[],
            vec![reply(list())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ls --all --workspace",
            &["ls", "--all", "--workspace", "w é&x+y"],
            &[],
            vec![reply(list())],
            owner_and_port,
            0,
            1,
        ),
        // covers: job list --json
        case(
            "list --json: the body as it came",
            &["list", "--json"],
            &[],
            vec![ok("{\"jobs\":[]}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, none",
            &["list"],
            &[],
            vec![ok(r#"{"jobs":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, thousands of rows",
            &["list"],
            &[],
            vec![reply(many(3000))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json, thousands of rows",
            &["list", "--json"],
            &[],
            vec![reply(many(3000))],
            owner_and_port,
            0,
            1,
        ),
        // covers: job show
        case(
            "show",
            &["show", "backup"],
            &[],
            vec![reply(show())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, a bare job",
            &["show", "n", "--run", "r1"],
            &[],
            vec![ok(
                r#"{"job":{"name":"n","updatedBy":"x","updatedAt":1,"createdAt":1.0}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: job show --json
        case(
            "show --json",
            &["show", "n", "--json"],
            &[],
            vec![reply(show())],
            owner_and_port,
            0,
            1,
        ),
        // covers: job logs
        case(
            "logs --run",
            &["logs", "backup", "--run", "r 8"],
            &[],
            vec![reply(logs())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log, no run yet",
            &["log", "n"],
            &[],
            vec![ok(r#"{"log":""}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job logs --json
        case(
            "logs --json",
            &["logs", "n", "--json"],
            &[],
            vec![reply(logs())],
            owner_and_port,
            0,
            1,
        ),
        // covers: job add
        case(
            "add, every flag",
            &[
                "add",
                "backup",
                "--every",
                "15m",
                "--cron",
                "*/15 * * * *",
                "--timeout",
                "10m",
                "--cwd",
                "sub dir",
                "--workspace",
                "acme é",
                "--",
                "sh",
                "-c",
                "echo \"é ☃\" --json",
            ],
            &[],
            vec![done(add)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job add --json
        case(
            "add --json",
            &["add", "n", "--json", "--every", "1m", "--", "true"],
            &[],
            vec![done(add)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job edit
        case(
            "edit, a new command",
            &["edit", "n", "--timeout", "1h", "--", "ls"],
            &[],
            vec![done(
                r#"{"job":{"name":"n","schedule":"every 1h","command":"ls"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: job run
        case(
            "run",
            &["run", "n"],
            &[],
            vec![done(r#"{"job":"n","run":"r10"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job run --json
        case(
            "run-now",
            &["run-now", "n", "--json"],
            &[],
            vec![done(r#"{"job":"n","run":"r10"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job enable
        case(
            "enable",
            &["enable", "n"],
            &[],
            vec![done(r#"{"job":{"name":"n","enabled":true}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job disable
        case(
            "disable",
            &["disable", "n"],
            &[],
            vec![done(r#"{"job":{"name":"n","enabled":false}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job kill
        case(
            "kill",
            &["kill", "n"],
            &[],
            vec![done(r#"{"job":"n","run":"r10"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job rm
        case(
            "rm",
            &["rm", "n"],
            &[],
            vec![done(r#"{"removed":"n","grantsRevoked":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remove",
            &["remove", "n"],
            &[],
            vec![done(r#"{"removed":"n","grantsRevoked":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "delete",
            &["delete", "n"],
            &[],
            vec![done(r#"{"removed":"n","grantsRevoked":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job pause
        case(
            "pause --all",
            &["pause", "--all"],
            &[],
            vec![done(r#"{"paused":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job resume
        case(
            "resume one",
            &["resume", "n"],
            &[],
            vec![done(r#"{"job":{"name":"n","enabled":true}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "resume --all, an answer with neither",
            &["resume", "--all"],
            &[],
            vec![done(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job enable --json
        case(
            "enable --json",
            &["enable", "n", "--json"],
            &[],
            vec![ok(r#"{"job":{"name":"n","enabled":true}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job disable --json
        case(
            "disable --json",
            &["disable", "n", "--json"],
            &[],
            vec![ok(r#"{"job":{"name":"n","enabled":false}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job rm --json
        case(
            "rm --json",
            &["rm", "n", "--json"],
            &[],
            vec![ok(r#"{"removed":"n","grantsRevoked":2}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job pause --json
        case(
            "pause --all --json",
            &["pause", "--all", "--json"],
            &[],
            vec![ok(r#"{"paused":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: job resume --json
        case(
            "resume one --json",
            &["resume", "n", "--json"],
            &[],
            vec![ok(r#"{"job":{"name":"n","enabled":true}}"#)],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "list, not JSON",
            &["list"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "list, a workspace that is a list",
            &["list"],
            &[],
            vec![ok(r#"{"jobs":[{"workspace":[1]},{"workspace":"a"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "show, a grant with a number for its name",
            &["show", "n"],
            &[],
            vec![ok(r#"{"job":{"grants":[{"envName":5}]}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "add, an answer with no job",
            &["add", "n", "--every", "1m", "--", "x"],
            &[],
            vec![ok(r#"{"job":"n"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "logs, a log that is a number",
            &["logs", "n"],
            &[],
            vec![ok(r#"{"log":5}"#)],
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
fn job_refusals() {
    let rig = Rig::new(VERB, "refuse");
    let v = vec![
        case(
            "403 with a permission card: no closing line",
            &["add", "n", "--every", "1m", "--", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "401",
            &["list"],
            &[],
            vec![Reply::Answer(
                401,
                r#"{"error":"Invalid or missing auth token"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // covers: job edit --json
        case(
            "400, --json prints nothing",
            &["edit", "n", "--json", "--every", "bad"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_schedule","hint":"say every 15m"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "413",
            &["add", "n", "--every", "1m", "--", "x"],
            &[],
            vec![Reply::Answer(413, r#"{"error":{"hint":"too big"}}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "404",
            &["show", "nope"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":null}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "500, no body",
            &["kill", "n"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, not JSON",
            &["run", "n"],
            &[],
            vec![Reply::Answer(500, format!("boom é\r\n{}", "x".repeat(400)))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "403, an error that is a list",
            &["list"],
            &[],
            vec![Reply::Answer(403, r#"{"error":["a",1]}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "201 is no answer the shell takes",
            &["list"],
            &[],
            vec![Reply::Answer(201, r#"{"jobs":[]}"#.into())],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn job_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            with_passport(&[]),
            case(
                "a host session: the passport over TCP, no workspace",
                &["list"],
                &[],
                vec![reply(list())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a bulletin in the answer: no line",
                &["list"],
                &[],
                vec![ok(
                    r#"{"jobs":[],"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
                )],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a refused passport: no heal, no probe",
                &["run", "n"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a GET",
                &["show", "n"],
                &[],
                vec![reply(show())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a POST",
                &["add", "n", "--workspace", "w", "--every", "1m", "--", "x"],
                &[],
                vec![ok(
                    r#"{"job":{"name":"n","schedule":"every 1m","command":"x"}}"#,
                )],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served there: a POST goes on to TCP",
                &["enable", "n"],
                &[],
                vec![not_served(), ok(r#"{"job":{"name":"n","enabled":true}}"#)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "no answer there: a GET goes on to TCP",
                &["list"],
                &[],
                vec![Reply::Close, reply(list())],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(&[]),
            case(
                "no answer there: a POST is never sent again",
                &["run", "n"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        waited(with(
            cell(&[]),
            // covers: job kill --json
            case(
                "a reset there: a POST is never sent again",
                &["kill", "n", "--json"],
                &[],
                vec![Reply::Reset],
                port_only,
                1,
                1,
            ),
        )),
        with(
            cell(&[]),
            case(
                "a refusal there is the answer",
                &["disable", "n"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: a POST goes on to TCP",
                &["pause", "--all"],
                &[],
                vec![ok(r#"{"paused":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn job_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let port = dead_port();
    let v = vec![
        case(
            "an empty reply",
            &["list"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["add", "n", "--every", "1m", "--", "x"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["show", "n"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens",
            &["run", "n"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `jobs` is the same command.
#[test]
fn jobs_is_job() {
    let rig = Rig::new("jobs", "alias");
    rig.check(&case(
        "jobs list",
        &["ls"],
        &[],
        vec![reply(list())],
        owner_and_port,
        0,
        1,
    ));
    rig.check(&case("jobs alone", &[], &[], vec![], owner_and_port, 0, 0));
    rig.check(&case(
        "jobs with an unknown verb",
        &["nosuch"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    ));
}

/// Help never acts: each verb that changes something, with `-h` or
/// `--help` anywhere before `--`, prints the help and sends nothing.
/// After `--` a help flag is the job's command's own word (bash's
/// `cmd_job` stops looking there), so that line adds the job.
#[test]
fn job_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for verb in [
        "add", "edit", "run", "run-now", "enable", "disable", "kill", "rm", "remove", "delete",
        "pause", "resume",
    ] {
        for (flag, at) in [("-h", 0), ("--help", 1), ("-h", 2)] {
            let mut words = vec![verb, "n", "--every", "1m"];
            words.insert(at + 1, flag);
            let args: &'static [&'static str] = Box::leak(words.into_boxed_slice());
            let name: &'static str = Box::leak(format!("{verb} {flag} at {at}").into_boxed_str());
            v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
        }
    }
    // The front hands every line with -h or --help to the shell (S0's
    // front.rs): both sides' requests are the shell's.
    v.push(Case {
        compare: Compare::Shell,
        ..case(
            "a help flag after -- is the command's: the job is added",
            &["add", "n", "--every", "1m", "--", "ls", "-h"],
            &[],
            vec![ok(
                r#"{"job":{"name":"n","schedule":"every 1m","command":"ls -h"}}"#,
            )],
            owner_and_port,
            0,
            1,
        )
    });
    for c in v {
        rig.check(&c);
    }
}
