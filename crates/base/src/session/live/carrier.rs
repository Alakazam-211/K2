//! A live doc's **carrier** (`docs/SESSIONS.md`, "The carrier"): for a pack that models
//! its format only in part and opts in ([`Pack::wants_carrier`]), the bytes the kit last read
//! from the doc's file or wrote to it. Rendering the doc splices its content into them
//! ([`Pack::encode_from`]), so every byte the pack doesn't model stays out of the engine and in
//! the file.
//!
//! - **Updated** on every read of the file that becomes the sync point (the open, an outside
//!   save taken in, a read of the file as last synced) and on every kit write (`write_back`):
//!   it is always the bytes the sync point names, never a rendering. An outside save is the
//!   carrier, with its pairing, before anything renders the doc that took it
//!   (`Session::settle_saved`).
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
//!
//! **The pairing** (`docs/SESSIONS.md`, "The carrier's pairing"): for a pack that mints its
//! keys ([`Pack::wants_pairing`]), the carrier's keys in the order the pack's decode yields its
//! units, made by [`Pack::pairing`] at every sync point from the content the file holds there
//! (the content written, or the disk replica) and kept beside the carrier, in memory
//! (`State::pairing`) and in the store (`carrier_pairings`), live and per version. Every
//! rendering hands it to [`Pack::encode_from_paired`]; a pairing that doesn't fit is refused
//! ([`SessionError::PairingMismatch`]), never spliced by guess. Keys only: nothing to erase.

use super::*;
use crate::blob::{BlobId, BlobStore};
use crate::session::pack::{Pairing, SpliceError};
use crate::store::{Released, VersionId, LIVE_SLOT};

/// The planted fault [`crate::differential::KeyedMutant::SyncedContentStale`].
fn planted_synced_content_stale() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::SyncedContentStale)
    )
}

/// The planted fault [`crate::differential::KeyedMutant::SpliceWithoutMatch`].
fn planted_splice_without_match() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::SpliceWithoutMatch)
    )
}

/// The planted fault [`crate::differential::KeyedMutant::CarrierStale`].
pub(super) fn planted_carrier_stale() -> bool {
    plant_on!(
        crate::differential::keyed_mutant() == Some(crate::differential::KeyedMutant::CarrierStale)
    )
}

/// The planted fault [`crate::differential::KeyedMutant::SettleBeforeCarrier`].
pub(super) fn planted_settle_before_carrier() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::SettleBeforeCarrier)
    )
}

/// The planted fault [`crate::differential::KeyedMutant::PurgeKeepsCarrier`].
fn planted_purge_keeps_carrier() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::PurgeKeepsCarrier)
    )
}

impl Session {
    /// `content` as file bytes: spliced into the carrier ([`Pack::encode_from`]; with the
    /// carrier's pairing, [`Pack::encode_from_paired`]) when the pack opts in and the session
    /// holds one, else [`Pack::encode`]. A pack that pairs and has no pairing beside the carrier
    /// is refused ([`SessionError::PairingMismatch`]), never spliced by guess.
    pub(super) fn encode_live(
        &self,
        st: &State,
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>> {
        match &st.carrier {
            Some(c) if self.pack.wants_carrier() => {
                match (self.pack.wants_pairing(), &st.pairing) {
                    (false, _) => Ok(self.pack.encode_from(c, content, codec)?),
                    (true, Some((_, p))) => self.splice_paired(c, p, content, codec),
                    (true, None) => Err(SessionError::PairingMismatch {
                        reason: "no pairing is kept with the file's bytes".into(),
                    }),
                }
            }
            _ => Ok(self.pack.encode(content, codec)?),
        }
    }

    /// [`Pack::encode_from_paired`]: a pairing that doesn't fit is
    /// [`SessionError::PairingMismatch`], with no fallback to another hook.
    fn splice_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>> {
        match self
            .pack
            .encode_from_paired(carrier, pairing, content, codec)
        {
            Err(SpliceError::PairingMismatch(_)) if planted_splice_without_match() => {
                Ok(self.pack.encode_from(carrier, content, codec)?)
            }
            r => Ok(r?),
        }
    }

    /// Whether `content`, rendered with `codec` onto the file's `bytes` (the carrier those bytes
    /// become, for a pack that opts in, paired with `content`'s own keys for a pack that pairs),
    /// is those bytes. A pairing that doesn't fit the bytes is a no.
    pub(super) fn renders_as(
        &self,
        content: &[(String, Neutral)],
        bytes: &[u8],
        codec: &[u8],
    ) -> Result<bool> {
        let ours = match (self.pack.wants_carrier(), self.pack.wants_pairing()) {
            (true, true) => {
                let p = self.make_pairing(content, codec)?;
                match self.splice_paired(bytes, &p, content, codec) {
                    Err(SessionError::PairingMismatch { .. }) => return Ok(false),
                    r => r?,
                }
            }
            (true, false) => self.pack.encode_from(bytes, content, codec)?,
            (false, _) => self.pack.encode(content, codec)?,
        };
        Ok(ours == bytes)
    }

    /// The pack's pairing of `content`, counted in test builds.
    pub(super) fn make_pairing(
        &self,
        content: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Pairing> {
        #[cfg(any(test, feature = "differential"))]
        crate::differential::count_pairing_made();
        Ok(self.pack.pairing(content, codec)?)
    }

    /// An engine doc's content, root by root in schema order (the disk replica's, a stored
    /// synced state's, a throwaway copy's).
    pub(super) fn content_of(&self, doc: &Doc) -> Result<Vec<(String, Neutral)>> {
        self.schema
            .roots
            .iter()
            .map(|(r, k)| {
                let n = match k {
                    Kind::Text => Neutral::Text(doc.text(r)?),
                    Kind::MovableList => Neutral::List(doc.list(r)?),
                    Kind::Map => Neutral::Map(doc.map(r)?),
                    other => return Err(DocError::Unsupported(*other).into()),
                };
                Ok(((*r).to_owned(), n))
            })
            .collect()
    }

    /// The pairing of the file as the disk replica holds it (the sync point's content).
    fn replica_pairing(&self, st: &mut State) -> Result<Pairing> {
        let codec = st.codec.clone();
        let d = st
            .disk
            .as_mut()
            .ok_or_else(|| SessionError::Disk("no disk replica to pair the carrier".into()))?;
        let content = self.content_of(d.replica.get()?)?;
        self.make_pairing(&content, &codec)
    }

    /// A change that moved no byte made the replica the doc at sync point `seq`: the
    /// carrier is unchanged, but its keys may not be (an insert and its delete, a delete and a
    /// retype), so the pairing is made again from the doc and stored, the live row and the
    /// versions at `seq`. Never fails: what can't be made is reported, and a rendering is then
    /// refused until the next sync point, never spliced by guess.
    pub(super) fn resync_pairing(&self, st: &mut State, seq: Seq) {
        if !self.pack.wants_carrier() || !self.pack.wants_pairing() || st.carrier.is_none() {
            return;
        }
        let made = self
            .content(st)
            .and_then(|c| self.make_pairing(&c, &st.codec));
        match made {
            Ok(p) => {
                if let Ok(store) = self.store(st) {
                    if let Err(e) = store.set_carrier_pairing(self.doc_id, seq, &p.to_bytes()) {
                        self.disk_error(st, "keeping the carrier's pairing", e);
                    }
                }
                st.pairing = Some((seq, Arc::new(p)));
            }
            Err(e) => {
                self.disk_error(st, "making the carrier's pairing", e);
                st.pairing = None;
            }
        }
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
        let synced_state = store.sync_states(self.doc_id).ok().and_then(|(s, _)| s);
        let synced = synced_state.as_ref().map(|s| s.hash.clone());
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
                if let Some(s) = synced_state.filter(|_| self.pack.wants_pairing()) {
                    self.load_pairing(st, &store, &row.id, s);
                }
            }
            None => {
                if let Ok(gone) = store.set_live_carrier(self.doc_id, None, 0, now, None) {
                    let _ = blobs.remove_unreferenced_now(&gone);
                }
            }
        }
        // versions an older kit's retention dropped give their carriers' counts back
        let _ = store.sweep_version_carriers(self.doc_id, now);
    }

    /// The stored carrier's pairing, at a load: the stored live row when it names the
    /// carrier's blob (`blob`) and the sync point's seq; otherwise (an older kit's write, a
    /// store from before the pairing) made again from the stored synced state (the content
    /// the file held at the sync point, the one the carrier hashes to) and stored. What can't
    /// be made leaves none: a rendering is then refused, never spliced by guess.
    fn load_pairing(
        &self,
        st: &mut State,
        store: &DocStore,
        blob: &BlobId,
        synced: crate::store::SyncState,
    ) {
        let stored = store
            .carrier_pairing(self.doc_id, LIVE_SLOT)
            .ok()
            .flatten()
            .filter(|p| p.blob == *blob && p.seq == synced.seq)
            .and_then(|p| Pairing::from_bytes(&p.keys));
        let pairing = match stored {
            Some(p) => Some(p),
            None => {
                let made = Doc::load(self.schema, &synced.state)
                    .map_err(SessionError::from)
                    .and_then(|d| self.content_of(&d))
                    .and_then(|c| self.make_pairing(&c, &st.codec));
                match made {
                    Ok(p) => {
                        let _ = store.set_carrier_pairing(self.doc_id, synced.seq, &p.to_bytes());
                        Some(p)
                    }
                    Err(_) => None,
                }
            }
        };
        st.pairing = pairing.map(|p| (synced.seq, Arc::new(p)));
    }

    /// The file holds `bytes` at sync point `seq`, as the kit last read or wrote them: they
    /// become the carrier, and every version at `seq` takes them, in the blob store when there
    /// is one. A blob a carrier leaves with no reference goes at once (a put still waiting to
    /// reference it stays). Never fails: what can't be stored is reported
    /// ([`AuditEvent::DiskError`]) and kept in memory (the stored one, no longer the sync
    /// point's, is dropped at the next load).
    ///
    /// For a pack that pairs, the carrier's pairing is made here too, once per sync point:
    /// from `held` (the content the bytes hold, when the caller has it: a kit write) or else from
    /// the disk replica, which holds the file's content at the sync point; then kept with the
    /// carrier (the same bytes at a new sync point store only the pairing).
    pub(super) fn keep_carrier(&self, st: &mut State, bytes: &[u8], seq: Seq, now: u64) {
        self.keep_carrier_held(st, bytes, seq, now, None)
    }

    /// [`Session::keep_carrier`], with the content `bytes` hold when the caller has it.
    pub(super) fn keep_carrier_held(
        &self,
        st: &mut State,
        bytes: &[u8],
        seq: Seq,
        now: u64,
        held: Option<&[(String, Neutral)]>,
    ) {
        if !self.pack.wants_carrier() {
            return;
        }
        let pairs = self.pack.wants_pairing();
        let same = st.carrier.as_deref() == Some(bytes);
        let paired_here = st.pairing.as_ref().is_some_and(|(s, _)| *s == seq);
        if same && (!pairs || paired_here) {
            return;
        }
        let pairing = match pairs {
            false => None,
            // planted: a kit write keeps the keys from the sync point before
            true if !same && planted_synced_content_stale() && st.pairing.is_some() => {
                st.pairing.as_ref().map(|(_, p)| p.clone())
            }
            true => {
                let made = match held {
                    Some(c) => self.make_pairing(c, &st.codec),
                    None => self.replica_pairing(st),
                };
                match made {
                    Ok(p) => Some(Arc::new(p)),
                    Err(e) => {
                        self.disk_error(st, "making the carrier's pairing", e);
                        None
                    }
                }
            }
        };
        let keys = pairing.as_ref().map(|p| p.to_bytes());
        st.pairing = pairing.map(|p| (seq, p));
        // a rendering refused with the carrier or pairing before says nothing of these: the
        // next read renders nothing until a rendering with them finds a failure of its own
        *st.render_failed.borrow_mut() = None;
        if same {
            // the same bytes at a new sync point: only the pairing is new
            if let (Some(keys), Ok(store)) = (&keys, self.store(st)) {
                if let Err(e) = store.set_carrier_pairing(self.doc_id, seq, keys) {
                    self.disk_error(st, "keeping the carrier's pairing", e);
                }
            }
            return;
        }
        if let Some(blobs) = self.blobs.get() {
            match self.store_carrier(st, blobs, bytes, seq, now, keys.as_deref()) {
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
        // a watched doc with a 3D slot: placements the new carrier moves reach the watchers
        if st.watch.active()
            && st.view_carrier_before.is_none()
            && self.pack.view_schema().is_some_and(|v| v.space.is_some())
        {
            st.view_carrier_before = Self::view_index_kept_before(st);
        }
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
        pairing: Option<&[u8]>,
    ) -> std::result::Result<Vec<Released>, String> {
        let store = self.store(st).map_err(|e| e.to_string())?;
        let space = &st.key.space;
        let info = blobs.put_bytes(space, bytes).map_err(|e| e.to_string())?;
        let set = store.set_live_carrier(
            self.doc_id,
            Some((space, info.id, info.size)),
            seq,
            now,
            pairing,
        );
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
        let Some(c) = carrier else {
            return Ok(None);
        };
        if !self.pack.wants_pairing() {
            return Ok(Some(self.pack.encode_from(&c, &v.content, &codec)?));
        }
        // the version's own pairing, made with its carrier: none, or one naming another
        // blob (an older kit's), is no exact file, never one paired by guess
        let pairing = store
            .carrier_pairing(self.doc_id, vid)?
            .filter(|p| p.blob == row.id)
            .and_then(|p| Pairing::from_bytes(&p.keys));
        let Some(p) = pairing else {
            return Ok(None);
        };
        match self.splice_paired(&c, &p, &v.content, &codec) {
            Err(SessionError::PairingMismatch { .. }) => Ok(None),
            r => Ok(Some(r?)),
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
