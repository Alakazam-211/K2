//! The undo spec's **keyed-grid model** (spec v1.11 §6a, S1–S12): undo, redo and revert of
//! structured changes over plain data, written from the spec. The oracle the grid trace test
//! (`tests/grid_traces.rs`) compares the kit's sessions against, after every step.
//!
//! The state is plain: per list root, its elements' places in order (deleted ones kept where
//! they sit), the deleted set and the values; per map root, its keys and values. Every facet
//! has a **holder** (the change that wrote it, and the write it restores, S2). Every change
//! keeps its **record**: its ops by key, each with the live key before it, its values before
//! and after, the facet's holder before it, and what it restores (S1). Stacks hold changes;
//! undo, redo and revert all **reverse a change's record** (S9, S10).
//!
//! It shares the kit's **data types** (`Edit`, `ListOp`, `MapOp`, `Key`, `Value`) and nothing
//! of the kit's undo: the per-op semantics (insert after an anchor's place, deleted elements
//! keep theirs; a move, a delete, a restore, a set, a drop), the holders, the records and the
//! rules are written here again.
//!
//! **Mutants** ([`GridMutant`], spec §8 (xv)–(xxvi)) are wrong variants of the rules,
//! selectable per model (test builds only, the `mutants` feature): the grid trace test must
//! find each one disagreeing with the kit.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::Edit;

/// A change's position in the log (the kit's change id).
pub type Seq = u64;

/// A facet (S1); `Insert` is who first put an element there (`blame --structure`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Facet {
    Value,
    Position,
    Existence,
    Insert,
}

/// Who holds a facet: change `seq`'s write, counting as change `restores`' when it put that
/// write back (S2, path-compressed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Holder {
    pub seq: Seq,
    pub restores: Option<Seq>,
}

impl Holder {
    /// The change the write counts as (S2).
    pub fn resolved(&self) -> Seq {
        self.restores.unwrap_or(self.seq)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Insert,
    Move,
    Delete,
    Restore,
    Set,
    Drop,
    MapSet,
    MapDelete,
}

impl Kind {
    fn facet(self) -> Facet {
        match self {
            Kind::Insert | Kind::Move => Facet::Position,
            Kind::Delete | Kind::Restore | Kind::Drop => Facet::Existence,
            Kind::Set | Kind::MapSet | Kind::MapDelete => Facet::Value,
        }
    }
}

/// One op of a change, by key (S1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecOp {
    pub root: String,
    pub kind: Kind,
    pub key: String,
    /// A move: its left neighbours in place order before the op, nearest first, deleted ones
    /// included, up to and including the nearest live one, at most [`LEFT_CHAIN`] (S4).
    pub left: Vec<Key>,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub prev: Option<Holder>,
    pub restores: Option<Seq>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListRoot {
    pub places: Vec<Key>,
    pub dead: BTreeSet<Key>,
    pub vals: BTreeMap<Key, Value>,
}

impl ListRoot {
    pub fn live(&self) -> Vec<Key> {
        self.places
            .iter()
            .filter(|k| !self.dead.contains(*k))
            .cloned()
            .collect()
    }
    fn knows(&self, k: &Key) -> bool {
        self.vals.contains_key(k)
    }
    fn place_of(&self, k: &Key) -> Option<usize> {
        self.places.iter().position(|x| x == k)
    }
}

/// The wrong variants (spec §8, v1.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridMutant {
    None,
    /// (xv) conflict judged by value: §6's "live value ≠ after".
    CompareValues,
    /// (xvi) a move judged by its index, not its position holder.
    CompareIndices,
    /// (xvii) undo of a delete inserts a fresh key (the hidden cells stay behind).
    DeleteUndoneByNewKey,
    /// (xviii) redo retries the regions its undo skipped.
    RedoRetriesSkipped,
    /// (xix) own judged by author, not write identity.
    OwnByPid,
    /// (xx) the holder taken without `restores_write`.
    RestoresNotResolved,
    /// (xxi) a restore in a reversal (delete → restore) by a fresh key: the amendment's
    /// `undo-restore-by-new-key` (fails `table.cell_kept`).
    UndoRestoreByNewKey,
    /// (xxii) undo of a batch touching a dropped element re-inserts it.
    DropRevived,
    /// (xxiii) undo of a sort restores the whole old order.
    SortUndoWholeOrder,
    /// (xxiv) the header switch undone per facet under `--partial`.
    HeaderSplit,
    /// (xxv) a write with no identity counted as own (the class `write_with_no_identity`:
    /// an outside save whose writes the kit records without identity, a Yrs-only case).
    NoIdentityIsOwn,
    /// (xxvi) `already` judged by the holder instead of the live state.
    AlreadyByHolder,
    /// (xxxi) an undo, redo or revert writes a cell under a dropped row or column (S11).
    PartialRevertWritesDroppedKey,
    /// (xxix) an undo merges by cell over a row a text re-paired, later edits or not (S13).
    UndoCellMergesAfterTextPair,
    /// (xxx) a revert merges by cell over such a row, instead of rewriting it whole (S13).
    RevertCellMergesAfterTextPair,
    /// (xxviii) a move goes back after the nearest live key before its old left neighbour (v1.11's
    /// first wording), not after the neighbour itself when it is deleted.
    AnchorOnNearestLive,
}

/// At most this many left neighbours are recorded for a move (S4; the kit's `LEFT_CHAIN`).
pub const LEFT_CHAIN: usize = 8;

impl GridMutant {
    pub const ALL: [(&'static str, GridMutant); 16] = [
        ("xv-compare-values", GridMutant::CompareValues),
        ("xvi-compare-indices", GridMutant::CompareIndices),
        (
            "xvii-delete-undone-by-new-key",
            GridMutant::DeleteUndoneByNewKey,
        ),
        ("xviii-redo-retries-skipped", GridMutant::RedoRetriesSkipped),
        ("xix-own-by-pid", GridMutant::OwnByPid),
        ("xx-restores-not-resolved", GridMutant::RestoresNotResolved),
        (
            "xxi-undo-restore-by-new-key",
            GridMutant::UndoRestoreByNewKey,
        ),
        ("xxii-drop-revived", GridMutant::DropRevived),
        (
            "xxiii-sort-undo-whole-order",
            GridMutant::SortUndoWholeOrder,
        ),
        ("xxiv-header-split", GridMutant::HeaderSplit),
        ("xxv-no-identity-is-own", GridMutant::NoIdentityIsOwn),
        ("xxvi-already-by-holder", GridMutant::AlreadyByHolder),
        (
            "xxviii-anchor-on-nearest-live",
            GridMutant::AnchorOnNearestLive,
        ),
        (
            "xxix-undo-cell-merges-after-text-pair",
            GridMutant::UndoCellMergesAfterTextPair,
        ),
        (
            "xxx-restore-cell-merges",
            GridMutant::RevertCellMergesAfterTextPair,
        ),
        (
            "xxxi-partial-revert-writes-dropped-key",
            GridMutant::PartialRevertWritesDroppedKey,
        ),
    ];
}

/// What an undo, redo or revert came to (as the kit's `UndoOutcome`, by kind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `steps` taken; `skipped` regions (with `--partial`) as `(root, key, why)`, sorted;
    /// `blocked` when a later step's every region conflicted.
    Applied {
        steps: usize,
        skipped: Vec<(String, String, String)>,
        blocked: bool,
    },
    Conflicts {
        step: usize,
        conflicts: Vec<(String, String, String)>,
    },
    Blocked {
        conflicts: Vec<(String, String, String)>,
    },
    Dropped,
    Nothing,
    HistoryLost,
}

/// One region's result: `Ok(None)` `already`, `Ok(Some(inverse))`, or `Err(why)`.
type RegionResult = Result<Option<(Option<ListOp>, Option<MapOp>)>, &'static str>;

/// One undo-stack entry: a change, and whom it was made for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    seq: Seq,
    obo: Option<String>,
}

/// The model.
#[derive(Debug, Clone)]
pub struct Grid {
    pub lists: BTreeMap<String, ListRoot>,
    pub maps: BTreeMap<String, BTreeMap<String, Value>>,
    pub holders: HashMap<(String, String, Facet), Holder>,
    pub records: HashMap<Seq, Vec<RecOp>>,
    pub authors: HashMap<Seq, String>,
    /// Whom a participant acts for (an agent joined on a person's behalf): every change it
    /// makes is on that person's behalf ([`Grid::on_behalf_of`]).
    pub acts_for: HashMap<String, String>,
    pub seq: Seq,
    stacks: HashMap<String, (Vec<Entry>, Vec<Entry>)>,
    loaded_seq: Seq,
    pub mutant: GridMutant,
    /// Under (xvii)/(xxi): fresh keys minted by a wrong restore.
    fresh: u64,
    /// Depth limit per stack (U6), as the kit's config.
    pub depth: usize,
    /// Changes whose writes carry no identity (a write merged in from elsewhere whose writer
    /// the engine doesn't report: KC7, Yrs only, S2). Their writes hold facets like any
    /// other's and count as someone else's.
    pub anon: std::collections::HashSet<Seq>,
    /// S13: the latest change that left each row on its key by a non-exact pairing, or changed
    /// a marked row by undo, redo or revert; and the latest that remapped the columns.
    pub marks: BTreeMap<String, Seq>,
    pub remap: Option<Seq>,
    /// Undo, redo and revert steps: their marks aren't a later text's.
    reversals: BTreeSet<Seq>,
    /// The cells map just before each change (revert's whole rows, S13).
    cells_before: BTreeMap<Seq, BTreeMap<String, Value>>,
    /// How often S13 decided (coverage: the trace test's floors): an undo or redo held a row, a
    /// revert rewrote one whole.
    pub s13_held: u64,
    pub s13_whole: u64,
}

/// What reversing one record plans (one step).
struct Plan {
    edits: Vec<Edit>,
    restores: HashMap<(String, String, Facet), Seq>,
    conflicts: Vec<(String, String, String)>,
    applied: usize,
    /// (xviii): the inverse ops of the conflicting regions, which a wrong redo would add.
    skipped_ops: Vec<RecOp>,
}

impl Grid {
    /// A grid with these roots (lists, then maps), empty.
    pub fn new(lists: &[&str], maps: &[&str]) -> Self {
        Grid {
            lists: lists
                .iter()
                .map(|r| ((*r).to_owned(), ListRoot::default()))
                .collect(),
            maps: maps
                .iter()
                .map(|r| ((*r).to_owned(), BTreeMap::new()))
                .collect(),
            holders: HashMap::new(),
            records: HashMap::new(),
            authors: HashMap::new(),
            acts_for: HashMap::new(),
            seq: 0,
            stacks: HashMap::new(),
            loaded_seq: 0,
            mutant: GridMutant::None,
            fresh: 0,
            depth: 100,
            anon: Default::default(),
            marks: BTreeMap::new(),
            remap: None,
            reversals: BTreeSet::new(),
            cells_before: BTreeMap::new(),
            s13_held: 0,
            s13_whole: 0,
        }
    }

    // ----- applying a change -----

    /// Apply `edits` as change `seq + 1` by `pid` (an import, an edit, an outside save, an
    /// undo, redo or revert step), with `restores` per facet; record it. Returns the change's
    /// seq, or `None` when nothing was written (no seq taken).
    pub fn apply(
        &mut self,
        pid: &str,
        edits: &[Edit],
        restores: &HashMap<(String, String, Facet), Seq>,
    ) -> Option<Seq> {
        let seq = self.seq + 1;
        let cells_before = self.maps.get("cells").cloned().unwrap_or_default();
        let mut rec = Vec::new();
        for e in edits {
            match e {
                Edit::List { root, ops } => {
                    for op in ops {
                        if let Some(r) = self.apply_list(seq, root, op, restores) {
                            rec.push(r);
                        }
                    }
                }
                Edit::Map { root, ops } => {
                    for op in ops {
                        if let Some(r) = self.apply_map(seq, root, op, restores) {
                            rec.push(r);
                        }
                    }
                }
                _ => panic!("the grid model takes list and map edits"),
            }
        }
        if rec.is_empty() {
            return None;
        }
        self.seq = seq;
        self.cells_before.insert(seq, cells_before);
        self.records.insert(seq, rec);
        self.authors.insert(seq, pid.to_owned());
        Some(seq)
    }

    fn hold(
        &mut self,
        root: &str,
        key: &str,
        f: Facet,
        seq: Seq,
        restores: &HashMap<(String, String, Facet), Seq>,
    ) {
        let r = restores.get(&(root.to_owned(), key.to_owned(), f)).copied();
        self.holders.insert(
            (root.to_owned(), key.to_owned(), f),
            Holder { seq, restores: r },
        );
    }

    fn holder(&self, root: &str, key: &str, f: Facet) -> Option<Holder> {
        self.holders
            .get(&(root.to_owned(), key.to_owned(), f))
            .copied()
    }

    fn apply_list(
        &mut self,
        seq: Seq,
        root: &str,
        op: &ListOp,
        restores: &HashMap<(String, String, Facet), Seq>,
    ) -> Option<RecOp> {
        let l = self.lists.get(root).expect("a list root");
        let key = op.key().clone();
        // a move's left neighbours in place order, nearest first, to the nearest live one
        let mut left = Vec::new();
        if matches!(op, ListOp::Move { .. }) {
            if let Some(at) = l.place_of(&key) {
                for k in l.places[..at].iter().rev() {
                    left.push(k.clone());
                    if !l.dead.contains(k) || left.len() == LEFT_CHAIN {
                        break;
                    }
                }
            }
        }
        let value = l.vals.get(&key).cloned();
        let anchor_at = |l: &ListRoot, at: &Anchor| match at {
            Anchor::Start => 0,
            Anchor::End => l.places.len(),
            Anchor::After(a) => l.place_of(a).map_or(l.places.len(), |i| i + 1),
            Anchor::Before(a) => l.place_of(a).unwrap_or(0),
        };
        let (kind, before, after) = match op {
            ListOp::Insert { at, value: v, .. } => {
                let l = self.lists.get_mut(root).expect("a list root");
                assert!(!l.knows(&key), "the generator never reuses a key");
                let p = anchor_at(l, at);
                l.places.insert(p, key.clone());
                l.vals.insert(key.clone(), v.clone());
                (Kind::Insert, None, Some(v.clone()))
            }
            ListOp::Move { to, .. } => {
                let l = self.lists.get_mut(root).expect("a list root");
                let from = l.place_of(&key).expect("a known key");
                l.places.remove(from);
                let p = anchor_at(l, to);
                l.places.insert(p, key.clone());
                (Kind::Move, value.clone(), value)
            }
            ListOp::Delete { .. } => {
                let l = self.lists.get_mut(root).expect("a list root");
                if !l.dead.insert(key.clone()) {
                    return None;
                }
                (Kind::Delete, value.clone(), value)
            }
            ListOp::Restore { .. } => {
                let l = self.lists.get_mut(root).expect("a list root");
                if !l.dead.remove(&key) {
                    return None;
                }
                (Kind::Restore, value.clone(), value)
            }
            ListOp::Set { value: v, .. } => {
                let l = self.lists.get_mut(root).expect("a list root");
                l.vals.insert(key.clone(), v.clone());
                (Kind::Set, value, Some(v.clone()))
            }
            ListOp::Drop { .. } => {
                let l = self.lists.get_mut(root).expect("a list root");
                assert!(l.dead.remove(&key), "only a deleted element drops");
                let at = l.place_of(&key).expect("a known key");
                l.places.remove(at);
                l.vals.remove(&key);
                (Kind::Drop, value, None)
            }
            _ => panic!("an op the grid model doesn't know"),
        };
        let k = key.as_str().to_owned();
        let prev = self.holder(root, &k, kind.facet());
        match kind {
            Kind::Insert => {
                for f in [Facet::Insert, Facet::Position, Facet::Existence] {
                    self.hold(root, &k, f, seq, restores);
                }
                if after.as_ref().is_some_and(|v| *v != Value::Null) {
                    self.hold(root, &k, Facet::Value, seq, restores);
                }
            }
            Kind::Drop => {
                for f in [
                    Facet::Value,
                    Facet::Position,
                    Facet::Existence,
                    Facet::Insert,
                ] {
                    self.holders.remove(&(root.to_owned(), k.clone(), f));
                }
            }
            kind => self.hold(root, &k, kind.facet(), seq, restores),
        }
        Some(RecOp {
            root: root.to_owned(),
            kind,
            key: k.clone(),
            left,
            before,
            after,
            prev,
            restores: restores.get(&(root.to_owned(), k, kind.facet())).copied(),
        })
    }

    fn apply_map(
        &mut self,
        seq: Seq,
        root: &str,
        op: &MapOp,
        restores: &HashMap<(String, String, Facet), Seq>,
    ) -> Option<RecOp> {
        let m = self.maps.get_mut(root).expect("a map root");
        let key = op.key().to_owned();
        let before = m.get(&key).cloned();
        let (kind, after) = match op {
            MapOp::Set { value, .. } => {
                m.insert(key.clone(), value.clone());
                (Kind::MapSet, Some(value.clone()))
            }
            MapOp::Delete { .. } => {
                m.remove(&key)?;
                (Kind::MapDelete, None)
            }
            _ => panic!("an op the grid model doesn't know"),
        };
        let prev = self.holder(root, &key, Facet::Value);
        self.hold(root, &key, Facet::Value, seq, restores);
        Some(RecOp {
            root: root.to_owned(),
            kind,
            key: key.clone(),
            left: Vec::new(),
            before,
            after,
            prev,
            restores: restores.get(&(root.to_owned(), key, Facet::Value)).copied(),
        })
    }

    // ----- stacks -----

    /// A participant's change goes on their undo stack (U1, U7: it clears their redo).
    pub fn push_edit(&mut self, pid: &str, seq: Seq, obo: Option<&str>) {
        let depth = self.depth;
        let s = self.stacks.entry(pid.to_owned()).or_default();
        s.1.clear();
        s.0.push(Entry {
            seq,
            obo: obo.map(str::to_owned),
        });
        if s.0.len() > depth {
            s.0.remove(0);
        }
    }

    /// Whom change `seq` was made for: its author's person, when its author acts for one
    /// (an edit, an undo, redo or revert step alike); `None` otherwise.
    pub fn on_behalf_of(&self, seq: Seq) -> Option<&str> {
        let a = self.authors.get(&seq)?;
        self.acts_for.get(a).map(String::as_str)
    }

    pub fn depths(&self, pid: &str) -> (usize, usize) {
        self.stacks
            .get(pid)
            .map_or((0, 0), |s| (s.0.len(), s.1.len()))
    }

    /// A restart (U6): every stack is lost; what was made before it is history lost.
    pub fn restart(&mut self) {
        self.stacks.clear();
        self.loaded_seq = self.seq;
    }

    // ----- the rules (§6a) -----

    /// Where a move goes back to (S4): right after the first of `left` the list still knows
    /// (deleted or not; a dropped key isn't), or the start. (xxviii): after the nearest live key
    /// at or before that key's place.
    fn back_after(&self, l: &ListRoot, left: &[Key], moving: &Key) -> Anchor {
        let Some(at) = left
            .iter()
            .filter(|k| *k != moving)
            .find_map(|k| l.place_of(k))
        else {
            return Anchor::Start;
        };
        if self.mutant != GridMutant::AnchorOnNearestLive {
            return Anchor::After(l.places[at].clone());
        }
        l.places[..=at]
            .iter()
            .rev()
            .find(|k| !l.dead.contains(*k) && *k != moving)
            .map_or(Anchor::Start, |k| Anchor::After(k.clone()))
    }

    fn list_state(&self, cur: &Grid, root: &str, k: &Key) -> Option<bool> {
        let _ = self;
        let l = cur.lists.get(root)?;
        l.knows(k).then(|| !l.dead.contains(k))
    }

    /// One region, judged on `cur` (S3a, S3, S4). `Ok(inverse)` or `Err(why)`; `None` for
    /// `already`.
    fn region(
        &self,
        cur: &Grid,
        own: Seq,
        own_pid: &str,
        o: &RecOp,
        is_table: bool,
    ) -> RegionResult {
        let mine = o.restores.unwrap_or(own);
        let resolve = |h: &Holder| match self.mutant {
            GridMutant::RestoresNotResolved => h.seq,
            _ => h.resolved(),
        };
        // (xxv): a holder with no identity taken as own
        let no_identity_own = |h: &Holder| {
            self.mutant == GridMutant::NoIdentityIsOwn
                && h.restores.is_none()
                && self.anon.contains(&h.seq)
        };
        let own_h = |h: Option<Holder>| match self.mutant {
            GridMutant::OwnByPid => {
                h.is_some_and(|h| self.authors.get(&h.seq).map(String::as_str) == Some(own_pid))
            }
            _ => h.is_some_and(|h| resolve(&h) == mine || no_identity_own(&h)),
        };
        let already_by_holder = self.mutant == GridMutant::AlreadyByHolder;
        let by_value = self.mutant == GridMutant::CompareValues;
        let root = o.root.as_str();
        // S11: a cell (or its quoting) whose row or column is dropped: nothing written there
        // ((xxxi) writes it anyway)
        if is_table
            && matches!(root, "cells" | "quoting")
            && self.mutant != GridMutant::PartialRevertWritesDroppedKey
        {
            if let Some((r, c)) = k2_mpc_base::table::parse_cell_key(&o.key) {
                let knows = |root: &str, k: &Key| cur.lists.get(root).is_some_and(|l| l.knows(k));
                if !knows("rows", &r) || !knows("cols", &c) {
                    return Err("dropped");
                }
            }
        }
        if matches!(o.kind, Kind::MapSet | Kind::MapDelete) {
            let live = cur.maps.get(root).and_then(|m| m.get(&o.key)).cloned();
            let h = cur.holder(root, &o.key, Facet::Value);
            let already = if already_by_holder {
                h.is_some_and(|h| resolve(&h) == o.prev.map_or(0, |p| p.resolved()))
            } else {
                live == o.before
            };
            if already {
                return Ok(None);
            }
            let ok = if by_value { live == o.after } else { own_h(h) };
            if !ok {
                return Err("written since");
            }
            let op = match &o.before {
                Some(v) => MapOp::Set {
                    key: o.key.clone(),
                    value: v.clone(),
                },
                None => MapOp::Delete { key: o.key.clone() },
            };
            return Ok(Some((None, Some(op))));
        }
        let k = Key::new(o.key.as_str()).expect("a list key");
        let state = self.list_state(cur, root, &k);
        if o.kind == Kind::Drop {
            return Err("dropped");
        }
        let Some(live) = state else {
            if self.mutant == GridMutant::DropRevived {
                return Ok(Some((
                    Some(ListOp::Insert {
                        key: self.fresh_key(),
                        at: Anchor::End,
                        value: o.before.clone().unwrap_or_default(),
                    }),
                    None,
                )));
            }
            return Err("dropped");
        };
        let lst = |op| Ok(Some((Some(op), None)));
        match o.kind {
            Kind::Insert => {
                if !live {
                    return Ok(None);
                }
                for f in [Facet::Position, Facet::Value, Facet::Existence] {
                    let h = cur.holder(root, &o.key, f);
                    if h.is_some() && !own_h(h) {
                        return Err(if f == Facet::Existence {
                            "restored since"
                        } else {
                            "written since"
                        });
                    }
                }
                if is_table && root == "rows" {
                    let prefix = format!("{}\u{1f}", o.key);
                    for ((r, ck, f), h) in &cur.holders {
                        if r == "cells"
                            && *f == Facet::Value
                            && ck.starts_with(&prefix)
                            && h.seq > own
                            && resolve(h) != own
                            && !no_identity_own(h)
                        {
                            return Err("cell written since");
                        }
                    }
                }
                lst(ListOp::Delete { key: k })
            }
            Kind::Delete => {
                if live {
                    return Ok(None);
                }
                if matches!(
                    self.mutant,
                    GridMutant::DeleteUndoneByNewKey | GridMutant::UndoRestoreByNewKey
                ) {
                    return lst(ListOp::Insert {
                        key: self.fresh_key(),
                        at: Anchor::End,
                        value: o.before.clone().unwrap_or_default(),
                    });
                }
                lst(ListOp::Restore { key: k })
            }
            Kind::Restore => {
                if !live {
                    return Ok(None);
                }
                if !own_h(cur.holder(root, &o.key, Facet::Existence)) {
                    return Err("existence written since");
                }
                lst(ListOp::Delete { key: k })
            }
            Kind::Move => {
                if !live {
                    return Err("deleted since");
                }
                let l = cur.lists.get(root).expect("a list");
                let to = self.back_after(l, &o.left, &k);
                let lv = l.live();
                let at = lv.iter().position(|x| *x == k).expect("live");
                // S3a: already right after the anchor in place order
                let p = l.place_of(&k).expect("a known key");
                let sits = match &to {
                    Anchor::Start => p == 0,
                    Anchor::After(a) => p > 0 && l.places[p - 1] == *a,
                    _ => false,
                };
                if sits {
                    return Ok(None);
                }
                let ok = match self.mutant {
                    GridMutant::CompareIndices => {
                        // judged by the index it was moved to: unmoved iff it's still there
                        if o.left.is_empty() {
                            at == 0
                        } else {
                            true
                        }
                    }
                    _ => own_h(cur.holder(root, &o.key, Facet::Position)),
                };
                if !ok {
                    return Err("moved since");
                }
                lst(ListOp::Move { key: k, to })
            }
            Kind::Set => {
                let livev = cur
                    .lists
                    .get(root)
                    .and_then(|l| l.vals.get(&k))
                    .cloned()
                    .unwrap_or_default();
                let before = o.before.clone().unwrap_or_default();
                if livev == before {
                    return Ok(None);
                }
                let ok = if by_value {
                    Some(livev) == o.after
                } else {
                    own_h(cur.holder(root, &o.key, Facet::Value))
                };
                if !ok {
                    return Err("written since");
                }
                lst(ListOp::Set {
                    key: k,
                    value: before,
                })
            }
            Kind::MapSet | Kind::MapDelete | Kind::Drop => unreachable!("handled above"),
        }
    }

    fn fresh_key(&self) -> Key {
        Key::new(format!("z{}", self.fresh + self.seq)).expect("a key")
    }

    /// Plan reversing change `own`'s record on `self` (S1–S7): newest op first, each judged
    /// on what the earlier ones leave.
    fn plan(
        &self,
        own: Seq,
        own_pid: &str,
        is_table: bool,
        revert: bool,
        (mut rewritten, mut remap): (Vec<String>, bool),
    ) -> Plan {
        let off = if revert {
            GridMutant::RevertCellMergesAfterTextPair
        } else {
            GridMutant::UndoCellMergesAfterTextPair
        };
        if self.mutant == off {
            rewritten.clear();
            remap = false;
        }
        let row_of = |o: &RecOp| match o.root.as_str() {
            "rows" => Some(o.key.clone()),
            "cells" | "quoting" => {
                k2_mpc_base::table::parse_cell_key(&o.key).map(|(r, _)| r.to_string())
            }
            _ => None,
        };
        let held = |o: &RecOp| remap || row_of(o).is_some_and(|r| rewritten.contains(&r));
        let revert_whole = revert && !remap;
        let rec = self.records.get(&own).cloned().unwrap_or_default();
        let mut cur = self.clone();
        let mut roots: Vec<(String, Vec<ListOp>, Vec<MapOp>)> = Vec::new();
        let mut plan = Plan {
            edits: Vec::new(),
            restores: HashMap::new(),
            conflicts: Vec::new(),
            applied: 0,
            skipped_ops: Vec::new(),
        };
        let mut regions = Vec::new();
        let sort_whole = self.mutant == GridMutant::SortUndoWholeOrder
            && rec.iter().filter(|o| o.kind == Kind::Move).count() > 1;
        for o in rec.iter().rev() {
            if held(o) {
                if !revert_whole {
                    // S11 before S13 (v1.14): an op on a row (or its cells) the doc has
                    // dropped is `dropped`, whatever re-paired it
                    let dropped = row_of(o).is_some_and(|r| {
                        let k = Key::new(r.as_str()).expect("a key");
                        !self.lists.get("rows").is_some_and(|l| l.knows(&k))
                    });
                    regions.push((
                        o.clone(),
                        Err(if dropped { "dropped" } else { "rewritten_row" }),
                    ));
                }
                continue;
            }
            let r = if sort_whole && o.kind == Kind::Move {
                // (xxiii): every move put back, whoever moved it since
                let k = Key::new(o.key.as_str()).expect("a key");
                let l = cur.lists.get(&o.root).expect("a list");
                Ok(Some((
                    Some(ListOp::Move {
                        to: self.back_after(l, &o.left, &k),
                        key: k,
                    }),
                    None,
                )))
            } else {
                self.region(&cur, own, own_pid, o, is_table)
            };
            if let Ok(Some((l, m))) = &r {
                let mut e = Vec::new();
                if let Some(op) = l {
                    e.push(Edit::List {
                        root: o.root.clone(),
                        ops: vec![op.clone()],
                    });
                }
                if let Some(op) = m {
                    e.push(Edit::Map {
                        root: o.root.clone(),
                        ops: vec![op.clone()],
                    });
                }
                // the step's working state (holders untouched: within a batch every write
                // is its own)
                let holders = cur.holders.clone();
                cur.apply("", &e, &HashMap::new());
                cur.holders = holders;
            }
            regions.push((o.clone(), r));
        }
        let header = is_table
            && self.mutant != GridMutant::HeaderSplit
            && rec.iter().any(|o| o.root == "meta" && o.key == "header");
        let held = header && regions.iter().any(|(_, r)| r.is_err());
        for (o, r) in regions {
            match r {
                Ok(None) => {}
                Err(why) => {
                    plan.conflicts
                        .push((o.root.clone(), o.key.clone(), why.to_owned()));
                    plan.skipped_ops.push(o);
                }
                Ok(Some(_)) if held => {}
                Ok(Some((l, m))) => {
                    plan.applied += 1;
                    // a key deleted back puts no write back (the kit records none)
                    let deletes_back =
                        matches!(o.kind, Kind::MapSet | Kind::MapDelete) && o.before.is_none();
                    if let (Some(p), false) = (&o.prev, deletes_back) {
                        plan.restores.insert(
                            (o.root.clone(), o.key.clone(), o.kind.facet()),
                            p.resolved(),
                        );
                    }
                    let at = match roots.iter().position(|(x, ..)| *x == o.root) {
                        Some(at) => at,
                        None => {
                            roots.push((o.root.clone(), Vec::new(), Vec::new()));
                            roots.len() - 1
                        }
                    };
                    roots[at].1.extend(l);
                    roots[at].2.extend(m);
                }
            }
        }
        if revert_whole {
            // S13: each row back to what it held just before the change, whole
            let empty = BTreeMap::new();
            let before = self.cells_before.get(&own).unwrap_or(&empty);
            let now = self.maps.get("cells").unwrap_or(&empty);
            let of_row = |m: &BTreeMap<String, Value>, k: &str| -> BTreeMap<String, Value> {
                m.iter()
                    .filter(|(c, _)| {
                        k2_mpc_base::table::parse_cell_key(c).is_some_and(|(r, _)| r.as_str() == k)
                    })
                    .map(|(c, v)| (c.clone(), v.clone()))
                    .collect()
            };
            for k in &rewritten {
                let key = Key::new(k.as_str()).expect("a key");
                // S11 before S13 (v1.14): a row the doc has dropped can't come back: a
                // `dropped` conflict (skipped under `--partial`), never rewritten whole
                if !self.lists.get("rows").is_some_and(|l| l.knows(&key)) {
                    plan.conflicts
                        .push(("rows".to_owned(), k.clone(), "dropped".to_owned()));
                    continue;
                }
                let mut lops = Vec::new();
                let dead = self
                    .lists
                    .get("rows")
                    .is_some_and(|l| l.dead.contains(&key));
                if dead {
                    lops.push(ListOp::Restore { key });
                }
                let (was, is) = (of_row(before, k), of_row(now, k));
                let mut mops = Vec::new();
                for c in was.keys().chain(is.keys()).collect::<BTreeSet<_>>() {
                    match (was.get(c), is.get(c)) {
                        (Some(a), b) if Some(a) != b => mops.push(MapOp::Set {
                            key: c.clone(),
                            value: a.clone(),
                        }),
                        (None, Some(_)) => mops.push(MapOp::Delete { key: c.clone() }),
                        _ => {}
                    }
                }
                if !lops.is_empty() || !mops.is_empty() {
                    plan.applied += 1;
                }
                for (root, l, m) in [("rows", lops, Vec::new()), ("cells", Vec::new(), mops)] {
                    let at = match roots.iter().position(|(x, ..)| x == root) {
                        Some(at) => at,
                        None => {
                            roots.push((root.to_owned(), Vec::new(), Vec::new()));
                            roots.len() - 1
                        }
                    };
                    roots[at].1.extend(l);
                    roots[at].2.extend(m);
                }
            }
        }
        for (root, l, m) in roots {
            if !l.is_empty() {
                plan.edits.push(Edit::List {
                    root: root.clone(),
                    ops: l,
                });
            }
            if !m.is_empty() {
                plan.edits.push(Edit::Map { root, ops: m });
            }
        }
        plan
    }

    /// Reverse `changes` (newest first) by `pid` as one request (U13, U14): returns the
    /// outcome and the changes made (`None`: a step with nothing to write).
    fn reverse(
        &mut self,
        pid: &str,
        changes: &[Seq],
        partial: bool,
        is_table: bool,
        revert: bool,
    ) -> (Outcome, Vec<Option<Seq>>) {
        let mut scratch = self.clone();
        let mut plans = Vec::new();
        let mut blocked = None;
        for (k, own) in changes.iter().enumerate() {
            let own_pid = self.authors.get(own).cloned().unwrap_or_default();
            // (S13 is judged on the doc as it stands before the request, as the kit's store)
            let rw = if is_table {
                self.rewritten(*own)
            } else {
                (Vec::new(), false)
            };
            if !rw.0.is_empty() || rw.1 {
                if revert {
                    self.s13_whole += 1;
                } else {
                    self.s13_held += 1;
                }
            }
            let mut p = scratch.plan(*own, &own_pid, is_table, revert, rw);
            p.conflicts.sort();
            if !p.conflicts.is_empty() && !partial {
                return (
                    Outcome::Conflicts {
                        step: k + 1,
                        conflicts: p.conflicts,
                    },
                    Vec::new(),
                );
            }
            if partial && p.applied == 0 && !p.conflicts.is_empty() {
                blocked = Some(p.conflicts);
                break;
            }
            scratch.apply(pid, &p.edits, &p.restores);
            plans.push(p);
        }
        if plans.is_empty() {
            return (
                Outcome::Blocked {
                    conflicts: blocked.unwrap_or_default(),
                },
                Vec::new(),
            );
        }
        let mut skipped = Vec::new();
        let mut made = Vec::new();
        for p in plans {
            skipped.extend(p.conflicts.iter().cloned());
            let s = self.apply(pid, &p.edits, &p.restores);
            if let Some(s) = s {
                // S13: a marked row this step changed is marked at its own version
                self.reversals.insert(s);
                for k in self.rows_of(s) {
                    if self.marks.contains_key(&k) {
                        self.marks.insert(k, s);
                    }
                }
                let cols = self
                    .records
                    .get(&s)
                    .is_some_and(|r| r.iter().any(|o| o.root == "cols"));
                if cols && self.remap.is_some() {
                    self.remap = Some(s);
                }
            }
            if let (Some(s), GridMutant::RedoRetriesSkipped) = (s, self.mutant) {
                // the undo's record also carries what it skipped (inverted back), so a redo
                // of it retries them
                let extra: Vec<RecOp> = p
                    .skipped_ops
                    .iter()
                    .map(|o| RecOp {
                        before: o.after.clone(),
                        after: o.before.clone(),
                        ..o.clone()
                    })
                    .collect();
                self.records.get_mut(&s).expect("just made").extend(extra);
            }
            made.push(s);
        }
        skipped.sort();
        (
            Outcome::Applied {
                steps: made.len(),
                skipped,
                blocked: blocked.is_some(),
            },
            made,
        )
    }

    /// S13: change `seq` left `rows` on their keys by a non-exact pairing of a text.
    pub fn mark_text(&mut self, seq: Seq, rows: &[Key], remap: bool) {
        for k in rows {
            self.marks.insert(k.to_string(), seq);
        }
        if remap {
            self.remap = Some(seq);
        }
    }

    /// The rows of change `own`'s record, by key.
    fn rows_of(&self, own: Seq) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for o in self.records.get(&own).into_iter().flatten() {
            let k = match o.root.as_str() {
                "rows" => Some(o.key.clone()),
                "cells" => k2_mpc_base::table::parse_cell_key(&o.key).map(|(r, _)| r.to_string()),
                _ => None,
            };
            if let Some(k) = k {
                if !out.contains(&k) {
                    out.push(k);
                }
            }
        }
        out
    }

    /// S13: the rows of `own` a merge by cell can't undo: marked at `own` or later, with a cell
    /// value or the row's existence held by a later change, or a later text's mark. And whether
    /// `own` remapped the columns with a cell written since.
    fn rewritten(&self, own: Seq) -> (Vec<String>, bool) {
        let later = |h: &Holder| h.resolved() > own;
        let remap = self.remap == Some(own)
            && self
                .holders
                .iter()
                .any(|((root, _, f), h)| root == "cells" && *f == Facet::Value && later(h));
        if remap {
            return (Vec::new(), true);
        }
        let mut rows = Vec::new();
        for k in self.rows_of(own) {
            let Some(mark) = self.marks.get(&k).copied() else {
                continue;
            };
            if mark < own {
                continue;
            }
            let later_text = mark > own && !self.reversals.contains(&mark);
            let later_cell = self.holders.iter().any(|((root, key, f), h)| {
                root == "cells"
                    && *f == Facet::Value
                    && later(h)
                    && k2_mpc_base::table::parse_cell_key(key).is_some_and(|(r, _)| r.as_str() == k)
            });
            let later_existence = self
                .holders
                .get(&("rows".to_owned(), k.clone(), Facet::Existence))
                .is_some_and(later);
            if later_text || later_cell || later_existence {
                rows.push(k);
            }
        }
        (rows, false)
    }

    /// `undo` / `redo` (U13–U17 by element, S9).
    pub fn undo(
        &mut self,
        pid: &str,
        steps: usize,
        partial: bool,
        drop: bool,
        redo: bool,
        is_table: bool,
    ) -> Outcome {
        let s = self.stacks.entry(pid.to_owned()).or_default();
        let stack = if redo { &s.1 } else { &s.0 };
        if stack.is_empty() {
            let lost = !redo
                && self.loaded_seq > 0
                && self
                    .authors
                    .iter()
                    .any(|(q, a)| *q <= self.loaded_seq && a == pid);
            return if lost {
                Outcome::HistoryLost
            } else {
                Outcome::Nothing
            };
        }
        let n = if drop { 1 } else { steps.max(1) }.min(stack.len());
        let picked: Vec<Entry> = stack.iter().rev().take(n).cloned().collect();
        if drop {
            let s = self.stacks.get_mut(pid).expect("a stack");
            if redo {
                s.1.pop()
            } else {
                s.0.pop()
            };
            return Outcome::Dropped;
        }
        let seqs: Vec<Seq> = picked.iter().map(|e| e.seq).collect();
        let (out, made) = self.reverse(pid, &seqs, partial, is_table, false);
        if let Outcome::Applied { .. } = &out {
            let depth = self.depth;
            let s = self.stacks.get_mut(pid).expect("a stack");
            for (m, e) in made.iter().zip(&picked) {
                if redo {
                    s.1.pop()
                } else {
                    s.0.pop()
                };
                if let Some(seq) = m {
                    let to = if redo { &mut s.0 } else { &mut s.1 };
                    to.push(Entry {
                        seq: *seq,
                        obo: e.obo.clone(),
                    });
                    if to.len() > depth {
                        to.remove(0);
                    }
                }
            }
        }
        out
    }

    /// `undo --for person` / `redo --for person`: only the entries made on that person's behalf.
    pub fn undo_for(
        &mut self,
        pid: &str,
        person: &str,
        steps: usize,
        partial: bool,
        redo: bool,
        is_table: bool,
    ) -> Outcome {
        let s = self.stacks.entry(pid.to_owned()).or_default();
        let stack = if redo { &s.1 } else { &s.0 };
        let picked: Vec<usize> = (0..stack.len())
            .rev()
            .filter(|&k| stack[k].obo.as_deref() == Some(person))
            .take(steps.max(1))
            .collect();
        if picked.is_empty() {
            let lost = !redo
                && stack.is_empty()
                && self.loaded_seq > 0
                && self
                    .authors
                    .iter()
                    .any(|(q, a)| *q <= self.loaded_seq && a == pid);
            return if lost {
                Outcome::HistoryLost
            } else {
                Outcome::Nothing
            };
        }
        let entries: Vec<Entry> = picked.iter().map(|&k| stack[k].clone()).collect();
        let seqs: Vec<Seq> = entries.iter().map(|e| e.seq).collect();
        let (out, made) = self.reverse(pid, &seqs, partial, is_table, false);
        if let Outcome::Applied { .. } = &out {
            let depth = self.depth;
            let s = self.stacks.get_mut(pid).expect("a stack");
            for ((k, e), m) in picked.iter().zip(&entries).zip(&made) {
                if redo {
                    s.1.remove(*k);
                } else {
                    s.0.remove(*k);
                }
                if let Some(seq) = m {
                    let to = if redo { &mut s.0 } else { &mut s.1 };
                    to.push(Entry {
                        seq: *seq,
                        obo: e.obo.clone(),
                    });
                    if to.len() > depth {
                        to.remove(0);
                    }
                }
            }
        }
        out
    }

    /// `revert <change>` (S10): reverse its record; the revert goes on the caller's stack.
    pub fn revert(&mut self, pid: &str, change: Seq, partial: bool, is_table: bool) -> Outcome {
        let (out, made) = self.reverse(pid, &[change], partial, is_table, true);
        if let Some(Some(seq)) = made.first() {
            self.push_edit(pid, *seq, None);
        }
        out
    }
}
