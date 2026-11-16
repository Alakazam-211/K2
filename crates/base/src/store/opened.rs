//! The version a person's app opened a doc's file at (`note_opened`): one record per doc, in
//! slot [`OPENED`] of the doc's sync table (`sync` for a live doc, `blob_sync` for a blob
//! doc), not a table of its own. Every kit since v0.18.0 copies the sync tables through a
//! store rewrite, erases them with the doc and clears them on a tier change by the doc's whole
//! slot range, and reads only the slots it names: so a kit that doesn't know this slot carries
//! it through a rewrite (never refusing the store as a newer kit's), erases it with the doc,
//! and drops it when the doc changes tier. A session checks the record at every use (its
//! version must still be there) and drops one that fails.

use super::blob::BLOB_SYNC;
use super::enc::{R, W};
use super::{DocId, DocStore, StoreError, SYNC};
use crate::blob::BlobId;

/// The sync tables' slot that holds the record (0 synced, 1 a write-back in flight, 2 a live
/// doc's rendering).
pub(super) const OPENED: u8 = 3;

/// The record: the version the app opened, when it was noted, how, and for a blob doc that
/// version's blob. No content and no participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenedRec {
    /// A live doc's change seq, or a blob doc's version id.
    pub version: u64,
    /// Host clock, ms.
    pub at_ms: u64,
    /// How it was set: 0 the file as it was, 1 a version the host named, 2 an outside save.
    pub via: u8,
    /// A blob doc's version's blob, checked at use.
    pub blob: Option<BlobId>,
}

fn enc(p: &OpenedRec) -> Vec<u8> {
    let mut w = W::default();
    w.u8(1)
        .u64(p.version)
        .u64(p.at_ms)
        .u8(p.via)
        .opt_bytes(p.blob.as_ref().map(|b| b.as_bytes().as_slice()));
    w.0
}

fn dec(b: &[u8]) -> Result<OpenedRec, StoreError> {
    let mut r = R::new(b, "opened");
    if r.u8()? != 1 {
        return Err(StoreError::Corrupt("opened record version"));
    }
    let version = r.u64()?;
    let at_ms = r.u64()?;
    let via = r.u8()?;
    let blob = match r.opt_bytes()? {
        Some(b) => Some(BlobId::from_slice(&b).ok_or(StoreError::Corrupt("blob id"))?),
        None => None,
    };
    r.end()?;
    Ok(OpenedRec {
        version,
        at_ms,
        via,
        blob,
    })
}

impl DocStore {
    /// Doc `id`'s record, if any (`blob_doc`: in `blob_sync`, else in `sync`).
    pub(crate) fn opened_rec(
        &self,
        id: DocId,
        blob_doc: bool,
    ) -> Result<Option<OpenedRec>, StoreError> {
        let r = self.begin_read()?;
        let t = match blob_doc {
            true => r.open_table(BLOB_SYNC)?,
            false => r.open_table(SYNC)?,
        };
        let v = t.get((id, OPENED))?;
        v.map(|v| dec(v.value())).transpose()
    }

    /// Set doc `id`'s record, or remove it (`None`).
    pub(crate) fn set_opened_rec(
        &self,
        id: DocId,
        blob_doc: bool,
        rec: Option<&OpenedRec>,
    ) -> Result<(), StoreError> {
        let w = self.begin_write()?;
        {
            let mut t = match blob_doc {
                true => w.open_table(BLOB_SYNC)?,
                false => w.open_table(SYNC)?,
            };
            match rec {
                Some(p) => {
                    t.insert((id, OPENED), enc(p).as_slice())?;
                }
                None => {
                    t.remove((id, OPENED))?;
                }
            }
        }
        w.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_round_trips() {
        for p in [
            OpenedRec {
                version: 7,
                at_ms: 1_000,
                via: 0,
                blob: None,
            },
            OpenedRec {
                version: u64::MAX,
                at_ms: 0,
                via: 2,
                blob: Some(BlobId::from_slice(&[9; 32]).unwrap()),
            },
        ] {
            assert_eq!(dec(&enc(&p)).unwrap(), p);
        }
        assert!(dec(&[2]).is_err(), "another record version is refused");
    }
}
