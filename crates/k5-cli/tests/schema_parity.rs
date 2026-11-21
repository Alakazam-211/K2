//! `k5 --schema` and `k5 schema` ([`k5_cli::families::schema`]): the
//! manifest the binary joins at build time is the heredoc in `cli/k2`,
//! byte for byte, and `cli/k2` and the binary, run as people run them
//! against one logging stub daemon ([`common::parity`]), send nothing and
//! print the same bytes with the same exit.
//!
//! The rows: an owner's terminal; an agent's, on its cell socket and over
//! TCP; `HOOK_SOCK` set with no passport, and with the socket gone (the
//! tool is an open one: no check runs); no token and no port (the shell
//! skips the connection gate for both words); the virtual desktop's
//! guard, which stops `schema` but not `--schema` or a form with a help
//! flag; and a `CODEX_` setting. Either word with one help flag after it
//! is the family's own line (`text::FAMILY_FORMS`: the manifest, as the
//! shell prints it); a form with a help flag and other words is the
//! shell's (the front hands it over), compared as such.

#![cfg(unix)]

mod common;

use common::parity::{
    case, nothing, owner_and_port, owner_only, port_only, with_passport, Case, Compare, Rig,
};

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// `cmd_schema`'s heredoc in `cli/k2`: the lines after its
/// `    cat <<'EOF'` up to the `EOF` line.
fn heredoc() -> String {
    let script = std::fs::read_to_string(common::parity::cli_k2()).expect("read cli/k2");
    let start = script
        .find("\ncmd_schema() {\n")
        .expect("cli/k2 defines cmd_schema");
    let rest = &script[start..];
    let open = rest
        .find("\n    cat <<'EOF'\n")
        .expect("cmd_schema's heredoc")
        + "\n    cat <<'EOF'\n".len();
    let body = &rest[open..];
    let end = body.find("\nEOF\n").expect("the heredoc's end") + 1;
    body[..end].to_string()
}

#[test]
fn the_joined_manifest_is_cli_k2_s() {
    let ours = k5_cli::families::schema::BODY;
    let theirs = heredoc();
    assert!(theirs.len() > 100_000, "a short heredoc: {}", theirs.len());
    if ours != theirs {
        let at = ours
            .bytes()
            .zip(theirs.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(ours.len().min(theirs.len()));
        let from = at.saturating_sub(80);
        panic!(
            "the joined manifest differs from cli/k2's at byte {at}\nours:   {:?}\ncli/k2: {:?}",
            &ours[from..(at + 80).min(ours.len())],
            &theirs[from..(at + 80).min(theirs.len())]
        );
    }
}

/// A form: its first word, the words after it, and a tag for the rig.
struct Form {
    verb: &'static str,
    args: &'static [&'static str],
    tag: &'static str,
}

impl Form {
    /// The virtual desktop's guard lets it by (`_k2_offline_verb`).
    fn offline(&self) -> bool {
        self.verb == "--schema" || self.help()
    }

    /// A help word is in it.
    fn help(&self) -> bool {
        self.args.iter().any(|a| matches!(*a, "-h" | "--help"))
    }

    /// The front hands the line to the shell: a help word, and the line is
    /// not one help flag alone (a family form).
    fn handed(&self) -> bool {
        self.help() && !matches!(self.args, ["--help" | "-h"])
    }
}

fn rows(f: &Form) -> Vec<Case> {
    let compare = if f.handed() {
        Compare::Shell
    } else {
        Compare::Exact
    };
    let row = |name, setup, code| Case {
        compare,
        ..case(name, f.args, &[], vec![], setup, code, 0)
    };
    vec![
        row("an owner's terminal: the manifest", owner_and_port, 0),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            row("an agent on its cell socket: nothing sent", nothing, 0),
        ),
        with(
            with_passport(&[]),
            row("an agent over TCP: nothing sent", nothing, 0),
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
        row("no token: the gate is skipped", port_only, 0),
        row("no port: the gate is skipped", owner_only, 0),
        row("neither: the gate is skipped", nothing, 0),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            row(
                "the virtual desktop's switch and no passport",
                owner_and_port,
                if f.offline() { 0 } else { 3 },
            ),
        ),
        with(
            with_passport(&[("K2_NO_OWNER_FALLBACK", "1")]),
            row("the virtual desktop's switch with a passport", nothing, 0),
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}"), ("CODEX_THREAD_ID", "t1")]),
            row("a CODEX_ setting: an open tool", nothing, 0),
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
fn dash_dash_schema() {
    check(&[
        // covers: --schema
        Form {
            verb: "--schema",
            args: &[],
            tag: "bare",
        },
        Form {
            verb: "--schema",
            args: &["json"],
            tag: "json",
        },
        // native: --schema --help
        Form {
            verb: "--schema",
            args: &["--help"],
            tag: "help",
        },
        Form {
            verb: "--schema",
            args: &["-h"],
            tag: "help-h",
        },
        Form {
            verb: "--schema",
            args: &["--help", "x"],
            tag: "help-and-word",
        },
    ]);
}

#[test]
fn the_word_schema() {
    check(&[
        Form {
            verb: "schema",
            args: &[],
            tag: "bare",
        },
        Form {
            verb: "schema",
            args: &["json", "mail"],
            tag: "json",
        },
        Form {
            verb: "schema",
            args: &["-h"],
            tag: "help",
        },
        Form {
            verb: "schema",
            args: &["--help"],
            tag: "help-long",
        },
    ]);
}
