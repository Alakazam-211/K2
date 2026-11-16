//! The version a person's app opened, on a live doc ([`Session::note_opened`];
//! `session/opened.rs`): the pin in slot 3 of the doc's `sync` rows, a stored version at its
//! change seq (so the base stays readable), checked at every use, and the write-back hold after
//! a `FileNow` pin.

use super::*;
use crate::doc::Edit;
use crate::elem::{Key, ListOp, MapOp, Value as ElemValue};
use crate::session::opened::{self as op, Opened, OpenedAt, OpenedVia};
use crate::session::Pack;
use crate::store::OpenedRec;

impl Session {
    /// **Note the version a person's app opened** the file at (`docs/SESSIONS.md`, "The version
    /// an app opened"), so a save from that app can merge from it. Needs edit.
    ///
    /// - [`OpenedAt::FileNow`] (the usual call, right before the app reads the file): the kit
    ///   checks the file first (an outside save it hasn't seen is taken in), writes the doc
    ///   back if the file doesn't hold it, then pins the change seq the file holds (cutting a
    ///   stored version there unless one is) and holds its next write-backs for
    ///   [`Config::opened_hold_ms`] (until the first outside save, at most).
    /// - [`OpenedAt::Version`]: a change seq the host handed the app itself; a stored version
    ///   must be there (or it is the doc's current one, and a version is cut).
    ///
    /// One pin per doc: a later call replaces it. An outside save taken in moves it to the
    /// save's change seq ([`OpenedVia::OutsideSave`]). It is kept with the doc's sync state, so
    /// a restart keeps it.
    ///
    /// Refused, nothing noted: a doc with no file, or whose file is missing or declined
    /// ([`SessionError::OpenedNoFile`]); a seq past the doc's ([`SessionError::OpenedUnknown`]);
    /// an earlier one with no stored version ([`SessionError::OpenedGone`]).
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
        let (version, via) = match at {
            OpenedAt::FileNow => {
                if !op::planted_skips_check() {
                    self.disk_check(st, now, true)?;
                    if let Some(d) = st.disk.as_mut() {
                        d.hold_until = None;
                    }
                    self.write_back(st, now)?;
                }
                let d = st.disk.as_ref().expect("has disk");
                if d.missing || st.declined.is_some() {
                    return Err(SessionError::OpenedNoFile);
                }
                (d.synced_seq, OpenedVia::FileNow)
            }
            OpenedAt::Version(v) => {
                if v > st.version {
                    return Err(SessionError::OpenedUnknown { version: v });
                }
                (v, OpenedVia::Version)
            }
        };
        if !self.version_held_at(st, version, now)? {
            return Err(SessionError::OpenedGone { version });
        }
        let rec = OpenedRec {
            version,
            at_ms: now,
            via: via.code(),
            blob: None,
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

    /// **Drop the pin** ([`Session::note_opened`]): the app was closed. A save then merges as
    /// it would with no pin. Needs edit. `Ok` with no pin too.
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

    /// The doc's pin, checked: a stored version must still be at its change seq; one that
    /// isn't is dropped here ([`AuditEvent::OpenedDropped`], `not_held`). Metadata only; no
    /// connection needed.
    pub fn opened(&self) -> Result<Option<Opened>> {
        let mut st = self.lock()?;
        Ok(self.pin_checked(&mut st)?.map(|r| Opened::of_rec(&r)))
    }

    /// Whether a stored version is at change seq `seq`; at the doc's current seq one is cut
    /// (`Idle`: a checkpoint) when none is.
    fn version_held_at(&self, st: &mut State, seq: Seq, now: u64) -> Result<bool> {
        let store = self.store(st)?.clone();
        if store
            .version_infos(self.doc_id)?
            .iter()
            .any(|v| v.seq == seq)
        {
            return Ok(true);
        }
        if seq != st.version {
            return Ok(false);
        }
        let snap = self.snapshot(st)?;
        let v = Self::version_of(st, st.version, VersionReason::Idle, None, snap, now);
        store.put_version(self.doc_id, &v)?;
        st.last_version_seq = st.version;
        Ok(true)
    }

    /// The pin as stored (or, planted, as kept in memory), unchecked.
    pub(super) fn pin_raw(&self, st: &State) -> Result<Option<OpenedRec>> {
        if op::planted_in_memory() {
            return Ok(st.opened_mem.clone());
        }
        Ok(self.store(st)?.opened_rec(self.doc_id, false)?)
    }

    fn set_pin(&self, st: &mut State, rec: Option<&OpenedRec>) -> Result<()> {
        if op::planted_in_memory() {
            st.opened_mem = rec.cloned();
            return Ok(());
        }
        Ok(self.store(st)?.set_opened_rec(self.doc_id, false, rec)?)
    }

    /// [`Session::pin_raw`], checked at use: a stored version is at its change seq; else the
    /// pin is dropped and `None`.
    pub(super) fn pin_checked(&self, st: &mut State) -> Result<Option<OpenedRec>> {
        let Some(rec) = self.pin_raw(st)? else {
            return Ok(None);
        };
        if op::planted_unchecked() {
            return Ok(Some(rec));
        }
        let held = self
            .store(st)?
            .version_infos(self.doc_id)?
            .iter()
            .any(|v| v.seq == rec.version);
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

    /// An outside save was taken in: the hold ends, and a pin moves to the save's change seq
    /// (the app's buffer is its own save now), a stored version cut there when the doc is at
    /// it; when it isn't (a merge left the doc ahead) and none is stored, the pin is dropped.
    /// No pin, none made.
    pub(super) fn pin_follows_save(&self, st: &mut State, now: u64) -> Result<()> {
        if let Some(d) = st.disk.as_mut() {
            d.hold_until = None;
        }
        if op::planted_never_moves() || self.pin_raw(st)?.is_none() {
            return Ok(());
        }
        let Some(seq) = st.disk.as_ref().map(|d| d.synced_seq) else {
            return Ok(());
        };
        if !self.version_held_at(st, seq, now)? {
            return self.set_pin(st, None);
        }
        let rec = OpenedRec {
            version: seq,
            at_ms: now,
            via: OpenedVia::OutsideSave.code(),
            blob: None,
        };
        self.set_pin(st, Some(&rec))
    }

    /// The version the app opened (the pin, checked), as a view of its stored version (one is
    /// kept at the pinned seq), for an outside save's merge base; `None` with no pin, or when
    /// that version was redacted by a purge.
    pub(super) fn pinned_view(&self, st: &mut State) -> Result<Option<Arc<View>>> {
        if op::planted_ignored() {
            return Ok(None);
        }
        let Some(rec) = self.pin_checked(st)? else {
            return Ok(None);
        };
        let store = self.store(st)?;
        let Some(vid) = store
            .version_infos(self.doc_id)?
            .into_iter()
            .find(|v| v.seq == rec.version && v.redacted.is_empty())
            .map(|v| v.id)
        else {
            return Ok(None);
        };
        let v = store.version(self.doc_id, vid)?;
        Ok(Some(Arc::new(View::of_content(&v.content))))
    }

    /// **A keyed doc's field merge**: the save's changes since the version the app opened
    /// (`pinned`, the pack's reconcile of the save against it), each kept where the live doc
    /// still holds the opened value (the kit didn't change it since), skipped where the live
    /// doc already holds the save's, and a **clash** anywhere else: a field both changed to
    /// different values, a list element's insert under a key the live doc already holds (a key
    /// minted against the opened version can't collide with a later live insert), an element
    /// deleted or moved that the kit changed or moved since, a restore or drop. The kept edits
    /// are checked by the pack on the merged doc ([`Pack::check_merged_view`]). `None`: a clash,
    /// a refusal, or a save that doesn't reconcile against the opened version: the save keeps
    /// both, as with no pin.
    pub(super) fn field_merge(
        &self,
        st: &mut State,
        pinned: &View,
        bytes: &[u8],
    ) -> Result<Option<Vec<Edit>>> {
        let Ok(rec) = self.pack.reconcile(pinned, bytes, &st.codec) else {
            return Ok(None);
        };
        let edits = self.normalize(rec.edits)?;
        let live = self.verb_view(st)?;
        let clash_merged = op::planted_clash_merged();
        let mut out = Vec::new();
        for e in edits {
            match e {
                Edit::Map { root, ops } => {
                    let mut keep = Vec::new();
                    for o in ops {
                        match map_op_fate(pinned, &live, &root, &o) {
                            Fate::Keep => keep.push(o),
                            Fate::Skip => {}
                            Fate::Clash if clash_merged => keep.push(o),
                            Fate::Clash => return Ok(None),
                        }
                    }
                    if !keep.is_empty() {
                        out.push(Edit::Map { root, ops: keep });
                    }
                }
                Edit::List { root, ops } => {
                    let mut keep = Vec::new();
                    for o in ops {
                        match list_op_fate(&*self.pack, pinned, &live, &root, &o) {
                            Fate::Keep => keep.push(o),
                            Fate::Skip => {}
                            Fate::Clash if clash_merged => keep.push(o),
                            Fate::Clash => return Ok(None),
                        }
                    }
                    if !keep.is_empty() {
                        out.push(Edit::List { root, ops: keep });
                    }
                }
                _ => return Ok(None),
            }
        }
        drop(live);
        if out.is_empty() || op::planted_view_check_skipped() {
            return Ok(Some(out));
        }
        let merged = self.live_with(st, &out)?;
        if self
            .pack
            .check_merged_view(&View::of_content(&merged))
            .is_err()
        {
            return Ok(None);
        }
        Ok(Some(out))
    }

    /// Whether write-backs wait for an app reading its pinned version (`FileNow`).
    pub(super) fn held(st: &State, now: u64) -> bool {
        if op::planted_hold_ignored() {
            return false;
        }
        st.disk
            .as_ref()
            .and_then(|d| d.hold_until)
            .is_some_and(|t| now < t)
    }
}

/// What the field merge does with one of the save's ops ([`Session::field_merge`]).
enum Fate {
    Keep,
    Skip,
    Clash,
}

/// A map op: kept where the live value is the opened one, skipped where it is the save's, a
/// clash otherwise.
fn map_op_fate(pinned: &View, live: &View, root: &str, o: &MapOp) -> Fate {
    let key = o.key();
    let base = pinned.map(root).and_then(|m| m.get(key));
    let cur = live.map(root).and_then(|m| m.get(key));
    let new = match o {
        MapOp::Set { value, .. } => Some(value),
        MapOp::Delete { .. } => None,
    };
    if cur == base {
        Fate::Keep
    } else if cur == new {
        Fate::Skip
    } else {
        Fate::Clash
    }
}

/// An element's value in a view (live or deleted), and whether it is live.
fn element(v: &View, root: &str, key: &Key) -> (Option<ElemValue>, bool) {
    if let Some((_, x)) = v.list(root).and_then(|l| l.iter().find(|(k, _)| k == key)) {
        return (Some(x.clone()), true);
    }
    (v.hidden(root).and_then(|h| h.get(key)).cloned(), false)
}

/// The element just before `key` in a view's live order (`None`: first, or not live).
fn before<'a>(v: &'a View, root: &str, key: &Key) -> Option<&'a Key> {
    let l = v.list(root)?;
    let i = l.iter().position(|(k, _)| k == key)?;
    i.checked_sub(1).map(|j| &l[j].0)
}

/// Whether the kit changed element `key`'s fields (in the pack's composite maps over `root`)
/// since the opened version.
fn fields_changed(pack: &dyn Pack, pinned: &View, live: &View, root: &str, key: &Key) -> bool {
    let prefix = format!("{}{}", key.as_str(), crate::table::CELL_SEP);
    pack.composite()
        .iter()
        .filter(|(_, l)| *l == root)
        .any(|(m, _)| {
            let of = |v: &View| -> Vec<(String, ElemValue)> {
                v.map(m)
                    .map(|map| {
                        map.range(prefix.clone()..)
                            .take_while(|(k, _)| k.starts_with(&prefix))
                            .map(|(k, x)| (k.clone(), x.clone()))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            of(pinned) != of(live)
        })
}

/// A list op: an insert under a key the live doc holds is a clash; a delete, a move or a value
/// set is kept only where the kit didn't change that element since the opened version; a
/// restore or a drop is a clash.
fn list_op_fate(pack: &dyn Pack, pinned: &View, live: &View, root: &str, o: &ListOp) -> Fate {
    let key = o.key();
    let (base, _) = element(pinned, root, key);
    let (cur, live_now) = element(live, root, key);
    match o {
        ListOp::Insert { .. } => match cur {
            Some(_) => Fate::Clash,
            None => Fate::Keep,
        },
        ListOp::Delete { .. } => {
            if !live_now {
                Fate::Skip
            } else if cur == base && !fields_changed(pack, pinned, live, root, key) {
                Fate::Keep
            } else {
                Fate::Clash
            }
        }
        ListOp::Move { .. } => {
            match live_now && before(pinned, root, key) == before(live, root, key) {
                true => Fate::Keep,
                false => Fate::Clash,
            }
        }
        ListOp::Set { value, .. } => {
            if cur == base {
                Fate::Keep
            } else if cur.as_ref() == Some(value) {
                Fate::Skip
            } else {
                Fate::Clash
            }
        }
        _ => Fate::Clash,
    }
}
