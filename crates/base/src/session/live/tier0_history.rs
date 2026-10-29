//! The **tier-0 history** of a text doc promoted from tier 0 (v0.9.2,
//! [`super::super::SessionManager::promote_blob_file`]): the blob versions the promotion
//! kept (content that didn't read as text, purged entries, and on-behalf versions whose
//! approval had no text change to live on), with their bytes, entries and approvals as
//! they were. A child of `live`, so it works on the session's own state.
//!
//! Reads need history (as `log`); the purge is owner only; settling an approval is the
//! person's (as on the blob doc). A doc never promoted has none: the listing is empty.

use std::fs::{self, File};

use super::*;
use crate::blob::BlobId;
use crate::session::tier0::{blob_log, resolve_copy, BlobApproval, BlobLogEntry, BlobRead};
use crate::store::{PurgeCounts, StoreError, VersionId};

impl Session {
    /// Let this session read and purge its tier-0 history (the manager's blob store).
    pub(crate) fn set_blob_store(&self, blobs: Arc<crate::blob::BlobStore>) {
        let _ = self.blobs.set(blobs);
    }

    fn blob_store(&self) -> Result<&Arc<crate::blob::BlobStore>> {
        self.blobs.get().ok_or(SessionError::NoBlobStore)
    }

    /// The file's folder (conflict copies recorded on tier-0 versions resolve against it).
    fn copy_base(st: &State) -> Option<PathBuf> {
        let d = st.disk.as_ref()?;
        match disk::probe(&d.path, &d.root) {
            Ok(Probe::File { real, .. }) => real.parent().map(Path::to_owned),
            _ => d.path.parent().map(Path::to_owned),
        }
    }

    /// The doc's **tier-0 history**, oldest first: the blob versions a promotion from tier 0
    /// kept, as the blob doc's `log` listed them (same ids, kinds, authors, times, names,
    /// purge state, conflict copies and approval states). Empty for a doc that was never a
    /// blob doc. Needs history.
    pub fn tier0_log(&self, conn: ConnId) -> Result<Vec<BlobLogEntry>> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::History, Action::History, now)?;
        let base = Self::copy_base(&st);
        blob_log(self.store(&st)?, self.doc_id, base.as_deref())
    }

    /// One tier-0 version's bytes, streaming and verified (as
    /// [`crate::session::BlobSession::read_version`]). Needs history.
    /// [`StoreError::PurgedVersion`] for a purged one, [`StoreError::NoVersion`] for one the
    /// tier-0 history doesn't hold (a version that became text is read with `show`),
    /// [`SessionError::NoBlobStore`] when the manager has no blob store.
    pub fn read_tier0_version(&self, conn: ConnId, vid: VersionId) -> Result<BlobRead> {
        let now = self.now();
        let mut st = self.lock()?;
        self.admit(&mut st, conn, Need::History, Action::Versions, now)?;
        let blobs = self.blob_store()?;
        let info = self.store(&st)?.blob_version(self.doc_id, vid)?;
        if info.redacted {
            return Err(StoreError::PurgedVersion(self.doc_id, vid).into());
        }
        let reader = blobs.reader(&st.key.space, &info.blob)?;
        Ok(BlobRead {
            version: vid,
            info,
            reader,
        })
    }

    /// Settle the approval of a tier-0 version an agent wrote on someone's behalf (the blob
    /// doc's [`crate::session::BlobSession::set_approval`] rule: only that person; once;
    /// the same state again returns the record). The text doc can't restore a non-text
    /// version, so the result has no way back (`restore_to: None`); `later` lists the
    /// tier-0 versions after it.
    pub fn set_tier0_approval(
        &self,
        conn: ConnId,
        vid: VersionId,
        state: ApprovalState,
    ) -> Result<BlobApproval> {
        let now = self.now();
        let mut st = self.lock()?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Approve, now)?;
        let pid = st.members[i].p.pid.clone();
        let store = self.store(&st)?.clone();
        let Some(cur) = store.blob_approval(self.doc_id, vid)? else {
            return Err(SessionError::NoApproval { seq: vid });
        };
        if cur.on_behalf_of != pid {
            self.emit(AuditEvent::Refused {
                key: st.key.clone(),
                pid,
                action: Action::Approve,
                need: Need::Approver,
            });
            return Err(SessionError::Forbidden {
                need: Need::Approver,
            });
        }
        if state == ApprovalState::Pending {
            return Err(SessionError::BadApproval(
                "an approval can only be settled as approved or declined".into(),
            ));
        }
        let later = store
            .blob_versions(self.doc_id)?
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| *id > vid)
            .collect();
        let out = |approval| BlobApproval {
            version: vid,
            approval,
            restore_to: None,
            later,
        };
        match cur.state {
            ApprovalState::Pending => {}
            s if s == state => return Ok(out(cur)),
            s => {
                return Err(SessionError::BadApproval(format!(
                    "version {vid} was already settled as {s:?}"
                )))
            }
        }
        let new = cur.settled(state, pid.clone(), now);
        store.set_blob_approval(self.doc_id, vid, &new)?;
        self.emit(AuditEvent::Approval {
            key: st.key.clone(),
            seq: vid,
            pid,
            state,
        });
        Ok(out(new))
    }

    /// **Purge** tier-0 version `version` (`Some`), or (`None`) the whole tier-0 history,
    /// as [`crate::session::BlobSession::purge`] does on a blob doc: each keeps its entry
    /// (who, when, why, size) with `redacted` set; a released blob nothing else uses is
    /// removed at once; recorded conflict copies holding purged bytes are deleted; the
    /// store file is rewritten. No version here is current, so none is refused. Owner only.
    /// Audited as [`AuditEvent::Purged`] with [`PurgedScope::BlobVersions`]. The text
    /// history is not touched ([`Session::purge`] does that, and its
    /// [`PurgeScope::History`] reaches the tier-0 history too).
    pub fn purge_tier0(&self, conn: ConnId, version: Option<VersionId>) -> Result<PurgeOutcome> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Owner, Action::Purge, now)?;
        let by = st.members[i].p.pid.clone();
        let store = self.store(st)?.clone();
        // a store holding a newer kit's data can't be rewritten: refuse before anything
        // changes (v0.9.5)
        store.check_rewritable()?;
        let all = store.blob_versions(self.doc_id)?;
        let vids: Vec<VersionId> = match version {
            Some(v) => {
                if !all.iter().any(|(id, _)| *id == v) {
                    return Err(StoreError::NoVersion(self.doc_id, v).into());
                }
                vec![v]
            }
            None => all
                .iter()
                .filter(|(_, v)| !v.redacted)
                .map(|(id, _)| *id)
                .collect(),
        };
        let mut counts = PurgeCounts::default();
        let mut not_reached = Vec::new();
        self.purge_tier0_rows(st, &store, &vids, now, &mut counts, &mut not_reached)?;
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
            scope: PurgedScope::BlobVersions { version },
            counts,
        });
        Ok(PurgeOutcome::of_blobs(counts, not_reached, rewrite))
    }

    /// From a text purge (before its store rewrite): a `History` purge (`whole`) also
    /// purges every tier-0 version; any other scope doesn't search them, and says so
    /// ([`NotReached::Tier0History`]) when some still hold content.
    pub(super) fn purge_kept_tier0(
        &self,
        st: &State,
        store: &Arc<DocStore>,
        whole: bool,
        now: u64,
        counts: &mut PurgeCounts,
        not_reached: &mut Vec<NotReached>,
    ) -> Result<()> {
        let live: Vec<VersionId> = store
            .blob_versions(self.doc_id)?
            .into_iter()
            .filter(|(_, v)| !v.redacted)
            .map(|(id, _)| id)
            .collect();
        if live.is_empty() {
            return Ok(());
        }
        if !whole {
            not_reached.push(NotReached::Tier0History(live.len() as u64));
            return Ok(());
        }
        self.purge_tier0_rows(st, store, &live, now, counts, not_reached)
    }

    /// Purge tier-0 versions `vids` in the store, remove the blobs nothing else uses and the
    /// conflict copies holding purged bytes.
    fn purge_tier0_rows(
        &self,
        st: &State,
        store: &Arc<DocStore>,
        vids: &[VersionId],
        now: u64,
        counts: &mut PurgeCounts,
        not_reached: &mut Vec<NotReached>,
    ) -> Result<()> {
        let before = store.blob_versions(self.doc_id)?;
        let (released, approvals) = store.blob_purge_versions(self.doc_id, vids, now)?;
        counts.blob_versions += vids.len() as u64;
        counts.approvals += approvals;
        let purged: Vec<BlobId> = released.iter().map(|r| r.1).collect();
        let free: Vec<(u64, BlobId, u64)> = released
            .iter()
            .filter(|r| r.3 == 0)
            .map(|r| (r.0, r.1, r.2))
            .collect();
        let shared = (released.len() - free.len()) as u64;
        if shared > 0 {
            not_reached.push(NotReached::SharedBlobs(shared));
        }
        let Ok(blobs) = self.blob_store() else {
            // the rows are purged; the bytes go with the blob store's GC
            if !free.is_empty() {
                not_reached.push(NotReached::Tier0History(free.len() as u64));
            }
            return Ok(());
        };
        counts.blobs_removed += blobs.remove_unreferenced_now(&free)?.0;
        // conflict copies the kit recorded on these versions, holding purged bytes
        let records: Vec<String> = before
            .iter()
            .filter_map(|(_, v)| v.conflict_copy.clone())
            .collect();
        if records.is_empty() {
            return Ok(());
        }
        let Some(base) = Self::copy_base(st) else {
            not_reached.push(NotReached::File);
            return Ok(());
        };
        let (mut removed, mut left) = (0, 0);
        for rec in records {
            let p = resolve_copy(&base, &rec);
            let Ok(mut f) = File::open(&p) else { continue };
            let id = blobs.id_of(&st.key.space, &mut f).ok().map(|x| x.0);
            drop(f);
            match id {
                Some(id) if purged.contains(&id) => {
                    if fs::remove_file(&p).is_ok() {
                        removed += 1;
                    } else {
                        left += 1;
                    }
                }
                _ => left += 1,
            }
        }
        counts.conflict_copies += removed;
        if left > 0 {
            not_reached.push(NotReached::ConflictCopies(left));
        }
        Ok(())
    }
}
