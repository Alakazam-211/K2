//! A first word the router has no arm for (its `*)` arm), which the
//! binary answers itself ([`k5_cli::pages::UNKNOWN_NATIVE`]): a retired
//! verb's line on stderr and exit 1, or `Unknown command: <word>` on
//! stderr, the daily tier on stdout and exit 1. With a passport the
//! shell's `cmd_help` asks the daemon whether agents may report, then
//! whether permission cards are on, and so does the binary: each probe
//! row ([`common::cards::daily_rows`]) on
//! `cli/k2` and the binary through the shared rig, the same request and
//! the same bytes. Without one nothing is asked; the connection gate and
//! the virtual desktop's guard (a word that is no help) stop it first.

#![cfg(unix)]

mod common;

use common::cards::{AGENTS_ON, ON};
use common::parity::{
    case, nothing, ok, owner_and_port, owner_only, port_only, with_passport, Case, Compare, Rig,
};

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

/// The unknown words, as the rig runs them: the first word, the words
/// after it, a tag.
const FORMS: [(&str, &[&str], &str); 4] = [
    ("nope", &[], "nope"),
    ("--nope", &["x", "--help"], "flag"),
    ("agents", &["nope"], "agents"),
    ("Help", &["--json"], "upper"),
];

#[test]
fn the_unknown_command_with_a_passport() {
    for (verb, args, tag) in FORMS {
        let rig = Rig::new(verb, tag);
        for c in common::cards::daily_rows(&rig, args, 1, 1) {
            rig.check(&c);
        }
    }
}

#[test]
fn the_unknown_command_reads_the_daemon_itself_without_python3() {
    for (verb, args, tag) in FORMS {
        let rig = Rig::new(verb, tag);
        common::cards::check_daily_without_python3(&rig, args, 1, 1);
    }
}

/// The rows with no passport, for a form whose bash exit is `code` where
/// it gets past the gates.
fn plain_rows(args: &'static [&'static str], code: i32) -> Vec<Case> {
    let row = |name, setup, code| case(name, args, &[], vec![], setup, code, 0);
    vec![
        row("an owner's terminal: nothing asked", owner_and_port, code),
        with(
            vec![("K2_HOOK_SOCK", "{sock}".to_string())],
            row(
                "HOOK_SOCK set and no passport: nothing asked",
                owner_and_port,
                code,
            ),
        ),
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            row("HOOK_SOCK gone and no passport", owner_and_port, code),
        ),
        row("no token: the connection gate, exit 1", port_only, 1),
        row("no port: the connection gate, exit 1", owner_only, 1),
        row("nothing at all: the connection gate, exit 1", nothing, 1),
        with(
            vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
            row(
                "the virtual desktop's switch and no passport: its guard, exit 3",
                owner_and_port,
                3,
            ),
        ),
    ]
}

#[test]
fn the_unknown_command_without_a_passport() {
    for (verb, args, tag) in FORMS {
        let rig = Rig::new(verb, tag);
        // A help flag in the line makes it an offline verb to the guard:
        // then the gate's error, as for help.
        let offline = args.iter().any(|a| matches!(*a, "-h" | "--help"));
        for mut c in plain_rows(args, 1) {
            if offline && c.code == 3 {
                c.code = 1;
            }
            rig.check(&c);
        }
    }
}

/// Every retired verb whose first word the router has no arm for: its
/// line, exit 1, nothing asked; alone and with a word after it.
#[test]
fn every_retired_verb() {
    let words: Vec<&'static str> = k5_cli::retired::TABLE
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| {
            let first = k.split(' ').next().unwrap();
            !k5_cli::route::TABLE.iter().any(|n| n.names(first))
        })
        .collect();
    assert!(words.len() > 20, "only {} retired verbs", words.len());
    for (n, key) in words.iter().enumerate() {
        let mut parts = key.split(' ');
        let verb = parts.next().unwrap();
        let rest: Vec<&'static str> = parts.collect();
        let rig = Rig::new(verb, &format!("r{n}"));
        let alone: &'static [&'static str] = Box::leak(rest.clone().into_boxed_slice());
        let mut more = rest.clone();
        more.push("x");
        let more: &'static [&'static str] = Box::leak(more.into_boxed_slice());
        for args in [alone, more] {
            rig.check(&case(
                "an owner's terminal: the line, exit 1",
                args,
                &[],
                vec![],
                owner_and_port,
                1,
                0,
            ));
        }
        // One verb through every row, and an agent's terminal.
        if n == 0 {
            for c in plain_rows(alone, 1) {
                rig.check(&c);
            }
            rig.check(&with(
                with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
                case(
                    "an agent on its cell socket: the line, nothing asked",
                    alone,
                    &[],
                    vec![],
                    nothing,
                    1,
                    0,
                ),
            ));
        }
    }
}

/// A word the router has no arm for whose tool is an `id` tool (`fs`):
/// with `HOOK_SOCK` set the shell's session checks run first, so the
/// binary hands it over; without, it is the unknown command.
#[test]
fn a_word_whose_tool_is_an_id_tool() {
    let rig = Rig::new("fs", "id");
    let cases = vec![
        with(
            vec![("K2_HOOK_SOCK", "{nosock}".to_string())],
            Case {
                compare: Compare::Shell,
                ..case(
                    "HOOK_SOCK and no passport: the shell refuses it, exit 3",
                    &[],
                    &[],
                    vec![],
                    owner_and_port,
                    3,
                    0,
                )
            },
        ),
        with(
            with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
            Case {
                compare: Compare::Shell,
                ..case(
                    "a passport and its socket: the shell's checks pass, then the tier",
                    &[],
                    &[],
                    vec![ok(AGENTS_ON), ok(ON)],
                    nothing,
                    1,
                    2,
                )
            },
        ),
        case(
            "an owner's terminal: the unknown command",
            &[],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ];
    for c in &cases {
        rig.check(c);
    }
}
