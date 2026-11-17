//! Movable lists (the reference model), composed from two layers that never see each other:
//! - the **order layer** (`mpce-order`): one FugueMax list of *slots*. A slot is the ID of an
//!   insert or a move; it is never removed. Inserts and moves add a slot; nothing else touches
//!   the order;
//! - the **register layer** (`mpce-reg`): per *element* (the ID of its insert), the grow-only
//!   set of its ops (insert, move, set, delete, undel, drop) and a pure `read` of it.
//!
//! This module is the only place that reads both, and only in the **view**: a slot is shown
//! iff it is its element's winning position and the element is live. Integration and apply
//! never cross: a slot op goes to the order (its placement) and to its element's set (as a
//! position write); a register op goes to its element's set alone. **Generation** reads the
//! view, as intended: an insert or a move picks its origins from what the user sees, and the
//! origins travel in the op.
//!
//! A Quint model of the same rules exists (with the register read): undel, drop, the applied
//! stream and `winner()`, and `restores_write`.
//! Events have `(agent, seq)` IDs, parents (the creator's frontier) and a lamport time
//! (1 + the parents' maximum, 0 for none); delivery is causal and idempotent.
//!
//! A move's target index counts positions without the moved element,
//! as the Quint model's `moveFrom`.

use crate::text::Id;
use crate::tree::{Origin, SiblingRule, TreeError};
use mpce_order::{Placement, SlotOrder};
use mpce_reg::{self as reg, Existence, Facet, Kind, Op as ROp, ReadRule, Reg, RegError, View};
use std::collections::{BTreeMap, BTreeSet};

pub type Value = String;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MOp {
    /// a new element (= this event's ID) at a new slot (= this ID)
    Insert { value: Value, place: Placement<Id> },
    /// element `elem` to a new slot (= this ID); `from`: the slot it was seen at
    Move { elem: Id, from: Id, place: Placement<Id> },
    /// a value write; `idx`: the element's visible index at its creator (provenance only: the
    /// correct model never reads it; the `RegisterReadsOrder` mutant does)
    Set { elem: Id, value: Value, idx: Option<usize> },
    Delete { elem: Id },
    Undel { elem: Id },
    Drop { elem: Id },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum OpKind {
    Insert,
    Move,
    Set,
    Delete,
    Undel,
    Drop,
}

impl OpKind {
    /// the facet an op of this kind writes (an insert writes all three; position is reported)
    pub fn facet(self) -> Facet {
        match self {
            OpKind::Insert | OpKind::Move => Facet::Position,
            OpKind::Set => Facet::Value,
            OpKind::Delete | OpKind::Undel | OpKind::Drop => Facet::Existence,
        }
    }
}

impl MOp {
    /// the element the op is about (an insert's element is its own event)
    pub fn elem(&self, id: Id) -> Id {
        match self {
            MOp::Insert { .. } => id,
            MOp::Move { elem, .. } | MOp::Set { elem, .. } | MOp::Delete { elem } | MOp::Undel { elem } | MOp::Drop { elem } => *elem,
        }
    }
    pub fn kind(&self) -> OpKind {
        match self {
            MOp::Insert { .. } => OpKind::Insert,
            MOp::Move { .. } => OpKind::Move,
            MOp::Set { .. } => OpKind::Set,
            MOp::Delete { .. } => OpKind::Delete,
            MOp::Undel { .. } => OpKind::Undel,
            MOp::Drop { .. } => OpKind::Drop,
        }
    }
    pub fn placement(&self) -> Option<&Placement<Id>> {
        match self {
            MOp::Insert { place, .. } | MOp::Move { place, .. } => Some(place),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Event {
    pub id: Id,
    pub parents: BTreeSet<Id>,
    pub lamport: u64,
    pub op: MOp,
    /// the write an undo, redo or revert puts back (stamped by the host)
    pub restores_write: Option<Id>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MError {
    /// a parent is not known yet (causal delivery)
    MissingDeps,
    /// an index beyond the visible (or hidden) list
    OutOfRange,
    /// an op on an element whose insert is not known
    UnknownElem,
    /// an event whose ID is known with different content
    Conflict,
    /// lamport is not 1 + the parents' maximum
    BadLamport,
    /// a version names an event this replica has not received
    UnknownVersion,
    /// an op the list kind does not have (list.rs)
    NotListOp,
    Order(TreeError),
    Reg(RegError),
}

impl From<TreeError> for MError {
    fn from(e: TreeError) -> Self {
        MError::Order(e)
    }
}
impl From<RegError> for MError {
    fn from(e: RegError) -> Self {
        MError::Reg(e)
    }
}

/// Composition variants. `Correct` is the model; `RegisterReadsOrder` is the planted negative
/// for the layer property test (`register-reads-order`: a set lands on the element at its
/// generation index in the receiver's visible list, so register apply reads the order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    Correct,
    RegisterReadsOrder,
}

/// A replica's configuration: the order layer's sibling rule, the register layer's read rule
/// and the composition variant (anything but FugueMax / Correct / Correct is a mutant).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub rule: SiblingRule,
    pub read: ReadRule,
    pub variant: Variant,
}

impl Default for Config {
    fn default() -> Self {
        Config { rule: SiblingRule::FugueMax, read: ReadRule::Correct, variant: Variant::Correct }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Effect {
    /// the visible list changed
    Visible,
    /// nothing visible changed (`effect: none`)
    None,
}

/// One entry of the applied stream: every op this replica applies, in apply
/// order, local and merged alike.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Applied {
    pub kind: OpKind,
    pub elem: Id,
    pub event: Id,
    pub index_before: Option<usize>,
    pub index_after: Option<usize>,
    /// the element's value register (shown or not); `None` before its insert
    pub value_before: Option<Value>,
    pub value_after: Option<Value>,
    pub existence_before: Option<Existence>,
    pub existence_after: Option<Existence>,
    pub effect: Effect,
    /// the winner of the facet the op writes, after it (`winner(elem, kind.facet())`)
    pub winner: Option<Id>,
    /// generated here (true) or merged (false)
    pub local: bool,
}

/// What a local op does, in terms of the view (generation reads the view).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Intent {
    /// insert `value` at visible index `index`
    Insert { index: usize, value: Value },
    /// move the visible element at `from` to index `to` of the list without it
    Move { from: usize, to: usize },
    Set { index: usize, value: Value },
    Delete { index: usize },
    /// restore the element at index `index` of the hidden (not dropped) elements, in slot order
    Undel { index: usize },
    /// drop a known, not yet dropped element
    Drop { elem: Id },
}

/// One replica.
#[derive(Clone, Debug)]
pub struct Doc {
    config: Config,
    events: BTreeMap<Id, Event>,
    /// delivery order (a causal order)
    log: Vec<Id>,
    frontier: BTreeSet<Id>,
    /// the order layer
    order: SlotOrder<Id>,
    /// the composition's own map: each slot's element
    slot_elem: BTreeMap<Id, Id>,
    /// the register layer: per element, the set of its ops
    regs: BTreeMap<Id, Reg<Id, Value>>,
    stream: Vec<Applied>,
    /// lowest next seq per agent, carried by `fork_at`
    seq_floor: BTreeMap<u64, u64>,
}

impl Default for Doc {
    fn default() -> Self {
        Doc::new()
    }
}

/// lamport(e) = 1 + max(lamport(parents)), 0 for no parents
fn lamport_of(events: &BTreeMap<Id, Event>, parents: &BTreeSet<Id>) -> Option<u64> {
    let mut m: Option<u64> = None;
    for p in parents {
        let l = events.get(p)?.lamport;
        m = Some(m.map_or(l, |x: u64| x.max(l)));
    }
    Some(m.map_or(0, |x| x + 1))
}

/// the causal past of a set of parents: everything reachable from them (they included)
fn closure_in(events: &BTreeMap<Id, Event>, roots: &BTreeSet<Id>) -> BTreeSet<Id> {
    let mut c = BTreeSet::new();
    let mut stack: Vec<Id> = roots.iter().copied().collect();
    while let Some(id) = stack.pop() {
        if c.insert(id) {
            if let Some(e) = events.get(&id) {
                stack.extend(e.parents.iter().copied());
            }
        }
    }
    c
}

impl Doc {
    pub fn new() -> Doc {
        Doc::with_config(Config::default())
    }

    pub fn with_config(config: Config) -> Doc {
        Doc {
            config,
            events: BTreeMap::new(),
            log: Vec::new(),
            frontier: BTreeSet::new(),
            order: SlotOrder::with_rule(config.rule),
            slot_elem: BTreeMap::new(),
            regs: BTreeMap::new(),
            stream: Vec::new(),
            seq_floor: BTreeMap::new(),
        }
    }

    pub fn config(&self) -> Config {
        self.config
    }

    // -------------------------------------------------------------------------------------
    // Event graph

    pub fn frontier(&self) -> &BTreeSet<Id> {
        &self.frontier
    }
    pub fn has(&self, id: Id) -> bool {
        self.events.contains_key(&id)
    }
    pub fn event(&self, id: Id) -> Option<&Event> {
        self.events.get(&id)
    }
    /// every event, in delivery order
    pub fn log(&self) -> Vec<Event> {
        self.log.iter().map(|id| self.events[id].clone()).collect()
    }
    pub fn event_ids(&self) -> BTreeSet<Id> {
        self.events.keys().copied().collect()
    }
    /// the causal past of `id` (not including it)
    pub fn past_of(&self, id: Id) -> BTreeSet<Id> {
        self.events.get(&id).map_or_else(BTreeSet::new, |e| closure_in(&self.events, &e.parents))
    }

    /// The next ID for `agent`: one past its highest known seq (and at least the fork floor).
    pub fn fresh_id(&self, agent: u64) -> Id {
        let next = self.events.range(Id::new(agent, 0)..=Id::new(agent, u64::MAX)).next_back().map_or(0, |(id, _)| id.seq + 1);
        Id::new(agent, next.max(self.seq_floor.get(&agent).copied().unwrap_or(0)))
    }

    // -------------------------------------------------------------------------------------
    // Delivery (integration and apply)

    /// Deliver an event (causal broadcast). Ok(true) if new, Ok(false) if already known (a
    /// no-op); `Conflict` for a known ID with different content. Atomic.
    pub fn deliver(&mut self, e: &Event) -> Result<bool, MError> {
        self.integrate(e, false)
    }

    fn integrate(&mut self, e: &Event, local: bool) -> Result<bool, MError> {
        if let Some(old) = self.events.get(&e.id) {
            return if old == e { Ok(false) } else { Err(MError::Conflict) };
        }
        if !e.parents.iter().all(|p| self.events.contains_key(p)) {
            return Err(MError::MissingDeps);
        }
        if lamport_of(&self.events, &e.parents) != Some(e.lamport) {
            return Err(MError::BadLamport);
        }
        let kind = e.op.kind();
        let mut elem = e.op.elem(e.id);
        if kind != OpKind::Insert && !self.regs.contains_key(&elem) {
            return Err(MError::UnknownElem);
        }
        let vis_before = self.visible();
        let view_before = self.view();
        // the mutant: a set lands on the element at its generation index in this replica's view
        if let (Variant::RegisterReadsOrder, MOp::Set { idx: Some(i), .. }) = (self.config.variant, &e.op) {
            if let Some(&x) = vis_before.get(*i) {
                elem = x;
            }
        }
        let (rd_before, ex_before) = match self.read(elem) {
            Some(v) => (Some(v.value), Some(v.existence)),
            None => (None, None),
        };
        // causality as data: the op's past, restricted to its element's ops (all of which are
        // here already, by causal delivery)
        let past: BTreeSet<Id> = match self.regs.get(&elem) {
            Some(r) => closure_in(&self.events, &e.parents).into_iter().filter(|x| r.contains(*x)).collect(),
            None => BTreeSet::new(),
        };
        let rkind = match &e.op {
            MOp::Insert { value, .. } => Kind::Insert(value.clone()),
            MOp::Move { .. } => Kind::Move,
            MOp::Set { value, .. } => Kind::Set(value.clone()),
            MOp::Delete { .. } => Kind::Delete,
            MOp::Undel { .. } => Kind::Undel,
            MOp::Drop { .. } => Kind::Drop,
        };
        let rop = ROp { id: e.id, agent: e.id.agent, lamport: e.lamport, past, kind: rkind, restores_write: e.restores_write };
        // order layer: slot ops only (a failure here leaves everything as it was)
        if let Some(p) = e.op.placement() {
            self.order.insert(e.id, p)?;
            self.slot_elem.insert(e.id, elem);
        }
        // register layer: the element's set (union; the ID is new, so this cannot conflict)
        self.regs.entry(elem).or_default().apply(rop)?;
        for p in &e.parents {
            self.frontier.remove(p);
        }
        self.frontier.insert(e.id);
        self.events.insert(e.id, e.clone());
        self.log.push(e.id);
        // the applied stream
        let vis_after = self.visible();
        let view_after = self.view();
        let rd_after = self.read(elem);
        self.stream.push(Applied {
            kind,
            elem,
            event: e.id,
            index_before: vis_before.iter().position(|&x| x == elem),
            index_after: vis_after.iter().position(|&x| x == elem),
            value_before: rd_before,
            value_after: rd_after.as_ref().map(|v| v.value.clone()),
            existence_before: ex_before,
            existence_after: rd_after.as_ref().map(|v| v.existence),
            effect: if view_before == view_after { Effect::None } else { Effect::Visible },
            winner: self.winner(elem, kind.facet()),
            local,
        });
        Ok(true)
    }

    /// Deliver a batch in a causal order (any input order, duplicates allowed). Atomic.
    /// Returns the number of new events.
    pub fn deliver_all(&mut self, events: &[Event]) -> Result<usize, MError> {
        let mut work = self.clone();
        let mut pending: Vec<&Event> = events.iter().collect();
        let mut n = 0;
        while !pending.is_empty() {
            let before = pending.len();
            let mut rest = Vec::new();
            for e in pending {
                if work.has(e.id) || e.parents.iter().all(|p| work.has(*p)) {
                    if work.deliver(e)? {
                        n += 1;
                    }
                } else {
                    rest.push(e);
                }
            }
            if rest.len() == before {
                return Err(MError::MissingDeps);
            }
            pending = rest;
        }
        *self = work;
        Ok(n)
    }

    /// Merge every event `other` has. Atomic.
    pub fn merge_from(&mut self, other: &Doc) -> Result<usize, MError> {
        self.deliver_all(&other.log())
    }

    /// A replica holding exactly the causal closure of `v` (an older version: a branch), with
    /// this replica's configuration; seqs continue past every seq known here.
    pub fn fork_at(&self, v: &BTreeSet<Id>) -> Result<Doc, MError> {
        if !v.iter().all(|x| self.has(*x)) {
            return Err(MError::UnknownVersion);
        }
        let c = closure_in(&self.events, v);
        let mut d = Doc::with_config(self.config);
        d.seq_floor = self.seq_floor.clone();
        for id in self.events.keys() {
            let f = d.seq_floor.entry(id.agent).or_insert(0);
            *f = (*f).max(id.seq + 1);
        }
        for id in self.log.iter().filter(|x| c.contains(x)) {
            d.deliver(&self.events[id])?;
        }
        d.stream.clear();
        Ok(d)
    }

    // -------------------------------------------------------------------------------------
    // Reads

    /// one element's read (the register layer), by this replica's read rule
    pub fn read(&self, elem: Id) -> Option<View<Id, Value>> {
        self.regs.get(&elem).and_then(|r| reg::read_with(r, self.config.read))
    }

    /// the element's op set (the register layer's state)
    pub fn reg(&self, elem: Id) -> Option<&Reg<Id, Value>> {
        self.regs.get(&elem)
    }

    /// the known elements (received inserts)
    pub fn elems(&self) -> BTreeSet<Id> {
        self.regs.iter().filter(|(_, r)| reg::read(r).is_some()).map(|(e, _)| *e).collect()
    }

    /// the element a slot belongs to
    pub fn elem_of_slot(&self, slot: Id) -> Option<Id> {
        self.slot_elem.get(&slot).copied()
    }

    /// every slot, tombstones included (the order layer's state)
    pub fn slots(&self) -> Vec<Id> {
        self.order.slots()
    }

    /// THE VIEW (the one place both layers are read): the slots in order, each shown iff it is
    /// its element's winning position and the element is live.
    pub fn visible_slots(&self) -> Vec<Id> {
        let reads: BTreeMap<Id, View<Id, Value>> = self.regs.keys().filter_map(|e| self.read(*e).map(|v| (*e, v))).collect();
        self.order.visible(|s| self.slot_elem.get(&s).and_then(|e| reads.get(e)).is_some_and(|v| v.pos == s && !v.hidden()))
    }

    /// the visible list, as elements
    pub fn visible(&self) -> Vec<Id> {
        self.visible_slots().into_iter().map(|s| self.slot_elem[&s]).collect()
    }

    /// the visible list as (element, value) pairs
    pub fn view(&self) -> Vec<(Id, Value)> {
        self.visible().into_iter().map(|e| (e, self.read(e).expect("a visible element is known").value)).collect()
    }

    /// the visible list as (element, winning value write, value)
    pub fn view_writes(&self) -> Vec<(Id, Id, Value)> {
        self.visible()
            .into_iter()
            .map(|e| {
                let v = self.read(e).expect("a visible element is known");
                (e, v.value_write, v.value)
            })
            .collect()
    }

    /// the slots of hidden (not dropped) elements, each at its position slot, in slot order
    pub fn hidden_slots(&self) -> Vec<Id> {
        let reads: BTreeMap<Id, View<Id, Value>> = self.regs.keys().filter_map(|e| self.read(*e).map(|v| (*e, v))).collect();
        self.order.visible(|s| self.slot_elem.get(&s).and_then(|e| reads.get(e)).is_some_and(|v| v.pos == s && v.existence == Existence::Hidden))
    }

    /// `winner(elem, facet)`
    pub fn winner(&self, elem: Id, facet: Facet) -> Option<Id> {
        let v = self.read(elem)?;
        Some(match facet {
            Facet::Value => v.value_write,
            Facet::Position => v.pos,
            Facet::Existence => v.existence_holder,
        })
    }

    /// `winner_resolved(elem, facet)`
    pub fn winner_resolved(&self, elem: Id, facet: Facet) -> Option<Id> {
        let w = self.winner(elem, facet)?;
        Some(self.events.get(&w).and_then(|e| e.restores_write).unwrap_or(w))
    }

    /// the element's maximal existence ops
    pub fn max_existence(&self, elem: Id) -> BTreeSet<Id> {
        self.regs.get(&elem).map_or_else(BTreeSet::new, reg::max_existence)
    }

    /// the applied stream so far
    pub fn stream(&self) -> &[Applied] {
        &self.stream
    }

    /// take the applied stream, leaving it empty
    pub fn take_stream(&mut self) -> Vec<Applied> {
        std::mem::take(&mut self.stream)
    }

    // -------------------------------------------------------------------------------------
    // Generation (reads the view)

    /// Make a local op as `agent` from the view, apply it here, and return it for delivery
    /// elsewhere. `restores_write`: restore provenance (the host stamps it).
    pub fn local(&mut self, agent: u64, intent: Intent, restores_write: Option<Id>) -> Result<Event, MError> {
        let vs = self.visible_slots();
        let at = |i: usize| vs.get(i).copied().ok_or(MError::OutOfRange);
        let op = match intent {
            Intent::Insert { index, value } => {
                if index > vs.len() {
                    return Err(MError::OutOfRange);
                }
                let left = if index == 0 { Origin::Start } else { Origin::Elem(vs[index - 1]) };
                MOp::Insert { value, place: self.order.place_after(left)? }
            }
            Intent::Move { from, to } => {
                let s = at(from)?;
                let others: Vec<Id> = vs.iter().copied().filter(|&t| t != s).collect();
                if to > others.len() {
                    return Err(MError::OutOfRange);
                }
                let left = if to == 0 { Origin::Start } else { Origin::Elem(others[to - 1]) };
                MOp::Move { elem: self.slot_elem[&s], from: s, place: self.order.place_after(left)? }
            }
            Intent::Set { index, value } => MOp::Set { elem: self.slot_elem[&at(index)?], value, idx: Some(index) },
            Intent::Delete { index } => MOp::Delete { elem: self.slot_elem[&at(index)?] },
            Intent::Undel { index } => {
                let hs = self.hidden_slots();
                MOp::Undel { elem: self.slot_elem[hs.get(index).ok_or(MError::OutOfRange)?] }
            }
            Intent::Drop { elem } => {
                if self.read(elem).is_none_or(|v| v.existence == Existence::Dropped) {
                    return Err(MError::UnknownElem);
                }
                MOp::Drop { elem }
            }
        };
        let e = Event {
            id: self.fresh_id(agent),
            parents: self.frontier.clone(),
            lamport: lamport_of(&self.events, &self.frontier).expect("the frontier is known"),
            op,
            restores_write,
        };
        self.integrate(&e, true)?;
        Ok(e)
    }

    pub fn insert(&mut self, agent: u64, index: usize, value: &str) -> Result<Event, MError> {
        self.local(agent, Intent::Insert { index, value: value.to_string() }, None)
    }
    pub fn move_elem(&mut self, agent: u64, from: usize, to: usize) -> Result<Event, MError> {
        self.local(agent, Intent::Move { from, to }, None)
    }
    pub fn set(&mut self, agent: u64, index: usize, value: &str) -> Result<Event, MError> {
        self.local(agent, Intent::Set { index, value: value.to_string() }, None)
    }
    pub fn delete(&mut self, agent: u64, index: usize) -> Result<Event, MError> {
        self.local(agent, Intent::Delete { index }, None)
    }
    pub fn undel(&mut self, agent: u64, hidden_index: usize) -> Result<Event, MError> {
        self.local(agent, Intent::Undel { index: hidden_index }, None)
    }
    pub fn drop_elem(&mut self, agent: u64, elem: Id) -> Result<Event, MError> {
        self.local(agent, Intent::Drop { elem }, None)
    }

    /// A branch merge: make an op as `agent` on the older version `v` of
    /// this replica (its view there; its parents are v's frontier), then integrate it here,
    /// where it meets what `v` lacks. Returns the event.
    pub fn branch(&mut self, agent: u64, v: &BTreeSet<Id>, intent: Intent, restores_write: Option<Id>) -> Result<Event, MError> {
        let mut old = self.fork_at(v)?;
        let e = old.local(agent, intent, restores_write)?;
        self.deliver(&e)?;
        Ok(e)
    }
}
