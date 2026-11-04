//! A connection's element selection, pointer and viewport. A child of `live`, so it works on the
//! session's own state.
//!
//! All three need read only and stay outside the doc's numbering: nothing here takes or moves a
//! version, writes to the store, or marks the doc dirty. Subscribers see them through the roster
//! ([`RosterEvent::Selection`](super::roster::RosterEvent::Selection) and its two siblings), one
//! event per connection per flush, the last value.

use super::*;
use crate::elem::{ElemState, Key};
use crate::session::participant::{ElemSelection, Point, Rect, ELEM_SELECTION_MAX};

/// One connection's values.
#[derive(Default)]
pub(super) struct ElemPresence {
    selection: Option<ElemSelection>,
    pointer: Option<Point>,
    viewport: Option<Rect>,
}

/// Fill a roster row from its connections, most active first.
pub(super) fn fold(conns: &[&Member], entry: &mut RosterEntry) {
    entry.selection = conns.iter().find_map(|m| m.elem.selection.clone());
    entry.pointer = conns.iter().find_map(|m| m.elem.pointer);
    entry.viewport = conns.iter().find_map(|m| m.elem.viewport);
    for d in &mut entry.devices {
        if let Some(m) = conns.iter().find(|m| m.client == d.client) {
            d.selection = m.elem.selection.clone();
            d.pointer = m.elem.pointer;
            d.viewport = m.elem.viewport;
        }
    }
}

impl Session {
    /// Select elements by key, or clear with `None` (or no keys). Needs read. The root must be a
    /// MovableList and every key a live element of it ([`SessionError::BadPosition`] otherwise),
    /// with at most [`ELEM_SELECTION_MAX`] keys ([`SessionError::SelectionTooLarge`]); a refusal
    /// leaves the selection as it was. A repeated key counts once. Returns the selection as
    /// stored. It follows moves, and a key leaves it when its element stops being live, for good:
    /// bringing the element back doesn't select it again. A watcher is refused
    /// ([`SessionError::Watching`]).
    pub fn set_selection(
        &self,
        conn: ConnId,
        selection: Option<ElemSelection>,
    ) -> Result<Option<ElemSelection>> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Cursor, now)?;
        self.refuse_watcher(st, i)?;
        let selection = match selection {
            Some(s) if !s.keys.is_empty() => Some(self.checked_selection(st, s)?),
            _ => None,
        };
        let m = &mut st.members[i];
        if selection.is_some() {
            m.last_cursor_ms = Some(now);
        }
        m.elem.selection = selection.clone();
        Ok(selection)
    }

    /// Set this connection's pointer, or clear it with `None`. Needs read. Subscribers get the
    /// latest value at each roster flush, however many calls came between. A watcher is refused
    /// ([`SessionError::Watching`]).
    pub fn set_pointer(&self, conn: ConnId, pointer: Option<Point>) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::Cursor, now)?;
        self.refuse_watcher(st, i)?;
        st.members[i].elem.pointer = pointer;
        Ok(())
    }

    /// Set the area this connection is looking at, or clear it with `None`. Needs read. Delivered
    /// as [`Session::set_pointer`]. For a text doc's lines, [`Session::set_view`] is the call. A
    /// watcher is refused ([`SessionError::Watching`]).
    pub fn set_viewport(&self, conn: ConnId, viewport: Option<Rect>) -> Result<()> {
        let now = self.now();
        let mut st = self.lock()?;
        let st = &mut *st;
        let i = self.admit(st, conn, Need::Read, Action::View, now)?;
        self.refuse_watcher(st, i)?;
        st.members[i].elem.viewport = viewport;
        Ok(())
    }

    fn refuse_watcher(&self, st: &State, i: usize) -> Result<()> {
        if st.members[i].watcher {
            return Err(SessionError::Watching {
                max: self.config.participants_max,
            });
        }
        Ok(())
    }

    fn checked_selection(&self, st: &State, s: ElemSelection) -> Result<ElemSelection> {
        if s.keys.len() > ELEM_SELECTION_MAX {
            return Err(SessionError::SelectionTooLarge {
                len: s.keys.len(),
                max: ELEM_SELECTION_MAX,
            });
        }
        let is_list = self
            .schema
            .roots
            .iter()
            .any(|(r, k)| *r == s.root && *k == Kind::MovableList);
        if !is_list {
            return Err(SessionError::BadPosition(format!(
                "{:?} is not a list of elements in this doc",
                s.root
            )));
        }
        let mut keys: Vec<Key> = Vec::with_capacity(s.keys.len());
        for k in s.keys {
            if !matches!(st.doc.elem(&s.root, &k), Ok(ElemState::Live(_))) {
                return Err(SessionError::BadPosition(format!(
                    "element {k} of {:?} is not live (unknown or deleted)",
                    s.root
                )));
            }
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        Ok(ElemSelection::new(s.root, keys))
    }

    /// After anything that changed the doc: every selection drops the keys of elements that
    /// aren't live now, and a selection left with none is gone.
    pub(super) fn elem_presence_after(st: &mut State) {
        if planted_selection_keeps_deleted() {
            return;
        }
        let State { members, doc, .. } = st;
        for m in members.iter_mut() {
            let Some(s) = m.elem.selection.as_mut() else {
                continue;
            };
            let ElemSelection { root, keys } = s;
            keys.retain(|k| matches!(doc.elem(root, k), Ok(ElemState::Live(_))));
            if keys.is_empty() {
                m.elem.selection = None;
            }
        }
    }
}

/// Planted ([`crate::differential::SelectionMutant::KeepsDeleted`]): selections keep every key.
/// Without the `differential` feature there is no hook: `false`.
#[cfg(any(test, feature = "differential"))]
fn planted_selection_keeps_deleted() -> bool {
    crate::differential::selection_mutant()
        == Some(crate::differential::SelectionMutant::KeepsDeleted)
}

#[cfg(not(any(test, feature = "differential")))]
fn planted_selection_keeps_deleted() -> bool {
    false
}
