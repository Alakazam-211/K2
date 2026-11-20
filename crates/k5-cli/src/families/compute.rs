//! `k5 compute`: the shell CLI's `cmd_compute` (`cli/src/compute-command.sh`).
//! The verbs made native so far are [`writes`]' (the verbs that write,
//! wait or move files); every other verb, `compute` alone and its help
//! stay the shell's.

pub mod writes;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["compute"];
/// The shell CLI answers `compute` alone and every verb not in [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// The verbs the binary answers, one row each, so each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[
    crate::route::Verb {
        words: &["cancel"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["wait"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["fetch"],
        owner: crate::route::Owner::Native,
    },
];

/// A command line the route sends here: [`writes::run`].
pub fn run(args: &[std::ffi::OsString]) -> Option<crate::exit::Exit> {
    writes::run(args)
}
