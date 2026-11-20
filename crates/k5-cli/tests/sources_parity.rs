//! `k5-cli sources` against the shell CLI's `cli/k2 sources`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for the forms the binary answers (the help, `list`, `show`,
//! `read`, `history`, `status`, `export`, `pause`, `resume`, `mute`,
//! `unmute`, `remove`, `pending`, `access`), and the hand-back of the rest.
//!
//! - The help: the noun's page and each verb's, with no daemon set up,
//!   behind the guard, and in a session with no passport. Help never
//!   acts: each verb that changes something with `-h` or `--help` sends
//!   nothing.
//! - The words: a missing name, an unknown flag, durations, and a value
//!   flag as the last word (the rows named `bash D6:`, below).
//! - Every verb, with `--json` and without; many rows (above 256 KiB);
//!   the renderer's odd answers.
//! - Refusals: the codes that need a person (3, with or without the
//!   daemon's hint), not found (4), `--json` on a refusal, no body, not
//!   JSON; no answer; the cell socket.
//!
//! The rows named `bash D6:` are a bash bug, copied until bash is fixed:
//! a value flag as the last word is `shift 2` failing under `set -e`
//! (`sources.sh:661-663`, `:788-789`, `:974`), so bash ends with 1 and
//! says nothing. They flip to K2's D6 ruling (one `usage:` line, exit 2)
//! when bash is fixed.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "sources";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn view(display: &str, extra: &str) -> String {
    format!(r#"{{"display":"{display}","serverNow":1763600000,"updatedAt":1763599000{extra}}}"#)
}

fn list() -> String {
    format!(
        r#"{{"sources":[
        {{"name":"web é","status":"active","secret":"API","triggers":1,"keep":"states","view":{}}},
        {{"name":"db","status":"paused","triggers":3,"view":{}}},
        {{"name":"never","status":"proposed","view":{}}}]}}"#,
        view(
            "warn",
            r#","counts":{"ok":3,"warn":1,"unknown":0},"reason":"slow","soft":{"count":2,"of":3,"level":"crit"},"muted":{"by":"owner"},"simulated":{"state":"crit"},"pinned":true"#
        ),
        view("paused", r#","soft":{"count":1,"of":3}"#),
        r#"{"display":"pending","serverNow":10,"updatedAt":null}"#
    )
}

/// Thousands of rows, above 256 KiB.
fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"name":"src-{i}-{}","status":"{}","view":{}}}"#,
                "x".repeat(i % 20),
                if i % 3 == 0 { "active" } else { "paused" },
                view(
                    ["ok", "warn", "crit", "stale"][i % 4],
                    &format!(r#","counts":{{"ok":{}}}"#, i % 7)
                )
            )
        })
        .collect();
    let body = format!(r#"{{"sources":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    body
}

fn read() -> String {
    r#"{"name":"web","label":"The web é","display":"crit","serverNow":1763600000,"updatedAt":1763000000,
        "reasonText":"HTTP 500 from the origin","gap":{"kind":"poller_down","from":100,"to":160.5},
        "fields":{"z":[1,2],"a":"é","n":null},"masked":true,
        "items":[{"key":"eu-1","health":"ok","fields":{"ms":12}},
                 {"key":"us-1","health":"crit","reason":"timeout","soft":{"count":1,"of":2},"fields":{}}]}"#
        .to_string()
}

fn show() -> String {
    format!(
        r#"{{"source":{{"status":"active","everySecs":60,"madeBy":"acme","viewers":"member","agentAccess":"values","keep":"all",
            "secret":"API","secretBound":false,"fetch":{{"urlSecret":{{"name":"URL","origin":"https://example.com"}}}}}},
            "view":{},"warnings":[{{"hint":"the origin is slow"}}]}}"#,
        r#"{"name":"web","label":"L","display":"ok","serverNow":5,"updatedAt":5}"#
    )
}

#[test]
fn sources_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        ("the help: sources alone", &[][..]),
        ("help", &["help"]),
        ("-h", &["-h"]),
        ("list -h: the read page", &["list", "-h"]),
        ("ls --help: the noun's page", &["ls", "--help"]),
        ("history --help", &["history", "x", "--help"]),
        ("export -h", &["export", "-h"]),
        ("pending -h", &["pending", "-h"]),
        ("propose -h: the flags page", &["propose", "-h"]),
        ("add -h", &["add", "--help"]),
        ("edit -h", &["edit", "x", "-h"]),
        ("try -h", &["try", "-h"]),
        ("an unknown verb's help", &["nosuch", "-h"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(case(
        "the help with no daemon set up",
        &["read", "--help"],
        &[],
        vec![],
        nothing,
        0,
        0,
    ));
    v.push(case(
        "the help behind the virtual desktop's guard",
        &["help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
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
        ("show with no name", &["show", "--json"][..]),
        ("read with no name", &["read"]),
        ("export with no name", &["export"]),
        ("pause with no name", &["pause"]),
        ("history with no field", &["history", "web"]),
        (
            "history, an unknown flag",
            &["history", "web", "health", "--since", "1"],
        ),
        (
            "history, a bad --from",
            &["history", "web", "health", "--from", "6x"],
        ),
        ("mute with no --for", &["mute", "web"]),
        ("mute, a bad duration", &["mute", "web", "--for", "5q"]),
        (
            "mute, an unknown flag",
            &["mute", "web", "--for", "5m", "--x"],
        ),
        ("remove with no --yes and no terminal", &["remove", "web"]),
        ("rm with no name", &["rm", "--yes"]),
        ("pending, a word it doesn't take", &["pending", "web"]),
        ("access with nothing", &["access"]),
        ("access on off", &["access", "on", "off"]),
        (
            "access, an empty workspace",
            &["access", "on", "--workspace", ""],
        ),
        ("access, a word it doesn't take", &["access", "on", "x"]),
        ("an unknown verb", &["nosuch"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    // bash D6: a value flag as the last word; flips to a usage line,
    // exit 2.
    for (name, args) in [
        (
            "bash D6: history --from last",
            &["history", "web", "health", "--from"][..],
        ),
        (
            "bash D6: mute --reason last",
            &["mute", "web", "--for", "1m", "--reason"],
        ),
        ("bash D6: pending --names last", &["pending", "--names"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 1, 0));
    }
    v.push(case(
        "the guard",
        &["list"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    for c in v {
        rig.check(&c);
    }
    // `source` is the same command.
    let rig = Rig::new("source", "alias");
    rig.check(&case(
        "source list",
        &["ls"],
        &[],
        vec![ok(&list())],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn sources_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let done = || ok(r#"{"ok":true}"#);
    let v = vec![
        // covers: sources list
        case(
            "list",
            &["list"],
            &[],
            vec![ok(&list())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, none",
            &["ls"],
            &[],
            vec![ok(r#"{"sources":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources list --json
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![ok(&list())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, thousands of rows",
            &["list"],
            &[],
            vec![ok(&many(2500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json, thousands of rows",
            &["list", "--json"],
            &[],
            vec![ok(&many(2500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json, not JSON",
            &["list", "--json"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources show
        case(
            "show",
            &["show", "web é"],
            &[],
            vec![ok(&show())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources show --json
        case(
            "show --json",
            &["show", "--json", "web"],
            &[],
            vec![ok(&show())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources read
        case(
            "read",
            &["read", "web"],
            &[],
            vec![ok(&read())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources read --json
        case(
            "read --json",
            &["read", "web", "--json"],
            &[],
            vec![ok(&read())],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources history
        case(
            "history health, epochs",
            &[
                "history", "web", "health", "--item", "eu 1", "--from", "100", "--to", "200",
            ],
            &[],
            vec![ok(
                r#"{"field":"health","events":[{"t":150,"kind":"state","item":"eu 1","from":"ok","to":"crit","reason":"timeout"},{"t":160.9,"kind":"gap"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "history health, none",
            &["history", "web", "health"],
            &[],
            vec![ok(r#"{"field":"health","events":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources history --json
        case(
            "history trend --json",
            &["history", "web", "ms", "--json"],
            &[],
            vec![ok(
                r#"{"field":"ms","points":[{"t":1,"v":null},{"t":2,"v":1.5}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "history trend",
            &["history", "web", "ms"],
            &[],
            vec![ok(
                r#"{"field":"ms","points":[{"t":1,"v":null},{"t":2,"v":1.5}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources status
        case(
            "status",
            &["status"],
            &[],
            vec![ok(
                r#"{"running":true,"sources":4,"active":3,"targets":9,"inFlight":1,"behindSecs":12.7,"parkedOrigins":[{"origin":"https://a","untilAt":5,"reason":"429"}],"lastGap":{"kind":"sleep","from":1,"to":31},"airgap":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources status --json
        case(
            "status --json",
            &["status", "--json"],
            &[],
            vec![ok(
                r#"{"running":false,"sources":0,"active":0,"targets":0,"inFlight":0}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources export
        case(
            "export: always JSON",
            &["export", "web"],
            &[],
            vec![ok(
                r#"{"name":"web","label":"é","fetch":{"url":"https://example.com"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources pause
        case(
            "pause",
            &["pause", "web"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources pause --json
        case(
            "pause --json",
            &["pause", "web", "--json"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        case(
            "resume",
            &["resume", "web"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources mute
        case(
            "mute",
            &["mute", "web", "--for", "30m", "--reason", "deploy é"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources mute --json
        case(
            "mute --json, seconds",
            &["mute", "web", "--for", "45", "--json"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources unmute
        case(
            "unmute",
            &["unmute", "web"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources unmute --json
        case(
            "unmute --json",
            &["unmute", "--json", "web"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources remove
        case(
            "remove --yes",
            &["remove", "web", "--yes"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources remove --json
        case(
            "rm --yes --json",
            &["rm", "--yes", "web", "--json"],
            &[],
            vec![done()],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources pending
        case(
            "pending",
            &["pending", "--names", "a b,c"],
            &[],
            vec![ok(
                r#"{"sources":[{"name":"web","approval":"one_click","everySecs":60,"targets":2,"origins":["https://a","https://b"],"maker":{"handle":"acme"},"urlSecret":"URL"}],
                    "triggers":[{"id":"0123456789ab","source":"web","on":{"kind":"crit","secs":30},"agent":{"handle":"acme"},"madeBy":"owner"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "pending, own, none",
            &["pending"],
            &[],
            vec![ok(r#"{"scope":"own"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "pending, own",
            &["pending"],
            &[],
            vec![ok(
                r#"{"scope":"own","sources":[{"name":"w","approval":"owner_only","everySecs":5,"targets":1,"madeBy":"me"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources pending --json
        case(
            "pending --json",
            &["pending", "--json"],
            &[],
            vec![ok(r#"{"sources":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access on",
            &["access", "on", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanManageGardens":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access off, here",
            &["access", "off"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanManageGardens":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access off --json",
            &["access", "off", "--json", "--workspace", "w"],
            &[],
            vec![ok(r#"{"workspace":"w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access, a 403",
            &["access", "on", "--workspace", "w"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        traceback(case(
            "read, not JSON",
            &["read", "web"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "read, a display that is a number",
            &["read", "web"],
            &[],
            vec![ok(r#"{"name":"n","label":"L","display":5,"serverNow":1}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "list, a row with no view",
            &["list"],
            &[],
            vec![ok(r#"{"sources":[{"name":"a"}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "status, a count that is text",
            &["status"],
            &[],
            vec![ok(r#"{"sources":"4","active":1,"targets":1,"inFlight":0}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "pending, a trigger kind that is a number",
            &["pending"],
            &[],
            vec![ok(r#"{"triggers":[{"on":{"kind":5}}]}"#)],
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
fn sources_refusals_and_no_answer() {
    let rig = Rig::new(VERB, "refuse");
    let port = dead_port();
    let v = vec![
        case(
            "person_only with a hint: the person line too",
            &["pause", "web"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"person_only","hint":"ask a person"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "lan_owner_only with a hint: no line of ours",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"lan_owner_only","hint":"from the LAN"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "forbidden with no hint: the person line",
            &["mute", "web", "--for", "1h"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "secret_not_usable: the secret line",
            &["resume", "web"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"secret_not_usable","hint":" "}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "owner_only: never a line of ours",
            &["unmute", "web"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "source_not_found --json: the body on stdout",
            &["show", "nope", "--json"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"source_not_found","hint":"none"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an error that is text",
            &["status"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, not JSON",
            &["export", "web"],
            &[],
            vec![Reply::Answer(500, format!("boom é\r\n{}", "x".repeat(400)))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, no body",
            &["read", "web"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
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
            &["pause", "web"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens",
            &["status"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "the cell socket: a GET",
                &["read", "web"],
                &[],
                vec![ok(&read())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "the cell socket: access goes on TCP",
                &["access", "on", "--workspace", "w"],
                &[],
                vec![ok(r#"{"workspace":"w","agentsCanManageGardens":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "remove in a session needs --yes",
                &["remove", "web"],
                &[],
                vec![],
                port_only,
                2,
                0,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `simulate` and `refresh`. (Every verb is native now; the shell's
/// own arithmetic errors still go to it, but they name the script's path,
/// so no row can compare them: D6.)
#[test]
fn sources_simulate_refresh_and_the_shell() {
    let rig = Rig::new(VERB, "sim");
    let read_at = |t: u32| {
        ok(&format!(
            r#"{{"name":"web","label":"L","display":"ok","serverNow":9,"updatedAt":9,"checkedAt":{t}}}"#
        ))
    };
    let v = vec![
        // covers: sources simulate
        case(
            "simulate crit --for",
            &["simulate", "web", "crit", "--for", "1h"],
            &[],
            vec![ok(
                r#"{"name":"web","simulated":{"state":"crit","until":1763603600}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "simulate off",
            &["simulate", "web", "off"],
            &[],
            vec![ok(r#"{"name":"web"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources simulate --json
        case(
            "simulate --json",
            &["simulate", "web", "ok", "--json"],
            &[],
            vec![ok(r#"{"name":"web"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "simulate, a bad state",
            &["simulate", "web", "red"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "simulate, --for last",
            &["simulate", "web", "ok", "--for"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "simulate, a bad duration",
            &["simulate", "web", "ok", "--for", "9q"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: sources refresh
        case(
            "refresh",
            &["refresh", "web"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sources refresh --json
        case(
            "refresh --json",
            &["refresh", "web", "--json"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "refresh --wait: a new check",
            &["refresh", "web", "--wait"],
            &[],
            vec![read_at(1), ok(r#"{"ok":true}"#), read_at(1), read_at(2)],
            owner_and_port,
            0,
            4,
        ),
        traceback(case(
            "refresh --wait, not JSON",
            &["refresh", "web", "--wait"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Help never acts: each verb that changes something, with `-h` or
/// `--help` anywhere, prints a help page and sends nothing.
#[test]
fn sources_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for words in [
        &["pause", "web"][..],
        &["resume", "web"],
        &["mute", "web", "--for", "1h"],
        &["unmute", "web"],
        &["remove", "web", "--yes"],
        &["access", "on", "--workspace", "w"],
        &[
            "propose",
            "--name",
            "n",
            "--label",
            "L",
            "--url",
            "https://e",
        ],
        &["approve", "web"],
        &["refresh", "web"],
        &["add", "--name", "n", "--label", "L", "--url", "https://e"],
        &["edit", "web", "--label", "L"],
        &["try", "--url", "https://e"],
        &["approve", "web", "--secret", "S"],
        &["secret", "web", "API"],
        &["pin", "web"],
        &["unpin", "web"],
        &["trigger", "propose", "--source", "web", "--on", "went_crit"],
        &["trigger", "pause", "abc"],
        &["trigger", "edit", "abc", "--no-note"],
    ] {
        for (flag, at) in [("-h", 1), ("--help", words.len())] {
            let mut line = words.to_vec();
            line.insert(at.min(line.len()), flag);
            let args: &'static [&'static str] = Box::leak(line.into_boxed_slice());
            let name: &'static str =
                Box::leak(format!("{} {flag} at {at}", words[0]).into_boxed_str());
            v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
        }
    }
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let rig = Rig::new(VERB, "brand");
    let cli = k2_client::brand::CLI_NAME;
    for (args, want) in [
        (
            &["show"][..],
            format!("Error: usage: {cli} sources show <name>\n"),
        ),
        (
            &["mute", "web"],
            format!("Error: usage: {cli} sources mute <name> --for 30m [--reason \"\u{2026}\"]\n"),
        ),
        (
            &["access"],
            format!("usage: {cli} sources access on|off [--workspace <ws>] [--json]\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
