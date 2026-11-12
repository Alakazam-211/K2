//! A live doc's **carrier** (K1; `docs/SESSIONS.md`, "The carrier (K1)"): for a pack that models
//! its format only in part and opts in ([`Pack::wants_carrier`]), the bytes the kit last read
//! from the doc's file or wrote to it. Rendering the doc splices its content into them
//! ([`Pack::encode_from`]), so every byte the pack doesn't model stays out of the engine and in
//! the file.
//!
//! - **Updated** on every read of the file that becomes the sync point (the open, an outside
//!   save taken in, a read of the file as last synced) and on every kit write (`write_back`):
//!   it is always the bytes the sync point names, never a rendering.
//! - **Kept** as a blob in the doc's space (`store/live_carrier.rs`: the row and its count). A
//!   carrier it replaces goes **at once** when nothing else references it (index and file, no
//!   GC grace), so no older carrier waits in the blob folder: a purge's write-back, which
//!   writes the purged doc, leaves no byte of the erased content in the kit's blobs.
//! - **Loaded** with the doc, and kept only if it still hashes to the sync point (an older
//!   kit's write, a crash between a write and its carrier, a moved doc: dropped, and the file's
//!   next read is the carrier).
//! - **Each version keeps its own** (the store's `version_carriers`): a version's file
//!   ([`Session::version_file`]: `file_at`, the conflict copies' comparisons) is that version
//!   spliced into it; a version with none is refused (`MPC-E208`), never `encode`d. A purge
//!   drops the carriers of the versions it erased ([`Session::purge_version_carriers`]).
//! - **A blob left with no reference goes at once** unless a put of it still waits for its
//!   writer's reference (`blob.rs`): another doc never finds bytes it just put gone.
//! - **Never leaves the kit** but to the pack's hook. Without a blob store it is kept in memory
//!   only, and taken from the file again after a reload.

use super::*;
use crate::blob::{BlobId, BlobStore};
use crate::store::{Released, VersionId};

/// The planted fault [`crate::differential::KeyedMutant::CarrierStale`].
pub(super) fn planted_carrier_stale() -> bool {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::keyed_mutant() == Some(crate::differential::KeyedMutant::CarrierStale) {
        return true;
    }
    false
}

/// The planted fault [`crate::differential::KeyedMutant::PurgeKeepsCarrier`].
fn planted_purge_keeps_carrier() -> bool {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::keyed_mutant()
        == Some(crate::differential::KeyedMutant::PurgeKeepsCarrier)
    {
        return true;
    }
    false
}

impl Session {
    /// `content` as file bytes: spliced into the carrier ([`Pack::encode_from`]) when the pack
    /// opts in and the session holds one, else [`Pack::encode`].
    pub(super) fn encode_live(
        &self,
        st: &State,
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>> {
        match &st.carrier {
            Some(c) if self.pack.wants_carrier() => Ok(self.pack.encode_from(c, content, codec)?),
            _ => Ok(self.pack.encode(content, codec)?),
        }
    }

    /// Whether `content`, rendered with `codec` onto the file's `bytes` (the carrier those bytes
    /// become, for a pack that opts in), is those bytes.
    pub(super) fn renders_as(
        &self,
        content: &[(String, Neutral)],
        bytes: &[u8],
        codec: &[u8],
    ) -> Result<bool> {
        let ours = match self.pack.wants_carrier() {
            true => self.pack.encode_from(bytes, content, codec)?,
            false => self.pack.encode(content, codec)?,
        };
        Ok(ours == bytes)
    }

    /// The doc's stored carrier, when the blob store is set (a load or an import): kept only if
    /// its bytes still hash to the stored sync point; otherwise the row goes (its count given
    /// back, the blob removed if nothing else holds it) and the file's next read is the carrier.
    /// Never fails: what can't be read is no carrier.
    pub(super) fn load_carrier(&self) {
        if !self.pack.wants_carrier() {
            return;
        }
        let Some(blobs) = self.blobs.get() else {
            return;
        };
        let Ok(mut st) = self.lock() else {
            return;
        };
        let st = &mut *st;
        if st.carrier.is_some() {
            return;
        }
        let Ok(store) = self.store(st).cloned() else {
            return;
        };
        let Ok(Some(row)) = store.live_carrier(self.doc_id) else {
            return;
        };
        let synced = store
            .sync_states(self.doc_id)
            .ok()
            .and_then(|(s, _)| s)
            .map(|s| s.hash);
        let space = st.key.space.clone();
        let held = store
            .blob_space(&space)
            .ok()
            .flatten()
            .filter(|k| k.no == row.no && row.size <= self.config.disk_read_max_bytes)
            .and_then(|_| {
                blobs
                    .read_to_vec(&space, &row.id, self.config.disk_read_max_bytes)
                    .ok()
            })
            .filter(|b| synced.as_deref() == Some(file_hash(&st.hash_key, b).as_slice()));
        let now = self.host.now_ms();
        match held {
            Some(b) => {
                st.carrier = Some(Arc::from(b));
                st.view_carrier = None;
            }
            None => {
                if let Ok(gone) = store.set_live_carrier(self.doc_id, None, 0, now) {
                    let _ = blobs.remove_unreferenced_now(&gone);
                }
            }
        }
        // versions an older kit's retention dropped give their carriers' counts back
        let _ = store.sweep_version_carriers(self.doc_id, now);
    }

    /// The file holds `bytes` at sync point `seq`, as the kit last read or wrote them: they
    /// become the carrier, and every version at `seq` takes them, in the blob store when there
    /// is one. A blob a carrier leaves with no reference goes at once (a put still waiting to
    /// reference it stays). Never fails: what can't be stored is reported
    /// ([`AuditEvent::DiskError`]) and kept in memory (the stored one, no longer the sync
    /// point's, is dropped at the next load).
    pub(super) fn keep_carrier(&self, st: &mut State, bytes: &[u8], seq: Seq, now: u64) {
        if !self.pack.wants_carrier() || st.carrier.as_deref() == Some(bytes) {
            return;
        }
        if let Some(blobs) = self.blobs.get() {
            match self.store_carrier(st, blobs, bytes, seq, now) {
                Ok(gone) if !planted_purge_keeps_carrier() => {
                    if let Err(e) = blobs.remove_unreferenced_now(&gone) {
                        self.disk_error(st, "removing the replaced carrier", e);
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    self.disk_error(st, "keeping the file's bytes (the carrier)", e);
                }
            }
        }
        st.carrier = Some(Arc::from(bytes));
        // the view index's derive reads the new carrier, decoded afresh
        st.view_carrier = None;
    }

    /// Put `bytes` in the doc's space and make them the stored carrier at `seq`. Returns the
    /// blobs the carriers they replace leave with no reference.
    fn store_carrier(
        &self,
        st: &State,
        blobs: &BlobStore,
        bytes: &[u8],
        seq: Seq,
        now: u64,
    ) -> std::result::Result<Vec<Released>, String> {
        let store = self.store(st).map_err(|e| e.to_string())?;
        let space = &st.key.space;
        let info = blobs.put_bytes(space, bytes).map_err(|e| e.to_string())?;
        let set = store.set_live_carrier(self.doc_id, Some((space, info.id, info.size)), seq, now);
        // the put no longer waits: the row references it, or it was given up
        blobs.settle(space, &info.id);
        set.map_err(|e| e.to_string())
    }

    /// Version `vid` (`v`) as file bytes. For a pack that keeps a carrier (unless `encode` is
    /// asked for): spliced into the version's carrier, `None` when it has none (a version cut
    /// before this build or by an older kit, one whose carrier a purge erased, or one whose
    /// blob is gone); otherwise the pack's encoding. With its own codec settings, else the
    /// doc's.
    pub(super) fn version_file(
        &self,
        st: &State,
        vid: VersionId,
        v: &crate::store::Version,
        encode: bool,
    ) -> Result<Option<Vec<u8>>> {
        let codec = v.codec.clone().unwrap_or_else(|| st.codec.clone());
        if encode || !self.pack.wants_carrier() {
            return Ok(Some(self.pack.encode(&v.content, &codec)?));
        }
        let store = self.store(st)?;
        let (Some(row), Some(blobs)) = (store.version_carrier(self.doc_id, vid)?, self.blobs.get())
        else {
            return Ok(None);
        };
        let space = &st.key.space;
        let here = store.blob_space(space)?.is_some_and(|k| k.no == row.no);
        let carrier = match here {
            true => blobs
                .read_to_vec(space, &row.id, self.config.disk_read_max_bytes)
                .ok(),
            false => None,
        };
        match carrier {
            Some(c) => Ok(Some(self.pack.encode_from(&c, &v.content, &codec)?)),
            None => Ok(None),
        }
    }

    /// A purge's version carriers (after its store commit): the carriers of the versions it
    /// erased (`whole`, or whose content it redacted) and of any version whose carrier holds
    /// a purged text (`needles`, read from the blob) are dropped, and the blobs nothing else
    /// references are removed at once (counted in `blobs_removed`; one still referenced, or a
    /// put still waiting, stays and is reported shared).
    pub(super) fn purge_version_carriers(
        &self,
        st: &State,
        whole: &crate::store::Pick,
        needles: &crate::store::Needles,
        now: u64,
        counts: &mut crate::store::PurgeCounts,
        not_reached: &mut Vec<NotReached>,
    ) -> Result<()> {
        let store = self.store(st)?.clone();
        let rows = store.version_carriers(self.doc_id)?;
        if rows.is_empty() {
            return Ok(());
        }
        let infos = store.version_infos(self.doc_id)?;
        let space = st.key.space.clone();
        let blobs = self.blobs.get();
        let mut hit: std::collections::HashMap<BlobId, bool> = std::collections::HashMap::new();
        let mut drop = Vec::new();
        for (vid, row) in rows {
            let erased = whole.has(vid)
                || infos
                    .iter()
                    .find(|i| i.id == vid)
                    .map_or(true, |i| !i.redacted.is_empty());
            let holds = *hit.entry(row.id).or_insert_with(|| {
                blobs.is_some_and(|b| {
                    b.read_to_vec(&space, &row.id, u64::MAX)
                        .is_ok_and(|bytes| needles.hits(&bytes))
                })
            });
            if erased || holds {
                drop.push(vid);
            }
        }
        let gone = store.drop_version_carriers(self.doc_id, &drop, now)?;
        if gone.is_empty() {
            return Ok(());
        }
        match blobs {
            Some(b) => {
                let (removed, _) = b.remove_unreferenced_now(&gone)?;
                counts.blobs_removed += removed;
                let left = gone.len() as u64 - removed;
                if left > 0 {
                    not_reached.push(NotReached::SharedBlobs(left));
                }
            }
            None => not_reached.push(NotReached::LeftForBlobGc(gone.len() as u64)),
        }
        Ok(())
    }
}
