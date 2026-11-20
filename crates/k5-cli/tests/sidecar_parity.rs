//! `k5-cli sidecar` against the shell CLI's `cli/k2 sidecar`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every form.
//!
//! - The help: `sidecar` alone, `help`, `--help`, `-h` anywhere, with no
//!   daemon set up, behind the virtual desktop's guard, and in a session
//!   with no passport. Help never acts: each verb that changes something
//!   with `-h` or `--help` first, in the middle or last sends nothing.
//! - The words: each verb's usage lines, an unknown verb or flag, and
//!   a value flag as the last word (the rows named `bash D6:`, below).
//! - Every verb, with `--json` and without: the bodies and queries
//!   (unicode, a brief from a file and from stdin), many rows (an answer
//!   above 256 KiB), the readers' odd answers (not JSON, a field of
//!   another type).
//! - `land` and `retire --discard`: the review first, `--yes`, no
//!   terminal to ask on, nothing to land, a refused review.
//! - Refusals (400, 401, 403 with a permission card, 404, 500 with no
//!   body or not JSON), the owner from disk, a passport over TCP, a live
//!   cell socket (a GET goes on to TCP after no answer there, a POST only
//!   after "not served"), no answer and no daemon.
//!
//! The rows named `bash D6:` are a bash bug, copied until bash is fixed:
//! a value flag as the last word is `shift 2` failing under `set -e`
//! (`sidecar.sh:302-307`, `:381`, `:397`, `:430`, `:467`, `:591`), so
//! bash ends with 1 and says nothing. They flip to K2's D6 ruling (one
//! `usage:` line, exit 2) when bash is fixed.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "sidecar";

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

fn stdin(bytes: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(bytes),
        ..c
    }
}

fn list() -> String {
    r#"{"workspace":"acme é","live":1,"cap":4,"agentsCanManage":true,"sidecars":[
        {"address":"acme/scout","name":"scout with a very long name indeed","harness":"claude","state":"live","createdBy":"agent acme/main","lastSeenAt":1763600000,
         "taskState":"working","worktree":{"branch":"sc/scout","ahead":3,"dirty":true}},
        {"address":"acme/n日本","name":null,"harness":null,"state":"asleep","lastSeenAt":0},
        {"address":"acme/f","name":"f","harness":"codex","state":"asleep","lastSeenAt":1763600000.7,"worktree":{"branch":null}}],
        "worktrees":{"open":1,"cap":2}}"#
        .to_string()
}

/// Thousands of rows, above 256 KiB.
fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"address":"ws/sc-{i}","name":"name {i} {}","harness":"claude","state":"{}","createdBy":"app","lastSeenAt":{}}}"#,
                "x".repeat(i % 30),
                if i % 2 == 0 { "live" } else { "asleep" },
                1_763_000_000 + i * 60
            )
        })
        .collect();
    let body = format!(
        r#"{{"workspace":"ws","live":{},"cap":5000,"sidecars":[{}]}}"#,
        n / 2,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024, "{} bytes", body.len());
    body
}

fn review(blocked: &str) -> String {
    format!(
        r#"{{"address":"acme/scout","branch":"sc/scout","dir":"/w/scout","taskState":"done","onto":"main","ontoHead":"aaaaaaaaaaaa1111",
            "tip":"bbbbbbbbbbbb2222","commits":[{{"sha":"bbbbbbbbbbbb2222","subject":"fix é"}},{{"sha":"cccccccccccc","subject":"merge","merge":true}}],
            "files":[{{"status":"M","path":"src/a.rs"}}],"dirty":["notes.md"],"mainDirty":["x","y"],"mainBranch":"dev","blocked":{blocked}}}"#
    )
}

fn brief_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("brief.md"), "Look at the logs é.\n").expect("write the brief");
    std::fs::write(r.proj.join("bad.md"), b"\xff\xfe").expect("write the bad brief");
}

#[test]
fn sidecar_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        ("the help: sidecar alone", &[][..]),
        ("help", &["help"]),
        ("--help", &["--help"]),
        ("-h after a verb", &["list", "-h"]),
        ("--help after an unknown verb", &["nosuch", "--help"]),
        (
            "--help as a flag's value",
            &["new", "a", "--harness", "--help"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(case(
        "the help with no daemon set up",
        &["new", "--help"],
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
        ("an unknown verb", &["nosuch"][..]),
        ("new with no name", &["new", "--harness", "h"]),
        ("new with no harness", &["new", "a"]),
        ("new with two names", &["new", "a", "b", "--harness", "h"]),
        (
            "new, --worktree takes the name",
            &["new", "a", "--worktree", "b", "--harness", "h"],
        ),
        (
            "new, an unknown flag",
            &["new", "a", "--harness", "h", "--bogus"],
        ),
        ("new, - alone is a flag", &["new", "-", "--harness", "h"]),
        (
            "new, --base without --worktree",
            &["new", "a", "--harness", "h", "--base", "main"],
        ),
        (
            "new, --brief and --brief-text",
            &[
                "new",
                "a",
                "--harness",
                "h",
                "--brief",
                "brief.md",
                "--brief-text",
                "x",
            ],
        ),
        (
            "new, no brief file",
            &["new", "a", "--harness", "h", "--brief", "nope.md"],
        ),
        (
            "new, a folder for a brief",
            &["new", "a", "--harness", "h", "--brief", "."],
        ),
        (
            "new, a brief that is not UTF-8",
            &["new", "a", "--harness", "h", "--brief", "bad.md"],
        ),
        ("list, a word it doesn't take", &["list", "x"]),
        ("ls --all", &["ls", "--all"]),
        ("stop with no name", &["stop", "--json"]),
        ("stop with two names", &["stop", "a", "b"]),
        ("stop, an unknown flag", &["stop", "a", "--all"]),
        ("archive with nothing", &["archive"]),
        (
            "archive with a name and --all-stopped",
            &["archive", "a", "--all-stopped"],
        ),
        ("archive, an unknown flag", &["archive", "-x"]),
        ("access with nothing", &["access"]),
        ("access, a verb it doesn't have", &["access", "maybe"]),
        ("access status --json", &["access", "status", "--json"]),
        ("interrupt with no name", &["interrupt"]),
        ("review with two names", &["review", "a", "b"]),
        ("land, an unknown flag", &["land", "a", "--force"]),
        ("retire, an unknown short flag", &["retire", "a", "-n"]),
    ] {
        v.push(case(name, args, &[], vec![], brief_file, 2, 0));
    }
    // bash D6: a value flag as the last word; flips to a usage line,
    // exit 2.
    for (name, args) in [
        (
            "bash D6: new --harness last",
            &["new", "a", "--harness"][..],
        ),
        (
            "bash D6: new --brief-text last",
            &["new", "a", "--harness", "h", "--brief-text"],
        ),
        ("bash D6: new --workspace last", &["new", "--workspace"]),
        ("bash D6: list --workspace last", &["list", "--workspace"]),
        (
            "bash D6: stop --workspace last",
            &["stop", "a", "--workspace"],
        ),
        (
            "bash D6: archive --workspace last",
            &["archive", "--workspace"],
        ),
        (
            "bash D6: access on --workspace last",
            &["access", "on", "--workspace"],
        ),
        (
            "bash D6: land --workspace last",
            &["land", "a", "--workspace"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 1, 0));
    }
    v.push(case(
        "a usage error with no daemon set up: the gate first",
        &["new"],
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
fn sidecar_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let opened = r#"{"address":"acme/scout","harness":"claude","resumed":false,"brief":{"path":"/w/.k2/brief.md","bytes":20},"worktree":{"dir":"/w/scout","branch":"sc/scout","base":"main","baseSha":"0123456789abcdef"}}"#;
    let v = vec![
        // covers: sidecar new
        case(
            "new, every flag",
            &[
                "new",
                "scout é",
                "--harness",
                "claude",
                "--model",
                "opus",
                "--workspace",
                "acme w",
                "--worktree",
                "--base",
                "main",
                "--brief-text",
                "do \"this\" ☃",
            ],
            &[],
            vec![ok(opened)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "new --worktree <name>, resumed",
            &["new", "--worktree", "scout", "--harness", "claude"],
            &[],
            vec![ok(
                r#"{"address":"acme/scout","harness":"claude","resumed":true}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "new, a brief from a file",
            &["new", "a", "--harness", "h", "--brief", "brief.md"],
            &[],
            vec![ok(
                r#"{"address":"acme/a","harness":"h","brief":{"path":"/b","bytes":20}}"#,
            )],
            brief_file,
            0,
            1,
        ),
        stdin(
            b"from stdin\n\xe2\x98\x83 \x00 end",
            case(
                "new, a brief on stdin",
                &["new", "a", "--harness", "h", "--brief", "-"],
                &[],
                vec![ok(r#"{"address":"acme/a","harness":"h"}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        stdin(
            b"\xff",
            case(
                "new, a brief on stdin that is not UTF-8",
                &["new", "a", "--harness", "h", "--brief", "-"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
        ),
        case(
            "new, words after -- are dropped",
            &["new", "a", "--harness", "h", "--", "--bogus"],
            &[],
            vec![ok(r#"{"address":"acme/a","harness":"h"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar new --json
        case(
            "new --json",
            &["new", "a", "--harness", "h", "--json"],
            &[],
            vec![ok(opened)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar list
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
            "ls --workspace",
            &["ls", "--workspace", "w é&x+y"],
            &[],
            vec![ok(r#"{"workspace":"w","sidecars":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar list --json
        case(
            "list --json: the body as it came",
            &["list", "--json"],
            &[],
            vec![ok("{\"sidecars\":[]}\n\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list, thousands of rows",
            &["list"],
            &[("TZ", "UTC")],
            vec![ok(&many(3000))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json, thousands of rows",
            &["list", "--json"],
            &[],
            vec![ok(&many(3000))],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar stop
        case(
            "stop",
            &["stop", "scout", "--force"],
            &[],
            vec![ok(r#"{"address":"acme/scout","stopped":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar stop --json
        case(
            "stop --json, already asleep",
            &["stop", "scout", "--json", "--workspace", "w"],
            &[],
            vec![ok(r#"{"address":"acme/scout","stopped":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stop, already asleep",
            &["stop", "scout"],
            &[],
            vec![ok(r#"{"address":"acme/scout","stopped":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar archive
        case(
            "archive one",
            &["archive", "scout"],
            &[],
            vec![ok(r#"{"address":"acme/scout"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archive --all-stopped",
            &["archive", "--all-stopped"],
            &[],
            vec![ok(
                r#"{"count":2,"archived":["acme/a","acme/b"],"skipped":[{"address":"acme/c","hint":"it is live"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archive --all-stopped, none",
            &["archive", "--all-stopped"],
            &[],
            vec![ok(r#"{"count":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar archive --json
        case(
            "archive --json",
            &["archive", "--all-stopped", "--json"],
            &[],
            vec![ok(r#"{"count":1,"archived":["acme/a"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: sidecar access
        case(
            "access on",
            &["access", "on", "--workspace", "acme"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanManage":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access off",
            &["access", "off"],
            &[],
            vec![ok(r#"{"workspace":"acme","agentsCanManage":false}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "access status",
            &["access", "status"],
            &[],
            vec![ok(&list())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "interrupt",
            &["interrupt", "scout"],
            &[],
            vec![ok(
                r#"{"address":"acme/scout","interrupted":true,"key":"Escape"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "interrupt, asleep",
            &["interrupt", "scout", "--json"],
            &[],
            vec![ok(r#"{"address":"acme/scout"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "review",
            &["review", "scout", "--workspace", "acme é"],
            &[],
            vec![ok(&review(r#""main_has_uncommitted_changes""#))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "review, ready",
            &["review", "scout"],
            &[],
            vec![ok(&review("null"))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "review, retired",
            &["review", "scout"],
            &[],
            vec![ok(r#"{"address":"a","retiredAt":5,"retiredHow":"landed"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "review, a reason it doesn't know",
            &["review", "scout"],
            &[],
            vec![ok(&review(r#""odd""#))],
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
            "list, a name that is a number",
            &["list"],
            &[],
            vec![ok(r#"{"sidecars":[{"address":"a","name":5}]}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "new, an answer with no address",
            &["new", "a", "--harness", "h"],
            &[],
            vec![ok(r#"{"harness":"h"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "review, a reason that is a list",
            &["review", "a"],
            &[],
            vec![ok(r#"{"address":"a","blocked":["x"]}"#)],
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
fn sidecar_land_and_retire() {
    let rig = Rig::new(VERB, "land");
    let landed = r#"{"address":"acme/scout","count":1,"onto":"main","from":"aaaaaaaaaaaaaaa","to":"bbbbbbbbbbbbbbbbb","retired":{"branchDeleted":true}}"#;
    let v = vec![
        case(
            "land --yes: the review, then the land",
            &["land", "scout", "--yes"],
            &[],
            vec![ok(&review("null")), ok(landed)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "land -y --keep --json: the review on stderr",
            &["land", "scout", "-y", "--keep", "--json"],
            &[],
            vec![ok(&review("null")), ok(landed)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "land --yes, not retired",
            &["land", "scout", "--yes"],
            &[],
            vec![
                ok(r#"{"address":"a","onto":"main"}"#),
                ok(
                    r#"{"address":"a","count":2,"retireError":{"code":"busy","hint":"it is live"}}"#,
                ),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "land with no terminal to ask on",
            &["land", "scout"],
            &[],
            vec![ok(&review("null"))],
            owner_and_port,
            2,
            1,
        ),
        case(
            "land, nothing to land",
            &["land", "scout", "--yes"],
            &[],
            vec![ok(&review(r#""already_landed""#))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "land, a refused review",
            &["land", "scout", "--yes"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"no_such_sidecar","hint":"none"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "retire",
            &["retire", "scout", "--keep-branch"],
            &[],
            vec![ok(r#"{"address":"acme/scout","how":"landed"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "retire --discard --yes: the review on stderr",
            &["retire", "scout", "--discard", "--yes"],
            &[],
            vec![
                ok(&review("null")),
                ok(
                    r#"{"address":"acme/scout","how":"discarded","branchDeleted":true,"discardedCommits":[{"sha":"aaaaaaaaaaaaaa"},{"sha":"bbbbbbbbbbbbbbbbb"}]}"#,
                ),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "retire --discard with no terminal to ask on",
            &["retire", "scout", "--discard"],
            &[],
            vec![ok(&review("null"))],
            owner_and_port,
            2,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sidecar_refusals() {
    let rig = Rig::new(VERB, "refuse");
    let v = vec![
        case(
            "403 with a permission card: the closing line",
            &["new", "a", "--harness", "h"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "401 is 1",
            &["list"],
            &[],
            vec![Reply::Answer(401, REFUSED.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "400, --json prints nothing",
            &["stop", "a", "--json"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_name","hint":"say a name"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "404",
            &["access", "status"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "500, no body",
            &["archive", "a"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "500, not JSON",
            &["interrupt", "a"],
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
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn sidecar_with_a_passport() {
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
                "a host session: the passport over TCP",
                &["list"],
                &[],
                vec![ok(&list())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            // The refusal names no live passport, so the EXIT trap asks
            // whoami about it (`_permission_note_flush`).
            case(
                "a refused passport",
                &["stop", "a"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                2,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a GET",
                &["review", "a"],
                &[],
                vec![ok(&review("null"))],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "the cell socket: a POST",
                &["stop", "a"],
                &[],
                vec![ok(r#"{"address":"a","stopped":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(&[]),
            case(
                "not served there: a POST goes on to TCP",
                &["access", "on"],
                &[],
                vec![
                    not_served(),
                    ok(r#"{"workspace":"w","agentsCanManage":true}"#),
                ],
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
                vec![Reply::Close, ok(&list())],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(&[]),
            case(
                "no answer there: a POST is never sent again",
                &["new", "a", "--harness", "h"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        with(
            cell(&[]),
            case(
                "a refusal there is the answer",
                &["retire", "a"],
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
                &["interrupt", "a"],
                &[],
                vec![ok(r#"{"address":"a"}"#)],
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
fn sidecar_with_no_answer() {
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
            &["stop", "a"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["review", "a"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens: a GET",
            &["list"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "nothing listens: a POST",
            &["archive", "a"],
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

/// Help never acts: each verb that changes something, with `-h` or
/// `--help` first, in the middle or last, prints the help and sends
/// nothing (`cmd_sidecar` looks at every word first).
#[test]
fn sidecar_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    let mut v = Vec::new();
    for words in [
        &["new", "a", "--harness", "h"][..],
        &["stop", "a", "--force"],
        &["archive", "--all-stopped"],
        &["access", "on"],
        &["interrupt", "a"],
        &["land", "a", "--yes"],
        &["retire", "a", "--discard", "--yes"],
    ] {
        for (flag, at) in [("-h", 1), ("--help", 2), ("-h", words.len())] {
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
            &["list", "x"][..],
            format!("usage: {cli} sidecar list [--workspace <ws>] [--json]\n"),
        ),
        (
            &["stop", "a", "--bad"],
            format!("usage: unknown flag '--bad' ({cli} sidecar --help)\n"),
        ),
        (
            &["stop"],
            format!("usage: {cli} sidecar stop <name> [--workspace <ws>] [--force] [--json]\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
