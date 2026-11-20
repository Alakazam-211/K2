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
//! - The shell's: `-h` on a verb that changes something.
//! - Garden data (`--widget`, [`store_widget_answers_as_the_shell_does`]):
//!   every verb, `--json`, the usage errors, the daemon's error codes,
//!   no answer, a transaction, a file in and out, and one answer above
//!   256 KiB.

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

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn fed(stdin: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(stdin),
        ..c
    }
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

/// A file to send as a blob: a little over two chunks of 2 MiB.
fn blob_file(r: &Rig) {
    owner_and_port(r);
    let bytes: Vec<u8> = (0..(2 * 2_097_152 + 17)).map(|i| (i % 251) as u8).collect();
    std::fs::write(r.proj.join("photo.bin"), bytes).unwrap();
}

fn value_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("v.json"), "{\"a\": [1, 2.50],\r\n \"é\": null}\n").unwrap();
}

/// A list page with `n` keys.
fn many_keys(n: usize, next: &str) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"key":"k/{i}","rev":{i},"value":{{"n":{i},"t":"{}"}}}}"#,
                "ü".repeat(i % 50)
            )
        })
        .collect();
    format!(r#"{{"items":[{}],"next":{next}}}"#, rows.join(","))
}

/// Ops for a batch: `n` puts, as one JSON array.
fn ops(n: usize) -> &'static [u8] {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"{{"op":"put","key":"k{i}","value":{{"i":{i}}}}}"#))
        .collect();
    Box::leak(format!("[{}]", rows.join(",")).into_bytes().into_boxed_slice())
}

const B64: &str = "aGVsbG8gd29ybGQ=";

#[test]
fn store_widget_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "widget");
    let big = many_keys(4000, "null");
    assert!(
        big.len() > 256 * 1024,
        "the large answer is {} bytes",
        big.len()
    );
    let big: &'static str = Box::leak(big.into_boxed_str());
    let cases = vec![
        case(
            "--widget with no name",
            &["get", "k", "--widget"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an empty widget",
            &["get", "k", "--widget", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb",
            &["nope", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "get with no key",
            &["get", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["get", "--widget", "w", "k", "--nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a limit that is not a number",
            &["list", "--widget", "w", "--limit", "5x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: store --widget
        ok_owner(
            "get",
            &["get", "--widget", "w é", "k/1 é"],
            vec![ok(
                r#"{"entry":{"key":"k/1","rev":3,"by":"owner","value":{"a":[1,2.50,"é"],"b":{}}}}"#,
            )],
        ),
        // covers: store --widget --json
        ok_owner(
            "get --json",
            &["get", "k", "--json", "--widget", "w"],
            vec![ok(r#"{"entry":{"key":"k","rev":3,"by":"a","value":1}}"#)],
        ),
        case(
            "get, no such key",
            &["get", "--widget", "w", "k"],
            &[],
            vec![ok(r#"{"entry":null}"#)],
            owner_and_port,
            4,
            1,
        ),
        ok_owner(
            "put --value",
            &["put", "--widget", "w", "k", "--value", r#"{"x": 1.50, "y": "ü"}"#, "--if-rev", "07"],
            vec![ok(r#"{"rev":8}"#)],
        ),
        case(
            "put, no value",
            &["put", "--widget", "w", "k"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "put, a value that is not JSON",
            &["put", "--widget", "w", "k", "--value", "{nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "put, a file that isn't there",
                &["put", "--widget", "w", "k", "--file", "missing.json"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            )
        },
        ok_owner(
            "patch, nothing changed",
            &["patch", "--widget", "w", "k", "--value", r#"{"a": null}"#],
            vec![ok(r#"{"rev":4,"changed":false,"value":{"b":2}}"#)],
        ),
        case(
            "patch, not an object",
            &["patch", "--widget", "w", "k", "--value", "[1]"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "rm, no such key",
            &["rm", "--widget", "w", "k"],
            vec![ok(r#"{"rev":4,"deleted":false}"#)],
        ),
        ok_owner(
            "delete --if-rev",
            &["delete", "--widget", "w", "k", "--if-rev", "3", "--json"],
            vec![ok(r#"{"rev":5,"deleted":true}"#)],
        ),
        ok_owner(
            "list, one page",
            &["ls", "--widget", "w", "--prefix", "a/", "--limit", "2"],
            vec![ok(r#"{"items":[{"key":"a/1","rev":1,"value":"x"},{"key":"a/2","rev":2,"value":[1,2]}],"next":"a/2"}"#)],
        ),
        ok_owner(
            "list --all, two pages",
            &["list", "--widget", "w", "--all"],
            vec![
                ok(r#"{"items":[{"key":"a","rev":1,"value":{"é":1.50}}],"next":"a"}"#),
                ok(r#"{"items":[{"key":"b","rev":2,"value":null}],"next":null}"#),
            ],
        ),
        ok_owner(
            "list --all --json, two pages",
            &["list", "--widget", "w", "--all", "--json"],
            vec![
                ok(r#"{"items":[{"key":"a","rev":1,"value":{"é":1.50}}],"next":"a"}"#),
                ok(r#"{"items":[],"next":null}"#),
            ],
        ),
        ok_owner(
            "list, thousands of keys",
            &["list", "--widget", "w"],
            vec![ok(big)],
        ),
        ok_owner(
            "list --all, thousands of keys",
            &["list", "--widget", "w", "--all", "--json"],
            vec![ok(big)],
        ),
        ok_owner(
            "changes, two pages",
            &["changes", "--widget", "w", "--since", "3"],
            vec![
                ok(r#"{"items":[{"rev":4,"key":"a","op":"put"}],"next":{"since":4,"after":"a"}}"#),
                ok(r#"{"items":[{"rev":5,"key":"b","op":"delete"}],"next":null}"#),
            ],
        ),
        case(
            "changes, nothing new (exit 1 under set -e)",
            &["changes", "--widget", "w"],
            &[],
            vec![ok(r#"{"items":[],"next":null}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "changes, a resync (exit 9 under set -e)",
            &["changes", "--widget", "w", "--prefix", "p/"],
            &[],
            vec![answer(
                409,
                r#"{"error":{"code":"resync","hint":"too old","reapedRev":40}}"#,
            )],
            owner_and_port,
            9,
            1,
        ),
        ok_owner(
            "changes --json",
            &["changes", "--widget", "w", "--json", "--wait", "2"],
            vec![ok(r#"{"items":[],"next":null}"#)],
        ),
        fed(
            b"{\"op\":\"put\",\"key\":\"a\",\"value\":1}\n{\"op\":\"delete\",\"key\":\"b\"}\n",
            ok_owner(
                "batch, one call",
                &["batch", "--widget", "w"],
                vec![ok(r#"{"rev":7,"ops":2}"#)],
            ),
        ),
        fed(
            ops(130),
            ok_owner(
                "batch, a transaction",
                &["batch", "--widget", "w"],
                vec![
                    ok(r#"{"txId":"tx1"}"#),
                    ok(r#"{"ok":true}"#),
                    ok(r#"{"ok":true}"#),
                    ok(r#"{"ok":true}"#),
                    ok(r#"{"rev":12,"ops":130}"#),
                ],
            ),
        ),
        fed(
            ops(70),
            case(
                "batch, an add that conflicts",
                &["batch", "--widget", "w"],
                &[],
                vec![
                    ok(r#"{"txId":"tx2"}"#),
                    answer(409, r#"{"error":{"code":"conflict","hint":"op 3 (k3): rev moved"}}"#),
                    ok(r#"{"ok":true}"#),
                ],
                owner_and_port,
                5,
                3,
            ),
        ),
        fed(
            b"[1,",
            case(
                "batch, ops that are not JSON",
                &["batch", "--widget", "w"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
        ),
        case(
            "batch, nothing on stdin",
            &["batch", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "grants",
            &["grants", "--widget", "w"],
            vec![ok(
                r#"{"space":"data:w","grants":[{"prefix":"","level":"read","from":{"kind":"agent"}},{"prefix":"a/","level":"write","from":{"kind":"project","name":"acme"},"admin":true},{"prefix":"b/","level":"read","subjectKind":"agent","subjectId":"x"}],"made":[{"prefix":"c/","level":"read","subjectKind":"agent","subjectId":"y"}]}"#,
            )],
        ),
        ok_owner(
            "grants, none",
            &["grants", "--widget", "w"],
            vec![ok(r#"{"space":"data:w","grants":[]}"#)],
        ),
        ok_owner(
            "grant to a project",
            &["grant", "--widget", "w", "--project", "acme", "--level", "read", "--prefix", "a/"],
            vec![ok(
                r#"{"grant":{"level":"read","prefix":"a/","subjectKind":"project","subjectId":"acme"},"warning":"project_grant_convenience"}"#,
            )],
        ),
        case(
            "grant, both an agent and a project",
            &["grant", "--widget", "w", "--project", "p", "--agent", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "revoke",
            &["revoke", "--widget", "w", "--agent", "a"],
            vec![ok(r#"{"ok":true}"#)],
        ),
        ok_owner(
            "export, two pages",
            &["export", "--widget", "w", "--at", "2026-10-13"],
            vec![
                ok(r#"{"header":{"space":"data:w","rev":9},"items":[{"key":"a","rev":1,"value":{"é":1},"updatedAt":5,"by":"owner","extra":1}],"next":"a"}"#),
                ok(r#"{"header":{"space":"data:w"},"items":[{"key":"b","rev":2,"value":null,"updatedAt":6,"by":"x"}],"next":null}"#),
            ],
        ),
        case(
            "export, a bad date",
            &["export", "--widget", "w", "--at", "13-10-2026"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "blob ls",
            &["blob", "ls", "--widget", "w", "--limit", "2"],
            vec![ok(
                r#"{"items":[{"ref":"k2blob:sha256-aa","size":1234,"mime":"image/png","pending":false,"refs":1,"name":"a.png"},{"ref":"k2blob:sha256-bb","size":5,"mime":"text/plain","pending":true,"refs":0,"name":null}],"bytes":3145728,"quota":104857600,"next":"k2blob:sha256-bb"}"#,
            )],
        ),
        Case {
            setup: blob_file,
            ..ok_owner(
                "blob put, three chunks",
                &["blob", "put", "photo.bin", "--widget", "w", "--mime", "application/x-test"],
                vec![
                    ok(r#"{"upload":"u1"}"#),
                    ok(r#"{"upload":"u1"}"#),
                    ok(r#"{"ref":"k2blob:sha256-cc","size":4194321}"#),
                ],
            )
        },
        case(
            "blob put, no such file",
            &["blob", "put", "nope.bin", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "blob get, two chunks",
            &["blob", "get", "k2blob:sha256-cc", "--widget", "w", "--out", "got.bin", "--px", "256"],
            vec![
                ok(&format!(r#"{{"base64":"{B64}","offset":0,"len":11,"eof":false,"size":22,"mime":"text/plain"}}"#)),
                ok(&format!(r#"{{"base64":"{B64}","offset":11,"len":11,"eof":true,"size":22,"mime":"text/plain"}}"#)),
            ],
        ),
        case(
            "blob, an unknown verb",
            &["blob", "rm", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "history",
            &["history", "--widget", "w", "--key", "k"],
            vec![ok(
                r#"{"items":[{"rev":5,"key":"k","by":"owner","deleted":false,"prevKind":"none"},{"rev":6,"key":"k","by":"a","deleted":true,"prevKind":"value","restore":"r1"}],"head":6,"oldest":1,"next":{"before":5}}"#,
            )],
        ),
        ok_owner(
            "history --all",
            &["history", "--widget", "w", "--all", "--limit", "1"],
            vec![
                ok(r#"{"items":[{"rev":6,"key":"k","by":"a","deleted":false,"prevKind":"value"}],"head":6,"oldest":1,"next":{"before":6,"after":"k"}}"#),
                ok(r#"{"items":[],"head":6,"oldest":1,"next":null}"#),
            ],
        ),
        ok_owner(
            "restore --dry-run",
            &["restore", "--widget", "w", "--rev", "4", "--prefix", "a/", "--dry-run"],
            vec![ok(
                r#"{"id":null,"rev":null,"written":1,"deleted":1,"families":["a/"],"keys":["a/1","a/2"]}"#,
            )],
        ),
        ok_owner(
            "restore",
            &["restore", "--widget", "w", "--at", "2026-10-26T17:30", "--except", "b/"],
            vec![ok(
                r#"{"id":"r2","rev":11,"written":2,"deleted":0,"families":[],"keys":[]}"#,
            )],
        ),
        case(
            "restore with two points",
            &["restore", "--widget", "w", "--rev", "4", "--snapshot", "s"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "snapshot",
            &["snapshot", "--widget", "w", "--label", "before", "--prefix", "a/", "--json"],
            vec![ok(r#"{"ref":"snap-1","keys":3,"rev":9,"bytes":120}"#)],
        ),
        case(
            "owner_only: a person's verb",
            &["grant", "--widget", "w", "--agent", "a", "--level", "read"],
            &[],
            vec![answer(403, PERMISSION_ASK)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a refusal, --json",
            &["get", "--widget", "w", "k", "--json"],
            &[],
            vec![answer(403, r#"{"error":{"code":"not_granted","hint":"ask"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not found",
            &["grants", "--widget", "w"],
            &[],
            vec![answer(404, r#"{"error":{"code":"widget_not_found","hint":"no such widget"}}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "a conflict",
            &["put", "--widget", "w", "k", "--value", "1", "--if-rev", "2"],
            &[],
            vec![answer(409, r#"{"error":{"code":"conflict","hint":"rev is 3"}}"#)],
            owner_and_port,
            5,
            1,
        ),
        case(
            "an answer that is not JSON",
            &["get", "--widget", "w", "k"],
            &[],
            vec![answer(500, "oops")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listening",
            &["get", "--widget", "w", "k"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        Case {
            compare: Compare::Traceback,
            ..case(
                "an answer the reader can't read",
                &["put", "--widget", "w", "k", "--value", "1"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                1,
                1,
            )
        },
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a passport on the cell socket",
                &["get", "--widget", "w", "k"],
                &[],
                vec![ok(r#"{"entry":{"rev":1,"by":"a","value":true}}"#)],
                port_only,
                0,
                1,
            ),
        ),
        case(
            "the id tool in a session with no passport",
            &["get", "--widget", "w", "k"],
            &[("K2_HOOK_SOCK", "{deadsock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        shell(case(
            "put -h: help, nothing sent",
            &["put", "--widget", "w", "k", "--value", "1", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "batch --help: help, nothing sent",
            &["batch", "--help", "--widget", "w"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "restore -h: help, nothing sent",
            &["restore", "--widget", "w", "--rev", "1", "-h"],
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
    // `put --file` reads its file from the folder the command runs in.
    rig.check(&Case {
        setup: value_file,
        ..ok_owner(
            "put --file, the file there",
            &["put", "--widget", "w", "k", "--file", "v.json"],
            vec![ok(r#"{"rev":9}"#)],
        )
    });
}
