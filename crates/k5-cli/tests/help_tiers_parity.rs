//! The help tiers the router's own arms print (`help-advanced`,
//! `help-internal`, `help-deprecated`) and `help <word>` with a page,
//! which the front prints while the help family's rows are the shell's
//! ([`k5_cli::families::help`]): alone they are pages; with words after
//! them (which the shell never reads) or a help flag, the binary hands
//! them to the shell. `cli/k2` and the binary, run as people run them
//! against one logging stub daemon ([`common::parity`]), must send
//! nothing and print the same bytes with the same exit.
//!
//! The rows: an owner's terminal; an agent's, on its cell socket and over
//! TCP; `HOOK_SOCK` set with no passport, and with the socket gone (the
//! tool is `help`, an open one: no check runs); the connection gate (no
//! token, no port: the binary hands those to the shell); the virtual
//! desktop's guard, which stops a tier word with no token, and the
//! connection gate where it lets `help` or a help flag by;
//! and a `CODEX_` setting, in a Codex tab K2 started and with no tab
//! marker (the shell's Codex tab gate: exit 3 but for help's words and a
//! help flag).

#![cfg(unix)]

mod common;

use common::parity::{
    case, nothing, owner_and_port, owner_only, port_only, with_passport, Case, Compare, Rig,
    PASSPORT,
};

/// The passport's session: what a Codex tab K2 started has in
/// `K2_CODEX_TAB_ISOLATED`.
fn session() -> &'static str {
    PASSPORT
        .split_once('.')
        .expect("a passport is session.secret")
        .0
}

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
    /// A help flag is a word of it.
    fn help_flag(&self) -> bool {
        self.args.iter().any(|a| matches!(*a, "-h" | "--help"))
    }

    /// The virtual desktop's guard lets it by (`_k2_offline_verb`).
    fn offline(&self) -> bool {
        self.verb == "help" || self.help_flag()
    }
}

fn rows(f: &Form) -> Vec<Case> {
    let page = if f.verb == "help" {
        f.args.len() == 1
    } else {
        f.args.is_empty()
    };
    let compare = if f.help_flag() || !page {
        Compare::Shell
    } else {
        Compare::Exact
    };
    let row = |name, setup, code| Case {
        compare,
        ..case(name, f.args, &[], vec![], setup, code, 0)
    };
    vec![
        row("an owner's terminal: the page", owner_and_port, 0),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            row(
                "an agent on its cell socket: the page, nothing sent",
                nothing,
                0,
            ),
        ),
        with(
            with_passport(&[]),
            row("an agent over TCP: the page", nothing, 0),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            row(
                "HOOK_SOCK set and no passport: an open tool",
                owner_and_port,
                0,
            ),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{nosock}")]),
            row("the socket gone: an open tool asks nothing", nothing, 0),
        ),
        row("no token: the gate", port_only, 1),
        row("no port: the gate", owner_only, 1),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            row(
                "the virtual desktop's switch and no passport",
                owner_and_port,
                // No token is read with the switch on: the guard, or the
                // connection gate where the guard lets the line by.
                if f.offline() { 1 } else { 3 },
            ),
        ),
        with(
            with_passport(&[("K2_NO_OWNER_FALLBACK", "1")]),
            row("the virtual desktop's switch with a passport", nothing, 0),
        ),
        // A passport inside a Codex tab K2 started: the tab marker names the
        // passport's session, so the shell's Codex tab gate lets it by.
        with(
            with_passport(&[
                ("K2_HOOK_SOCK", "{sock}"),
                ("CODEX_THREAD_ID", "t1"),
                ("K2_CODEX_TAB_ISOLATED", session()),
            ]),
            row(
                "a CODEX_ setting in a tab K2 started: an open tool",
                nothing,
                0,
            ),
        ),
        // No tab marker: the shell's Codex tab gate (_cli_codex_tab_gate)
        // lets help's words and a help flag by and refuses the rest, exit 3,
        // nothing sent.
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}"), ("CODEX_THREAD_ID", "t1")]),
            row(
                "a CODEX_ setting and no tab marker: the Codex tab gate",
                nothing,
                if f.offline() { 0 } else { 3 },
            ),
        ),
    ]
}

fn check(forms: &[Form]) {
    for f in forms {
        let rig = Rig::new(f.verb, f.tag);
        for c in rows(f) {
            rig.check(&c);
        }
    }
}

#[test]
fn help_advanced() {
    check(&[
        Form {
            verb: "help-advanced",
            args: &[],
            tag: "bare",
        },
        Form {
            verb: "help-advanced",
            args: &["msg", "--json", ""],
            tag: "words",
        },
        Form {
            verb: "help-advanced",
            args: &["x", "--help"],
            tag: "flag",
        },
    ]);
}

#[test]
fn help_internal() {
    check(&[
        Form {
            verb: "help-internal",
            args: &[],
            tag: "bare",
        },
        Form {
            verb: "help-internal",
            args: &["advanced", "x y"],
            tag: "words",
        },
        Form {
            verb: "help-internal",
            args: &["-h"],
            tag: "flag",
        },
    ]);
}

#[test]
fn help_deprecated() {
    check(&[
        Form {
            verb: "help-deprecated",
            args: &["chat"],
            tag: "words",
        },
        Form {
            verb: "help-deprecated",
            args: &["--deprecated", "-x"],
            tag: "words2",
        },
        Form {
            verb: "help-deprecated",
            args: &["--help"],
            tag: "flag",
        },
    ]);
}

#[test]
fn help_with_a_word() {
    check(&[
        // covers: help
        Form {
            verb: "help",
            args: &["advanced"],
            tag: "advanced",
        },
        Form {
            verb: "help",
            args: &["msg"],
            tag: "msg",
        },
        // `cmd_help` reads only its first word: the shell's, as is a
        // tier word with words after it.
        Form {
            verb: "help",
            args: &["internal", "x"],
            tag: "two",
        },
    ]);
}
