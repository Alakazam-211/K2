//! The host-given turn of each change ([`crate::turn`]).
//!
//! Two additive per-doc tables, **without a format bump** (the terms of `client_kinds`,
//! `doc_blob_refs` and `live_copies`; `docs/SESSIONS.md`, "Turns"):
//!
//! - `change_turns`: `(doc, seq)` → the turn of a live doc's change;
//! - `blob_turns`: `(doc, version)` → the turn of a tier-0 version.
//!
//! Each is created lazily, by the first change or version that has a turn, and written in that
//! change's own transaction, so a row never disagrees with its change. A change with no turn
//! (a person's, a shell's, an outside save, anything before this kit) has no row. A store
//! without the table, or without a change's row, reads as "no turn".
//!
//! **An older kit** reads and writes such a store as before (its changes get no rows), refuses
//! a rewrite, and so a purge or an erasure, before changing anything
//! (`StoreError::NewerData`, `MPC-E502`; never a silent drop), and leaves the rows of a doc it
//! deletes, which every open here removes (`sweep_orphans`).
//!
//! **Erasure** removes the rows of the changes (and versions) the erased participant made or
//! that were made on their behalf: a turn links changes into one agent run. A content purge
//! leaves them, as it leaves authors and times. A row holds no doc content.

use redb::{ReadableTable, TableDefinition, TableHandle, WriteTransaction};

use super::enc::{R, W};
use super::{DocId, Seq, StoreError};
use crate::turn::Turn;

/// `(doc, seq)` → a live change's turn (record version 1).
pub(super) const CHANGE_TURNS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("change_turns");
/// `(doc, version)` → a tier-0 version's turn (record version 1).
pub(super) const BLOB_TURNS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("blob_turns");

pub(super) fn enc_turn(t: &Turn) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).str(t.as_str());
    w.0
}

/// `None` for a record this kit can't read (another version, or not a turn): read as no turn.
pub(super) fn dec_turn(b: &[u8]) -> Option<Turn> {
    let mut r = R::new(b, "turn");
    if r.u8().ok()? != 1 {
        return None;
    }
    let s = r.str().ok()?;
    r.end().ok()?;
    Turn::new(&s).ok()
}

/// Write `(id, n)`'s turn into `table` in `w`, the change's own transaction (creating the
/// table with its first row). No turn writes nothing.
pub(super) fn put_in(
    w: &WriteTransaction,
    table: TableDefinition<(u64, u64), &[u8]>,
    id: DocId,
    n: u64,
    turn: Option<&Turn>,
) -> Result<(), StoreError> {
    if let Some(t) = turn {
        w.open_table(table)?
            .insert((id, n), enc_turn(t).as_slice())?;
    }
    Ok(())
}

/// A turns table open for reading, `None` when the store has none (no change had a turn).
pub(super) type ReadTurns = Option<redb::ReadOnlyTable<(u64, u64), &'static [u8]>>;

/// `table` in a read, or `None` without it.
pub(super) fn open_read(
    r: &redb::ReadTransaction,
    table: TableDefinition<'static, (u64, u64), &'static [u8]>,
) -> Result<ReadTurns, StoreError> {
    match r.open_table(table) {
        Ok(t) => Ok(Some(t)),
        Err(redb::TableError::TableDoesNotExist(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The turn of `(id, n)` in a table [`open_read`] gave.
pub(super) fn get(t: &ReadTurns, id: DocId, n: u64) -> Result<Option<Turn>, StoreError> {
    let Some(t) = t else {
        return Ok(None);
    };
    Ok(t.get((id, n))?.and_then(|v| dec_turn(v.value())))
}

/// Remove doc `id`'s rows in `table` for `ns`, in `w` (a deleted version's; an erasure's, by
/// [`erase_in`]; an erasure's verbs, `verbs.rs`). Returns how many went.
pub(super) fn remove_in(
    w: &WriteTransaction,
    table: TableDefinition<(u64, u64), &[u8]>,
    id: DocId,
    ns: &[u64],
) -> Result<u64, StoreError> {
    if ns.is_empty() || !super::history::has_table(w, table.name())? {
        return Ok(0);
    }
    let mut t = w.open_table(table)?;
    let mut n = 0;
    for &k in ns {
        if t.remove((id, k))?.is_some() {
            n += 1;
        }
    }
    Ok(n)
}

/// Erasure: remove doc `id`'s rows in `table` for `ns` (the changes or versions the erased
/// participant made, or that were made on their behalf), in `w`. Returns how many went.
pub(super) fn erase_in(
    w: &WriteTransaction,
    table: TableDefinition<(u64, u64), &[u8]>,
    id: DocId,
    ns: &[u64],
) -> Result<u64, StoreError> {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::contract_mutant()
        == Some(crate::differential::ContractMutant::TurnNotClearedOnErase)
    {
        return Ok(0);
    }
    remove_in(w, table, id, ns)
}

/// Doc `id`'s changes (live) that `pid` made or that were made on `pid`'s behalf, from the
/// change summaries in `w`: what an erasure clears the turns and verbs of (none to read when
/// the store has neither table).
pub(super) fn changes_of(
    w: &WriteTransaction,
    id: DocId,
    pid: &str,
) -> Result<Vec<Seq>, StoreError> {
    if !super::history::has_table(w, CHANGE_TURNS.name())?
        && !super::history::has_table(w, super::verbs::CHANGE_VERBS.name())?
    {
        return Ok(Vec::new());
    }
    let changes = w.open_table(super::history::CHANGES)?;
    let mut out = Vec::new();
    for kv in changes.range((id, 0)..=(id, u64::MAX))? {
        let (k, v) = kv?;
        let (info, _) = super::history::dec_change(k.value().1, v.value(), false)?;
        if info.author == pid || info.on_behalf_of.as_deref() == Some(pid) {
            out.push(k.value().1);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_round_trips_and_another_version_reads_as_none() {
        let t = Turn::new("t:9c41e07ab2d35f68.12").unwrap();
        assert_eq!(dec_turn(&enc_turn(&t)), Some(t));
        let mut other = enc_turn(&t);
        other[0] = 2;
        assert_eq!(dec_turn(&other), None);
        let mut long = enc_turn(&t);
        long.push(0);
        assert_eq!(dec_turn(&long), None, "a longer record is another layout");
        let mut w = W::default();
        w.u8(1).str("not a turn");
        assert_eq!(
            dec_turn(&w.0),
            None,
            "a stored string that isn't a turn is none"
        );
    }
}
