//! The pack verb that made each live change (`docs/SESSIONS.md`,
//! "Verbs").
//!
//! One additive per-doc table, **without a format bump**, on the terms of `change_turns`
//! (`turns.rs`):
//!
//! - `change_verbs`: `(doc, seq)` → the id of the pack verb that made a live change (`shape
//!   move`, `replace-section`).
//!
//! It is created lazily, by the first change a pack verb made, and written in that change's own
//! transaction, so a row never disagrees with its change. A change no pack verb made (a raw
//! write, a kit tool, an outside save, an undo, a redo, a revert, a restore, anything before
//! this kit) has no row, and a store without the table, or without a change's row, reads as "no
//! verb". Tier 0 has no pack verb that records a version, so there is no blob table.
//!
//! **An older kit** reads and writes such a store as before (its changes get no rows), refuses a
//! rewrite, and so a purge or an erasure, before changing anything (`StoreError::NewerData`,
//! `MPC-E502`; never a silent drop), and leaves the rows of a doc it deletes, which every open
//! here removes (`sweep_orphans`).
//!
//! **Erasure** removes the rows of the changes the erased participant made or that were made on
//! their behalf, as it removes their turns (`turns.rs`). A content purge leaves them, as it
//! leaves authors and times. A row holds no doc content: only the pack's own verb name.

use redb::{TableDefinition, WriteTransaction};

use super::enc::{R, W};
use super::turns::ReadTurns;
use super::{DocId, StoreError};

/// `(doc, seq)` → the pack verb of a live change (record version 1).
pub(super) const CHANGE_VERBS: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("change_verbs");

/// At most this many bytes of a verb id are stored (a pack's verb names are a word or two).
pub(super) const VERB_MAX: usize = 64;

pub(super) fn enc_verb(v: &str) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).str(v);
    w.0
}

/// `None` for a record this kit can't read (another version, an empty or over-long id): read as
/// no verb.
pub(super) fn dec_verb(b: &[u8]) -> Option<String> {
    let mut r = R::new(b, "verb");
    if r.u8().ok()? != 1 {
        return None;
    }
    let s = r.str().ok()?;
    r.end().ok()?;
    (!s.is_empty() && s.len() <= VERB_MAX).then_some(s)
}

/// Write `(id, seq)`'s verb in `w`, the change's own transaction (creating the table with its
/// first row). No verb, or an id past [`VERB_MAX`] (never a pack's), writes nothing.
pub(super) fn put_in(
    w: &WriteTransaction,
    id: DocId,
    seq: u64,
    verb: Option<&str>,
) -> Result<(), StoreError> {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::contract_mutant()
        == Some(crate::differential::ContractMutant::VerbNotRecorded)
    {
        return Ok(());
    }
    if let Some(v) = verb.filter(|v| !v.is_empty() && v.len() <= VERB_MAX) {
        w.open_table(CHANGE_VERBS)?
            .insert((id, seq), enc_verb(v).as_slice())?;
    }
    Ok(())
}

/// The verb of `(id, seq)` in a table [`super::turns::open_read`] gave for [`CHANGE_VERBS`].
pub(super) fn get(t: &ReadTurns, id: DocId, seq: u64) -> Result<Option<String>, StoreError> {
    let Some(t) = t else {
        return Ok(None);
    };
    Ok(t.get((id, seq))?.and_then(|v| dec_verb(v.value())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_round_trips_and_another_version_reads_as_none() {
        assert_eq!(
            dec_verb(&enc_verb("shape move")).as_deref(),
            Some("shape move")
        );
        let mut other = enc_verb("shape move");
        other[0] = 2;
        assert_eq!(dec_verb(&other), None);
        let mut long = enc_verb("shape move");
        long.push(0);
        assert_eq!(dec_verb(&long), None, "a longer record is another layout");
        assert_eq!(dec_verb(&enc_verb("")), None, "an empty id is none");
        assert_eq!(
            dec_verb(&enc_verb(&"x".repeat(VERB_MAX + 1))),
            None,
            "an id past the cap is none"
        );
    }
}
