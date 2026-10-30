//! Deleting docs: every kit row of a doc (or of every doc in a space) in **one** transaction.
//!
//! Erased per doc: its `paths` entry and every table in [`super::DOC_TABLES`]: `docs`,
//! `snapshots`, `updates`, `clients`, `versions`, `approvals`, `sync` (the disk replica),
//! `doc_keys` (its hash key), the history tables (`changes`, `reverts`, `revert_index`,
//! `revert_carry`, `revert_stats`, `authored`, `change_clocks`, `provenance`) and the
//! tier-0 tables (`blob_versions`, `blob_sync`, `blob_approvals`). A blob doc's versions
//! each held a reference on their blob in `blobs`: those references are released (the
//! blob's grace starts then), so [`crate::blob::BlobStore::gc`] removes a blob nothing else
//! references once the grace runs out.
//!
//! Left alone: `meta` (doc ids are never reused), the host's table (`host`, opaque to the
//! kit: erasing a deleted doc's or space's rows there is the host's job), and a space's blob
//! key (`blob_spaces`; [`crate::blob::BlobStore::purge_space_key`] destroys it and every blob
//! of the space at once).
//!
//! A new per-doc table goes in [`super::DOC_TABLES`] (v0.9.7: one list for the delete, the
//! rewrite and `tables_holding`). `tables_holding` (tests only) panics on a table in the
//! file that is in neither that list nor [`super::SHARED_TABLES`], so a forgotten one fails
//! the delete tests, and a rewrite refuses it.

use redb::{ReadableTable, WriteTransaction};

use super::{blob, dec_meta, DocId, DocKey, DocStore, Erase, StoreError, DOCS, DOC_TABLES, PATHS};

/// Erase every kit row of doc `id` in `w`; returns its key.
fn delete_in(w: &WriteTransaction, id: DocId, now: u64) -> Result<DocKey, StoreError> {
    let key = {
        let docs = w.open_table(DOCS)?;
        let m = docs.get(id)?.ok_or(StoreError::NoDoc(id))?;
        dec_meta(m.value())?.key
    };
    {
        let mut paths = w.open_table(PATHS)?;
        let k = key.index();
        if paths.get(k.as_str())?.map(|v| v.value()) == Some(id) {
            paths.remove(k.as_str())?;
        }
    }
    for (t, how) in DOC_TABLES {
        if *how == Erase::Range {
            t.erase(w, id)?;
        }
    }
    // the tier-0 tables: their blob references are released first
    blob::delete_rows(w, id, &key.space, now)?;
    Ok(key)
}

impl DocStore {
    /// Delete doc `id`: every kit table's rows for it, in one durable transaction (all or
    /// nothing). A blob doc's references on its blobs are released at `now_ms` (host
    /// clock), so blob GC removes the ones nothing else references after its grace. The
    /// host's table and the space's blob key are left alone (module docs). Returns the
    /// doc's key; [`StoreError::NoDoc`] when there is no such doc.
    ///
    /// The store doesn't know about loaded sessions: a host deletes through
    /// [`crate::session::SessionManager::delete_doc`], which closes them first.
    pub fn delete_doc(&self, id: DocId, now_ms: u64) -> Result<DocKey, StoreError> {
        let w = self.begin_write()?;
        let key = delete_in(&w, id, now_ms)?;
        w.commit()?;
        Ok(key)
    }

    /// Delete every doc in `space`, as [`DocStore::delete_doc`] does, in **one** durable
    /// transaction. Returns their keys (empty when the space has none).
    pub fn delete_space(&self, space: &str, now_ms: u64) -> Result<Vec<DocKey>, StoreError> {
        let w = self.begin_write()?;
        let ids: Vec<DocId> = {
            let prefix = format!("{space}\u{0}");
            let paths = w.open_table(PATHS)?;
            let docs = w.open_table(DOCS)?;
            let mut ids = Vec::new();
            for kv in paths.range(prefix.as_str()..)? {
                let (k, v) = kv?;
                if !k.value().starts_with(&prefix) {
                    break;
                }
                let id = v.value();
                // the index joins space and path with NUL: check the doc's own key
                if let Some(m) = docs.get(id)? {
                    if dec_meta(m.value())?.key.space == space {
                        ids.push(id);
                    }
                }
            }
            ids
        };
        let mut keys = Vec::with_capacity(ids.len());
        for id in ids {
            keys.push(delete_in(&w, id, now_ms)?);
        }
        w.commit()?;
        Ok(keys)
    }

    /// Every table holding a row of doc `id` (tests only: after a delete it must be empty).
    /// Enumerates the store's **actual** tables and panics on one in neither
    /// [`super::DOC_TABLES`] nor [`super::SHARED_TABLES`], so a per-doc table added later
    /// without being listed (and so erased by `delete_doc`) fails the tests. Blob
    /// references (`blobs`, per space) are checked by the caller.
    #[cfg(test)]
    pub(crate) fn tables_holding(&self, id: DocId) -> Result<Vec<&'static str>, StoreError> {
        use redb::{MultimapTableHandle, TableHandle};

        let r = self.begin_read()?;
        let multimap: Vec<String> = r
            .list_multimap_tables()?
            .map(|t| t.name().to_owned())
            .collect();
        assert!(
            multimap.is_empty(),
            "multimap tables {multimap:?}: classify them in tables_holding and erase them in delete_doc"
        );
        let names: Vec<String> = r.list_tables()?.map(|t| t.name().to_owned()).collect();
        let mut out = Vec::new();
        for name in names {
            let (n, held): (&'static str, bool) = if name == "paths" {
                let mut found = false;
                for kv in r.open_table(PATHS)?.iter()? {
                    found |= kv?.1.value() == id;
                }
                ("paths", found)
            } else if super::SHARED_TABLES.contains(&name.as_str()) {
                continue; // not per doc
            } else if let Some((t, _)) = DOC_TABLES.iter().find(|(t, _)| t.name() == name) {
                (t.name(), t.holds(&r, id)?)
            } else {
                panic!(
                    "table {name:?} is not classified: list it in store::DOC_TABLES (erased by \
                     DocStore::delete_doc) or store::SHARED_TABLES"
                )
            };
            if held {
                out.push(n);
            }
        }
        out.sort_unstable();
        Ok(out)
    }
}
