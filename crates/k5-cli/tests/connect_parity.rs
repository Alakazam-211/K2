//! `k5-cli connect status` against the shell CLI's `cli/k2 connect
//! status`, on the parity rig ([`common::parity`]): the same requests,
//! stdout, stderr and exit code. A stub daemon answers; nothing talks to
//! the Connect control plane or the account service (status never does).
//!
//! - The files: no account and no pairing; an account session (its
//!   email, a token with a newline in it, values that are not strings,
//!   not JSON, not a dict); a pairing with its token on disk, without it,
//!   not a dict, a value that is not a string.
//! - The daemon: its token (held, not held, its vault unreadable, a
//!   refusal, a reset: `no`), the tunnel (running with or without a URL,
//!   released, disabled, stopped, its error, a permission card, not JSON,
//!   empty, a list), the bulletin, migrating; no port or no token (the
//!   gate is skipped: one line says there is no daemon), `daemon.port`
//!   when `heartbeat.port` is missing.
//! - A passport in a K2 terminal (TCP: the cell socket doesn't serve
//!   /cli/tunnel/), a heal, the locked tool's checks, the guard, a word
//!   after `status`.
//! - No answer: to the token question, the line and death by SIGPIPE; to
//!   the status, the line and exit 1 after the four lines.
//! - What the binary hands to the shell: help, a subdomain the shell's
//!   `eval` reads otherwise, a file that is not UTF-8, a port that is no
//!   port.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "connect";
const ACCOUNT: &str =
    r#"{"access_token":"a.b.c","refresh_token":"r1","email":" me@acme.example "}"#;
const NO_TOKEN: &str = r#"{"subdomain":"acme","token":"","device_id":"d1"}"#;
const PAIRED: &str = r#"{"subdomain":"acme","token":"tok","device_id":"d1"}"#;
const RUNNING: &str = r#"{"running":true,"public_url":"https://acme.k2.dev","enabled":true}"#;
const STOPPED: &str = r#"{"running":false,"enabled":true}"#;

fn put(r: &Rig, name: &str, body: Option<&[u8]>) {
    let p = r.k2home.join(name);
    match body {
        Some(b) => std::fs::write(&p, b).unwrap(),
        None => {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The two files and `daemon.port` as a case wants them.
fn files(r: &Rig, account: Option<&str>, tunnel: Option<&str>) {
    put(r, "connect-account.json", account.map(str::as_bytes));
    put(r, "tunnel.json", tunnel.map(str::as_bytes));
    put(r, "daemon.port", None);
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn connect_status_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        case(
            "nothing paired: the daemon's token, then the tunnel",
            &["status"],
            &[],
            vec![ok(r#"{"tokenSet":false}"#), ok(STOPPED)],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "an account and a pairing on disk: no token question",
            &["status"],
            &[],
            vec![ok(RUNNING)],
            |r| {
                files(r, Some(ACCOUNT), Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "the daemon holds the token; released",
            &["status"],
            &[],
            vec![
                ok(r#"{"tokenSet":true,"subdomain":"acme"}"#),
                ok(r#"{"running":false,"released":true,"enabled":false}"#),
            ],
            |r| {
                files(r, Some(ACCOUNT), Some(NO_TOKEN));
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "its vault can't be read; disabled",
            &["status"],
            &[],
            vec![
                ok(r#"{"tokenSet":false,"tokenUnreadable":true}"#),
                ok(r#"{"running":false,"enabled":false}"#),
            ],
            |r| {
                files(r, None, Some(NO_TOKEN));
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "running with no URL",
            &["status"],
            &[],
            vec![ok(r#"{"running":1,"public_url":""}"#)],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "a refusal to both: no, then the error",
            &["status"],
            &[],
            vec![
                Reply::Answer(403, r#"{"error":"forbidden"}"#.into()),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"owner_only","hint":"no é"}}"#.into(),
                ),
            ],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "a permission card on the status: no closing line (exit 0)",
            &["status"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "the status is not JSON",
            &["status"],
            &[],
            vec![ok("<html>\n")],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "the status is empty",
            &["status"],
            &[],
            vec![ok("\n\n")],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "the status is a list; the token answer is not JSON",
            &["status"],
            &[],
            vec![ok("nope"), ok("[1]")],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "the bulletin line",
            &["status"],
            &[],
            vec![ok(BULLETIN)],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "migrating, then the answers",
            &["status"],
            &[],
            vec![
                Reply::Answer(503, r#"{"state":"migrating"}"#.into()),
                ok(r#"{"tokenSet":true}"#),
                ok(RUNNING),
            ],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            0,
            3,
        ),
        case(
            "a reset on the token question: no",
            &["status"],
            &[],
            vec![Reply::Reset, ok(STOPPED)],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "a reset on the status: read as empty",
            &["status"],
            &[],
            vec![Reply::Reset],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        // The files.
        case(
            "an account file that is not JSON",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(r, Some("{"), Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "a token with a newline moves the email down",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(
                    r,
                    Some(r#"{"access_token":"a\nb","refresh_token":"r","email":"me"}"#),
                    Some(PAIRED),
                );
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "account values that are not strings",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(
                    r,
                    Some(r#"{"access_token":0,"refresh_token":{"x":1},"email":"me"}"#),
                    Some(PAIRED),
                );
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "an account file that is a list; a pairing that is a list",
            &["status"],
            &[],
            vec![ok(r#"{"tokenSet":false}"#), ok(STOPPED)],
            |r| {
                files(r, Some("[]"), Some("[1]"));
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "a pairing whose token is not a string",
            &["status"],
            &[],
            vec![ok(r#"{}"#), ok(STOPPED)],
            |r| {
                files(r, None, Some(r#"{"subdomain":"acme","token":5}"#));
                owner_and_port(r)
            },
            0,
            2,
        ),
        case(
            "a pairing with spaces, a $ and a double quote",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(
                    r,
                    None,
                    Some(r#"{"subdomain":"  a b\"$HOME  ","token":"t"}"#),
                );
                owner_and_port(r)
            },
            0,
            1,
        ),
        case(
            "an empty pairing file",
            &["status"],
            &[],
            vec![ok(r#"{"tokenSet":0}"#), ok(STOPPED)],
            |r| {
                files(r, Some(""), Some(""));
                owner_and_port(r)
            },
            0,
            2,
        ),
        // No daemon: the gate is skipped.
        case(
            "no port: no daemon, nothing sent",
            &["status"],
            &[],
            vec![],
            |r| {
                files(r, Some(ACCOUNT), Some(NO_TOKEN));
                owner_only(r)
            },
            0,
            0,
        ),
        case(
            "no token: no daemon",
            &["status"],
            &[],
            vec![],
            |r| {
                files(r, None, None);
                port_only(r)
            },
            0,
            0,
        ),
        case(
            "daemon.port when heartbeat.port is missing",
            &["status"],
            &[],
            vec![ok(r#"{"tokenSet":true}"#), ok(RUNNING)],
            |r| {
                files(r, None, None);
                owner_only(r);
                std::fs::write(r.k2home.join("daemon.port"), format!("{}\n", r.stub.port)).unwrap();
            },
            0,
            2,
        ),
        case(
            "a word after status: usage, exit 2",
            &["status", "now"],
            &[],
            vec![],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            2,
            0,
        ),
        case(
            "the virtual desktop's guard",
            &["status"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            3,
            0,
        ),
        case(
            "PROJECT_PATH",
            &["status"],
            &[("K2_PROJECT_PATH", "/srv/acme ws")],
            vec![ok(RUNNING)],
            |r| {
                files(r, None, Some(PAIRED));
                owner_and_port(r)
            },
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connect_status_with_a_passport() {
    let rig = Rig::new(VERB, "pass");
    let h = |extra: &[(&'static str, &str)]| with_passport(rig.stub.port, extra);
    let v = vec![
        with(
            h(&[("K2_HOOK_SOCK", "{sock}")]),
            case(
                "a K2 terminal: the passport over TCP",
                &["status"],
                &[],
                vec![ok(r#"{"tokenSet":true}"#), ok(RUNNING)],
                |r| {
                    files(r, None, None);
                    port_only(r)
                },
                0,
                2,
            ),
        ),
        with(
            h(&[]),
            case(
                "a refused passport heals once",
                &["status"],
                &[],
                vec![heal(&rig), ok(r#"{"tokenSet":true}"#), ok(STOPPED)],
                |r| {
                    files(r, None, None);
                    port_only(r)
                },
                0,
                3,
            ),
        ),
        case(
            "HOOK_SOCK set and no passport: the require-passport lines",
            &["status"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            3,
            0,
        ),
        with(
            h(&[("K2_HOOK_SOCK", "{nosock}")]),
            case(
                "HOOK_SOCK gone and the passport refused: the stale lines",
                &["status"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                |r| {
                    files(r, None, None);
                    port_only(r)
                },
                3,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connect_status_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let in_pipe = |c: Case| Case {
        compare: Compare::Bash32GetPipe {
            stdout: "",
            stderr: "",
        },
        ..c
    };
    let waited = |c: Case| Case {
        compare: Compare::Waited,
        ..c
    };
    let v = vec![
        in_pipe(case(
            "no answer to the token question: the line, SIGPIPE",
            &["status"],
            &[],
            vec![Reply::Close],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            141,
            1,
        )),
        in_pipe(case(
            "a 5xx with no body to the token question",
            &["status"],
            &[],
            vec![Reply::Answer(502, String::new())],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            141,
            1,
        )),
        waited(case(
            "no answer to the status: the four lines, the line, exit 1",
            &["status"],
            &[],
            vec![ok(r#"{"tokenSet":true}"#), Reply::Close],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            1,
            2,
        )),
        waited(case(
            "a 5xx with no body to the status",
            &["status"],
            &[],
            vec![Reply::Answer(500, String::new())],
            |r| {
                files(r, Some(ACCOUNT), Some(PAIRED));
                owner_and_port(r)
            },
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn connect_status_with_no_daemon_listening() {
    let rig = Rig::new(VERB, "dead");
    let port = dead_port();
    rig.check(&case(
        "nothing listens (7): no, and the tunnel read as empty",
        &["status"],
        &[("K2_PORT", port.as_str())],
        vec![],
        |r| {
            files(r, None, None);
            owner_only(r)
        },
        0,
        0,
    ));
}

/// What the binary hands to the shell, before it prints or sends
/// anything: both sides are the shell's.
#[test]
fn connect_status_hands_these_to_the_shell() {
    let rig = Rig::new(VERB, "shell");
    let shell = |c: Case| Case {
        compare: Compare::Shell,
        ..c
    };
    let v = vec![
        shell(case(
            "status --help",
            &["status", "--help"],
            &[],
            vec![],
            |r| {
                files(r, None, None);
                owner_and_port(r)
            },
            0,
            0,
        )),
        shell(case(
            "a subdomain with a quote: the shell's eval",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(r, None, Some(r#"{"subdomain":"it's","token":"t"}"#));
                owner_and_port(r)
            },
            0,
            1,
        )),
        shell(case(
            "a subdomain that is not ASCII",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(r, None, Some(r#"{"subdomain":"été","token":"t"}"#));
                owner_and_port(r)
            },
            0,
            1,
        )),
        shell(case(
            "an account file that is not UTF-8",
            &["status"],
            &[],
            vec![ok(STOPPED)],
            |r| {
                files(r, None, Some(PAIRED));
                put(r, "connect-account.json", Some(b"{\"email\":\"\xff\"}"));
                owner_and_port(r)
            },
            0,
            1,
        )),
        shell(case(
            "a port that is no port",
            &["status"],
            &[("K2_PORT", "41o0")],
            vec![],
            |r| {
                files(r, None, None);
                owner_only(r)
            },
            0,
            0,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}
