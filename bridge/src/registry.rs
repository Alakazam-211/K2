//! The board's file verbs as the kit's file registry, for `capabilities` and `explain`: built
//! once from the board crate's own lists and the kit's rows, so a verb, an argument or a row
//! added on either side shows here with no second list to keep.
//!
//! - The verbs and their order are [`verbs::ARGS`]'s.
//! - A verb in [`verbs::WRITE_VERBS`], `apply` and `inverse` change the file (`inverse` applies
//!   the undo group it plans); the others read it.
//! - Its usage line is the parser's ([`crate::parse::usage`]).
//! - Its exits are 0, the exit of every kit row marked `@file` that names it, and the exits of
//!   the parser's usage refusals ([`USAGE`]), which any verb's command line can give.
//! - The parity rows it serves are [`parity::ROWS`]' rows whose way names it.

use k2_mpc_base::errors::{entries, lookup};
use k2_mpc_base::parity::{FileVerb, Registry, VerbKind};
use k2_mpc_board::parity::{self, Way};
use k2_mpc_board::verbs;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// The board's file verbs, as the kit's registry: what a host passes to the kit's `run_meta`
/// for `capabilities` and `explain` on a board file. Built on first use, then kept.
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let file_verbs: Vec<FileVerb> = verbs::ARGS
            .iter()
            .map(|&(name, _)| file_verb(name))
            .collect();
        Registry::for_file(&file_verbs)
            .expect("check: the board's verb lists make a file registry the kit takes")
    })
}

/// Whether the verb changes the file.
pub fn kind_of(name: &str) -> VerbKind {
    if verbs::WRITE_VERBS.contains(&name) || name == "apply" || name == "inverse" {
        VerbKind::Edit
    } else {
        VerbKind::Read
    }
}

/// The kit's rows the parser refuses a command line with, for any verb: an unknown verb or flag,
/// and a missing or bad argument.
pub const USAGE: &[&str] = &["MPC-E120", "MPC-E122"];

/// Every exit the verb can end with: 0, the exit of each `@file` row that names it, and the
/// usage refusals'.
pub fn exits_of(name: &str) -> Vec<u8> {
    let mut exits = BTreeSet::from([0u8]);
    for e in entries() {
        if e.verbs.first() == Some(&"@file") && e.verbs[1..].contains(&name) {
            exits.insert(e.exit);
        }
    }
    for id in USAGE {
        if let Some(e) = lookup(id) {
            exits.insert(e.exit);
        }
    }
    exits.into_iter().collect()
}

/// The parity rows the verb serves, by their names.
pub fn serves_of(name: &str) -> Vec<&'static str> {
    parity::ROWS
        .iter()
        .filter(|r| matches!(r.way, Way::Verbs(vs) if vs.contains(&name)))
        .map(|r| r.name)
        .collect()
}

fn file_verb(name: &'static str) -> FileVerb {
    // the kit's file verb holds `'static` data; the registry is built once, so this is kept once
    let usage: &'static str = Box::leak(
        crate::parse::usage(name)
            .unwrap_or_default()
            .into_boxed_str(),
    );
    let exits: &'static [u8] = Box::leak(exits_of(name).into_boxed_slice());
    let serves: &'static [&'static str] = Box::leak(serves_of(name).into_boxed_slice());
    FileVerb::new(name, kind_of(name), usage, exits, serves)
}
