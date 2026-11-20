//! `k5-cli zen` against the shell CLI's `cli/k2 zen`, on the parity rig
//! ([`common::parity`]): the same requests (`_compute_send`'s), stdout,
//! stderr and exit code.
//!
//! Native so far: `zen data` (the person's Garden data admin). Every
//! verb, `--json`, the prompts of `unadmin` and `delete` (answered on
//! stdin, and `--yes`), export with its blobs, import, the usage errors,
//! the daemon's error codes, the Garden access hint, one answer above
//! 256 KiB, and the lines the binary leaves to the shell.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "zen";

fn ok_owner(name: &'static str, args: &'static [&'static str], replies: Vec<Reply>) -> Case {
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, 0, n)
}

fn fed(stdin: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(stdin),
        ..c
    }
}

fn shell(c: Case) -> Case {
    Case {
        compare: Compare::Shell,
        ..c
    }
}

fn answer(status: u16, body: &str) -> Reply {
    Reply::Answer(status, body.to_string())
}

fn many_spaces(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"widget":"w-{i}-{}","bytes":{},"keys":{i},"grants":1,"admins":["a{i}"],"madeBy":{{"kind":"agent","handle":"h{i}"}}}}"#,
                "é".repeat(i % 30),
                i * 997
            )
        })
        .collect();
    format!(r#"{{"spaces":[{}],"saves":[]}}"#, rows.join(","))
}

/// A blobs folder for import: one file named by its sha256, one not.
fn blobs_folder(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join("blobs");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("a".repeat(64)), "hello").unwrap();
    std::fs::write(d.join("notes.txt"), "skipped").unwrap();
}

/// A workspace with its `.k2` and no hint marker yet.
fn workspace_dot(r: &Rig) {
    owner_and_port(r);
    let dot = r.proj.join(".k2");
    std::fs::create_dir_all(&dot).unwrap();
    let _ = std::fs::remove_file(dot.join(".garden-changes-hint"));
}

fn clean_export(r: &Rig) {
    owner_and_port(r);
    let _ = std::fs::remove_dir_all(r.proj.join("out-blobs"));
}

const REF: &str = "k2blob:sha256-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn zen_data_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "data");
    let big: &'static str = Box::leak(many_spaces(3000).into_boxed_str());
    assert!(
        big.len() > 256 * 1024,
        "the large answer is {} bytes",
        big.len()
    );
    let export_page: &'static str = Box::leak(
        format!(
            r#"{{"header":{{"space":"data:w"}},"items":[{{"key":"a","rev":1,"value":{{"pic":"{REF}"}},"updatedAt":5,"by":"owner"}}],"next":null}}"#
        )
        .into_boxed_str(),
    );
    let cases = vec![
        // covers: zen data
        ok_owner("no verb: the help page", &["data"], vec![]),
        ok_owner("help", &["data", "help"], vec![]),
        case(
            "an unknown verb",
            &["data", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "list",
            &["data", "list"],
            vec![ok(
                r#"{"spaces":[{"widget":"board","bytes":2048,"keys":3,"grants":1,"admins":["ag"],"madeBy":{"kind":"owner"}},{"widget":"old","bytes":10,"keys":0,"grants":0,"admins":[],"madeBy":{"kind":"k2"},"orphanedAt":5},{"widget":"x","bytes":3145728,"keys":9,"grants":0,"admins":[],"madeBy":null}],"saves":[{"widget":"notes","bytes":100,"keys":2}]}"#,
            )],
        ),
        ok_owner(
            "list, nothing yet",
            &["data", "ls"],
            vec![ok(r#"{"spaces":[],"saves":[]}"#)],
        ),
        // covers: zen data --json
        ok_owner(
            "list --json",
            &["data", "list", "--json"],
            vec![ok(r#"{"spaces":[],"saves":[]}"#)],
        ),
        ok_owner("list, thousands", &["data", "list"], vec![ok(big)]),
        ok_owner(
            "show",
            &["data", "show", "board"],
            vec![ok(
                r#"{"space":{"widget":"board","keys":3,"bytes":2048,"rev":7},"madeBy":{"kind":"agent","handle":"h1"},"grants":[{"subjectKind":"agent","subjectName":"ag","level":"write","prefix":"","admin":true,"grantedBy":"owner"},{"subjectKind":"project","subjectName":"acme","level":"read","prefix":"a/","admin":false,"grantedBy":"ag"}]}"#,
            )],
        ),
        case(
            "show, no widget",
            &["data", "show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "grant",
            &[
                "data", "grant", "board", "--agent", "ag", "--level", "write",
            ],
            vec![ok(
                r#"{"grant":{"level":"write","prefix":"","subjectKind":"agent","subjectId":"ag"}}"#,
            )],
        ),
        ok_owner(
            "revoke",
            &[
                "data",
                "revoke",
                "board",
                "--project",
                "acme",
                "--prefix",
                "a/",
            ],
            vec![ok(r#"{"ok":true}"#)],
        ),
        ok_owner(
            "admin",
            &[
                "data", "admin", "board", "--agent", "ag", "--level", "read", "--prefix", "p/",
            ],
            vec![ok(
                r#"{"grant":{"level":"read","prefix":"p/","subjectKind":"agent","subjectId":"ag"}}"#,
            )],
        ),
        case(
            "admin to a project",
            &[
                "data",
                "admin",
                "board",
                "--project",
                "acme",
                "--level",
                "read",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "unadmin --yes",
            &["data", "unadmin", "board", "--agent", "ag", "--yes"],
            vec![ok(r#"{"cascaded":2}"#), ok(r#"{"cascaded":2}"#)],
        ),
        fed(
            b"  yes \n",
            ok_owner(
                "unadmin, yes on stdin",
                &["data", "unadmin", "board", "--agent", "ag"],
                vec![ok(r#"{"cascaded":1}"#), ok(r#"{"cascaded":1}"#)],
            ),
        ),
        fed(
            b"n\n",
            case(
                "unadmin, no on stdin",
                &["data", "unadmin", "board", "--agent", "ag"],
                &[],
                vec![ok(r#"{"cascaded":1}"#)],
                owner_and_port,
                1,
                1,
            ),
        ),
        fed(
            b"board\n",
            ok_owner(
                "delete, the name on stdin",
                &["data", "delete", "board"],
                vec![ok(
                    r#"{"deleted":"board","grantsRemoved":2,"snapshot":"snap-9"}"#,
                )],
            ),
        ),
        case(
            "delete, nothing on stdin",
            &["data", "rm", "board"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        ok_owner(
            "move",
            &["data", "mv", "old", "new"],
            vec![ok(r#"{"grantsMoved":3}"#)],
        ),
        case(
            "move, one name",
            &["data", "move", "old"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "export",
            &["data", "export", "board", "--at", "2026-10-13"],
            vec![ok(export_page)],
        ),
        Case {
            setup: clean_export,
            ..ok_owner(
                "export --blobs",
                &["data", "export", "board", "--blobs", "out-blobs"],
                vec![
                    ok(export_page),
                    ok(
                        r#"{"base64":"aGVsbG8=","offset":0,"len":5,"eof":true,"size":5,"mime":"text/plain"}"#,
                    ),
                ],
            )
        },
        case(
            "export, no widget",
            &["data", "export", "--at", "2026-10-13"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        fed(
            b"{\"space\":\"data:board\"}\n{\"key\":\"a\",\"value\":{\"n\":1}}\n",
            ok_owner(
                "import",
                &["data", "import", "board", "--replace"],
                vec![ok(r#"{"written":1,"rev":12,"deleted":4}"#)],
            ),
        ),
        Case {
            setup: blobs_folder,
            ..fed(
                b"[{\"key\":\"a\",\"value\":1}]",
                ok_owner(
                    "import --blobs",
                    &["data", "import", "board", "--blobs", "blobs", "--json"],
                    vec![
                        ok(&format!(r#"{{"ref":"{REF}","size":5}}"#)),
                        ok(r#"{"written":1,"rev":13,"deleted":0}"#),
                    ],
                ),
            )
        },
        fed(
            b"{\"key\":\"a\"}\n",
            case(
                "import, a row with no value",
                &["data", "import", "board"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
        ),
        ok_owner(
            "restore",
            &[
                "data",
                "restore",
                "board",
                "--snapshot",
                "snap-9",
                "--prefix",
                "a/",
            ],
            vec![ok(
                r#"{"id":"r3","rev":20,"written":3,"deleted":1,"families":["a/"],"keys":[]}"#,
            )],
        ),
        case(
            "restore, no widget",
            &["data", "restore", "--rev", "3"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "person_only in a session",
            &["data", "list"],
            &[],
            vec![answer(
                403,
                r#"{"error":{"code":"person_only","hint":"a person only"}}"#,
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "space not found, --json",
            &["data", "show", "gone", "--json"],
            &[],
            vec![answer(
                404,
                r#"{"error":{"code":"space_not_found","hint":"no data"}}"#,
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "nothing listening",
            &["data", "list"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        shell(case(
            "restore -h: help, nothing sent",
            &["data", "restore", "board", "--rev", "1", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "delete -h: help, nothing sent",
            &["data", "delete", "board", "--yes", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "grant --help: help, nothing sent",
            &[
                "data", "grant", "board", "--agent", "a", "--level", "read", "--help",
            ],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "zen pages stays the shell's",
            &["pages"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
    ];
    for c in &cases {
        rig.check(c);
    }
    // The Garden access hint: once per workspace, the marker in its .k2.
    let proj = rig.proj.to_string_lossy().into_owned();
    rig.check(&Case {
        env: vec![("K2_PROJECT_PATH", proj)],
        setup: workspace_dot,
        ..ok_owner(
            "the Garden access hint, once",
            &["data", "list"],
            vec![ok(r#"{"spaces":[],"saves":[]}"#)],
        )
    });
}
