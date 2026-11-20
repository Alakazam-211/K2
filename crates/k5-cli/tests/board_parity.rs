//! `k5-cli board` against the shell CLI's `cli/k2 board`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - A Garden board: each kind of answer the text reader prints (boards,
//!   objects with their count, an object, deleted, the log, templates, a
//!   template added, applied ids, meta, anything else), `--json`, the
//!   one-board warning, the refusals and their exits (an error object, a
//!   text, not JSON, an empty answer), `--out`'s export file, one large
//!   list (above 256 KiB), the parser's usage lines and its raise, help.
//! - A `.k5board` file: the path last, the plans and their routes, the
//!   reader (a list and a read in their fence, a saved change, the kit's
//!   row, the bridge's own page, a merge conflict, an error, a large list),
//!   the planner's usage lines. The fence's nonce and a group's id are
//!   random on both sides, so these compare with them masked
//!   ([`check_masked`]).
//! - `_compute_send`: the owner token over TCP, a passport on the cell
//!   socket, no passport in a session (the id tool's lines), a POST with
//!   no answer on the socket (the may-have-been-applied line, sent once),
//!   no answer over TCP (a dead port, none in time, an empty reply, not
//!   HTTP), the gates.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "board";

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn ops_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(
        r.proj.join("ops.json"),
        "{\"ops\": [{\"op\": \"add\", \"x\": 1.50}],\r\n \"extra\": true}\n",
    )
    .unwrap();
}

fn bad_ops_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("ops.json"), "{\"ops\": [1,\n  2 3]}").unwrap();
}

/// The objects of a board with `n` stickies.
fn many_objects(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"id":"s-{i:010}","type":"sticky","text":"note number {i} é with some words to make it long"}}"#))
        .collect();
    format!(
        r#"{{"ok":true,"board":"b1","count":{n},"objects":[{}]}}"#,
        rows.join(",")
    )
}

#[test]
fn a_garden_board_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "garden");
    let large = many_objects(3000);
    assert!(large.len() > 256 * 1024, "the large list is large");
    let v = vec![
        case(
            "boards",
            &["boards"],
            &[],
            vec![ok(
                r#"{"ok":true,"boards":[{"id":"b1a2b3c","name":"Launch","project":null,"trashed":false},{"id":"b2","name":"Old é","trashed":true}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: board
        case(
            "no boards",
            &["boards", "--all"],
            &[],
            vec![ok(r#"{"ok":true,"boards":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "read, named",
            &["read", "h-7f3a2b1c9d", "--board", "Launch"],
            &[],
            vec![ok(
                r#"{"ok":true,"board":"b1a2b3c","object":{"id":"h-7f3a2b1c9d","type":"shape","x":1.5,"t":"é"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "add with no board: the warning",
            &[
                "add",
                "sticky",
                "--text",
                "Ship the beta",
                "--x",
                "400",
                "--color=yellow",
            ],
            &[],
            vec![ok(
                r#"{"ok":true,"board":"b1a2b3c","rev":7,"object":{"id":"s-0123456789","type":"sticky"}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "delete and its cascade",
            &["rm", "h-1", "--board", "Launch"],
            &[],
            vec![ok(
                r#"{"ok":true,"board":"b1","rev":9,"deleted":["h-1","c-0123456789"]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the log",
            &["log", "--limit", "2", "--board=b1"],
            &[],
            vec![ok(
                r#"{"log":[{"rev":3,"at":"2026-11-01","by":"agent:x","changed":["a","b"],"deleted":2},{"rev":2,"at":"t","by":"y","changed":[]}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "templates",
            &["templates", "--board", "b1"],
            &[],
            vec![ok(
                r#"{"templates":[{"name":"kanban","label":"Kanban"},{"name":"a-very-long-name","label":"L"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a template added",
            &[
                "template", "kanban", "--x", "10", "--y", "-2.5", "--board", "b1",
            ],
            &[],
            vec![ok(r#"{"template":"kanban","ids":["a","b","c"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "objects and their count",
            &["list", "--type", "sticky", "--full", "--board", "b1"],
            &[],
            vec![ok(r#"{"board":"b1","count":1,"objects":[{"id":"s-1"}]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "meta",
            &["meta", "--title", "Plan", "--board", "b1"],
            &[],
            vec![ok(r#"{"meta":{"title":"Plan"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "anything else is dumped",
            &["board-rename", "--name", "New", "--board", "b1"],
            &[],
            vec![ok(r#"{"ok":true,"renamed":"New"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "apply from a file: its ids",
            &["apply", "ops.json", "--board", "b1"],
            &[],
            vec![ok(r#"{"ids":["x1"],"count":1}"#)],
            ops_file,
            0,
            1,
        ),
        case(
            "--json, kind added",
            &["read", "x", "--json", "--board", "b1"],
            &[],
            vec![ok(r#"{"ok":true,"object":{"t":"é"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: board --json
        case(
            "--json, with no board named: the warning too",
            &["boards", "--json"],
            &[],
            vec![ok(r#"{"kind":"x","board":"b9"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a large list",
            &["list", "--board", "b1"],
            &[],
            vec![ok(&large)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the export file",
            &["export", "--out", "out.json", "--board", "b1"],
            &[],
            vec![ok(
                r#"{"board":"b1","export":{"objects":[{"id":"é"}],"blobs":{},"people":{"p":1}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // Help never acts: the front hands a line with -h or --help in it
        // to the shell, which prints the board help and sends nothing.
        Case {
            compare: Compare::Shell,
            ..case(
                "--help after the verb: the help, nothing sent",
                &["add", "--help"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..case(
                "delete <id> -h: the help, nothing sent",
                &["delete", "s-1", "-h"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn refusals_keep_their_exits() {
    let rig = Rig::new(VERB, "refused");
    let v = vec![
        case(
            "locked: 5",
            &["read", "h", "--board", "b"],
            &[],
            vec![answer(
                409,
                r#"{"ok":false,"error":{"code":"locked","hint":"h is locked"}}"#,
            )],
            owner_and_port,
            5,
            1,
        ),
        case(
            "a conflict: 1",
            &["read", "h", "--board", "b"],
            &[],
            vec![answer(
                409,
                r#"{"error":{"code":"conflict","hint":"it changed"}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "newer_format: 4",
            &["read", "h", "--board", "b"],
            &[],
            vec![answer(
                409,
                r#"{"error":{"code":"newer_format","hint":"newer"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "core_off: 3",
            &["list", "--board", "Off"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"core_off","hint":"the Board core is off"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "too_long: 2",
            &["update", "s", "--text", "x", "--board", "b"],
            &[],
            vec![answer(
                400,
                r#"{"error":{"code":"too_long","hint":"text: at most 5000 characters","max":5000}}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "an error that is text",
            &["list", "--board", "b"],
            &[],
            vec![answer(500, r#"{"error":"boom"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list", "--board", "b"],
            &[],
            vec![answer(502, "<html>bad gateway</html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["list", "--board", "b"],
            &[],
            vec![answer(503, "")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--json: the answer as it came",
            &["list", "--json", "--board", "b"],
            &[],
            vec![answer(403, r#"{"error":{"code":"not_granted"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "200 but not JSON: the reader raises",
            &["list", "--board", "b"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        let c = if c.name.starts_with("200 but") {
            compare(Compare::Traceback, c)
        } else {
            c
        };
        rig.check(&c);
    }
}

#[test]
fn the_words_stop_before_a_request() {
    let rig = Rig::new(VERB, "usage");
    let v = vec![
        case("help alone", &[], &[], vec![], owner_and_port, 0, 0),
        case(
            "help as a word",
            &["help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "an unknown command",
            &["zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "update with no id",
            &["update"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "update with no field",
            &["set", "s-1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a flag the verb doesn't take",
            &["boards", "--zz", "1", "--aa"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--board with no name",
            &["list", "--board"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--x not a number",
            &["template", "k", "--x", "ten"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "apply with nothing on stdin",
            &["apply", "--board", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "apply: ops that are not JSON",
            &["apply", "ops.json", "--board", "b"],
            &[],
            vec![],
            bad_ops_file,
            2,
            0,
        ),
        case(
            "apply: no such file",
            &["apply", "nope.json", "--board", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two targets",
            &["list", "--board", "b", "plan.k5board"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("the gate", &["boards"], &[], vec![], nothing, 1, 0),
        case("the gate before help", &[], &[], vec![], nothing, 1, 0),
    ];
    for c in v {
        rig.check(&c);
    }
    rig.check(&compare(
        Compare::Traceback,
        case(
            "--limit inf: the parser raises",
            &["log", "--limit", "inf", "--board", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ));
}

#[test]
fn compute_send_s_channels_and_credentials() {
    let rig = Rig::new(VERB, "send");
    let sock = |extra: &[(&'static str, &str)]| {
        let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
        e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
        e
    };
    let v = vec![
        with(
            sock(&[]),
            case(
                "a passport on the cell socket",
                &["boards"],
                &[],
                vec![ok(r#"{"boards":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a passport over TCP",
                &["boards"],
                &[],
                vec![ok(r#"{"boards":[]}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            case(
                "a session with no passport: the id tool",
                &["boards"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        with(
            vec![("K2_PORT", dead_port())],
            case(
                "a dead port: four tries",
                &["boards"],
                &[],
                vec![],
                owner_only,
                1,
                0,
            ),
        ),
        with(
            vec![("K2_CP_MAX_TIME", "2".to_string())],
            case(
                "none in time",
                &["boards"],
                &[],
                vec![Reply::Hang(Duration::from_secs(4))],
                owner_and_port,
                1,
                1,
            ),
        ),
        case(
            "an empty reply",
            &["boards"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["boards"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    rig.check(&with(
        sock(&[("K2_CP_MAX_TIME", "2")]),
        compare(
            Compare::Waited,
            case(
                "a POST with no answer on the socket: never sent again",
                &["boards"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        ),
    ));
}

/// Hex runs of `len` after `prefix`, masked.
fn mask_after(text: &str, prefix: &str, len: usize) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        let (head, tail) = rest.split_at(at + prefix.len());
        out.push_str(head);
        let hex = tail.bytes().take(len).all(|b| b.is_ascii_hexdigit()) && tail.len() >= len;
        if hex {
            out.push_str(&"x".repeat(len));
            rest = &tail[len..];
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The random parts masked: the fence's nonce and a group's id.
fn masked(text: &str) -> String {
    mask_after(&mask_after(text, "(fence ", 16), "\"cli-", 32)
}

fn hosts_masked(log: &[String]) -> Vec<String> {
    log.iter()
        .map(|r| {
            let r = r
                .split('\n')
                .map(|l| {
                    if l.starts_with("Host: ") {
                        "Host: <host>"
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            masked(&r)
        })
        .collect()
}

/// [`Rig::check`] for a case whose output and requests hold random hex:
/// bash, the binary, and the binary with no `python3`, each compared with
/// it masked.
fn check_masked(rig: &Rig, c: &Case) {
    rig.prepare(Side::Bash, c);
    let b = rig.run(true, c);
    assert_eq!(
        rig.stub().left(),
        0,
        "{}: bash left answers unused: {b:?}",
        c.name
    );
    let bl: Vec<String> = rig
        .read(Side::Bash, false, c.name)
        .iter()
        .map(|r| normalise(r, false, rig.noun(), c.name))
        .collect();
    println!("== {}\n-- bash: {b:?}", c.name);
    assert_eq!(b.code, Some(c.code), "{}: bash's exit: {b:?}", c.name);
    assert_eq!(bl.len(), c.requests, "{}: bash's requests: {bl:?}", c.name);
    for without in [false, true] {
        let (r, rl) = rig.binary(c, without);
        println!("-- rust (no python3: {without}): {r:?}");
        assert_eq!(
            rig.stub().left(),
            0,
            "{}: the binary left answers unused",
            c.name
        );
        assert_eq!(
            (r.code, r.signal),
            (b.code, b.signal),
            "{}: the exit",
            c.name
        );
        assert_eq!(masked(&r.stdout), masked(&b.stdout), "{}: stdout", c.name);
        assert_eq!(masked(&r.stderr), masked(&b.stderr), "{}: stderr", c.name);
        assert_eq!(
            hosts_masked(&rl),
            hosts_masked(&bl),
            "{}: the requests",
            c.name
        );
    }
}

fn a_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("notes"), "x").unwrap();
}

const LIST: &str = r#"{"hash":"h1","settings":{"bg":"é"},"settingsRev":"r0","objects":[{"rev":"r1","object":{"id":"s-1","type":"sticky","text":"Ship"}},{"rev":"r2","object":{"id":"c-2","type":"connector"}}]}"#;

#[test]
fn a_board_file_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "file");
    let rows: Vec<String> = (0..3000)
        .map(|i| format!(r#"{{"rev":"r{i}","object":{{"id":"s-{i}","type":"sticky","text":"a sticky with a long text, number {i}, to pass the size"}}}}"#))
        .collect();
    let large = format!(
        r#"{{"hash":"h","settings":{{}},"settingsRev":"r","objects":[{}]}}"#,
        rows.join(",")
    );
    assert!(large.len() > 256 * 1024);
    let v = vec![
        case(
            "list, the path last",
            &["list", "plan.k5board"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --type",
            &["list", "--type", "sticky", "plan.k5board"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the last word, a file that exists, is the path",
            &["list", "notes"],
            &[],
            vec![],
            a_file,
            2,
            0,
        ),
        case(
            "search",
            &["search", "SHIP", "--file", "plan.k5board"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "read in its fence",
            &["read", "s-1", "plan.k5board"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "read, no such object",
            &["read", "s-9", "plan.k5board"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "read --json",
            &["read", "s-1", "plan.k5board", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json, a workspace",
            &["list", "--file=plan.k5board", "-w", "ws one", "--json"],
            &[],
            vec![ok(LIST)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a large list",
            &["list", "plan.k5board"],
            &[],
            vec![ok(&large)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "new",
            &["new", "sub/plan.k5board"],
            &[],
            vec![ok(r#"{"path":"/w/sub/plan.k5board","hash":"h0"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "fields",
            &["fields", "sticky", "plan.k5board"],
            &[],
            vec![ok(r#"{"types":{"sticky":["text"]}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a board verb, saved",
            &["add", "sticky", "--text", "hi", "plan.k5board"],
            &[],
            vec![ok(
                r#"{"on":{"version":"v2"},"value":{"verb":"add","saved":{"guard":"g2","revs":{"s-9":"r9"}},"labels":{"b":"s-9"},"report":{"added":["s-9"],"moved":[{"id":"s-9"}]}}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a board verb, a dry run",
            &["delete", "s-1", "--dry-run", "plan.k5board"],
            &[],
            vec![ok(
                r#"{"on":{"version":"v2"},"dry_run":true,"value":{"verb":"delete","would_write":null}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a board verb's value in its fence",
            &["get", "s-1", "plan.k5board"],
            &[],
            vec![ok(r#"{"on":{"version":"v3"},"value":{"id":"s-1"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a board verb's value, --json",
            &["get", "s-1", "plan.k5board", "--json"],
            &[],
            vec![ok(r#"{"on":{"version":"v3"},"value":{"id":"s-1"}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a row of the kit",
            &["zap", "plan.k5board"],
            &[],
            vec![answer(
                400,
                r#"{"id":"MPC-E120","code":"unknown_verb","message":"no verb zap","exit":2,"details":{},"next":"see capabilities"}"#,
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "the bridge's own page",
            &["capabilities", "plan.k5board"],
            &[],
            vec![ok(r#"{"stdout":"add\nupdate\n","exit":0}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a merge conflict",
            &["merge", "a.k5board", "b.k5board", "plan.k5board"],
            &[],
            vec![answer(
                409,
                r#"{"conflicts":{"objects":[{"id":"s-1","why":"both moved it"}],"settings":true}}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error and its exit",
            &["list", "plan.k5board"],
            &[],
            vec![answer(403, r#"{"error":{"code":"core_off","hint":"off"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an error, --json",
            &["list", "plan.k5board", "--json"],
            &[],
            vec![answer(500, "")],
            owner_and_port,
            1,
            1,
        ),
        with(
            vec![("K2_PORT", dead_port())],
            case(
                "no answer",
                &["list", "plan.k5board"],
                &[],
                vec![],
                owner_only,
                1,
                0,
            ),
        ),
        case(
            "apply from a file: a fresh group id",
            &["apply", "ops.json", "plan.k5board"],
            &[],
            vec![ok(
                r#"{"wrote":true,"hash":"h2","revs":{"b":"r2","a":"r1"},"removed":{"z":1}}"#,
            )],
            ops_file,
            0,
            1,
        ),
        case(
            "apply: not JSON",
            &["apply", "-", "plan.k5board"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no verb",
            &["--file", "plan.k5board"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "not a .k5board",
            &["list", "--file", "plan.txt"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "list takes no --dry-run",
            &["list", "--dry-run", "plan.k5board"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    for c in v {
        check_masked(&rig, &c);
    }
}
