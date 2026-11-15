//! **Text → tier 0**: a live text doc becomes a tier-0 blob doc, **keeping its key, its doc id
//! and its history** (the mirror of [`crate::session::SessionManager::promote_blob_file`]).
//! Driven by [`crate::session::SessionManager::demote_text_file`] / `demote_text`; the contract
//! is in `docs/SESSIONS.md`, "Text → tier 0".
//!
//! Under the session's lock, never through the text doc's own disk path (which would merge an
//! outside save by lines, the very thing a demotion avoids):
//!
//! 1. the live doc's final state is rendered, as a write-back writes it;
//! 2. the file is read once into the blob store and judged against the doc's sync point: the
//!    file holds the live state, or what the kit last synced (or had in flight), or else an
//!    outside save made since;
//! 3. every text version becomes a blob version holding exactly what `file_at` gives for it;
//!    the live state becomes the next one (unless the last already holds it); an outside save
//!    becomes current after it, the live state also written as a conflict copy (tier 0's
//!    keep-both rule);
//! 4. one store transaction switches the doc ([`crate::store::DocStore::demote_text_doc`]),
//!    keeping the change log as the live log;
//! 5. the session closes with typed events: roster subscriptions get
//!    [`RosterEvent::Closed`] (`demoted`), claims end as `left`, the host gets
//!    [`AuditEvent::SessionClosed`] and [`AuditEvent::Demoted`], and later calls fail with
//!    [`SessionError::Closed`] (`demoted`).
//!
//! The manager then opens the blob doc on the file, and its write-back writes the live state
//! when the file doesn't hold it yet (retried on `tick` when it can't).

use std::io::Write as _;

use super::*;
use crate::blob::{BlobError, BlobId, BlobInfo, BlobStore};
use crate::session::tier0::{place_conflict_copy, resolve_copy, BlobPack};
use crate::store::{
    BlobReason, BlobSync, BlobVersion, DemoteRows, DemotedChange, VersionId, BLOB_DOC_PACK,
};

/// The reason a demotion closes the text session with ([`SessionError::Closed`],
/// [`RosterEvent::Closed`], [`AuditEvent::SessionClosed`]).
pub const DEMOTED: &str = "demoted";

/// Returned by [`crate::session::SessionManager::demote_text_file`] and `demote_text`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Demotion {
    pub key: DocKey,
    /// The doc was tier 0 already (a second demotion): nothing changed, and the rest is empty.
    pub already: bool,
    /// Each text version, oldest first, and the tier-0 version it became: its bytes are what
    /// the text doc's `file_at` gave for it.
    pub versions: Vec<(VersionId, VersionId)>,
    /// The tier-0 version holding the live doc's final state (the last of `versions` when it
    /// held it already).
    pub live: VersionId,
    /// The current tier-0 version: `live`, or the outside save's when `kept_both`.
    pub current: VersionId,
    /// Changes kept as the live log ([`crate::session::BlobSession::live_log`]).
    pub changes: u64,
    /// The file was saved from outside since the last sync: its bytes are current, and the
    /// live state is version `live` and a conflict copy.
    pub kept_both: bool,
    /// The conflict copy holding the live state, when `kept_both` and it could be written.
    pub conflict_copy: Option<PathBuf>,
    /// The demotion wrote the live state to the file. `false` when the file held it already,
    /// there is no file (or it is missing), or it couldn't be written: then it is the current
    /// version, and the tier-0 session's write-back retries on `tick`.
    pub written: bool,
    /// The connections that were in the text session (gone with it): tell their tabs.
    pub connections: Vec<(ConnId, String)>,
}

impl Demotion {
    pub(crate) fn already(key: DocKey) -> Self {
        Self {
            key,
            already: true,
            versions: Vec::new(),
            live: 0,
            current: 0,
            changes: 0,
            kept_both: false,
            conflict_copy: None,
            written: false,
            connections: Vec::new(),
        }
    }
}

/// The file a demotion reconciles with: its resolved path, `None` when it is missing.
pub(crate) struct DemoteFile {
    pub real: Option<PathBuf>,
}

/// The planted fault [`crate::differential::DemoteMutant::DropsUnwritten`].
fn planted_drops_unwritten() -> bool {
    plant_on!(
        crate::differential::demote_mutant()
            == Some(crate::differential::DemoteMutant::DropsUnwritten)
    )
}

/// The planted fault [`crate::differential::DemoteMutant::MergesByLine`], for the manager.
pub(crate) fn planted_merges_by_line() -> bool {
    plant_on!(
        crate::differential::demote_mutant()
            == Some(crate::differential::DemoteMutant::MergesByLine)
    )
}

impl Session {
    /// Demote this text doc to a tier-0 blob doc (see the module docs). On success the
    /// session is closed; on any error nothing changed (blobs put on the way are collected
    /// after the grace).
    ///
    /// Errors: [`SessionError::Declined`] `not_text` for a structured doc or a pack that keeps
    /// a carrier; [`SessionError::BadPath`] for a doc that lives in a file when `file` is
    /// `None`; [`SessionError::Declined`] `too_large` when the file is over the blob limit; a
    /// disk, blob or store error.
    pub(crate) fn demote(
        &self,
        blobs: &BlobStore,
        to: &dyn BlobPack,
        file: Option<&DemoteFile>,
    ) -> Result<Demotion> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        if self.structured || self.pack.wants_carrier() {
            return Err(SessionError::Declined(Declined {
                code: "not_text",
                message: format!(
                    "a {} doc isn't plain text: only a text doc becomes tier 0 this way",
                    self.schema.name
                ),
            }));
        }
        let store = self.store(st)?.clone();
        if file.is_none() && (st.disk.is_some() || store.sync_states(self.doc_id)?.0.is_some()) {
            return Err(SessionError::BadPath(
                "the doc lives in a file: demote it with demote_text_file".into(),
            ));
        }
        let space = st.key.space.clone();
        let mut puts: Vec<BlobId> = Vec::new();
        let out = self.demote_in(st, &store, blobs, to, file, now, &mut puts);
        // the puts stop waiting: referenced by a version, or given up
        for id in &puts {
            blobs.settle(&space, id);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn demote_in(
        &self,
        st: &mut State,
        store: &Arc<DocStore>,
        blobs: &BlobStore,
        to: &dyn BlobPack,
        file: Option<&DemoteFile>,
        now: u64,
        puts: &mut Vec<BlobId>,
    ) -> Result<Demotion> {
        let space = st.key.space.clone();
        let max = self.config.disk_read_max_bytes;
        let put = |bytes: &[u8], puts: &mut Vec<BlobId>| -> Result<BlobInfo> {
            let i = blobs.put_bytes(&space, bytes)?;
            puts.push(i.id);
            Ok(i)
        };
        let too_large = |e: BlobError| match e {
            BlobError::TooLarge { limit } => SessionError::Declined(Declined {
                code: "too_large",
                message: format!("file is over the {limit}-byte blob limit"),
            }),
            e => e.into(),
        };

        // the file as it is now, read once into the blob store
        let theirs: Option<(BlobInfo, disk::FileStat, PathBuf)> = match file
            .and_then(|f| f.real.as_ref())
        {
            None => None,
            Some(real) => {
                let io = |e: std::io::Error| SessionError::Disk(format!("{}: {e}", real.display()));
                let mut f = std::fs::File::open(real).map_err(io)?;
                let stat = disk::FileStat::of(&f.metadata().map_err(io)?);
                let info = blobs.put(&space, &mut f).map_err(too_large)?;
                puts.push(info.id);
                Some((info, stat, real.clone()))
            }
        };
        let file_bytes = match &theirs {
            Some((info, ..)) if info.size <= max => Some(blobs.read_to_vec(&space, &info.id, max)?),
            _ => None,
        };

        // the live doc's final state, as a write-back writes it
        let mut live = self.render(st)?;
        // the file was saved from outside since the last sync: it holds neither the live state
        // nor what the kit last synced (or had in flight)
        let outside = match (&theirs, &file_bytes) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(_), Some(f)) if *f == live => false,
            (Some(_), Some(f)) => {
                let key = store.doc_key(self.doc_id)?;
                let h = file_hash(&key, f);
                let (synced, pending) = store.sync_states(self.doc_id)?;
                let known = |s: &Option<SyncState>| {
                    s.as_ref()
                        .is_some_and(|s| s.hash == h || legacy_matches(&s.hash, f))
                };
                !(known(&synced) || known(&pending))
            }
        };

        // every text version, as `file_at` gives it (one version's content in memory at a time)
        let texts = store.version_infos(self.doc_id)?;
        let mut versions: Vec<BlobVersion> = Vec::with_capacity(texts.len() + 2);
        let mut text_ids: Vec<VersionId> = Vec::with_capacity(texts.len());
        let mut last_bytes: Option<Vec<u8>> = None;
        for info in &texts {
            let vid = &info.id;
            let v = store.version(self.doc_id, *vid)?;
            let bytes = self
                .pack
                .encode(&v.content, v.codec.as_deref().unwrap_or(&st.codec))?;
            let info = put(&bytes, puts)?;
            let author = v
                .authors
                .last()
                .cloned()
                .unwrap_or_else(|| IMPORT_PID.to_owned());
            let reason = match v.reason {
                VersionReason::Import if versions.is_empty() => BlobReason::Import,
                VersionReason::External => BlobReason::External,
                _ => BlobReason::Write,
            };
            let mut b = BlobVersion::new(
                info.id,
                info.size,
                v.at_ms,
                author.clone(),
                author,
                "live",
                reason,
            );
            b.name = v.name.clone();
            versions.push(b);
            text_ids.push(*vid);
            last_bytes = Some(bytes);
        }

        // the planted fault: the live state isn't kept (the tier-0 doc starts from the file,
        // or the last version)
        if planted_drops_unwritten() {
            if let Some(b) = file_bytes.clone().or_else(|| last_bytes.clone()) {
                live = b;
            }
        }

        // the live log: every change the doc logged, oldest first, with its approval
        let approvals: HashMap<Seq, Approval> = store.approvals(self.doc_id)?.into_iter().collect();
        let mut infos: Vec<ChangeInfo> = Vec::new();
        store.scan_changes(self.doc_id, |c| {
            infos.push(c);
            true
        })?;
        infos.reverse();
        let changes: Vec<DemotedChange> = infos
            .into_iter()
            .map(|c| {
                let a = approvals.get(&c.seq).cloned();
                DemotedChange::new(c, a)
            })
            .collect();

        // the live state: the last version when it holds it, else a version of its own
        let live_info = put(&live, puts)?;
        if versions.last().map(|v| v.blob) != Some(live_info.id) {
            let author = changes
                .last()
                .map(|c| c.info.author.clone())
                .unwrap_or_else(|| IMPORT_PID.to_owned());
            versions.push(BlobVersion::new(
                live_info.id,
                live_info.size,
                now,
                author.clone(),
                author,
                "live",
                BlobReason::Write,
            ));
        }
        let live_at = versions.len() - 1;

        // an outside save since the last sync: kept both ways, never merged by lines
        let mut copy = None;
        if outside {
            let (info, stat, real) = theirs.as_ref().expect("an outside save has a file");
            let file_name = real
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_owned();
            let who = versions[live_at].author_name.clone();
            copy = match place_conflict_copy(
                &*self.host,
                &|f, at, w| to.conflict_copy_name(f, at, w),
                real,
                &file_name,
                &who,
                now,
                |f| f.write_all(&live),
            ) {
                Ok(rec) => rec,
                Err((what, e)) => {
                    self.emit(AuditEvent::DiskError {
                        key: st.key.clone(),
                        error: format!("{what}: {e}"),
                    });
                    None
                }
            };
            let mut t = BlobVersion::new(
                info.id,
                info.size,
                now,
                EXTERNAL_PID,
                "external edit",
                "disk",
                BlobReason::External,
            );
            t.mtime_ms = stat.mtime_ms();
            t.conflict_copy = copy.clone();
            versions.push(t);
        }
        let sync = theirs.as_ref().map(|(info, stat, _)| BlobSync {
            blob: info.id,
            stat: disk::Seen::now(*stat).persisted(),
        });

        // the switch: one transaction
        let old = store.meta(self.doc_id)?;
        let meta = DocMeta {
            key: st.key.clone(),
            pack: BLOB_DOC_PACK.to_owned(),
            engine: crate::engine::NO_ENGINE.to_owned(),
            engine_format: 0,
            codec: Vec::new(),
            created_ms: old.created_ms,
            synced_hash: None,
            synced_seq: 0,
            declined: None,
        };
        let vids = store.demote_text_doc(
            self.doc_id,
            &DemoteRows {
                pack: self.schema.name,
                meta: &meta,
                versions: &versions,
                sync: sync.as_ref(),
                changes: &changes,
                now,
            },
        )?;

        // the session closes: nothing more reaches the text doc
        let key = st.key.clone();
        let connections: Vec<(ConnId, String)> = st
            .members
            .iter()
            .map(|m| (m.conn, m.p.pid.clone()))
            .collect();
        self.roster_flush(st, now, true);
        st.roster.end_with(RosterEvent::Closed { reason: DEMOTED });
        self.end_all_claims(st, awareness::ClaimEnd::Left);
        self.emit(AuditEvent::SessionClosed {
            key: key.clone(),
            connections: connections.clone(),
            reason: DEMOTED,
        });
        let current = *vids.last().expect("at least one version");
        if outside {
            self.emit(AuditEvent::BlobConflict {
                key: key.clone(),
                current,
                kept: vids[live_at],
                copy: copy.clone(),
                lease_holder: None,
            });
        }
        self.emit(AuditEvent::Demoted {
            key: key.clone(),
            connections: connections.clone(),
            versions: text_ids.len() as u64,
            changes: changes.len() as u64,
            kept_both: outside,
        });
        st.aware = Aware::default();
        st.closed = Some(SessionError::Closed { reason: DEMOTED });
        st.store = None;
        st.subs.clear();
        st.watch.clear();
        st.roster.close();
        st.members.clear();

        let base = theirs
            .as_ref()
            .and_then(|(_, _, real)| real.parent().map(Path::to_owned));
        Ok(Demotion {
            key,
            already: false,
            versions: text_ids.into_iter().zip(vids.iter().copied()).collect(),
            live: vids[live_at],
            current,
            changes: changes.len() as u64,
            kept_both: outside,
            conflict_copy: copy.and_then(|c| base.map(|b| resolve_copy(&b, &c))),
            written: false,
            connections,
        })
    }
}
