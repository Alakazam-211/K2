//! The register layer of the reference models: one element's state as a grow-only set of its
//! ops, plus a pure [`read`] of that set.
//!
//! Its shape:
//! - **L2a.** The per-element state is the set of the element's ops a replica has received,
//!   and nothing else ([`Reg`] holds only the set). Apply is set union, so the state is a
//!   function of the received set whatever the delivery order.
//! - **L3.** This crate knows nothing of slot order: it has no dependency on `mpce-order`
//!   (a layer check fails the build otherwise). Causality comes in as data: each op
//!   carries its causal past ([`Op::past`]).
//!
//! The read:
//! - **position** = the position write (insert or move) with the highest `(lamport, agent)`;
//! - **value** = the value write (insert or set) with the highest `(lamport, agent)`;
//! - **existence**:
//!   - *dropped* iff some drop is in the set (absorbing: no undel, before or after, revives it);
//!   - otherwise *hidden* iff some delete is not causally before an undel (so a delete
//!     concurrent with an undel wins), and *live* otherwise.
//!
//!   Sets and moves never touch existence: a delete beats any concurrent set or move whatever
//!   their timestamps (the list rule), and hiding keeps both registers.
//! - **winners** ([`winner`]): value and position are
//!   the winning writes above; the existence holder is the highest drop (dropped), the highest
//!   delete that no undel covers (hidden), or, live, the highest undel among those covering at
//!   least one delete (with no deletes at all: the highest undel), else the insert. *Covers*:
//!   the delete is causally before the undel. [`winner_resolved`] follows the holder's
//!   `restores_write`.
//! - **undo's own test** ([`restore_undo_is_own`]): undoing a restore is own only
//!   if every covering undel outside the undoer's batch is in the batch's causal past; that the
//!   holder is own is not enough (the planted rule [`OwnRule::ByHolderOnly`]).
//!
//! [`key`] is the map's per-key register in the same form: a grow-only set of a key's writes
//! and an LWW read.
#![forbid(unsafe_code)]

pub mod key;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

/// `(lamport, agent)`: the LWW order.
pub type Ts = (u64, u64);

/// What an op does to its element.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind<V> {
    /// creates the element: writes its first position and its first value (once per element)
    Insert(V),
    /// a position write (the new slot is the op's own ID)
    Move,
    /// a value write
    Set(V),
    Delete,
    /// restore: the same element, with the registers it converged to
    Undel,
    /// hidden forever
    Drop,
}

impl<V> Kind<V> {
    pub fn is_position(&self) -> bool {
        matches!(self, Kind::Insert(_) | Kind::Move)
    }
    pub fn is_value(&self) -> bool {
        matches!(self, Kind::Insert(_) | Kind::Set(_))
    }
    /// a delete or an undel (the ops the existence rule and the maximal set are about)
    pub fn is_existence(&self) -> bool {
        matches!(self, Kind::Delete | Kind::Undel)
    }
    pub fn value(&self) -> Option<&V> {
        match self {
            Kind::Insert(v) | Kind::Set(v) => Some(v),
            _ => None,
        }
    }
}

/// One op on one element.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Op<K: Ord, V> {
    pub id: K,
    /// the creating agent (the LWW tie-break)
    pub agent: u64,
    pub lamport: u64,
    /// the IDs of the ops causally before this one: at least every op of this element in its
    /// causal past (more is harmless; the read only asks about this element's ops)
    pub past: BTreeSet<K>,
    pub kind: Kind<V>,
    /// the write an undo, redo or revert puts back (path-compressed:
    /// the original write, never a chain)
    pub restores_write: Option<K>,
}

impl<K: Ord, V> Op<K, V> {
    pub fn ts(&self) -> Ts {
        (self.lamport, self.agent)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegError {
    /// an op whose ID is known with different content
    Conflict,
}

/// One element's state: the set of its ops received here. Nothing else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reg<K: Ord, V> {
    ops: BTreeMap<K, Op<K, V>>,
}

impl<K: Ord, V> Default for Reg<K, V> {
    fn default() -> Self {
        Reg { ops: BTreeMap::new() }
    }
}

impl<K: Ord + Copy + Debug, V: Clone + Eq + Debug> Reg<K, V> {
    pub fn new() -> Reg<K, V> {
        Reg::default()
    }

    /// Apply = set union. Ok(true) if the op is new, Ok(false) if it was already here.
    pub fn apply(&mut self, op: Op<K, V>) -> Result<bool, RegError> {
        match self.ops.get(&op.id) {
            Some(o) if *o == op => Ok(false),
            Some(_) => Err(RegError::Conflict),
            None => {
                self.ops.insert(op.id, op);
                Ok(true)
            }
        }
    }

    /// Union with another replica's set of this element's ops. Atomic.
    pub fn merge(&mut self, other: &Reg<K, V>) -> Result<usize, RegError> {
        let mut work = self.clone();
        let mut n = 0;
        for o in other.ops.values() {
            if work.apply(o.clone())? {
                n += 1;
            }
        }
        *self = work;
        Ok(n)
    }

    pub fn contains(&self, id: K) -> bool {
        self.ops.contains_key(&id)
    }

    pub fn get(&self, id: K) -> Option<&Op<K, V>> {
        self.ops.get(&id)
    }

    /// The set, by ID.
    pub fn ops(&self) -> impl Iterator<Item = &Op<K, V>> {
        self.ops.values()
    }

    pub fn ids(&self) -> BTreeSet<K> {
        self.ops.keys().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// x is causally before y
pub fn before<K: Ord, V>(x: &Op<K, V>, y: &Op<K, V>) -> bool {
    y.past.contains(&x.id)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Existence {
    Live,
    Hidden,
    Dropped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Facet {
    Value,
    Position,
    Existence,
}

/// What a reader sees of one element (the insert is known).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct View<K, V> {
    /// the winning position write: the element's slot
    pub pos: K,
    pub pos_ts: Ts,
    /// the winning value write
    pub value_write: K,
    pub value_ts: Ts,
    pub value: V,
    pub existence: Existence,
    /// the write that holds the existence state
    pub existence_holder: K,
}

impl<K, V> View<K, V> {
    /// not live: hidden or dropped
    pub fn hidden(&self) -> bool {
        self.existence != Existence::Live
    }
}

/// The read rule. `Correct` is the model; the others are mutants (consistent implementations
/// of a wrong rule, as the formal model's rule variants), kept so that replays and tests have teeth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReadRule {
    Correct,
    /// existence is LWW over deletes and undels (`undel-lww`)
    UndelLww,
    /// a delete hides only if its timestamp is above the winning value write's
    /// (`delete-loses-to-set`: teeth for the list rule)
    DeleteLosesToSet,
    /// position and value winners tie-break on the lower agent (the agent order reversed)
    AgentTiebreakReversed,
    /// the live existence holder is the highest undel covering *every* delete, else the
    /// insert (an earlier wording of the rule: `holder-covers-every-delete`)
    HolderCoversEveryDelete,
}

fn ts_with(o: Ts, rule: ReadRule) -> (u64, std::cmp::Reverse<u64>, u64) {
    // a key whose order is (lamport, agent), or (lamport, reversed agent) for the mutant
    if rule == ReadRule::AgentTiebreakReversed {
        (o.0, std::cmp::Reverse(o.1), 0)
    } else {
        (o.0, std::cmp::Reverse(0), o.1)
    }
}

/// the op of `it` with the highest `(lamport, agent)` (by the rule's order)
fn max_by_ts<'a, K: Ord + 'a, V: 'a>(it: impl Iterator<Item = &'a Op<K, V>>, rule: ReadRule) -> Option<&'a Op<K, V>> {
    it.max_by_key(|o| ts_with(o.ts(), rule))
}

/// The read of one element's op set ([`ReadRule::Correct`]). `None` until its insert is here.
pub fn read<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>) -> Option<View<K, V>> {
    read_with(reg, ReadRule::Correct)
}

/// The read by a given rule (mutants for teeth; see [`ReadRule`]).
pub fn read_with<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>, rule: ReadRule) -> Option<View<K, V>> {
    let ins = reg.ops().find(|o| matches!(o.kind, Kind::Insert(_)))?;
    let pw = max_by_ts(reg.ops().filter(|o| o.kind.is_position()), rule).unwrap_or(ins);
    let vw = max_by_ts(reg.ops().filter(|o| o.kind.is_value()), rule).unwrap_or(ins);
    let dels: Vec<&Op<K, V>> = reg.ops().filter(|o| o.kind == Kind::Delete).collect();
    let undels: Vec<&Op<K, V>> = reg.ops().filter(|o| o.kind == Kind::Undel).collect();
    let drops: Vec<&Op<K, V>> = reg.ops().filter(|o| o.kind == Kind::Drop).collect();
    let covered = |d: &Op<K, V>| undels.iter().any(|u| before(d, u));
    let uncovered: Vec<&Op<K, V>> = dels.iter().copied().filter(|d| !covered(d)).collect();
    let (existence, holder) = if let Some(dr) = max_by_ts(drops.iter().copied(), ReadRule::Correct) {
        (Existence::Dropped, dr.id)
    } else {
        let hidden = match rule {
            ReadRule::UndelLww => max_by_ts(reg.ops().filter(|o| o.kind.is_existence()), rule).is_some_and(|o| o.kind == Kind::Delete),
            ReadRule::DeleteLosesToSet => uncovered.iter().any(|d| d.ts() > vw.ts()),
            _ => !uncovered.is_empty(),
        };
        if hidden {
            let h = max_by_ts(uncovered.iter().copied(), ReadRule::Correct)
                .or_else(|| max_by_ts(dels.iter().copied(), ReadRule::Correct))
                .map_or(ins.id, |d| d.id);
            (Existence::Hidden, h)
        } else {
            // live: the highest undel among those covering at least one delete
            // (every delete is covered); with no deletes, the highest undel; else the insert
            let cov = undels.iter().copied().filter(|u| {
                if rule == ReadRule::HolderCoversEveryDelete {
                    dels.iter().all(|d| before(d, u))
                } else {
                    dels.is_empty() || dels.iter().any(|d| before(d, u))
                }
            });
            (Existence::Live, max_by_ts(cov, ReadRule::Correct).map_or(ins.id, |u| u.id))
        }
    };
    Some(View {
        pos: pw.id,
        pos_ts: pw.ts(),
        value_write: vw.id,
        value_ts: vw.ts(),
        value: vw.kind.value().cloned().expect("a value write carries a value"),
        existence,
        existence_holder: holder,
    })
}

/// `winner(elem, facet)`: the write that holds the facet. `None` until
/// the insert is here.
pub fn winner<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>, facet: Facet) -> Option<K> {
    read(reg).map(|v| match facet {
        Facet::Value => v.value_write,
        Facet::Position => v.pos,
        Facet::Existence => v.existence_holder,
    })
}

/// `winner_resolved(elem, facet)`: the restored write when the holder carries
/// `restores_write`, else the holder.
pub fn winner_resolved<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>, facet: Facet) -> Option<K> {
    let w = winner(reg, facet)?;
    Some(reg.get(w).and_then(|o| o.restores_write).unwrap_or(w))
}

/// The covering undels: the undels that at least one delete is causally before.
pub fn covering_undels<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>) -> BTreeSet<K> {
    let dels: Vec<&Op<K, V>> = reg.ops().filter(|o| o.kind == Kind::Delete).collect();
    reg.ops().filter(|u| u.kind == Kind::Undel && dels.iter().any(|d| before(d, u))).map(|u| u.id).collect()
}

/// How undo decides that undoing a restore (restore -> delete) is the undoer's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OwnRule {
    /// every covering undel that is not in the undoer's batch is in the batch's causal past
    Correct,
    /// the planted mutant `own-by-holder-only`: own iff the
    /// existence holder is in the batch.
    /// It lets one participant's undo silently re-hide an element that another participant
    /// restored concurrently.
    ByHolderOnly,
}

/// Undo's own test for an undo of a restore: true iff every covering undel of the
/// element that is not in `batch` (the undoer's own ops) is in `batch_past` (the batch's
/// causal past). Otherwise the undo conflicts. A pure function of the element's op set.
pub fn restore_undo_is_own<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>, batch: &BTreeSet<K>, batch_past: &BTreeSet<K>) -> bool {
    restore_undo_is_own_with(reg, batch, batch_past, OwnRule::Correct)
}

pub fn restore_undo_is_own_with<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>, batch: &BTreeSet<K>, batch_past: &BTreeSet<K>, rule: OwnRule) -> bool {
    match rule {
        OwnRule::Correct => covering_undels(reg).iter().all(|u| batch.contains(u) || batch_past.contains(u)),
        OwnRule::ByHolderOnly => winner(reg, Facet::Existence).is_some_and(|h| batch.contains(&h)),
    }
}

/// The maximal existence ops (deletes and undels that no other existence op of the set is
/// after), which the formal models keep on arrival. Without drops, hidden iff one of them is a
/// delete (tested against the existence rule as the module docs state it).
pub fn max_existence<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &Reg<K, V>) -> BTreeSet<K> {
    let ex: Vec<&Op<K, V>> = reg.ops().filter(|o| o.kind.is_existence()).collect();
    ex.iter().filter(|x| !ex.iter().any(|y| before(x, y))).map(|x| x.id).collect()
}

#[cfg(test)]
mod tests;
