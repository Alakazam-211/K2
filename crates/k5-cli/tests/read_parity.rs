//! `k5-cli read` against the shell CLI's `cli/k2 read`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code
//! for every local path `cmd_read` has.
//!
//! - The words: the workspace (the first word, a flag too), `--lines`,
//!   `--agent`, `--session`, `--token`, extra words, no workspace.
//! - The workspace as `_split_remote_target` leaves it on this server:
//!   `ws@`, `a::b`, the wire form.
//! - The answers: lines (many, unicode, long, not strings), `lines` as a
//!   string or an object, a daemon error object (exit 3 for its refusal
//!   codes, else 1), a text error, not JSON, a refusal with a permission
//!   card (the closing line), the bulletin.
//! - The owner from disk over TCP, a passport over TCP, a live cell socket
//!   ("not served", an empty answer and no answer go on to TCP), a socket
//!   file nothing listens on; the owner token on disk with a passport,
//!   which neither side sends since 0.46.17.
//! - A re-minted, a healed and a dead passport, the gates, PROJECT_PATH
//!   and a PWD through a link.
//! - No answer (an empty reply, a 5xx with no body, no answer in time):
//!   the line, then death by SIGPIPE (141); a reset, not HTTP, an empty
//!   200, TMPDIR unusable and no daemon: the reader on an empty input
//!   (an empty line on stderr, exit 1).
//! - What the binary hands to the shell: a workspace on another server, a
//!   `--lines` with a space or a `#`, `HOOK_SOCK` naming a live socket and
//!   no passport, `--help` (the shell prints the read help and sends
//!   nothing, since 0.46.17), a port that is no port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "read";
const LINES: &str =
    r#"{"lines":["$ cargo test","   Compiling k5 v0.1","test result: ok. 3 passed"]}"#;
const ODD: &str =
    r#"{"lines":["é ü 日本 🚀","tab\there","",1,null,true,[1,"x"],{"k":"v"}," trailing  "]}"#;
const ROLE: &str = r#"{"role":"canonical"}"#;

fn many_lines(n: usize) -> String {
    let lines: Vec<String> = (0..n)
        .map(|i| format!("\"line {i}: {}\"", "x".repeat(i % 80)))
        .collect();
    format!(r#"{{"lines":[{}]}}"#, lines.join(","))
}

fn cell(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    e
}

fn host(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    with_passport(extra)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// No answer on the left of the pipe: the line, then SIGPIPE. macOS's
/// bash 3.2 runs the reader on an empty input instead, which prints an
/// empty line on stderr ([`Compare::Bash32PipeReader`]).
fn in_pipe(c: Case) -> Case {
    Case {
        compare: Compare::Bash32PipeReader {
            stdout: "",
            stderr: "\n",
        },
        ..c
    }
}

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn words(v: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(v.into_boxed_slice())
}

#[test]
fn read_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let h = host;
    let v = vec![
        case(
            "a workspace",
            &["acme"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--lines and --agent",
            &["acme ws", "--lines", "5", "--agent", "rev iewer é"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--session after a workspace",
            &["acme", "--session", "s 1/é", "--agent", "a"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--session first is the workspace --session",
            &["--session", "s1"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--token is skipped here",
            &["acme", "--token", "t-1"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "extra words are skipped",
            &["acme", "more", "words"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "--lines as it is: & and %XX",
            &["acme", "--lines", "5&x=%4a"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "an empty --lines",
            &["acme", "--lines", ""],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a value flag's value like a flag",
            &["acme", "--agent", "--lines"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "ws@ is this server",
            &["acme@"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a::b with no dot is this server",
            &["a::b"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "the wire form is this server",
            &["fp::ws::agent"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case("no workspace", &[], &[], vec![], owner_and_port, 1, 0),
        case(
            "an empty workspace, no session",
            &["", "--lines", "5"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "an empty workspace and a session",
            &["", "--session", "s1"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "odd lines",
            &["acme"],
            &[],
            vec![ok(ODD)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "many lines",
            &["acme", "--lines", "500"],
            &[],
            vec![reply(many_lines(500))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a long line",
            &["acme"],
            &[],
            vec![reply(format!(r#"{{"lines":["{}"]}}"#, "ü".repeat(12000)))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no lines",
            &["acme"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "lines as a string",
            &["acme"],
            &[],
            vec![ok(r#"{"lines":"ab"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "lines as an object",
            &["acme"],
            &[],
            vec![ok(r#"{"lines":{"k":1,"j":2}}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a refusal code: exit 3",
            &["acme"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"nope é"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "not_connected: exit 3",
            &["acme"],
            &[],
            vec![ok(r#"{"error":{"code":"not_connected"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "gated: exit 3",
            &["acme"],
            &[],
            vec![ok(r#"{"error":{"code":"gated","hint":"ask first"}}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "another code: exit 1",
            &["acme"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no such workspace"}}"#.into(),
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an error object with neither",
            &["acme"],
            &[],
            vec![ok(r#"{"error":{"x":1}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a code that is not a string",
            &["acme"],
            &[],
            vec![ok(r#"{"error":{"code":5,"hint":[1,"a"]}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a text error",
            &["acme"],
            &[],
            vec![ok(r#"{"error":"bad","lines":["x"]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a null error",
            &["acme"],
            &[],
            vec![ok(r#"{"error":null}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a refusal with a permission card: the closing line",
            &["acme"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON: its text on stderr",
            &["acme"],
            &[],
            vec![ok("x\n")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON, long: 200 characters",
            &["acme"],
            &[],
            vec![reply("é".repeat(300))],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an empty answer",
            &["acme"],
            &[],
            vec![ok("")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the bulletin line when it worked",
            &["acme"],
            &[],
            vec![ok(BULLETIN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "migrating, then the answer",
            &["acme"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(LINES),
            ],
            owner_and_port,
            0,
            2,
        ),
        Case {
            cwd: Some(link_to_proj(&rig)),
            ..case(
                "PWD through a link",
                &["acme"],
                &[],
                vec![ok(LINES)],
                owner_and_port,
                0,
                1,
            )
        },
        case(
            "PROJECT_PATH",
            &["acme"],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "a long workspace name",
            words(vec![leak("w é".repeat(700))]),
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        ),
        // A passport.
        with(
            h(&[]),
            case(
                "a host session: the passport over TCP",
                &["acme"],
                &[],
                vec![ok(LINES)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport is used from the start",
                &["acme"],
                &[],
                vec![ok(LINES)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["acme"],
                &[],
                vec![heal(&rig), ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["acme"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                1,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport the probe finds good: no line",
                &["acme"],
                &[],
                vec![Reply::Answer(403, REFUSED.into()), ok(ROLE)],
                port_only,
                1,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone, no owner on disk: the passport",
                &["acme"],
                &[],
                vec![ok(LINES)],
                port_only,
                0,
                1,
            ),
        ),
        // 0.46.17: the shell's cli_send sends the passport too when the
        // session's socket failed (`_k2_sock_fallback_token`).
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone, the owner on disk: the passport",
                &["acme"],
                &[],
                vec![ok(LINES)],
                owner_and_port,
                0,
                1,
            ),
        ),
        // The gates.
        case("no port", &["acme"], &[], vec![], owner_only, 1, 0),
        case("no token", &["acme"], &[], vec![], port_only, 1, 0),
        case(
            "the gate comes before the words",
            &[],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &["acme"],
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

#[test]
fn read_over_the_cell_socket() {
    let rig = Rig::new(VERB, "cell");
    let c = cell;
    let not_served = || Reply::Answer(404, r#"{"error":"route not served on cell socket"}"#.into());
    let v = vec![
        with(
            c(&[]),
            case(
                "the cell socket",
                &["acme", "--lines", "3"],
                &[],
                vec![ok(LINES)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a session over it",
                &["acme", "--session", "s1"],
                &[],
                vec![ok(LINES)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "its trailing newlines go",
                &["acme"],
                &[],
                vec![ok("notjson\n\n")],
                port_only,
                1,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "not served: TCP",
                &["acme"],
                &[],
                vec![not_served(), ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "an empty answer: TCP",
                &["acme"],
                &[],
                vec![ok(""), ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "only newlines: TCP",
                &["acme"],
                &[],
                vec![ok("\n"), ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "no answer there: TCP",
                &["acme"],
                &[],
                vec![Reply::Close, ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "a reset there: TCP",
                &["acme"],
                &[],
                vec![Reply::Reset, ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refusal there is the answer",
                &["acme"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                1,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refusal code there",
                &["acme"],
                &[],
                vec![ok(r#"{"error":{"code":"gated","hint":"h"}}"#)],
                port_only,
                3,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refused passport heals there",
                &["acme"],
                &[],
                vec![heal(&rig), ok(LINES)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "the bulletin there",
                &["acme"],
                &[],
                vec![ok(BULLETIN)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[("K2_HOOK_SOCK", "{deadsock}")]),
            case(
                "a socket file nothing listens on: TCP",
                &["acme"],
                &[],
                vec![ok(LINES)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn read_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let c = || cell(&[]);
    let v = vec![
        in_pipe(case(
            "an empty reply: the line, then SIGPIPE",
            &["acme"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            141,
            1,
        )),
        in_pipe(case(
            "a 5xx with no body: the line, then SIGPIPE",
            &["acme"],
            &[],
            vec![Reply::Answer(502, String::new())],
            owner_and_port,
            141,
            1,
        )),
        case(
            "a reset: an empty line, exit 1",
            &["acme"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["acme"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        no_tmp(case(
            "TMPDIR unusable, a reset",
            &["acme"],
            &[],
            vec![Reply::Reset],
            owner_and_port,
            1,
            1,
        )),
        no_tmp(case(
            "TMPDIR unusable, an answer",
            &["acme"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        )),
        in_pipe(with(
            c(),
            case(
                "the cell socket and TCP: an empty reply on both",
                &["acme"],
                &[],
                vec![Reply::Close, Reply::Close],
                port_only,
                141,
                2,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn read_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens",
        &["acme"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
    rig.check(&with(
        cell(&[("K2_HOOK_SOCK", "{deadsock}"), ("K2_PORT", port.as_str())]),
        case(
            "a dead socket and no daemon",
            &["acme"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
    ));
}

/// No answer within 30 s: the line, then SIGPIPE. Slow.
#[test]
fn read_with_a_daemon_that_never_answers() {
    let rig = Rig::new(VERB, "hang");
    let c = in_pipe(case(
        "no answer in time (28)",
        &["acme"],
        &[],
        vec![Reply::Hang(Duration::from_secs(35))],
        owner_and_port,
        141,
        1,
    ));
    rig.check(&c);
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn read_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "a workspace on another server, no token for it",
            &["acme@box.example"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "ws::host.example, no token for it",
            &["acme::box.example"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "--lines with a space: curl refuses it",
            &["acme", "--lines", "5 6"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        )),
        shell(case(
            "--lines with a #: curl cuts it",
            &["acme", "--lines", "5#6"],
            &[],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        )),
        shell(case(
            "HOOK_SOCK a live socket and no passport: an empty Bearer",
            &["acme"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![ok(LINES)],
            owner_and_port,
            0,
            1,
        )),
        // Help never acts (0.46.17): the shell prints the read help and
        // reads nothing, even as a flag's value.
        shell(case(
            "--help prints the read help",
            &["--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "--agent --help",
            &["acme", "--agent", "--help"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        )),
        shell(case(
            "a port that is no port: the shell's port refusal",
            &["acme"],
            &[("K2_PORT", "41o0")],
            vec![],
            owner_only,
            2,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
