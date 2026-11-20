//! `k5 compute`: the family's one entry. Its row in [`crate::route::TABLE`]
//! is [`WORDS`], [`OWNER`] and [`VERBS`]; [`run`] hands each native verb to
//! the side that answers it:
//!
//! - [`reads`]: the verbs that only read (`nodes`, `ls`, `info`, `queue`,
//!   `jobs`, `get`, `status`, `grants`, `receipt`, `events`, `stats`,
//!   `usage`, `logs`, `preview` and `placement` with no change, and
//!   `rightsize`).
//! - [`writes`]: the verbs that write, wait or move files (`cancel`,
//!   `wait`, `fetch` and `grant`).
//!
//! Every other verb, and `compute` alone, is the shell's. The modules below
//! are shared: [`pyrt`] (the Python operations the shell's readers use),
//! [`render`] (`_compute_render`), [`lines`] (where the shell's Python
//! lines are) and `reads::call` (`_compute_call`).

pub mod events;
pub mod lines;
pub mod logs;
pub mod pyrt;
pub mod reads;
pub mod render;
pub mod status;
pub mod writes;

use crate::exit::Exit;
use crate::route::{Owner, Verb};
use std::ffi::OsString;

/// The router arm's words.
pub const WORDS: &[&str] = &["compute"];
/// The shell's but for [`VERBS`].
pub const OWNER: Owner = Owner::Bash;
/// One verb row per arm of `cmd_compute` (`cli/src/compute-command.sh`), so
/// each can turn back alone.
pub const VERBS: &[Verb] = &[
    Verb {
        words: &["nodes", "ls"],
        owner: Owner::Native,
    },
    Verb {
        words: &["info"],
        owner: Owner::Native,
    },
    Verb {
        words: &["queue"],
        owner: Owner::Native,
    },
    Verb {
        words: &["jobs"],
        owner: Owner::Native,
    },
    Verb {
        words: &["get", "status"],
        owner: Owner::Native,
    },
    Verb {
        words: &["grants"],
        owner: Owner::Native,
    },
    Verb {
        words: &["receipt"],
        owner: Owner::Native,
    },
    Verb {
        words: &["events"],
        owner: Owner::Native,
    },
    Verb {
        words: &["stats"],
        owner: Owner::Native,
    },
    Verb {
        words: &["usage"],
        owner: Owner::Native,
    },
    Verb {
        words: &["logs"],
        owner: Owner::Native,
    },
    Verb {
        words: &["preview"],
        owner: Owner::Native,
    },
    Verb {
        words: &["placement"],
        owner: Owner::Native,
    },
    Verb {
        words: &["rightsize"],
        owner: Owner::Native,
    },
    Verb {
        words: &["cancel"],
        owner: Owner::Native,
    },
    Verb {
        words: &["wait"],
        owner: Owner::Native,
    },
    Verb {
        words: &["fetch"],
        owner: Owner::Native,
    },
    Verb {
        words: &["grant"],
        owner: Owner::Native,
    },
];

/// Whether this family's code answers `verb` (a second word).
pub fn answers(verb: &str) -> bool {
    reads::ANSWERS.contains(&verb) || writes::ANSWERS.contains(&verb)
}

/// Runs `args` (the whole command line after `argv[0]`). `None`: the
/// shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let verb = args.get(1).and_then(|w| w.to_str())?;
    if reads::ANSWERS.contains(&verb) {
        return reads::run(args);
    }
    writes::run(args)
}
