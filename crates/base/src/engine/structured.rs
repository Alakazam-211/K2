//! Map and MovableList on Yrs: the layout of the CSV seam design §4.1, with the engine
//! answers E1 (restore is an undelete) and E4 (every register holds its winning write).
//!
//! For a MovableList root `R` (Kleppmann's list of slots plus an LWW position register):
//! - `R/slots` (`YArray`): one string `"<slot id>\x1f<key>"` per slot. A slot id is the
//!   [`WriteId`] of the insert or move that made it.
//! - `R/pos` (`YMap`): key → the slot id of its current place (the **position register**).
//! - `R/val` (`YMap`): key → `"<write>\x1f<value>"` (the **value register**; absent = Null).
//! - `R/dead` (`YMap`): key → the delete's write (the **delete flag**). Restore removes it,
//!   so a delete concurrent with a restore wins (a Yrs map set beats a concurrent remove).
//!
//! An element shows iff `pos[key]` is the slot and `key ∉ dead`, so each key shows at most
//! once. Insert puts a slot after its anchor's current slot (dead slots count: a deleted
//! element keeps its place); move puts a new slot, re-points `pos` and removes the old slot
//! (its item stays as a tombstone, so a concurrent insert anchored on it still lands at the
//! old spot); delete sets the flag; restore removes it; set writes the value register.
//!
//! A Map root `R` is one `YMap`: key → `"<write>\x1f<value>"`; delete removes the key.
//!
//! **Register spelling** (the value registers and map entries): the write's id, `\x1f`, then
//! `n` (Null) or `s` + the string. Holding the winning write in the register is what lets
//! [`crate::Doc::winner`] answer from the state alone (yrs offers no public way to read a map
//! entry's item ID).
//!
//! **One change, in this order** (so a forward chain of moves is one run of consecutive IDs):
//! for each list root, all its slot inserts in op order, then its slot removals, then its
//! register writes (each register written once per change, with its final value); then each
//! map root's writes. Text roots are applied before all of them, as before, so text clocks
//! ([`super::insert_clocks`]) don't move.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use yrs::{Any, Array, ArrayRef, Map, MapRef, Out, ReadTxn, TransactionMut};

#[cfg(test)]
use super::yrs::is_root;
use super::yrs::{root_array, root_map};
use crate::doc::{ClientId, DocError, Edit};
use crate::elem::{
    check_map_key, Anchor, Applied, Effect, ElemState, Facet, Key, ListApplied, ListOp, MapApplied,
    MapOp, OpRefusal, Value, WriteId,
};

const SEP: char = '\x1f';

/// The four Yrs roots of one MovableList root.
#[derive(Clone)]
pub(crate) struct ListRefs {
    slots: ArrayRef,
    pos: MapRef,
    val: MapRef,
    dead: MapRef,
}

impl ListRefs {
    pub(crate) fn declare(doc: &yrs::Doc, root: &str) -> Self {
        Self {
            slots: root_array(doc, &format!("{root}/slots")),
            pos: root_map(doc, &format!("{root}/pos")),
            val: root_map(doc, &format!("{root}/val")),
            dead: root_map(doc, &format!("{root}/dead")),
        }
    }

    /// Whether all four handles are roots (rule 8's check).
    #[cfg(test)]
    pub(crate) fn are_roots(&self) -> bool {
        is_root(&self.slots) && is_root(&self.pos) && is_root(&self.val) && is_root(&self.dead)
    }

    #[cfg(any(test, feature = "conformance"))]
    pub(crate) fn slots(&self) -> &ArrayRef {
        &self.slots
    }
}

fn malformed(what: impl Into<String>) -> DocError {
    DocError::Malformed(what.into())
}

fn out_str(o: &Out) -> Option<&str> {
    match o {
        Out::Any(Any::String(s)) => Some(s),
        _ => None,
    }
}

fn any(s: String) -> Any {
    Any::String(Arc::from(s))
}

/// A value register's spelling.
pub(crate) fn enc_reg(w: &WriteId, v: &Value) -> String {
    let mut s = w.encode();
    s.push(SEP);
    match v {
        Value::Null => s.push('n'),
        Value::Str(t) => {
            s.push('s');
            s.push_str(t);
        }
    }
    s
}

pub(crate) fn dec_reg(s: &str) -> Result<(WriteId, Value), DocError> {
    let bad = || malformed(format!("value register {s:?}"));
    let (w, rest) = s.split_once(SEP).ok_or_else(bad)?;
    let w = WriteId::decode(w).ok_or_else(bad)?;
    let v = match rest.as_bytes().first() {
        Some(b'n') if rest.len() == 1 => Value::Null,
        Some(b's') => Value::Str(rest[1..].to_owned()),
        _ => return Err(bad()),
    };
    Ok((w, v))
}

fn dec_write(s: &str) -> Result<WriteId, DocError> {
    WriteId::decode(s).ok_or_else(|| malformed(format!("write id {s:?}")))
}

/// A list root's state as read from the doc: its slots in order, and its position and
/// delete registers.
struct ListRead {
    slots: Vec<(Arc<str>, Key)>,
    pos: HashMap<Key, Arc<str>>,
    dead: HashMap<Key, Arc<str>>,
}

fn read_list<T: ReadTxn>(txn: &T, r: &ListRefs) -> Result<ListRead, DocError> {
    let mut slots = Vec::with_capacity(r.slots.len(txn) as usize);
    for o in r.slots.iter(txn) {
        let s = out_str(&o).ok_or_else(|| malformed("a slot that is not a string"))?;
        let (id, key) = s
            .split_once(SEP)
            .ok_or_else(|| malformed(format!("slot {s:?}")))?;
        let key = Key::new(key).map_err(|_| malformed(format!("slot key {key:?}")))?;
        slots.push((Arc::from(id), key));
    }
    let reg = |m: &MapRef| -> Result<HashMap<Key, Arc<str>>, DocError> {
        m.iter(txn)
            .map(|(k, o)| {
                let key = Key::new(k).map_err(|_| malformed(format!("register key {k:?}")))?;
                let v = out_str(&o).ok_or_else(|| malformed("a register that is not a string"))?;
                Ok((key, Arc::from(v)))
            })
            .collect()
    };
    Ok(ListRead {
        slots,
        pos: reg(&r.pos)?,
        dead: reg(&r.dead)?,
    })
}

/// A list root's live order, cached by the doc between changes.
pub(crate) struct Order {
    pub keys: Vec<Key>,
    pub index: HashMap<Key, usize>,
    /// Every element's place, deleted ones included, in slot order.
    pub places: Vec<Key>,
}

pub(crate) fn order<T: ReadTxn>(txn: &T, r: &ListRefs) -> Result<Order, DocError> {
    let l = read_list(txn, r)?;
    let places: Vec<Key> = l
        .slots
        .into_iter()
        .filter(|(id, k)| l.pos.get(k) == Some(id))
        .map(|(_, k)| k)
        .collect();
    let keys: Vec<Key> = places
        .iter()
        .filter(|k| !l.dead.contains_key(*k))
        .cloned()
        .collect();
    let index = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (k.clone(), i))
        .collect();
    Ok(Order {
        keys,
        index,
        places,
    })
}

/// The live elements in order, with their values.
pub(crate) fn list<T: ReadTxn>(
    txn: &T,
    r: &ListRefs,
    o: &Order,
) -> Result<Vec<(Key, Value)>, DocError> {
    let mut vals: HashMap<&str, Value> = HashMap::new();
    for (k, v) in r.val.iter(txn) {
        let s = out_str(&v).ok_or_else(|| malformed("a register that is not a string"))?;
        vals.insert(k, dec_reg(s)?.1);
    }
    Ok(o.keys
        .iter()
        .map(|k| (k.clone(), vals.remove(k.as_str()).unwrap_or_default()))
        .collect())
}

/// Live from the cached order; otherwise the position register says whether the list ever
/// held the key.
pub(crate) fn elem<T: ReadTxn>(txn: &T, r: &ListRefs, o: &Order, key: &str) -> ElemState {
    match o.index.get(key) {
        Some(i) => ElemState::Live(*i),
        None if r.pos.get(txn, key).is_some() => ElemState::Deleted,
        None => ElemState::Unknown,
    }
}

/// The slot id `key`'s position register points at (`None`: never inserted). Test-only.
#[cfg(feature = "conformance")]
pub(crate) fn slot_of_key<T: ReadTxn>(
    txn: &T,
    r: &ListRefs,
    key: &str,
) -> Result<Option<String>, DocError> {
    r.pos
        .get(txn, key)
        .map(|o| {
            out_str(&o)
                .map(str::to_owned)
                .ok_or_else(|| malformed("a register that is not a string"))
        })
        .transpose()
}

/// Every element the list has held that doesn't show (deleted), with its value.
pub(crate) fn hidden<T: ReadTxn>(
    txn: &T,
    r: &ListRefs,
    o: &Order,
) -> Result<BTreeMap<Key, Value>, DocError> {
    let mut out = BTreeMap::new();
    for (k, _) in r.pos.iter(txn) {
        if o.index.contains_key(k) {
            continue;
        }
        let key = Key::new(k).map_err(|_| malformed(format!("register key {k:?}")))?;
        let v = reg_of(txn, &r.val, k)?.map(|(_, v)| v).unwrap_or_default();
        out.insert(key, v);
    }
    Ok(out)
}

fn reg_of<T: ReadTxn>(
    txn: &T,
    m: &MapRef,
    key: &str,
) -> Result<Option<(WriteId, Value)>, DocError> {
    match m.get(txn, key) {
        None => Ok(None),
        Some(o) => {
            let s = out_str(&o).ok_or_else(|| malformed("a register that is not a string"))?;
            dec_reg(s).map(Some)
        }
    }
}

/// A known element's value (live or deleted); `None` for a key the list never held.
pub(crate) fn elem_value<T: ReadTxn>(
    txn: &T,
    r: &ListRefs,
    key: &str,
) -> Result<Option<Value>, DocError> {
    if r.pos.get(txn, key).is_none() {
        return Ok(None);
    }
    Ok(Some(
        reg_of(txn, &r.val, key)?
            .map(|(_, v)| v)
            .unwrap_or_default(),
    ))
}

pub(crate) fn map<T: ReadTxn>(txn: &T, m: &MapRef) -> Result<BTreeMap<String, Value>, DocError> {
    m.iter(txn)
        .map(|(k, o)| {
            let s = out_str(&o).ok_or_else(|| malformed("a map entry that is not a string"))?;
            Ok((k.to_owned(), dec_reg(s)?.1))
        })
        .collect()
}

pub(crate) fn map_get<T: ReadTxn>(
    txn: &T,
    m: &MapRef,
    key: &str,
) -> Result<Option<Value>, DocError> {
    Ok(reg_of(txn, m, key)?.map(|(_, v)| v))
}

/// The write that holds a list element's register now. `Existence` is the delete that hides
/// it; a live element has none on Yrs (an undelete removes the flag, and yrs keeps no
/// identity for a removal), so it is `None`, as is a value never set since an insert that
/// gave none.
pub(crate) fn list_winner<T: ReadTxn>(
    txn: &T,
    r: &ListRefs,
    key: &str,
    facet: Facet,
) -> Result<Option<WriteId>, DocError> {
    let raw = |m: &MapRef| -> Result<Option<String>, DocError> {
        m.get(txn, key)
            .map(|o| {
                out_str(&o)
                    .map(str::to_owned)
                    .ok_or_else(|| malformed("a register that is not a string"))
            })
            .transpose()
    };
    match facet {
        Facet::Value => Ok(reg_of(txn, &r.val, key)?.map(|(w, _)| w)),
        Facet::Position => raw(&r.pos)?.as_deref().map(dec_write).transpose(),
        Facet::Existence => raw(&r.dead)?.as_deref().map(dec_write).transpose(),
    }
}

/// A map key's winning write (`None`: absent; on Yrs a delete leaves no identity). Maps have
/// only the value facet.
pub(crate) fn map_winner<T: ReadTxn>(
    txn: &T,
    m: &MapRef,
    key: &str,
    facet: Facet,
) -> Result<Option<WriteId>, DocError> {
    if facet != Facet::Value {
        return Ok(None);
    }
    Ok(reg_of(txn, m, key)?.map(|(w, _)| w))
}

/// Registers written once per change, in first-write order, with their last value.
#[derive(Clone)]
struct Writes<V> {
    order: Vec<(Key, V)>,
    at: HashMap<Key, usize>,
}

impl<V> Default for Writes<V> {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            at: HashMap::new(),
        }
    }
}

impl<V> Writes<V> {
    fn put(&mut self, k: &Key, v: V) {
        match self.at.get(k) {
            Some(&i) => self.order[i].1 = v,
            None => {
                self.at.insert(k.clone(), self.order.len());
                self.order.push((k.clone(), v));
            }
        }
    }
}

/// One list root's change being planned: the slots as yrs will hold them while the change's
/// inserts run (removals come after), and the registers as the ops left them.
#[derive(Clone)]
struct ListPlan {
    /// Slot entries by entry id: (slot id, key).
    arena: Vec<(Arc<str>, Key)>,
    /// Entry ids in slot order (removed slots included until the end of the change).
    raw: Vec<u32>,
    /// By entry id: the element shows here.
    live: Vec<bool>,
    /// By entry id: removed at the end of the change.
    removed: Vec<bool>,
    /// Every key the list has held → its current slot entry (`None`: its slot is missing,
    /// which the kit never writes).
    cur: HashMap<Key, Option<u32>>,
    /// Deleted keys → the delete's write.
    dead: HashMap<Key, WriteId>,
    dead_at_start: HashSet<Key>,
    /// Values written by this change.
    vals: HashMap<Key, (WriteId, Value)>,
    /// Keys this change dropped: their registers are removed after every other write.
    dropped: Vec<Key>,
    // the change, for yrs
    inserts: Vec<(u32, String)>,
    pos_w: Writes<String>,
    val_w: Writes<String>,
    dead_w: Writes<Option<String>>,
}

impl ListPlan {
    fn new<T: ReadTxn>(txn: &T, r: &ListRefs) -> Result<Self, DocError> {
        let l = read_list(txn, r)?;
        let n = l.slots.len();
        let mut by_slot: HashMap<Arc<str>, u32> = HashMap::with_capacity(n);
        for (i, (id, _)) in l.slots.iter().enumerate() {
            by_slot.insert(id.clone(), i as u32);
        }
        let mut dead = HashMap::with_capacity(l.dead.len());
        for (k, w) in &l.dead {
            dead.insert(k.clone(), dec_write(w)?);
        }
        let mut cur = HashMap::with_capacity(l.pos.len());
        let mut live = vec![false; n];
        for (k, slot) in &l.pos {
            let e = by_slot.get(slot).copied();
            if let Some(e) = e {
                if l.arena_key(e) == k {
                    live[e as usize] = !dead.contains_key(k);
                }
            }
            cur.insert(k.clone(), e.filter(|&e| l.arena_key(e) == k));
        }
        Ok(Self {
            raw: (0..n as u32).collect(),
            arena: l.slots,
            live,
            removed: vec![false; n],
            cur,
            dead_at_start: dead.keys().cloned().collect(),
            dead,
            vals: HashMap::new(),
            dropped: Vec::new(),
            inserts: Vec::new(),
            pos_w: Writes::default(),
            val_w: Writes::default(),
            dead_w: Writes::default(),
        })
    }

    fn writes(&self) -> bool {
        !self.inserts.is_empty()
            || !self.dropped.is_empty()
            || self.removed.iter().any(|r| *r)
            || !self.pos_w.order.is_empty()
            || !self.val_w.order.is_empty()
            || self.dead_w.order.iter().any(|(k, d)| match d {
                Some(_) => true,
                None => self.dead_at_start.contains(k),
            })
    }

    fn raw_at(&self, e: u32) -> usize {
        self.raw
            .iter()
            .position(|&x| x == e)
            .expect("a current slot is in the slots")
    }

    /// Live elements before raw position `p`: the live index of whatever is at `p`.
    fn live_before(&self, p: usize) -> usize {
        self.raw[..p]
            .iter()
            .filter(|&&e| self.live[e as usize])
            .count()
    }

    /// The live index of `key`'s element, if it shows.
    fn index_of(&self, key: &Key) -> Option<usize> {
        let e = self.cur.get(key).copied().flatten()?;
        self.live[e as usize].then(|| self.live_before(self.raw_at(e)))
    }

    fn slot_of(&self, key: &Key) -> Result<Option<u32>, OpRefusal> {
        self.cur
            .get(key)
            .copied()
            .ok_or_else(|| OpRefusal::UnknownKey(key.to_string()))
    }

    /// Where a new slot goes for `at`, as a raw position.
    fn resolve(&self, at: &Anchor, moving: Option<&Key>) -> Result<usize, OpRefusalOr> {
        let anchor = |k: &Key| -> Result<usize, OpRefusalOr> {
            if Some(k) == moving {
                return Err(OpRefusal::SelfAnchor(k.to_string()).into());
            }
            match self.slot_of(k)? {
                Some(e) => Ok(self.raw_at(e)),
                None => Err(OpRefusalOr::Doc(malformed(format!(
                    "element {k} has no slot"
                )))),
            }
        };
        match at {
            Anchor::Start => Ok(0),
            Anchor::End => Ok(self.raw.len()),
            Anchor::After(k) => Ok(anchor(k)? + 1),
            Anchor::Before(k) => anchor(k),
        }
    }

    fn value<T: ReadTxn>(&self, txn: &T, r: &ListRefs, key: &Key) -> Result<Value, DocError> {
        if let Some((_, v)) = self.vals.get(key) {
            return Ok(v.clone());
        }
        Ok(reg_of(txn, &r.val, key.as_str())?
            .map(|(_, v)| v)
            .unwrap_or_default())
    }

    /// Put a new slot for `key` at raw position `p`; returns its entry id.
    fn put_slot(&mut self, p: usize, key: &Key, w: &WriteId) -> u32 {
        let slot = w.encode();
        let e = self.arena.len() as u32;
        self.inserts
            .push((p as u32, format!("{slot}{SEP}{}", key.as_str())));
        self.arena.push((Arc::from(slot.as_str()), key.clone()));
        self.raw.insert(p, e);
        self.live.push(!self.dead.contains_key(key));
        self.removed.push(false);
        self.cur.insert(key.clone(), Some(e));
        self.pos_w.put(key, slot);
        e
    }

    fn apply<T: ReadTxn>(
        &mut self,
        txn: &T,
        r: &ListRefs,
        op: &ListOp,
        w: WriteId,
    ) -> Result<ListApplied, OpRefusalOr> {
        let key = op.key();
        let applied = |index_before, index_after, value_before, value_after, effect| ListApplied {
            kind: op.kind(),
            key: key.clone(),
            write: w,
            index_before,
            index_after,
            value_before,
            value_after,
            effect,
        };
        Ok(match op {
            ListOp::Insert { at, value, .. } => {
                if self.cur.contains_key(key) || self.dropped.contains(key) {
                    return Err(OpRefusal::KeyExists(key.to_string()).into());
                }
                let p = self.resolve(at, None)?;
                let e = self.put_slot(p, key, &w);
                if *value != Value::Null {
                    self.vals.insert(key.clone(), (w, value.clone()));
                    self.val_w.put(key, enc_reg(&w, value));
                }
                let after = self.live_before(self.raw_at(e));
                applied(
                    None,
                    Some(after),
                    None,
                    Some(value.clone()),
                    Effect::Visible,
                )
            }
            ListOp::Move { to, .. } => {
                let old = self.slot_of(key)?;
                let p = self.resolve(to, Some(key))?;
                let before = self.index_of(key);
                let v = self.value(txn, r, key)?;
                if let Some(old) = old {
                    self.live[old as usize] = false;
                    self.removed[old as usize] = true;
                }
                self.put_slot(p, key, &w);
                let after = self.index_of(key);
                let effect = if after.is_some() {
                    Effect::Visible
                } else {
                    Effect::Hidden
                };
                applied(before, after, Some(v.clone()), Some(v), effect)
            }
            ListOp::Delete { .. } => {
                let e = self.slot_of(key)?;
                let v = self.value(txn, r, key)?;
                if let Some(d) = self.dead.get(key) {
                    let winner = Some(*d);
                    return Ok(applied(
                        None,
                        None,
                        Some(v.clone()),
                        Some(v),
                        Effect::None { winner },
                    ));
                }
                let before = self.index_of(key);
                if let Some(e) = e {
                    self.live[e as usize] = false;
                }
                self.dead.insert(key.clone(), w);
                self.dead_w.put(key, Some(w.encode()));
                applied(before, None, Some(v.clone()), Some(v), Effect::Visible)
            }
            ListOp::Restore { .. } => {
                let e = self.slot_of(key)?;
                let v = self.value(txn, r, key)?;
                if self.dead.remove(key).is_none() {
                    let i = self.index_of(key);
                    return Ok(applied(
                        i,
                        i,
                        Some(v.clone()),
                        Some(v),
                        Effect::None { winner: None },
                    ));
                }
                if let Some(e) = e {
                    self.live[e as usize] = true;
                }
                self.dead_w.put(key, None);
                let after = self.index_of(key);
                applied(None, after, Some(v.clone()), Some(v), Effect::Visible)
            }
            ListOp::Set { value, .. } => {
                self.slot_of(key)?;
                let before = self.value(txn, r, key)?;
                self.vals.insert(key.clone(), (w, value.clone()));
                self.val_w.put(key, enc_reg(&w, value));
                let i = self.index_of(key);
                let effect = if i.is_some() {
                    Effect::Visible
                } else {
                    Effect::Hidden
                };
                applied(i, i, Some(before), Some(value.clone()), effect)
            }
            ListOp::Drop { .. } => {
                let e = self.slot_of(key)?;
                if !self.dead.contains_key(key) {
                    return Err(OpRefusal::NotDeleted(key.to_string()).into());
                }
                let v = self.value(txn, r, key)?;
                if let Some(e) = e {
                    self.live[e as usize] = false;
                    self.removed[e as usize] = true;
                }
                self.cur.remove(key);
                self.dead.remove(key);
                self.vals.remove(key);
                self.dropped.push(key.clone());
                applied(None, None, Some(v), None, Effect::Hidden)
            }
        })
    }

    /// Write the change into a replica's transaction.
    fn emit(&self, txn: &mut TransactionMut, r: &ListRefs) {
        // A run of inserts each right after the previous one (a forward chain: an import, a
        // block move, a sort) goes to yrs as one range: the same items, origins and IDs, but
        // yrs 0.28 holds O(n^2) bytes at its peak for n single inserts in one transaction
        // (measured: 4,000 single inserts peak at 188 MB, one range of 4,000 at 0.2 MB).
        let mut i = 0;
        while i < self.inserts.len() {
            let start = self.inserts[i].0;
            let mut j = i + 1;
            while j < self.inserts.len() && self.inserts[j].0 == start + (j - i) as u32 {
                j += 1;
            }
            r.slots.insert_range(
                txn,
                start,
                self.inserts[i..j].iter().map(|(_, s)| any(s.clone())),
            );
            i = j;
        }
        let gone: Vec<u32> = self
            .raw
            .iter()
            .enumerate()
            .filter(|(_, &e)| self.removed[e as usize])
            .map(|(p, _)| p as u32)
            .collect();
        // Back to front, a run of adjacent slots at a time (a sort removes its old slots as
        // a few long runs; one remove per slot walks the array from the start each time).
        let mut end = gone.len();
        while end > 0 {
            let mut start = end - 1;
            while start > 0 && gone[start - 1] + 1 == gone[start] {
                start -= 1;
            }
            r.slots.remove_range(txn, gone[start], (end - start) as u32);
            end = start;
        }
        for (k, s) in &self.pos_w.order {
            r.pos.insert(txn, k.as_str(), any(s.clone()));
        }
        for (k, s) in &self.val_w.order {
            r.val.insert(txn, k.as_str(), any(s.clone()));
        }
        for (k, d) in &self.dead_w.order {
            match d {
                Some(s) => {
                    r.dead.insert(txn, k.as_str(), any(s.clone()));
                }
                None if self.dead_at_start.contains(k) => {
                    r.dead.remove(txn, k.as_str());
                }
                None => {}
            }
        }
        // a dropped element leaves every register (its slot went with the removals above)
        for k in &self.dropped {
            r.pos.remove(txn, k.as_str());
            r.val.remove(txn, k.as_str());
            r.dead.remove(txn, k.as_str());
        }
    }
}

impl ListRead {
    fn arena_key(&self, e: u32) -> &Key {
        &self.slots[e as usize].1
    }
}

/// A refusal of the op, or a doc error found while planning it.
enum OpRefusalOr {
    Refused(OpRefusal),
    Doc(DocError),
}

impl From<OpRefusal> for OpRefusalOr {
    fn from(r: OpRefusal) -> Self {
        OpRefusalOr::Refused(r)
    }
}

impl From<DocError> for OpRefusalOr {
    fn from(e: DocError) -> Self {
        OpRefusalOr::Doc(e)
    }
}

/// One map root's change being planned.
#[derive(Clone)]
struct MapPlan {
    /// Keys this change wrote → their value now (`None`: deleted).
    now: HashMap<String, Option<(WriteId, Value)>>,
    present_at_start: HashSet<String>,
    writes: Vec<(String, Option<String>)>,
    at: HashMap<String, usize>,
}

impl MapPlan {
    fn new() -> Self {
        Self {
            now: HashMap::new(),
            present_at_start: HashSet::new(),
            writes: Vec::new(),
            at: HashMap::new(),
        }
    }

    fn current<T: ReadTxn>(
        &mut self,
        txn: &T,
        m: &MapRef,
        key: &str,
    ) -> Result<Option<(WriteId, Value)>, DocError> {
        if let Some(v) = self.now.get(key) {
            return Ok(v.clone());
        }
        let v = reg_of(txn, m, key)?;
        if v.is_some() {
            self.present_at_start.insert(key.to_owned());
        }
        Ok(v)
    }

    fn put(&mut self, key: &str, v: Option<String>) {
        match self.at.get(key) {
            Some(&i) => self.writes[i].1 = v,
            None => {
                self.at.insert(key.to_owned(), self.writes.len());
                self.writes.push((key.to_owned(), v));
            }
        }
    }

    fn apply<T: ReadTxn>(
        &mut self,
        txn: &T,
        m: &MapRef,
        op: &MapOp,
        w: WriteId,
    ) -> Result<MapApplied, OpRefusalOr> {
        let key = op.key();
        check_map_key(key).map_err(OpRefusal::from)?;
        let before = self.current(txn, m, key)?;
        let (after, effect) = match op {
            MapOp::Set { value, .. } => {
                self.now.insert(key.to_owned(), Some((w, value.clone())));
                self.put(key, Some(enc_reg(&w, value)));
                (Some(value.clone()), Effect::Visible)
            }
            MapOp::Delete { .. } => match &before {
                None => (None, Effect::None { winner: None }),
                Some(_) => {
                    self.now.insert(key.to_owned(), None);
                    self.put(key, None);
                    (None, Effect::Visible)
                }
            },
        };
        Ok(MapApplied {
            kind: op.kind(),
            key: key.to_owned(),
            write: w,
            before: before.map(|(_, v)| v),
            after,
            effect,
        })
    }

    fn writes(&self) -> bool {
        self.writes
            .iter()
            .any(|(k, v)| v.is_some() || self.present_at_start.contains(k))
    }

    fn emit(&self, txn: &mut TransactionMut, m: &MapRef) {
        for (k, v) in &self.writes {
            match v {
                Some(s) => {
                    m.insert(txn, k.as_str(), any(s.clone()));
                }
                None if self.present_at_start.contains(k) => {
                    m.remove(txn, k);
                }
                None => {}
            }
        }
    }
}

/// A change's structured part, planned against the doc before any replica exists: the
/// applied stream (text edits included, in edit order) and what to write.
#[derive(Clone)]
pub(crate) struct Plan {
    pub applied: Vec<Applied>,
    lists: Vec<(&'static str, ListPlan)>,
    maps: Vec<(&'static str, MapPlan)>,
}

impl Plan {
    pub(crate) fn writes(&self) -> bool {
        self.lists.iter().any(|(_, p)| p.writes()) || self.maps.iter().any(|(_, p)| p.writes())
    }

    /// Write the planned change into a replica's transaction (after its text edits).
    pub(crate) fn emit(
        &self,
        txn: &mut TransactionMut,
        lists: &HashMap<&'static str, ListRefs>,
        maps: &HashMap<&'static str, MapRef>,
    ) {
        for (root, p) in &self.lists {
            p.emit(txn, &lists[root]);
        }
        for (root, p) in &self.maps {
            p.emit(txn, &maps[root]);
        }
    }
}

/// Plan `edits` (roots and kinds already checked) as `client`'s change number `change`.
/// Ops are checked one by one, in order, against the state the previous ones left; the first
/// refusal refuses the whole change.
pub(crate) fn plan<T: ReadTxn>(
    txn: &T,
    lists: &HashMap<&'static str, ListRefs>,
    maps: &HashMap<&'static str, MapRef>,
    client: ClientId,
    change: u64,
    edits: &[Edit],
) -> Result<Plan, DocError> {
    let mut out = Plan {
        applied: Vec::with_capacity(edits.len()),
        lists: Vec::new(),
        maps: Vec::new(),
    };
    let mut op_no: u32 = 0;
    let mut next = || {
        let w = WriteId::new(client, change, op_no);
        op_no += 1;
        w
    };
    let refused = |root: &str, index: usize, e: OpRefusalOr| match e {
        OpRefusalOr::Refused(refusal) => DocError::Op {
            root: root.to_owned(),
            index,
            refusal,
        },
        OpRefusalOr::Doc(e) => e,
    };
    for e in edits {
        match e {
            Edit::Text { root, hunks } => out.applied.push(Applied::Text {
                root: root.clone(),
                hunks: hunks.clone(),
            }),
            Edit::List { root, ops } => {
                let (&name, refs) = lists
                    .get_key_value(root.as_str())
                    .expect("list roots are checked");
                let at = match out.lists.iter().position(|(r, _)| *r == name) {
                    Some(i) => i,
                    None => {
                        out.lists.push((name, ListPlan::new(txn, refs)?));
                        out.lists.len() - 1
                    }
                };
                let p = &mut out.lists[at].1;
                let mut done = Vec::with_capacity(ops.len());
                for (i, op) in ops.iter().enumerate() {
                    done.push(
                        p.apply(txn, refs, op, next())
                            .map_err(|e| refused(root, i, e))?,
                    );
                }
                out.applied.push(Applied::List {
                    root: root.clone(),
                    ops: done,
                });
            }
            Edit::Map { root, ops } => {
                let (&name, m) = maps
                    .get_key_value(root.as_str())
                    .expect("map roots are checked");
                let at = match out.maps.iter().position(|(r, _)| *r == name) {
                    Some(i) => i,
                    None => {
                        out.maps.push((name, MapPlan::new()));
                        out.maps.len() - 1
                    }
                };
                let p = &mut out.maps[at].1;
                let mut done = Vec::with_capacity(ops.len());
                for (i, op) in ops.iter().enumerate() {
                    done.push(
                        p.apply(txn, m, op, next())
                            .map_err(|e| refused(root, i, e))?,
                    );
                }
                out.applied.push(Applied::Map {
                    root: root.clone(),
                    ops: done,
                });
            }
        }
    }
    Ok(out)
}
