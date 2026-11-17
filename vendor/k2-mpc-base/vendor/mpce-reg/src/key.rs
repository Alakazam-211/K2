//! The map's per-key register: a grow-only set of one key's writes plus
//! a pure read. The winner is the write with the highest `(lamport, agent)`; the key is live
//! iff the winner is not a delete (so a later set resurrects a deleted key). Keys are
//! independent: each key has its own [`KeyReg`] and an op names one key.

use crate::{RegError, Ts};
use std::collections::BTreeMap;
use std::fmt::Debug;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KeyKind<V> {
    /// a plain value
    Set(V),
    /// a new nested root (its ID is the write's own)
    Root,
    Delete,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyWrite<K, V> {
    pub id: K,
    pub agent: u64,
    pub lamport: u64,
    pub kind: KeyKind<V>,
    /// the write an undo, redo or revert puts back
    pub restores_write: Option<K>,
}

impl<K, V> KeyWrite<K, V> {
    pub fn ts(&self) -> Ts {
        (self.lamport, self.agent)
    }
    pub fn live(&self) -> bool {
        !matches!(self.kind, KeyKind::Delete)
    }
}

/// One key's state: the set of its writes received here. Nothing else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyReg<K: Ord, V> {
    writes: BTreeMap<K, KeyWrite<K, V>>,
}

impl<K: Ord, V> Default for KeyReg<K, V> {
    fn default() -> Self {
        KeyReg { writes: BTreeMap::new() }
    }
}

/// The read rule: `Correct`, or a mutant for teeth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyRule {
    Correct,
    /// LWW by lamport only; a lamport tie goes to the lower ID (`lww-no-agent-tiebreak`, in set form)
    NoAgentTiebreak,
    /// a delete beats every set, whatever the timestamps (`delete-not-lww`, in set form)
    DeleteWins,
}

impl<K: Ord + Copy + Debug, V: Clone + Eq + Debug> KeyReg<K, V> {
    pub fn new() -> KeyReg<K, V> {
        KeyReg::default()
    }

    /// Apply = set union. Ok(true) if new.
    pub fn apply(&mut self, w: KeyWrite<K, V>) -> Result<bool, RegError> {
        match self.writes.get(&w.id) {
            Some(o) if *o == w => Ok(false),
            Some(_) => Err(RegError::Conflict),
            None => {
                self.writes.insert(w.id, w);
                Ok(true)
            }
        }
    }

    pub fn get(&self, id: K) -> Option<&KeyWrite<K, V>> {
        self.writes.get(&id)
    }

    pub fn writes(&self) -> impl Iterator<Item = &KeyWrite<K, V>> {
        self.writes.values()
    }

    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }
}

/// The winning write (`winner(map, key)`), or `None` if the key was never written.
pub fn read_key<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &KeyReg<K, V>) -> Option<&KeyWrite<K, V>> {
    read_key_with(reg, KeyRule::Correct)
}

pub fn read_key_with<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &KeyReg<K, V>, rule: KeyRule) -> Option<&KeyWrite<K, V>> {
    match rule {
        KeyRule::Correct => reg.writes().max_by_key(|w| w.ts()),
        // max_by_key keeps the last maximum: iterate by descending ID so a tie keeps the lower ID
        KeyRule::NoAgentTiebreak => reg.writes.values().rev().max_by_key(|w| w.lamport),
        KeyRule::DeleteWins => reg.writes().filter(|w| !w.live()).max_by_key(|w| w.ts()).or_else(|| reg.writes().max_by_key(|w| w.ts())),
    }
}

/// `winner_resolved(map, key)`: the restored write when the winner carries `restores_write`.
pub fn winner_resolved_key<K: Ord + Copy + Debug, V: Clone + Eq + Debug>(reg: &KeyReg<K, V>) -> Option<K> {
    read_key(reg).map(|w| w.restores_write.unwrap_or(w.id))
}
