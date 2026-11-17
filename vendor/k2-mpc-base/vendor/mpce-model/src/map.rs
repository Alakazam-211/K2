//! Maps from string keys (the reference model), with a Quint model of the same rules.
//!
//! - **Per key** (`mpce_reg::key`): the grow-only set of the key's writes (set a value, set a
//!   new nested root, delete) and a pure LWW read: the winner is the highest `(lamport, agent)`
//!   write; the key is live iff the winner is not a delete, so a later set resurrects a deleted
//!   key; replacing a nested root is an LWW set like any other.
//! - **Keys are independent** (M2): each key's state is its own value here, and an op names one
//!   key. Nothing reads across keys.
//! - **Nested roots**: a root is an opaque ID (its write's event), whose content is a grow-only
//!   set of edits ("nest" events). An edit targets the root its editor saw at the key, and stays
//!   with that root whether or not the root is replaced; an edit concurrent with replacing its
//!   root is not shown, and the applied stream reports it.

use crate::text::Id;
use mpce_reg::key::{read_key_with, KeyKind, KeyReg, KeyRule, KeyWrite};
use mpce_reg::RegError;
use std::collections::{BTreeMap, BTreeSet};

pub type Value = String;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KOp {
    Set { key: String, value: Value },
    /// a new nested root at `key` (its ID is this event's)
    Root { key: String },
    Delete { key: String },
    /// an edit inside nested root `target`, the root its editor saw at `key`
    Nest { key: String, target: Id },
}

impl KOp {
    pub fn key(&self) -> &str {
        match self {
            KOp::Set { key, .. } | KOp::Root { key } | KOp::Delete { key } | KOp::Nest { key, .. } => key,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Event {
    pub id: Id,
    pub parents: BTreeSet<Id>,
    pub lamport: u64,
    pub op: KOp,
    /// the write an undo, redo or revert puts back
    pub restores_write: Option<Id>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KError {
    MissingDeps,
    BadLamport,
    Conflict,
    /// a delete of a key that is not live, or an edit of a key with no live nested root
    NotApplicable,
    /// an edit whose target is not a known nested root
    UnknownRoot,
    Reg(RegError),
}

impl From<RegError> for KError {
    fn from(e: RegError) -> Self {
        KError::Reg(e)
    }
}

/// a key's winner as a cell (the Quint model's `Cell`)
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Cell {
    pub key: String,
    pub ts: (u64, u64),
    pub w: Id,
    pub live: bool,
    pub root: bool,
    pub value: Option<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum KOpKind {
    Set,
    Root,
    Delete,
    Nest,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Effect {
    Visible,
    None,
}

/// One entry of the applied stream for maps.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Applied {
    pub kind: KOpKind,
    pub key: String,
    pub event: Id,
    /// the key's winner before and after (`None`: never written)
    pub before: Option<Cell>,
    pub after: Option<Cell>,
    pub effect: Effect,
    /// `winner(map, key)` after the op
    pub winner: Option<Id>,
    /// an edit whose root is no longer the live root at its key: kept with
    /// its root, not shown
    pub into_replaced_root: bool,
    pub local: bool,
}

/// One replica.
#[derive(Clone, Debug)]
pub struct Doc {
    rule: KeyRule,
    events: BTreeMap<Id, Event>,
    log: Vec<Id>,
    frontier: BTreeSet<Id>,
    keys: BTreeMap<String, KeyReg<Id, Value>>,
    nested: BTreeMap<Id, BTreeSet<Id>>,
    stream: Vec<Applied>,
}

impl Default for Doc {
    fn default() -> Self {
        Doc::new()
    }
}

/// the visible map: live keys -> (winning write, its nested root's content if it is a root)
pub type MapView = BTreeMap<String, (Id, Option<BTreeSet<Id>>)>;

impl Doc {
    pub fn new() -> Doc {
        Doc::with_rule(KeyRule::Correct)
    }

    /// a replica with an explicit read rule (anything but `Correct` is a mutant)
    pub fn with_rule(rule: KeyRule) -> Doc {
        Doc { rule, events: BTreeMap::new(), log: Vec::new(), frontier: BTreeSet::new(), keys: BTreeMap::new(), nested: BTreeMap::new(), stream: Vec::new() }
    }

    pub fn frontier(&self) -> &BTreeSet<Id> {
        &self.frontier
    }
    pub fn has(&self, id: Id) -> bool {
        self.events.contains_key(&id)
    }
    pub fn event_ids(&self) -> BTreeSet<Id> {
        self.events.keys().copied().collect()
    }
    pub fn log(&self) -> Vec<Event> {
        self.log.iter().map(|id| self.events[id].clone()).collect()
    }

    fn lamport_of(&self, parents: &BTreeSet<Id>) -> Option<u64> {
        let mut m: Option<u64> = None;
        for p in parents {
            let l = self.events.get(p)?.lamport;
            m = Some(m.map_or(l, |x: u64| x.max(l)));
        }
        Some(m.map_or(0, |x| x + 1))
    }

    pub fn fresh_id(&self, agent: u64) -> Id {
        let next = self.events.range(Id::new(agent, 0)..=Id::new(agent, u64::MAX)).next_back().map_or(0, |(id, _)| id.seq + 1);
        Id::new(agent, next)
    }

    /// the key's winning write, as a cell
    pub fn cell(&self, key: &str) -> Option<Cell> {
        let w = read_key_with(self.keys.get(key)?, self.rule)?;
        Some(Cell {
            key: key.to_string(),
            ts: w.ts(),
            w: w.id,
            live: w.live(),
            root: w.kind == KeyKind::Root,
            value: match &w.kind {
                KeyKind::Set(v) => Some(v.clone()),
                _ => None,
            },
        })
    }

    /// every written key's cell (one per key)
    pub fn cells(&self) -> Vec<Cell> {
        self.keys.keys().filter_map(|k| self.cell(k)).collect()
    }

    /// `winner(map, key)`
    pub fn winner(&self, key: &str) -> Option<Id> {
        self.cell(key).map(|c| c.w)
    }

    /// `winner_resolved(map, key)`
    pub fn winner_resolved(&self, key: &str) -> Option<Id> {
        let w = self.winner(key)?;
        Some(self.events.get(&w).and_then(|e| e.restores_write).unwrap_or(w))
    }

    pub fn is_live(&self, key: &str) -> bool {
        self.cell(key).is_some_and(|c| c.live)
    }

    /// the live nested root at `key`
    pub fn root_at(&self, key: &str) -> Option<Id> {
        self.cell(key).filter(|c| c.live && c.root).map(|c| c.w)
    }

    /// a nested root's edits (every root received has an entry)
    pub fn nested(&self) -> &BTreeMap<Id, BTreeSet<Id>> {
        &self.nested
    }

    /// the key's write set (the per-key register state)
    pub fn key_reg(&self, key: &str) -> Option<&KeyReg<Id, Value>> {
        self.keys.get(key)
    }

    pub fn view(&self) -> MapView {
        self.keys
            .keys()
            .filter_map(|k| self.cell(k).filter(|c| c.live).map(|c| (k.clone(), (c.w, if c.root { Some(self.nested.get(&c.w).cloned().unwrap_or_default()) } else { None }))))
            .collect()
    }

    pub fn stream(&self) -> &[Applied] {
        &self.stream
    }
    pub fn take_stream(&mut self) -> Vec<Applied> {
        std::mem::take(&mut self.stream)
    }

    pub fn deliver(&mut self, e: &Event) -> Result<bool, KError> {
        self.integrate(e, false)
    }

    fn integrate(&mut self, e: &Event, local: bool) -> Result<bool, KError> {
        if let Some(old) = self.events.get(&e.id) {
            return if old == e { Ok(false) } else { Err(KError::Conflict) };
        }
        if !e.parents.iter().all(|p| self.events.contains_key(p)) {
            return Err(KError::MissingDeps);
        }
        if self.lamport_of(&e.parents) != Some(e.lamport) {
            return Err(KError::BadLamport);
        }
        let key = e.op.key().to_string();
        let before = self.cell(&key);
        let view_before = self.view();
        let kw = |kind| KeyWrite { id: e.id, agent: e.id.agent, lamport: e.lamport, kind, restores_write: e.restores_write };
        let mut into_replaced = false;
        let kind = match &e.op {
            KOp::Set { value, .. } => {
                self.keys.entry(key.clone()).or_default().apply(kw(KeyKind::Set(value.clone())))?;
                KOpKind::Set
            }
            KOp::Root { .. } => {
                self.keys.entry(key.clone()).or_default().apply(kw(KeyKind::Root))?;
                self.nested.insert(e.id, BTreeSet::new());
                KOpKind::Root
            }
            KOp::Delete { .. } => {
                self.keys.entry(key.clone()).or_default().apply(kw(KeyKind::Delete))?;
                KOpKind::Delete
            }
            KOp::Nest { target, .. } => {
                // causal delivery: the target root is here; the edit stays with it
                let edits = self.nested.get_mut(target).ok_or(KError::UnknownRoot)?;
                edits.insert(e.id);
                into_replaced = self.root_at(&key) != Some(*target);
                KOpKind::Nest
            }
        };
        for p in &e.parents {
            self.frontier.remove(p);
        }
        self.frontier.insert(e.id);
        self.events.insert(e.id, e.clone());
        self.log.push(e.id);
        let view_after = self.view();
        self.stream.push(Applied {
            kind,
            key: key.clone(),
            event: e.id,
            before,
            after: self.cell(&key),
            effect: if view_before == view_after { Effect::None } else { Effect::Visible },
            winner: self.winner(&key),
            into_replaced_root: into_replaced,
            local,
        });
        Ok(true)
    }

    fn generate(&mut self, agent: u64, op: KOp, restores_write: Option<Id>) -> Result<Event, KError> {
        let e = Event { id: self.fresh_id(agent), parents: self.frontier.clone(), lamport: self.lamport_of(&self.frontier).expect("frontier known"), op, restores_write };
        self.integrate(&e, true)?;
        Ok(e)
    }

    pub fn set(&mut self, agent: u64, key: &str, value: &str) -> Result<Event, KError> {
        self.generate(agent, KOp::Set { key: key.to_string(), value: value.to_string() }, None)
    }
    /// a set carrying restore provenance
    pub fn set_restoring(&mut self, agent: u64, key: &str, value: &str, restores_write: Id) -> Result<Event, KError> {
        self.generate(agent, KOp::Set { key: key.to_string(), value: value.to_string() }, Some(restores_write))
    }
    pub fn set_root(&mut self, agent: u64, key: &str) -> Result<Event, KError> {
        self.generate(agent, KOp::Root { key: key.to_string() }, None)
    }
    /// delete a live key
    pub fn delete(&mut self, agent: u64, key: &str) -> Result<Event, KError> {
        if !self.is_live(key) {
            return Err(KError::NotApplicable);
        }
        self.generate(agent, KOp::Delete { key: key.to_string() }, None)
    }
    /// an edit inside the live nested root at `key`
    pub fn nest(&mut self, agent: u64, key: &str) -> Result<Event, KError> {
        let target = self.root_at(key).ok_or(KError::NotApplicable)?;
        self.generate(agent, KOp::Nest { key: key.to_string(), target }, None)
    }
}
