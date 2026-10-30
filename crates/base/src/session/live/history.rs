//! History on a live text doc (C7a): versions (automatic cuts, named, retention), the
//! history reads (`log`, `diff --version`, `show --version`), `restore`, and `revert` with
//! its window. A child of `live`, so it works on the session's own state.
//!
//! These are session methods, not tool verbs: they return structured values (a host's CLI
//! renders `log` its own way) and need rights the text tools don't (history, owner). The
//! `tools` module stays the pure text verbs.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::store::{
    window_plan, Carry, Retention, RevertWindow, StoreError, VersionId, VersionInfo,
};
use crate::text::diff;

/// `log` filters (K-HIST-9, -18). `Default` lists everything, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogQuery {
    /// `--by <pid>`: changes by this participant, versions with them among the authors.
    pub by: Option<String>,
    /// `--since`: items at or after this time (host clock, ms).
    pub since_ms: Option<u64>,
    /// `-n`: at most this many items.
    pub limit: Option<usize>,
    /// `--on-behalf-of <pid>`: changes made for this person (versions are left out).
    pub on_behalf_of: Option<String>,
    /// `--pending`: on-behalf changes still waiting for approval (versions are left out).
    pub pending: bool,
    /// `--pending --all` (`approvals --all`, v0.9.2): with `pending`, every on-behalf
    /// change with an approval record, settled ones too (approved, declined).
    pub settled: bool,
}

impl LogQuery {
    pub fn by(self, pid: impl Into<String>) -> Self {
        Self {
            by: Some(pid.into()),
            ..self
        }
    }
    pub fn since_ms(self, at: u64) -> Self {
        Self {
            since_ms: Some(at),
            ..self
        }
    }
    pub fn limit(self, n: usize) -> Self {
        Self {
            limit: Some(n),
            ..self
        }
    }
    pub fn on_behalf_of(self, pid: impl Into<String>) -> Self {
        Self {
            on_behalf_of: Some(pid.into()),
            ..self
        }
    }
    pub fn pending(self) -> Self {
        Self {
            pending: true,
            ..self
        }
    }
    /// `--pending --all`: every approval record, pending or settled ([`LogQuery::settled`]).
    pub fn with_settled(self) -> Self {
        Self {
            pending: true,
            settled: true,
            ..self
        }
    }
}

/// One `log` line: a change or a version.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogItem {
    Change {
        info: ChangeInfo,
        /// The approval state of an on-behalf change.
        approval: Option<ApprovalState>,
    },
    Version(VersionInfo),
}

impl LogItem {
    fn key(&self) -> (Seq, u8) {
        match self {
            // a version at position S reflects the doc after change S: list it first
            LogItem::Change { info, .. } => (info.seq, 0),
            LogItem::Version(v) => (v.seq, 1),
        }
    }
}

/// Returned by [`Session::diff_version`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffOutput {
    /// A unified diff (`diff -u`), empty when the texts are the same.
    pub text: String,
    pub differs: bool,
}

impl DiffOutput {
    /// 0 when the same, 1 when different (K-HIST-10).
    pub fn exit(&self) -> u8 {
        if self.differs {
            crate::tools::exit::NO
        } else {
            crate::tools::exit::OK
        }
    }
}

/// What one retention pass did (for [`super::super::TickReport`]).
#[derive(Debug, Default)]
pub(crate) struct HistoryTick {
    pub cut: Option<VersionId>,
    pub thinned: usize,
    pub reverts_dropped: u64,
}

fn no_version(vid: VersionId) -> impl FnOnce(StoreError) -> SessionError {
    move |e| match e {
        StoreError::NoVersion(..) => SessionError::NoSuchChange(vid),
        e => e.into(),
    }
}

impl Session {
    fn admit_history(&self, st: &mut State, conn: ConnId, now: u64) -> Result<usize> {
        self.admit(st, conn, Need::Read, Action::History, now)?;
        self.admit(st, conn, Need::History, Action::History, now)
    }

    /// Every version of the doc, oldest first, without content. Needs history.
    pub fn versions(&self, conn: ConnId) -> Result<Vec<VersionInfo>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit_history(&mut st, conn, now)?;
        Ok(self.store(&st)?.version_infos(self.doc_id)?)
    }

    /// `show --version V`: each root's text at version `vid`. Needs history.
    pub fn show(&self, conn: ConnId, vid: VersionId) -> Result<Vec<(String, String)>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit_history(&mut st, conn, now)?;
        let v = self
            .store(&st)?
            .version(self.doc_id, vid)
            .map_err(no_version(vid))?;
        Ok(v.content
            .into_iter()
            .map(|(r, n)| match n {
                Neutral::Text(t) => (r, t),
            })
            .collect())
    }

    /// **The file at version `vid`** (exports): the bytes a write-back of that version's
    /// content would write (the pack's encoding, with the doc's codec settings), its doc
    /// version ([`FileBytes::version`]) and how the codec was applied ([`FileCodec`]: today's
    /// settings; exact for the latest content). Reads only: no version, change, lease, claim
    /// or read base. Needs history, like `show`; an unknown `vid` is
    /// [`SessionError::NoSuchChange`].
    pub fn file_at(&self, conn: ConnId, vid: VersionId) -> Result<FileBytes> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit_history(&mut st, conn, now)?;
        let v = self
            .store(&st)?
            .version(self.doc_id, vid)
            .map_err(no_version(vid))?;
        let bytes = self.pack.encode(&v.content, &st.codec)?;
        let current = v.seq == st.version
            && v.content
                .iter()
                .zip(st.texts.iter())
                .all(|((r, Neutral::Text(t)), (lr, lt))| r == lr && t == lt)
            && v.content.len() == st.texts.len();
        Ok(FileBytes {
            bytes,
            version: v.seq,
            codec: if current {
                FileCodec::Exact
            } else {
                FileCodec::Current
            },
        })
    }

    /// **The file now** (exports: "now" resolved first): the current doc's bytes, as a
    /// write-back writes them, and the doc version they are. Needs read only. Reads only: no
    /// read base is recorded and "contributors since last read" doesn't move.
    pub fn file_now(&self, conn: ConnId) -> Result<FileBytes> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Read, Action::Read, now)?;
        Ok(FileBytes {
            bytes: self.render(&st)?,
            version: st.version,
            codec: FileCodec::Exact,
        })
    }

    /// [`Session::file_at`], written to `out` ([`FileCopy`]: on an error, discard what was
    /// written). A live doc is rendered whole first (its size is bounded by the pack).
    pub fn file_at_to(
        &self,
        conn: ConnId,
        vid: VersionId,
        out: &mut dyn std::io::Write,
    ) -> Result<FileCopy> {
        copy_file(self.file_at(conn, vid)?, out)
    }

    /// [`Session::file_now`], written to `out` ([`FileCopy`]).
    pub fn file_now_to(&self, conn: ConnId, out: &mut dyn std::io::Write) -> Result<FileCopy> {
        copy_file(self.file_now(conn)?, out)
    }

    /// `diff --version V [--to W]`: a unified diff of `root` from version `from` to version
    /// `to` (`None`: the live text), with `context` lines. Exit 0 when the same, 1 when
    /// different ([`DiffOutput::exit`]). Needs history.
    pub fn diff_version(
        &self,
        conn: ConnId,
        root: &str,
        from: VersionId,
        to: Option<VersionId>,
        context: usize,
    ) -> Result<DiffOutput> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit_history(&mut st, conn, now)?;
        let name = st.key.path.clone();
        let store = self.store(&st)?.clone();
        let text_at = |vid: VersionId| -> Result<String> {
            let v = store.version(self.doc_id, vid).map_err(no_version(vid))?;
            Ok(v.content
                .into_iter()
                .find(|(r, _)| r == root)
                .map(|(_, Neutral::Text(t))| t)
                .unwrap_or_default())
        };
        let old = text_at(from)?;
        let (new, to_label) = match to {
            Some(v) => (text_at(v)?, format!("{name} (version {v})")),
            None => (
                self.text_of(&st, root)?.1.to_owned(),
                format!("{name} (live)"),
            ),
        };
        let differs = old != new;
        let text = if differs {
            crate::tools::unified(
                &old,
                &new,
                context,
                &format!("{name} (version {from})"),
                &to_label,
            )
        } else {
            String::new()
        };
        Ok(DiffOutput { text, differs })
    }

    /// `log`: changes and versions, newest first, filtered by `q` (K-HIST-9, -18). Each
    /// change has its author, via, time, ± lines, kind and approval state. Needs history.
    pub fn log(&self, conn: ConnId, q: &LogQuery) -> Result<Vec<LogItem>> {
        // an App viewer filters by its handles (`--by p2`): the pid behind each
        let q = self.unhandle_query(conn, q);
        let out = self.log_inner(conn, &q);
        self.for_viewer(conn, out, |l, items| {
            items.into_iter().map(|i| l.log_item(i)).collect()
        })
    }

    fn log_inner(&self, conn: ConnId, q: &LogQuery) -> Result<Vec<LogItem>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit_history(&mut st, conn, now)?;
        let store = self.store(&st)?.clone();
        let approvals: HashMap<Seq, ApprovalState> = store
            .approvals(self.doc_id)?
            .into_iter()
            .map(|(s, a)| (s, a.state))
            .collect();
        let limit = q.limit.unwrap_or(usize::MAX);
        let mut items = Vec::new();
        store.scan_changes(self.doc_id, |c| {
            let approval = approvals.get(&c.seq).copied();
            let keep = q.by.as_ref().map_or(true, |b| *b == c.author)
                && q.since_ms.map_or(true, |t| c.at_ms >= t)
                && q.on_behalf_of
                    .as_ref()
                    .map_or(true, |p| c.on_behalf_of.as_ref() == Some(p))
                && (!q.pending
                    || approval == Some(ApprovalState::Pending)
                    || (q.settled && approval.is_some()));
            if keep {
                items.push(LogItem::Change { info: c, approval });
            }
            items.len() < limit
        })?;
        if q.on_behalf_of.is_none() && !q.pending {
            for v in store.version_infos(self.doc_id)? {
                if q.by.as_ref().map_or(true, |b| v.authors.contains(b))
                    && q.since_ms.map_or(true, |t| v.at_ms >= t)
                {
                    items.push(LogItem::Version(v));
                }
            }
        }
        items.sort_by_key(|x| std::cmp::Reverse(x.key()));
        items.truncate(limit);
        Ok(items)
    }

    /// `version --name N`: name version `vid`, or (`None`) cut a named version of the live
    /// doc. Named versions are never removed by retention (K-HIST-6). Needs edit.
    pub fn name_version(
        &self,
        conn: ConnId,
        name: &str,
        vid: Option<VersionId>,
    ) -> Result<VersionId> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::Edit, Action::Versions, now)?;
        if name.is_empty() || name.len() > 256 {
            return Err(SessionError::BadEdit(
                "a version name is 1 to 256 bytes".into(),
            ));
        }
        let store = self.store(&st)?.clone();
        if let Some(v) = vid {
            store
                .name_version(self.doc_id, v, name)
                .map_err(no_version(v))?;
            return Ok(v);
        }
        let v = Self::version_of(
            &st,
            st.version,
            VersionReason::Named,
            Some(name.to_owned()),
            &st.texts,
            now,
        );
        let id = store.put_version(self.doc_id, &v)?;
        st.last_version_seq = st.version;
        Ok(id)
    }

    /// `restore --version V`: make the doc equal version `vid` again as a **new change by
    /// the caller** (history is never rewritten), after cutting a version of the doc as it
    /// is (K-HIST-8). The restore is the caller's batch: their `undo` reverses it. Needs
    /// edit and history.
    pub fn restore(&self, conn: ConnId, vid: VersionId) -> Result<EditResult> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Restore, now)?;
        self.admit(&mut st, conn, Need::History, Action::Restore, now)?;
        self.check_backlog(&st, i)?;
        let target = self
            .store(&st)?
            .version(self.doc_id, vid)
            .map_err(no_version(vid))?;
        let mut edits = Vec::new();
        for (root, live) in &st.texts {
            let Some((_, Neutral::Text(t))) = target.content.iter().find(|(r, _)| r == root) else {
                continue;
            };
            let t = self.pack.normalize_input(root, t);
            let hunks = lines::align_insertions(live, diff::hunks(live, &t));
            if !hunks.is_empty() {
                edits.push(Edit::Text {
                    root: (*root).to_owned(),
                    hunks,
                });
            }
        }
        let mut versions = Vec::new();
        if !edits.is_empty() && st.last_version_seq != st.version {
            versions.push(Self::version_of(
                &st,
                st.version,
                VersionReason::BeforeRestore,
                None,
                &st.texts,
                now,
            ));
        }
        self.commit_edit(
            &mut st,
            i,
            edits,
            ChangeKind::Restore,
            Some(vid),
            now,
            versions,
        )
    }

    /// `revert <change>` (undo spec U22, K-HIST-19): reverse one named change from its
    /// persisted revert record, whoever made it and however long ago, **inside the revert
    /// window** (older: [`SessionError::RevertTooOld`], exit 8). Its regions are carried to
    /// the live text and merged as `undo`'s are: all or nothing, or with `partial` every
    /// clean region (a fully conflicting revert applies nothing either way). Allowed for the
    /// change's author, the person it was made for, and an owner ([`Rights::owner`]); needs
    /// edit. The revert is a new change by the caller, on their undo stack. Works after a
    /// restart: records and their carried bounds live in the store.
    ///
    /// **By root** (undo spec v1.8, U22): the record keeps the roots of the elements its
    /// change inserted and deleted, so the revert acts on the live elements whose root the
    /// change inserted (a change undone and redone has its roots live again: its revert
    /// applies), and what it puts back records the roots it restores. A record written
    /// before store format 5, or of an outside edit, has no roots: it is reverted as in v1.7.
    pub fn revert(&self, conn: ConnId, change: Seq, partial: bool) -> Result<UndoOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit_edit(&mut st, conn, Action::Revert, now)?;
        let st = &mut *st;
        let store = self.store(st)?.clone();
        let Some(rec) = store.revert_record(self.doc_id, change)? else {
            return Err(if change >= 1 && change <= st.version {
                SessionError::RevertTooOld { seq: change }
            } else {
                SessionError::NoSuchChange(change)
            });
        };
        let pid = st.members[i].p.pid.clone();
        let allowed = rec.author == pid
            || rec.on_behalf_of.as_deref() == Some(pid.as_str())
            || st.members[i].rights.owner;
        if !allowed {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid,
                action: Action::Revert,
                need: Need::AuthorOrOwner,
            });
            return Err(SessionError::Forbidden {
                need: Need::AuthorOrOwner,
            });
        }
        let edges = self
            .revert_index(st)?
            .edges(change)
            .ok_or(SessionError::RevertTooOld { seq: change })?;
        // theirs (U10, v1.7): the record's regions replayed through every later change's
        // shape, which the store keeps for the doc's life (the change's own shape too: a
        // purge blanks a redacted record's text, never its shape); the edges are the index's
        let shapes = store.change_shapes(self.doc_id, change.saturating_sub(1))?;
        let carried = match shapes.split_first() {
            Some(((seq, own), later)) if *seq == change => undo::replay_record(own, later),
            _ => Vec::new(),
        };
        debug_assert_eq!(
            carried.iter().map(undo::Carried::edges).collect::<Vec<_>>(),
            edges,
            "the replayed edges are the revert index's"
        );
        let carried = if carried
            .iter()
            .map(undo::Carried::edges)
            .eq(edges.iter().cloned())
        {
            carried
        } else {
            // never on a correct store: the index's bounds, and theirs unknown, so every
            // region conflicts (as tainted) rather than guess which elements are whose
            edges
                .into_iter()
                .map(|(root, start, end, _, _)| undo::Carried {
                    root,
                    start,
                    end,
                    start_taint: true,
                    end_taint: true,
                    theirs: Vec::new(),
                })
                .collect()
        };
        let batch = Batch {
            seq: change,
            on_behalf_of: None,
            regions: undo::revert_regions(&rec, carried),
            touched: false,
        };
        let mode = if partial { Mode::Partial } else { Mode::All };
        self.prov(st)?;
        let client = st.members[i].client;
        let next = ElemId::new(client, st.doc.clock_of(client));
        let prov = st.prov.as_ref().expect("just built");
        let step = match undo::plan(&st.texts, prov, next, vec![batch], mode) {
            Plan::Conflict { step, conflicts } => {
                return Ok(UndoOutcome::Conflicts { step, conflicts })
            }
            Plan::Steps { mut steps, blocked } => match steps.pop() {
                Some(s) => s,
                None => {
                    return Ok(UndoOutcome::Blocked {
                        conflicts: blocked.unwrap_or_default(),
                    })
                }
            },
        };
        // what it puts back records the roots it restores (U23)
        let result = self.commit_edit_restoring(
            st,
            i,
            step.edits,
            Some(step.restores),
            ChangeKind::Revert,
            Some(change),
            now,
            Vec::new(),
        )?;
        Ok(UndoOutcome::Applied {
            steps: 1,
            result,
            skipped: step.skipped,
            blocked: None,
        })
    }

    /// The revert index, built from the store on first use.
    fn revert_index<'a>(&self, st: &'a mut State) -> Result<&'a mut RevertIndex> {
        if st.reverts.is_none() {
            let ix = RevertIndex::build(self.store(st)?, self.doc_id)?;
            st.reverts = Some(ix);
        }
        Ok(st.reverts.as_mut().expect("just built"))
    }

    /// The history duties of [`super::super::SessionManager::tick`]: an idle version
    /// ([`Config::idle_version_ms`] after the last change), and every
    /// [`Config::history_tick_ms`] version retention (K-HIST-7) and the revert window
    /// (K-HIST-23).
    pub(crate) fn history_tick(&self) -> Result<HistoryTick> {
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        let c = self.config.clone();
        let store = self.store(st)?.clone();
        let mut rep = HistoryTick::default();
        if st.version != st.last_version_seq
            && now.saturating_sub(st.last_change_ms) >= c.idle_version_ms
        {
            let v = Self::version_of(st, st.version, VersionReason::Idle, None, &st.texts, now);
            rep.cut = Some(store.put_version(self.doc_id, &v)?);
            st.last_version_seq = st.version;
        }
        if now < st.next_history_ms {
            return Ok(rep);
        }
        st.next_history_ms = now.saturating_add(c.history_tick_ms);
        let retention = Retention {
            keep_all_ms: c.version_keep_all_ms,
            hourly_until_ms: c.version_hourly_until_ms,
            max_count: c.version_max_count,
            max_bytes: c.version_max_bytes,
        };
        rep.thinned = store.thin_versions(self.doc_id, now, &retention)?.len();
        rep.reverts_dropped = self.prune_reverts(st, now)?;
        Ok(rep)
    }

    /// Move the revert window: drop the records outside it, persisting the carried bounds
    /// of the pending records older than the cut, each edge's taint apart (so a rebuild
    /// never replays past it, and equals the live index).
    fn prune_reverts(&self, st: &mut State, now: u64) -> Result<u64> {
        let c = &self.config;
        let store = self.store(st)?.clone();
        let stats = store.revert_stats(self.doc_id)?;
        if stats.count <= c.revert_keep_changes && stats.bytes <= c.revert_max_bytes {
            return Ok(0);
        }
        let index = store.revert_index(self.doc_id)?;
        let pending: HashSet<Seq> = store
            .approvals(self.doc_id)?
            .into_iter()
            .filter(|(_, a)| a.state == ApprovalState::Pending)
            .map(|(s, _)| s)
            .collect();
        let window = RevertWindow {
            keep_ms: c.revert_keep_ms,
            keep_changes: c.revert_keep_changes,
            max_bytes: c.revert_max_bytes,
        };
        let drop = window_plan(&index, &pending, st.version, now, &window);
        let Some(&cut) = drop.iter().max() else {
            return Ok(0);
        };
        let head = st.version;
        let ix = self.revert_index(st)?;
        let mut carries = Vec::new();
        for &(seq, ..) in index
            .iter()
            .filter(|(s, ..)| *s < cut && pending.contains(s))
        {
            // each edge's taint apart (U22a): a rebuild from it equals the live index, so
            // a restart changes no revert's outcome
            if let Some(b) = ix.edges(seq) {
                let regions = b
                    .into_iter()
                    .map(|(r, s, e, ts, te)| (r, s as u64, e as u64, ts, te))
                    .collect();
                carries.push((
                    seq,
                    Carry {
                        carried_to: head,
                        regions,
                    },
                ));
            }
        }
        let (n, _) = store.prune_reverts(self.doc_id, &drop, &carries)?;
        ix.remove(&drop);
        Ok(n)
    }
}

fn copy_file(f: FileBytes, out: &mut dyn std::io::Write) -> Result<FileCopy> {
    out.write_all(&f.bytes)
        .and_then(|()| out.flush())
        .map_err(|e| {
            SessionError::Blob(crate::blob::BlobError::Io(format!("writing the copy: {e}")))
        })?;
    Ok(FileCopy {
        version: f.version,
        bytes: f.bytes.len() as u64,
        codec: f.codec,
    })
}
