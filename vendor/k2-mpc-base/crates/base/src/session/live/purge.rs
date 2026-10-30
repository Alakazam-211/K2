//! Erasure on a live text doc (K-PURGE-1…7, -10, -13; undo spec U19, U20):
//! [`Session::purge`]. A child of `live`, so it works on the session's own state.
//!
//! **Purge removes history, not live text.** What the doc says now stays; every other copy
//! the kit holds goes. To take text out of the doc too, edit it out first, then purge (or
//! pass [`PurgeRequest::and_remove`] with a text scope: the session deletes every live
//! copy as an ordinary change by the owner, then purges, under one lock).

use super::*;
use crate::store::{
    fresh_key, DocPurge, Needles, Pick, PurgeCounts, RewriteReport, VersionId, MIN_MATCH,
};

/// What a purge targets ([`PurgeRequest`]).
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PurgeScope {
    /// The whole history before now: every change's content, every version's content, the
    /// log, snapshots, caches, undo stacks and the sync replica. Only the live text stays.
    History,
    /// The changes at log positions `from..=to`: their revert records whole, and every other
    /// copy of the text they inserted or deleted (8-byte windows of it) in versions and other
    /// changes' records. A piece shorter than 8 bytes is redacted in its own record only.
    Changes { from: Seq, to: Seq },
    /// Versions `from..=to` (ids): their content whole (entries and names stay), and the
    /// changes between them as [`PurgeScope::Changes`].
    Versions { from: VersionId, to: VersionId },
    /// A text the owner names (`purge --text`, read from stdin): every copy of it, whole or by
    /// any 8-byte window, in every version and change record. Nothing about it is kept after.
    /// It must be at least [`MIN_MATCH`] (8) bytes: a shorter one would match innocent text
    /// across the whole history, so it is refused ([`SessionError::BadEdit`], exit 2).
    Text(String),
    /// Participant erasure (K-PURGE-7): replace `pid` by `pseudonym` in attribution (log,
    /// changes, revert records, approvals, versions' authors, the client table), keeping
    /// counts, and purge the content of every change they authored, as
    /// [`PurgeScope::Changes`]. Present connections of `pid` are removed.
    Participant { pid: String, pseudonym: String },
}

/// Never prints the text of [`PurgeScope::Text`].
impl std::fmt::Debug for PurgeScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::History => f.write_str("History"),
            Self::Changes { from, to } => write!(f, "Changes {{ from: {from}, to: {to} }}"),
            Self::Versions { from, to } => write!(f, "Versions {{ from: {from}, to: {to} }}"),
            Self::Text(t) => write!(f, "Text(<{} bytes>)", t.len()),
            Self::Participant { pseudonym, .. } => {
                write!(
                    f,
                    "Participant {{ pid: <erased>, pseudonym: {pseudonym:?} }}"
                )
            }
        }
    }
}

/// [`Session::purge`]'s request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PurgeRequest {
    pub scope: PurgeScope,
    /// With [`PurgeScope::Text`] only: first delete every live copy of the text (an ordinary
    /// change by the purging owner, which needs edit too), then purge.
    pub and_remove: bool,
}

impl PurgeRequest {
    pub fn new(scope: PurgeScope) -> Self {
        Self {
            scope,
            and_remove: false,
        }
    }

    /// `purge --and-remove`.
    pub fn and_remove(self) -> Self {
        Self {
            and_remove: true,
            ..self
        }
    }
}

/// A place purge does not reach (K-PURGE-10, -12). Every purge returns the standing list
/// ([`NotReached::ALWAYS`]) plus what applies to this one; the host prints it with
/// [`NotReached::describe`] and tells the owner to **rotate the secret**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NotReached {
    /// The doc's file wasn't rewritten (missing, declined, unwritable, or not attached to
    /// this session): it, and the sync replica of it the kit keeps, still hold its text as
    /// last written. Attach and flush the file, then purge again.
    File,
    /// This many targeted changes had already left the revert window: the text they changed
    /// is unknown, so copies of it elsewhere couldn't be found. Purge by text, or the whole
    /// history.
    ChangesOutsideWindow(u64),
    /// Rewriting the store file failed: freed pages of it may still hold purged bytes. Run
    /// [`crate::store::DocStore::rewrite_file`].
    StoreFile,
    /// Tier 0: this many purged blobs are still referenced elsewhere (another version or doc
    /// with the same bytes, or a pack's retain): their bytes stay.
    SharedBlobs(u64),
    /// Tier 0: this many recorded conflict copies didn't match purged content (edited since,
    /// or holding the current content) and were left.
    ConflictCopies(u64),
    /// A doc promoted from tier 0 (v0.9.2): this many tier-0 versions it kept (content that
    /// didn't read as text) were not searched. Only [`PurgeScope::History`] reaches them, or
    /// [`Session::purge_tier0`]; also reported by a `History` purge whose released blobs
    /// couldn't be removed (no blob store): they go with the blob store's GC.
    Tier0History(u64),
    /// Git history, if the file was ever committed.
    GitHistory,
    /// Agent transcripts and terminal scrollback.
    Transcripts,
    /// Spotlight's (or another indexer's) index.
    SearchIndex,
    /// APFS snapshots, Time Machine and other OS-level backups.
    OsSnapshots,
    /// Synced-folder history (iCloud Drive, Dropbox...).
    SyncedFolders,
    /// K2 backups made before the purge (K-PURGE-9).
    BackupsBeforePurge,
    /// Clients and devices that haven't reconnected since.
    OfflineClients,
    /// Other apps that read the file, and sent notifications.
    OtherApps,
    /// The host's own rows in the store (`host_*`): the kit never reads them.
    HostRows,
    /// Disk blocks of the old store file, released to the file system, not overwritten.
    FreedDiskBlocks,
}

impl NotReached {
    /// What no purge reaches, ever.
    pub const ALWAYS: [NotReached; 10] = [
        NotReached::GitHistory,
        NotReached::Transcripts,
        NotReached::SearchIndex,
        NotReached::OsSnapshots,
        NotReached::SyncedFolders,
        NotReached::BackupsBeforePurge,
        NotReached::OfflineClients,
        NotReached::OtherApps,
        NotReached::HostRows,
        NotReached::FreedDiskBlocks,
    ];

    /// One line for the owner.
    pub fn describe(&self) -> &'static str {
        match self {
            NotReached::File => "the file on disk (not rewritten) and its last-written copy",
            NotReached::ChangesOutsideWindow(_) => {
                "changes older than the revert window (their text is unknown)"
            }
            NotReached::StoreFile => "freed pages of the store file (rewrite failed)",
            NotReached::SharedBlobs(_) => "blobs still used by other versions or docs",
            NotReached::ConflictCopies(_) => "conflict copies that don't match purged content",
            NotReached::Tier0History(_) => {
                "non-text versions kept from before the doc read as text (purge_tier0)"
            }
            NotReached::GitHistory => "git history, if the file was committed",
            NotReached::Transcripts => "agent transcripts and terminal scrollback",
            NotReached::SearchIndex => "Spotlight and other search indexes",
            NotReached::OsSnapshots => "APFS snapshots, Time Machine and OS backups",
            NotReached::SyncedFolders => "synced-folder history",
            NotReached::BackupsBeforePurge => "K2 backups made before the purge",
            NotReached::OfflineClients => "devices that haven't reconnected",
            NotReached::OtherApps => "other apps that read the file; sent notifications",
            NotReached::HostRows => "the host's own records",
            NotReached::FreedDiskBlocks => "freed disk blocks of the old store file",
        }
    }
}

/// Returned by [`Session::purge`] (and the tier-0 purges).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PurgeOutcome {
    pub counts: PurgeCounts,
    /// Spans of the live text the purged content still matches (whole or by 8-byte window).
    /// Purge doesn't change live text: edit them out, then purge again.
    pub live_matches: usize,
    /// `and_remove`'s change, when it removed anything.
    pub removed: Option<EditResult>,
    /// Where the purge didn't reach (K-PURGE-10): [`NotReached::ALWAYS`] and more.
    pub not_reached: Vec<NotReached>,
    /// The store file's rewrite (`None` if it failed: see [`NotReached::StoreFile`]).
    pub rewrite: Option<RewriteReport>,
}

impl PurgeOutcome {
    pub(crate) fn of_blobs(
        counts: PurgeCounts,
        not_reached: Vec<NotReached>,
        rewrite: Option<RewriteReport>,
    ) -> Self {
        Self {
            counts,
            live_matches: 0,
            removed: None,
            not_reached,
            rewrite,
        }
    }
}

/// Every live copy of `t` in `text`, left to right, as deletions.
fn deletions(text: &str, t: &str) -> Vec<Hunk> {
    text.match_indices(t)
        .map(|(i, m)| Hunk {
            start: i,
            end: i + m.len(),
            insert: String::new(),
        })
        .collect()
}

impl Session {
    /// **Purge** (K-PURGE-1…7, -13): remove the content `req` targets from everything the kit
    /// holds for this doc, keeping change ids, authors, lengths and times as metadata. Owner
    /// only ([`Rights::owner`]; others get [`SessionError::Forbidden`] with [`Need::Owner`]
    /// and an [`AuditEvent::Refused`]). Live text is not changed (see
    /// [`PurgeRequest::and_remove`]).
    ///
    /// In order: the file is written back (so it holds the live text); every undo and redo
    /// stack of the doc is cleared (U19: holders see [`UndoOutcome::ClearedByPurge`]) and
    /// the read-base cache dropped; then **one store transaction** redacts the log (a
    /// snapshot re-encoded from the live doc replaces it), versions, revert records (a
    /// revert over them reports [`crate::session::ConflictKind::Erased`]) and the sync replica, and replaces
    /// the doc's hash key (old sync and own-write hashes mean nothing); the in-memory doc
    /// and replica are reloaded from what was stored; then the store file is rewritten
    /// ([`crate::store::DocStore::rewrite_file`]) so no freed page keeps a purged byte.
    /// Audited as [`AuditEvent::Purged`] (metadata only).
    pub fn purge(&self, conn: ConnId, req: PurgeRequest) -> Result<PurgeOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Owner, Action::Purge, now)?;
        let by = st.members[i].p.pid.clone();
        let store = self.store(st)?.clone();
        // a store holding a newer kit's data can't be rewritten: refuse before anything
        // changes (v0.9.5)
        store.check_rewritable()?;
        // check the scope before anything changes
        let mut picks = (Pick::None, Pick::None);
        let mut erase: Option<(String, String)> = None;
        let mut summary = PurgedScope::History;
        match &req.scope {
            PurgeScope::History => picks = (Pick::All, Pick::All),
            PurgeScope::Changes { from, to } => {
                for c in [*from, *to] {
                    if c == 0 || c > st.version {
                        return Err(SessionError::NoSuchChange(c));
                    }
                }
                if from > to {
                    return Err(SessionError::NoSuchChange(*from));
                }
                picks.0 = Pick::Range(*from, *to);
                summary = PurgedScope::Changes {
                    from: *from,
                    to: *to,
                };
            }
            PurgeScope::Versions { from, to } => {
                let infos = store.version_infos(self.doc_id)?;
                let seq_of = |v: VersionId| {
                    infos
                        .iter()
                        .find(|x| x.id == v)
                        .map(|x| x.seq)
                        .ok_or(SessionError::NoSuchChange(v))
                };
                let (a, b) = (seq_of(*from)?, seq_of(*to)?);
                if from > to {
                    return Err(SessionError::NoSuchChange(*from));
                }
                picks = (
                    if a < b {
                        Pick::Range(a + 1, b)
                    } else {
                        Pick::None
                    },
                    Pick::Range(*from, *to),
                );
                summary = PurgedScope::Versions {
                    from: *from,
                    to: *to,
                };
            }
            PurgeScope::Text(t) => {
                if t.len() < MIN_MATCH {
                    return Err(SessionError::BadEdit(format!(
                        "purge text of at least {MIN_MATCH} bytes, or purge by \
                         change/version/participant"
                    )));
                }
                summary = PurgedScope::Text { bytes: t.len() };
            }
            PurgeScope::Participant { pid, pseudonym } => {
                let bad = |m: &str| Err(SessionError::BadParticipant(m.into()));
                if pid.is_empty() || pseudonym.is_empty() || pid == pseudonym {
                    return bad("erasure needs a pid and a different, non-empty pseudonym");
                }
                if pid.starts_with(RESERVED_PID_PREFIX)
                    || pseudonym.starts_with(RESERVED_PID_PREFIX)
                {
                    return bad("the kit's own pids can't be erased or used as pseudonyms");
                }
                if *pid == by {
                    return bad("an owner can't erase themselves");
                }
                let mut mine = std::collections::HashSet::new();
                store.scan_changes(self.doc_id, |c| {
                    if c.author == *pid {
                        mine.insert(c.seq);
                    }
                    true
                })?;
                picks.0 = Pick::Set(mine);
                erase = Some((pid.clone(), pseudonym.clone()));
                summary = PurgedScope::Participant {
                    pseudonym: pseudonym.clone(),
                };
            }
        }
        if req.and_remove && !matches!(req.scope, PurgeScope::Text(_)) {
            return Err(SessionError::BadEdit(
                "and_remove needs a text scope (the live text to delete)".into(),
            ));
        }
        // and_remove: delete every live copy first, as the owner's own change
        let mut removed = None;
        if let (true, PurgeScope::Text(t)) = (req.and_remove, &req.scope) {
            let i = self.admit_edit(st, conn, Action::Purge, now)?;
            let edits: Vec<Edit> = st
                .texts
                .iter()
                .map(|(root, text)| Edit::Text {
                    root: (*root).to_owned(),
                    hunks: deletions(text, t),
                })
                .filter(|e| matches!(e, Edit::Text { hunks, .. } if !hunks.is_empty()))
                .collect();
            if !edits.is_empty() {
                // a purge's removal is no authorship: roots given (none put back), so D1
                // leaves it as it is (v0.9.7)
                let none = vec![None; edits.len()];
                removed = Some(self.commit_edit_restoring(
                    st,
                    i,
                    edits,
                    Some(none),
                    ChangeKind::Edit,
                    None,
                    now,
                    Vec::new(),
                )?);
            }
        }
        // the texts to look for outside the targeted records
        let mut texts: Vec<String> = Vec::new();
        let mut outside_window = 0;
        if let PurgeScope::Text(t) = &req.scope {
            texts.push(t.clone());
        } else if !matches!(picks.0, Pick::None | Pick::All) {
            let records = store.revert_records(self.doc_id)?;
            let pick = &picks.0;
            let held: std::collections::HashSet<Seq> = records.iter().map(|(s, _)| *s).collect();
            for (seq, rec) in &records {
                if !pick.has(*seq) {
                    continue;
                }
                for h in rec.roots.iter().flat_map(|r| &r.hunks) {
                    for t in [&h.insert, &h.deleted] {
                        if !t.is_empty() {
                            texts.push(t.clone());
                        }
                    }
                }
            }
            outside_window = match pick {
                Pick::Range(a, b) => (*a..=*b).filter(|s| !held.contains(s)).count() as u64,
                Pick::Set(set) => set.iter().filter(|s| !held.contains(s)).count() as u64,
                _ => 0,
            };
        }
        let needles = Needles::new(
            texts.iter().map(|t| t.as_bytes()),
            matches!(req.scope, PurgeScope::Text(_)),
        );
        let mut not_reached: Vec<NotReached> = Vec::new();
        if outside_window > 0 {
            not_reached.push(NotReached::ChangesOutsideWindow(outside_window));
        }
        // the file first: write it back so it holds the live text (and the replica with it)
        let file_holds_live = st.disk.is_some()
            && self.write_back(st, now).is_ok()
            && matches!(Self::disk_state(st), Some(DiskState::Clean));
        // memory that holds history: every undo and redo stack (U19), the read bases
        let undo_batches = Self::clear_stacks(st);
        let read_bases = st.read_bases.len() as u64;
        st.read_bases.clear();
        st.read_base_bytes = 0;
        if let Some((pid, ps)) = &erase {
            st.hist.remove(pid); // U20: the erased person's own history goes
            for (_, a) in st.authors.iter_mut() {
                if a == pid {
                    *a = ps.clone();
                }
            }
            self.remove_pid(st, pid, LeaveReason::Kicked);
            // the presence extras keep no trace of them either (activity, note dedupe)
            st.aware.forget(pid);
            st.handles.forget(pid);
        }
        // waiting notes hold capped text: a purge takes it, keeping the fact and the count
        self.withhold_notes(st);
        // the new state: re-encoded from the live doc (GC on: no deleted content)
        let state = match st.doc.encode_state() {
            Ok(s) => s,
            Err(e) => {
                self.recover(st, &e.to_string());
                return Err(e.into());
            }
        };
        let doc = self.checked(Doc::load(self.schema, &state)?);
        if !file_holds_live {
            // the replica is kept below (re-encoded): if the doc still renders to the synced
            // bytes (the file missing, say), it must be the doc's elements, not an older copy
            // of its text, before the new key leaves the sync point unknown (v0.9.4)
            self.resync_if_synced(st)?;
        }
        let key = fresh_key()?;
        let sync = if file_holds_live {
            Some(SyncState {
                hash: file_hash(&key, &self.render(st)?),
                seq: st.version,
                state: state.clone(),
            })
        } else {
            let stored = store.sync_states(self.doc_id)?.0;
            if st.disk.is_some() || stored.is_some() {
                not_reached.push(NotReached::File);
            }
            match (&st.disk, stored) {
                // the file's text stays (purge can't reach the file); its state is re-encoded
                (Some(d), _) => {
                    let bytes = disk::probe(&d.path, &d.root).ok().and_then(|p| match p {
                        Probe::File { real, .. } => {
                            match disk::read(&real, self.config.disk_read_max_bytes) {
                                Ok(ReadFile::Bytes(b, _)) => Some(b),
                                _ => None,
                            }
                        }
                        _ => None,
                    });
                    Some(SyncState {
                        hash: bytes.map_or_else(Vec::new, |b| file_hash(&key, &b)),
                        seq: d.synced_seq,
                        state: d.replica.encode_state()?,
                    })
                }
                (None, Some(s)) => Some(SyncState {
                    hash: Vec::new(),
                    seq: s.seq,
                    state: Doc::load(self.schema, &s.state)?.encode_state()?,
                }),
                (None, None) => None,
            }
        };
        let live_matches = st
            .texts
            .iter()
            .map(|(_, t)| needles.text_spans(t).len())
            .sum();
        let purge = DocPurge {
            head: st.version,
            state: &state,
            changes: picks.0,
            versions: picks.1,
            needles: &needles,
            sync: sync.clone(),
            key,
            erase: erase.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
        };
        let mut counts = store.purge_doc(self.doc_id, &purge)?;
        counts.undo_batches = undo_batches;
        counts.read_bases = read_bases;
        // memory follows the store
        st.doc = doc;
        st.hash_key = key;
        st.reverts = None;
        // the re-encoded doc keeps every live element's ID; rebuilt on next need anyway
        st.prov = None;
        // a participant purge renames client owners: they are read again (D1's owners and
        // the agent client IDs reused from them)
        st.owners = None;
        st.log_entries = 0;
        st.log_bytes = 0;
        st.state_bytes = state.len() as u64;
        if let (Some(d), Some(s)) = (st.disk.as_mut(), &sync) {
            d.replica = self.checked(Doc::load(self.schema, &s.state)?);
            d.replica_bytes = s.state.len() as u64;
            d.synced_hash = s.hash.clone();
            d.synced_seq = s.seq;
            d.own.clear();
            // it held the purged text: gone; the next write-back reloads the replica
            d.catch_up = None;
            d.echoed = None;
        }
        // a doc promoted from tier 0 (v0.9.2): the tier-0 versions it kept are history too
        let whole = matches!(req.scope, PurgeScope::History);
        self.purge_kept_tier0(st, &store, whole, now, &mut counts, &mut not_reached)?;
        let rewrite = match store.rewrite_file() {
            Ok(r) => Some(r),
            Err(_) => {
                not_reached.push(NotReached::StoreFile);
                None
            }
        };
        not_reached.extend(NotReached::ALWAYS);
        self.emit(AuditEvent::Purged {
            key: st.key.clone(),
            by,
            scope: summary,
            counts,
        });
        Ok(PurgeOutcome {
            counts,
            live_matches,
            removed,
            not_reached,
            rewrite,
        })
    }
}
