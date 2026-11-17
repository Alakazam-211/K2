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

pub(super) fn limits_of(c: &Config) -> IndexLimits {
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
    /// Recorded from a verb's output index at its commit ([`IndexJob::seed`]), not by reading
    /// the version.
    seeded: bool,
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
        let mut reader = self.blobs.reader_of(&self.space, self.doc, &self.blob)?;
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
        let reader = self.blobs.reader_of(&self.space, self.doc, &self.blob)?;
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

    /// Record the index a verb already read from its output (`docs/SESSIONS.md`, "Verbs on a
    /// blob doc"): the pack's draft of exactly the bytes this version holds (the kit applied the
    /// same plan to the same verified reader), recorded as [`IndexJob::run`] records a draft: the
    /// same checks, limits, keyed row, minted keys and events. So an agent's next verb finds the
    /// index in place instead of waiting for the host's job.
    pub(super) fn seed(
        mut self,
        draft: crate::index::IndexDraft,
    ) -> std::result::Result<IndexStatus, IndexJobError> {
        self.seeded = true;
        self.finish(IndexOutcome::Draft(draft))
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
                |v, ver| self.stored_view(sp.no, v, &ver.blob),
            );
        }
        Ok(status)
    }

    /// The version's index as just recorded (the row and its keys, read back), for the
    /// anchors; `None` when it can't be read (it moved since: nothing to observe).
    fn recorded_view(&self, no: u64) -> Option<IndexView> {
        self.stored_view(no, self.version, &self.blob)
    }

    /// Version `version`'s index (its bytes `blob`) as recorded: its row in this format or the
    /// older one the spec reads, with its keys as a read takes them ([`ensure_keys_in`]: the
    /// stored ones, else minted, carried from a neighbour and stored, exactly as a read of the
    /// version mints them); `None` when it isn't there.
    fn stored_view(&self, no: u64, version: VersionId, blob: &BlobId) -> Option<IndexView> {
        let formats = [Some(self.spec.format), self.spec.older_format];
        formats.into_iter().flatten().find_map(|format| {
            let raw = self
                .store
                .blob_struct_row(no, blob, self.spec.pack, format)
                .ok()??;
            let StructRow::Ready(ready) = dec_struct_row(&raw).ok()? else {
                return None;
            };
            let keys = if !needs_mint(&ready) {
                KeysRow::default()
            } else if plant_on!(
                crate::differential::tier0_verb_mutant()
                    == Some(crate::differential::Tier0VerbMutant::StoredSkipsUnminted)
            ) {
                // the planted fault: a version whose keys were never recorded is skipped
                let b = self.store.blob_struct_keys(self.doc, version).ok()??;
                let k = dec_keys_row(&b).ok()?;
                if !k.is_for(self.spec.pack, format) {
                    return None;
                }
                k
            } else {
                let spec = IndexSpec {
                    format,
                    ..self.spec
                };
                ensure_keys_in(&self.store, self.doc, version, no, blob, &spec, &ready).ok()?
            };
            view_of(version, ready, &keys).ok()
        })
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
            // the view items only from a pack that bumped its format for them: an older kit,
            // asking under the older format, never meets a row that carries them
            IndexOutcome::Draft(d) if d.has_view_items() && self.spec.older().is_none() => {
                return Err(IndexJobError::BadResult(
                    "view items from a pack whose index spec names no older format \
                     (IndexSpec::with_older_format)"
                        .to_owned(),
                ))
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
                    // the planted fault: a seeded row's keys minted afresh, not carried
                    let carry = !(self.seeded
                        && plant_on!(
                            crate::differential::tier0_verb_mutant()
                                == Some(crate::differential::Tier0VerbMutant::SeedWithoutCarry)
                        ));
                    let keys = if needs_mint(&ready) {
                        Some(minted(
                            &self.store,
                            self.doc,
                            version,
                            no,
                            &self.blob,
                            &self.spec,
                            &ready,
                            carry,
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

/// Version `version`'s minted keys as a read takes them: the stored ones for this pack and
/// format, else minted (carried from a neighbour, [`minted`]) and stored. The one rule for a
/// read of the version and for an index job's anchors ([`IndexJob`]'s recorded indexes), so
/// both see the same keys.
fn ensure_keys_in(
    store: &DocStore,
    doc: DocId,
    version: VersionId,
    no: u64,
    blob: &BlobId,
    spec: &IndexSpec,
    ready: &crate::index::ReadyRow,
) -> Result<KeysRow> {
    if let Some(b) = store.blob_struct_keys(doc, version)? {
        let keys = dec_keys_row(&b)?;
        if keys.is_for(spec.pack, spec.format) {
            return Ok(keys);
        }
    }
    let keys = minted(store, doc, version, no, blob, spec, ready, true)?;
    let stored = store.blob_struct_put_keys(
        doc,
        version,
        blob,
        spec.pack,
        spec.format,
        &enc_keys_row(&keys),
    )?;
    // purged while this read was minting
    let Some(stored) = stored else {
        return Err(SessionError::NotIndexed {
            version,
            why: NotIndexedWhy::Purged,
        });
    };
    Ok(dec_keys_row(&stored)?)
}

/// Minted keys for `ready` at `version`: the same content's keys when the doc already has this
/// blob (a restore: same bytes, same keys), else carried from the nearest indexed version of
/// the doc (`carry`; false only for a planted fault).
#[allow(clippy::too_many_arguments)]
fn minted(
    store: &DocStore,
    doc: DocId,
    version: VersionId,
    no: u64,
    blob: &BlobId,
    spec: &IndexSpec,
    ready: &crate::index::ReadyRow,
    carry: bool,
) -> std::result::Result<KeysRow, crate::store::StoreError> {
    if let Some(keys) =
        store.blob_struct_keys_of_blob(doc, version, blob, spec.pack, spec.format)?
    {
        return dec_keys_row(&keys);
    }
    let neighbour = if carry {
        store.blob_struct_neighbour(doc, version, no, spec.pack, spec.format)?
    } else {
        None
    };
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
    pub(super) fn index_target(&self, version: Option<VersionId>) -> Result<Target> {
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
        Ok(self.status_at(t, spec)?.0)
    }

    /// [`BlobSession::status_of`], and the format of the row it read: `spec.format`'s, else
    /// (none there, and the spec names an older format) a ready row of that older format,
    /// which reads with the view items absent ([`IndexSpec::with_older_format`]).
    fn status_at(&self, t: &Target, spec: &IndexSpec) -> Result<(IndexStatus, u32)> {
        let version = t.version;
        let cap = self.config.blob_index_max_bytes;
        let not_indexed = |why| Ok((IndexStatus::NotIndexed { version, why }, spec.format));
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
        // a chunked version's manifest row stands in for the blob row
        if t.store.blob_row(sp.no, &t.ver.blob)?.is_none()
            && !self.blobs.is_chunked(self.doc_id, &t.ver.blob)?
        {
            return not_indexed(NotIndexedWhy::Purged);
        }
        let status = |f| t.store.blob_struct_status(sp.no, &t.ver.blob, spec.pack, f);
        Ok(match status(spec.format)? {
            Some(RowStatus::Ready { nodes }) => {
                (IndexStatus::Ready { version, nodes }, spec.format)
            }
            Some(RowStatus::NotIndexed(why)) => {
                (IndexStatus::NotIndexed { version, why }, spec.format)
            }
            None => {
                let older = match spec.older() {
                    Some(o) => status(o)?.map(|s| (s, o)),
                    None => None,
                };
                match older {
                    Some((RowStatus::Ready { nodes }, o)) => {
                        (IndexStatus::Ready { version, nodes }, o)
                    }
                    _ => (
                        IndexStatus::Pending {
                            version,
                            running: self.index_flight.contains(&t.ver.blob),
                        },
                        spec.format,
                    ),
                }
            }
        })
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
    /// [`BlobSession::index_status`] says which). The host runs it on its own worker. A
    /// version read from a row of the spec's older format ([`IndexSpec::with_older_format`]:
    /// `Ready`, the view items absent) gets a job too, which indexes it again under this
    /// format; running it is optional.
    pub fn index_job(&self, version: Option<VersionId>) -> Result<Option<IndexJob>> {
        let Some(spec) = self.pack.index_spec() else {
            return Ok(None);
        };
        let t = self.index_target(version)?;
        let due = match self.status_at(&t, &spec)? {
            (IndexStatus::Pending { running, .. }, _) => !running,
            (IndexStatus::Ready { .. }, f) => f != spec.format,
            _ => false,
        };
        if !due {
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
            seeded: false,
            _slot: slot,
        }))
    }

    /// **Test-only:** version `version`'s stored index (its row and minted keys, encoded) beside
    /// what an index job would record for it now (the pack's reader on the version's stored
    /// bytes, the job's own row, keys and limits; nothing recorded). A row a verb's commit
    /// seeded must equal the job's.
    #[cfg(any(test, feature = "differential"))]
    #[allow(clippy::type_complexity)]
    pub fn index_rows_for_tests(
        &self,
        version: VersionId,
    ) -> Result<(
        (Option<Vec<u8>>, Option<Vec<u8>>),
        (Vec<u8>, Option<Vec<u8>>),
    )> {
        let spec = self
            .pack
            .index_spec()
            .ok_or(SessionError::IndexUnsupported)?;
        let t = self.index_target(Some(version))?;
        let broken = |e: IndexJobError| SessionError::Broken(e.to_string());
        let sp = t
            .store
            .blob_space(&t.space)?
            .ok_or_else(|| SessionError::Broken("no space".into()))?;
        let stored = (
            t.store
                .blob_struct_row(sp.no, &t.ver.blob, spec.pack, spec.format)?,
            t.store.blob_struct_keys(self.doc_id, version)?,
        );
        let slot = self
            .index_flight
            .claim(t.ver.blob)
            .ok_or_else(|| SessionError::Broken("a job is out for these bytes".into()))?;
        let job = IndexJob {
            host: self.host.clone(),
            store: t.store.clone(),
            blobs: self.blobs.clone(),
            pack: self.pack.clone(),
            spec,
            doc: self.doc_id,
            version,
            space: t.space.clone(),
            blob: t.ver.blob,
            size: t.ver.size,
            limits: limits_of(&self.config),
            id: 0,
            seeded: false,
            _slot: slot,
        };
        let mut reader = self.blobs.reader_of(&t.space, self.doc_id, &t.ver.blob)?;
        let out = self.pack.index(&mut reader, t.ver.size, &job.limits);
        drain(&mut reader).map_err(broken)?;
        let outcome = match out {
            Ok(d) => IndexOutcome::Draft(d),
            Err(r) => IndexOutcome::Refused(r),
        };
        let (row, keys, _) = job.to_row(outcome, &sp.key, sp.no).map_err(broken)?;
        Ok((
            stored,
            (enc_struct_row(&row), keys.as_ref().map(enc_keys_row)),
        ))
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
        let (ready, keys) = self.load_parts(t, spec)?;
        Ok(view_of(t.version, ready, &keys)?)
    }

    /// The index of a verb's output (`docs/SESSIONS.md`, "Verbs on a blob doc"), built in memory
    /// as an index job would record it for `version`, the version the output would be: the
    /// draft checked against the limits, its hashes keyed by the doc's space, and minted keys
    /// carried from `t`'s version (the one the verb read). Nothing is stored. A draft the kit
    /// would not record is [`SessionError::OutputUnreadable`].
    pub(super) fn output_view(
        &self,
        t: &Target,
        spec: &IndexSpec,
        draft: &crate::index::IndexDraft,
        version: VersionId,
    ) -> Result<IndexView> {
        let unreadable = |why: String| SessionError::OutputUnreadable { why };
        if draft.has_view_items() && spec.older().is_none() {
            return Err(unreadable(
                "view items from a pack whose index spec names no older format".to_owned(),
            ));
        }
        let limits = limits_of(&self.config);
        let shape = match draft.shape(&limits) {
            Ok(shape) => shape,
            Err(DraftFault::Malformed(m)) => return Err(unreadable(m)),
            Err(DraftFault::Over(why)) => return Err(unreadable(why.to_string())),
        };
        let sp = t
            .store
            .blob_space(&t.space)?
            .ok_or(SessionError::NotIndexed {
                version: t.version,
                why: NotIndexedWhy::Purged,
            })?;
        let ready = build_row(draft, &shape, &sp.key);
        let row = StructRow::Ready(ready);
        let bytes = enc_struct_row(&row).len() as u64;
        if bytes > limits.max_stored_bytes {
            return Err(unreadable(
                NotIndexedWhy::IndexTooBig {
                    bytes,
                    limit: limits.max_stored_bytes,
                }
                .to_string(),
            ));
        }
        let StructRow::Ready(ready) = row else {
            unreachable!("built as ready")
        };
        let keys = if needs_mint(&ready) {
            let (was, was_keys) = self.load_parts(t, spec)?;
            mint_keys(
                version,
                &ready,
                Some((&was, &was_keys)),
                spec.pack,
                spec.format,
            )
        } else {
            KeysRow::default()
        };
        Ok(view_of(version, ready, &keys)?)
    }

    /// The row and keys [`BlobSession::load_view`] reads.
    fn load_parts(
        &self,
        t: &Target,
        spec: &IndexSpec,
    ) -> Result<(crate::index::ReadyRow, KeysRow)> {
        #[cfg(any(test, feature = "differential"))]
        crate::differential::count_index_load(matches!(
            self.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let (status, format) = self.status_at(t, spec)?;
        // the row read: this format's, or the older one's (its items absent, its keys its own)
        let spec = IndexSpec { format, ..*spec };
        match status {
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
        Ok((ready, keys))
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
        ensure_keys_in(
            &t.store,
            self.doc_id,
            t.version,
            no,
            &t.ver.blob,
            spec,
            ready,
        )
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
