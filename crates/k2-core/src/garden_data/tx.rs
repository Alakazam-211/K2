//! Chunked transactions (contract §3.5, §4.5; PRD §5.3.1, BG4). C0 → B2.
//!
//! `begin → parts (≤ 64 ops, ≤ 60 KiB each) → commit`, ≤ 1,000 ops and
//! ≤ 1 MiB per commit. Parts are staged here in daemon memory, invisible to
//! readers, bound to their caller and one space, and dropped after
//! `TX_IDLE_SECS` with no part. Grants are checked per part by the route
//! and again at commit. Pure: the clock is passed in.

use std::collections::HashMap;

use super::backend::Op;
use super::{not_built, DataError, Space};

/// Who may use a transaction id. Anyone else presenting it gets `bad_tx`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TxOwner {
    /// A widget call: the placement and the live hash it ran under.
    Placement { garden: String, placement: String, hash: String },
    /// An agent (or K2 shell tab), by workspace.
    Agent { project_id: String },
}

/// One open transaction's running totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxStatus {
    pub ops: usize,
    pub bytes: usize,
    pub expires_at: i64,
}

#[allow(dead_code)]
struct Staged {
    owner: TxOwner,
    space: Space,
    ops: Vec<Op>,
    bytes: usize,
    last_at: i64,
}

/// Every open transaction on this daemon.
#[derive(Default)]
pub struct TxTable {
    #[allow(dead_code)]
    open: HashMap<String, Staged>,
}

impl TxTable {
    pub fn new() -> TxTable {
        TxTable::default()
    }

    /// Open one. Answers `(txId, expiresAt)`; `txId` is 32 hex chars
    /// (128 random bits). Caps: `MAX_TX_PER_PLACEMENT`, `MAX_TX_PER_AGENT`.
    pub fn begin(&mut self, _owner: TxOwner, _space: Space, _now: i64) -> Result<(String, i64), DataError> {
        Err(not_built())
    }

    /// Stage one part (the route already checked keys, sizes and grants).
    pub fn add(&mut self, _tx_id: &str, _owner: &TxOwner, _ops: Vec<Op>, _now: i64) -> Result<TxStatus, DataError> {
        Err(not_built())
    }

    /// Remove and hand back the staged ops for commit.
    pub fn take(&mut self, _tx_id: &str, _owner: &TxOwner, _now: i64) -> Result<(Space, Vec<Op>), DataError> {
        Err(not_built())
    }

    /// Drop one. True when it existed and belonged to `owner`.
    pub fn abort(&mut self, _tx_id: &str, _owner: &TxOwner) -> bool {
        false
    }

    /// Drop everything idle past `TX_IDLE_SECS`. Answers how many.
    pub fn expire(&mut self, _now: i64) -> usize {
        0
    }
}
