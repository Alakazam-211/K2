//! `k2`, `k2 help` and `k2 --help`: the daily tier of the help
//! (`cmd_help_v2_daily`), which the binary prints itself
//! ([`k5_cli::pages::DAILY_NATIVE`]). With a passport the shell asks the
//! daemon whether permission cards are on, and so does the binary: each
//! probe row ([`common::cards::help_rows`]) on `cli/k2` and the binary,
//! the same request and the same page. Without one nothing is asked, and
//! the connection gate wants a port and a token, as for any help.

#![cfg(unix)]

mod common;

use common::parity::{case, dead_port, nothing, owner_and_port, owner_only, port_only, Case, Rig};

/// Each form as the rig runs it: its first word (none for `k2` alone)
/// and a tag for the rig's folder.
const FORMS: [(&str, &str); 3] = [("", "bare"), ("help", "word"), ("--help", "flag")];

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

#[test]
fn the_tier_with_a_passport() {
    for (verb, tag) in FORMS {
        let rig = Rig::new(verb, tag);
        for c in common::cards::help_rows(&rig, &[]) {
            rig.check(&c);
        }
    }
}

#[test]
fn the_tier_without_a_passport() {
    for (verb, tag) in FORMS {
        let rig = Rig::new(verb, tag);
        let row = |name, setup, code| case(name, &[], &[], vec![], setup, code, 0);
        let cases = vec![
            row(
                "an owner's terminal, no daemon: the tier, nothing asked",
                owner_and_port,
                0,
            ),
            with(
                vec![("K2_PORT", dead_port())],
                row("K2_PORT and the owner token on disk", owner_only, 0),
            ),
            with(
                vec![("K2_HOOK_SOCK", "{sock}".to_string())],
                row(
                    "HOOK_SOCK set and no passport: the tier, nothing asked",
                    owner_and_port,
                    0,
                ),
            ),
            row("no token: the connection gate, exit 1", port_only, 1),
            row("no port: the connection gate, exit 1", owner_only, 1),
            row("nothing at all: the connection gate, exit 1", nothing, 1),
            with(
                vec![("K2_NO_OWNER_FALLBACK", "1".to_string())],
                row(
                    "the virtual desktop's switch and no passport: the gate, exit 1",
                    owner_and_port,
                    1,
                ),
            ),
        ];
        for c in &cases {
            rig.check(c);
        }
    }
}

#[test]
fn the_tier_reads_the_daemon_itself_without_python3() {
    for (verb, tag) in FORMS {
        let rig = Rig::new(verb, tag);
        common::cards::check_without_python3(&rig, &[], 0, 0);
    }
}
