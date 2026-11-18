//! `k2 study`, `k2 study list` and `k2 study --help` with a passport:
//! the shell asks the daemon whether permission cards are on, and so does
//! the binary ([`k5_cli::pages::LIST_WITH_PASSPORT`]). Each probe row
//! ([`common::cards::rows`]) on `cli/k2` and the binary: the same
//! request, the same page.

#![cfg(unix)]

mod common;

use common::parity::Rig;

#[test]
fn the_list_with_a_passport() {
    let rig = Rig::new("study", "list");
    for args in [&[][..], &["list"]] {
        for c in common::cards::rows(&rig, args, 0, 0) {
            rig.check(&c);
        }
    }
}

#[test]
fn the_usage_with_a_passport() {
    let rig = Rig::new("study", "usage");
    for args in [&["--help"][..], &["-h"], &["errors", "--help"]] {
        for c in common::cards::rows(&rig, args, 0, 0) {
            rig.check(&c);
        }
    }
}
