//! Lists of values (the reference model): insert, delete, set, with a Quint model of the same
//! rules ("a delete beats a concurrent set").
//!
//! A list is the movable list's composition ([`crate::mlist`]) with only these three ops: the
//! order layer holds one slot per element (its insert, never moved), the register layer holds
//! each element's insert, sets and deletes, and the view shows an element iff no delete of it
//! is received (a list has no undel, so the read's rule reduces to that). Concurrent sets are
//! LWW by `(lamport, agent)`, and every set stays in the element's set (history); a delete is
//! a tombstone that no concurrent set brings back, and the element's value register keeps
//! converging behind it.

use crate::mlist::{self, Applied, Config, Event, Intent, MError, MOp, Value};
use crate::text::Id;
use mpce_reg::{Facet, View};
use std::collections::BTreeSet;

/// One replica.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    inner: mlist::Doc,
}

impl Doc {
    pub fn new() -> Doc {
        Doc::default()
    }

    pub fn with_config(config: Config) -> Doc {
        Doc { inner: mlist::Doc::with_config(config) }
    }

    /// the composition underneath (read-only)
    pub fn as_mlist(&self) -> &mlist::Doc {
        &self.inner
    }

    /// Deliver an event: an insert, a delete or a set (anything else is `NotListOp`).
    pub fn deliver(&mut self, e: &Event) -> Result<bool, MError> {
        match e.op {
            MOp::Insert { .. } | MOp::Delete { .. } | MOp::Set { .. } => self.inner.deliver(e),
            _ => Err(MError::NotListOp),
        }
    }

    pub fn deliver_all(&mut self, events: &[Event]) -> Result<usize, MError> {
        if events.iter().any(|e| !matches!(e.op, MOp::Insert { .. } | MOp::Delete { .. } | MOp::Set { .. })) {
            return Err(MError::NotListOp);
        }
        self.inner.deliver_all(events)
    }

    pub fn insert(&mut self, agent: u64, index: usize, value: &str) -> Result<Event, MError> {
        self.inner.insert(agent, index, value)
    }
    pub fn delete(&mut self, agent: u64, index: usize) -> Result<Event, MError> {
        self.inner.delete(agent, index)
    }
    pub fn set(&mut self, agent: u64, index: usize, value: &str) -> Result<Event, MError> {
        self.inner.set(agent, index, value)
    }
    /// a set carrying restore provenance
    pub fn set_restoring(&mut self, agent: u64, index: usize, value: &str, restores_write: Id) -> Result<Event, MError> {
        self.inner.local(agent, Intent::Set { index, value: value.to_string() }, Some(restores_write))
    }

    pub fn frontier(&self) -> &BTreeSet<Id> {
        self.inner.frontier()
    }
    pub fn log(&self) -> Vec<Event> {
        self.inner.log()
    }
    pub fn event_ids(&self) -> BTreeSet<Id> {
        self.inner.event_ids()
    }
    /// every element's slot, tombstones included (an element's slot is its insert)
    pub fn list_with_tombstones(&self) -> Vec<Id> {
        self.inner.slots()
    }
    pub fn elems(&self) -> BTreeSet<Id> {
        self.inner.elems()
    }
    pub fn read(&self, elem: Id) -> Option<View<Id, Value>> {
        self.inner.read(elem)
    }
    pub fn visible(&self) -> Vec<Id> {
        self.inner.visible()
    }
    pub fn view(&self) -> Vec<(Id, Value)> {
        self.inner.view()
    }
    pub fn winner(&self, elem: Id, facet: Facet) -> Option<Id> {
        self.inner.winner(elem, facet)
    }
    pub fn winner_resolved(&self, elem: Id, facet: Facet) -> Option<Id> {
        self.inner.winner_resolved(elem, facet)
    }
    pub fn stream(&self) -> &[Applied] {
        self.inner.stream()
    }
    pub fn take_stream(&mut self) -> Vec<Applied> {
        self.inner.take_stream()
    }
}
