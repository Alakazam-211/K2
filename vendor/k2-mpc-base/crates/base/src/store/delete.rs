//! Deleting docs: every kit row of a doc (or of every doc in a space) in **one** transaction.
//!
//! Erased per doc: `paths`, `docs`, `snapshots`, `updates`, `clients`, `versions`,
//! `approvals`, `sync` (the disk replica), `doc_keys` (its hash key), the history tables (`changes`, `reverts`,
//! `revert_index`, `revert_carry`, `revert_stats`) and the tier-0 tables (`blob_versions`,
//! `blob_sync`). A blob doc's versions each held a reference on their blob in `blobs`: those
//! references are released (the blob's grace starts then), so [`crate::blob::BlobStore::gc`]
//! removes a blob nothing else references once the grace runs out.
//!
//! Left alone: `meta` (doc ids are never reused), the host's table (`host`, opaque to the
//! kit: erasing a deleted doc's or space's rows there is the host's job), and a space's blob
//! key (`blob_spaces`; [`crate::blob::BlobStore::purge_space_key`] destroys it and every blob
//! of the space at once).
//!
//! A new per-doc table must be erased here **and** listed in `tables_holding` (tests only),
//! which panics on a table it doesn't know, so a forgotten one fails the delete tests.

use redb::{ReadableTable, WriteTransaction};

use super::{
    blob, dec_meta, history, DocId, DocKey, DocStore, StoreError, APPROVALS, CLIENTS, DOCS,
    DOC_KEYS, PATHS, SNAPSHOTS, SYNC, UPDATES, VERSIONS,
};

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
    w.open_table(DOCS)?.remove(id)?;
    w.open_table(SNAPSHOTS)?.remove(id)?;
    w.open_table(DOC_KEYS)?.remove(id)?;
    for t in [UPDATES, VERSIONS, APPROVALS] {
        w.open_table(t)?
            .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?;
    }
    w.open_table(CLIENTS)?
        .retain_in((id, 0)..=(id, u64::MAX), |_, _| false)?;
    w.open_table(SYNC)?
        .retain_in((id, 0)..=(id, u8::MAX), |_, _| false)?;
    history::delete_rows(w, id)?;
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
    /// Enumerates the store's **actual** tables and panics on one it doesn't classify, so
    /// a per-doc table added later without being erased by `delete_doc` fails the tests.
    /// Blob references (`blobs`, per space) are checked by the caller.
    #[cfg(test)]
    pub(crate) fn tables_holding(&self, id: DocId) -> Result<Vec<&'static str>, StoreError> {
        use super::blob::{BLOB_SYNC, BLOB_VERSIONS};
        use super::history::{
            CHANGES, CHANGE_CLOCKS, REVERTS, REVERT_CARRY, REVERT_INDEX, REVERT_STATS,
        };
        use redb::{MultimapTableHandle, TableDefinition, TableHandle};

        fn pair<V: redb::Value + 'static>(
            r: &redb::ReadTransaction,
            t: TableDefinition<(u64, u64), V>,
            id: DocId,
        ) -> Result<bool, StoreError> {
            Ok(r.open_table(t)?
                .range((id, 0)..=(id, u64::MAX))?
                .next()
                .is_some())
        }
        fn slot(
            r: &redb::ReadTransaction,
            t: TableDefinition<(u64, u8), &[u8]>,
            id: DocId,
        ) -> Result<bool, StoreError> {
            Ok(r.open_table(t)?
                .range((id, 0)..=(id, u8::MAX))?
                .next()
                .is_some())
        }
        fn one(
            r: &redb::ReadTransaction,
            t: TableDefinition<u64, &[u8]>,
            id: DocId,
        ) -> Result<bool, StoreError> {
            Ok(r.open_table(t)?.get(id)?.is_some())
        }

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
            let (n, held): (&'static str, bool) = match name.as_str() {
                // not per doc
                "meta" | "host" | "blob_spaces" | "blobs" => continue,
                "paths" => {
                    let mut found = false;
                    for kv in r.open_table(PATHS)?.iter()? {
                        found |= kv?.1.value() == id;
                    }
                    ("paths", found)
                }
                "docs" => ("docs", one(&r, DOCS, id)?),
                "snapshots" => ("snapshots", one(&r, SNAPSHOTS, id)?),
                "doc_keys" => ("doc_keys", one(&r, DOC_KEYS, id)?),
                "revert_stats" => ("revert_stats", one(&r, REVERT_STATS, id)?),
                "updates" => ("updates", pair(&r, UPDATES, id)?),
                "clients" => ("clients", pair(&r, CLIENTS, id)?),
                "versions" => ("versions", pair(&r, VERSIONS, id)?),
                "approvals" => ("approvals", pair(&r, APPROVALS, id)?),
                "changes" => ("changes", pair(&r, CHANGES, id)?),
                "reverts" => ("reverts", pair(&r, REVERTS, id)?),
                "revert_index" => ("revert_index", pair(&r, REVERT_INDEX, id)?),
                "revert_carry" => ("revert_carry", pair(&r, REVERT_CARRY, id)?),
                "change_clocks" => (
                    "change_clocks",
                    r.open_table(CHANGE_CLOCKS)?
                        .range((id, 0, 0)..=(id, u64::MAX, u64::MAX))?
                        .next()
                        .is_some(),
                ),
                "blob_versions" => ("blob_versions", pair(&r, BLOB_VERSIONS, id)?),
                "sync" => ("sync", slot(&r, SYNC, id)?),
                "blob_sync" => ("blob_sync", slot(&r, BLOB_SYNC, id)?),
                other => panic!(
                    "table {other:?} is not classified: erase it in DocStore::delete_doc and \
                     list it in tables_holding"
                ),
            };
            if held {
                out.push(n);
            }
        }
        out.sort_unstable();
        Ok(out)
    }
}
