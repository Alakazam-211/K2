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
    std::fs::write(
        d.join("manifest.json"),
        r#"{"id":"wid","entry":"index.html"}"#,
    )
    .unwrap();
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
        case(
            "zen alone, no daemon: still the help",
            &[],
            &[],
            vec![],
            nothing,
            0,
            0,
        ),
        // covers: zen --json
        case(
            "--json is no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb",
            &["nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "pages is gone",
            &["pages"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "grant is no verb",
            &["grants"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
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
            vec![ok(
                r#"{"ok":true,"changed":true,"errors":[],"warnings":[]}"#,
            )],
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
            vec![ok(
                r#"{"file":"gardens/g1.toml","restored":"s1","snapshot":"s9"}"#,
            )],
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
                ok(
                    r#"{"widget":"w","stamped":true,"madeBy":{"kind":"agent","handle":"ag"},"vouched":true}"#,
                ),
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
            vec![Reply::Answer(403, r#"{"error":"zen_local_only"}"#.into())],
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
    let big = leak(many_files(12000));
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
    assert_eq!(r.stdout.lines().count(), 12000 * 2);
    assert!(
        r.stdout.starts_with(
            "gardens/g0.toml (1 snapshot)\n  snap-0-é  2026-11-01T10:00:00Z  (has errors)\n"
        ),
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

/// The daemon wrote its Garden access mark in the K2 home.
fn access_marked(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.k2home.join("garden-access-on"), "").unwrap();
}

fn access_unmarked(r: &Rig) {
    owner_and_port(r);
    let _ = std::fs::remove_file(r.k2home.join("garden-access-on"));
}

const GARDENS: &str = r#"{"gardens":[{"index":1,"id":"g1","name":"Home é","template":"texting","theme":"nord"},{"index":2,"id":"g2","name":"Work","template":"blank","locked":true}]}"#;
const SYNC: &str = r#"{"gardens":[{"index":1,"id":"g1","name":"Home é","page":{"mode":"synced"},"theme":{"mode":"own","defaults":"v3","k2Version":"0.46.2","newerDefault":true},"ownChanges":["theme"],"undo":{"parts":["page"]},"keepPrevious":{"page":true}},{"index":2,"id":"g2","name":"Work","page":{"mode":"synced"},"theme":{"mode":"synced"},"ownChanges":null}],"liveDefaults":"v4","k2Version":"0.46.17","previousDefaults":"v3"}"#;
const ACCESS: &str = r#"{"name":"Home","locked":true,"made":{"handle":"ag"},"editors":[{"handle":"ag","addedBy":"owner","addedAt":1700000000},{"handle":"bo","addedBy":"card","addedAt":"x"},{"handle":"cy","addedBy":"other"}]}"#;
const ACCESS_HISTORY: &str = r#"{"items":[{"at":1700000000,"kind":"renamed","actor":{"name":"ag","kind":"agent"},"detail":{"to":"New"}},{"at":null,"kind":"access","actor":{"kind":"owner"},"widget":"board","detail":{"workspace":"acme"}}],"unsigned":[{"widget":"board","hash":"h1"}],"next":"c2"}"#;

/// Many Gardens, for the large list.
fn many_gardens(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"index":{i},"id":"g{i}","name":"Garden {i} é","template":"texting","locked":{}}}"#,
                i % 5 == 0
            )
        })
        .collect();
    format!(r#"{{"gardens":[{}]}}"#, rows.join(","))
}

#[test]
fn zen_gardens_and_settings_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "gardens");
    let proj = rig.proj.to_string_lossy().into_owned();
    let page = leak(format!("{proj}/page.toml"));
    let folder = leak(format!("{proj}/staged-w"));
    let new_args: &'static [&'static str] = Box::leak(
        vec![
            "garden",
            "new",
            "My",
            "Garden",
            "--texting",
            "--at",
            "2",
            "--from",
            page,
            "--widgets",
            folder,
        ]
        .into_boxed_slice(),
    );
    let put_args: &'static [&'static str] =
        Box::leak(vec!["garden", "put", "g1", "--from", page, "--note", "tidy"].into_boxed_slice());
    let inspect_args: &'static [&'static str] =
        Box::leak(vec!["inspect", folder].into_boxed_slice());
    let mid: &'static str = leak(many_gardens(400));
    let cases = vec![
        // covers: zen garden
        ok_owner("garden alone: the help", &["garden"], vec![]),
        ok_owner("garden list", &["garden", "list"], vec![ok(GARDENS), ok(SYNC)]),
        ok_owner(
            "garden list, no sync route",
            &["gardens", "list"],
            vec![ok(GARDENS), Reply::Answer(404, r#"{"error":"unknown zen route"}"#.into())],
        ),
        ok_owner("garden list, a long list", &["garden", "list"], vec![ok(mid), ok("{}")]),
        // covers: zen garden --json
        ok_owner(
            "garden list --json",
            &["garden", "list", "--json"],
            vec![ok(GARDENS), ok(SYNC)],
        ),
        case(
            "garden list, Zen not set up",
            &["garden", "list"],
            &[],
            vec![ok(r#"{"setUp":false,"gardens":[]}"#), ok("{}")],
            owner_and_port,
            3,
            2,
        ),
        case(
            "garden list takes only --json",
            &["garden", "list", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            setup: access_unmarked,
            ..ok_owner(
                "garden new, an older daemon",
                &["garden", "new", "My", "Garden", "--texting", "--at", "02"],
                vec![ok(
                    r#"{"garden":{"id":"g3"},"path":"/z/gardens/g3.toml","made":true,"message":"Made it."}"#,
                )],
            )
        },
        Case {
            setup: staged_marked,
            ..ok_owner(
                "garden new with a page and widgets",
                new_args,
                vec![
                    ok(r#"{"gardens":[]}"#),
                    ok(r#"{"garden":{"id":"g4"},"path":"/z/gardens/g4.toml"}"#),
                ],
            )
        },
        case(
            "garden new --from on an older daemon",
            new_args,
            &[],
            vec![],
            staged_unmarked,
            2,
            0,
        ),
        case(
            "garden new --at must be a number",
            &["garden", "new", "x", "--at", "two"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "garden rename --texting is new's",
            &["garden", "rename", "g1", "x", "--texting"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "garden rename",
            &["garden", "rename", "g1", "New", "name"],
            vec![ok(r#"{"garden":{"id":"g1","name":"New name"},"changed":true}"#)],
        ),
        ok_owner(
            "garden reorder",
            &["garden", "reorder", "g2", "1"],
            vec![ok(GARDENS)],
        ),
        case(
            "garden reorder, not a number",
            &["garden", "reorder", "g2", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        ok_owner(
            "garden template --force",
            &["garden", "template", "g1", "blank", "--force"],
            vec![ok(
                r#"{"garden":{"id":"g1","name":"Home"},"changed":true,"template":"blank","snapshot":"s4"}"#,
            )],
        ),
        case(
            "garden template, has changes",
            &["garden", "template", "g1", "texting"],
            &[],
            vec![Reply::Answer(
                409,
                r#"{"error":"garden_has_changes","garden":"g1","keys":["a","b"]}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        ok_owner(
            "garden delete",
            &["garden", "delete", "Work"],
            vec![ok(r#"{"deleted":"g2","name":"Work","snapshot":"s5"}"#)],
        ),
        case(
            "garden delete, the person must",
            &["garden", "delete", "Work"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":"garden_delete_person","name":"Work","card":{"card":"c1"},"message":"Asked."}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        ok_owner(
            "garden lock",
            &["garden", "lock", "g1"],
            vec![ok(r#"{"garden":{"id":"g1","name":"Home"},"locked":true,"changed":true}"#)],
        ),
        case(
            "garden unlock, an agent",
            &["garden", "unlock", "g1"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"owner_only"}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        Case {
            setup: access_marked,
            ..ok_owner(
                "garden access",
                &["garden", "access", "Home"],
                vec![ok(r#"{"ok":true}"#), ok(ACCESS)],
            )
        },
        Case {
            setup: access_marked,
            ..ok_owner(
                "garden access, not listed",
                &["garden", "access", "Home", "--json"],
                vec![ok(r#"{"ok":true}"#), ok(r#"{"name":"Home","listed":false}"#)],
            )
        },
        Case {
            setup: access_marked,
            ..ok_owner(
                "garden access add",
                &["garden", "access", "Home", "add", "ag"],
                vec![
                    ok(r#"{"ok":true}"#),
                    ok(r#"{"agent":{"handle":"ag"},"garden":{"name":"Home"},"listed":true,"changed":false}"#),
                ],
            )
        },
        Case {
            setup: access_marked,
            ..ok_owner(
                "garden access ask --reason",
                &["garden", "access", "Home", "ask", "--reason", "to tidy"],
                vec![ok(r#"{"ok":true}"#), ok(r#"{"message":"Asked the person."}"#)],
            )
        },
        Case {
            setup: access_marked,
            ..case(
                "garden access, no such Garden",
                &["garden", "access", "Nope"],
                &[],
                vec![
                    ok(r#"{"ok":true}"#),
                    Reply::Answer(404, r#"{"error":"unknown_garden","message":"No Garden Nope."}"#.into()),
                ],
                access_marked,
                4,
                2,
            )
        },
        Case {
            setup: access_marked,
            ..ok_owner(
                "garden history",
                &["garden", "history", "Home", "--kind", "changes", "--limit", "5", "--before", "c1"],
                vec![ok(r#"{"ok":true}"#), ok(ACCESS_HISTORY)],
            )
        },
        Case {
            setup: access_marked,
            ..case(
                "garden history, an older daemon",
                &["garden", "history", "Home"],
                &[],
                vec![Reply::Answer(404, r#"{"error":"unknown zen route"}"#.into())],
                access_marked,
                2,
                1,
            )
        },
        case(
            "garden access, unmarked: an unknown verb",
            &["garden", "access", "Home"],
            &[],
            vec![],
            access_unmarked,
            2,
            0,
        ),
        Case {
            setup: access_marked,
            ..case(
                "garden nope: the long list",
                &["garden", "nope"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                access_marked,
                2,
                1,
            )
        },
        Case {
            setup: staged,
            ..ok_owner(
                "garden put",
                put_args,
                vec![ok(
                    r#"{"file":"gardens/g1.toml","changed":false,"snapshot":"s6","keepAsMine":true,"keepCard":{"card":"c1"},"validate":{"errors":[],"warnings":[{"file":"gardens/g1.toml","line":1,"col":1,"message":"w"}]}}"#,
                )],
            )
        },
        case(
            "garden put --note last",
            &["garden", "put", "g1", "--from", "-", "--note"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: zen theme
        ok_owner(
            "theme list",
            &["theme", "list", "--garden", "g1"],
            vec![ok(
                r#"{"themes":[{"name":"nord","label":"Nord","builtin":true,"user":true,"active":true,"summary":"cool"},{"name":"mine","user":true}],"garden":"g1","gardenTheme":true,"missing":"old","active":"nord"}"#,
            )],
        ),
        // covers: zen theme --json
        ok_owner(
            "theme next --json",
            &["theme", "next", "--json"],
            vec![ok(r#"{"theme":"nord"}"#)],
        ),
        ok_owner(
            "theme set for a Garden",
            &["themes", "set", "nord", "--garden=g1"],
            vec![ok(r#"{"theme":"nord","garden":"g1","scope":"garden"}"#)],
        ),
        ok_owner(
            "theme set --clear",
            &["theme", "set", "--clear", "--garden", "g1"],
            vec![ok(r#"{"theme":"nord","garden":"g1","scope":"computer"}"#)],
        ),
        ok_owner(
            "theme new",
            &["theme", "new", "mine", "--from", "nord"],
            vec![ok(
                r#"{"path":"/z/themes/mine/theme.toml","from":"nord","copiedImage":"bg.png","name":"mine"}"#,
            )],
        ),
        case("theme --json is no verb", &["theme", "--json"], &[], vec![], owner_and_port, 2, 0),
        case(
            "theme set, two names",
            &["theme", "set", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen sync
        ok_owner("sync", &["sync"], vec![ok(SYNC)]),
        // covers: zen sync --json
        ok_owner(
            "sync on --page --json",
            &["sync", "g1", "on", "--page", "--json"],
            vec![ok(r#"{"changed":true,"garden":{"id":"g1"}}"#)],
        ),
        ok_owner(
            "sync off",
            &["sync", "g1", "off"],
            vec![ok(
                r#"{"changed":true,"garden":{"id":"g1","name":"Home","page":{"mode":"own","defaults":"v4"},"theme":{"mode":"synced"},"undo":{"parts":["page"]}}}"#,
            )],
        ),
        ok_owner(
            "sync keep-previous --theme",
            &["sync", "g1", "keep-previous", "--theme"],
            vec![ok(r#"{"changed":false,"garden":{"id":"g1","name":"Home"}}"#)],
        ),
        ok_owner(
            "sync preview",
            &["sync", "g1", "preview"],
            vec![ok(
                r##"{"garden":{"id":"g1","name":"Home"},"page":{"template":"texting","layout":{"split":"2"},"widgets":[1,2]},"theme":{"name":"nord","tokens":{"colors":{"light":{"accent":"#111"},"dark":{"accent":"#eee"}}}},"sync":{"preview":"both","page":{"mode":"synced"},"theme":{"mode":"own"}}}"##,
            )],
        ),
        case(
            "sync undo --page",
            &["sync", "g1", "undo", "--page"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "sync, an agent",
            &["sync", "g1", "undo"],
            &[],
            vec![Reply::Answer(403, r#"{"error":"zen_local_only"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        // covers: zen news
        ok_owner(
            "news",
            &["news"],
            vec![ok(
                r#"{"items":[{"id":"n1","kind":"notice","title":"Hi","body":"There","detail":["a","b"]},{"id":"n2","kind":"catalog","label":"Board","description":"Notes"},{"id":"n3","kind":"update","gardenName":"Home","part":"theme","keepPrevious":true}],"copiesWithNewerDefault":2}"#,
            )],
        ),
        ok_owner("news, none", &["news"], vec![ok(r#"{"items":[],"copiesWithNewerDefault":1}"#)]),
        // covers: zen news --json
        ok_owner(
            "news seen --json",
            &["news", "seen", "n1", "n2", "--json"],
            vec![ok(r#"{"seen":["n1","n2"]}"#)],
        ),
        ok_owner("news seen --all", &["news", "seen", "--all"], vec![ok(r#"{"seen":[]}"#)]),
        case(
            "news seen, ids and --all",
            &["news", "seen", "n1", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen agents-order
        ok_owner(
            "agents-order",
            &["agents-order"],
            vec![ok(
                r#"{"gardens":[{"name":"Home","id":"g1","order":["a","b"],"pins":null,"updatedAt":"2026-11-20"},{"name":"Work","id":"g2","order":[],"pins":[]}]}"#,
            )],
        ),
        // covers: zen agents-order --json
        ok_owner(
            "agents-order --json",
            &["agents-order", "Home", "--json"],
            vec![ok(r#"{"gardens":[]}"#)],
        ),
        case(
            "agents-order, an older K2",
            &["agents-order"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"unknown zen route"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "agents-order, a list for an answer",
            &["agents-order"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        )),
        // covers: zen links
        ok_owner(
            "links",
            &["links"],
            vec![ok(r#"{"gardens":{"g2":["b.example","a.example"],"g1":["c.example"]}}"#)],
        ),
        ok_owner(
            "links for a Garden",
            &["links", "g1"],
            vec![ok(r#"{"sites":["a.example","b.example"]}"#)],
        ),
        ok_owner(
            "links forget",
            &["links", "forget", "g1", "a.example"],
            vec![ok(r#"{"removed":true,"site":"a.example","garden":"g1"}"#)],
        ),
        // covers: zen links --json
        case(
            "links forget --json, an agent",
            &["links", "forget", "g1", "a.example", "--json"],
            &[],
            vec![Reply::Answer(403, r#"{"error":{"code":"owner_only","hint":"The person forgets sites."}}"#.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "links, an empty answer",
            &["links"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        // covers: zen inspect
        Case {
            setup: staged,
            ..ok_owner(
                "inspect",
                inspect_args,
                vec![ok(
                    r#"{"notice":"Untrusted.","source":{"kind":"folder","path":"/x/é"},"summary":{"files":3,"widgets":1,"errors":1,"warnings":0,"verbs":2,"verbsNotOpenToWidgets":1,"network":1,"obfuscation":0,"refused":0},"widgets":[{"folder":"w","id":"w","name":"W","caps":["a","b"],"libs":[],"ok":false}],"verbs":[{"verb":"msg","known":true,"cap":"msg","calls":2,"at":["a.js:1"],"widget":true},{"verb":"x","known":false,"calls":1,"at":[]}],"k2Members":["store","open"],"network":[{"kind":"fetch","detail":"https://a","at":"a.js:3"}],"urls":[{"detail":"https://a","at":"a.js:3"}],"findings":{"errors":[{"file":"a.js","line":1,"col":2,"message":"bad"}],"warnings":[]},"truncated":{"urls":true,"verbs":false}}"#,
                )],
            )
        },
        // covers: zen inspect --json
        ok_owner(
            "inspect --json",
            &["inspect", "x.k2garden", "--json"],
            vec![ok(r#"{"notice":"n"}"#)],
        ),
        case(
            "inspect, the daemon refuses",
            &["inspect", "x.k2garden"],
            &[],
            vec![Reply::Answer(400, r#"{"error":"bad_path","message":"No such file."}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "inspect, no path",
            &["inspect"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "inspect, nothing listening",
            &["inspect", "x.k2garden"],
            &[("K2_PORT", &dead_port())],
            vec![],
            owner_only,
            1,
            0,
        ),
        Case {
            setup: access_marked,
            ..ok_owner(
                "proposals",
                &["proposals"],
                vec![
                    ok(
                        r#"{"proposals":[{"id":"p1","status":"pending","kind":"folder","target":"w","files":["a","b"],"madeBy":"ag","bytes":10,"note":"n"},{"id":"p2","status":"pending","target":"g1","madeBy":"ag","bytes":5}]}"#,
                    ),
                    ok(r#"{"ok":true}"#),
                ],
            )
        },
        case(
            "proposals approve, retired --json",
            &["proposals", "approve", "p1", "--json"],
            &[],
            vec![Reply::Answer(410, r#"{"error":"retired"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        shell(case(
            "garden delete -h: help, nothing sent",
            &["garden", "delete", "g1", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "sync on --help: help, nothing sent",
            &["sync", "g1", "on", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "links forget -h: help, nothing sent",
            &["links", "forget", "g1", "a", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "theme set -h: help, nothing sent",
            &["theme", "set", "nord", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "news seen --help: help, nothing sent",
            &["news", "seen", "--all", "--help"],
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

/// A staged widget and no Garden access mark (an earlier case may have
/// left one in the shared K2 home).
fn staged_unmarked(r: &Rig) {
    staged(r);
    let _ = std::fs::remove_file(r.k2home.join("garden-access-on"));
}

fn staged_marked(r: &Rig) {
    staged(r);
    std::fs::write(r.k2home.join("garden-access-on"), "").unwrap();
}

/// Many runtime errors, for the large list (under what bash's variable
/// holds).
fn many_errors(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"seq":{i},"at":"t{i}","current":{},"message":"boom é {i}","stack":"a\nb\nc","suggestion":"fix {i}"}}"#,
                i % 2 == 0
            )
        })
        .collect();
    format!(
        r#"{{"widget":"w","errors":[{}],"last":{n}}}"#,
        rows.join(",")
    )
}

#[test]
fn zen_widgets_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "widgets");
    let proj = rig.proj.to_string_lossy().into_owned();
    let folder = leak(format!("{proj}/staged-w"));
    let html = leak(format!("{proj}/staged-w/index.html"));
    let png = leak(format!("{proj}/staged-w/logo.png"));
    let put_dir: &'static [&'static str] = Box::leak(
        vec![
            "widget", "put", "w2", "--dir", folder, "--prune", "--note", "n", "--json",
        ]
        .into_boxed_slice(),
    );
    let apply: &'static [&'static str] =
        Box::leak(vec!["widget", "apply", "--from", folder].into_boxed_slice());
    let put_one: &'static [&'static str] = Box::leak(
        vec![
            "widget",
            "put",
            "w",
            "index.html",
            "--from",
            html,
            "--note",
            "x",
        ]
        .into_boxed_slice(),
    );
    let put_bin: &'static [&'static str] =
        Box::leak(vec!["widget", "put", "w", "logo.png", "--from", png].into_boxed_slice());
    let preview: &'static [&'static str] = Box::leak(
        vec![
            "widget",
            "preview",
            "--dir",
            folder,
            "w3",
            "--config",
            " size =big",
            "--config=a=b",
        ]
        .into_boxed_slice(),
    );
    let bad_cfg: &'static [&'static str] = Box::leak(
        vec!["widget", "preview", "--dir", folder, "--config", "nokey"].into_boxed_slice(),
    );
    let mid: &'static str = leak(many_errors(800));
    let cases = vec![
        // covers: zen widget
        ok_owner("widget alone: the help", &["widget"], vec![]),
        case("widget grant: no permissions", &["widget", "grant"], &[], vec![], nothing, 0, 0),
        case("widget revoke --from is new's", &["widget", "revoke", "--from", "x"], &[], vec![], nothing, 2, 0),
        ok_owner(
            "widget list",
            &["widget", "list"],
            vec![ok(
                r#"{"widgets":[{"name":"board","state":"ok","title":"Board","caps":["store"],"errors":[{"file":"a.js","line":1,"col":1,"message":"e1"},{"file":"a.js","line":2,"col":1,"message":"e2","suggestion":"s"},{"file":"a.js","line":3,"col":1,"message":"e3"},{"file":"a.js","line":4,"col":1,"message":"e4"}],"placements":[{"garden":"g1","placement":"w1","paused":true},{"garden":"g2","placement":"w2"}]},{"name":"empty","state":"new","placements":[]}]}"#,
            )],
        ),
        ok_owner("widget list, none", &["widgets", "list"], vec![ok(r#"{"widgets":[]}"#)]),
        // covers: zen widget --json
        ok_owner(
            "widget new --json",
            &["widget", "new", "board", "--from", "k2:diary", "--json"],
            vec![ok(r#"{"name":"board","path":"/z/widgets/board","files":["index.html"]}"#)],
        ),
        ok_owner(
            "widget new",
            &["widget", "new", "board"],
            vec![ok(r#"{"name":"board","path":"/z/widgets/board","files":["index.html","manifest.json"]}"#)],
        ),
        ok_owner(
            "widget sign --ask",
            &["widget", "sign", "board", "--ask"],
            vec![ok(r#"{"widget":"board","asked":true,"card":"c1","waiting":true,"changes":"new caps"}"#)],
        ),
        ok_owner(
            "widget sign, a built-in",
            &["widget", "sign", "diary"],
            vec![ok(r#"{"widget":"diary","madeBy":{"kind":"k2"}}"#)],
        ),
        case(
            "widget sign, the person must",
            &["widget", "sign", "board"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"sign_needs_person","hint":"Ask the person."},"changes":"caps","card":"c2"}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        ok_owner(
            "widget claim",
            &["widget", "claim", "board"],
            vec![ok(r#"{"widget":"board","madeBy":{"kind":"owner"},"stamped":true}"#)],
        ),
        ok_owner(
            "widget claim, retired: it signs",
            &["widget", "claim", "board", "--json"],
            vec![
                Reply::Answer(410, r#"{"error":{"code":"retired"}}"#.into()),
                ok(r#"{"widget":"board","madeBy":{"kind":"owner"},"stamped":true,"vouched":true}"#),
            ],
        ),
        case(
            "widget nope",
            &["widget", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "widget list --widget",
            &["widget", "list", "--widget", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        Case {
            setup: access_marked,
            ..ok_owner(
                "widget history",
                &["widget", "history", "board"],
                vec![ok(r#"{"ok":true}"#), ok(ACCESS_HISTORY)],
            )
        },
        ok_owner(
            "widget show",
            &["widget", "show", "board"],
            vec![ok(
                r#"{"widget":"board","name":"Board","state":"ok","requested":["store"],"caps":[],"reach":"posts","sources":["mail:inbox"],"reads":["notes"],"runtime":{"ran":true},"size":{"code":100,"codeMax":2048,"page":10,"pageMax":20},"placements":[{"gardenName":"Home","placement":"w1","locked":true,"paused":true,"note":"ok","sources":["a"]}]}"#,
            )],
        ),
        case(
            "widget show, no such widget",
            &["widget", "show", "nope"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"not_found","message":"No widget nope."}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        // covers: zen widget errors
        case(
            "widget errors",
            &["widget", "errors", "board", "--since", "3"],
            &[],
            vec![ok(
                r#"{"widget":"board","errors":[{"seq":4,"at":"t4","current":true,"kind":"refusal","class":"read","code":"denied","address":"mail:inbox","line":3,"redacted":true,"stack":"s1\ns2"},{"seq":5,"at":"t5","staleReason":"old","message":"boom","counted":false,"suggestion":"wait"}]}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        ok_owner(
            "widget errors, none",
            &["widget", "errors", "board"],
            vec![ok(r#"{"widget":"board","errors":[]}"#)],
        ),
        case(
            "widget errors, a long list",
            &["widget", "errors", "board"],
            &[],
            vec![ok(mid)],
            owner_and_port,
            1,
            1,
        ),
        // covers: zen widget errors --json
        ok_owner(
            "widget errors --clear --all --json",
            &["widget", "errors", "board", "--clear", "--all", "--json"],
            vec![ok(r#"{"widget":"board","cleared":3,"all":true,"left":0}"#)],
        ),
        ok_owner(
            "widget errors --clear",
            &["widget", "errors", "board", "--clear"],
            vec![ok(r#"{"widget":"board","cleared":1,"left":2}"#)],
        ),
        case(
            "widget errors, unknown widget",
            &["widget", "errors", "nope"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"not_found","message":"No widget."}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "widget errors --all without --clear",
            &["widget", "errors", "board", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "widget errors --since x",
            &["widget", "errors", "board", "--since", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen widget put
        Case {
            setup: staged,
            ..ok_owner(
                "widget put one file",
                put_one,
                vec![ok(
                    r#"{"file":"widgets/w/index.html","snapshot":"s7","validate":{"errors":[{"file":"index.html","line":1,"col":1,"message":"bad","suggestion":"fix"}],"size":{"code":5000000,"codeMax":4194304,"codeWarnAt":1048576,"page":1,"pageMax":2,"libs":0}}}"#,
                )],
            )
        },
        Case {
            setup: staged,
            ..case(
                "widget put, not UTF-8",
                put_bin,
                &[],
                vec![],
                staged,
                2,
                0,
            )
        },
        // covers: zen widget put --json
        Case {
            setup: staged,
            ..ok_owner(
                "widget put --dir --json",
                put_dir,
                vec![ok(r#"{"file":"widgets/w2","written":["index.html"],"removed":["old.js"],"kept":["x.css"]}"#)],
            )
        },
        Case {
            setup: staged,
            ..ok_owner(
                "widget apply --from",
                apply,
                vec![ok(
                    r#"{"proposed":true,"proposals":[{"id":"p1","kind":"folder","target":"wid","files":["a","b"]},{"id":"p2","target":"wid","file":"c.js"}],"message":"Asked."}"#,
                )],
            )
        },
        case(
            "widget apply, no --from",
            &["widget", "apply", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "widget put, --prune without --dir",
            &["widget", "put", "w", "a.js", "--from", "-", "--prune"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen widget delete
        ok_owner(
            "widget delete",
            &["widget", "delete", "board"],
            vec![ok(r#"{"file":"widgets/board","snapshot":"s8","undo":"k2 zen reset --to s8"}"#)],
        ),
        // covers: zen widget delete --json
        case(
            "widget delete --json, placed",
            &["widget", "delete", "board", "--json"],
            &[],
            vec![Reply::Answer(409, r#"{"error":"widget_placed","message":"Take it out first."}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "widget delete, two names",
            &["widget", "delete", "a", "b"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: zen widget preview
        Case {
            setup: staged,
            ..ok_owner(
                "widget preview --dir",
                preview,
                vec![ok(
                    r#"{"widget":"w3","preview":"preview-w3","garden":{"name":"Preview","id":"gp"},"validate":{"warnings":[{"file":"a","line":1,"col":1,"message":"w"}]},"open":"Open Zen."}"#,
                )],
            )
        },
        Case {
            setup: staged,
            ..case(
                "widget preview, a bad --config",
                bad_cfg,
                &[],
                vec![],
                staged,
                2,
                0,
            )
        },
        // covers: zen widget preview --json
        ok_owner(
            "widget preview --clear --json",
            &["widget", "preview", "--clear", "--json"],
            vec![ok(r#"{"removed":["preview-w3"],"garden":true}"#)],
        ),
        ok_owner(
            "widget preview --clear",
            &["widget", "preview", "--clear"],
            vec![ok(r#"{"removed":["preview-w3","preview-w4"],"garden":true}"#)],
        ),
        case(
            "widget preview, no --dir",
            &["widget", "preview", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        shell(case(
            "widget errors --follow stays the shell's",
            &["widget", "errors", "board", "--follow", "--since", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )),
        shell(case(
            "widget delete -h: help, nothing sent",
            &["widget", "delete", "board", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "widget put --help: help, nothing sent",
            &["widget", "put", "w", "a.js", "--from", "-", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "widget preview -h: help, nothing sent",
            &["widget", "preview", "--clear", "-h"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "widget sign -h: help, nothing sent",
            &["widget", "sign", "board", "-h"],
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

#[test]
fn zen_guide_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "guide");
    let page = |name: &'static str, args: &'static [&'static str]| {
        case(name, args, &[], vec![], nothing, 0, 0)
    };
    let usage = |name: &'static str, args: &'static [&'static str]| {
        case(name, args, &[], vec![], nothing, 2, 0)
    };
    let cases = vec![
        // covers: zen guide
        page("guide: the index", &["guide"]),
        page("guide gardens", &["guide", "gardens"]),
        page("guide safe-mode", &["guide", "safe-mode"]),
        page("guide api (generated)", &["guide", "api"]),
        page("guide libs (generated)", &["guide", "libs"]),
        page("guide services", &["guide", "services"]),
        page("guide example", &["guide", "example"]),
        page("guide example --list", &["guide", "examples", "--list"]),
        page("guide example rail-top", &["guide", "example", "rail-top"]),
        page(
            "guide example --toml",
            &["guide", "example", "column-corner", "--toml"],
        ),
        page(
            "guide example board-lite",
            &["guide", "example", "board-lite"],
        ),
        page(
            "guide example board-lite --toml",
            &["guide", "example", "board-lite", "--toml"],
        ),
        page(
            "guide example board-lite --script",
            &["guide", "example", "board-lite", "--script"],
        ),
        // covers: zen guide --json
        page("guide --json: the index", &["guide", "--json"]),
        page("guide themes --json", &["guide", "themes", "--json"]),
        page(
            "guide example --list --json",
            &["guide", "example", "--list", "--json"],
        ),
        page(
            "guide example --json",
            &["guide", "example", "menu-both", "--json"],
        ),
        page("guide examples --json", &["guide", "examples", "--json"]),
        page(
            "guide board-lite --json",
            &["guide", "example", "board-lite", "--json"],
        ),
        usage("guide, no topic", &["guide", "nope"]),
        usage("guide, no example", &["guide", "example", "nope"]),
        usage(
            "guide --toml, no example name",
            &["guide", "example", "--toml"],
        ),
        usage(
            "guide --list with a name",
            &["guide", "example", "x", "--list"],
        ),
        usage(
            "guide --toml and --script",
            &["guide", "example", "board-lite", "--toml", "--script"],
        ),
        usage(
            "guide --script not board-lite",
            &["guide", "example", "rail-top", "--script"],
        ),
        usage("guide --toml on a topic", &["guide", "gardens", "--toml"]),
        usage("guide, an extra word", &["guide", "gardens", "x"]),
        usage(
            "guide, a third word",
            &["guide", "example", "rail-top", "x"],
        ),
        usage("guide, an unknown flag", &["guide", "--nope"]),
        ok_owner("guide with a daemon", &["guide", "undo"], vec![]),
    ];
    for c in &cases {
        rig.check(c);
    }
}
