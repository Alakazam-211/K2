//! `k2 study permissions` (text and `--json`) with a passport: a topic
//! only while permission cards are on, which the shell asks the daemon,
//! and so does the binary ([`k5_cli::pages::PERMISSIONS_WITH_PASSPORT`]).
//! On, the page (exit 0); off, the shell's unknown-topic error and usage
//! on stderr (exit 2), which the binary prints itself rather than let the
//! shell ask again. Each probe row ([`common::cards::rows`]) on `cli/k2`
//! and the binary: the same request, the same output.

#![cfg(unix)]

mod common;

use common::parity::{case, owner_and_port, Case, Rig};

#[test]
fn the_cards_page_with_a_passport() {
    let rig = Rig::new("study", "perm");
    for args in [&["permissions"][..], &["permissions", "--json"]] {
        for c in common::cards::rows(&rig, args, 0, 2) {
            rig.check(&c);
        }
    }
}

#[test]
fn without_a_passport_it_is_the_shell_s_error() {
    let rig = Rig::new("study", "noperm");
    let c = Case {
        env: vec![("K2_HOOK_SOCK", "{sock}".to_string())],
        ..case(
            "no passport: an unknown topic, nothing asked",
            &["permissions"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        )
    };
    rig.check(&c);
}
