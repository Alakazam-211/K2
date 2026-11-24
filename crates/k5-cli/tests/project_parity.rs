//! `k5-cli project ...` against the shell CLI's `cli/k2 project ...` on
//! the parity rig ([`common::parity`]): the same requests, stdout, stderr
//! and exit code.
//!
//! - The family alone, an unknown verb, help anywhere on a line that
//!   changes something (help, nothing sent).
//! - Every verb's usage lines.
//! - The `id` tool's session rules: no passport (exit 3), a passport,
//!   a socket that is gone with a passport the daemon refuses (exit 3).
//! - Every verb's answers: `create`, `list` (one answer above 256 KiB),
//!   `show`, `add`, `remove`, `poc`, `msg` (the project found from this
//!   workspace, each delivery, the text on stdin), `read`, `rename`,
//!   `delete`, `pin`, `view` (a project's and one agent's), `access`.
//! - The daemon's errors (each exit), a refusal's permission card lines,
//!   an answer that is not JSON or not ok, no daemon, no answer.
//! - The named difference: `msg` text above 128 KiB, which the shell hands
//!   its Python in the environment (`msg_text_above_128_kib`).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "project";

fn check(rig: &Rig, cases: Vec<Case>) {
    for c in cases {
        rig.check(&c);
    }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn stdin(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn leak(args: &[&str]) -> &'static [&'static str] {
    let owned: Vec<&'static str> = args
        .iter()
        .map(|a| &*Box::leak(a.to_string().into_boxed_str()))
        .collect();
    Box::leak(owned.into_boxed_slice())
}

/// A project's `show` answer: `members` as given.
fn show(id: &str, name: &str, poc: &str, members: &str) -> Reply {
    ok(&format!(
        r#"{{"ok":true,"id":"{id}","name":"{name}","pinned":true,"pocWorkspaceId":"{poc}","defaultView":"terminal","members":[{members}]}}"#
    ))
}

fn member(id: &str, name: &str, path: &str) -> String {
    format!(
        r#"{{"workspaceId":"{id}","name":"{name}","path":"{path}","defaultView":"thread","ownDefaultView":null}}"#
    )
}

#[test]
fn the_family_alone_and_help() {
    let rig = Rig::new(VERB, "alone");
    let mut v = vec![
        case(
            "project alone: its help",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case("an empty verb", &[""], &[], vec![], owner_and_port, 0, 0),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("no daemon: the gate", &["list"], &[], vec![], nothing, 1, 0),
    ];
    for args in [
        &["create", "--help"][..],
        &["create", "Acme", "-h"],
        &["rename", "a", "b", "--help"],
        &["delete", "a", "--yes", "-h"],
        &["pin", "a", "off", "--help"],
        &["add", "a", "qa", "-h"],
        &["remove", "-h"],
        &["poc", "a", "qa", "--help"],
        &["msg", "hello", "-h"],
        &["msg", "--help"],
        &["view", "a", "thread", "-h"],
        &["access", "on", "--help"],
        &["read", "-h"],
    ] {
        v.push(case(
            "help, nothing sent",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn the_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for args in [
        &["create"][..],
        &["create", "a", "b"],
        &["create", "-x"],
        &["rename", "a"],
        &["rename", "a", "b", "c"],
        &["rename", "--x"],
        &["delete"],
        &["delete", "a"],
        &["delete", "a", "b", "--yes"],
        &["delete", "a", "-n"],
        &["pin"],
        &["pin", "a", "maybe"],
        &["pin", "a", "on", "x"],
        &["pin", "a", "-x"],
        &["view"],
        &["view", "a", "split"],
        &["view", "a", "grid"],
        &["view", "a", "thread", "x"],
        &["view", "a", "default"],
        &["view", "a", "--agent"],
        &["view", "a", "--agent", ""],
        &["view", "a", "--x"],
        &["list", "x"],
        &["show"],
        &["show", "a", "b"],
        &["show", "-x"],
        &["add", "a"],
        &["add", "a", "b", "c"],
        &["add", "-x"],
        &["remove", "a"],
        &["remove", "a", "b", "c"],
        &["poc"],
        &["poc", "a", "b", "c"],
        &["poc", "-q"],
        &["msg"],
        &["msg", "a", "b", "c d"],
        &["msg", "--x"],
        &["msg", "--"],
        &["msg", "--", "-x", "--json", "y"],
        &["msg", "--json", "--"],
        &["read", "--after"],
        &["read", "--limit"],
        &["read", "a", "b"],
        &["read", "-x"],
        // project-chat-thread-v1: --file needs a path.
        &["msg", "hi", "--file"],
        &["msg", "Acme", "hi", "--file", ""],
        &["msg", "a", "b", "--file", "x", "c"],
        &["access"],
        &["access", "on", "off"],
        &["access", "on", "--workspace"],
        &["access", "maybe"],
    ] {
        v.push(case(
            "a usage line",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ));
    }
    check(&rig, v);
}

#[test]
fn an_id_tool_in_a_session() {
    let rig = Rig::new(VERB, "session");
    let list = ok(r#"{"ok":true,"groups":[]}"#);
    check(
        &rig,
        vec![
            case(
                "no passport: refused",
                &["list"],
                &[("K2_HOOK_SOCK", "{nosock}"), ("K2_PORT", PORT)],
                vec![],
                owner_and_port,
                3,
                0,
            ),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a passport: sent",
                    &["list"],
                    &[],
                    vec![list.clone()],
                    owner_and_port,
                    0,
                    1,
                ),
            ),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                case(
                    "a socket that is gone, the passport refused",
                    &["list"],
                    &[],
                    vec![Reply::Answer(403, REFUSED.into())],
                    owner_and_port,
                    3,
                    1,
                ),
            ),
            with(
                with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
                case(
                    "a socket that is gone, the passport good",
                    &["list", "--json"],
                    &[],
                    vec![ok(r#"{"role":"canonical"}"#), list.clone()],
                    owner_and_port,
                    0,
                    2,
                ),
            ),
            with(
                with_passport(&[]),
                case(
                    "a passport, no session",
                    &["list"],
                    &[],
                    vec![list],
                    owner_and_port,
                    0,
                    1,
                ),
            ),
        ],
    );
}

#[test]
fn create_list_rename_delete_pin() {
    let rig = Rig::new(VERB, "crud");
    let created = ok(r#"{"ok":true,"id":"pg_1","name":"Acme é","memberCount":"2"}"#);
    let groups = ok(
        r#"{"ok":true,"groups":[{"id":"pg_1","name":"Acme","memberCount":2,"pocWorkspaceId":"ws_1","pinned":true},{"id":"pg_2","name":"Globex research","memberCount":0},{"id":"pg_3","memberCount":null}]}"#,
    );
    let pg1 = show("pg_1", "Acme", "ws_1", &member("ws_1", "qa", "/srv/qa"));
    check(
        &rig,
        vec![
            // covers: project create
            case(
                "create",
                &["create", "Acme é"],
                &[],
                vec![created.clone()],
                owner_and_port,
                0,
                1,
            ),
            // covers: project create --json
            case(
                "create --json",
                &["create", "--json", "Acme"],
                &[],
                vec![created],
                owner_and_port,
                0,
                1,
            ),
            // covers: project list
            case(
                "list",
                &["list"],
                &[],
                vec![groups.clone(), pg1.clone()],
                owner_and_port,
                0,
                2,
            ),
            // covers: project list --json
            case(
                "list --json",
                &["list", "--json"],
                &[],
                vec![groups, pg1],
                owner_and_port,
                0,
                2,
            ),
            case(
                "no projects",
                &["list"],
                &[],
                vec![ok(r#"{"ok":true,"groups":[]}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "rename",
                &["rename", "Acme", "Acme Labs"],
                &[],
                vec![ok(r#"{"ok":true,"id":"pg_1","name":"Acme Labs"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "rename --json",
                &["rename", "Acme", "Acme Labs", "--json"],
                &[],
                vec![ok(r#"{"ok":true,"id":"pg_1","name":"Acme Labs"}"#)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "delete",
                &["delete", "Acme", "-y"],
                &[],
                vec![
                    show("pg_1", "Acme", "", ""),
                    ok(r#"{"ok":true,"id":"pg_1"}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "pin off",
                &["pin", "Acme", "off"],
                &[],
                vec![ok(
                    r#"{"ok":true,"id":"pg_1","name":"Acme","pinned":false}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "pin --json",
                &["pin", "--json", "Acme"],
                &[],
                vec![ok(r#"{"ok":true,"id":"pg_1","name":"Acme","pinned":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ],
    );
}

/// One list answer above 256 KiB, thousands of rows.
#[test]
fn a_list_above_256_kib() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"id":"pg_{i:05}","name":"project number {i} of the large list","memberCount":{},"pinned":{}}}"#,
                i % 7,
                i % 3 == 0
            )
        })
        .collect();
    let body = format!(r#"{{"ok":true,"groups":[{}]}}"#, rows.join(","));
    assert!(
        body.len() > 256 * 1024,
        "the answer is {} bytes",
        body.len()
    );
    check(
        &rig,
        vec![
            case(
                "a large list",
                &["list"],
                &[],
                vec![ok(&body)],
                owner_and_port,
                0,
                1,
            ),
            case(
                "a large list, --json",
                &["list", "--json"],
                &[],
                vec![ok(&body)],
                owner_and_port,
                0,
                1,
            ),
        ],
    );
}

#[test]
fn show_add_remove_poc() {
    let rig = Rig::new(VERB, "members");
    let home = rig.home.display().to_string();
    let members = format!(
        "{},{},{}",
        member("ws_1", "qa", &format!("{home}/agents/qa")),
        r#"{"workspaceId":"ws_2","agentName":"Builder Bot","path":"/srv/builder/"}"#,
        r#"{"workspaceId":"ws_3","path":"/srv/x/notes/"}"#,
    );
    let acme = show("pg_1", "Acme", "ws_1", &members);
    let msgs = ok(
        r#"{"ok":true,"messages":[{"id":"m1","author":"ada","body":"first line\nsecond","createdAt":0},{"id":"m2","author":"qa","body":"","createdAt":4102444800}]}"#,
    );
    check(
        &rig,
        vec![
            // covers: project show
            case(
                "show",
                &["show", "Acme"],
                &[],
                vec![acme.clone(), msgs.clone()],
                owner_and_port,
                0,
                2,
            ),
            // covers: project show --json
            case(
                "show --json",
                &["show", "Acme", "--json"],
                &[],
                vec![acme.clone(), msgs],
                owner_and_port,
                0,
                2,
            ),
            case(
                "show, no members, no messages",
                &["show", "Acme"],
                &[],
                vec![
                    show("pg_1", "Acme", "", ""),
                    ok(r#"{"ok":true,"messages":[]}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "add: became the PoC",
                &["add", "Acme", "qa"],
                &[],
                vec![
                    ok(r#"{"ok":true,"groupId":"pg_1","workspaceId":"ws_1","becamePoc":true}"#),
                    acme.clone(),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "add: a member already",
                &["add", "Acme", "qa"],
                &[],
                vec![
                    ok(r#"{"ok":true,"groupId":"pg_1","workspaceId":"ws_1","alreadyMember":true}"#),
                    acme.clone(),
                ],
                owner_and_port,
                0,
                2,
            ),
            // covers: project add
            case(
                "add: one more",
                &["add", "Acme", "/srv/x/notes"],
                &[],
                vec![
                    ok(r#"{"ok":true,"groupId":"pg_1","workspaceId":"ws_3"}"#),
                    acme.clone(),
                ],
                owner_and_port,
                0,
                2,
            ),
            // covers: project add --json
            case(
                "add --json, not found in the show",
                &["add", "Acme", "zed", "--json"],
                &[],
                vec![
                    ok(r#"{"ok":true,"groupId":"pg_1","workspaceId":"ws_9"}"#),
                    acme.clone(),
                ],
                owner_and_port,
                0,
                2,
            ),
            // covers: project remove
            case(
                "remove",
                &["remove", "Acme", "Builder Bot"],
                &[],
                vec![acme.clone(), ok(r#"{"ok":true,"workspaceId":"ws_2"}"#)],
                owner_and_port,
                0,
                2,
            ),
            // covers: project remove --json
            case(
                "remove --json",
                &["remove", "Acme", "zed", "--json"],
                &[],
                vec![acme.clone(), ok(r#"{"ok":true,"workspaceId":"ws_9"}"#)],
                owner_and_port,
                0,
                2,
            ),
            // covers: project poc
            case(
                "poc",
                &["poc", "Acme"],
                &[],
                vec![acme.clone()],
                owner_and_port,
                0,
                1,
            ),
            case(
                "poc, none",
                &["poc", "Acme"],
                &[],
                vec![show("pg_1", "Acme", "", "")],
                owner_and_port,
                0,
                1,
            ),
            // covers: project poc --json
            case(
                "poc --json",
                &["poc", "Acme", "--json"],
                &[],
                vec![acme.clone()],
                owner_and_port,
                0,
                1,
            ),
            case(
                "poc: reassigned",
                &["poc", "Acme", "Builder Bot"],
                &[],
                vec![acme.clone(), ok(r#"{"ok":true,"pocWorkspaceId":"ws_2"}"#)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "poc --json: reassigned",
                &["poc", "Acme", "zed", "--json"],
                &[],
                vec![acme, ok(r#"{"ok":true,"pocWorkspaceId":"ws_9"}"#)],
                owner_and_port,
                0,
                2,
            ),
        ],
    );
}

#[test]
fn msg_and_read() {
    let rig = Rig::new(VERB, "msg");
    let me = rig.proj.display().to_string();
    let mine = show("pg_1", "Acme", "ws_1", &member("ws_1", "acme-agent", &me));
    let other = show("pg_2", "Globex", "", &member("ws_7", "x", "/srv/x"));
    let list = ok(r#"{"ok":true,"groups":[{"id":"pg_1"},{"id":"pg_2"}]}"#);
    let posted = |reason: &str, delivered: bool| {
        ok(&format!(
            r#"{{"ok":true,"id":"m9","groupName":"Acme","author":"acme-agent","delivered":{delivered},"deliveryReason":{reason},"deliveredSessionId":"s1"}}"#
        ))
    };
    let msgs = ok(
        r#"{"ok":true,"truncated":true,"messages":[{"id":"m1","author":"ada","body":"hi","createdAt":1700000000}]}"#,
    );
    let with_files = ok(
        r#"{"ok":true,"id":"m9","groupName":"Acme","author":"acme-agent","delivered":false,"deliveryReason":"queued","attachments":[{"kind":"file","path":".k2/downloads/projects/pg_1/gone.md","line":3,"lineEnd":9,"size":5},{"kind":"file","path":"hosts","size":12}]}"#,
    );
    let deleted = ok(
        r#"{"ok":true,"messages":[{"id":"m1","author":"ada","body":"hi","createdAt":1700000000},{"id":"m2","author":"bo","body":"","deleted":true,"createdAt":1700000001}]}"#,
    );
    check(
        &rig,
        vec![
            // covers: project msg
            case(
                "msg: the project found, delivered",
                &["msg", "Ship it"],
                &[],
                vec![
                    list.clone(),
                    mine.clone(),
                    other.clone(),
                    posted("null", true),
                ],
                owner_and_port,
                0,
                4,
            ),
            // After a bare `--` the words are the text as typed.
            case(
                "msg: a text that starts with a dash, after --",
                &["msg", "--json", "Acme", "--", "-- --help"],
                &[],
                vec![mine.clone(), posted("null", true)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "msg: the author is the PoC",
                &["msg", "Acme", "Ship it"],
                &[],
                vec![mine.clone(), posted("\"author-is-poc\"", false)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "msg: not delivered",
                &["msg", "Acme", "Ship it"],
                &[],
                vec![mine.clone(), posted("\"session_gone\"", false)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "msg: queued",
                &["msg", "Acme", "Ship it"],
                &[],
                vec![mine.clone(), posted("\"queued\"", false)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "msg: no PoC",
                &["msg", "Acme", "Ship it", "--json"],
                &[],
                vec![mine.clone(), posted("\"no_poc\"", false)],
                owner_and_port,
                0,
                2,
            ),
            // covers: project msg --json
            case(
                "msg --json: delivered",
                &["msg", "--json", "Acme", "x"],
                &[("K2_AGENT_NAME", "acme-agent")],
                vec![other.clone(), posted("\"x\"", true)],
                owner_and_port,
                0,
                2,
            ),
            stdin(
                b"from stdin\n\n",
                case(
                    "msg: the text on stdin",
                    &["msg", "Acme", "-"],
                    &[],
                    vec![mine.clone(), posted("null", true)],
                    owner_and_port,
                    0,
                    2,
                ),
            ),
            case(
                "msg: the first word is no project",
                &["msg", "Ship", "it \"now\""],
                &[],
                vec![Reply::Answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no project 'Ship'"}}"#.into(),
                )],
                owner_and_port,
                4,
                1,
            ),
            case(
                "msg: no project",
                &["msg", "hi"],
                &[],
                vec![list.clone(), other.clone(), other.clone()],
                owner_and_port,
                4,
                3,
            ),
            case(
                "msg: two projects",
                &["msg", "hi"],
                &[],
                vec![list.clone(), mine.clone(), mine.clone()],
                owner_and_port,
                4,
                3,
            ),
            // covers: project read
            case(
                "read",
                &["read", "Acme"],
                &[],
                vec![mine.clone(), msgs.clone()],
                owner_and_port,
                0,
                2,
            ),
            // covers: project read --json
            case(
                "read --json, --after, --limit",
                &["read", "Acme", "--after", "17", "--limit", "5", "--json"],
                &[],
                vec![mine.clone(), msgs.clone()],
                owner_and_port,
                0,
                2,
            ),
            case(
                "read: the project found",
                &["read"],
                &[],
                vec![list, mine.clone(), other, msgs],
                owner_and_port,
                0,
                4,
            ),
            // project-chat-thread-v1: files ride the line; a spec that is no
            // file here is sent as typed with its lines, one that is goes
            // absolute; the answer's files are listed.
            // covers: project msg
            case(
                "msg --file: files sent and listed",
                &[
                    "msg",
                    "Acme",
                    "Notes",
                    "--file",
                    "gone.md:3-9",
                    "--file",
                    "/etc/hosts",
                ],
                &[],
                vec![mine.clone(), with_files.clone()],
                owner_and_port,
                0,
                2,
            ),
            // covers: project msg --json
            case(
                "msg --file --json: files alone",
                &["msg", "Acme", "", "--file", "gone.md", "--json"],
                &[],
                vec![mine.clone(), with_files.clone()],
                owner_and_port,
                0,
                2,
            ),
            case(
                "msg --file: a server that drops files",
                &["msg", "Acme", "hi", "--file", "gone.md"],
                &[],
                vec![mine.clone(), posted("null", true)],
                owner_and_port,
                1,
                2,
            ),
            // covers: project read
            case(
                "read --all: a deleted line",
                &["read", "Acme", "--all"],
                &[],
                vec![mine.clone(), deleted.clone()],
                owner_and_port,
                0,
                2,
            ),
            // covers: project read --json
            case(
                "read --all --json: a deleted line",
                &["read", "Acme", "--all", "--json"],
                &[],
                vec![mine, deleted],
                owner_and_port,
                0,
                2,
            ),
        ],
    );
}

#[test]
fn view_and_access() {
    let rig = Rig::new(VERB, "view");
    let members = format!(
        "{},{}",
        member("ws_1", "qa", "/srv/qa"),
        r#"{"workspaceId":"ws_2","agentName":"Builder Bot","path":"/srv/builder","defaultView":"chatter","ownDefaultView":"chatter"}"#
    );
    let acme = show("pg_1", "Acme", "ws_1", &members);
    check(
        &rig,
        vec![
            // covers: project view
            case(
                "view",
                &["view", "Acme"],
                &[],
                vec![acme.clone()],
                owner_and_port,
                0,
                1,
            ),
            // covers: project view --json
            case(
                "view --json",
                &["view", "Acme", "--json"],
                &[],
                vec![acme.clone()],
                owner_and_port,
                0,
                1,
            ),
            case(
                "view: set",
                &["view", "Acme", "Thread"],
                &[],
                vec![
                    acme.clone(),
                    ok(r#"{"ok":true,"name":"Acme","defaultView":"thread"}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "view: one agent's",
                &["view", "Acme", "--agent", "builder bot"],
                &[],
                vec![acme.clone()],
                owner_and_port,
                0,
                1,
            ),
            case(
                "view: one agent's, by path",
                &["view", "Acme", "--agent", "/srv/qa/", "--json"],
                &[],
                vec![acme.clone()],
                owner_and_port,
                0,
                1,
            ),
            case(
                "view: one agent's, set",
                &["view", "Acme", "chatter", "--agent", "qa"],
                &[],
                vec![
                    acme.clone(),
                    ok(r#"{"ok":true,"defaultView":"chatter","ownDefaultView":"chatter"}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "view: one agent's, back to the project's",
                &["view", "Acme", "default", "--agent", "ws_2"],
                &[],
                vec![
                    acme.clone(),
                    ok(r#"{"ok":true,"defaultView":"terminal","ownDefaultView":null}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "view: not a member",
                &["view", "Acme", "--agent", "zed"],
                &[],
                vec![acme],
                owner_and_port,
                1,
                1,
            ),
            case(
                "view: too old",
                &["view", "Acme"],
                &[],
                vec![ok(r#"{"ok":true,"id":"pg_1","name":"Acme"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "view: too old for an agent's own",
                &["view", "Acme", "--agent", "qa"],
                &[],
                vec![ok(
                    r#"{"ok":true,"id":"pg_1","name":"Acme","defaultView":"thread","members":[{"workspaceId":"ws_1","name":"qa"}]}"#,
                )],
                owner_and_port,
                1,
                1,
            ),
            case(
                "access on",
                &["access", "on", "--workspace", "qa"],
                &[],
                vec![ok(
                    r#"{"ok":true,"workspace":"qa","agentsCanManageProjects":true}"#,
                )],
                owner_and_port,
                0,
                1,
            ),
            case(
                "access off, this workspace, --json",
                &["access", "off", "--json"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                0,
                1,
            ),
        ],
    );
}

#[test]
fn the_daemon_s_errors() {
    let rig = Rig::new(VERB, "errors");
    let ask = r#"{"error":{"code":"forbidden","hint":"Only the owner or an agent with Manage K2 projects."},"permission":{"command":"k2 permission request --scope projects.manage --why \"...\"","requestable":true}}"#;
    let cannot = r#"{"error":"forbidden","permission":{"requestable":false,"hint":"Projects are the owner's."}}"#;
    check(
        &rig,
        vec![
            case(
                "not found",
                &["show", "Nope"],
                &[],
                vec![Reply::Answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no project 'Nope'"}}"#.into(),
                )],
                owner_and_port,
                4,
                1,
            ),
            case(
                "ambiguous",
                &["show", "A"],
                &[],
                vec![ok(
                    r#"{"error":{"code":"ambiguous_id","hint":"Acme, Atlas"}}"#,
                )],
                owner_and_port,
                4,
                1,
            ),
            case(
                "a name taken",
                &["create", "Acme"],
                &[],
                vec![Reply::Answer(
                    409,
                    r#"{"error":{"code":"name_taken","hint":"Acme exists"}}"#.into(),
                )],
                owner_and_port,
                1,
                1,
            ),
            case(
                "usage from the daemon",
                &["create", "Acme"],
                &[],
                vec![ok(r#"{"error":{"code":"usage"}}"#)],
                owner_and_port,
                2,
                1,
            ),
            case(
                "an error as text",
                &["create", "Acme"],
                &[],
                vec![ok(r#"{"error":"nope"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a refusal: the permission card's line",
                &["create", "Acme"],
                &[],
                vec![Reply::Answer(403, ask.into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a refusal that can't be requested",
                &["rename", "Acme", "B"],
                &[],
                vec![Reply::Answer(403, cannot.into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a refusal that is not JSON",
                &["create", "Acme"],
                &[],
                vec![Reply::Answer(502, "Bad Gateway".into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "not ok",
                &["create", "Acme"],
                &[],
                vec![ok(r#"{"name":"é"}"#)],
                owner_and_port,
                1,
                1,
            ),
            case(
                "not JSON",
                &["create", "Acme"],
                &[],
                vec![ok("<html>")],
                owner_and_port,
                1,
                1,
            ),
            traceback(case(
                "a list",
                &["create", "Acme"],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            )),
            traceback(case(
                "an error that is a list",
                &["create", "Acme"],
                &[],
                vec![ok(r#"{"error":[1]}"#)],
                owner_and_port,
                1,
                1,
            )),
            case(
                "an empty reply",
                &["list"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            ),
        ],
    );
    let port = dead_port();
    rig.check(&case(
        "no daemon",
        &["list"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// The named difference: `msg`'s text rides the shell's Python
/// environment, and Linux refuses one environment string over 128 KiB:
/// the shell prints `Argument list too long`, exits 126 and sends
/// nothing; the binary sends the message.
#[cfg(target_os = "linux")]
#[test]
fn msg_text_above_128_kib() {
    let rig = Rig::new(VERB, "bigmsg");
    let me = rig.proj.display().to_string();
    static TEXT: [u8; 200 * 1024] = [b'x'; 200 * 1024];
    let text = std::str::from_utf8(&TEXT).expect("ASCII");
    let c = stdin(
        &TEXT,
        case(
            "a message above 128 KiB",
            &["msg", "Acme", "-"],
            &[],
            vec![
                show("pg_1", "Acme", "ws_1", &member("ws_1", "acme-agent", &me)),
                ok(r#"{"ok":true,"id":"m9","groupName":"Acme","delivered":true}"#),
            ],
            owner_and_port,
            126,
            0,
        ),
    );
    rig.prepare(Side::Bash, &c);
    let b = rig.run(true, &c);
    let bash_sent = rig.read(Side::Bash, false, c.name);
    assert_eq!(b.code, Some(126), "bash: {b:?}");
    assert!(
        b.stderr.contains("Argument list too long"),
        "bash: {}",
        b.stderr
    );
    assert!(bash_sent.is_empty(), "bash sent {}", bash_sent.len());
    // The binary's own stub gets the same answers.
    rig.prepare(Side::Rust, &c);
    let r = rig.run(false, &c);
    let sent = rig.read(Side::Rust, true, c.name);
    assert_eq!(
        (r.code, r.stdout.as_str(), r.stderr.as_str()),
        (
            Some(0),
            "Posted to Acme \u{2014} delivered to PoC acme-agent.\n",
            ""
        ),
        "the binary"
    );
    assert_eq!(sent.len(), 2, "the binary's requests");
    let post = String::from_utf8_lossy(&sent[1].bytes);
    assert!(post.contains(text), "the message went out whole");
}
