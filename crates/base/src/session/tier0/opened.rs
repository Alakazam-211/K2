//! The version a person's app opened, on a blob doc ([`BlobSession::note_opened`];
//! `session/opened.rs`): the pin in slot 3 of the doc's `blob_sync` rows, checked at every use,
//! and the write-back hold after a `FileNow` pin.

use super::*;
use crate::session::opened::{self as op, Opened, OpenedAt, OpenedVia};
use crate::store::OpenedRec;

impl BlobSession {
    /// **Note the version a person's app opened** the file at (`docs/SESSIONS.md`, "The version
    /// an app opened"), so a save from that app can merge from it. Needs edit.
    ///
    /// - [`OpenedAt::FileNow`] (the usual call, right before the app reads the file): the kit
    ///   checks the file first (an outside save it hasn't seen is taken in), writes back the
    ///   current version if the file doesn't hold it, then pins the version the file holds and
    ///   holds its next write-backs for [`Config::opened_hold_ms`] (until the first outside
    ///   save, at most), so the app reads that version.
    /// - [`OpenedAt::Version`]: a version the host handed the app itself.
    ///
    /// One pin per doc: a later call replaces it ([`AuditEvent::OpenedDropped`], `replaced`).
    /// An outside save taken in moves it to the save's version ([`OpenedVia::OutsideSave`]).
    /// It is kept with the doc's sync state, so a restart keeps it.
    ///
    /// Refused, nothing noted: a doc with no file, or whose file is missing
    /// ([`SessionError::OpenedNoFile`]); a version the doc never had
    /// ([`SessionError::OpenedUnknown`]); a purged one ([`SessionError::OpenedGone`]).
    pub fn note_opened(&self, conn: ConnId, at: OpenedAt) -> Result<Opened> {
        let r = self.note_opened_in(conn, at);
        self.shown(r)
    }

    fn note_opened_in(&self, conn: ConnId, at: OpenedAt) -> Result<Opened> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Edit, Action::Opened, now)?;
        if st.disk.is_none() {
            return Err(SessionError::OpenedNoFile);
        }
        let store = Self::store(st)?;
        let (version, blob, via) = match at {
            OpenedAt::FileNow => {
                // what the app reads is the file now: an unseen outside save first, then the
                // current version written if the file lacks it
                if !op::planted_skips_check() {
                    self.disk_check(st, now, true)?;
                    if let Some(d) = st.disk.as_mut() {
                        d.hold_until = None;
                    }
                    self.write_back(st, now)?;
                }
                let d = st.disk.as_ref().expect("has disk");
                if d.missing || d.declined.is_some() {
                    return Err(SessionError::OpenedNoFile);
                }
                let synced = d.synced;
                let vid = match st.cur.blob == synced {
                    true => st.vid,
                    false => store
                        .blob_versions(self.doc_id)?
                        .into_iter()
                        .rev()
                        .find(|(_, v)| v.blob == synced && !v.redacted)
                        .map(|(id, _)| id)
                        .ok_or(SessionError::OpenedGone { version: st.vid })?,
                };
                (vid, synced, OpenedVia::FileNow)
            }
            OpenedAt::Version(v) => {
                let found = match store.blob_version(self.doc_id, v) {
                    Ok(found) => found,
                    Err(crate::store::StoreError::NoVersion(..)) => {
                        return Err(SessionError::OpenedUnknown { version: v })
                    }
                    Err(e) => return Err(e.into()),
                };
                if found.redacted {
                    return Err(SessionError::OpenedGone { version: v });
                }
                (v, found.blob, OpenedVia::Version)
            }
        };
        let rec = OpenedRec {
            version,
            at_ms: now,
            via: via.code(),
            blob: Some(blob),
        };
        if let Some(old) = self.pin_raw(st)? {
            if old.version != version {
                self.emit(AuditEvent::OpenedDropped {
                    key: st.key.clone(),
                    version: old.version,
                    why: "replaced",
                });
            }
        }
        self.set_pin(st, Some(&rec))?;
        if via == OpenedVia::FileNow && self.config.opened_hold_ms > 0 {
            if let Some(d) = st.disk.as_mut() {
                d.hold_until = Some(now.saturating_add(self.config.opened_hold_ms));
            }
        }
        self.emit(AuditEvent::OpenedNoted {
            key: st.key.clone(),
            version,
        });
        Ok(Opened::of_rec(&rec))
    }

    /// **Drop the pin** ([`BlobSession::note_opened`]): the app was closed. A save then
    /// merges as it would with no pin. Needs edit. `Ok` with no pin too.
    pub fn clear_opened(&self, conn: ConnId) -> Result<()> {
        let r = self.clear_opened_in(conn);
        self.shown(r)
    }

    fn clear_opened_in(&self, conn: ConnId) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        self.admit(st, conn, Need::Edit, Action::Opened, now)?;
        if let Some(d) = st.disk.as_mut() {
            d.hold_until = None;
        }
        if let Some(old) = self.pin_raw(st)? {
            self.set_pin(st, None)?;
            self.emit(AuditEvent::OpenedDropped {
                key: st.key.clone(),
                version: old.version,
                why: "cleared",
            });
        }
        Ok(())
    }

    /// The doc's pin, checked: its version must still be held (not purged, the same blob);
    /// one that isn't is dropped here ([`AuditEvent::OpenedDropped`], `not_held`). Metadata
    /// only; no connection needed.
    pub fn opened(&self) -> Result<Option<Opened>> {
        let mut st = self.lock()?;
        Ok(self.pin_checked(&mut st)?.map(|r| Opened::of_rec(&r)))
    }

    /// The pin as stored (or, planted, as kept in memory), unchecked.
    pub(super) fn pin_raw(&self, st: &BState) -> Result<Option<OpenedRec>> {
        if op::planted_in_memory() {
            return Ok(st.opened_mem.clone());
        }
        Ok(Self::store(st)?.opened_rec(self.doc_id, true)?)
    }

    fn set_pin(&self, st: &mut BState, rec: Option<&OpenedRec>) -> Result<()> {
        if op::planted_in_memory() {
            st.opened_mem = rec.cloned();
            return Ok(());
        }
        Ok(Self::store(st)?.set_opened_rec(self.doc_id, true, rec)?)
    }

    /// [`BlobSession::pin_raw`], checked at use: the version is still there, not purged, and
    /// holds the pinned blob; else the pin is dropped and `None`.
    pub(super) fn pin_checked(&self, st: &mut BState) -> Result<Option<OpenedRec>> {
        let Some(rec) = self.pin_raw(st)? else {
            return Ok(None);
        };
        if op::planted_unchecked() {
            return Ok(Some(rec));
        }
        let held = match Self::store(st)?.blob_version(self.doc_id, rec.version) {
            Ok(v) => !v.redacted && Some(v.blob) == rec.blob,
            Err(crate::store::StoreError::NoVersion(..)) => false,
            Err(e) => return Err(e.into()),
        };
        if held {
            return Ok(Some(rec));
        }
        self.set_pin(st, None)?;
        self.emit(AuditEvent::OpenedDropped {
            key: st.key.clone(),
            version: rec.version,
            why: "not_held",
        });
        Ok(None)
    }

    /// An outside save holding blob `saved` was taken in: the hold ends, and a pin moves to
    /// the save's version (the app's buffer is its own save now). No pin, none made.
    pub(super) fn pin_follows_save(&self, st: &mut BState, saved: BlobId, now: u64) -> Result<()> {
        if let Some(d) = st.disk.as_mut() {
            d.hold_until = None;
        }
        if op::planted_never_moves() || self.pin_raw(st)?.is_none() {
            return Ok(());
        }
        let store = Self::store(st)?;
        let found = store
            .blob_versions(self.doc_id)?
            .into_iter()
            .rev()
            .find(|(_, v)| v.blob == saved && !v.redacted)
            .map(|(id, _)| id);
        let Some(version) = found else {
            return Ok(());
        };
        let rec = OpenedRec {
            version,
            at_ms: now,
            via: OpenedVia::OutsideSave.code(),
            blob: Some(saved),
        };
        self.set_pin(st, Some(&rec))
    }

    /// Whether write-backs wait for an app reading its pinned version (`FileNow`).
    pub(super) fn held(st: &BState, now: u64) -> bool {
        if op::planted_hold_ignored() {
            return false;
        }
        st.disk
            .as_ref()
            .and_then(|d| d.hold_until)
            .is_some_and(|t| now < t)
    }
}
