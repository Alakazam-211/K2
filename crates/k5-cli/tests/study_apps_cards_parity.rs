//! `k2 study apps` and `k2 study skins` (text and `--json`) with a
//! passport: the shell asks the daemon whether permission cards are on
//! for the page's `#if-cards` lines, and so does the binary
//! ([`k5_cli::pages::APPS_WITH_PASSPORT`]). Each probe row
//! ([`common::cards::rows`]) on `cli/k2` and the binary: the same
//! request, the same page.

#![cfg(unix)]

mod common;

use common::parity::Rig;

#[test]
fn the_apps_page_with_a_passport() {
    let rig = Rig::new("study", "apps");
    for args in [&["apps"][..], &["apps", "--json"]] {
        for c in common::cards::rows(&rig, args, 0, 0) {
            rig.check(&c);
        }
    }
}

#[test]
fn the_skins_alias_with_a_passport() {
    let rig = Rig::new("study", "skins");
    for args in [&["skins"][..], &["--json", "skins"]] {
        for c in common::cards::rows(&rig, args, 0, 0) {
            rig.check(&c);
        }
    }
}
