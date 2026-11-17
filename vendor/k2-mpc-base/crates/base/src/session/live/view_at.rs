//! **The view an edit leaves** ([`ViewAt`]): where a change is, by the addresses a person's view
//! and a still use. The containers and units are the ones the acting agent's presence moves to
//! ([`Session::verb_presence`]: the last container touched wins), from the same reads, so the
//! two can't drift; a text doc's are its changed lines and the pack's named spans
//! ([`crate::session::Pack::text_units`]); a grid verb's cells are the cells presence it gives
//! its agent. Worked out after the change applied, under its lock, only when the call asked for
//! its result or an agent's verb made it ([`ViewWant`]).

use std::collections::BTreeSet;

use super::*;
use crate::elem::{Applied as ElemApplied, Key, ListOpKind};
use crate::session::participant::{presence_address, TimePosition, ELEM_SELECTION_MAX};

/// One element a change touched, by address: `live` is false when the change deleted it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewUnit {
    pub address: String,
    pub live: bool,
}

/// Where a change is ([`EditResult::view`]), as of the version it made.
///
/// - `address`: the primary view address, what a still renders by default and what an agent's
///   view address moves to: with a 3D slot ([`crate::view_index::ViewSchema::space`]) the last
///   live unit touched, else the last container touched; a container the change deleted gives
///   the next live one in its list, else the previous one; a text doc's innermost named span
///   holding its first changed line ([`crate::session::Pack::text_units`]), else `L<first>`;
///   none when nothing it touched has a place.
/// - `containers`: every live container the change touched, in the order of its ops, each once
///   (the last is the focus); a text doc's spans.
/// - `units`: the elements the change touched in the list root of the last one, in order, at
///   most [`crate::session::ELEM_SELECTION_MAX`] (the last ones; the rest counted in `units_more`).
/// - `cells`: what a grid verb wrote, as its agent's cells presence.
/// - `lines`: a text change's line ranges, in the new text.
/// - `time`: the playhead the verb set ([`crate::tools::StructOutput::time_position`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewAt {
    pub address: Option<String>,
    pub containers: Vec<String>,
    pub units: Vec<ViewUnit>,
    pub units_more: usize,
    pub cells: Option<crate::grid::Cells>,
    pub lines: Vec<LineRange>,
    pub time: Option<TimePosition>,
}

impl ViewAt {
    /// `{address, containers, units, units_more, cells, lines, time}`: each line range with its
    /// address `L<first>`; the time as the `time_position` presence (`at_ms` left out).
    pub fn to_value(&self) -> crate::errors::Value {
        use crate::errors::Value;
        let units = self.units.iter().map(|u| {
            Value::obj([
                ("address", Value::from(u.address.as_str())),
                ("live", Value::from(u.live)),
            ])
        });
        let lines = self.lines.iter().map(|l| {
            Value::obj([
                ("root", Value::from(l.root.as_str())),
                ("first", Value::from(l.first)),
                ("last", Value::from(l.last)),
                ("address", Value::from(format!("L{}", l.first))),
            ])
        });
        let time = self.time.as_ref().map_or(Value::Null, |t| {
            Value::obj([
                ("at", Value::from(t.at())),
                ("state", Value::from(t.state().as_str())),
                ("in", Value::from(t.in_address())),
                ("rate", Value::from(t.rate_text())),
            ])
        });
        Value::obj([
            ("address", Value::from(self.address.as_deref())),
            (
                "containers",
                Value::list(self.containers.iter().map(String::as_str)),
            ),
            ("units", Value::List(units.collect())),
            ("units_more", Value::from(self.units_more)),
            (
                "cells",
                self.cells
                    .as_ref()
                    .map_or(Value::Null, crate::grid::Cells::to_value),
            ),
            ("lines", Value::List(lines.collect())),
            ("time", time),
        ])
    }
}

/// Who asked for the view of the change about to commit ([`State::next_view`]): the call's
/// result, and an agent's verb (whose view address follows it). Taken by the commit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ViewWant {
    pub(super) result: bool,
    pub(super) verb: bool,
}

impl ViewWant {
    pub(super) fn any(self) -> bool {
        self.result || self.verb
    }
}

/// Whether the view is planted as dropped from the change (test-only).
fn planted_view_dropped() -> bool {
    plant_on!(
        crate::differential::ergo_mutant() == Some(crate::differential::ErgoMutant::ViewDropped)
    )
}

/// Whether the view is planted as taking the first container touched, not the last (test-only).
fn planted_view_first_container() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::ViewFirstContainer)
    )
}

/// Whether a deleted unit is planted as listed live (test-only).
fn planted_view_deleted_live() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::ViewDeletedLive)
    )
}

/// Whether a person's verb is planted as moving their view address (test-only).
fn planted_view_moves_person() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::ViewMovesPerson)
    )
}

/// Whether a text change's view is planted as taking its outermost span (test-only).
fn planted_view_section_outer() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::ViewSectionOuter)
    )
}

impl Session {
    /// Whether element `key` of list root `root` is a container of the doc's view: the view
    /// schema's containers root, or (without one) an element [`Pack::container_of`] names as
    /// its own container.
    fn is_container(&self, root: &str, key: &Key) -> bool {
        match self.pack.view_schema() {
            Some(vs) => root == vs.containers,
            None => self
                .pack
                .container_of(root, key)
                .is_some_and(|(r, k)| r == root && &k == key),
        }
    }

    /// The view of a change that just applied (see [`ViewAt`]). `named`: the elements its
    /// edits name, in order ([`elem_presence::touched_in_order`]); `stream`: what it applied;
    /// `changed`: its line ranges; `cells`: a grid verb's cells presence.
    pub(super) fn view_at(
        &self,
        st: &State,
        named: &[(String, Key)],
        stream: &[ElemApplied],
        changed: &[LineRange],
        cells: Option<crate::grid::Cells>,
    ) -> ViewAt {
        let cells = cells.map(|mut c| {
            super::cells::address_cells(&*self.pack, &mut c);
            c
        });
        let mut v = ViewAt {
            lines: changed.to_vec(),
            cells,
            ..ViewAt::default()
        };
        if self.structured {
            self.view_of_elems(st, named, stream, &mut v);
        } else {
            self.view_of_text(st, changed, &mut v);
        }
        if v.address.is_none() {
            v.address = v
                .cells
                .as_ref()
                .map(|c| c.address.clone())
                .or_else(|| changed.first().map(|l| format!("L{}", l.first)))
                .or_else(|| v.units.last().map(|u| u.address.clone()));
        }
        v.address = v
            .address
            .and_then(|a| presence_address("the view address", &a).ok());
        v
    }

    /// [`Session::view_at`] on a keyed doc or a table: containers and units by the presence's
    /// reads, and the primary address by the slot.
    fn view_of_elems(
        &self,
        st: &State,
        named: &[(String, Key)],
        stream: &[ElemApplied],
        v: &mut ViewAt,
    ) {
        let landed = super::elem_claims::landed_elems(self.pack.composite(), stream);
        let touched: Vec<&(String, Key)> = named
            .iter()
            .filter(|(r, k)| landed.get(r.as_str()).is_some_and(|s| s.contains(k)))
            .collect();
        let Some((last_root, _)) = touched.last() else {
            return;
        };
        // a table's rows are units, never containers (its presence is its own)
        if !self.is_grid() {
            for (r, k) in &touched {
                if let Some(f) = self.container_focus(st, r, k) {
                    let a = self.pack.address_of(&f.root, &f.key);
                    v.containers.retain(|c| *c != a);
                    v.containers.push(a);
                }
            }
        }
        let mut units: Vec<ViewUnit> = touched
            .iter()
            .filter(|(r, _)| r == last_root)
            .map(|(r, k)| ViewUnit {
                address: self.pack.address_of(r, k),
                live: self.elem_live(st, r, k) || planted_view_deleted_live(),
            })
            .collect();
        if units.len() > ELEM_SELECTION_MAX {
            v.units_more = units.len() - ELEM_SELECTION_MAX;
            units.drain(..v.units_more);
        }
        v.units = units;
        let three_d = self.pack.view_schema().is_some_and(|vs| vs.space.is_some());
        let unit = v
            .units
            .iter()
            .rev()
            .find(|u| u.live)
            .map(|u| u.address.clone());
        let container = match planted_view_first_container() {
            true => v.containers.first().cloned(),
            false => v.containers.last().cloned(),
        };
        // a container the change deleted last: the live one now at its place (the next),
        // else the one before it, from the live view by index (no scan)
        let neighbour = touched.last().and_then(|(r, k)| {
            let at = stream.iter().rev().find_map(|a| match a {
                ElemApplied::List { root, ops } if root == r => ops.iter().rev().find_map(|o| {
                    let deleted = o.kind == ListOpKind::Delete && o.key == *k;
                    deleted.then_some(o.index_before).flatten()
                }),
                _ => None,
            })?;
            if !self.is_container(r, k) || self.elem_live(st, r, k) {
                return None;
            }
            let next = self.live_elem_at(st, r, at);
            let prev = at.checked_sub(1).and_then(|p| self.live_elem_at(st, r, p));
            next.or(prev).map(|to| self.pack.address_of(r, &to))
        });
        v.address = match three_d {
            true => unit.or(container),
            false => neighbour.or(container),
        };
    }

    /// [`Session::view_at`] on a text doc: its changed lines, and the pack's named spans
    /// holding them (the innermost holding each range's first line).
    fn view_of_text(&self, st: &State, changed: &[LineRange], v: &mut ViewAt) {
        let Some(first) = changed.first() else {
            return;
        };
        let Ok((_, text)) = self.text_of(st, &first.root) else {
            return;
        };
        let spans = self.pack.text_units(&first.root, text);
        if spans.is_empty() {
            return;
        }
        let outer = planted_view_section_outer();
        let innermost = |line: usize| {
            let holding = spans.iter().filter(|s| s.first <= line && line <= s.last);
            match outer {
                true => holding.max_by_key(|s| s.last - s.first),
                false => holding.min_by_key(|s| s.last - s.first),
            }
            .map(|s| s.address.clone())
        };
        let mut seen = BTreeSet::new();
        for l in changed.iter().filter(|l| l.root == first.root) {
            if let Some(a) = innermost(l.first) {
                if seen.insert(a.clone()) {
                    v.containers.push(a);
                }
            }
        }
        v.address = innermost(first.first);
    }

    /// The view of the change about to commit, if one was asked for, and an agent's verb's
    /// view address moved to it (a person's, a watcher's never).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn change_view(
        &self,
        st: &mut State,
        i: usize,
        want: ViewWant,
        named: &[(String, Key)],
        stream: &[ElemApplied],
        changed: &[LineRange],
        cells: Option<crate::grid::Cells>,
    ) -> Option<Box<ViewAt>> {
        let m = &st.members[i];
        let agent =
            (m.p.kind == ParticipantKind::Agent || planted_view_moves_person()) && !m.watcher;
        let presence = want.verb && agent;
        if !(want.result || presence) {
            return None;
        }
        let v = self.view_at(st, named, stream, changed, cells);
        if presence {
            if let Some(a) = &v.address {
                st.members[i].elem.verb_viewed(a.clone());
            }
        }
        (want.result && !planted_view_dropped()).then(|| Box::new(v))
    }
}
