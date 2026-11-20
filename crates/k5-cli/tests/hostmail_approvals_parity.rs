//! `k5-cli hostmail approvals` against the shell CLI's `cli/k2 hostmail
//! approvals …`, on the parity rig ([`common::parity`]): the same requests
//! (urllib's, from `_mail_py`), stdout, stderr and exit code.
//!
//! - The words: each usage line (exit 2, nothing sent); a flag first or
//!   no word at all is `list`.
//! - list (none, one with every field, the count, an expiry), show (with
//!   files, without), save (the file written, a path that is there, a
//!   folder that isn't), approve (a hint, a status), deny; each `--json`.
//! - A refusal (the permission card), an answer the Python raises on, no
//!   answer.
//! - One answer above 256 KiB: thousands of pending sends.
//! - Help never acts.
//!
//! The forms here have no census entry (the census is the `--schema`
//! manifest's), so a `// covers:` tag would fail the coverage gate: each
//! row's tag is `// native: <form>`, for the native-bar gate to read.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "hostmail";

const PENDING: &str = r#"{"ok":true,"count":2,"pending":[
 {"id":"out_1","from":"bot@acme.example","fromName":"Bot\nto: x","workspace":"alpha","createdAt":1600000000,"expiresAt":1600000100,
  "to":["a@x.example","b@x.example"],"cc":["c@x.example"],"bcc":["d@x.example"],"policyBcc":["audit@acme.example"],
  "replyTo":["r@x.example"],"forwardOf":{"mailbox":"bot@acme.example","sender":"s@x.example","asEml":true},
  "subject":"Hi\u001b[2J there","sourceDmarc":"fail","sourceCheck":"SPF softfail",
  "files":[{"n":1,"name":"a.pdf","size":1536,"contentType":"application/pdf","scanned":true},{"name":"b\tc","size":"3000000"}],
  "bodyPreview":"line one\nline two"},
 {"id":"out_2","agentName":"Helper","to":[],"bodyPreview":null,"expiresAt":"bad"}]}"#;

const SHOW: &str = r#"{"ok":true,"message":{"id":"out_1","from":"bot@acme.example","workspace":"alpha",
 "to":["a@x.example"],"subject":null,"sourceDmarc":"pass",
 "files":[{"n":1,"name":"a.pdf","size":10,"contentType":"application/pdf","scanned":false}],
 "text":"Hello\u0007 é\n\tindented"}}"#;

const SAVE: &str =
    r#"{"ok":true,"id":"out_1","n":1,"name":"a\u001b.pdf","dataBase64":"aGVsbG8gd29ybGQK"}"#;

fn compare(compare: Compare, c: Case) -> Case {
    Case { compare, ..c }
}

fn ok_case(name: &'static str, args: &'static [&'static str], body: &str) -> Case {
    case(name, args, &[], vec![ok(body)], owner_and_port, 0, 1)
}

fn usage_case(name: &'static str, args: &'static [&'static str]) -> Case {
    case(name, args, &[], vec![], owner_and_port, 2, 0)
}

/// No `saved.pdf` in the workspace before each side.
fn no_file(r: &Rig) {
    owner_and_port(r);
    let _ = std::fs::remove_file(r.proj.join("saved.pdf"));
}

/// `saved.pdf` is there already.
fn a_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("saved.pdf"), "old").unwrap();
}

#[test]
fn hostmail_approvals_take_the_words_as_the_shell_does() {
    let rig = Rig::new(VERB, "awords");
    let v = vec![
        usage_case("an unknown command", &["approvals", "nope"]),
        usage_case("list takes no id", &["approvals", "list", "out_1"]),
        usage_case("list takes no note", &["approvals", "--note", "x"]),
        usage_case(
            "an unknown flag",
            &["approvals", "show", "out_1", "--bogus"],
        ),
        usage_case("a single dash", &["approvals", "show", "-"]),
        usage_case("two ids", &["approvals", "approve", "out_1", "out_2"]),
        usage_case("--note last", &["approvals", "approve", "out_1", "--note"]),
        usage_case("--file last", &["approvals", "save", "out_1", "--file"]),
        usage_case("--out last", &["approvals", "save", "out_1", "--out"]),
        usage_case("show with no id", &["approvals", "show"]),
        usage_case(
            "save with no --out",
            &["approvals", "save", "out_1", "--file", "1"],
        ),
        usage_case("approve with no id", &["approvals", "approve", "--json"]),
        usage_case("deny with no id", &["approvals", "deny", "--note", "x"]),
        usage_case("deny with no note", &["approvals", "deny", "out_1"]),
    ];
    for c in &v {
        rig.check(c);
    }
}

#[test]
fn hostmail_approvals_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "aanswer");
    let v = vec![
        // native: hostmail approvals list
        ok_case("list", &["approvals", "list"], PENDING),
        ok_case("bare: list", &["approvals"], PENDING),
        // native: hostmail approvals list --json
        ok_case("list --json", &["approvals", "list", "--json"], PENDING),
        ok_case("--json first: list", &["approvals", "--json"], PENDING),
        ok_case(
            "list, none",
            &["approvals", "list"],
            r#"{"ok":true,"pending":[]}"#,
        ),
        ok_case(
            "list, no count",
            &["approvals"],
            r#"{"ok":true,"pending":[{"id":"out_3"}]}"#,
        ),
        // native: hostmail approvals show
        ok_case("show", &["approvals", "show", "out_1"], SHOW),
        ok_case(
            "show, the bare minimum",
            &["approvals", "show", "out_1"],
            r#"{"ok":true}"#,
        ),
        // native: hostmail approvals show --json
        ok_case(
            "show --json",
            &["approvals", "show", "out_1", "--json"],
            SHOW,
        ),
        // native: hostmail approvals save
        case(
            "save",
            &[
                "approvals",
                "save",
                "out_1",
                "--file",
                "1",
                "--out",
                "saved.pdf",
            ],
            &[],
            vec![ok(SAVE)],
            no_file,
            0,
            1,
        ),
        // native: hostmail approvals save --json
        case(
            "save --json",
            &[
                "approvals",
                "save",
                "out_1",
                "--file",
                "1",
                "--out",
                "saved.pdf",
                "--json",
            ],
            &[],
            vec![ok(SAVE)],
            no_file,
            0,
            1,
        ),
        case(
            "save, the path is there",
            &[
                "approvals",
                "save",
                "out_1",
                "--file",
                "1",
                "--out",
                "saved.pdf",
            ],
            &[],
            vec![],
            a_file,
            2,
            0,
        ),
        case(
            "save, into a folder that isn't there",
            &[
                "approvals",
                "save",
                "out_1",
                "--file",
                "1",
                "--out",
                "nodir/saved.pdf",
            ],
            &[],
            vec![ok(SAVE)],
            no_file,
            2,
            1,
        ),
        // native: hostmail approvals approve
        ok_case(
            "approve, a hint",
            &["approvals", "approve", "out_1", "--note", "fine"],
            r#"{"ok":true,"id":"out_1","hint":"sent via relay"}"#,
        ),
        ok_case(
            "approve, a status",
            &["approvals", "approve", "out_1"],
            r#"{"ok":true,"id":"out_1","status":"approved"}"#,
        ),
        // native: hostmail approvals approve --json
        ok_case(
            "approve --json",
            &["approvals", "approve", "out_1", "--json"],
            r#"{"ok":true}"#,
        ),
        // native: hostmail approvals deny
        ok_case(
            "deny",
            &["approvals", "deny", "out_1", "--note", "not to that list"],
            r#"{"ok":true,"id":"out_1"}"#,
        ),
        // native: hostmail approvals deny --json
        ok_case(
            "deny --json",
            &["approvals", "deny", "--note", "no", "out_1", "--json"],
            r#"{"ok":true}"#,
        ),
        case(
            "an agent: the card",
            &["approvals", "approve", "out_1"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.to_string())],
            owner_and_port,
            3,
            1,
        ),
        compare(
            Compare::Traceback,
            case(
                "a file size that is no number",
                &["approvals", "show", "out_1"],
                &[],
                vec![ok(r#"{"ok":true,"message":{"files":[{"size":"big"}]}}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        compare(
            Compare::Traceback,
            case(
                "to that is a number",
                &["approvals"],
                &[],
                vec![ok(r#"{"ok":true,"pending":[{"to":5}]}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        case(
            "closed, not sent again",
            &["approvals", "deny", "out_1", "--note", "x"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The one large answer: 2,000 pending sends (above 256 KiB).
#[test]
fn hostmail_approvals_large_list() {
    let rig = Rig::new(VERB, "alarge");
    let rows: Vec<String> = (0..2000)
        .map(|i| {
            format!(
                r#"{{"id":"out_{i:05}","from":"bot{i}@acme.example","workspace":"ws-{i}","to":["a{i}@x.example"],"subject":"Subject number {i} é","bodyPreview":"preview {i}"}}"#
            )
        })
        .collect();
    let body = format!(
        r#"{{"ok":true,"count":2000,"pending":[{}]}}"#,
        rows.join(",")
    );
    assert!(body.len() > 256 * 1024);
    rig.check(&case(
        "2,000 pending sends",
        &["approvals"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

/// The verbs that change something, with `-h` or `--help` anywhere: the
/// help, and nothing sent.
#[test]
fn hostmail_approvals_help_never_acts() {
    let rig = Rig::new(VERB, "ahelp");
    const LINES: &[&[&str]] = &[
        &["approvals", "approve", "out_1", "-h"],
        &["approvals", "deny", "out_1", "--note", "x", "--help"],
        &[
            "approvals",
            "save",
            "--help",
            "out_1",
            "--file",
            "1",
            "--out",
            "f",
        ],
    ];
    for line in LINES {
        rig.check(&compare(
            Compare::Shell,
            case(
                "help, nothing sent",
                line,
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
        ));
    }
}
