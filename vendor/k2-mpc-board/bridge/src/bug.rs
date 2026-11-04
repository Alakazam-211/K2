//! What the board crate can't give: a host bug, typed, never a row a caller sees.

use std::fmt;

/// Something the bridge was given that the board crate doesn't produce: a host's bug, or a board
/// crate newer than this bridge. It is never mapped to a row, because any row would be a plausible
/// but wrong answer. A host reports it as its own failure.
///
/// `non_exhaustive`: a `match` on it needs a wildcard arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostBug {
    /// A conflict the board crate never gives: a change to a person, or an add of the settings.
    ImpossibleConflict {
        /// `changed` or `exists`.
        clash: &'static str,
        /// `person` or `settings`.
        target: &'static str,
    },
    /// A number in a board value that isn't finite. The format refuses one on the way in.
    NotFinite {
        /// Where, as a JSON pointer.
        at: String,
    },
    /// A refusal code this bridge has no row for.
    Unmapped {
        /// The code's stable string.
        code: &'static str,
    },
    /// A refusal that lacks what its row's message names.
    MissingFact {
        /// The row.
        row: &'static str,
        /// The key.
        key: String,
    },
    /// A row the kit doesn't hold.
    NoRow {
        /// The row's id.
        id: &'static str,
    },
    /// An error kind this bridge doesn't know.
    UnknownError {
        /// What it was, as the board crate prints it.
        what: String,
    },
}

impl fmt::Display for HostBug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostBug::ImpossibleConflict { clash, target } => write!(
                f,
                "host bug: the board crate gives no `{clash}` conflict on the {target}"
            ),
            HostBug::NotFinite { at } => {
                write!(f, "host bug: a number at {at} isn't finite")
            }
            HostBug::Unmapped { code } => write!(f, "host bug: no row for refusal code {code}"),
            HostBug::MissingFact { row, key } => {
                write!(f, "host bug: the refusal lacks {key}, which {row} names")
            }
            HostBug::NoRow { id } => write!(f, "host bug: the kit has no row {id}"),
            HostBug::UnknownError { what } => write!(f, "host bug: an unknown error: {what}"),
        }
    }
}

impl std::error::Error for HostBug {}
