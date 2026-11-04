//! What a bridge call gives back: an answer or a refusal, each with what it was about (the board
//! crate's [`On`]: the kind of file, the path the host gave, the guard, the mode).

use crate::bug::HostBug;
use k2_mpc_base::errors::{ErrorReport, Value as KValue};
use k2_mpc_board::On;

/// A call's answer: its value as the kit's value (through [`crate::value_out`]), and the file it
/// was about.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    /// Which file, by which guard, in which mode.
    pub on: On,
    /// The answer.
    pub value: KValue,
    /// True when it was a dry run: the save went nowhere and the file is as it was.
    pub dry_run: bool,
}

/// A call's refusal, and the file it was about. Before the file was read, `on.version` is `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    /// Which file, by which guard, in which mode.
    pub on: On,
    /// Why.
    pub why: Why,
}

/// Why a call was refused: a row of the kit's, or a host bug, which has no row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// The kit's report: its id, exit, message, next step and details.
    Report(ErrorReport),
    /// What the board crate never gives: the host's to report as its own failure.
    HostBug(HostBug),
}

impl Why {
    /// The exit its row gives, by the kit's table; `None` for a host bug, which has no row, so the
    /// host gives its own.
    pub fn exit(&self) -> Option<u8> {
        match self {
            Why::Report(r) => Some(r.exit()),
            Why::HostBug(_) => None,
        }
    }
}

impl From<ErrorReport> for Why {
    fn from(r: ErrorReport) -> Why {
        Why::Report(r)
    }
}

impl From<HostBug> for Why {
    fn from(b: HostBug) -> Why {
        Why::HostBug(b)
    }
}

impl From<Result<ErrorReport, HostBug>> for Why {
    /// A report the bridge built, or the host bug that stopped it.
    fn from(r: Result<ErrorReport, HostBug>) -> Why {
        match r {
            Ok(r) => Why::Report(r),
            Err(b) => Why::HostBug(b),
        }
    }
}
