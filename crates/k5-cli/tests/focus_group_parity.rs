//! `k5-cli focus-group` (and `agent set --focus-group`) against the shell
//! CLI's `cli/k2`, on the parity rig ([`common::parity`]): the same
//! requests, stdout, stderr and exit code for every form.
//!
//! - The verbs: the bare noun, `list` (`ls`, and the `focus-groups`
//!   alias), `create`, `rename`, `add`, `remove` (`rm`), each with
//!   `--json`; a group by its id, its name in any case and the start of
//!   one name; a workspace by name, path, `~` and the folder; nothing to
//!   change; many groups (an answer above 256 KiB).
//! - The words: `--` (a word after it is a name), an unknown flag (with
//!   and without `--json` before it), each verb's missing or extra words,
//!   an empty name, an unknown verb, an empty first word.
//! - The answers: an agent's change refused with the card's closing line
//!   (and one that can't be requested), a 401, a 404 with a message, a
//!   5xx, not JSON, empty, not a list, a row that is no dict and a name
//!   that is no string (Python's raise), an unknown or ambiguous group, an
//!   unknown workspace.
//! - The owner from disk, a passport over TCP, a live cell socket (the
//!   list over TCP; a change on the socket, and "not served" goes on to
//!   TCP; a change that may have been applied), the id tool's checks, a
//!   re-minted and a healed passport; the gates.
//! - No answer (an empty reply, a reset, not HTTP, nothing listening).
//! - Help never acts: `-h` and `--help` alone are the front's page, and
//!   anywhere else the shell's, each sending nothing.
//! - `agent set --focus-group`: the plan, the assign (with the steps
//!   before it), `none`, already so, `--dry-run`, `--json`, the list's
//!   failures, the assign refused (with the card's line), its errors and
//!   no answer, and a session's passport.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "focus-group";

const GROUPS: &str = r#"[{"id":"g-infra","name":"Infra","color":null,"tabOrder":0,"createdAt":1},{"id":"g-ca","name":"Client A","color":null,"tabOrder":1,"createdAt":2},{"id":"g-cb","name":"Client B","color":null,"tabOrder":2,"createdAt":3}]"#;

const CONF_SALES: &str =
    r#"{"ok":true,"id":"w-sales","name":"sales","path":"/tmp/sales","focusGroupId":null}"#;

const CONF_IN: &str = r#"{"ok":true,"id":"w-in","name":"inside","path":"/tmp/inside","focusGroupId":"g-infra","enabled":true}"#;

const CREATED: &str = r#"{"id":"g-new","name":"Lab","color":null,"tabOrder":3,"createdAt":4}"#;

const ASSIGNED: &str = r#"{"id":"w-sales","focusGroupId":"g-ca"}"#;

const CARD_CREATE: &str = r#"{"error":{"code":"focus_group_needs_person","hint":"Creating focus group is a person's step.","name":"Lab"},"permission":{"action":"focus-groups.create","command":"k2 permission request focus-groups.create name='Lab' --why \"<what you need it for>\"","approvers":"owner or admin"}}"#;

const CARD_ASSIGN: &str = r#"{"error":{"code":"focus_group_needs_person","hint":"Filling a group is a person's step.","target":"sales","group":"Infra"},"permission":{"action":"focus-groups.assign","command":"k2 permission request focus-groups.assign group=Infra target=sales --why \"<what you need it for>\"","approvers":"owner or admin"}}"#;

const NOT_ASKABLE: &str = r#"{"error":{"code":"focus_group_needs_person","hint":"A person's step."},"permission":{"requestable":false,"hint":"Only a person at the machine can do this."}}"#;

fn not_served() -> Reply {
    Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into())
}

fn many(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"g-{i:05}","name":"group {i} {}","color":null,"tabOrder":{i},"createdAt":{i}}}"#,
                "é".repeat(i % 7)
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn compared(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn usage(args: &'static [&'static str]) -> Case {
    case("a usage line", args, &[], vec![], owner_and_port, 2, 0)
}

#[test]
fn focus_group_lists_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let big = many(6000);
    assert!(
        big.len() > 256 * 1024,
        "the large list is {} bytes",
        big.len()
    );
    let v = vec![
        // covers: focus-group
        case(
            "the bare noun",
            &[],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: focus-group --json
        case(
            "--json alone",
            &["--json"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            0,
            1,
        ),
        // native: focus-group list
        case(
            "list",
            &["list"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            0,
            1,
        ),
        // native: focus-group ls
        case("ls", &["ls"], &[], vec![ok(GROUPS)], owner_and_port, 0, 1),
        // native: focus-group list --json
        case(
            "list --json",
            &["list", "--json"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --",
            &["list", "--"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no groups",
            &["list"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no groups --json",
            &["--json"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not a list",
            &["list"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "null",
            &["list", "--json"],
            &[],
            vec![ok("null")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd rows",
            &["list"],
            &[],
            vec![ok(
                r#"[{"name":"","id":"g1"},{"name":"a name longer than twenty-four","id":5},{"name":7},{"id":null,"name":"été"},{"name":["x"],"id":{"a":1}}]"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        compared(
            Compare::Traceback,
            case(
                "a row that is no dict",
                &["list"],
                &[],
                vec![ok(r#"[{"name":"a","id":"b"},3]"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        case(
            "many groups",
            &["list"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many groups --json",
            &["--json"],
            &[],
            vec![ok(&big)],
            owner_and_port,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let rig = Rig::new("focus-groups", "alias");
    for c in [
        // native: focus-groups
        case(
            "the alias",
            &[],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the alias, create",
            &["create", "Lab"],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn focus_group_changes_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let updated = r#"{"id":"g-infra","name":"Ops","color":null,"tabOrder":0,"createdAt":1}"#;
    let v = vec![
        // native: focus-group create
        case(
            "create",
            &["create", "Client work"],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        // native: focus-group create --json
        case(
            "create --json",
            &["create", "Lab", "--json"],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "create --json, an answer with its own newline",
            &["create", "--json", "Lab"],
            &[],
            vec![ok("{\"id\":\"g-new\"}\n")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "create, a name past ASCII",
            &["create", "été ü"],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "create, a tab is a name",
            &["create", "\t"],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "create, after --",
            &["create", "--", "--json"],
            &[],
            vec![ok(CREATED)],
            owner_and_port,
            0,
            1,
        ),
        // native: focus-group rename
        case(
            "rename, the name in any case",
            &["rename", "infra", "Ops"],
            &[],
            vec![ok(GROUPS), ok(updated)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "rename by id",
            &["rename", "g-cb", "B2"],
            &[],
            vec![ok(GROUPS), ok(updated)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "rename by the start of one name",
            &["rename", "Inf", "Ops"],
            &[],
            vec![ok(GROUPS), ok(updated)],
            owner_and_port,
            0,
            2,
        ),
        // native: focus-group rename --json
        case(
            "rename --json",
            &["rename", "Client A", "Ops", "--json"],
            &[],
            vec![ok(GROUPS), ok(updated)],
            owner_and_port,
            0,
            2,
        ),
        // native: focus-group add
        case(
            "add",
            &["add", "client a", "sales"],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        // native: focus-group add --json
        case(
            "add --json",
            &["add", "client a", "sales", "--json"],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, already there",
            &["add", "Infra", "inside"],
            &[],
            vec![ok(GROUPS), ok(CONF_IN)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "add --json, already there",
            &["add", "--json", "Infra", "inside"],
            &[],
            vec![ok(GROUPS), ok(CONF_IN)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "add, a workspace by path",
            &["add", "Infra", "/tmp/sales"],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, a workspace under HOME",
            &["add", "Infra", "~/ws/sales"],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, a relative path",
            &["add", "Infra", "./sub/sales"],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, an empty workspace is the folder",
            &["add", "Infra", ""],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, a name with a newline",
            &["add", "Infra", "two"],
            &[],
            vec![
                ok(GROUPS),
                ok(r#"{"ok":true,"id":"w2","name":"two\nlines","focusGroupId":null}"#),
                ok(ASSIGNED),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, a conf with no name",
            &["add", "Infra", "p"],
            &[],
            vec![
                ok(GROUPS),
                ok(r#"{"ok":true,"id":7,"path":"/tmp/p"}"#),
                ok(ASSIGNED),
            ],
            owner_and_port,
            0,
            3,
        ),
        case(
            "add, a group with no id",
            &["add", "Loose", "sales"],
            &[],
            vec![ok(r#"[{"name":"Loose"}]"#), ok(CONF_SALES)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "add, a group whose id is a number",
            &["add", "Num", "sales"],
            &[],
            vec![
                ok(r#"[{"id":5,"name":"Num"}]"#),
                ok(CONF_SALES),
                ok(ASSIGNED),
            ],
            owner_and_port,
            0,
            3,
        ),
        // native: focus-group remove
        case(
            "remove",
            &["remove", "Infra", "inside"],
            &[],
            vec![ok(GROUPS), ok(CONF_IN), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        // native: focus-group rm
        // native: focus-group rm --json
        case(
            "rm --json",
            &["rm", "Infra", "inside", "--json"],
            &[],
            vec![ok(GROUPS), ok(CONF_IN), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "remove from a group it isn't in",
            &["remove", "Infra", "sales"],
            &[],
            vec![ok(GROUPS), ok(CONF_SALES)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "remove --json from a group it isn't in",
            &["remove", "Client B", "inside", "--json"],
            &[],
            vec![ok(GROUPS), ok(CONF_IN)],
            owner_and_port,
            0,
            2,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn focus_group_stops_as_the_shell_does() {
    let rig = Rig::new(VERB, "stops");
    let mut v = vec![
        usage(&["create"]),
        usage(&["create", "a", "b"]),
        usage(&["create", "  "]),
        usage(&["rename", "Infra"]),
        usage(&["rename", "Infra", "  "]),
        usage(&["add", "Infra"]),
        usage(&["remove", "Infra"]),
        usage(&["rm", "a", "b", "c"]),
        usage(&["list", "extra"]),
        usage(&["ls", "--json", "x"]),
        usage(&["bogus"]),
        usage(&["bogus", "--json"]),
        usage(&["create", "X", "--bogus"]),
        usage(&["create", "X", "--bogus", "--json"]),
        usage(&["--json", "--bogus"]),
        usage(&["list", "-"]),
        usage(&[""]),
        case(
            "an unknown group",
            &["add", "Nope", "sales"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an unknown group --json",
            &["rename", "Nope", "x", "--json"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an ambiguous group",
            &["add", "client", "sales"],
            &[],
            vec![ok(GROUPS)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an ambiguous group, nameless rows",
            &["add", "", "sales"],
            &[],
            vec![ok(
                r#"[{"id":"z1","name":""},{"id":"a1","name":null},{"name":""}]"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "an unknown workspace",
            &["add", "Infra", "nope"],
            &[],
            vec![
                ok(GROUPS),
                Reply::Answer(
                    404,
                    r#"{"error":{"code":"not_found","hint":"no workspace matches 'nope'"}}"#.into(),
                ),
            ],
            owner_and_port,
            4,
            2,
        ),
        case(
            "the list fails before the workspace",
            &["remove", "Infra", "sales"],
            &[],
            vec![Reply::Answer(500, r#"{"error":"db locked"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in [
        case(
            "a row that is no dict",
            &["add", "x", "sales"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a name that is no string",
            &["rename", "zz", "y"],
            &[],
            vec![ok(r#"[{"id":"x","name":5}]"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a name that is no string, at the start",
            &["rename", "zz", "y"],
            &[],
            vec![ok(r#"[{"id":"x","name":"a"},{"id":"y","name":[1]}]"#)],
            owner_and_port,
            1,
            1,
        ),
    ] {
        v.push(compared(Compare::Traceback, c));
    }
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn focus_group_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "answers");
    let v = vec![
        case(
            "create refused: the card's line",
            &["create", "Lab"],
            &[],
            vec![Reply::Answer(403, CARD_CREATE.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "create refused --json",
            &["create", "Lab", "--json"],
            &[],
            vec![Reply::Answer(403, CARD_CREATE.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "rename refused",
            &["rename", "Infra", "Ops"],
            &[],
            vec![ok(GROUPS), Reply::Answer(403, CARD_CREATE.into())],
            owner_and_port,
            3,
            2,
        ),
        case(
            "add refused",
            &["add", "Infra", "sales"],
            &[],
            vec![
                ok(GROUPS),
                ok(CONF_SALES),
                Reply::Answer(403, CARD_ASSIGN.into()),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "remove refused",
            &["remove", "Infra", "inside"],
            &[],
            vec![
                ok(GROUPS),
                ok(CONF_IN),
                Reply::Answer(403, CARD_ASSIGN.into()),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "refused, and it can't be requested",
            &["create", "Lab"],
            &[],
            vec![Reply::Answer(403, NOT_ASKABLE.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 401",
            &["list"],
            &[],
            vec![Reply::Answer(401, r#"{"error":{"message":"who?"}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 404 with a message",
            &["list"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"gone","message":"no such route"}}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "a 409 with a code and no hint",
            &["create", "Lab"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":{"code":"duplicate"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx",
            &["create", "Lab"],
            &[],
            vec![Reply::Answer(500, r#"{"oops":1}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a 5xx, empty",
            &["create", "Lab"],
            &[],
            vec![Reply::Answer(503, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["create", "Lab"],
            &[],
            vec![ok("not json at all")],
            owner_and_port,
            1,
            1,
        ),
        case("empty", &["list"], &[], vec![ok("")], owner_and_port, 1, 1),
        case(
            "an error in a 200",
            &["create", "Lab", "--json"],
            &[],
            vec![ok(r#"{"error":"busy"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal that is no object",
            &["list"],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a refusal that is no object --json",
            &["list", "--json"],
            &[],
            vec![Reply::Answer(403, "[1]".into())],
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
fn focus_group_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = with_passport;
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        with(
            h(&[]),
            case(
                "the passport over TCP",
                &["list"],
                &[],
                vec![ok(GROUPS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a change over TCP, refused",
                &["create", "Lab"],
                &[],
                vec![Reply::Answer(403, CARD_CREATE.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport",
                &["list"],
                &[],
                vec![ok(GROUPS)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["list"],
                &[],
                vec![heal(&rig), ok(GROUPS)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["create", "Lab"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                port_only,
                3,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone, the passport good: an agent's change refused",
                &["add", "Infra", "sales"],
                &[],
                vec![
                    ok(r#"{"ok":true}"#),
                    ok(GROUPS),
                    ok(CONF_SALES),
                    Reply::Answer(403, CARD_ASSIGN.into()),
                ],
                port_only,
                3,
                4,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(),
            case(
                "the cell socket: the list over TCP",
                &["list"],
                &[],
                vec![ok(GROUPS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket: a change there",
                &["create", "Lab"],
                &[],
                vec![ok(CREATED)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "not served: TCP",
                &["create", "Lab"],
                &[],
                vec![not_served(), ok(CREATED)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "not served, then refused over TCP",
                &["create", "Lab"],
                &[],
                vec![not_served(), Reply::Answer(403, CARD_CREATE.into())],
                port_only,
                3,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "add on the cell socket, refused",
                &["add", "Infra", "sales"],
                &[],
                vec![
                    ok(GROUPS),
                    ok(CONF_SALES),
                    Reply::Answer(403, CARD_ASSIGN.into()),
                ],
                port_only,
                3,
                3,
            ),
        ),
        compared(
            Compare::Waited,
            with(
                cell(),
                case(
                    "no answer to a change on the cell socket: not sent again",
                    &["create", "Lab"],
                    &[],
                    vec![Reply::Close],
                    port_only,
                    1,
                    1,
                ),
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn focus_group_with_no_answer() {
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
            &["create", "Lab"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["list"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listens",
            &["create", "Lab"],
            &[("K2_PORT", port.as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
        compared(
            Compare::Waited,
            case(
                "no answer to the conf",
                &["add", "Infra", "sales"],
                &[],
                vec![ok(GROUPS), Reply::Close],
                owner_and_port,
                1,
                2,
            ),
        ),
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case("no token", &["list"], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &["create"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &["list"],
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

/// Help never acts: `-h` and `--help` alone are the front's page; on any
/// other line the shell prints the help. Each sends nothing.
#[test]
fn focus_group_help_never_acts() {
    let rig = Rig::new(VERB, "help");
    for args in [&["-h"][..], &["--help"]] {
        rig.check(&case(
            "the help page",
            leak(args),
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
    // native: focus-group list --help
    // native: focus-group create --help
    // native: focus-group rename --help
    // native: focus-group add --help
    // native: focus-group remove --help
    // native: focus-group --help
    for args in [
        &["list", "-h"][..],
        &["create", "X", "-h"],
        &["create", "-h", "X"],
        &["rename", "Infra", "Ops", "--help"],
        &["add", "Infra", "sales", "-h"],
        &["remove", "Infra", "sales", "--help"],
        &["rm", "--help", "Infra", "sales"],
        &["bogus", "-h"],
        &["--json", "-h"],
    ] {
        rig.check(&compared(
            Compare::Shell,
            case(
                "help on the line",
                leak(args),
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
    let rig = Rig::new("agent", "fghelp");
    for args in [
        &["set", "sales", "--focus-group", "Infra", "-h"][..],
        &["set", "sales", "-h", "--focus-group", "Infra"],
        &["set", "sales", "--focus-group", "--help"],
    ] {
        rig.check(&compared(
            Compare::Shell,
            case(
                "agent set's help",
                leak(args),
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn focus_group_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    rig.check(&compared(
        Compare::Shell,
        case(
            "a port that is no port: the shell's port refusal",
            &["list"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        ),
    ));
}

#[test]
fn agent_set_focus_group_as_the_shell_does() {
    let rig = Rig::new("agent", "fgset");
    let set_ok = r#"{"ok":true}"#;
    let mut v = vec![
        case(
            "a focus group",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "a focus group --json",
            &["set", "sales", "--focus-group", "client a", "--json"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "the start of one name",
            &["set", "sales", "--focus-group", "inf"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), ok(ASSIGNED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "none",
            &["set", "inside", "--focus-group", "none"],
            &[],
            vec![ok(CONF_IN), ok(ASSIGNED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "-, --json",
            &["set", "inside", "--focus-group", "-", "--json"],
            &[],
            vec![ok(CONF_IN), ok(ASSIGNED)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "already so",
            &["set", "inside", "--focus-group", "INFRA"],
            &[],
            vec![ok(CONF_IN), ok(GROUPS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "already in none",
            &["set", "sales", "--focus-group", "Off"],
            &[],
            vec![ok(CONF_SALES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--dry-run",
            &["set", "sales", "--focus-group", "Infra", "--dry-run"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "--dry-run --json",
            &[
                "set",
                "sales",
                "--focus-group",
                "Infra",
                "--dry-run",
                "--json",
            ],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "only spaces: no step",
            &["set", "sales", "--focus-group", "  "],
            &[],
            vec![ok(CONF_SALES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an unknown group",
            &["set", "sales", "--focus-group", "Nope"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS)],
            owner_and_port,
            4,
            2,
        ),
        case(
            "an ambiguous group",
            &["set", "sales", "--focus-group", "client"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS)],
            owner_and_port,
            4,
            2,
        ),
        case(
            "the list refused",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![
                ok(CONF_SALES),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"nope","hint":"no list for you"}}"#.into(),
                ),
            ],
            owner_and_port,
            1,
            2,
        ),
        case(
            "the list down",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), Reply::Answer(500, "down".into())],
            owner_and_port,
            1,
            2,
        ),
        case(
            "the list not a list",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), ok(r#"{"ok":true}"#)],
            owner_and_port,
            1,
            2,
        ),
        case(
            "the assign refused: the card's line",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![
                ok(CONF_SALES),
                ok(GROUPS),
                Reply::Answer(403, CARD_ASSIGN.into()),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "the assign refused --json",
            &["set", "sales", "--focus-group", "Infra", "--json"],
            &[],
            vec![
                ok(CONF_SALES),
                ok(GROUPS),
                Reply::Answer(403, CARD_ASSIGN.into()),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "the assign refused, and it can't be requested",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![
                ok(CONF_SALES),
                ok(GROUPS),
                Reply::Answer(403, NOT_ASKABLE.into()),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "the assign down",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![
                ok(CONF_SALES),
                ok(GROUPS),
                Reply::Answer(500, "down".into()),
            ],
            owner_and_port,
            1,
            3,
        ),
        case(
            "the assign answers no JSON",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), ok("nope")],
            owner_and_port,
            1,
            3,
        ),
        case(
            "the assign answers an error",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), ok(r#"{"error":"busy"}"#)],
            owner_and_port,
            1,
            3,
        ),
        case(
            "the assign not found",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![
                ok(CONF_SALES),
                ok(GROUPS),
                Reply::Answer(404, r#"{"error":{"code":"not_found"}}"#.into()),
            ],
            owner_and_port,
            4,
            3,
        ),
        case(
            "no answer to the assign",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), Reply::Close],
            owner_and_port,
            1,
            3,
        ),
        case(
            "enabled, then the focus group",
            &[
                "set",
                "sales",
                "--enabled",
                "true",
                "--focus-group",
                "Infra",
            ],
            &[],
            vec![ok(CONF_SALES), ok(GROUPS), ok(set_ok), ok(ASSIGNED)],
            owner_and_port,
            0,
            4,
        ),
        case(
            "enabled done, then the focus group refused",
            &[
                "set",
                "sales",
                "--enabled",
                "true",
                "--focus-group",
                "Infra",
            ],
            &[],
            vec![
                ok(CONF_SALES),
                ok(GROUPS),
                ok(set_ok),
                Reply::Answer(403, CARD_ASSIGN.into()),
            ],
            owner_and_port,
            3,
            4,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "a session's passport, no owner on disk: the assign refused",
                &["set", "sales", "--focus-group", "Infra"],
                &[],
                vec![
                    ok(CONF_SALES),
                    ok(GROUPS),
                    Reply::Answer(403, CARD_ASSIGN.into()),
                ],
                port_only,
                3,
                3,
            ),
        ),
    ];
    v.push(compared(
        Compare::Traceback,
        case(
            "a row that is no dict",
            &["set", "sales", "--focus-group", "Infra"],
            &[],
            vec![ok(CONF_SALES), ok("[1]")],
            owner_and_port,
            1,
            2,
        ),
    ));
    v.push(usage(&["set", "sales", "--focus-group"]));
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2` or `k5`.
#[test]
fn focus_group_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let c = case("brand", &["create"], &[], vec![], owner_and_port, 2, 0);
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("Error (usage): create takes one name (usage: {cli} focus-group list | create <name> | rename <group> <new-name> | add <group> <workspace> | remove <group> <workspace>)\n")
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
    let c = case(
        "brand",
        &["list"],
        &[],
        vec![ok("[]")],
        owner_and_port,
        0,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stdout,
        format!("No focus groups. Create one: {cli} focus-group create <name>\n")
    );
    let c = case(
        "brand",
        &["add", "zz", "sales"],
        &[],
        vec![ok(GROUPS)],
        owner_and_port,
        4,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("Error (not_found): no focus group matches 'zz' (see: {cli} focus-group list)\n")
    );
    let rig = Rig::new("agent", "fgbrand");
    let c = case(
        "brand",
        &["set", "sales", "--focus-group", "zz"],
        &[],
        vec![ok(CONF_SALES), ok(GROUPS)],
        owner_and_port,
        4,
        2,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("{{\"error\": {{\"code\": \"not_found\", \"hint\": \"no focus group matches 'zz' (see: {cli} focus-group list)\"}}}}\n")
    );
    for text in [&out.stdout, &out.stderr] {
        if cli != "k2" {
            assert!(!text.contains("k2 "), "{text}");
        }
        assert!(!text.contains("k5 "), "{text}");
    }
}

fn leak(args: &[&'static str]) -> &'static [&'static str] {
    Box::leak(args.to_vec().into_boxed_slice())
}
