//! `k2 study`, `k2 study list` and `k2 study --help` with a passport:
//! the shell asks the daemon whether permission cards are on, and so does
//! the binary ([`k5_cli::pages::LIST_WITH_PASSPORT`]). Each probe row
//! ([`common::cards::rows`]) on `cli/k2` and the binary: the same
//! request, the same page. The list as JSON (`study --json`, `study list
//! --json`) asks the same way and is the binary's too: its titles are
//! [`k5_cli::pages::STUDY_TITLES`].

#![cfg(unix)]

mod common;

use common::parity::*;

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

#[test]
fn the_list_as_json_with_a_passport() {
    let rig = Rig::new("study", "json");
    // native: study --json
    for args in [&["--json"][..], &["list", "--json"], &["--json", "list"]] {
        for c in common::cards::rows(&rig, args, 0, 0) {
            rig.check(&c);
        }
    }
}

#[test]
fn the_list_as_json_without_a_passport() {
    let rig = Rig::new("study", "json-plain");
    for c in [
        // native: study --json
        case("the list", &["--json"], &[], vec![], owner_and_port, 0, 0),
        case("list", &["list", "--json"], &[], vec![], nothing, 0, 0),
        case("an empty topic", &["", "--json"], &[], vec![], nothing, 0, 0),
        case(
            "the virtual desktop's guard",
            &["--json"],
            &[("K2_NO_OWNER_FALLBACK", "1")],
            vec![],
            nothing,
            0,
            0,
        ),
    ] {
        rig.check(&c);
        rig.binary(&c, false);
        assert!(
            !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
            "{}: the binary handed the line to the shell",
            c.name
        );
    }
}

#[test]
fn the_list_and_usage_read_the_daemon_itself_without_python3() {
    let rig = Rig::new("study", "list-nopy");
    for args in [
        &[][..],
        &["list"],
        &["--help"],
        &["-h"],
        &["errors", "--help"],
        &["--json"],
        &["list", "--json"],
    ] {
        common::cards::check_without_python3(&rig, args, 0, 0);
    }
}
