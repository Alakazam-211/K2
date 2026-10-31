//! Helpers for the structured rules: op builders by key, seeds, a net of replicas, and the
//! replay of an applied stream over plain values (what a subscriber holding only the live
//! order and the map does).
//!
//! Writers are agents 101, 102, 103 (the seed is [`crate::SEED_AGENT`]); tie-breaks depend on
//! that numeric order. Keys are short (`a`, `r1`, `c2`) so a mutant adapter can suffix them.

use std::collections::BTreeMap;
use std::fmt::Debug;

use k2_mpc_base::elem::{Anchor, Applied, Effect, ListApplied, ListOp, ListOpKind, MapApplied};
use k2_mpc_base::elem::{MapOp, MapOpKind};
use k2_mpc_base::{Edit, Key, Value};

use crate::caps::{MLists, Maps, Structured, CELLS, COLS, ROWS};
use crate::TextReplica;

pub fn k(s: &str) -> Key {
    Key::new(s).unwrap_or_else(|e| panic!("key {s:?}: {e}"))
}

/// Space-separated keys.
pub fn ks(s: &str) -> Vec<Key> {
    s.split(' ').filter(|x| !x.is_empty()).map(k).collect()
}

pub fn after(s: &str) -> Anchor {
    Anchor::After(k(s))
}

pub fn before(s: &str) -> Anchor {
    Anchor::Before(k(s))
}

pub fn ins(key: &str, at: Anchor) -> ListOp {
    ListOp::Insert {
        key: k(key),
        at,
        value: Value::Null,
    }
}

pub fn ins_v(key: &str, at: Anchor, v: &str) -> ListOp {
    ListOp::Insert {
        key: k(key),
        at,
        value: Value::str(v),
    }
}

pub fn mv(key: &str, to: Anchor) -> ListOp {
    ListOp::Move { key: k(key), to }
}

pub fn del(key: &str) -> ListOp {
    ListOp::Delete { key: k(key) }
}

pub fn res(key: &str) -> ListOp {
    ListOp::Restore { key: k(key) }
}

pub fn set(key: &str, v: &str) -> ListOp {
    ListOp::Set {
        key: k(key),
        value: Value::str(v),
    }
}

pub fn mset(key: &str, v: &str) -> MapOp {
    MapOp::Set {
        key: key.into(),
        value: Value::str(v),
    }
}

pub fn mdel(key: &str) -> MapOp {
    MapOp::Delete { key: key.into() }
}

pub fn list_edit(root: &str, ops: Vec<ListOp>) -> Edit {
    Edit::List {
        root: root.into(),
        ops,
    }
}

pub fn rows(ops: Vec<ListOp>) -> Edit {
    list_edit(ROWS, ops)
}

pub fn cols(ops: Vec<ListOp>) -> Edit {
    list_edit(COLS, ops)
}

pub fn cells(ops: Vec<MapOp>) -> Edit {
    Edit::Map {
        root: CELLS.into(),
        ops,
    }
}

/// A cell's map key (CSV contract §1.1).
pub fn cell(row: &str, col: &str) -> String {
    format!("{row}\u{1f}{col}")
}

/// The value the seed gives cell (row, col): it names its cell, so a rule can check every
/// cell is still on its row and column.
pub fn cell_value(row: &str, col: &str) -> String {
    format!("{row}.{col}")
}

/// `keys` (space-separated) inserted into `root` in order, as an import does (one forward
/// chain).
pub fn seed_list(root: &str, keys: &str) -> Edit {
    let mut at = Anchor::Start;
    let mut ops = Vec::new();
    for key in keys.split(' ').filter(|x| !x.is_empty()) {
        ops.push(ins(key, at));
        at = after(key);
    }
    list_edit(root, ops)
}

/// A table: `rows` × `cols` (space-separated keys), every cell present with
/// [`cell_value`].
pub fn seed_table(rows: &str, cols: &str) -> Vec<Edit> {
    let mut map = Vec::new();
    for r in rows.split(' ') {
        for c in cols.split(' ') {
            map.push(mset(&cell(r, c), &cell_value(r, c)));
        }
    }
    vec![
        seed_list(ROWS, rows),
        seed_list(COLS, cols),
        self::cells(map),
    ]
}

/// A list's live keys, space-separated.
pub fn order<S: MLists>(r: &S, root: &str) -> String {
    r.list(root)
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A set of replicas of one structured doc on engine `S`.
pub struct Grid<S: Structured> {
    pub r: Vec<S>,
}

impl<S: Structured> Grid<S> {
    /// One replica per agent, all holding `seed`.
    pub fn new(agents: &[u64], seed: &[Edit]) -> Self {
        Grid {
            r: agents.iter().map(|&a| S::replica(a, seed)).collect(),
        }
    }

    /// Replica `i` applies `edits` as one change; a refusal fails the rule.
    pub fn edit(&mut self, i: usize, edits: Vec<Edit>) -> Vec<Applied<S::Write>> {
        self.r[i]
            .edit(&edits)
            .unwrap_or_else(|e| panic!("replica {i} refused {edits:?}: {e:?}"))
    }

    /// `n` changes of replica `i`'s own that touch nothing a rule reads (a scratch map key):
    /// they raise its lamport clock above a replica that made fewer.
    pub fn bump(&mut self, i: usize, n: usize) {
        for j in 0..n {
            self.edit(i, vec![cells(vec![mset("bump", &format!("{i}.{j}"))])]);
        }
    }

    /// Deliver everything replica `from` has to replica `to`.
    pub fn sync(&mut self, from: usize, to: usize) {
        assert_ne!(from, to, "sync a replica with itself");
        let (a, b) = pair(&mut self.r, from, to);
        b.sync_from(a);
    }

    /// Everyone gets everything (rounds of all pairs until the versions agree, at most
    /// three: a replica may make a change while it merges); then assert they agree.
    pub fn sync_all(&mut self) {
        for _ in 0..3 {
            for a in 0..self.r.len() {
                for b in 0..self.r.len() {
                    if a != b {
                        self.sync(a, b);
                    }
                }
            }
            if self.r.iter().all(|x| x.version() == self.r[0].version()) {
                return;
            }
        }
        panic!("replicas saw other changes after three rounds of sync");
    }
}

/// `(&r[from], &mut r[to])`.
pub fn pair<T>(r: &mut [T], from: usize, to: usize) -> (&T, &mut T) {
    if from < to {
        let (l, h) = r.split_at_mut(to);
        (&l[from], &mut h[0])
    } else {
        let (l, h) = r.split_at_mut(from);
        (&h[0], &mut l[to])
    }
}

/// Every replica has the same lists and maps.
pub fn assert_converged<S: MLists + Maps>(g: &Grid<S>, what: &str) {
    let engine = S::Engine::ENGINE;
    for i in 1..g.r.len() {
        for root in [ROWS, COLS] {
            assert_eq!(
                g.r[i].list(root),
                g.r[0].list(root),
                "{engine}: {what}: replica {i}'s {root} diverged"
            );
        }
        assert_eq!(
            g.r[i].map(CELLS),
            g.r[0].map(CELLS),
            "{engine}: {what}: replica {i}'s cells diverged"
        );
    }
}

/// Live lists and maps by root: the plain values a stream is replayed over.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plain {
    pub lists: BTreeMap<String, Vec<(Key, Value)>>,
    pub maps: BTreeMap<String, BTreeMap<String, Value>>,
}

impl Plain {
    /// A replica's [`ROWS`], [`COLS`] and [`CELLS`].
    pub fn of<S: MLists + Maps>(r: &S) -> Self {
        Plain {
            lists: [ROWS, COLS]
                .into_iter()
                .map(|x| (x.to_owned(), r.list(x)))
                .collect(),
            maps: [(CELLS.to_owned(), r.map(CELLS))].into_iter().collect(),
        }
    }

    /// Replay `applied` over these values. Ops without a visible effect must not move.
    pub fn replay<W: Debug>(&mut self, applied: &[Applied<W>]) -> Result<(), String> {
        for a in applied {
            match a {
                Applied::List { root, ops } => {
                    let l = self.lists.entry(root.clone()).or_default();
                    replay_list(l, ops).map_err(|e| format!("{root}: {e}"))?;
                }
                Applied::Map { root, ops } => {
                    let m = self.maps.entry(root.clone()).or_default();
                    replay_map(m, ops).map_err(|e| format!("{root}: {e}"))?;
                }
                other => return Err(format!("a structured rule's stream holds {other:?}")),
            }
        }
        Ok(())
    }
}

/// Replay list ops over a live order.
pub fn replay_list<W: Debug>(
    list: &mut Vec<(Key, Value)>,
    ops: &[ListApplied<W>],
) -> Result<(), String> {
    for (i, o) in ops.iter().enumerate() {
        let bad = |why: &str| format!("op {i} ({o:?}): {why}");
        if !matches!(o.effect, Effect::Visible) {
            if o.index_before != o.index_after {
                return Err(bad("an op without a visible effect moved"));
            }
            continue;
        }
        let take = |list: &mut Vec<(Key, Value)>, at: Option<usize>| -> Result<(), String> {
            let at = at.ok_or_else(|| bad("no index before"))?;
            match list.get(at) {
                Some((k, _)) if *k == o.key => {
                    list.remove(at);
                    Ok(())
                }
                other => Err(bad(&format!("index before holds {other:?}"))),
            }
        };
        let put = |list: &mut Vec<(Key, Value)>, at: Option<usize>| -> Result<(), String> {
            let at = at.ok_or_else(|| bad("no index after"))?;
            if at > list.len() {
                return Err(bad("index after past the end"));
            }
            let v = o.value_after.clone().ok_or_else(|| bad("no value after"))?;
            list.insert(at, (o.key.clone(), v));
            Ok(())
        };
        match o.kind {
            ListOpKind::Insert | ListOpKind::Restore => put(list, o.index_after)?,
            ListOpKind::Delete => take(list, o.index_before)?,
            ListOpKind::Move => {
                take(list, o.index_before)?;
                put(list, o.index_after)?;
            }
            // a deleted element leaves for good: nothing shown changes
            ListOpKind::Drop => {}
            ListOpKind::Set => {
                let at = o.index_after.ok_or_else(|| bad("no index"))?;
                let slot = list.get_mut(at).ok_or_else(|| bad("index past the end"))?;
                if slot.0 != o.key {
                    return Err(bad("index holds another key"));
                }
                slot.1 = o.value_after.clone().ok_or_else(|| bad("no value after"))?;
            }
        }
    }
    Ok(())
}

/// Replay map ops over a map.
pub fn replay_map<W: Debug>(
    map: &mut BTreeMap<String, Value>,
    ops: &[MapApplied<W>],
) -> Result<(), String> {
    for (i, o) in ops.iter().enumerate() {
        if map.get(&o.key) != o.before.as_ref() {
            return Err(format!(
                "op {i} ({o:?}): before is {:?}, the map holds {:?}",
                o.before,
                map.get(&o.key)
            ));
        }
        match (o.kind, &o.after) {
            (MapOpKind::Set, Some(v)) => {
                map.insert(o.key.clone(), v.clone());
            }
            (MapOpKind::Delete, None) => {
                map.remove(&o.key);
            }
            _ => return Err(format!("op {i}: {o:?}")),
        }
    }
    Ok(())
}
