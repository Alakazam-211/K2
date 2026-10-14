//! Widget authorship: the `zen_widget_authors` table (migration 0164;
//! contract §3.5; PRD §4.2, §4.3, BG3). C0 → B2.
//!
//! `(widget, live bundle hash) → maker` decides a widget's data rights.
//! The first signer of a hash wins; only the person's claim ("It's mine")
//! overwrites a stamp. Built-ins (`k2:<name>`) are K2's and need no row.
//! The newest 20 rows per widget are kept.

use rusqlite::Connection;

use super::{not_built, DataError, MadeBy};

/// Rows kept per widget.
pub const KEEP_PER_WIDGET: usize = 20;

/// Why a stamp was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampSource {
    New,
    Sign,
    Claim,
    Import,
}

impl StampSource {
    pub fn as_str(self) -> &'static str {
        match self {
            StampSource::New => "new",
            StampSource::Sign => "sign",
            StampSource::Claim => "claim",
            StampSource::Import => "import",
        }
    }
}

/// Who presented the request that stamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    Owner,
    Agent,
    Shell,
}

impl CallerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CallerKind::Owner => "owner",
            CallerKind::Agent => "agent",
            CallerKind::Shell => "shell",
        }
    }
}

/// What a stamp did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampOutcome {
    Stamped,
    /// The hash already had a maker; a sign never changes it.
    Kept { existing: MadeBy },
}

/// Stamp `(widget, hash)` with `author` unless it already has a maker.
/// `author` is `Owner` or `Agent`; `K2`/`Unclaimed` are refused.
pub fn stamp(
    _conn: &Connection,
    _widget: &str,
    _hash: &str,
    _author: &MadeBy,
    _source: StampSource,
    _caller: CallerKind,
    _now: i64,
) -> Result<StampOutcome, DataError> {
    Err(not_built())
}

/// The person's "It's mine": stamp `(widget, hash)` `owner`, overwriting.
pub fn claim(_conn: &Connection, _widget: &str, _hash: &str, _now: i64) -> Result<(), DataError> {
    Err(not_built())
}

/// The maker of `(widget, hash)`: `k2:*` → K2; no row → Unclaimed; an agent
/// whose workspace is gone → Unclaimed.
pub fn maker_of(_conn: &Connection, _widget: &str, _hash: &str) -> Result<MadeBy, DataError> {
    Err(not_built())
}
