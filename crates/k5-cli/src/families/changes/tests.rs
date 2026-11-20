use super::*;

fn src(file: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file),
    )
    .unwrap_or_else(|e| panic!("{file}: {e}"))
}

/// The `n`th `python3 -c '` block after `after` in `file`, numbered as
/// `python3 -c` numbers it.
fn python_of(file: &str, after: &str, nth: usize) -> Vec<String> {
    let text = src(file);
    let mut body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"))
        .1;
    for _ in 0..nth {
        let at = body.find("python3 -c '").expect("a python3 -c block") + 1;
        body = &body[at..];
    }
    let start = body.find("python3 -c '").expect("a python3 -c block") + "python3 -c '".len();
    let end = body[start..].find('\'').expect("the end of the Python");
    body[start..start + end]
        .split('\n')
        .map(str::to_string)
        .collect()
}

fn at(lines: &[String], n: usize, text: &str) {
    let l = &lines[n - 1];
    assert!(l.contains(text), "line {n} is {l:?}, not {text:?}");
}

#[test]
fn the_readers_lines_are_the_shell_s() {
    let l = python_of("changes.sh", "\n_changes_list() {", 0);
    for (n, t) in [
        (3, "s = json.load(sys.stdin)"),
        (4, r#"if not s.get("isRepo"):"#),
        (5, "Not a git repository"),
        (7, r#"head = s.get("branch")"#),
        (9, r#"(s.get("headOid") or "")[:7]"#),
        (13, r#"if s.get("upstream"):"#),
        (14, r#"s["upstream"]"#),
        (16, r#"if s.get("state") and s["state"] != "clean":"#),
        (17, "In progress"),
        (20, "mb ="),
        (21, "Compared with"),
        (22, r#"files = s.get("files") or []"#),
        (29, "width = min(60"),
        (31, r#"rows = [f for f in files if f.get("group") == g]"#),
        (32, "if not rows and not c.get(g):"),
        (35, r#"print("%s (%d)""#),
        (37, r#"name = f["path"]"#),
        (45, r#"stat = "+%d -%d""#),
        (46, r#"f.get("status", "?")"#),
        (49, "Showing the first"),
        (51, "change%s"),
    ] {
        at(&l, n, t);
    }
    let l = python_of("changes.sh", "\n_changes_discard() {", 1);
    at(&l, 3, "r = json.load(sys.stdin)");
    at(&l, 8, "for p in restored:");
    at(&l, 14, "for s in skipped:");
    at(&l, 15, "skipped   %s: %s");
    at(&l, 17, "FAILED");
    let l = python_of("changes-notes.sh", "\n_changes_notes_list() {", 0);
    for (n, t) in [
        (3, "r = json.load(sys.stdin)"),
        (4, "notes, c ="),
        (10, "for n in notes:"),
        (11, "currentStartLine"),
        (13, "where ="),
        (24, "by %s%s"),
        (25, r#"(n.get("body") or "").splitlines()"#),
    ] {
        at(&l, n, t);
    }
    let l = python_of("changes-notes.sh", "\n_changes_notes_ids() {", 0);
    at(&l, 5, "Deleted");
    at(&l, 11, r#"", ".join(done)"#);
    at(&l, 13, "Already");
    let l = python_of("changes-notes.sh", "\n_changes_notes_send() {", 0);
    at(&l, 5, "Sent %d note%s");
    at(&l, 7, r#"r["deliveryId"]"#);
    at(&l, 10, r#"r["remaining"]"#);
    let render = src("changes.sh");
    let render = render.split_once("_CH_PY_RENDER='").unwrap().1;
    let render: Vec<String> = render[..render.find('\'').unwrap()]
        .split('\n')
        .map(str::to_string)
        .collect();
    at(&render, 4, "d = json.load(sys.stdin)");
    at(&render, 5, r#"new_path = d.get("path") or """#);
    at(&render, 8, r#"a = "a/" + old_path"#);
    at(&render, 14, r#"parts = t.split("\n")"#);
    at(&render, 29, r#"if d.get("binary"):"#);
    at(&render, 31, r#"elif d.get("tooLarge"):"#);
    at(&render, 34, "ot, nt =");
    at(
        &render,
        37,
        "difflib.unified_diff(lines(ot), lines(nt), a, b, n=ctx)",
    );
    at(&render, 46, "meta = {k: d.get(k)");
}

#[test]
fn the_texts_are_the_shell_s() {
    let text = src("changes.sh") + &src("changes-notes.sh") + &src("changes-open.sh");
    for t in [
        "--workspace needs a path",
        "--base needs a ref",
        "unknown argument for k2 changes: $1 (see k2 changes --help)",
        "-U needs a number",
        "--base can't be mixed with --staged/--unstaged",
        "unknown flag for k2 changes diff: $1",
        "k2 changes unstage takes paths (no --all)",
        "k2 changes stage: give paths or --all, not both",
        "usage: k2 changes discard <path>... [--staged] [--yes]",
        "k2 changes discard needs --yes when not run on a terminal (untracked files and every copy it replaces go to the Trash)",
        "unknown argument for k2 changes commit: $1 (usage: k2 changes commit -m \\\"message\\\" [--all])",
        "nothing is staged to commit; stage files first (k2 changes stage <path>) or use --all",
        "not a git repository (make one with: k2 changes init)",
        "--status needs open|unsent|sent|resolved|all",
        "unknown argument for k2 changes notes: $1 (see k2 changes notes --help)",
        "usage: k2 changes notes add <path> --line <n>[-<m>] [--old] -m \\\"comment\\\"",
        "--line takes <n> or <n>-<m> (numbers)",
        "usage: k2 changes notes edit <id> -m \\\"comment\\\"",
        "unknown k2 changes notes command: $sub (list, add, resolve, reopen, edit, delete, send)",
        "unknown k2 changes command: $sub (list, diff, stage, unstage, discard, commit, notes, open, init)",
        "note: $fpath is an untracked folder (not expanded); name a file inside it",
        "note: the change list is capped at %d files; name paths to see the rest",
        "Asked K2 to open the Changes tab for %s (any window showing that workspace opens it).",
        "Initialized a git repository in %s (no commits yet).",
        "Discard changes to %d file(s)? Edited files go %s; untracked files and the current copies go to the Trash. [y/N] ",
        "Large files %s and %s differ (over 10 MB or 120,000 lines; not shown)",
    ] {
        assert!(text.contains(t), "the shell has no {t:?}");
    }
}

fn verb(words: &[&str]) -> Verb {
    parse(words).unwrap().0
}

fn hint(words: &[&str]) -> (String, bool) {
    let d = parse(words).unwrap_err();
    (d.hint, d.json)
}

#[test]
fn the_words() {
    let n = cli_name!();
    assert_eq!(
        verb(&[]),
        Verb::List {
            ws: String::new(),
            base: String::new()
        }
    );
    assert_eq!(
        verb(&["--base=main", "-w", "x"]),
        Verb::List {
            ws: "x".into(),
            base: "main".into()
        }
    );
    assert_eq!(verb(&["status", "--help"]), Verb::Help { notes: false });
    assert_eq!(verb(&["note", "add", "-h"]), Verb::Help { notes: true });
    assert_eq!(verb(&["notes", "help"]), Verb::Help { notes: true });
    assert_eq!(verb(&["help"]), Verb::Help { notes: false });
    assert_eq!(
        verb(&["zap"]),
        Verb::Unknown {
            notes: false,
            word: "zap".into()
        }
    );
    assert_eq!(
        verb(&["notes", "zap"]),
        Verb::Unknown {
            notes: true,
            word: "zap".into()
        }
    );
    assert_eq!(
        verb(&["commit", "-m", "a", "--message", "b"]),
        Verb::Commit {
            ws: String::new(),
            all: false,
            msg: "a\n\nb".into()
        }
    );
    assert_eq!(
        hint(&["commit", "--json", "-m", " \n"]),
        (
            format!("usage: {n} changes commit -m \"message\" [--all]"),
            true
        )
    );
    assert_eq!(hint(&["diff", "-Ux"]).0, "-U needs a number");
    assert_eq!(
        hint(&["diff", "--staged", "--base", "m"]).0,
        "--base can't be mixed with --staged/--unstaged"
    );
    assert_eq!(
        verb(&["diff", "-U0", "--base", "m", "--", "-a", "b"]),
        Verb::Diff(diff::Opts {
            ws: String::new(),
            base: "m".into(),
            mode: "base",
            ctx: "0".into(),
            paths: vec!["-a".into(), "b".into()],
        })
    );
    assert_eq!(
        hint(&["unstage", "-A"]).0,
        format!("{n} changes unstage takes paths (no --all)")
    );
    assert_eq!(
        hint(&["add", "-A", "x"]).0,
        format!("{n} changes stage: give paths or --all, not both")
    );
    assert_eq!(
        hint(&["stage"]).0,
        format!("usage: {n} changes stage <path>... | --all")
    );
    assert_eq!(
        hint(&["unstage"]).0,
        format!("usage: {n} changes unstage <path>...")
    );
    assert_eq!(
        hint(&["notes", "add", "f", "-L", "3-x", "-m", "c"]).0,
        "--line takes <n> or <n>-<m> (numbers)"
    );
    assert_eq!(
        verb(&["notes", "--file=f"]),
        Verb::NotesList {
            ws: String::new(),
            status: String::new(),
            file: "f".into()
        }
    );
    assert_eq!(
        verb(&["notes", "add", "f", "--lines=3-5", "--old", "-m", "c"]),
        Verb::NotesAdd {
            ws: String::new(),
            file: "f".into(),
            side: "old",
            start: "3".into(),
            end: "5".into(),
            msg: "c".into()
        }
    );
    assert_eq!(
        hint(&["open", "x"]).0,
        format!("unknown argument for {n} changes open: x")
    );
    assert_eq!(hint(&["-w"]).0, "--workspace needs a path");
}

#[test]
fn the_bodies() {
    assert_eq!(
        body("/w", Body::File("a é")),
        r#"{"path": "/w", "filePath": "a \u00e9"}"#
    );
    assert_eq!(body("", Body::All), "{}");
    assert_eq!(
        body(
            "",
            Body::Discard {
                staged: true,
                files: &["a".into(), "b".into()]
            }
        ),
        r#"{"includeStaged": true, "files": ["a", "b"]}"#
    );
    assert_eq!(
        json_body(
            "/w",
            &["ids[]=n1".into(), "ids[]=n2".into(), "resolved?=0".into()]
        ),
        r#"{"path": "/w", "ids": ["n1", "n2"], "resolved": false}"#
    );
    assert_eq!(
        json_body(
            "",
            &["startLine#=007".into(), "body=a=b".into(), "wake?=1".into()]
        ),
        r#"{"startLine": 7, "body": "a=b", "wake": true}"#
    );
    assert_eq!(q(""), "");
    assert_eq!(q("/a b"), "path=%2Fa%20b&");
}

#[test]
fn file_words() {
    let args: Vec<String> = ["x/../y/", "/abs", "./z"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        files("/w/sub", "/w", &args),
        vec!["/w/sub/y/", "/abs", "/w/sub/z"]
    );
    assert_eq!(files("/other", "/w", &args), args);
    assert_eq!(files("/w", "/w/", &args)[0], "x/../y/");
    assert_eq!(files("/w", "", &args), args);
}

#[test]
fn the_refusals() {
    let r = refusal(
        "409",
        br#"{"error":{"code":"conflict_x","message":"m"}}"#,
        false,
    );
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            "Error: m\n{\"error\": {\"code\": \"conflict_x\", \"hint\": \"m\", \"status\": 409}}\n",
            Exit::Refused
        )
    );
    let r = refusal("400", br#"{"error":"bad thing","code":"c1"}"#, true);
    assert_eq!(
        r.stderr,
        "{\"error\": {\"code\": \"c1\", \"hint\": \"bad thing\", \"status\": 400}}\n"
    );
    let r = refusal("000", b"", false);
    assert_eq!(r.exit, Exit::Error);
    let r = refusal("409", b"", false);
    assert!(r
        .stderr
        .contains("\"code\": \"conflicted\", \"hint\": \"the daemon refused (HTTP 409)\""));
}

#[test]
fn the_list() {
    let r = render::list(br#"{"isRepo":true,"branch":"main","upstream":"origin/main","ahead":1,"behind":0,"state":"merge_in_progress","base":{"ref":"dev","mergeBase":"abcdef123","ahead":2},"files":[{"path":"a.rs","group":"staged","status":"M","additions":3,"deletions":1},{"path":"new.rs","oldPath":"old.rs","group":"unstaged","status":"R"},{"path":"d/","group":"untracked","status":"?","dir":true}],"counts":{"staged":1,"unstaged":1,"untracked":1,"total":3}}"#);
    assert_eq!(
        r.stdout,
        "On main · 1 ahead, 0 behind origin/main\nIn progress: merge in progress\nCompared with dev (merge-base abcdef1 · 2 ahead, 0 behind)\n\nStaged (1)\n  M  a.rs             +3 -1\n\nUnstaged (1)\n  R  new.rs <- old.rs  \n\nUntracked (1)\n  ?  d/               (folder)\n\n3 changes\n"
    );
    let r = render::list(br#"{"isRepo":false,"workspacePath":"/w"}"#);
    assert_eq!(
        (r.stdout.as_str(), r.exit),
        ("Not a git repository: /w\n", Exit::NotFound)
    );
    let r = render::list(br#"{"isRepo":true,"detached":true,"headOid":5}"#);
    assert!(r
        .stderr
        .ends_with("TypeError: 'int' object is not subscriptable\n"));
    let r = render::list(br#"{"isRepo":true,"files":[{"group":"x"}]}"#);
    assert!(r.stderr.ends_with("KeyError: 'path'\n"), "{}", r.stderr);
}

#[test]
fn the_diff_render() {
    let r = diff::render(
        br#"{"path":"f.txt","old":{"exists":true,"text":"a\nb\n"},"new":{"exists":true,"text":"a\nc"}}"#,
        3,
        false,
    );
    assert_eq!(
        r.stdout,
        "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n\\ No newline at end of file\n"
    );
    let r = diff::render(
        br#"{"path":"n","new":{"exists":true,"text":"x\n"},"binary":false}"#,
        3,
        true,
    );
    assert_eq!(
        r.stdout,
        "{\"path\": \"n\", \"oldPath\": null, \"mode\": null, \"baseRef\": null, \"rev\": null, \"binary\": false, \"tooLarge\": null, \"additions\": null, \"deletions\": null, \"patch\": \"--- /dev/null\\n+++ b/n\\n@@ -0,0 +1 @@\\n+x\\n\"}\n"
    );
    let r = diff::render(
        br#"{"path":"b","oldPath":"a","old":{"exists":true},"new":{"exists":true},"binary":true}"#,
        3,
        false,
    );
    assert_eq!(
        r.stdout,
        "diff --git a/a b/b\nrename from a\nrename to b\nBinary files a/a and b/b differ\n"
    );
    let r = diff::render(b"[]", 3, false);
    assert!(r
        .stderr
        .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
}

#[test]
fn the_rows() {
    let r = diff::rows(
        br#"{"isRepo":true,"repoRoot":"/nonexistent/repo","rev":"r1","workspacePath":"/w","files":[{"path":"a","group":"staged"},{"path":"a","group":"unstaged"},{"path":"b","oldPath":"c","group":"unstaged"},{"path":"x/","group":"untracked","dir":true},{"path":"z","group":"base"}],"truncated":true,"limit":3}"#,
        "worktree",
        &[],
    );
    assert!(r.ok);
    assert_eq!(
        r.stdout,
        "{\"workspacePath\": \"/w\", \"repoRoot\": \"/nonexistent/repo\", \"rev\": \"r1\"}\na\x1f\x1f0\nb\x1fc\x1f0\nx/\x1f\x1f1\n"
    );
    assert_eq!(
        r.stderr,
        "note: the change list is capped at 3 files; name paths to see the rest\n"
    );
    let r = diff::rows(
        br#"{"isRepo":true,"repoRoot":"/nonexistent/repo","files":[{"path":"src/a","group":"staged"}]}"#,
        "staged",
        &["/nonexistent/repo/src/".into(), "./q".into()],
    );
    assert_eq!(r.stdout, "{\"workspacePath\": null, \"repoRoot\": \"/nonexistent/repo\", \"rev\": null}\nsrc/a\x1f\x1f0\nq\x1f\x1f0\n");
    assert!(!diff::rows(br#"{"isRepo":false}"#, "staged", &[]).ok);
}

#[test]
fn the_notes() {
    let r = render::notes(br#"{"workspacePath":"/w","counts":{"open":2,"unsent":1,"resolved":1},"notes":[{"id":"n1","path":"a.rs","startLine":3,"endLine":5,"currentStartLine":4,"currentEndLine":4,"side":"old","authorName":"Pat","sentAt":1,"outdated":true,"body":"one\ntwo"},{"id":"n2","path":"b","startLine":1,"endLine":1,"author":"x","resolvedAt":1}]}"#);
    assert_eq!(
        r.stdout,
        "Review notes on /w: 2 open (1 not sent yet), 1 resolved\n\nn1  a.rs:4 (before the change)  by Pat  [sent, outdated: the lines changed]\n    one\n    two\n\nn2  b:1  by x  [resolved by ?]\n    \n"
    );
    let r = render::ids(
        br#"{"changed":["n1"],"unchanged":["n2","n3"]}"#,
        IdsVerb::Reopen,
    );
    assert_eq!(r.stdout, "Reopened n1\nAlready open: n2, n3\n");
    let r = render::sent(br#"{"sent":1,"target":"ws","deliveryId":"d1","remaining":2}"#);
    assert_eq!(r.stdout, "Sent 1 note to the agent of ws (delivery d1)\n2 more did not fit in one message; send again.\n");
    assert_eq!(
        render::committed(br#"{"oid":"abcdef12","message":"first\nsecond"}"#).stdout,
        "Committed abcdef1: first\n"
    );
    assert_eq!(
        render::staged(br#"{"isRepo":true,"counts":{"staged":0.0}}"#).unwrap(),
        "0.0"
    );
    assert_eq!(render::staged(br#"{"isRepo":false}"#).unwrap(), "-1");
    assert_eq!(
        render::discard(br#"{"restored":["a"],"failed":[{"path":"b","error":"e"}]}"#).exit,
        Exit::Error
    );
    assert_eq!(render::discard_failed(br#"{"failed":[]}"#).exit, Exit::Ok);
    assert_eq!(render::resolved(br#"{"project_path":"/p"}"#), "/p");
}
