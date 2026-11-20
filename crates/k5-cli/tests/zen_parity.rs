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

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A staged widget folder, a Garden page file and a scratch workspace.
fn staged(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join("staged-w");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("manifest.json"), r#"{"id":"wid","entry":"index.html"}"#).unwrap();
    std::fs::write(d.join("index.html"), "<p>é</p>\n").unwrap();
    std::fs::write(d.join("logo.png"), [0u8, 1, 2, 250, 255]).unwrap();
    std::fs::write(d.join("README.md"), "skipped quietly").unwrap();
    std::fs::write(d.join("notes.txt"), "skipped with a note").unwrap();
    std::fs::write(r.proj.join("page.toml"), "schema = 1\n[layout]\n").unwrap();
}

const VALID_OK: &str = r#"{"ok":true,"status":"ok","files":["gardens/g1.toml"],"errors":[],"warnings":[{"file":"gardens/g1.toml","line":3,"col":1,"message":"old key","suggestion":"use new"}],"runtime":{"note":"ran clean","placements":[{"garden":"g1","gardenName":"Home","placement":"w1"}]},"size":{"code":2048,"codeMax":262144,"codeWarnAt":1024,"page":4096,"pageMax":1048576,"libs":3670016,"libsUnknown":true},"types":{"checker":"tsc","note":"2 files"}}"#;
const VALID_ERR: &str = r#"{"ok":false,"files":["gardens/g1.toml"],"errors":[{"file":"gardens/g1.toml","line":4,"col":2,"message":"unknown slot 'x' in band"},{"file":"gardens/g1.toml","line":9,"col":1,"message":"bad widget kind"},{"file":"zen.toml","line":1,"col":1,"message":"a menu needs items"}]}"#;
const PENDING: &str = r#"{"ok":true,"status":"pending","files":["widgets/w/index.html"],"widgets":[{"widget":"w","status":"pending","placements":[],"stale":2}],"staleRuntime":1}"#;
const HISTORY: &str = r#"{"files":[{"file":"zen.toml","snapshots":[{"name":"s1","at":"2026-11-20T10:00:00Z","clean":true}]},{"file":"gardens/g2.toml","deleted":true,"snapshots":[{"name":"s2","at":"t2","clean":false},{"name":"s3","at":"t3","clean":true}]},{"file":"themes/x/theme.toml","exists":false,"snapshots":[]}]}"#;

/// A history of `n` files.
fn many_files(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"file":"gardens/g{i}.toml","snapshots":[{{"name":"snap-{i}-é","at":"2026-11-{:02}T10:00:00Z","clean":{}}}]}}"#,
                i % 28 + 1,
                i % 3 != 0
            )
        })
        .collect();
    format!(r#"{{"files":[{}]}}"#, rows.join(","))
}

#[test]
fn zen_core_verbs_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "core");
    let proj = rig.proj.to_string_lossy().into_owned();
    let page = leak(format!("{proj}/page.toml"));
    let folder = leak(format!("{proj}/staged-w"));
    let from_args: &'static [&'static str] =
        Box::leak(vec!["validate", "--garden", "g1", "--from", page].into_boxed_slice());
    let dir_args: &'static [&'static str] =
        Box::leak(vec!["validate", "--dir", folder, "--as", "w2"].into_boxed_slice());
    let cases = vec![
        // covers: zen
        ok_owner("zen alone: the help", &[], vec![]),
        ok_owner("zen help", &["help"], vec![]),
        case("zen alone, no daemon: still the help", &[], &[], vec![], nothing, 0, 0),
        // covers: zen --json
        case("--json is no verb", &["--json"], &[], vec![], owner_and_port, 2, 0),
        case("an unknown verb", &["nope"], &[], vec![], owner_and_port, 2, 0),
        case("pages is gone", &["pages"], &[], vec![], owner_and_port, 2, 0),
        case("grant is no verb", &["grants"], &[], vec![], owner_and_port, 2, 0),
        case("no daemon", &["path"], &[], vec![], nothing, 1, 0),
        // covers: zen path
        ok_owner(
            "path",
            &["path"],
            vec![ok(r#"{"path":"/home/a/.k2/zen","setUp":true}"#)],
        ),
        case(
            "path, Zen not set up",
            &["path"],
            &[],
            vec![ok(r#"{"path":"/home/a/.k2/zen","setUp":false}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "path, Zen not set up, --json",
            &["path", "--json"],
            &[],
            vec![ok(r#"{"error":"zen_not_set_up","message":"Turn it on"}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "path takes only --json",
            &["path", "--garden", "g1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        traceback(case(
            "path, a list for an answer",
            &["path"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        )),
        // covers: zen doctor
        ok_owner(
            "doctor",
            &["doctor"],
            vec![ok(
                r#"{"ok":true,"checks":[{"ok":true,"name":"folder","detail":"there"},{"ok":true,"name":"theme","detail":"é"}]}"#,
            )],
        ),
        case(
            "doctor, a failed check",
            &["doctor"],
            &[],
            vec![ok(
                r#"{"ok":false,"checks":[{"ok":false,"name":"folder","detail":"missing"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        // covers: zen doctor --json
        ok_owner(
            "doctor --json",
            &["doctor", "--json"],
            vec![ok(r#"{"ok":false,"checks":[]}"#)],
        ),
        // covers: zen reload
        ok_owner(
            "reload",
            &["reload"],
            vec![ok(r#"{"ok":true,"changed":true,"errors":[],"warnings":[]}"#)],
        ),
        case(
            "reload with errors: the guide topics",
            &["reload"],
            &[],
            vec![ok(VALID_ERR)],
            owner_and_port,
            1,
            1,
        ),
        // covers: zen reload --json
        ok_owner(
            "reload --json",
            &["reload", "--json"],
            vec![ok(r#"{"ok":true,"changed":false}"#)],
        ),
        case(
            "reload takes only --json",
            &["reload", "--theme", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen reset
        ok_owner(
            "reset",
            &["reset", "--garden", "g1", "--to", "s1"],
            vec![ok(r#"{"file":"gardens/g1.toml","restored":"s1","snapshot":"s9"}"#)],
        ),
        case(
            "reset, the Garden is locked",
            &["reset", "--file", "zen.toml", "--json"],
            &[],
            vec![Reply::Answer(
                423,
                r#"{"error":"garden_locked","message":"Locked by the person."}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        // covers: zen history
        ok_owner("history", &["history"], vec![ok(HISTORY)]),
        // covers: zen history --json
        ok_owner("history --json", &["history", "--json"], vec![ok(HISTORY)]),
        case(
            "history --to is reset's",
            &["history", "--to", "s1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "history --strict is validate's",
            &["history", "--strict"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--home is gone",
            &["history", "--home"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "two targets",
            &["validate", "--garden", "a", "--theme", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen validate
        ok_owner(
            "validate a Garden",
            &["validate", "--garden", "Home é"],
            vec![ok(VALID_OK)],
        ),
        case(
            "validate, errors",
            &["validate"],
            &[],
            vec![ok(VALID_ERR)],
            owner_and_port,
            1,
            1,
        ),
        ok_owner("validate, pending", &["validate"], vec![ok(PENDING)]),
        case(
            "validate --strict, pending",
            &["validate", "--strict"],
            &[],
            vec![ok(PENDING)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "validate, the Garden is locked",
            &["validate", "--garden", "g1"],
            &[],
            vec![ok(r#"{"ok":true,"locked":{"message":"Locked."}}"#)],
            owner_and_port,
            3,
            1,
        ),
        // covers: zen validate --json
        case(
            "validate --json, errors",
            &["validate", "--json", "--widget", "w", "--types=all"],
            &[],
            vec![ok(VALID_ERR)],
            owner_and_port,
            1,
            1,
        ),
        ok_owner(
            "validate --widget --sign",
            &["validate", "--widget", "w", "--sign"],
            vec![
                ok(r#"{"ok":true,"files":["widgets/w/index.html"]}"#),
                ok(r#"{"widget":"w","stamped":true,"madeBy":{"kind":"agent","handle":"ag"},"vouched":true}"#),
            ],
        ),
        ok_owner(
            "validate --widget --sign --json",
            &["validate", "--widget", "w", "--sign", "--json"],
            vec![
                ok(r#"{"ok":true,"files":[]}"#),
                ok(r#"{"widget":"w","stamped":false,"madeBy":{"kind":"owner"}}"#),
            ],
        ),
        ok_owner(
            "validate --widget --sign, signing refused",
            &["validate", "--widget", "w", "--sign"],
            vec![
                ok(r#"{"ok":true,"files":[]}"#),
                ok(r#"{"error":{"code":"nope","hint":"no"}}"#),
            ],
        ),
        case(
            "--sign needs --widget",
            &["validate", "--sign"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--from needs --garden",
            &["validate", "--from", "-"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            setup: staged,
            ..ok_owner(
                "validate --garden --from a file",
                from_args,
                vec![ok(VALID_OK)],
            )
        },
        fed(
            b"schema = 1\n\xc3\xa9\n",
            ok_owner(
                "validate --garden --from stdin",
                &["validate", "--garden", "g1", "--from", "-"],
                vec![ok(VALID_OK)],
            ),
        ),
        case(
            "validate --from, no such file",
            &["validate", "--garden", "g1", "--from", "/nowhere/x.toml"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            setup: staged,
            ..ok_owner(
                "validate --dir a staged folder",
                dir_args,
                vec![ok(VALID_OK)],
            )
        },
        case(
            "validate --dir, no folder",
            &["validate", "--dir", "/nowhere/w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "the daemon refuses",
            &["doctor"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"zen_local_only"}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an answer that is not JSON",
            &["history"],
            &[],
            vec![Reply::Answer(500, "oops <html>".into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "nothing listening",
            &["reload"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            7,
            0,
        ),
        with(
            vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            case(
                "a session with no passport: reset sends no credential",
                &["reset", "--garden", "g1"],
                &[],
                vec![ok(r#"{"file":"gardens/g1.toml","restored":"s1"}"#)],
                owner_and_port,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a session: reset sends the passport",
                &["reset", "--garden", "g1"],
                &[],
                vec![ok(r#"{"file":"gardens/g1.toml","restored":"s1"}"#)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            compare: Compare::OwnerTokenWithPassport,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "a session, the owner on disk: doctor (PRD 13.9 #4)",
                    &["doctor"],
                    &[],
                    vec![ok(r#"{"ok":true,"checks":[]}"#)],
                    owner_and_port,
                    0,
                    1,
                ),
            )
        },
        shell(case(
            "reset -h: help, nothing sent",
            &["reset", "--garden", "g1", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "reload --help: help, nothing sent",
            &["reload", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "validate -h: help, nothing sent",
            &["validate", "--widget", "w", "--sign", "-h"],
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
    rig.check(&Case {
        env: vec![("K2_PROJECT_PATH", proj.clone())],
        setup: workspace_dot,
        ..ok_owner("the Garden access hint, zen alone", &[], vec![])
    });
}

/// `zen-large-answer`: an answer above what an environment variable may
/// hold. Bash stops at its Python (exit 126, "Argument list too long");
/// the binary prints it. Both send the same request.
#[test]
fn zen_history_with_a_large_answer() {
    let rig = Rig::new(VERB, "large");
    let big = leak(many_files(9000));
    assert!(big.len() > 1024 * 1024, "above macOS's whole ARG_MAX too");
    let c = Case {
        no_python3: NoPython3::Own,
        ..case(
            "history, an answer above 1 MiB",
            &["history"],
            &[],
            vec![ok(big)],
            owner_and_port,
            126,
            1,
        )
    };
    let mut logs = Vec::new();
    let mut outs = Vec::new();
    for (bash, side) in [(true, Side::Bash), (false, Side::Rust)] {
        rig.prepare(side, &c);
        let out = rig.run(bash, &c);
        let sent: Vec<String> = rig
            .read(side, !bash, c.name)
            .iter()
            .map(|r| {
                let head = String::from_utf8_lossy(&r.bytes).into_owned();
                fold_query_escapes(head.lines().next().unwrap_or(""))
            })
            .collect();
        logs.push(sent);
        outs.push(out);
    }
    let (b, r) = (&outs[0], &outs[1]);
    assert_eq!(logs[0].len(), 1, "bash's requests: {:?}", logs[0]);
    assert_eq!(logs[0], logs[1], "the requests");
    assert_eq!(b.code, Some(126), "bash: {b:?}");
    assert_eq!(b.stdout, "");
    assert!(
        b.stderr.contains("Argument list too long"),
        "bash: {}",
        b.stderr
    );
    assert_eq!(r.code, Some(0), "the binary: {r:?}");
    assert_eq!(r.stderr, "");
    assert_eq!(r.stdout.lines().count(), 9000 * 2);
    assert!(
        r.stdout
            .starts_with("gardens/g0.toml (1 snapshot)\n  snap-0-é  2026-11-01T10:00:00Z  (has errors)\n"),
        "the binary: {}",
        &r.stdout[..200]
    );
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn zen_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    for (args, want) in [
        (
            &["pages"][..],
            format!("Error: there is no '{cli} zen pages' any more: Zen pages are Gardens now. Use {cli} zen garden list.\n"),
        ),
        (
            &["history", "--to", "s1"],
            format!("Error: --to is for {cli} zen reset\n"),
        ),
        (
            &["validate", "--sign"],
            format!("Error: --sign is for {cli} zen validate --widget <name>\n"),
        ),
        (
            &["path", "--x"],
            format!("Error: unexpected '--x' (see {cli} zen --help)\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
