//! Daily snapshots of `gardens.sqlite` (contract §2.3; PRD §5.8, BG7).
//! C0 → B3.
//!
//! `~/.k2/stores/.snapshots/gardens-YYYY-MM-DD.sqlite` via `VACUUM INTO`
//! (0600), once a day, skipped when nothing changed; 7 daily + 4 weekly
//! kept. Pre-op snapshots (`…THHMMSS-<op>.sqlite`) before a space delete,
//! import or move, kept 14 days / at most 10. `export --at` reads only the
//! dated files, read-only.

use std::path::{Path, PathBuf};

use super::{not_built, DataError};

/// The folder under `~/.k2/stores/`.
pub const DIR_NAME: &str = ".snapshots";

/// Why a pre-op snapshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreOp {
    Delete,
    Import,
    Move,
}

impl PreOp {
    pub fn as_str(self) -> &'static str {
        match self {
            PreOp::Delete => "delete",
            PreOp::Import => "import",
            PreOp::Move => "move",
        }
    }
}

/// One dated snapshot file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// `YYYY-MM-DD` (local date).
    pub date: String,
    pub path: PathBuf,
}

/// Take today's snapshot unless one exists or nothing changed since the
/// last. Answers the file written, if any.
pub fn take_daily(_store: &Path, _dir: &Path, _today: &str) -> Result<Option<PathBuf>, DataError> {
    Err(not_built())
}

/// A pre-op snapshot. Answers its file name.
pub fn take_pre_op(_store: &Path, _dir: &Path, _op: PreOp, _stamp: &str) -> Result<String, DataError> {
    Err(not_built())
}

/// Pure: which dated snapshots to keep (7 newest + the newest of each of
/// the 4 ISO weeks before them). `dates` are `YYYY-MM-DD`.
pub fn keep_set(_dates: &[String]) -> Vec<String> {
    Vec::new()
}

/// The newest dated snapshot on or before `date`, or `no_snapshot`.
pub fn open_at(_dir: &Path, _date: &str) -> Result<Snapshot, DataError> {
    Err(not_built())
}
