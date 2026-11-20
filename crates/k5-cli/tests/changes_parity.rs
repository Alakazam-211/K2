//! `k5-cli changes` against the shell CLI's `cli/k2 changes`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The list (a repository, none, `--json`, `--base`, more than 256 KiB
//!   of files), `diff` (the snapshot, then each file; `--json`; `-U`; a
//!   path; a file the daemon refuses; an untracked folder), `stage`,
//!   `unstage`, `stage --all`, `discard` (`--yes`, no terminal, failures),
//!   `commit` (staged, nothing staged, `--all`, no repository), the review
//!   notes (`list`, `add`, `resolve`, `reopen`, `delete`, `edit`, `send`),
//!   `open` and `init`.
//! - The CLI's own refusals, with and without `--json`; the daemon's
//!   refusals (a closing line), no answer (curl 7, an empty reply), a
//!   POST on the socket with no answer (sent once).
//! - Help anywhere on every verb (no request), the gate, a session with
//!   no passport, a passport on the cell socket, `--workspace` by name.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "changes";
const SNAP: &str = r#"{"isRepo":true,"branch":"main","upstream":"origin/main","ahead":1,"behind":2,"repoRoot":"/nonexistent/repo","workspacePath":"/nonexistent/repo","rev":"r9","files":[{"path":"src/a.rs","group":"staged","status":"M","additions":3,"deletions":1},{"path":"src/a.rs","group":"unstaged","status":"M","additions":1,"deletions":0},{"path":"b é.txt","oldPath":"old.txt","group":"unstaged","status":"R"},{"path":"new/","group":"untracked","status":"?","dir":true}],"counts":{"staged":1,"unstaged":2,"untracked":1,"total":4}}"#;
const FILE: &str = r#"{"path":"src/a.rs","old":{"exists":true,"text":"fn a() {}\nfn b() {}\nfn c() {}\n"},"new":{"exists":true,"text":"fn a() {}\nfn B() {}\nfn c() {}\nfn d() {}"},"additions":2,"deletions":1,"rev":"r9","mode":"worktree"}"#;
const RENAMED: &str = r#"{"path":"b é.txt","oldPath":"old.txt","old":{"exists":true,"text":"x\n"},"new":{"exists":true,"text":"x\n"}}"#;
const NOTES: &str = r#"{"workspacePath":"/w","counts":{"open":1,"unsent":1,"resolved":0},"notes":[{"id":"n1","path":"a.rs","startLine":3,"endLine":5,"authorName":"Pat","body":"one\ntwo"}]}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn big_snapshot() -> String {
    let files: Vec<String> = (0..4000)
        .map(|i| {
            format!(
                r#"{{"path":"src/module_{i}/file_{i}.rs","group":"unstaged","status":"M","additions":{i},"deletions":1}}"#
            )
        })
        .collect();
    let body = format!(
        r#"{{"isRepo":true,"branch":"main","files":[{}],"counts":{{"unstaged":4000,"total":4000}}}}"#,
        files.join(",")
    );
    assert!(body.len() > 256 * 1024, "the large case is above 256 KiB");
    body
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

#[test]
fn changes_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let big = leak(big_snapshot());
    let v = vec![
        // covers: changes
        case("the list", &[], &[], vec![ok(SNAP)], owner_and_port, 0, 1),
        case(
            "status --base",
            &["status", "--base", "dev"],
            &[],
            vec![ok(
                r#"{"isRepo":true,"detached":true,"headOid":"0123456789","base":{"ref":"dev","mergeBase":"","ahead":3,"behind":1},"files":[]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: changes --json
        case(
            "list --json",
            &["--json"],
            &[],
            vec![ok(SNAP)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the list, more than 256 KiB",
            &["ls"],
            &[],
            vec![ok(big)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "not a repository",
            &["list"],
            &[],
            vec![ok(r#"{"isRepo":false,"workspacePath":"/w"}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a refusal's closing line",
            &[],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an unknown argument",
            &["--zz"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown argument, --json",
            &["--json", "--zz"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb",
            &["zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case(
            "notes help",
            &["notes", "help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        // covers: changes diff
        case(
            "diff",
            &["diff"],
            &[],
            vec![ok(SNAP), ok(FILE), ok(RENAMED)],
            owner_and_port,
            0,
            3,
        ),
        // covers: changes diff --json
        case(
            "diff --json -U1",
            &["diff", "--json", "-U1"],
            &[],
            vec![ok(SNAP), ok(FILE), ok(RENAMED)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "diff, a file the daemon refuses",
            &["diff", "--unstaged", "--unified=0"],
            &[],
            vec![
                ok(SNAP),
                Reply::Answer(
                    409,
                    r#"{"error":{"code":"stale","hint":"read again"}}"#.into(),
                ),
                ok(RENAMED),
            ],
            owner_and_port,
            3,
            3,
        ),
        case(
            "diff --base, a path",
            &["diff", "--base", "dev", "--", "nope.rs"],
            &[],
            vec![
                ok(SNAP),
                ok(r#"{"path":"nope.rs","new":{"exists":true,"text":"a"}}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "diff, no repository",
            &["diff"],
            &[],
            vec![ok(r#"{"isRepo":false}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "diff, a bad -U",
            &["diff", "-U", "x"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "diff, base and staged",
            &["diff", "--staged", "--base", "m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: changes stage
        case(
            "stage",
            &["stage", "a.rs", "dir/"],
            &[],
            vec![ok("{}"), ok("{}")],
            owner_and_port,
            0,
            2,
        ),
        // covers: changes stage --json
        case(
            "stage --json",
            &["add", "--json", "a.rs"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stage --all",
            &["stage", "-A"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "stage, the second refused",
            &["stage", "a", "b"],
            &[],
            vec![
                ok("{}"),
                Reply::Answer(404, r#"{"error":"no such file"}"#.into()),
            ],
            owner_and_port,
            3,
            2,
        ),
        case(
            "unstage",
            &["unstage", "--", "-a"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unstage --all",
            &["unstage", "--all"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "stage, nothing",
            &["stage"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: changes discard
        case(
            "discard --yes",
            &["discard", "-y", "--staged", "a", "b"],
            &[],
            vec![ok(
                r#"{"restored":["a"],"trashed":["b"],"skipped":[{"path":"c","reason":"r"}]}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "discard, a failure",
            &["discard", "--yes", "a"],
            &[],
            vec![ok(r#"{"failed":[{"path":"a","error":"busy"}]}"#)],
            owner_and_port,
            1,
            1,
        ),
        // covers: changes discard --json
        case(
            "discard --json, a failure",
            &["discard", "--yes", "--json", "a"],
            &[],
            vec![ok(r#"{"failed":[{"path":"a","error":"busy"}]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "discard with no terminal",
            &["discard", "a"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: changes commit
        case(
            "commit",
            &["commit", "-m", "First line", "-m", "second"],
            &[],
            vec![
                ok(SNAP),
                ok(r#"{"oid":"abcdef123456","message":"First line\n\nsecond"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        // covers: changes commit --json
        case(
            "commit --all --json",
            &["commit", "--all", "--json", "--message=x"],
            &[],
            vec![ok("{}"), ok(SNAP), ok(r#"{"oid":"a"}"#)],
            owner_and_port,
            0,
            3,
        ),
        case(
            "commit, nothing staged",
            &["commit", "-m", "x"],
            &[],
            vec![ok(r#"{"isRepo":true,"counts":{"staged":0}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "commit, no repository",
            &["commit", "-m", "x"],
            &[],
            vec![ok(r#"{"isRepo":false}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "commit, no message",
            &["commit", "-m", "  "],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: changes notes
        case(
            "notes",
            &["notes"],
            &[],
            vec![ok(NOTES)],
            owner_and_port,
            0,
            1,
        ),
        // covers: changes notes --json
        case(
            "notes --json",
            &["notes", "--json"],
            &[],
            vec![ok(NOTES)],
            owner_and_port,
            0,
            1,
        ),
        // covers: changes open --json
        case(
            "open --json",
            &["open", "--json"],
            &[],
            vec![ok(r#"{"workspacePath":"/w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes --status --file",
            &["note", "list", "--status", "all", "--file", "x.rs"],
            &[],
            vec![ok(r#"{"notes":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: changes notes add
        case(
            "notes add",
            &[
                "notes", "add", "src/a.rs", "-L", "3-5", "--old", "-m", "why?",
            ],
            &[],
            vec![ok(
                r#"{"note":{"id":"n2","path":"src/a.rs","startLine":3,"endLine":5}}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes add, bad lines",
            &["notes", "add", "f", "--line=x", "-m", "c"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "notes resolve",
            &["notes", "done", "n1", "n2"],
            &[],
            vec![ok(r#"{"changed":["n1"],"unchanged":["n2"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes reopen",
            &["notes", "reopen", "n1"],
            &[],
            vec![ok(r#"{"changed":["n1"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes delete",
            &["notes", "rm", "n1", "--json"],
            &[],
            vec![ok(r#"{"deleted":["n1"]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes delete, no ids",
            &["notes", "delete"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "notes edit",
            &["notes", "edit", "n1", "-m", "new words"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes send",
            &["notes", "send", "--no-wake", "n1"],
            &[],
            vec![ok(
                r#"{"sent":2,"target":"ws","deliveryId":"d1","remaining":1}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "notes, an unknown verb",
            &["notes", "zap"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: changes open
        case(
            "open",
            &["open"],
            &[],
            vec![ok(r#"{"workspacePath":"/w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: changes init --json
        case(
            "init",
            &["init", "--json"],
            &[],
            vec![ok(r#"{"repoRoot":"/w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--workspace by name",
            &["open", "-w", "docs"],
            &[],
            vec![
                ok(r#"{"projectPath":"/w/docs"}"#),
                ok(r#"{"workspacePath":"/w/docs"}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        // covers: changes init
        case(
            "--workspace, a folder",
            &["init", "--workspace=."],
            &[],
            vec![ok(r#"{"repoRoot":"/w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case("the gate", &["list"], &[], vec![], nothing, 1, 0),
        case(
            "the bulletin",
            &["open"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a passport on the cell socket",
                &["list"],
                &[],
                vec![ok(SNAP)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a passport, a POST",
                &["stage", "a"],
                &[],
                vec![ok("{}")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            case(
                "a session with no passport",
                &["list"],
                &[],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ),
        with(
            vec![("K2_PORT", dead_port())],
            case("a dead port", &["list"], &[], vec![], owner_only, 1, 0),
        ),
        case(
            "an empty reply",
            &["open"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a reset",
            &["list"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        Case {
            compare: Compare::Waited,
            ..with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "no answer on the socket: never sent again",
                    &["stage", "a"],
                    &[],
                    vec![Reply::Close],
                    port_only,
                    1,
                    1,
                ),
            )
        },
    ];
    for c in v {
        rig.check(&c);
    }
    for (name, args, replies, code, requests) in [
        ("the list, not JSON", &["list"][..], vec![ok("nope")], 1, 1),
        (
            "a list that raises",
            &[],
            vec![ok(r#"{"isRepo":true,"files":[{"group":"x"}]}"#)],
            1,
            1,
        ),
        (
            "a commit answer that is a list",
            &["commit", "-m", "x"],
            vec![ok(SNAP), ok("[1]")],
            1,
            2,
        ),
        ("notes, a list", &["notes"], vec![ok("[]")], 1, 1),
    ] {
        rig.check(&Case {
            compare: Compare::Traceback,
            ..case(name, args, &[], replies, owner_and_port, code, requests)
        });
    }
}

/// Help anywhere on the line prints the help page and sends nothing, on
/// every verb (each that changes something too).
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let lines: &'static [&'static [&'static str]] = &[
        &["stage", "a", "-h"],
        &["stage", "--all", "--help"],
        &["unstage", "a", "--help"],
        &["discard", "--yes", "a", "-h"],
        &["commit", "-m", "x", "--help"],
        &["commit", "-m", "-h"],
        &["diff", "-h"],
        &["list", "--help"],
        &["notes", "add", "f", "-L", "1", "-m", "c", "-h"],
        &["notes", "resolve", "n1", "--help"],
        &["notes", "delete", "n1", "-h"],
        &["notes", "edit", "n1", "-m", "c", "--help"],
        &["notes", "send", "-h"],
        &["open", "--help"],
        &["init", "-h"],
        &["zap", "--help"],
        &["-w", "docs", "--help"],
    ];
    for args in lines {
        rig.check(&case(
            "help anywhere",
            args,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
        rig.check(&case(
            "help anywhere, no daemon",
            args,
            &[],
            vec![],
            nothing,
            0,
            0,
        ));
    }
}
