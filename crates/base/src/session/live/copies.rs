//! A live doc's **conflict copies**: the kit's version of a keyed doc that isn't a table, written
//! beside its file (or in the host's folder) when an outside save over the kit's own write kept
//! both (`grid.rs`, `ingest_structured`). Each copy is recorded in the store (`live_copies`),
//! and `tick` removes it after [`Config::conflict_copy_keep_ms`] on the terms tier 0 removes
//! its own: only a copy still holding exactly its kept version's content, never one
//! a person edited, nor one whose version (kept, or the save's) is named or purged.

use std::fs;

use super::*;
use crate::session::tier0::resolve_copy;
use crate::store::{LiveCopyRow, VersionId};

/// A recorded copy as a purge found it before it ran: the save's version, the record, and its
/// kept version's rendering then (what an untouched copy holds; `None` when unreadable).
pub(super) type CopyBefore = (VersionId, LiveCopyRow, Option<Vec<u8>>);

/// The planted fault [`crate::differential::KeyedMutant::PurgeSkipsLiveCopies`].
fn purge_skips_live_copies() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::PurgeSkipsLiveCopies)
    )
}

/// The planted fault [`crate::differential::KeyedMutant::CopyComparedWithEncode`].
fn copy_compared_with_encode() -> bool {
    plant_on!(
        crate::differential::keyed_mutant()
            == Some(crate::differential::KeyedMutant::CopyComparedWithEncode)
    )
}

impl Session {
    /// Stored version `vid` as the kit writes it (as the copy was written: its own codec
    /// settings, else the doc's; for a pack that keeps a carrier, spliced into the version's
    /// carrier); `None` when it can't be read or rendered.
    fn version_bytes(&self, st: &State, vid: VersionId) -> Option<Vec<u8>> {
        let v = self.store(st).ok()?.version(self.doc_id, vid).ok()?;
        self.version_file(st, vid, &v, copy_compared_with_encode())
            .ok()
            .flatten()
    }

    /// Before a purge changes the store: the doc's recorded conflict copies, each with its
    /// kept version's rendering as it is now ([`Session::purge_live_copies`] reads them after).
    pub(super) fn copies_before_purge(&self, st: &State) -> Result<Vec<CopyBefore>> {
        let rows = self.store(st)?.live_copies(self.doc_id)?;
        Ok(rows
            .into_iter()
            .map(|(vid, r)| {
                let bytes = self.version_bytes(st, r.kept);
                (vid, r, bytes)
            })
            .collect())
    }

    /// After a purge's store commit (K-PURGE, as tier 0's purge does for its copies): a
    /// recorded conflict copy that still holds exactly its kept version's content as it was
    /// before the purge, when the purge changed that version (its content redacted or gone),
    /// is deleted with its record and counted ([`PurgeCounts::conflict_copies`]). One whose
    /// bytes are anything else (a person edited it, or it can't be read) may hold purged
    /// content the kit can't vouch for: it is left and counted as
    /// [`NotReached::ConflictCopies`]. One the purge didn't touch holds nothing it erased and
    /// stays. Without the file's folder (no file), recorded copies are [`NotReached::File`].
    pub(super) fn purge_live_copies(
        &self,
        st: &State,
        before: Vec<CopyBefore>,
        counts: &mut crate::store::PurgeCounts,
        not_reached: &mut Vec<NotReached>,
    ) -> Result<()> {
        if before.is_empty() || purge_skips_live_copies() {
            return Ok(());
        }
        let Some(base) = Self::copy_base(st) else {
            not_reached.push(NotReached::File);
            return Ok(());
        };
        let store = self.store(st)?.clone();
        let (mut removed, mut left) = (0, 0);
        for (vid, r, was) in before {
            let path = resolve_copy(&base, &r.rec);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                left += 1;
                continue;
            }
            let held = fs::read(&path).ok();
            let untouched = was.is_some() && held == was;
            if !untouched {
                left += 1;
                continue;
            }
            let now = self.version_bytes(st, r.kept);
            if now == was {
                // the purge didn't touch what it holds
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                store.delete_live_copy(self.doc_id, vid)?;
                removed += 1;
            } else {
                left += 1;
            }
        }
        counts.conflict_copies += removed;
        if left > 0 {
            not_reached.push(NotReached::ConflictCopies(left));
        }
        Ok(())
    }

    /// Record the conflict copy `rec` written for the outside save `version`, holding version
    /// `kept`. A failure is reported as a disk error and leaves the copy unrecorded: it is then
    /// never removed (nothing lost).
    pub(super) fn record_live_copy(
        &self,
        st: &State,
        version: VersionId,
        kept: VersionId,
        rec: String,
        now: u64,
    ) {
        let row = LiveCopyRow {
            rec,
            kept,
            at_ms: now,
        };
        let put = self
            .store(st)
            .and_then(|s| Ok(s.put_live_copy(self.doc_id, version, &row)?));
        if let Err(e) = put {
            self.disk_error(st, "recording the conflict copy", e);
        }
    }

    /// the cleanup for a live doc (on `tick`, at most every
    /// [`Config::history_tick_ms`]): delete a recorded copy older than
    /// [`Config::conflict_copy_keep_ms`] when it is still a regular file holding exactly its
    /// kept version's content (the pack's rendering of that version, as the copy was written).
    /// Kept: a copy whose version (the kept one, or the save that made it) is named, purged or
    /// gone; a copy a person edited. A record whose copy is gone is forgotten. Returns the
    /// paths removed.
    pub(crate) fn copies_tick(&self) -> Result<Vec<PathBuf>> {
        // the host's clock, never `self.now()`: a tick is no use of the session (it would keep
        // an idle one loaded)
        let now = self.host.now_ms();
        let mut st = self.lock()?;
        let st = &mut *st;
        if st
            .last_copy_cleanup_ms
            .is_some_and(|t| now.saturating_sub(t) < self.config.history_tick_ms)
        {
            return Ok(Vec::new());
        }
        st.last_copy_cleanup_ms = Some(now);
        let store = self.store(st)?.clone();
        let rows = store.live_copies(self.doc_id)?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let Some(base) = Self::copy_base(st) else {
            return Ok(Vec::new());
        };
        let infos = store.version_infos(self.doc_id)?;
        // a version that is there, not named and not purged
        let plain = |id: VersionId| {
            infos
                .iter()
                .find(|v| v.id == id)
                .is_some_and(|v| v.name.is_none() && v.redacted.is_empty())
        };
        let mut removed = Vec::new();
        for (version, r) in rows {
            if now.saturating_sub(r.at_ms) < self.config.conflict_copy_keep_ms {
                continue;
            }
            let path = resolve_copy(&base, &r.rec);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                // gone (moved or deleted by someone): nothing left to remove
                store.delete_live_copy(self.doc_id, version)?;
                st.copies_edited.remove(&path);
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let stat = disk::FileStat::of(&meta);
            if st
                .copies_edited
                .get(&path)
                .is_some_and(|s| s.vouches_for(stat))
            {
                continue;
            }
            if !plain(r.kept) || !plain(version) {
                continue;
            }
            let Some(want) = self.version_bytes(st, r.kept) else {
                continue;
            };
            let read = fs::read(&path)
                .and_then(|b| Ok((b, disk::FileStat::of(&fs::symlink_metadata(&path)?))));
            match read {
                // unchanged while it was read: delete it
                Ok((b, after)) if b == want && after == stat => {
                    if fs::remove_file(&path).is_ok() {
                        store.delete_live_copy(self.doc_id, version)?;
                        st.copies_edited.remove(&path);
                        removed.push(path);
                    }
                }
                Ok((b, _)) if b != want => {
                    st.copies_edited.insert(path, disk::Seen::now(stat));
                }
                _ => {}
            }
        }
        Ok(removed)
    }
}
