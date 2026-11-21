//! `k5-cli respond` against the shell CLI's `cli/k2 respond`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code for every path `cmd_respond` has.
//!
//! - The words: --final, --, `$*`, no message.
//! - The cell socket (a live `HOOK_SOCK`): the form over the socket only
//!   (a "not served" answer is the answer: no TCP after it), refusals
//!   (exit 0, no closing line), a re-minted, a healed and a dead passport,
//!   the bulletin, a port that is no port (the socket doesn't need it).
//! - An `id` tool in a session: `HOOK_SOCK` with no passport, a socket that
//!   is gone with a passport the probe finds dead (stale, exit 3) or good
//!   (then TCP).
//! - A host session (a passport, no socket): the shell's own curl over TCP
//!   with the passport as Bearer, nothing noted; no dot in the passport,
//!   or the owner only, is the "needs an API-launched session" line.
//! - The gates, before the words.
//! - No answer, as the 0.46.15 no-resend test makes each, on both arms:
//!   in time (28), an empty reply (52), a reset (56), an empty 200, not
//!   HTTP (curl 1), a 5xx with no body; TMPDIR unusable (no 76 here: the
//!   shell's curl for respond doesn't go through `_cli_curl`); nothing
//!   listening (7). Neither arm ever sends twice: the request count of
//!   each case says so.
//! - What the binary hands to the shell: -h and --help anywhere, and a
//!   `CODEX_` variable with `HOOK_SOCK` set.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::time::Duration;

const VERB: &str = "respond";
const OK: &str = r#"{"ok":true}"#;
const ROLE: &str = r#"{"role":"canonical"}"#;

/// A session the API started, with a live cell socket.
fn cell(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    let mut e = with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    e.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    e
}

/// A host session: the passport, no socket.
fn host(extra: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    with_passport(extra)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn respond_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let c = cell;
    let h = host;
    let refused = || Reply::Answer(403, REFUSED.into());
    let v = vec![
        // The cell socket.
        with(
            c(&[]),
            // covers: respond
            case(
                "a line over the cell socket",
                &["hello", "world"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "--final",
                &["--final", "done now"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "-- then a flag's word",
                &["--", "--final"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "--final twice, then --",
                &["--final", "--final", "--", "--", "x"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a word like a flag is text",
                &["-x", "hi"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "--json is text",
                &["--json", "hi"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "--final after text is text",
                &["hi", "--final"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "odd bytes",
                &["é & = /x \"q\" 'y' %41 + \t"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "two empty words are a space",
                &["", ""],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case("no message", &[], &[], vec![], port_only, 1, 0),
        ),
        with(
            c(&[]),
            case("an empty message", &[""], &[], vec![], port_only, 1, 0),
        ),
        with(
            c(&[]),
            case("--final alone", &["--final"], &[], vec![], port_only, 1, 0),
        ),
        with(
            c(&[]),
            case(
                "not served on the cell socket: no TCP after it",
                &["x"],
                &[],
                vec![Reply::Answer(
                    404,
                    r#"{"error":"route not served on cell socket"}"#.into(),
                )],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refusal: printed, exit 0, no closing line",
                &["x"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a re-minted passport is used from the start",
                &["x"],
                &[],
                vec![ok(OK)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refused passport heals once",
                &["x"],
                &[],
                vec![heal(&rig), ok(OK)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "a dead passport: the probe, then the restart line",
                &["x"],
                &[],
                vec![refused(), refused()],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "a refused passport the probe finds good: no line",
                &["x"],
                &[],
                vec![refused(), ok(ROLE)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            c(&[]),
            case(
                "the bulletin line when it worked",
                &["x"],
                &[],
                vec![ok(BULLETIN)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[]),
            case(
                "the answer's bytes as they came",
                &["x"],
                &[],
                vec![ok("x\n\n y \n")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(&[("K2_PORT", "41o0")]),
            case(
                "a port that is no port, with a socket: refused first",
                &["x"],
                &[],
                vec![],
                nothing,
                2,
                0,
            ),
        ),
        with(
            c(&[("K2_NO_OWNER_FALLBACK", "1")]),
            case(
                "the guard lets a passport through",
                &["x"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        Case {
            no_tmpdir: true,
            ..with(
                c(&[]),
                case(
                    "TMPDIR unusable",
                    &["x"],
                    &[],
                    vec![ok(OK)],
                    port_only,
                    0,
                    1,
                ),
            )
        },
        // An id tool in a session.
        case(
            "HOOK_SOCK and no passport",
            &["x"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "the socket gone, the probe finds the passport dead",
                &["x"],
                &[],
                vec![refused()],
                port_only,
                3,
                1,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "the socket gone, the probe finds it good: TCP",
                &["x"],
                &[],
                vec![ok(ROLE), ok(OK)],
                port_only,
                0,
                2,
            ),
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}"), ("K2_PORT", "41o0")]),
            case(
                "the socket gone and a port that is no port: the shell's port refusal",
                &["x"],
                &[],
                vec![],
                nothing,
                2,
                0,
            ),
        ),
        // A host session.
        with(
            h(&[]),
            case(
                "a host session: TCP, the passport as Bearer",
                &["hi", "there"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "--final over TCP",
                &["--final", "é&"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refusal over TCP: printed, no heal, no probe",
                &["x"],
                &[],
                vec![refused()],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a permission refusal over TCP: no closing line",
                &["x"],
                &[],
                vec![Reply::Answer(403, PERMISSION_ASK.into())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "the bulletin over TCP: no line",
                &["x"],
                &[],
                vec![ok(BULLETIN)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "the answer's bytes over TCP",
                &["x"],
                &[],
                vec![ok("x\n\n y \n")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case(
                "a re-minted passport over TCP",
                &["x"],
                &[],
                vec![ok(OK)],
                healed_on_disk,
                0,
                1,
            ),
        ),
        with(
            h(&[]),
            case("no message over TCP", &[], &[], vec![], port_only, 1, 0),
        ),
        with(
            h(&[("K2_PORT", "41o0")]),
            case(
                "a port that is no port over TCP: the shell's port refusal",
                &["x"],
                &[],
                vec![],
                nothing,
                2,
                0,
            ),
        ),
        case(
            "a passport with no dot",
            &["x"],
            &[("K2_HOOK_TOKEN", "nodot")],
            vec![],
            port_only,
            1,
            0,
        ),
        case("the owner only", &["x"], &[], vec![], owner_and_port, 1, 0),
        // The gates.
        case("no port", &["x"], &[], vec![], owner_only, 1, 0),
        case("no token", &["x"], &[], vec![], port_only, 1, 0),
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
            &["x"],
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

/// A Unix socket closed with the request unread: on Linux the reader gets
/// ECONNRESET (curl 56, nothing printed); macOS reads an end of stream,
/// an empty reply (52: the no-answer line, exit 1). Both CLIs see the
/// same on each; only bash's expected exit follows the OS.
fn cell_reset(c: Case) -> Case {
    if cfg!(target_os = "linux") {
        c
    } else {
        Case {
            code: 1,
            compare: Compare::Waited,
            ..c
        }
    }
}

/// The 0.46.15 no-resend test's no-answer kinds, on each arm. One request
/// each: the cell socket's try is the only one.
#[test]
fn respond_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let waited = |c: Case| Case {
        compare: Compare::Waited,
        ..c
    };
    let no_tmp = |c: Case| Case {
        no_tmpdir: true,
        ..c
    };
    let c = || cell(&[]);
    let h = || host(&[]);
    let garbage = || Reply::Raw(GARBAGE.to_vec());
    let empty_5xx = || Reply::Answer(502, String::new());
    let v = vec![
        waited(with(
            c(),
            case(
                "cell: an empty reply (52)",
                &["x"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        cell_reset(with(
            c(),
            case(
                "cell: a reset (56 on Linux)",
                &["x"],
                &[],
                vec![Reply::Reset],
                port_only,
                56,
                1,
            ),
        )),
        with(
            c(),
            case(
                "cell: an empty 200",
                &["x"],
                &[],
                vec![ok("")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            c(),
            case(
                "cell: not HTTP (curl 1)",
                &["x"],
                &[],
                vec![garbage()],
                port_only,
                1,
                1,
            ),
        ),
        with(
            c(),
            case(
                "cell: a 5xx with no body (no status read)",
                &["x"],
                &[],
                vec![empty_5xx()],
                port_only,
                0,
                1,
            ),
        ),
        no_tmp(waited(with(
            c(),
            case(
                "cell: TMPDIR unusable, an empty reply",
                &["x"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        ))),
        no_tmp(cell_reset(with(
            c(),
            case(
                "cell: TMPDIR unusable, a reset",
                &["x"],
                &[],
                vec![Reply::Reset],
                port_only,
                56,
                1,
            ),
        ))),
        with(
            h(),
            case(
                "TCP: an empty reply (52)",
                &["x"],
                &[],
                vec![Reply::Close],
                port_only,
                52,
                1,
            ),
        ),
        with(
            h(),
            case(
                "TCP: a reset (56)",
                &["x"],
                &[],
                vec![Reply::Reset],
                port_only,
                56,
                1,
            ),
        ),
        with(
            h(),
            case(
                "TCP: an empty 200",
                &["x"],
                &[],
                vec![ok("")],
                port_only,
                0,
                1,
            ),
        ),
        with(
            h(),
            case(
                "TCP: not HTTP (curl 1)",
                &["x"],
                &[],
                vec![garbage()],
                port_only,
                1,
                1,
            ),
        ),
        with(
            h(),
            case(
                "TCP: a 5xx with no body",
                &["x"],
                &[],
                vec![empty_5xx()],
                port_only,
                0,
                1,
            ),
        ),
        no_tmp(with(
            h(),
            case(
                "TCP: TMPDIR unusable, an empty reply",
                &["x"],
                &[],
                vec![Reply::Close],
                port_only,
                52,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Nothing accepts the connection: exit 7 after the tries, on either arm.
#[test]
fn respond_with_no_daemon() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&with(
        host(&[("K2_PORT", port.as_str())]),
        case("TCP: nothing listens", &["x"], &[], vec![], nothing, 7, 0),
    ));
    rig.check(&with(
        cell(&[("K2_HOOK_SOCK", "{deadsock}")]),
        case(
            "cell: a socket file nothing listens on",
            &["x"],
            &[],
            vec![],
            port_only,
            7,
            0,
        ),
    ));
}

/// No answer within 30 s on the cell socket: the line, exit 1. Slow.
#[test]
fn respond_on_a_cell_socket_that_never_answers() {
    let rig = Rig::new(VERB, "hangc");
    let c = Case {
        compare: Compare::Waited,
        ..with(
            cell(&[]),
            case(
                "cell: no answer in time (28)",
                &["x"],
                &[],
                vec![Reply::Hang(Duration::from_secs(35))],
                port_only,
                1,
                1,
            ),
        )
    };
    rig.check(&c);
}

/// No answer within 30 s over TCP: curl's 28, nothing printed. Slow.
#[test]
fn respond_over_tcp_that_never_answers() {
    let rig = Rig::new(VERB, "hangt");
    let c = with(
        host(&[]),
        case(
            "TCP: no answer in time (28)",
            &["x"],
            &[],
            vec![Reply::Hang(Duration::from_secs(35))],
            port_only,
            28,
            1,
        ),
    );
    rig.check(&c);
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn respond_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let c = cell;
    let v = vec![
        shell(with(
            c(&[]),
            case("--help", &["--help"], &[], vec![], port_only, 0, 0),
        )),
        shell(with(
            c(&[]),
            case(
                "-h after --final",
                &["--final", "-h"],
                &[],
                vec![],
                port_only,
                0,
                0,
            ),
        )),
        shell(with(
            c(&[]),
            case(
                "--help after text is text",
                &["hi", "--help"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        )),
        shell(with(
            c(&[]),
            case(
                "-- then --help",
                &["--", "--help"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        )),
        shell(with(
            // The marker of a tab K2 started (the passport's session): the
            // shell's Codex tab gate lets the call through.
            c(&[
                ("CODEX_THREAD_ID", "t1"),
                ("K2_CODEX_TAB_ISOLATED", "sessA"),
            ]),
            case(
                "a CODEX_ variable with HOOK_SOCK",
                &["x"],
                &[],
                vec![ok(OK)],
                port_only,
                0,
                1,
            ),
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
