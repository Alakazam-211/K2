//! The structure index on a blob doc (`docs/SESSIONS.md`, "The structure index"): the status
//! a host asks for, the job that reads a version off the session lock, the bounded commit, and
//! the read back. The kit starts no thread: a host runs a job on its own worker (in-process
//! with [`IndexJob::run`], or in a sandboxed process with [`IndexJob::open`] and
//! [`IndexJob::commit_wire`]), and hears about it through [`AuditEvent::IndexWanted`].

use std::collections::HashSet;

use super::*;
use crate::index::{
    build_row, dec_keys_row, dec_struct_row, enc_keys_row, enc_struct_row, mint_keys, needs_mint,
    view_of, DraftFault, IndexJobError, IndexLimits, IndexOutcome, IndexSpec, IndexStatus,
    IndexView, KeysRow, NotIndexedWhy, RowStatus, StructRow, INDEX_DEPTH_MAX, INDEX_RETRY_AFTER_MS,
    INDEX_TEXT_MAX,
};
use crate::store::{StructCommitted, StructPut};

/// The blobs a job is out for, per session; [`IndexJob`] frees its slot when dropped.
#[derive(Default)]
pub(super) struct InFlight(Mutex<HashSet<BlobId>>);

impl InFlight {
    fn lock(&self) -> MutexGuard<'_, HashSet<BlobId>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn contains(&self, blob: &BlobId) -> bool {
        self.lock().contains(blob)
    }

    fn claim(self: &Arc<Self>, blob: BlobId) -> Option<Slot> {
        self.lock().insert(blob).then(|| Slot {
            set: self.clone(),
            blob,
        })
    }
}

struct Slot {
    set: Arc<InFlight>,
    blob: BlobId,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.set.lock().remove(&self.blob);
    }
}

/// What a version-scoped call works on.
pub(super) struct Target {
    pub(super) store: Arc<DocStore>,
    pub(super) space: String,
    pub(super) version: VersionId,
    pub(super) ver: BlobVersion,
}

fn limits_of(c: &Config) -> IndexLimits {
    IndexLimits::new(
        c.blob_index_max_nodes,
        INDEX_DEPTH_MAX,
        c.blob_index_max_stored_bytes,
        INDEX_TEXT_MAX,
    )
}

/// Read the rest of a stream, so its verified end is reached: only a clean end proves the
/// bytes ([`crate::blob`]).
fn drain(reader: &mut BlobReader) -> std::result::Result<(), IndexJobError> {
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant() == Some(crate::differential::IndexMutant::NoDrain) {
        return Ok(());
    }
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(reader.error(e).into()),
        }
    }
}

/// A file's bytes handed to a sandboxed worker: read them to it, and hand this back to
/// [`IndexJob::commit_wire`] (the kit reads the rest, so a worker that stops early can't skip
/// the end check).
pub struct IndexInput {
    reader: BlobReader,
    job: u64,
}

impl std::io::Read for IndexInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

/// A version's bytes to index, off the session lock: `Send`, holds no lock. Dropping it frees
/// its slot, so an abandoned job can be taken again ([`BlobSession::index_job`]).
pub struct IndexJob {
    host: Arc<dyn Host>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    pack: Arc<dyn BlobPack>,
    spec: IndexSpec,
    doc: DocId,
    version: VersionId,
    space: String,
    blob: BlobId,
    size: u64,
    limits: IndexLimits,
    id: u64,
    _slot: Slot,
}

impl IndexJob {
    pub fn version(&self) -> VersionId {
        self.version
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn limits(&self) -> IndexLimits {
        self.limits
    }

    /// In-process: read, parse, read to the verified end, record. Runs [`BlobPack::index`]
    /// on the calling thread, so only for packs a host trusts with hostile bytes in its own
    /// process.
    pub fn run(self) -> std::result::Result<IndexStatus, IndexJobError> {
        let mut reader = self.blobs.reader(&self.space, &self.blob)?;
        let out = self.pack.index(&mut reader, self.size, &self.limits);
        drain(&mut reader)?;
        let outcome = match out {
            Ok(d) => IndexOutcome::Draft(d),
            Err(r) => IndexOutcome::Refused(r),
        };
        self.finish(outcome)
    }

    /// For a sandboxed worker: the verified stream and the limits to run it under. Feed the
    /// stream to [`crate::index::index_to_wire`] in the worker.
    pub fn open(&mut self) -> std::result::Result<(IndexInput, IndexLimits), IndexJobError> {
        let reader = self.blobs.reader(&self.space, &self.blob)?;
        Ok((
            IndexInput {
                reader,
                job: self.id,
            },
            self.limits,
        ))
    }

    /// Take a worker's result ([`crate::index::IndexOutcome::to_wire`]), read `input` to its
    /// verified end, check the result as untrusted, and record it.
    pub fn commit_wire(
        self,
        mut input: IndexInput,
        wire: &[u8],
    ) -> std::result::Result<IndexStatus, IndexJobError> {
        if input.job != self.id {
            return Err(IndexJobError::BadResult(
                "the input belongs to another job".to_owned(),
            ));
        }
        drain(&mut input.reader)?;
        let outcome = IndexOutcome::from_wire(wire, &self.limits)?;
        self.finish(outcome)
    }

    fn finish(self, outcome: IndexOutcome) -> std::result::Result<IndexStatus, IndexJobError> {
        let Some(sp) = self.store.blob_space(&self.space)? else {
            return Err(IndexJobError::Stale {
                version: self.version,
                reason: "the space's blobs were purged",
            });
        };
        let (row, keys, status) = self.to_row(outcome, &sp.key, sp.no)?;
        let mut encoded = enc_struct_row(&row);
        let mut keys = keys;
        let mut status = status;
        #[cfg(any(test, feature = "differential"))]
        let no_stored_cap = crate::differential::index_mutant()
            == Some(crate::differential::IndexMutant::NoStoredCap);
        #[cfg(not(any(test, feature = "differential")))]
        let no_stored_cap = false;
        if !no_stored_cap
            && matches!(row, StructRow::Ready(_))
            && encoded.len() as u64 > self.limits.max_stored_bytes
        {
            let why = NotIndexedWhy::IndexTooBig {
                bytes: encoded.len() as u64,
                limit: self.limits.max_stored_bytes,
            };
            encoded = enc_struct_row(&StructRow::NotIndexed(why.clone()));
            keys = None;
            status = IndexStatus::NotIndexed {
                version: self.version,
                why,
            };
        }
        let keys_enc = keys.as_ref().map(enc_keys_row);
        let done = self.store.blob_struct_commit(&StructPut {
            doc: self.doc,
            version: self.version,
            no: sp.no,
            blob: &self.blob,
            pack: self.spec.pack,
            format: self.spec.format,
            row: &encoded,
            keys: keys_enc.as_deref(),
        })?;
        match done {
            StructCommitted::Done => {}
            StructCommitted::Stale(reason) => {
                return Err(IndexJobError::Stale {
                    version: self.version,
                    reason,
                })
            }
            // another job recorded these bytes first: its row stands, this one is dropped
            StructCommitted::Existing => {
                let stored = self.store.blob_struct_status(
                    sp.no,
                    &self.blob,
                    self.spec.pack,
                    self.spec.format,
                )?;
                status = match stored {
                    Some(RowStatus::Ready { nodes }) => IndexStatus::Ready {
                        version: self.version,
                        nodes,
                    },
                    Some(RowStatus::NotIndexed(why)) => IndexStatus::NotIndexed {
                        version: self.version,
                        why,
                    },
                    None => {
                        return Err(IndexJobError::Stale {
                            version: self.version,
                            reason: "the row was removed",
                        })
                    }
                };
            }
        }
        let key = self.store.meta(self.doc)?.key;
        match &status {
            IndexStatus::Ready { nodes, .. } => self.host.on_event(&AuditEvent::IndexReady {
                key: key.clone(),
                version: self.version,
                nodes: *nodes,
            }),
            IndexStatus::NotIndexed { .. } => self.host.on_event(&AuditEvent::IndexNotIndexed {
                key: key.clone(),
                version: self.version,
            }),
            _ => {}
        }
        // the doc's node anchors: their last-seen records and states at this version
        if matches!(status, IndexStatus::Ready { .. }) {
            BlobSession::observe_anchors(
                &self.store,
                &*self.host,
                &*self.pack,
                self.doc,
                &key,
                || self.recorded_view(sp.no),
            );
        }
        Ok(status)
    }

    /// The version's index as just recorded (the row and its keys, read back), for the
    /// anchors; `None` when it can't be read (it moved since: nothing to observe).
    fn recorded_view(&self, no: u64) -> Option<IndexView> {
        let raw = self
            .store
            .blob_struct_row(no, &self.blob, self.spec.pack, self.spec.format)
            .ok()??;
        let StructRow::Ready(ready) = dec_struct_row(&raw).ok()? else {
            return None;
        };
        let keys = if needs_mint(&ready) {
            let b = self.store.blob_struct_keys(self.doc, self.version).ok()??;
            let k = dec_keys_row(&b).ok()?;
            if !k.is_for(self.spec.pack, self.spec.format) {
                return None;
            }
            k
        } else {
            KeysRow::default()
        };
        view_of(self.version, ready, &keys).ok()
    }

    fn to_row(
        &self,
        outcome: IndexOutcome,
        space_key: &[u8; 32],
        no: u64,
    ) -> std::result::Result<(StructRow, Option<KeysRow>, IndexStatus), IndexJobError> {
        let version = self.version;
        Ok(match outcome {
            IndexOutcome::Refused(r) => {
                let why = r.why().ok_or_else(|| {
                    IndexJobError::BadResult(format!("a stream failure, not a result: {r}"))
                })?;
                (
                    StructRow::NotIndexed(why.clone()),
                    None,
                    IndexStatus::NotIndexed { version, why },
                )
            }
            IndexOutcome::Draft(d) => match d.shape(&self.limits) {
                Err(DraftFault::Malformed(m)) => return Err(IndexJobError::BadResult(m)),
                Err(DraftFault::Over(why)) => (
                    StructRow::NotIndexed(why.clone()),
                    None,
                    IndexStatus::NotIndexed { version, why },
                ),
                Ok(shape) => {
                    let ready = build_row(&d, &shape, space_key);
                    let keys = if needs_mint(&ready) {
                        Some(minted(
                            &self.store,
                            self.doc,
                            version,
                            no,
                            &self.blob,
                            &self.spec,
                            &ready,
                        )?)
                    } else {
                        None
                    };
                    let nodes = ready.nodes.len() as u32;
                    (
                        StructRow::Ready(ready),
                        keys,
                        IndexStatus::Ready { version, nodes },
                    )
                }
            },
        })
    }
}

/// Minted keys for `ready` at `version`: the same content's keys when the doc already has this
/// blob (a restore: same bytes, same keys), else carried from the nearest indexed version of
/// the doc.
fn minted(
    store: &DocStore,
    doc: DocId,
    version: VersionId,
    no: u64,
    blob: &BlobId,
    spec: &IndexSpec,
    ready: &crate::index::ReadyRow,
) -> std::result::Result<KeysRow, crate::store::StoreError> {
    if let Some(keys) =
        store.blob_struct_keys_of_blob(doc, version, blob, spec.pack, spec.format)?
    {
        return dec_keys_row(&keys);
    }
    let neighbour = store.blob_struct_neighbour(doc, version, no, spec.pack, spec.format)?;
    let neighbour = match neighbour {
        Some((row, keys)) => match dec_struct_row(&row)? {
            StructRow::Ready(r) => Some((r, dec_keys_row(&keys)?)),
            StructRow::NotIndexed(_) => None,
        },
        None => None,
    };
    Ok(mint_keys(
        version,
        ready,
        neighbour.as_ref().map(|(r, k)| (r, k)),
        spec.pack,
        spec.format,
    ))
}

impl BlobSession {
    fn index_target(&self, version: Option<VersionId>) -> Result<Target> {
        let st = self.lock()?;
        self.target_of(&st, version)
    }

    /// [`BlobSession::index_target`] under a lock the caller holds.
    pub(super) fn target_of(&self, st: &BState, version: Option<VersionId>) -> Result<Target> {
        let store = Self::store(st)?;
        let (version, ver) = match version {
            None => (st.vid, st.cur.clone()),
            Some(v) if v == st.vid => (v, st.cur.clone()),
            Some(v) => (v, store.blob_version(self.doc_id, v)?),
        };
        Ok(Target {
            store,
            space: st.key.space.clone(),
            version,
            ver,
        })
    }

    pub(super) fn status_of(&self, t: &Target, spec: &IndexSpec) -> Result<IndexStatus> {
        let version = t.version;
        let cap = self.config.blob_index_max_bytes;
        let not_indexed = |why| Ok(IndexStatus::NotIndexed { version, why });
        if t.ver.redacted {
            return not_indexed(NotIndexedWhy::Purged);
        }
        if t.ver.size > cap {
            return not_indexed(NotIndexedWhy::TooLarge {
                size: t.ver.size,
                limit: cap,
            });
        }
        let Some(sp) = t.store.blob_space(&t.space)? else {
            return not_indexed(NotIndexedWhy::Purged);
        };
        if t.store.blob_row(sp.no, &t.ver.blob)?.is_none() {
            return not_indexed(NotIndexedWhy::Purged);
        }
        Ok(
            match t
                .store
                .blob_struct_status(sp.no, &t.ver.blob, spec.pack, spec.format)?
            {
                Some(RowStatus::Ready { nodes }) => IndexStatus::Ready { version, nodes },
                Some(RowStatus::NotIndexed(why)) => IndexStatus::NotIndexed { version, why },
                None => IndexStatus::Pending {
                    version,
                    running: self.index_flight.contains(&t.ver.blob),
                },
            },
        )
    }

    /// Where version `version`'s index stands (`None`: the current one). Host-internal, like
    /// [`BlobSession::status`]: no connection, no content.
    pub fn index_status(&self, version: Option<VersionId>) -> Result<IndexStatus> {
        let Some(spec) = self.pack.index_spec() else {
            return Ok(IndexStatus::NoIndexer);
        };
        let t = self.index_target(version)?;
        self.status_of(&t, &spec)
    }

    /// A job to index version `version` (`None`: the current one), or `None` when there is
    /// nothing to run: the pack has no index, the row exists (or is a recorded refusal), a job
    /// for these bytes is already out, or the version can't be read (over the cap, or purged;
    /// [`BlobSession::index_status`] says which). The host runs it on its own worker.
    pub fn index_job(&self, version: Option<VersionId>) -> Result<Option<IndexJob>> {
        let Some(spec) = self.pack.index_spec() else {
            return Ok(None);
        };
        let t = self.index_target(version)?;
        if !matches!(
            self.status_of(&t, &spec)?,
            IndexStatus::Pending { running: false, .. }
        ) {
            return Ok(None);
        }
        let Some(slot) = self.index_flight.claim(t.ver.blob) else {
            return Ok(None);
        };
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Ok(Some(IndexJob {
            host: self.host.clone(),
            store: t.store,
            blobs: self.blobs.clone(),
            pack: self.pack.clone(),
            spec,
            doc: self.doc_id,
            version: t.version,
            space: t.space,
            blob: t.ver.blob,
            size: t.ver.size,
            limits: limits_of(&self.config),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            _slot: slot,
        }))
    }

    /// Version `version`'s index (`None`: the current one): every node, with its key, keyed
    /// hashes and what the kit derived. Host-internal, as [`BlobSession::read_version`] is
    /// with a connection: the host decides who sees it; the typed reads that carry rights
    /// ([`BlobSession::index_list`] and the rest) build on it. Refused [`SessionError::IndexUnsupported`] without
    /// an index, [`SessionError::IndexPending`] until a job has run (retry after
    /// `retry_after_ms`), [`SessionError::NotIndexed`] past a cap, unreadable or purged.
    pub fn index_view(&self, version: Option<VersionId>) -> Result<IndexView> {
        let Some(spec) = self.pack.index_spec() else {
            return Err(SessionError::IndexUnsupported);
        };
        let t = self.index_target(version)?;
        self.load_view(&t, &spec)
    }

    /// The index of `t`'s version: [`BlobSession::index_view`] once the target is known. Takes
    /// no session lock (a caller may hold it): store reads, and the minted keys' first write.
    pub(super) fn load_view(&self, t: &Target, spec: &IndexSpec) -> Result<IndexView> {
        #[cfg(any(test, feature = "differential"))]
        crate::differential::count_index_load(matches!(
            self.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let spec = *spec;
        match self.status_of(t, &spec)? {
            IndexStatus::Ready { .. } => {}
            IndexStatus::Pending { version, .. } => {
                return Err(SessionError::IndexPending {
                    version,
                    retry_after_ms: INDEX_RETRY_AFTER_MS,
                })
            }
            IndexStatus::NotIndexed { version, why } => {
                return Err(SessionError::NotIndexed { version, why })
            }
            IndexStatus::NoIndexer => return Err(SessionError::IndexUnsupported),
        }
        let store = &t.store;
        let gone = || SessionError::NotIndexed {
            version: t.version,
            why: NotIndexedWhy::Purged,
        };
        let sp = store.blob_space(&t.space)?.ok_or_else(gone)?;
        let raw = store
            .blob_struct_row(sp.no, &t.ver.blob, spec.pack, spec.format)?
            .ok_or_else(gone)?;
        let StructRow::Ready(ready) = dec_struct_row(&raw)? else {
            return Err(gone());
        };
        let keys = if needs_mint(&ready) {
            self.ensure_keys(t, sp.no, &spec, &ready)?
        } else {
            KeysRow::default()
        };
        Ok(view_of(t.version, ready, &keys)?)
    }

    /// The version's minted keys, minting them now if this row was reused (a restore or the
    /// same bytes in another doc): first read decides, later ones keep it.
    fn ensure_keys(
        &self,
        t: &Target,
        no: u64,
        spec: &IndexSpec,
        ready: &crate::index::ReadyRow,
    ) -> Result<KeysRow> {
        if let Some(b) = t.store.blob_struct_keys(self.doc_id, t.version)? {
            let keys = dec_keys_row(&b)?;
            if keys.is_for(spec.pack, spec.format) {
                return Ok(keys);
            }
        }
        let keys = minted(
            &t.store,
            self.doc_id,
            t.version,
            no,
            &t.ver.blob,
            spec,
            ready,
        )?;
        let stored = t.store.blob_struct_put_keys(
            self.doc_id,
            t.version,
            &t.ver.blob,
            spec.pack,
            spec.format,
            &enc_keys_row(&keys),
        )?;
        // purged while this read was minting
        let Some(stored) = stored else {
            return Err(SessionError::NotIndexed {
                version: t.version,
                why: NotIndexedWhy::Purged,
            });
        };
        Ok(dec_keys_row(&stored)?)
    }

    /// After a version commits: tell the host a job would help, if the pack indexes, the
    /// size is within the cap and no row exists yet. A restore or equal bytes find their row,
    /// so they emit nothing.
    pub(super) fn want_index(&self, st: &BState, vid: VersionId, blob: BlobId, size: u64) {
        if let Ok(store) = Self::store(st) {
            self.want_index_key(&store, &st.key, vid, blob, size);
        }
    }

    pub(super) fn want_index_key(
        &self,
        store: &DocStore,
        key: &DocKey,
        vid: VersionId,
        blob: BlobId,
        size: u64,
    ) {
        let Some(spec) = self.pack.index_spec() else {
            return;
        };
        if size > self.config.blob_index_max_bytes {
            return;
        }
        let Ok(Some(sp)) = store.blob_space(&key.space) else {
            return;
        };
        if matches!(
            store.blob_struct_status(sp.no, &blob, spec.pack, spec.format),
            Ok(None)
        ) {
            self.emit(AuditEvent::IndexWanted {
                key: key.clone(),
                version: vid,
            });
        }
    }
}
