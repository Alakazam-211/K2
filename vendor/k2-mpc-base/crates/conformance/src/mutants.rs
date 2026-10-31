//! Planted negatives for the structured rules: [`Mutated`], the kit's Doc (Yrs) with one
//! semantic broken the way a shortcut or another design would break it, chosen per test with
//! [`with_mutant`]. Each rule must fail on its mutant (`tests/structured.rs`), so its checks
//! are shown to bite. (A pinned rule's negative is its conforming check run on the deviating
//! engine: `run_strict`.)
//!
//! The mutants, after the engine draft's mutants and the CSV seam design's rejected
//! approaches (§5.1):
//! - [`Mutant::MoveAsReinsert`] (`move-duplicates`, approach A): a move deletes the element
//!   and inserts a copy under a new physical key; two concurrent moves leave two copies, and
//!   a set concurrent with a move lands on the dead original.
//! - [`Mutant::FlagLww`] (`delete-loses-to-move`, `undel-lww`): delete, restore and move write
//!   a liveness flag, last writer wins, so a move or a restore can beat a concurrent delete.
//! - [`Mutant::InsertFollowsAnchor`]: an insert follows its anchor to the anchor's new place
//!   when a concurrent move arrives (it re-moves itself after each merge).
//! - [`Mutant::ReverseChains`]: a forward chain of moves (a sort, a block move) is generated
//!   backwards: the same order, but its places are not one run.
//! - [`Mutant::Fractional`] (approach B): a list as fractional positions in a map; two blocks
//!   moved into one gap get the same positions and interleave.
//! - [`Mutant::LineDeleteDropsCells`]: a row or column delete also deletes its cells.
//! - [`Mutant::CellsByIndex`]: cells keyed by row and column index instead of key.
//! - [`Mutant::SortAsSteps`]: a change of several list ops is applied as one change per op.
//! - [`Mutant::MergedNoMoves`], [`Mutant::MergedNoMap`]: a merge's stream loses its moves, or
//!   its map ops.
//!
//! And one planted renderer, for `table.ragged_render`: [`render_absent_as_no_field`].

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::elem::{Anchor, Applied, ElemState, KeyedRoot, ListOp, MapOp, WriteId};
use k2_mpc_base::{ClientId, Edit, Key, Kind, Schema, Value};

use crate::caps::{EventRefusal, MLists, Maps, Merged, Structured, CELLS, COLS, ROWS};
use crate::yrs::{YrsGrid, YrsText};
use crate::{Refused, SEED_AGENT};

/// Which semantic a [`Mutated`] replica breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutant {
    MoveAsReinsert,
    FlagLww,
    InsertFollowsAnchor,
    ReverseChains,
    Fractional,
    LineDeleteDropsCells,
    CellsByIndex,
    SortAsSteps,
    MergedNoMoves,
    MergedNoMap,
}

thread_local! {
    static MUTANT: Cell<Option<Mutant>> = const { Cell::new(None) };
}

/// Run `f` with [`Mutated`] replicas made (on this thread) carrying mutant `m`.
pub fn with_mutant<T>(m: Mutant, f: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            MUTANT.with(|c| c.set(None));
        }
    }
    MUTANT.with(|c| c.set(Some(m)));
    let _reset = Reset;
    f()
}

/// The mutants' schema: the rules' roots plus `aux`, a map some mutants keep their own
/// state in.
const MUTANT_GRID: Schema = Schema {
    name: "conformance-grid-mutant",
    roots: &[
        (ROWS, Kind::MovableList),
        (COLS, Kind::MovableList),
        (CELLS, Kind::Map),
        (AUX, Kind::Map),
    ],
};
const AUX: &str = "aux";

/// A kit Doc replica with one semantic broken ([`Mutant`]).
pub struct Mutated {
    inner: YrsGrid,
    m: Mutant,
    counter: u64,
    /// [`Mutant::InsertFollowsAnchor`]: this replica's inserts and their anchors.
    follow: Vec<(String, Key, Anchor)>,
}

/// The logical key of a [`Mutant::MoveAsReinsert`] physical key (`<key>--<copy>`).
fn logical(k: &Key) -> &str {
    k.as_str().split("--").next().unwrap_or(k.as_str())
}

impl Mutated {
    fn keyed_list(&self, root: &str) -> (Vec<(Key, Value)>, BTreeMap<Key, Value>) {
        let s = self.inner.doc().keyed_state().expect("the keyed state");
        for (r, x) in s.roots {
            if r == root {
                if let KeyedRoot::List { live, hidden, .. } = x {
                    return (live, hidden);
                }
            }
        }
        panic!("no list root {root}")
    }

    fn aux(&self) -> BTreeMap<String, Value> {
        self.inner.map(AUX)
    }

    fn flagged(&self, root: &str, key: &str) -> bool {
        self.aux().get(&format!("{root}/{key}")) == Some(&Value::str("1"))
    }

    /// The changes `edits` become under this mutant, computed against the current state.
    fn transform(&mut self, edits: &[Edit]) -> Vec<Vec<Edit>> {
        match self.m {
            Mutant::MoveAsReinsert => vec![self.reinsert(edits)],
            Mutant::FlagLww => vec![edits.iter().flat_map(|e| self.flag(e)).collect()],
            Mutant::InsertFollowsAnchor => {
                for e in edits {
                    if let Edit::List { root, ops } = e {
                        for op in ops {
                            if let ListOp::Insert {
                                key,
                                at: at @ (Anchor::After(_) | Anchor::Before(_)),
                                ..
                            } = op
                            {
                                self.follow.push((root.clone(), key.clone(), at.clone()));
                            }
                        }
                    }
                }
                vec![edits.to_vec()]
            }
            Mutant::ReverseChains => vec![edits.iter().map(reverse_chains).collect()],
            Mutant::Fractional => vec![self.fractional(edits)],
            Mutant::LineDeleteDropsCells => {
                let map = self.inner.map(CELLS);
                let mut out = edits.to_vec();
                for e in edits {
                    if let Edit::List { root, ops } = e {
                        for op in ops {
                            if let ListOp::Delete { key } = op {
                                let drop: Vec<MapOp> = map
                                    .keys()
                                    .filter(|c| match c.split_once('\u{1f}') {
                                        Some((r, _)) if root == ROWS => r == key.as_str(),
                                        Some((_, c)) if root == COLS => c == key.as_str(),
                                        _ => false,
                                    })
                                    .map(|c| MapOp::Delete { key: c.clone() })
                                    .collect();
                                if !drop.is_empty() {
                                    out.push(Edit::Map {
                                        root: CELLS.into(),
                                        ops: drop,
                                    });
                                }
                            }
                        }
                    }
                }
                vec![out]
            }
            // CellsByIndex is applied in `edit` (its map keys need the lists after the list
            // edits); SortAsSteps splits list edits into one change per op
            Mutant::SortAsSteps => {
                let mut out = Vec::new();
                for e in edits {
                    match e {
                        Edit::List { root, ops } => {
                            for op in ops {
                                out.push(vec![Edit::List {
                                    root: root.clone(),
                                    ops: vec![op.clone()],
                                }]);
                            }
                        }
                        other => out.push(vec![other.clone()]),
                    }
                }
                out
            }
            _ => vec![edits.to_vec()],
        }
    }

    // ---- MoveAsReinsert ----

    fn reinsert(&mut self, edits: &[Edit]) -> Vec<Edit> {
        let mut out = Vec::new();
        for e in edits {
            let Edit::List { root, ops } = e else {
                out.push(e.clone());
                continue;
            };
            let (live, hidden) = self.keyed_list(root);
            // logical key -> (live physical keys, hidden physical keys)
            let mut phys: HashMap<String, (Vec<Key>, Vec<Key>)> = HashMap::new();
            for (k, _) in &live {
                phys.entry(logical(k).into()).or_default().0.push(k.clone());
            }
            for k in hidden.keys() {
                phys.entry(logical(k).into()).or_default().1.push(k.clone());
            }
            let values: HashMap<Key, Value> = live.into_iter().chain(hidden).collect();
            let one = |phys: &HashMap<String, (Vec<Key>, Vec<Key>)>, k: &Key| -> Key {
                match phys.get(k.as_str()) {
                    Some((l, _)) if !l.is_empty() => l[0].clone(),
                    Some((_, h)) if !h.is_empty() => h[h.len() - 1].clone(),
                    _ => k.clone(),
                }
            };
            let anchor = |phys: &HashMap<String, (Vec<Key>, Vec<Key>)>, a: &Anchor| match a {
                Anchor::After(k) => Anchor::After(one(phys, k)),
                Anchor::Before(k) => Anchor::Before(one(phys, k)),
                other => other.clone(),
            };
            let mut new = Vec::new();
            for op in ops {
                match op {
                    ListOp::Insert { key, at, value } => {
                        new.push(ListOp::Insert {
                            key: key.clone(),
                            at: anchor(&phys, at),
                            value: value.clone(),
                        });
                        phys.entry(key.as_str().into())
                            .or_default()
                            .0
                            .push(key.clone());
                    }
                    ListOp::Move { key, to } => {
                        let entry = phys.entry(key.as_str().into()).or_default();
                        if entry.0.is_empty() {
                            new.push(ListOp::Move {
                                key: one(&phys, key),
                                to: anchor(&phys, to),
                            });
                            continue;
                        }
                        let old = entry.0.remove(0);
                        entry.1.push(old.clone());
                        self.counter += 1;
                        let copy = Key::new(format!(
                            "{key}--{}x{}",
                            self.inner.agent() % 1000,
                            self.counter
                        ))
                        .expect("a copy's key");
                        let at = anchor(&phys, to);
                        new.push(ListOp::Delete { key: old.clone() });
                        new.push(ListOp::Insert {
                            key: copy.clone(),
                            at,
                            value: values.get(&old).cloned().unwrap_or_default(),
                        });
                        phys.entry(key.as_str().into()).or_default().0.push(copy);
                    }
                    ListOp::Delete { key } => {
                        let entry = phys.entry(key.as_str().into()).or_default();
                        for p in std::mem::take(&mut entry.0) {
                            new.push(ListOp::Delete { key: p.clone() });
                            entry.1.push(p);
                        }
                    }
                    ListOp::Restore { key } => {
                        let entry = phys.entry(key.as_str().into()).or_default();
                        if entry.0.is_empty() {
                            if let Some(p) = entry.1.pop() {
                                new.push(ListOp::Restore { key: p.clone() });
                                entry.0.push(p);
                            }
                        }
                    }
                    ListOp::Set { key, value } => {
                        let entry = phys.entry(key.as_str().into()).or_default();
                        let targets = if entry.0.is_empty() {
                            entry.1.last().cloned().into_iter().collect()
                        } else {
                            entry.0.clone()
                        };
                        for p in targets {
                            new.push(ListOp::Set {
                                key: p,
                                value: value.clone(),
                            });
                        }
                    }
                    other => new.push(other.clone()),
                }
            }
            out.push(Edit::List {
                root: root.clone(),
                ops: new,
            });
        }
        out
    }

    // ---- FlagLww ----

    fn flag(&self, e: &Edit) -> Vec<Edit> {
        let Edit::List { root, ops } = e else {
            return vec![e.clone()];
        };
        let mut list = Vec::new();
        let mut flags = Vec::new();
        let f = |k: &Key, v: &str| MapOp::Set {
            key: format!("{root}/{k}"),
            value: Value::str(v),
        };
        for op in ops {
            match op {
                ListOp::Delete { key } => flags.push(f(key, "1")),
                ListOp::Restore { key } => flags.push(f(key, "0")),
                ListOp::Move { key, .. } => {
                    list.push(op.clone());
                    flags.push(f(key, "0"));
                }
                other => list.push(other.clone()),
            }
        }
        let mut out = vec![Edit::List {
            root: root.clone(),
            ops: list,
        }];
        if !flags.is_empty() {
            out.push(Edit::Map {
                root: AUX.into(),
                ops: flags,
            });
        }
        out
    }

    // ---- Fractional ----

    /// (position, value, deleted) of every element of `root` the aux map holds.
    fn frac_state(&self, root: &str) -> BTreeMap<Key, (f64, Value, bool)> {
        let aux = self.aux();
        let mut out = BTreeMap::new();
        let pre = format!("{root}|p|");
        for (k, v) in &aux {
            if let Some(key) = k.strip_prefix(&pre) {
                let p: f64 = v.as_str().and_then(|s| s.parse().ok()).expect("a position");
                let key = Key::new(key).expect("a key");
                let val = aux
                    .get(&format!("{root}|v|{key}"))
                    .cloned()
                    .unwrap_or_default();
                let dead = aux.get(&format!("{root}|d|{key}")) == Some(&Value::str("1"));
                out.insert(key, (p, val, dead));
            }
        }
        out
    }

    fn frac_order(
        st: &BTreeMap<Key, (f64, Value, bool)>,
        without: Option<&Key>,
    ) -> Vec<(f64, Key)> {
        let mut v: Vec<(f64, Key)> = st
            .iter()
            .filter(|(k, _)| Some(*k) != without)
            .map(|(k, (p, _, _))| (*p, k.clone()))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        v
    }

    fn frac_place(
        st: &BTreeMap<Key, (f64, Value, bool)>,
        at: &Anchor,
        moving: Option<&Key>,
    ) -> f64 {
        const END: f64 = 1e12;
        let o = Self::frac_order(st, moving);
        let idx = |k: &Key| o.iter().position(|(_, x)| x == k).expect("a known anchor");
        let (lo, hi) = match at {
            Anchor::Start => (0.0, o.first().map_or(END, |x| x.0)),
            Anchor::End => (o.last().map_or(0.0, |x| x.0), END),
            Anchor::After(k) => {
                let i = idx(k);
                (o[i].0, o.get(i + 1).map_or(END, |x| x.0))
            }
            Anchor::Before(k) => {
                let i = idx(k);
                (if i == 0 { 0.0 } else { o[i - 1].0 }, o[i].0)
            }
        };
        (lo + hi) / 2.0
    }

    fn fractional(&self, edits: &[Edit]) -> Vec<Edit> {
        let mut out = Vec::new();
        for e in edits {
            let Edit::List { root, ops } = e else {
                out.push(e.clone());
                continue;
            };
            let mut st = self.frac_state(root);
            let mut sets: Vec<MapOp> = Vec::new();
            let put = |sets: &mut Vec<MapOp>, what: &str, k: &Key, v: Value| {
                sets.push(MapOp::Set {
                    key: format!("{root}|{what}|{k}"),
                    value: v,
                })
            };
            for op in ops {
                match op {
                    ListOp::Insert { key, at, value } => {
                        let p = Self::frac_place(&st, at, None);
                        st.insert(key.clone(), (p, value.clone(), false));
                        put(&mut sets, "p", key, Value::str(p.to_string()));
                        put(&mut sets, "v", key, value.clone());
                    }
                    ListOp::Move { key, to } => {
                        let p = Self::frac_place(&st, to, Some(key));
                        st.get_mut(key).expect("a known key").0 = p;
                        put(&mut sets, "p", key, Value::str(p.to_string()));
                    }
                    ListOp::Delete { key } => {
                        st.get_mut(key).expect("a known key").2 = true;
                        put(&mut sets, "d", key, Value::str("1"));
                    }
                    ListOp::Restore { key } => {
                        st.get_mut(key).expect("a known key").2 = false;
                        put(&mut sets, "d", key, Value::str("0"));
                    }
                    ListOp::Set { key, value } => {
                        st.get_mut(key).expect("a known key").1 = value.clone();
                        put(&mut sets, "v", key, value.clone());
                    }
                    other => panic!("the fractional mutant has no {other:?}"),
                }
            }
            out.push(Edit::Map {
                root: AUX.into(),
                ops: sets,
            });
        }
        out
    }

    // ---- CellsByIndex ----

    fn by_index(&self, ops: &[MapOp]) -> Vec<MapOp> {
        let idx = |root: &str, k: &str| {
            self.inner
                .list(root)
                .iter()
                .position(|(x, _)| x.as_str() == k)
        };
        ops.iter()
            .map(|op| {
                let key = op.key();
                let phys = match key.split_once('\u{1f}') {
                    Some((r, c)) => match (idx(ROWS, r), idx(COLS, c)) {
                        (Some(i), Some(j)) => format!("#{i}\u{1f}#{j}"),
                        _ => key.to_owned(),
                    },
                    None => key.to_owned(),
                };
                match op {
                    MapOp::Set { value, .. } => MapOp::Set {
                        key: phys,
                        value: value.clone(),
                    },
                    _ => MapOp::Delete { key: phys },
                }
            })
            .collect()
    }

    // ---- after a merge ----

    /// [`Mutant::InsertFollowsAnchor`]: put each own insert next to its anchor again.
    fn follow_anchors(&mut self) {
        if self.m != Mutant::InsertFollowsAnchor {
            return;
        }
        for (root, key, at) in self.follow.clone() {
            let l: Vec<Key> = self.inner.list(&root).into_iter().map(|(k, _)| k).collect();
            let pos = |k: &Key| l.iter().position(|x| x == k);
            let (Some(i), Some(a)) = (
                pos(&key),
                match &at {
                    Anchor::After(a) | Anchor::Before(a) => pos(a),
                    _ => None,
                },
            ) else {
                continue;
            };
            let next_to = match at {
                Anchor::After(_) => i == a + 1,
                _ => i + 1 == a,
            };
            if !next_to {
                self.inner
                    .edit(&[Edit::List {
                        root,
                        ops: vec![ListOp::Move { key, to: at }],
                    }])
                    .expect("follow the anchor");
            }
        }
    }

    fn filter(&self, a: Vec<Applied<Option<WriteId>>>) -> Vec<Applied<Option<WriteId>>> {
        match self.m {
            Mutant::MergedNoMoves => a
                .into_iter()
                .map(|x| match x {
                    Applied::List { root, mut ops } => {
                        ops.retain(|o| o.kind != k2_mpc_base::elem::ListOpKind::Move);
                        Applied::List { root, ops }
                    }
                    other => other,
                })
                .collect(),
            Mutant::MergedNoMap => a
                .into_iter()
                .filter(|x| !matches!(x, Applied::Map { .. }))
                .collect(),
            _ => a,
        }
    }
}

fn reverse_chains(e: &Edit) -> Edit {
    let Edit::List { root, ops } = e else {
        return e.clone();
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        let ListOp::Move {
            key: first,
            to: to @ (Anchor::Start | Anchor::After(_)),
        } = &ops[i]
        else {
            out.push(ops[i].clone());
            i += 1;
            continue;
        };
        let mut keys = vec![first.clone()];
        let mut j = i + 1;
        while let Some(ListOp::Move {
            key,
            to: Anchor::After(prev),
        }) = ops.get(j)
        {
            if prev != keys.last().expect("a key") {
                break;
            }
            keys.push(key.clone());
            j += 1;
        }
        for k in keys.iter().rev() {
            out.push(ListOp::Move {
                key: k.clone(),
                to: to.clone(),
            });
        }
        i = j;
    }
    Edit::List {
        root: root.clone(),
        ops: out,
    }
}

impl Structured for Mutated {
    type Engine = YrsText;
    type Version = Vec<(ClientId, u32)>;
    type Write = WriteId;

    fn replica(agent: u64, seed: &[Edit]) -> Self {
        let m = MUTANT
            .with(Cell::get)
            .expect("Mutated replicas are made inside with_mutant");
        let mut s = Mutated {
            inner: YrsGrid::with_schema(MUTANT_GRID, SEED_AGENT, &[]),
            m,
            counter: 0,
            follow: Vec::new(),
        };
        if !seed.is_empty() {
            s.edit(seed).expect("seed the mutant");
        }
        Mutated {
            inner: s.inner.rebased(agent),
            m,
            counter: 0,
            follow: Vec::new(),
        }
    }

    fn edit(&mut self, edits: &[Edit]) -> Result<Vec<Applied<WriteId>>, Refused> {
        if self.m == Mutant::CellsByIndex {
            let (maps, other): (Vec<&Edit>, Vec<&Edit>) = edits
                .iter()
                .partition(|e| matches!(e, Edit::Map { root, .. } if root == CELLS));
            let mut out = Vec::new();
            if !other.is_empty() {
                let other: Vec<Edit> = other.into_iter().cloned().collect();
                out.extend(self.inner.edit(&other)?);
            }
            for e in maps {
                if let Edit::Map { root, ops } = e {
                    let ops = self.by_index(ops);
                    out.extend(self.inner.edit(&[Edit::Map {
                        root: root.clone(),
                        ops,
                    }])?);
                }
            }
            return Ok(out);
        }
        let mut out = Vec::new();
        for change in self.transform(edits) {
            out.extend(self.inner.edit(&change)?);
        }
        Ok(out)
    }

    fn sync_from(&mut self, from: &Self) {
        self.inner.sync_from(&from.inner);
        self.follow_anchors();
    }

    fn version(&self) -> Self::Version {
        self.inner.version()
    }

    fn agent_of(w: &WriteId) -> u64 {
        w.client()
    }

    fn reloaded(&self) -> Self {
        Mutated {
            inner: self.inner.reloaded(),
            m: self.m,
            counter: self.counter,
            follow: self.follow.clone(),
        }
    }
}

impl MLists for Mutated {
    fn list(&self, root: &str) -> Vec<(Key, Value)> {
        match self.m {
            Mutant::MoveAsReinsert => self
                .inner
                .list(root)
                .into_iter()
                .map(|(k, v)| (Key::new(logical(&k)).expect("a key"), v))
                .collect(),
            Mutant::FlagLww => self
                .inner
                .list(root)
                .into_iter()
                .filter(|(k, _)| !self.flagged(root, k.as_str()))
                .collect(),
            Mutant::Fractional => {
                let st = self.frac_state(root);
                Self::frac_order(&st, None)
                    .into_iter()
                    .filter(|(_, k)| !st[k].2)
                    .map(|(_, k)| {
                        let v = st[&k].1.clone();
                        (k, v)
                    })
                    .collect()
            }
            _ => self.inner.list(root),
        }
    }

    fn elem(&self, root: &str, key: &Key) -> ElemState {
        let known = match self.m {
            Mutant::MoveAsReinsert => {
                let (l, h) = self.keyed_list(root);
                l.iter()
                    .map(|(k, _)| k)
                    .chain(h.keys())
                    .any(|k| logical(k) == key.as_str())
            }
            Mutant::FlagLww => self.inner.elem(root, key) != ElemState::Unknown,
            Mutant::Fractional => self.frac_state(root).contains_key(key),
            _ => return self.inner.elem(root, key),
        };
        match self.list(root).iter().position(|(k, _)| k == key) {
            Some(i) => ElemState::Live(i),
            None if known => ElemState::Deleted,
            None => ElemState::Unknown,
        }
    }

    fn place_runs(&self, root: &str, keys: &[Key]) -> Option<usize> {
        self.inner.place_runs(root, keys)
    }
}

impl Maps for Mutated {
    fn map(&self, root: &str) -> BTreeMap<String, Value> {
        if self.m != Mutant::CellsByIndex || root != CELLS {
            return self.inner.map(root);
        }
        let rows = self.inner.list(ROWS);
        let cols = self.inner.list(COLS);
        let mut out = BTreeMap::new();
        for (k, v) in self.inner.map(root) {
            let logical = match k.split_once('\u{1f}') {
                Some((r, c)) if r.starts_with('#') && c.starts_with('#') => {
                    let (i, j): (usize, usize) = (
                        r[1..].parse().expect("an index"),
                        c[1..].parse().expect("an index"),
                    );
                    match (rows.get(i), cols.get(j)) {
                        (Some((r, _)), Some((c, _))) => format!("{r}\u{1f}{c}"),
                        _ => continue,
                    }
                }
                _ => k,
            };
            out.insert(logical, v);
        }
        out
    }

    fn map_winner(&self, root: &str, key: &str) -> Option<WriteId> {
        self.inner.map_winner(root, key)
    }
}

impl Merged for Mutated {
    type Change = Vec<u8>;

    fn own_changes(&self) -> Vec<Vec<u8>> {
        self.inner.own_changes()
    }

    fn deliver(&mut self, change: &Vec<u8>) -> Result<Vec<Applied<Option<WriteId>>>, EventRefusal> {
        let a = self.inner.deliver(change)?;
        self.follow_anchors();
        Ok(self.filter(a))
    }

    fn sync_applied(&mut self, from: &Self) -> Vec<Applied<Option<WriteId>>> {
        let a = self.inner.sync_applied(&from.inner);
        self.follow_anchors();
        self.filter(a)
    }
}

/// A renderer that writes a row's present cells only: an absent cell gets no field, so a row
/// added concurrently with a column is short in a `rect` table, and a hole in a `ragged`
/// row closes up (`table.ragged_render`'s planted negative).
pub fn render_absent_as_no_field(t: &k2_mpc_base::table::Table) -> Vec<k2_mpc_base::table::Record> {
    let cols = t.live_cols();
    t.live_rows()
        .iter()
        .map(|r| {
            cols.iter()
                .filter_map(|c| t.cell(r, c))
                .map(k2_mpc_base::table::Field::new)
                .collect()
        })
        .collect()
}
