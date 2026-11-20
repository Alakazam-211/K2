//! `k5-cli store` against the shell CLI's `cli/k2 store`, on the parity
//! rig ([`common::parity`]): the same requests (urllib's, from `_db_py`),
//! stdout, stderr and exit code.
//!
//! - Every verb, plain and `--json`: create, list, put (`--json <doc>`
//!   and `--id`), get, query (`--limit`, `--where` read and dropped), rm
//!   and drop (their `--json` is the caller's `D_JSON`).
//! - Usage errors, help, a value flag at the end (`set -e`), the words
//!   bash's `shift 2 || true` leaves for the loop.
//! - `--workspace` resolved; the daemon's errors; a document that is not
//!   JSON; no answer; one answer above 256 KiB.
//! - The shell's: `--widget` (Garden data), and `-h` on a verb that
//!   changes something.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "store";

fn ok_owner(name: &'static str, args: &'static [&'static str], replies: Vec<Reply>) -> Case {
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, 0, n)
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn many_docs(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":"doc-{i}","json":{{"n":{i},"t":"{}"}}}}"#,
                "ü".repeat(i % 40)
            )
        })
        .collect();
    format!(r#"{{"ok":true,"docs":[{}]}}"#, rows.join(","))
}

#[test]
fn store_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let big = many_docs(5000);
    assert!(
        big.len() > 256 * 1024,
        "the large answer is {} bytes",
        big.len()
    );
    let cases = vec![
        case("no verb", &[], &[], vec![], owner_and_port, 0, 0),
        case("unknown verb", &["nope"], &[], vec![], owner_and_port, 2, 0),
        case(
            "list, a stray word",
            &["list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "put --id at the end",
            &["put", "c", "--id"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "query --where at the end",
            &["query", "c", "--where"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "get with one word",
            &["get", "c"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "put, a document that is not JSON",
            &["put", "c", "--json", "{nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "create",
            &["create", "notes"],
            vec![Reply::Answer(200, r#"{"ok":true,"name":"notes"}"#.into())],
        ),
        ok_owner(
            "create --json",
            &["create", "notes", "--json"],
            vec![ok(r#"{"ok":true,"name":"notes"}"#)],
        ),
        // covers: store list
        ok_owner(
            "list",
            &["list"],
            vec![ok(r#"{"ok":true,"collections":["a","é"]}"#)],
        ),
        // covers: store list --json
        ok_owner(
            "list --json",
            &["list", "--json"],
            vec![ok(r#"{"ok":true,"collections":[]}"#)],
        ),
        // covers: store put
        ok_owner(
            "put",
            &[
                "put",
                "notes",
                "--id",
                "d 1",
                "--json",
                r#"{"a":[1,2.5,"é"],"b":null}"#,
            ],
            vec![ok(r#"{"ok":true,"id":"d 1"}"#)],
        ),
        // covers: store put --json
        ok_owner(
            "put, no document",
            &["put", "notes", "--json"],
            vec![ok(r#"{"ok":true,"id":"x"}"#)],
        ),
        ok_owner(
            "get",
            &["get", "notes", "d1"],
            vec![ok(r#"{"ok":true,"doc":{"a":1}}"#)],
        ),
        ok_owner(
            "get --json first",
            &["get", "--json"],
            vec![ok(r#"{"ok":true}"#)],
        ),
        ok_owner(
            "query",
            &["query", "notes", "--where", "a=1", "--limit", "5"],
            vec![ok(r#"{"ok":true,"docs":[]}"#)],
        ),
        ok_owner(
            "rm",
            &["rm", "notes", "d1", "ignored"],
            vec![ok(r#"{"ok":true,"removed":1}"#)],
        ),
        case(
            "rm, D_JSON",
            &["rm", "notes", "d1"],
            &[("D_JSON", "true")],
            vec![ok(r#"{"ok":true,"removed":1}"#)],
            owner_and_port,
            0,
            1,
        ),
        ok_owner("drop", &["drop", "notes"], vec![ok(r#"{"ok":true}"#)]),
        ok_owner("query, thousands", &["query", "notes"], vec![ok(&big)]),
        ok_owner(
            "query --json, thousands",
            &["query", "notes", "--json"],
            vec![ok(&big)],
        ),
        ok_owner(
            "--workspace, resolved",
            &["list", "--workspace", "acme"],
            vec![ok(r#"{"path":"/w/acme"}"#), ok(r#"{"ok":true}"#)],
        ),
        case(
            "a refusal",
            &["drop", "notes"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not found",
            &["get", "notes", "x"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"ok":false,"error":{"code":"not_found","hint":"no doc"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listening",
            &["list"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the id tool in a session with no passport",
            &["list"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        shell(case(
            "--widget goes to the shell",
            &["get", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "put -h",
            &["put", "notes", "--json", "{}", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "rm --help",
            &["rm", "notes", "d1", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "drop -h",
            &["drop", "notes", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "create --help",
            &["create", "notes", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
    ];
    for c in &cases {
        rig.check(c);
    }
}
