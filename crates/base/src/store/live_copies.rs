//! The conflict copies a live doc's session wrote (a keyed doc that isn't a table: an outside
//! save over the kit's own write keeps both), so `tick` can remove them after
//! `Config::conflict_copy_keep_ms`, as tier 0 removes the copies its blob versions record.
//!
//! One additive per-doc table, `live_copies`, **without a format bump** (the terms of
//! `client_kinds` and `doc_blob_refs`; `docs/SESSIONS.md`, "Outside saves on a keyed doc that
//! isn't a table"): created lazily by the first copy, keyed `(doc, version)` where `version` is
//! the outside save's version, holding the copy's record (its path relative to the file's
//! folder), the kept version and when it was written. A store without the table (or without a
//! doc's rows) has no recorded copies: nothing is removed. An older kit reads and writes such
//! a store as before (it never removes these copies: they stay, which loses nothing), refuses
//! a rewrite, and so a purge, before changing anything (`StoreError::NewerData`, `MPC-E502`),
//! and leaves the rows of a doc it deletes, which every open here removes.

use redb::{TableDefinition, TableHandle};

use super::enc::{R, W};
use super::{DocId, DocStore, StoreError, VersionId};

/// `(doc, the outside save's version)` → [`LiveCopyRow`] (record version 1).
pub(super) const LIVE_COPIES: TableDefinition<(u64, u64), &[u8]> =
    TableDefinition::new("live_copies");

/// A conflict copy a live doc's session wrote ([`DocStore::live_copies`]).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCopyRow {
    /// Its path relative to the file's folder (a plain file name when beside it), as tier 0
    /// records a copy.
    pub rec: String,
    /// The version holding the content the copy holds (what the save replaced).
    pub kept: VersionId,
    /// When it was written (host clock, ms).
    pub at_ms: u64,
}

fn enc_row(r: &LiveCopyRow) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1).str(&r.rec).u64(r.kept).u64(r.at_ms);
    w.0
}

/// `None` for a record this kit can't read (another version): that copy is never removed.
fn dec_row(b: &[u8]) -> Option<LiveCopyRow> {
    let mut r = R::new(b, "live copy");
    if r.u8().ok()? != 1 {
        return None;
    }
    let row = LiveCopyRow {
        rec: r.str().ok()?,
        kept: r.u64().ok()?,
        at_ms: r.u64().ok()?,
    };
    r.end().ok()?;
    Some(row)
}

impl DocStore {
    /// Record a conflict copy doc `id`'s session wrote for the outside save `version`, in one
    /// durable transaction (the table is created by the first).
    pub(crate) fn put_live_copy(
        &self,
        id: DocId,
        version: VersionId,
        row: &LiveCopyRow,
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        w.open_table(LIVE_COPIES)?
            .insert((id, version), enc_row(row).as_slice())?;
        w.commit()?;
        Ok(())
    }

    /// Doc `id`'s recorded conflict copies, by version, oldest first. Empty without the
    /// table; a record this kit can't read is left out (never removed).
    pub fn live_copies(&self, id: DocId) -> Result<Vec<(VersionId, LiveCopyRow)>, StoreError> {
        let r = self.begin_read()?;
        let t = match r.open_table(LIVE_COPIES) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for kv in t.range((id, 0)..=(id, u64::MAX))? {
            let (k, v) = kv?;
            if let Some(row) = dec_row(v.value()) {
                out.push((k.value().1, row));
            }
        }
        Ok(out)
    }

    /// Forget doc `id`'s copy record for `version` (the copy was removed, or is gone).
    pub(crate) fn delete_live_copy(&self, id: DocId, version: VersionId) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        if super::history::has_table(&w, LIVE_COPIES.name())? {
            w.open_table(LIVE_COPIES)?.remove((id, version))?;
        }
        w.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_round_trips_and_another_version_is_unread() {
        let row = LiveCopyRow {
            rec: "talk (conflict bot 2026-10-27 143001).deck".into(),
            kept: 7,
            at_ms: 1_793_111_401_000,
        };
        assert_eq!(dec_row(&enc_row(&row)), Some(row.clone()));
        let mut other = enc_row(&row);
        other[0] = 2;
        assert_eq!(dec_row(&other), None);
        let mut long = enc_row(&row);
        long.push(0);
        assert_eq!(dec_row(&long), None, "a longer record is another layout");
    }
}
