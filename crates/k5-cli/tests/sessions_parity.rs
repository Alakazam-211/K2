//! `k5-cli sessions` against the shell CLI's `cli/k2 sessions`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every local path of its read forms.
//!
//! - The roster (no verb, `--all`, `--json`, `list`, `ls`, `--workspace`),
//!   past conversations (`--past`, `past`, `--search`, `--days`, `--cli`,
//!   `--limit`, `--archived`, a stopped search, notes on stderr), `show`
//!   (`--tail`, messages, the runner, unsupported) and `main` (each state);
//!   many rows, unicode, long titles, times in each of `ago`'s units.
//! - The words: an unknown verb, an unknown flag, `show` with no id or
//!   two, `main` with a bad `--thread` or a word it doesn't take.
//! - The status: a refusal (403, with a permission card's closing line),
//!   400, 413, a 404 that is a missing route (`daemon_too_old`) and one
//!   that is not, 500, a 201; a body that is not JSON, or empty, or only
//!   `000`s.
//! - The owner from disk, a passport over TCP, a live cell socket (not
//!   served, no answer, a reset and a dead socket file go on to TCP; a
//!   refusal there is the answer); the id tool's checks; a re-minted and a
//!   dead passport; the gates.
//! - No answer (an empty reply, a 5xx with no body, a reset, not HTTP, none
//!   in time) and no daemon (each `_k2_curl` try's `\n000`).
//! - What the binary hands to the shell: help, the verbs that change
//!   something, `archives`, `compact`, `spawn`, a value flag as the last
//!   word, a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "sessions";

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// A time `secs` ago, in the middle of its `ago` unit.
fn ago(secs: f64) -> String {
    format!("{:.3}", now() - secs)
}

fn roster() -> String {
    format!(
        r#"{{"workspace":{{"handle":"acme"}},"sidecars":{{"cap":4,"live":1}},"asleep":1,"sessions":[
        {{"address":"acme","role":"main","cli":"claude","state":"working","lastActiveAt":{},"title":"Fix the thing\nnow é 日本","you":true}},
        {{"address":"acme/helper-with-a-long-name","role":"sidecar","cli":"codex","state":"idle","lastActiveAt":{},"title":"{}"}},
        {{"handle":"acme/h","state":"waiting","lastActiveAt":{}}},
        {{"address":"acme/d","state":"working","lastActiveAt":{}}}]}}"#,
        ago(20.0),
        ago(30.0 * 60.0),
        "ü".repeat(90),
        ago(5.0 * 3600.0 + 1800.0),
        ago(3.0 * 86400.0 + 43200.0)
    )
}

fn many_rows(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"address":"acme/s{i}","role":"sidecar","cli":"pi","state":"idle","title":"{}"}}"#,
                "t".repeat(i % 70)
            )
        })
        .collect();
    format!(r#"{{"sessions":[{}]}}"#, rows.join(","))
}

fn past() -> String {
    format!(
        r#"{{"workspace":"acme","days":30,"search":"bug é","cli":"codex","truncated":true,"scanned":{{"files":12}},
        "unsupported":[{{"cli":"pi","why":"no logs here"}}],"conversations":[
        {{"id":"ab12cd","cli":"claude","lastActiveAt":{},"worktree":"a-very-long-worktree-name-here","title":"Old","customName":"Named é","runner":{{"address":"acme/x"}},"match":"the bug is here"}},
        {{"id":"ef34","cli":"codex","title":null}}]}}"#,
        ago(2.0 * 86400.0 + 3600.0)
    )
}

fn show() -> String {
    let t = ago(10.0 * 60.0 + 30.0);
    format!(
        r#"{{"conversation":{{"id":"ab12","cli":"claude","title":"T é","lastActiveAt":{t},"worktree":"wt","branch":"b1","runner":{{"state":"live","address":"acme/x"}}}},"messageCount":3,"firstAsk":"do it\nplease","messages":[{{"role":"user","at":{t},"text":"hi"}},{{"role":"assistant","text":"ok 🚀"}}],"toolCalls":2}}"#
    )
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

#[test]
fn sessions_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: sessions
        case(
            "the roster",
            &[],
            &[],
            vec![reply(roster())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--all and --workspace",
            &["--all", "--workspace", "w é&x"],
            &[],
            vec![reply(roster())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty --workspace is the project",
            &["list", "--workspace", ""],
            &[],
            vec![reply(roster())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ls",
            &["ls", "-a"],
            &[],
            vec![reply(roster())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions --json
        case(
            "--json: the body as it came",
            &["--json"],
            &[],
            vec![ok("{\"sessions\":[]}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--json, an empty body",
            &["--json"],
            &[],
            vec![ok("")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "nobody, some asleep",
            &[],
            &[],
            vec![ok(r#"{"asleep":3,"sidecars":{"cap":2.9}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many rows",
            &[],
            &[],
            vec![reply(many_rows(300))],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions --past
        case(
            "past conversations",
            &["--past"],
            &[],
            vec![reply(past())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions --past --json
        case(
            "past, every flag",
            &[
                "past",
                "--search",
                "a b&c",
                "--days",
                "90",
                "--cli",
                "codex",
                "--limit",
                "5",
                "--archived",
                "--json",
            ],
            &[],
            vec![reply(past())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--search alone means past",
            &["--search", "x", "--all"],
            &[],
            vec![ok(r#"{"conversations":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "past, every one held",
            &["--past"],
            &[],
            vec![ok(
                r#"{"conversations":[{"id":"a","cli":"pi","runner":{"state":"asleep"}}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions show
        case(
            "show",
            &["show", "ab12", "--tail", "2"],
            &[],
            vec![reply(show())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions show --json
        case(
            "show --json",
            &["show", "--json", "a/b é"],
            &[],
            vec![reply(show())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show, unsupported",
            &["show", "x"],
            &[],
            vec![ok(
                r#"{"conversation":{"id":"x","cli":"pi"},"unsupported":"pi keeps no logs"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions main
        case(
            "main, none",
            &["main"],
            &[],
            vec![ok(r#"{"workspace":"acme","main":{"state":"none"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "main, missing: exit 1",
            &["main", "--workspace", "w"],
            &[],
            vec![ok(
                r#"{"workspace":"acme","main":{"state":"missing","conversationId":"c1","cli":"codex"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "main, unstarted",
            &["main", "--thread", "adopt-sidecar"],
            &[],
            vec![ok(
                r#"{"main":{"state":"unstarted","conversationId":"c1","cli":"codex"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "main, custom",
            &["main"],
            &[],
            vec![ok(r#"{"main":{"state":"custom"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sessions main --json
        case(
            "main, a path",
            &["main", "--json"],
            &[],
            vec![ok(
                r#"{"workspace":"w","main":{"state":"ok","conversationId":"c","cli":"claude","path":"/p/x.jsonl"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "main, a path as text",
            &["main"],
            &[],
            vec![ok(
                r#"{"workspace":"w","main":{"state":"ok","conversationId":"c","cli":"claude","path":"/p/é x.jsonl"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // The words.
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("an empty verb", &[""], &[], vec![], owner_and_port, 2, 0),
        case(
            "an unknown flag",
            &["--all", "--bogus"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word list doesn't take",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "show with no id",
            &["show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "show with two ids",
            &["show", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "show, a bad flag",
            &["show", "a", "-x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "main, a bad --thread",
            &["main", "--thread", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "main --thread last",
            &["main", "--thread"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "main, a word",
            &["main", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "the gate comes before the words",
            &["nope"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        // The status.
        case(
            "a refusal with a permission card: exit 3, the closing line",
            &[],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "400: exit 2",
            &["--past", "--days", "x"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_days","hint":"days is a number"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "413: exit 2",
            &[],
            &[],
            vec![Reply::Answer(413, r#"{"error":"too big"}"#.into())],
            owner_and_port,
            2,
            1,
        ),
        case(
            "a 404 with no route: too old",
            &["show", "x"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"route not found"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 404 Not Found: too old",
            &[],
            &[],
            vec![Reply::Answer(404, "Not Found".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 404 that names the id: exit 4",
            &["show", "zz"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no conversation zz"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "500, not JSON",
            &[],
            &[],
            vec![Reply::Answer(500, "boom é\n\n".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, a long text",
            &[],
            &[],
            vec![Reply::Answer(500, "x".repeat(300))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "201 is no success",
            &[],
            &[],
            vec![Reply::Answer(201, r#"{"error":["a"]}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "403, a list",
            &[],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 403 with an empty error object",
            &[],
            &[],
            vec![Reply::Answer(403, r#"{"error":{}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "migrating is an error here",
            &[],
            &[],
            vec![Reply::Answer(503, r#"{"state":"migrating"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a 200 that is not JSON",
            &[],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "an empty 200",
            &["main"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "a title that is a number: Python raises part way",
            &[],
            &[],
            vec![ok(r#"{"sessions":[{"address":"a"},{"title":5}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "past, no id on the first to open",
            &["--past"],
            &[],
            vec![ok(r#"{"conversations":[{"cli":"x"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "show, messages as a number",
            &["show", "x"],
            &[],
            vec![ok(r#"{"messages":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        Case {
            cwd: Some(link_to_proj(&rig)),
            ..case(
                "PWD through a link",
                &[],
                &[],
                vec![reply(roster())],
                owner_and_port,
                0,
                1,
            )
        },
        case(
            "PROJECT_PATH",
            &["--past"],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(r#"{"conversations":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a long workspace",
            Box::leak(vec!["--workspace", leak("w é".repeat(700))].into_boxed_slice()),
            &[],
            vec![reply(roster())],
            owner_and_port,
            0,
            1,
        ),
        // The gates.
        case("no port", &[], &[], vec![], owner_only, 1, 0),
        case("no token", &[], &[], vec![], port_only, 1, 0),
        case(
            "the virtual desktop's guard",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sessions_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let cell = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            h(&[]),
            case(
                "a host session: the passport over TCP",
                &[],
                &[],
                vec![reply(roster())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["--past"],
                &[],
                vec![reply(past())],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport: no heal, the probe, the line",
                &[],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                3,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport the probe finds good",
                &["main"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    ok(r#"{"role":"canonical"}"#),
                ],
                port_only,
                3,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone, the owner on disk: the passport (an id tool)",
                &[],
                &[],
                vec![ok(r#"{"role":"canonical"}"#), reply(roster())],
                owner_and_port,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &[],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(&[]),
            case(
                "the cell socket",
                &["show", "ab12"],
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
                "not served there: TCP",
                &[],
                &[],
                vec![not_served(), reply(roster())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "no answer there: TCP",
                &[],
                &[],
                vec![Reply::Close, reply(roster())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a reset there: TCP",
                &["--past"],
                &[],
                vec![Reply::Reset, reply(past())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not HTTP there: TCP",
                &[],
                &[],
                vec![Reply::Raw(GARBAGE.to_vec()), reply(roster())],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "an empty 200 there is the answer",
                &["--json"],
                &[],
                vec![ok("")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "a refusal there is the answer",
                &[],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket and TCP: no answer on both",
                &[],
                &[],
                vec![Reply::Close, Reply::Close],
                port_only,
                1,
                2,
            ),
        ),
        with(
            cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &["main"],
                &[],
                vec![ok(r#"{"main":{"state":"none"}}"#)],
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
fn sessions_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let v = vec![
        case(
            "an empty reply: status 000",
            &[],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx with no body: its status",
            &["--past"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["show", "x"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["main"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        Case {
            no_tmpdir: true,
            ..case(
                "TMPDIR unusable, a reset",
                &[],
                &[],
                vec![Reply::Reset],
                owner_and_port,
                1,
                1,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sessions_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens: each try's 000",
        &[],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
    rig.check(&with(
        with_passport(&[("K2_HOOK_SOCK", "{deadsock}"), ("K2_PORT", port.as_str())]),
        case(
            "a dead socket and no daemon",
            &["--past"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
    ));
}

/// No answer within 120 s (`_sidecar_send`'s limit). Slow: two minutes
/// a side.
#[test]
fn sessions_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    rig.check(&case(
        "no answer in time (28)",
        &[],
        &[],
        vec![Reply::Hang(Duration::from_secs(125))],
        owner_and_port,
        1,
        1,
    ));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn sessions_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case("help", &["help"], &[], vec![], owner_and_port, 0, 0)),
        shell(case(
            "--help",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "show -h",
            &["show", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "main set: a POST",
            &["main", "set", "ab12"],
            &[],
            vec![ok(
                r#"{"workspace":"w","main":{"conversationId":"ab12","cli":"claude"},"title":"T","restartWith":"k2 msg w"}"#,
            )],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "open: a POST",
            &["open", "ab12"],
            &[],
            vec![ok(r#"{"address":"acme/x","opened":true,"cli":"claude"}"#)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "spawn",
            &["spawn", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "archives",
            &["archives"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "compact",
            &["compact", "s1"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "a value flag as the last word",
            &["--past", "--search"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "show --tail as the last word",
            &["show", "x", "--tail"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "a port that is no port",
            &[],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
