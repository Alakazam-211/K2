//! The help pages bash puts together from pieces that ask nothing
//! ([`k5_cli::pages::Composed`]): the deprecated tier (`help deprecated`
//! and its other spellings, `help-deprecated`) and chat's usage (`chat`,
//! `chat --help`, `chat -h`, `chat help`, `help chat`). `cli/k2` and the
//! binary, run as people run them against one logging stub daemon
//! ([`common::parity`]), must send the same requests (none, but the
//! session checks' probe) and print the same bytes with the same exit.
//!
//! The rows: an owner's terminal; an agent's, on its cell socket and over
//! TCP; `HOOK_SOCK` set with no passport, and with the socket gone (chat
//! is an `id` tool: the shell's session checks run first, and so do the
//! binary's); the connection gate, which the shell skips for chat's own
//! forms (the binary hands those to the shell); the virtual desktop's
//! guard, which stops a form that is not an offline one; a `CODEX_`
//! setting; and `_CHAT_HELP_MANAGE`.

#![cfg(unix)]

mod common;

use common::parity::{
    case, nothing, ok, owner_and_port, owner_only, port_only, with_passport, Case, Compare, Rig,
    REFUSED,
};
use common::stub::Reply;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// A form: its first word, the words after it, and a tag for the rig.
struct Form {
    verb: &'static str,
    args: &'static [&'static str],
    tag: &'static str,
}

impl Form {
    /// `chat ...`: an `id` tool, and the shell skips the connection gate.
    fn chat_family(&self) -> bool {
        self.verb == "chat"
    }

    /// The virtual desktop's guard lets it by (`_k2_offline_verb`): the
    /// first word is help's, or a help flag is a word of it.
    fn offline(&self) -> bool {
        ["help", "--help", "-h"].contains(&self.verb)
            || self.args.iter().any(|a| matches!(*a, "-h" | "--help"))
    }
}

fn rows(rig: &Rig, f: &Form) -> Vec<Case> {
    let port = rig.stub.port;
    let id = f.chat_family();
    let row = |name, replies, setup, code, requests| {
        case(name, f.args, &[], replies, setup, code, requests)
    };
    // With no port or no token: the page where the shell skips the gate,
    // else the gate's error.
    let gate = if id { 0 } else { 1 };
    let guard = if !f.offline() { 3 } else { gate };
    let mut cases = vec![
        row(
            "an owner's terminal: the page",
            vec![],
            owner_and_port,
            0,
            0,
        ),
        with(
            with_passport(port, &[("K2_HOOK_SOCK", "{sock}")]),
            row(
                "an agent on its cell socket: the page, nothing sent",
                vec![],
                nothing,
                0,
                0,
            ),
        ),
        with(
            with_passport(port, &[]),
            row("an agent over TCP: the page", vec![], nothing, 0, 0),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            row(
                "HOOK_SOCK set and no passport: chat's own forms are refused, exit 3",
                vec![],
                owner_and_port,
                if id { 3 } else { 0 },
                0,
            ),
        ),
        with(
            with_passport(port, &[("K2_HOOK_SOCK", "{nosock}")]),
            row(
                "the socket gone, the daemon knows the passport: the page",
                if id {
                    vec![ok(r#"{"workspace":"acme"}"#)]
                } else {
                    vec![]
                },
                nothing,
                0,
                usize::from(id),
            ),
        ),
        with(
            with_passport(port, &[("K2_HOOK_SOCK", "{nosock}")]),
            row(
                "the socket gone, the daemon refuses the passport: chat's stale lines, exit 3",
                if id {
                    vec![Reply::Answer(401, REFUSED.to_string())]
                } else {
                    vec![]
                },
                nothing,
                if id { 3 } else { 0 },
                usize::from(id),
            ),
        ),
        row("no token: the gate", vec![], port_only, gate, 0),
        row("no port: the gate", vec![], owner_only, gate, 0),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            row(
                "the virtual desktop's switch and no passport",
                vec![],
                owner_and_port,
                guard,
                0,
            ),
        ),
        with(
            with_passport(
                port,
                &[("K2_HOOK_SOCK", "{sock}"), ("CODEX_THREAD_ID", "t1")],
            ),
            Case {
                // An id tool: the shell looks for a shared Codex server.
                compare: if id { Compare::Shell } else { Compare::Exact },
                ..row("a CODEX_ setting", vec![], nothing, 0, 0)
            },
        ),
    ];
    if f.verb == "chat" || f.args.contains(&"chat") {
        for v in ["0", "", "1", "yes"] {
            cases.push(with(
                vec![("_CHAT_HELP_MANAGE", v.to_string())],
                row(
                    "_CHAT_HELP_MANAGE: the manage piece only while empty or 1",
                    vec![],
                    owner_and_port,
                    0,
                    0,
                ),
            ));
        }
    }
    cases
}

fn check(forms: &[Form]) {
    for f in forms {
        let rig = Rig::new(f.verb, f.tag);
        for c in rows(&rig, f) {
            rig.check(&c);
        }
    }
}

#[test]
fn the_deprecated_tier() {
    check(&[
        Form {
            verb: "help",
            args: &["deprecated"],
            tag: "dep",
        },
        Form {
            verb: "-h",
            args: &["--deprecated"],
            tag: "dep2",
        },
        Form {
            verb: "help-deprecated",
            args: &[],
            tag: "dep3",
        },
    ]);
}

#[test]
fn chat_s_usage() {
    check(&[
        Form {
            verb: "chat",
            args: &[],
            tag: "bare",
        },
        Form {
            verb: "chat",
            args: &["--help"],
            tag: "flag",
        },
        Form {
            verb: "chat",
            args: &["help"],
            tag: "word",
        },
        Form {
            verb: "help",
            args: &["chat"],
            tag: "chat",
        },
    ]);
}
