//! `k5-cli feedback ...` (`tickets`) against the shell CLI's
//! `cli/k2 feedback ...` for every form but `list` and `show`
//! (`tests/feedback_parity.rs`), on the parity rig ([`common::parity`]):
//! the same requests, stdout, stderr and exit code.
//!
//! - The family alone, an unknown verb, help anywhere on a line that
//!   changes something (it prints help and sends nothing).
//! - `ask`: every usage line, every field of the payload, the files
//!   (`path:line-end`, `~/`, a file that is not there), the brief from a
//!   file and from stdin (`--html -`), the daemon's errors (each exit),
//!   warnings, an answer that is not JSON or not an object, no daemon,
//!   no answer, `--wait` to each end, `TEST_FEEDBACK_DETECT`.
//! - `template`, `comment`, `resolve`, `assign` and `settings`: their
//!   words, their answers, the daemon's errors, a refusal's closing line,
//!   no answer (curl 7, 52, 56, an empty reply).
//! - The credential in a session: the owner token from disk where the
//!   shell sends it (COMPAT-58, kept for these verbs), the passport for a
//!   settings change.
//! - The named difference: an answer above 128 KiB, which the shell
//!   hands its Python in the environment and Linux refuses
//!   (`answers_above_128_kib`).

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "feedback";
const CREATED: &str = r#"{"ok":true,"id":"fb_0123456789","title":"Ship it?","kind":"question","priority":3,"status":"waiting","assignees":[]}"#;

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

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
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

/// The owner token and port on disk, and files the cases name.
fn files(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("notes.txt"), "n\n").unwrap();
    std::fs::write(r.proj.join("brief.html"), "<h2>Problem</h2>\n<p>é</p>\n").unwrap();
    std::fs::write(r.proj.join("bad.bin"), b"\xff\xfe x").unwrap();
    std::fs::write(r.proj.join("body.txt"), "line one\nline two\n\n").unwrap();
    std::fs::write(r.proj.join("empty.txt"), "\n\n").unwrap();
    std::fs::create_dir_all(r.proj.join("sub")).unwrap();
    std::fs::write(r.home.join("h.txt"), "h").unwrap();
}

#[test]
fn the_family_alone_and_an_unknown_verb() {
    let rig = Rig::new(VERB, "alone");
    check(
        &rig,
        vec![
            case(
                "feedback alone: its help",
                &[],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
            case(
                "an empty verb: its help",
                &[""],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
            case(
                "an unknown verb",
                &["nope", "x"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
            case("no daemon: the gate first", &[], &[], vec![], nothing, 1, 0),
            case(
                "the virtual desktop's guard",
                &["nope"],
                &[("K2_NO_OWNER_FALLBACK", "1"), ("K2_PORT", PORT)],
                vec![],
                owner_and_port,
                3,
                0,
            ),
        ],
    );
}

/// Help anywhere on a line that changes something prints help and sends
/// nothing (K2's rule). A help form is a page; any other line is the
/// shell's, which prints the verb's help before it reads a word.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let lines: [&'static [&'static str]; 13] = [
        &["ask", "--help"],
        &["ask", "-h"],
        &["ask", "Ship it?", "--wait", "-h"],
        &["ask", "--body", "--help"],
        &["comment", "--help"],
        &["comment", "fb_1", "hi", "-h"],
        &["resolve", "-h"],
        &["resolve", "fb_1", "--answered", "x", "--help"],
        &["assign", "fb_1", "ada", "--help"],
        &["assign", "-h"],
        &["settings", "--brief", "require", "-h"],
        &["settings", "--help"],
        &["template", "-h"],
    ];
    for args in lines {
        rig.check(&case(
            "help, nothing sent",
            args,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
    }
}

#[test]
fn ask_s_words() {
    let rig = Rig::new(VERB, "askw");
    let mut v = Vec::new();
    for (name, args) in [
        ("--file with no value", &["ask", "t", "--file"][..]),
        ("--html with no value", &["ask", "t", "--html"]),
        ("--body with no value", &["ask", "t", "--body"]),
        ("--body-file with no value", &["ask", "t", "--body-file"]),
        ("--kind with no value", &["ask", "t", "--kind"]),
        ("--options with no value", &["ask", "t", "--options"]),
        ("--priority with no value", &["ask", "t", "--priority"]),
        ("--assign with no value", &["ask", "t", "--assign"]),
        ("--to with no value", &["ask", "t", "--to"]),
        ("--timeout with no value", &["ask", "t", "--timeout"]),
        ("an unknown flag", &["ask", "t", "--nope", "x"]),
        ("a dash alone", &["ask", "-"]),
        ("an extra word", &["ask", "a b", "c"]),
        ("no title", &["ask"]),
        ("an empty title", &["ask", ""]),
        ("a bad kind", &["ask", "t", "--kind", "poll"]),
        ("a bad priority", &["ask", "t", "--priority", "9"]),
        ("a priority of 05", &["ask", "t", "--priority", "05"]),
        (
            "--body and --body-file",
            &["ask", "t", "--body", "b", "--body-file", "body.txt"],
        ),
        ("no body file", &["ask", "t", "--body-file", "nope.txt"]),
        (
            "a folder as the body file",
            &["ask", "t", "--body-file", "sub"],
        ),
        ("no brief file", &["ask", "t", "--html", "nope.html"]),
        ("--timeout with no --wait", &["ask", "t", "--timeout", "5"]),
        ("--timeout 0", &["ask", "t", "--wait", "--timeout", "0"]),
        ("--timeout x", &["ask", "t", "--wait", "--timeout", "1x"]),
        (
            "eleven files",
            &[
                "ask", "t", "--file", "a", "--file", "b", "--file", "c", "--file", "d", "--file",
                "e", "--file", "f", "--file", "g", "--file", "h", "--file", "i", "--file", "j",
                "--file", "k",
            ],
        ),
        (
            "the kind is checked before the priority",
            &["ask", "t", "--priority", "x", "--kind", "y"],
        ),
    ] {
        v.push(case(name, args, &[], vec![], files, 2, 0));
    }
    check(&rig, v);
}

#[test]
fn ask_files_a_ticket() {
    let rig = Rig::new(VERB, "ask");
    let defaulted = r#"{"ok":true,"id":"fb_0123456789","title":"Ship it?","kind":"approval","priority":1,"status":"waiting","assignees":["owner"],"defaultAssignee":true,"warnings":[{"code":"brief_sanitized","hint":"we took out a script"},"x",{"code":""},{}],"hasBrief":true}"#;
    let attached = r#"{"ok":true,"id":"fb_0123456789","title":"t","attachments":[{"path":"notes.txt","line":3,"size":2048},{"path":"/srv/x.rs","state":"gone"},{"state":"hidden"}]}"#;
    check(
        &rig,
        vec![
            // covers: feedback ask
            case(
                "plain",
                &["ask", "Ship it?"],
                &[],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
            case(
                "--json",
                &["ask", "Ship it?", "--json"],
                &[],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
            // covers: feedback ask --json
            case(
                "every field",
                &[
                    "ask",
                    "Ship it? é",
                    "--body",
                    "a \"quoted\"\nbody",
                    "--kind",
                    "approval",
                    "--options",
                    " Ship it , Hold\\, then ship ,, ",
                    "--priority",
                    "1",
                    "--assign",
                    " owner , ,julie",
                    "--json",
                ],
                &[
                    ("K2_AGENT_NAME", "acme-agent"),
                    ("K2_SESSION_ID", "sess-1"),
                    ("K2_CELL", "sidecar"),
                ],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
            case(
                "the default assignee, warnings, a brief kept",
                &["ask", "Ship it?", "--html", "brief.html", "--to", "x"],
                &[("CLAUDE_SESSION_ID", "claude-1")],
                vec![ok(defaulted)],
                files,
                0,
                1,
            ),
            case(
                "a brief the server dropped",
                &["ask", "t", "--html", "brief.html"],
                &[("K2_API_CELL", "1"), ("K2_SESSION_ID", "api-1")],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
            case(
                "a body file, its last newlines dropped",
                &["ask", "t", "--body-file", "body.txt", "--kind", "fyi"],
                &[],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
            case(
                "a body file of newlines: no body",
                &["ask", "t", "--body-file", "empty.txt"],
                &[],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
            case(
                "a body file that is not UTF-8",
                &["ask", "t", "--body-file", "bad.bin"],
                &[],
                vec![],
                files,
                2,
                0,
            ),
            case(
                "a brief that is not UTF-8",
                &["ask", "t", "--html", "bad.bin"],
                &[],
                vec![],
                files,
                2,
                0,
            ),
            case(
                "files, and the files in the answer",
                &[
                    "ask",
                    "t",
                    "--file",
                    "notes.txt:3",
                    "--file",
                    "missing.rs:5-9",
                    "--file",
                    "~/h.txt",
                    "--file",
                    "notes.txt",
                    "--file",
                    "./sub/../notes.txt:007",
                ],
                &[],
                vec![ok(attached)],
                files,
                0,
                1,
            ),
            case(
                "files sent, an answer that doesn't keep them",
                &["ask", "t", "--file", "notes.txt"],
                &[],
                vec![ok(CREATED)],
                files,
                1,
                1,
            ),
            case(
                "the session kind only",
                &["ask", "t", "--body", "b"],
                &[("K2_TEST_FEEDBACK_DETECT", "1"), ("K2_CELL", "canonical")],
                vec![],
                files,
                0,
                0,
            ),
        ],
    );
}

#[test]
fn ask_with_a_brief_on_stdin() {
    let rig = Rig::new(VERB, "askin");
    let mut v = vec![
        stdin(
            "<p>from stdin é</p>\n".as_bytes(),
            case(
                "a brief on stdin",
                &["ask", "t", "--html", "-"],
                &[],
                vec![ok(CREATED)],
                files,
                0,
                1,
            ),
        ),
        case(
            "an empty stdin",
            &["ask", "t", "--html", "-"],
            &[],
            vec![ok(r#"{"ok":true,"id":"x","hasBrief":true}"#)],
            files,
            0,
            1,
        ),
        stdin(
            b"\xff",
            case(
                "stdin that is not UTF-8",
                &["ask", "t", "--html", "-"],
                &[],
                vec![],
                files,
                2,
                0,
            ),
        ),
        stdin(
            b"<p>x</p>",
            case(
                "stdin read, then a usage line",
                &["ask", "t", "--html", "-", "--timeout", "3"],
                &[],
                vec![],
                files,
                2,
                0,
            ),
        ),
    ];
    // GNU mktemp names the template in its error, not a random name.
    if cfg!(target_os = "linux") {
        v.push(Case {
            no_tmpdir: true,
            ..case(
                "no temp folder: mktemp's line, then ours",
                &["ask", "t", "--html", "-"],
                &[],
                vec![],
                files,
                1,
                0,
            )
        });
    }
    check(&rig, v);
}

#[test]
fn ask_and_the_daemon_s_errors() {
    let rig = Rig::new(VERB, "askerr");
    check(
        &rig,
        vec![
            case(
                "an error as text",
                &["ask", "t"],
                &[],
                vec![Reply::Answer(400, r#"{"error":"bad title"}"#.into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "not found",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"error":{"code":"not_found","hint":"no"}}"#)],
                owner_and_port,
                4,
                1,
            ),
            case(
                "a brief is required",
                &["ask", "t"],
                &[],
                vec![Reply::Answer(
                    422,
                    r#"{"error":{"code":"brief_required","hint":"Add --html. See k2 study ticket-brief."}}"#.into(),
                )],
                owner_and_port,
                2,
                1,
            ),
            case(
                "an assignee is required",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"error":{"code":"assignee_required","hint":"x"}}"#)],
                owner_and_port,
                2,
                1,
            ),
            case(
                "a code that is a number",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"error":{"code":5}}"#)],
                owner_and_port,
                1,
                1,
            ),
            traceback(case(
                "an error that is a list",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"error":[1]}"#)],
                owner_and_port,
                1,
                1,
            )),
            case(
                "a refusal that is not JSON",
                &["ask", "t"],
                &[],
                vec![Reply::Answer(403, "Forbidden".into())],
                owner_and_port,
                1,
                1,
            ),
            case(
                "not ok",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"ok":false,"note":"é"}"#)],
                owner_and_port,
                1,
                1,
            ),
            traceback(case(
                "no id",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"ok":true}"#)],
                owner_and_port,
                1,
                1,
            )),
            traceback(case(
                "an id that is a number",
                &["ask", "t"],
                &[],
                vec![ok(r#"{"ok":true,"id":7}"#)],
                owner_and_port,
                1,
                1,
            )),
            traceback(case(
                "an answer that is a list",
                &["ask", "t"],
                &[],
                vec![ok("[1]")],
                owner_and_port,
                1,
                1,
            )),
            case(
                "an answer that is not JSON",
                &["ask", "t"],
                &[],
                vec![ok("<html>")],
                owner_and_port,
                1,
                1,
            ),
            case(
                "an empty reply",
                &["ask", "t"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                1,
                1,
            ),
            case(
                "a reset",
                &["ask", "t"],
                &[],
                vec![Reply::Reset],
                owner_and_port,
                1,
                1,
            ),
        ],
    );
    let port = dead_port();
    rig.check(&case(
        "no daemon",
        &["ask", "t"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

#[test]
fn ask_waits() {
    let rig = Rig::new(VERB, "wait");
    let answered = r#"{"ok":true,"status":"answered","answer":"Ship it","projectPath":"/srv/acme","comments":[{"author":"ada","body":" Ship it ","attachments":[{"path":"a.png"},"x",{"path":"/abs.txt"},{"name":"n"}]},{"author":"bo","body":"other"}]}"#;
    let discussion = r#"{"ok":true,"status":"needs_discussion","agentName":"acme-agent","comments":[{"author":"acme-agent","body":"q"},{"author":"ada","body":"Let's talk\nfirst"},{"author":"acme-agent","body":"ok"}]}"#;
    check(
        &rig,
        vec![
            case(
                "answered",
                &["ask", "t", "--wait"],
                &[],
                vec![ok(CREATED), ok(answered)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "answered, --json",
                &["ask", "t", "--wait", "--json"],
                &[],
                vec![ok(CREATED), ok(answered)],
                owner_and_port,
                0,
                2,
            ),
            case(
                "answered with no files",
                &["ask", "t", "--wait"],
                &[],
                vec![
                    ok(CREATED),
                    ok(r#"{"ok":true,"status":"answered","answer":7,"comments":null}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "a person who wants to talk",
                &["ask", "t", "--wait"],
                &[],
                vec![ok(CREATED), ok(discussion)],
                owner_and_port,
                5,
                2,
            ),
            case(
                "resolved with an answer",
                &["ask", "t", "--wait"],
                &[],
                vec![
                    ok(CREATED),
                    ok(r#"{"ok":true,"status":"resolved","answer":"done"}"#),
                ],
                owner_and_port,
                0,
                2,
            ),
            case(
                "dismissed with none",
                &["ask", "t", "--wait"],
                &[],
                vec![
                    ok(CREATED),
                    ok(r#"{"ok":true,"status":"dismissed","answer":""}"#),
                ],
                owner_and_port,
                1,
                2,
            ),
            case(
                "a poll that failed, then the answer",
                &["ask", "t", "--wait"],
                &[],
                vec![
                    ok(CREATED),
                    Reply::Answer(500, "oops".into()),
                    ok(r#"{"ok":true,"status":"answered","answer":"yes"}"#),
                ],
                owner_and_port,
                0,
                3,
            ),
            case(
                "out of time: the ticket stays waiting",
                &["ask", "t", "--wait", "--timeout", "1"],
                &[],
                vec![
                    ok(CREATED),
                    ok(r#"{"ok":true,"status":"waiting"}"#),
                    ok(r#"{"ok":true,"status":"waiting"}"#),
                ],
                owner_and_port,
                1,
                3,
            ),
        ],
    );
}

#[test]
fn template() {
    let rig = Rig::new(VERB, "tmpl");
    check(
        &rig,
        vec![
            // covers: feedback template
            case(
                "the skeleton",
                &["template"],
                &[],
                vec![],
                owner_and_port,
                0,
                0,
            ),
            case(
                "no daemon needed",
                &["template"],
                &[],
                vec![],
                nothing,
                0,
                0,
            ),
            case(
                "a word",
                &["template", "x"],
                &[],
                vec![],
                owner_and_port,
                2,
                0,
            ),
            case(
                "the virtual desktop's guard",
                &["template"],
                &[("K2_NO_OWNER_FALLBACK", "1")],
                vec![],
                nothing,
                3,
                0,
            ),
        ],
    );
}

#[test]
fn comment() {
    let rig = Rig::new(VERB, "comment");
    let added =
        r#"{"ok":true,"id":"fb_0123456789","attachments":[{"path":"notes.txt","size":10}]}"#;
    let mut v = vec![
        case(
            "plain",
            &["comment", "fb_0123", "Thanks é"],
            &[],
            vec![ok(r#"{"ok":true,"id":"fb_0123456789"}"#)],
            files,
            0,
            1,
        ),
        // covers: feedback comment --json
        case(
            "--json, the agent's name",
            &["comment", "--json", "fb_0123", "hi"],
            &[("K2_AGENT_NAME", "acme-agent")],
            vec![ok(r#"{"ok":true,"id":"fb_0123456789"}"#)],
            files,
            0,
            1,
        ),
        // covers: feedback comment
        case(
            "files, no text",
            &[
                "comment",
                "fb_0123",
                "--file",
                "notes.txt:2-4",
                "--file",
                "nope",
            ],
            &[],
            vec![ok(added)],
            files,
            0,
            1,
        ),
        case(
            "files the server dropped",
            &["comment", "fb_0123", "x", "--file", "notes.txt"],
            &[],
            vec![ok(r#"{"ok":true,"id":"fb_0123456789"}"#)],
            files,
            1,
            1,
        ),
        case("no id", &["comment"], &[], vec![], files, 2, 0),
        case("no text", &["comment", "fb_1"], &[], vec![], files, 2, 0),
        case(
            "an extra word",
            &["comment", "fb_1", "a", "b"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["comment", "fb_1", "-x"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "--file with no value",
            &["comment", "fb_1", "a", "--file"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "not found",
            &["comment", "fb_9", "x"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no ticket fb_9"}}"#.into(),
            )],
            files,
            4,
            1,
        ),
        case(
            "ambiguous",
            &["comment", "fb", "x"],
            &[],
            vec![ok(
                r#"{"error":{"code":"ambiguous_id","hint":"fb_1, fb_2"}}"#,
            )],
            files,
            4,
            1,
        ),
        case(
            "usage from the daemon",
            &["comment", "fb", "x"],
            &[],
            vec![ok(r#"{"error":{"code":"usage","hint":"x"}}"#)],
            files,
            2,
            1,
        ),
        case(
            "an error as text",
            &["comment", "fb", "x"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            files,
            1,
            1,
        ),
        case(
            "a refusal and its closing line",
            &["comment", "fb", "x"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            files,
            1,
            1,
        ),
        case(
            "the bulletin line",
            &["comment", "fb", "x"],
            &[],
            vec![ok(
                r#"{"ok":true,"id":"fb_1","bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#,
            )],
            files,
            0,
            1,
        ),
        traceback(case(
            "not JSON",
            &["comment", "fb", "x"],
            &[],
            vec![ok("<html>")],
            files,
            1,
            1,
        )),
        traceback(case(
            "a list",
            &["comment", "fb", "x"],
            &[],
            vec![ok("[1]")],
            files,
            1,
            1,
        )),
        traceback(case(
            "an error that is a number",
            &["comment", "fb", "x"],
            &[],
            vec![ok(r#"{"error":5}"#)],
            files,
            1,
            1,
        )),
        case(
            "an answer of newlines",
            &["comment", "fb", "x"],
            &[],
            vec![ok("\n\n")],
            files,
            1,
            1,
        ),
    ];
    v.extend(no_answer(&["comment", "fb", "x"]));
    check(&rig, v);
    let port = dead_port();
    rig.check(&case(
        "no daemon",
        &["comment", "fb", "x"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// A POST with no answer: an empty reply (52), a reset (56), an empty
/// 200. Sent once.
fn no_answer(args: &'static [&'static str]) -> Vec<Case> {
    vec![
        waited(case(
            "an empty reply",
            args,
            &[],
            vec![Reply::Close],
            files,
            1,
            1,
        )),
        waited(case("a reset", args, &[], vec![Reply::Reset], files, 1, 1)),
        case("an empty 200", args, &[], vec![ok("")], files, 1, 1),
    ]
}

#[test]
fn resolve() {
    let rig = Rig::new(VERB, "resolve");
    let mut v = vec![
        case(
            "resolved",
            &["resolve", "fb_0123"],
            &[],
            vec![ok(
                r#"{"ok":true,"id":"fb_0123456789","status":"resolved"}"#,
            )],
            files,
            0,
            1,
        ),
        // covers: feedback resolve
        case(
            "answered, with files",
            &[
                "resolve",
                "fb_0123",
                "--answered",
                "Ship Friday",
                "--file",
                "notes.txt",
            ],
            &[("K2_AGENT_NAME", "acme-agent")],
            vec![ok(
                r#"{"ok":true,"id":"fb_0123456789","status":"answered","attachments":[{"path":"notes.txt","size":1048576}]}"#,
            )],
            files,
            0,
            1,
        ),
        // covers: feedback resolve --json
        case(
            "--json",
            &["resolve", "--json", "fb_0123"],
            &[],
            vec![ok(
                r#"{"ok":true,"id":"fb_0123456789","status":"resolved"}"#,
            )],
            files,
            0,
            1,
        ),
        case("no id", &["resolve"], &[], vec![], files, 2, 0),
        case(
            "an extra word",
            &["resolve", "a", "b"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "--answered with no value",
            &["resolve", "a", "--answered"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "--answered blank",
            &["resolve", "a", "--answered", " \t "],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "--file with no --answered",
            &["resolve", "a", "--file", "notes.txt"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["resolve", "a", "--x"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "files the server dropped",
            &["resolve", "a", "--answered", "x", "--file", "notes.txt"],
            &[],
            vec![ok(r#"{"ok":true,"id":"a","status":"answered"}"#)],
            files,
            1,
            1,
        ),
        case(
            "not found",
            &["resolve", "zz"],
            &[],
            vec![ok(r#"{"error":{"code":"not_found"}}"#)],
            files,
            4,
            1,
        ),
        traceback(case(
            "not JSON",
            &["resolve", "zz"],
            &[],
            vec![ok("x")],
            files,
            1,
            1,
        )),
    ];
    v.extend(no_answer(&["resolve", "zz"]));
    check(&rig, v);
}

#[test]
fn assign() {
    let rig = Rig::new(VERB, "assign");
    let mut v = vec![
        // covers: feedback assign
        case(
            "two people",
            &[
                "assign",
                "fb_0123",
                "ada",
                "--to",
                " bo ,, cy",
                "\u{2028}dee",
            ],
            &[],
            vec![ok(
                r#"{"ok":true,"id":"fb_0123456789","assignees":["ada","bo","cy","dee"],"warnings":[{"code":"assignee_unknown","hint":"dee is no user here"},1]}"#,
            )],
            files,
            0,
            1,
        ),
        // macOS's bash 3.2 writes users[@]'s unbound-variable line here
        // until the shell's fix lands (`bash32-empty-array`).
        bash32_empty_array(
            case(
                "cleared",
                &["assign", "fb_0123", "--clear", "ada"],
                &[],
                vec![ok(r#"{"ok":true,"id":"fb_0123456789","assignees":[]}"#)],
                files,
                0,
                1,
            ),
            &["users"],
            None,
        ),
        case(
            "an empty name counts as one",
            &["assign", "fb_0123", ""],
            &[],
            vec![ok(r#"{"ok":true,"id":"fb_0123456789"}"#)],
            files,
            0,
            1,
        ),
        // covers: feedback assign --json
        case(
            "--json",
            &["assign", "fb_0123", "ada", "--json"],
            &[],
            vec![ok(
                r#"{"ok":true,"id":"fb_0123456789","assignees":["ada"]}"#,
            )],
            files,
            0,
            1,
        ),
        case("no id", &["assign"], &[], vec![], files, 2, 0),
        case("no one", &["assign", "fb_1"], &[], vec![], files, 2, 0),
        case(
            "--to with only commas",
            &["assign", "fb_1", "--to", ",,"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "--to with no value",
            &["assign", "fb_1", "--to"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        case(
            "an unknown flag",
            &["assign", "fb_1", "-n"],
            &[],
            vec![],
            files,
            2,
            0,
        ),
        traceback(case(
            "assignees that are numbers",
            &["assign", "fb_1", "ada"],
            &[],
            vec![ok(r#"{"ok":true,"id":"fb_1","assignees":[1]}"#)],
            files,
            1,
            1,
        )),
        case(
            "a refusal",
            &["assign", "fb_1", "ada"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            files,
            1,
            1,
        ),
    ];
    v.extend(no_answer(&["assign", "fb_1", "ada"]));
    check(&rig, v);
}

#[test]
fn settings() {
    let rig = Rig::new(VERB, "settings");
    let read = r#"{"briefPolicy":"warn","assigneePolicy":"require","defaultAssignee":"julie","defaultAssigneeKnown":false,"canEdit":true}"#;
    let mut v = vec![
        // covers: feedback settings
        case(
            "read",
            &["settings"],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "read, not an owner, no default",
            &["settings"],
            &[],
            vec![ok(
                r#"{"briefPolicy":"odd","assigneePolicy":null,"defaultAssignee":""}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        // covers: feedback settings --json
        case(
            "read --json",
            &["settings", "--json"],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a change",
            &[
                "settings",
                "--brief",
                "require",
                "--assignee-required",
                "off",
                "--default-assignee",
                "none",
            ],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty default",
            &["settings", "--default-assignee", ""],
            &[],
            vec![ok(read)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "owner only",
            &["settings", "--brief", "off"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"Only the owner can change these."}}"#
                    .into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "an unknown user",
            &["settings", "--default-assignee", "zed"],
            &[],
            vec![ok(r#"{"error":{"code":"unknown_user","hint":"no zed"}}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "not JSON",
            &["settings"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "a policy that is a list",
            &["settings"],
            &[],
            vec![ok(r#"{"briefPolicy":["x"]}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "a bad value",
            &["settings", "--brief", "on"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no value",
            &["settings", "--assignee-required"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "no default",
            &["settings", "--default-assignee"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "a word",
            &["settings", "show"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ];
    v.extend(no_answer(&["settings"]));
    check(&rig, v);
}

/// In a session (`HOOK_SOCK` set) with the owner token on disk, the shell
/// sends the owner token over TCP for these verbs, a passport or not
/// (COMPAT-58), and so does the binary; a settings change sends the
/// passport.
#[test]
fn the_credential_in_a_session() {
    let rig = Rig::new(VERB, "session");
    let sock = with_passport(&[("K2_HOOK_SOCK", "{nosock}")]);
    let no_passport = vec![
        ("K2_PORT", PORT.to_string()),
        ("K2_HOOK_SOCK", "{nosock}".to_string()),
    ];
    let read = r#"{"briefPolicy":"off","assigneePolicy":"off","canEdit":false}"#;
    check(
        &rig,
        vec![
            with(
                sock.clone(),
                case(
                    "ask: the owner token",
                    &["ask", "t"],
                    &[],
                    vec![ok(CREATED)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "comment: the owner token",
                    &["comment", "fb", "x"],
                    &[],
                    vec![ok(r#"{"ok":true,"id":"fb"}"#)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "resolve: the owner token",
                    &["resolve", "fb"],
                    &[],
                    vec![ok(r#"{"ok":true,"id":"fb"}"#)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "assign: the owner token",
                    &["assign", "fb", "ada"],
                    &[],
                    vec![ok(r#"{"ok":true,"id":"fb"}"#)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "settings read: the owner token",
                    &["settings"],
                    &[],
                    vec![ok(read)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "settings change: the passport",
                    &["settings", "--brief", "warn"],
                    &[],
                    vec![ok(read)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "no owner token on disk: the passport",
                    &["comment", "fb", "x"],
                    &[],
                    vec![ok(r#"{"ok":true,"id":"fb"}"#)],
                    port_only,
                    0,
                    1,
                ),
            ),
            with(
                sock.clone(),
                case(
                    "ask, no owner token on disk: the passport",
                    &["ask", "t"],
                    &[],
                    vec![ok(CREATED)],
                    port_only,
                    0,
                    1,
                ),
            ),
            with(
                no_passport.clone(),
                case(
                    "no passport: the owner token, from disk",
                    &["ask", "t"],
                    &[],
                    vec![ok(CREATED)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                no_passport,
                case(
                    "no passport, comment",
                    &["comment", "fb", "x"],
                    &[],
                    vec![ok(r#"{"ok":true,"id":"fb"}"#)],
                    files,
                    0,
                    1,
                ),
            ),
            with(
                with_passport(&[]),
                case(
                    "a passport, no session: the passport",
                    &["ask", "t"],
                    &[],
                    vec![ok(CREATED)],
                    files,
                    0,
                    1,
                ),
            ),
        ],
    );
}

/// The named difference: the shell hands the answer to its Python in the
/// environment (`RESP="$resp"`), and Linux refuses one environment string
/// over 128 KiB, so the shell prints `Argument list too long` and exits
/// 126; the binary reads the answer from the body and prints what it
/// prints for a short one. The requests are the same.
#[cfg(target_os = "linux")]
#[test]
fn answers_above_128_kib() {
    let rig = Rig::new(VERB, "large");
    let pad = "x".repeat(200 * 1024);
    let big = format!(
        r#"{{"ok":true,"id":"fb_0123456789","status":"answered","assignees":["ada"],"briefPolicy":"off","assigneePolicy":"off","canEdit":true,"pad":"{pad}"}}"#
    );
    for (args, want) in [
        (&["comment", "fb", "x"][..], "Added comment to fb_01234.\n"),
        (&["resolve", "fb"], "Marked fb_01234 answered.\n"),
        (&["assign", "fb", "ada"], "Assigned fb_01234 \u{2192} ada.\n"),
        (
            &["settings"],
            "Ticket rules (Settings \u{2192} Tickets)\n  Require an HTML brief:  off\n  Require an assignee:    off\n  Default assignee:       none\n",
        ),
    ] {
        let args: &'static [&'static str] = Box::leak(args.to_vec().into_boxed_slice());
        let c = case("an answer above 128 KiB", args, &[], vec![ok(&big)], files, 126, 1);
        let mut logs = Vec::new();
        let mut outs = Vec::new();
        for (bash, side) in [(true, Side::Bash), (false, Side::Rust)] {
            rig.prepare(side, &c);
            let out = rig.run(bash, &c);
            logs.push(
                rig.read(side, !bash, c.name)
                    .iter()
                    .map(|r| normalise(r, !bash, VERB, c.name))
                    .collect::<Vec<_>>(),
            );
            outs.push(out);
        }
        let (b, r) = (&outs[0], &outs[1]);
        assert_eq!(b.code, Some(126), "{args:?}: bash: {b:?}");
        assert!(
            b.stderr.contains("Argument list too long"),
            "{args:?}: bash's stderr: {}",
            b.stderr
        );
        assert_eq!(b.stdout, "", "{args:?}: bash's stdout");
        assert_eq!(
            (r.code, r.stdout.as_str(), r.stderr.as_str()),
            (Some(0), want, ""),
            "{args:?}: the binary"
        );
        assert_eq!(logs[0].len(), 1, "{args:?}: bash's requests");
        // Each side has its own stub, so its own port in `Host:`.
        let unported = |r: &str| -> String {
            r.lines()
                .filter(|l| !l.starts_with("Host: "))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            unported(&logs[0][0]),
            unported(&logs[1][0]),
            "{args:?}: the requests"
        );
    }
}
