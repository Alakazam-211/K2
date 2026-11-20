//! The bare lines a family's bare row sends to the binary
//! ([`k5_cli::route::Verb::bare`]), against the shell CLI's on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code.
//!
//! - `compute` alone: the help page (`cmd_compute`'s `""` arm), after the
//!   virtual desktop's guard and the `id` tool's lines. The shell skips
//!   its connection gate for it, so it prints with no daemon too.
//! - `chat --json`: `--json` is no verb, so `cmd_chat` says so
//!   (`_chat_usage_err`, exit 2). `chat` alone stays the front's composed
//!   page (`help_tiers_parity.rs` and `chat_parity.rs` have it).
//! - The version (`version`, `--version`, `-v`), which the front answers
//!   by its own code with no row of the table in the way. With no daemon
//!   the two differ by the named difference `version-offline`
//!   (scripts/cli-parity.sh), so the rows here have one.
//!
//! Each row is the binary's own answer, not a hand-over that prints the
//! same: after the rig's check the binary runs the line once more and must
//! not have written the shell CLI out ([`answered_here`]). The check bites
//! on a line the binary hands over ([`the_native_check_bites`]).

#![cfg(unix)]

mod common;

use common::parity::*;

/// `c` on the rig, then the binary alone once more: it answered the line
/// itself, so it never wrote the shell CLI out to hand it over.
fn answered_here(rig: &Rig, c: &Case) {
    rig.check(c);
    rig.binary(c, false);
    assert!(
        !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
        "{}: the binary handed the line to the shell",
        c.name
    );
}

fn session(c: Case) -> Case {
    Case {
        env: with_passport(&[("K2_HOOK_SOCK", "{sock}")]),
        setup: port_only,
        ..c
    }
}

#[test]
fn compute_alone() {
    let rig = Rig::new("compute", "bare");
    for c in [
        // covers: compute
        case(
            "compute alone: the help page",
            &[],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "compute alone, no daemon: no connection gate",
            &[],
            &[],
            vec![],
            nothing,
            0,
            0,
        ),
        session(case(
            "compute alone in a session",
            &[],
            &[],
            vec![],
            port_only,
            0,
            0,
        )),
        case(
            "compute alone: the virtual desktop's guard still stands",
            &[],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case(
            "compute alone: HOOK_SOCK set and no passport",
            &[],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

#[test]
fn chat_json() {
    let rig = Rig::new("chat", "bare-json");
    for c in [
        // covers: chat --json
        case(
            "chat --json: no verb",
            &["--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "chat --json, no daemon: the connection gate",
            &["--json"],
            &[],
            vec![],
            nothing,
            1,
            0,
        ),
        session(case(
            "chat --json in a session",
            &["--json"],
            &[],
            vec![],
            port_only,
            2,
            0,
        )),
        case(
            "chat --json: HOOK_SOCK set and no passport",
            &["--json"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

#[test]
fn the_version() {
    for (word, tag) in [("version", "word"), ("--version", "flag"), ("-v", "v")] {
        let rig = Rig::new(word, tag);
        // covers: version
        let c = case("the version", &[], &[], vec![], owner_and_port, 0, 0);
        answered_here(&rig, &c);
    }
}

/// The check bites: a line the binary hands to the shell (`<noun> help`
/// for a family whose bare line moved by its own row: `help` is no verb
/// row) leaves the shell CLI written out. The family is read from the
/// table, so a lane that moves one moves this check with it.
#[test]
fn the_native_check_bites() {
    use k5_cli::front::{route, Route};
    use k5_cli::route::{Owner, TABLE};
    let noun = TABLE
        .iter()
        .find(|n| {
            let help: [std::ffi::OsString; 2] = [n.words[0].into(), "help".into()];
            n.owner == Owner::Bash && n.bare_row().is_some() && route(&help) == Route::Fallthrough
        })
        .expect("a family with a bare row of its own and its help the shell's");
    let rig = Rig::new(noun.words[0], "bite");
    let c = case(
        "help: the shell's",
        &["help"],
        &[],
        vec![],
        owner_and_port,
        0,
        0,
    );
    rig.binary(&c, false);
    assert!(
        k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
        "the binary answered a line it should hand over"
    );
}
