//! Carried positions for revert records: an **offset tree** (undo spec U22, v1.4 m9).
//!
//! Every revert record keeps its regions' bounds in live coordinates, carried through every
//! later change exactly as [`position::transform`] carries one position (a region's start
//! with [`Bias::Before`], its end with [`Bias::After`]), plus a **taint** flag (a later
//! deletion strictly across the position, U9). Doing that per record would cost O(R) per
//! keystroke for R records. Here every position of one bias lives in one tree, and a change
//! costs **O(log R) expected per hunk**, whatever R is:
//!
//! - **Structure.** A treap (randomised balanced binary tree; priorities from a seeded
//!   xorshift, so runs are deterministic) over the positions **in order**. Each node stores
//!   the **gap** to its in-order predecessor, and each subtree the sum of its gaps, so a
//!   position's value is the sum of the gaps up to it. One bias per tree: a hunk maps
//!   positions of one bias monotonically, so the order never changes and equal positions
//!   stay equal for good.
//! - **A hunk `(s, e, n)`** splits the tree by value in O(log R): positions in `(s, e)`
//!   (strictly inside a replaced range) collapse to `s` and are tainted, through one **lazy
//!   tag** on that subtree ("all gaps zero, all tainted"), resolved when a node is next
//!   visited; positions past the hunk shift by adding the size change to **one** gap (the
//!   first node after it). A pure insertion at `s` moves the After positions at `s` and
//!   leaves the Before ones. Hunks apply back to front, so the earlier ones keep their
//!   coordinates.
//! - **Reading** a node (at revert time) walks its parent pointers to the root, resolves the
//!   tags on the way down and sums the gaps: O(log R). Taint is therefore **lazy**: a
//!   deletion marks one subtree, and a record learns it is tainted when it is read.
//! - **Removing** records (the revert window moving) marks nodes dead; the index is rebuilt
//!   from live values when they pile up (amortised O(1) per removal).
//!
//! Bounds (expected, n = live nodes): insert, apply one hunk, read one node: O(log n);
//! memory: one 40-byte node per position (two per region). The per-apply work is counted
//! ([`OffsetTree::work`]) so a test can hold the bound without timing.

use std::collections::BTreeMap;

use super::position::Bias;
use crate::store::{DocId, DocStore, RootShape, Seq, StoreError};
use crate::text::diff::Hunk;

/// A node in an [`OffsetTree`].
pub(crate) type Handle = u32;
const NIL: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct Node {
    gap: u64,
    sum: u64,
    prio: u32,
    l: u32,
    r: u32,
    p: u32,
    /// Pending for the children: zero every gap below and taint every node below.
    collapse: bool,
    tainted: bool,
}

/// One bias class of carried positions (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct OffsetTree {
    bias: Bias,
    nodes: Vec<Node>,
    root: u32,
    rng: u64,
    /// Nodes visited, for the complexity test.
    work: u64,
}

impl OffsetTree {
    pub fn new(bias: Bias) -> Self {
        Self {
            bias,
            nodes: Vec::new(),
            root: NIL,
            rng: 0x9E37_79B9_7F4A_7C15,
            work: 0,
        }
    }

    /// Nodes visited so far (a deterministic cost measure).
    #[cfg(test)]
    pub fn work(&self) -> u64 {
        self.work
    }

    fn prio(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn sum(&self, t: u32) -> u64 {
        if t == NIL {
            0
        } else {
            self.nodes[t as usize].sum
        }
    }

    fn collapse(&mut self, t: u32) {
        if t != NIL {
            let n = &mut self.nodes[t as usize];
            n.gap = 0;
            n.sum = 0;
            n.tainted = true;
            n.collapse = true;
        }
    }

    fn push(&mut self, t: u32) {
        self.work += 1;
        let n = &mut self.nodes[t as usize];
        if n.collapse {
            n.collapse = false;
            let (l, r) = (n.l, n.r);
            self.collapse(l);
            self.collapse(r);
        }
    }

    fn pull(&mut self, t: u32) {
        let (l, r) = (self.nodes[t as usize].l, self.nodes[t as usize].r);
        let s = self.sum(l) + self.nodes[t as usize].gap + self.sum(r);
        self.nodes[t as usize].sum = s;
    }

    fn set_l(&mut self, t: u32, c: u32) {
        self.nodes[t as usize].l = c;
        if c != NIL {
            self.nodes[c as usize].p = t;
        }
    }

    fn set_r(&mut self, t: u32, c: u32) {
        self.nodes[t as usize].r = c;
        if c != NIL {
            self.nodes[c as usize].p = t;
        }
    }

    /// Split `t` (whose values start at `base`) into values `< v` (or `<= v` with `incl`)
    /// and the rest. Gaps don't change: the right part's first gap stays relative to the
    /// left part's last value.
    fn split(&mut self, t: u32, base: u64, v: u64, incl: bool) -> (u32, u32) {
        if t == NIL {
            return (NIL, NIL);
        }
        self.push(t);
        let l = self.nodes[t as usize].l;
        let val = base + self.sum(l) + self.nodes[t as usize].gap;
        let left = if incl { val <= v } else { val < v };
        if left {
            let r = self.nodes[t as usize].r;
            let (a, b) = self.split(r, val, v, incl);
            self.set_r(t, a);
            self.pull(t);
            (t, b)
        } else {
            let (a, b) = self.split(l, base, v, incl);
            self.set_l(t, b);
            self.pull(t);
            (a, t)
        }
    }

    fn merge(&mut self, a: u32, b: u32) -> u32 {
        if a == NIL {
            return b;
        }
        if b == NIL {
            return a;
        }
        if self.nodes[a as usize].prio >= self.nodes[b as usize].prio {
            self.push(a);
            let r = self.nodes[a as usize].r;
            let m = self.merge(r, b);
            self.set_r(a, m);
            self.pull(a);
            a
        } else {
            self.push(b);
            let l = self.nodes[b as usize].l;
            let m = self.merge(a, l);
            self.set_l(b, m);
            self.pull(b);
            b
        }
    }

    /// Change the first gap of `t` by `delta`.
    fn add_first(&mut self, t: u32, delta: i64) {
        if t == NIL || delta == 0 {
            return;
        }
        self.push(t);
        let l = self.nodes[t as usize].l;
        if l != NIL {
            self.add_first(l, delta);
        } else {
            let g = &mut self.nodes[t as usize].gap;
            *g = (*g as i64 + delta) as u64;
        }
        self.pull(t);
    }

    fn set_root(&mut self, t: u32) {
        self.root = t;
        if t != NIL {
            self.nodes[t as usize].p = NIL;
        }
    }

    /// Add a position with value `v`; returns its handle.
    pub fn insert(&mut self, v: usize) -> Handle {
        let v = v as u64;
        let id = self.nodes.len() as u32;
        let prio = self.prio();
        let root = self.root;
        let (a, b) = self.split(root, 0, v, true);
        let gap = v - self.sum(a);
        self.nodes.push(Node {
            gap,
            sum: gap,
            prio,
            l: NIL,
            r: NIL,
            p: NIL,
            collapse: false,
            tainted: false,
        });
        // b's first value is unchanged; its gap is now relative to v
        self.add_first(b, -(gap as i64));
        let m = self.merge(a, id);
        let m = self.merge(m, b);
        self.set_root(m);
        id
    }

    /// Carry every position through `hunks` (**canonical**: sorted, non-overlapping, never
    /// touching, in old coordinates, as every change the kit applies is): exactly
    /// [`super::position::transform`] with this tree's bias, plus taint for the positions
    /// strictly inside a replaced range. (Touching hunks must be coalesced first: applying
    /// them back to front would lose whether a position was strictly after an insertion.)
    pub fn apply(&mut self, hunks: &[Hunk]) {
        debug_assert!(hunks.windows(2).all(|w| w[0].end < w[1].start));
        for h in hunks.iter().rev() {
            self.apply_one(h.start as u64, h.end as u64, h.insert.len() as u64);
        }
    }

    /// [`OffsetTree::apply`] for hunks given as `(start, end, inserted length)`.
    pub fn apply_shape(&mut self, hunks: &[(u64, u64, u64)]) {
        debug_assert!(hunks.windows(2).all(|w| w[0].1 < w[1].0));
        for &(s, e, n) in hunks.iter().rev() {
            self.apply_one(s, e, n);
        }
    }

    fn apply_one(&mut self, s: u64, e: u64, n: u64) {
        let root = self.root;
        if root == NIL {
            return;
        }
        if s == e {
            // a pure insertion: Before positions at s stay, After ones move
            let incl = self.bias == Bias::Before;
            let (a, b) = self.split(root, 0, s, incl);
            self.add_first(b, n as i64);
            let m = self.merge(a, b);
            self.set_root(m);
            return;
        }
        let d = n as i64 - (e - s) as i64;
        let (a, rest) = self.split(root, 0, s, true);
        let sum_a = self.sum(a);
        let (m, b) = self.split(rest, sum_a, e, false);
        let sum_m = self.sum(m);
        let new_prev = if m != NIL {
            // strictly inside: collapse to s, tainted
            self.collapse(m);
            self.add_first(m, s as i64 - sum_a as i64);
            s
        } else {
            sum_a
        };
        // b's first value moves by d; its predecessor is now new_prev
        self.add_first(b, sum_a as i64 + sum_m as i64 + d - new_prev as i64);
        let x = self.merge(a, m);
        let x = self.merge(x, b);
        self.set_root(x);
    }

    /// The current value of `h` and whether it was tainted. O(log n).
    pub fn get(&mut self, h: Handle) -> (usize, bool) {
        let mut path = Vec::new();
        let mut x = h;
        while x != NIL {
            path.push(x);
            x = self.nodes[x as usize].p;
        }
        for &a in path.iter().rev() {
            self.push(a);
        }
        let n = &self.nodes[h as usize];
        let mut v = self.sum(n.l) + n.gap;
        let tainted = n.tainted;
        let mut x = h;
        loop {
            let p = self.nodes[x as usize].p;
            if p == NIL {
                break;
            }
            if self.nodes[p as usize].r == x {
                v += self.sum(self.nodes[p as usize].l) + self.nodes[p as usize].gap;
            }
            x = p;
        }
        (v as usize, tainted)
    }

    /// Every node's `(handle, value, tainted)`, in order. O(n).
    #[cfg(test)]
    pub fn all(&mut self) -> Vec<(Handle, usize, bool)> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = Vec::new();
        let mut t = self.root;
        let mut acc = 0u64;
        loop {
            while t != NIL {
                self.push(t);
                stack.push(t);
                t = self.nodes[t as usize].l;
            }
            let Some(x) = stack.pop() else { break };
            let n = &self.nodes[x as usize];
            acc += n.gap;
            out.push((x, acc as usize, n.tainted));
            t = n.r;
        }
        out
    }

    /// Set a node's taint (rebuilding the index from persisted carried bounds).
    pub fn set_tainted(&mut self, h: Handle) {
        self.get(h); // resolve the tags above it first
        self.nodes[h as usize].tainted = true;
    }
}

/// The regions of a change as positions after it: `(start, end)` per canonical hunk.
pub(crate) fn regions_of_shape(hunks: &[(u64, u64, u64)]) -> Vec<(usize, usize)> {
    let mut delta: i64 = 0;
    hunks
        .iter()
        .map(|&(s, e, n)| {
            let start = (s as i64 + delta) as usize;
            delta += n as i64 - (e - s) as i64;
            (start, start + n as usize)
        })
        .collect()
}

/// One record's regions in the index: `(root, start node, end node)`.
type Nodes = Vec<(usize, Handle, Handle)>;

/// The carried bounds of every revert record of one doc: per root, an [`OffsetTree`] of
/// region starts ([`Bias::Before`]) and one of region ends ([`Bias::After`]); per record, its
/// nodes. Built from the store on first use ([`RevertIndex::build`]) and then kept up to
/// date by every change the session applies (O(log R) per hunk).
#[derive(Debug)]
pub(crate) struct RevertIndex {
    roots: Vec<(String, OffsetTree, OffsetTree)>,
    records: BTreeMap<Seq, Nodes>,
    /// Records of changes applied but not yet logged (their positions are known before
    /// their log position is).
    staged: Vec<Nodes>,
    dead: usize,
}

impl RevertIndex {
    fn new() -> Self {
        Self {
            roots: Vec::new(),
            records: BTreeMap::new(),
            staged: Vec::new(),
            dead: 0,
        }
    }

    fn root(&mut self, root: &str) -> usize {
        match self.roots.iter().position(|(r, ..)| r == root) {
            Some(i) => i,
            None => {
                self.roots.push((
                    root.to_owned(),
                    OffsetTree::new(Bias::Before),
                    OffsetTree::new(Bias::After),
                ));
                self.roots.len() - 1
            }
        }
    }

    /// Records held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Nodes visited so far, over every tree (the complexity measure).
    #[cfg(test)]
    pub fn work(&self) -> u64 {
        self.roots.iter().map(|(_, a, b)| a.work() + b.work()).sum()
    }

    /// Carry every record through a change to `root`.
    pub fn carry(&mut self, root: &str, hunks: &[Hunk]) {
        if let Some((_, a, b)) = self.roots.iter_mut().find(|(r, ..)| r == root) {
            a.apply(hunks);
            b.apply(hunks);
        }
    }

    fn carry_shape(&mut self, shape: &[RootShape]) {
        for rs in shape {
            if let Some((_, a, b)) = self.roots.iter_mut().find(|(r, ..)| *r == rs.root) {
                a.apply_shape(&rs.hunks);
                b.apply_shape(&rs.hunks);
            }
        }
    }

    fn add(&mut self, regions: &[(String, usize, usize, bool)]) -> Nodes {
        let mut nodes = Vec::with_capacity(regions.len());
        for (root, s, e, tainted) in regions {
            let i = self.root(root);
            let (_, a, b) = &mut self.roots[i];
            let (hs, he) = (a.insert(*s), b.insert(*e));
            if *tainted {
                a.set_tainted(hs);
            }
            nodes.push((i, hs, he));
        }
        nodes
    }

    /// The regions of a change just applied (positions after it), until it is logged.
    pub fn stage(&mut self, regions: &[(String, usize, usize)]) {
        let r: Vec<(String, usize, usize, bool)> = regions
            .iter()
            .map(|(root, s, e)| (root.clone(), *s, *e, false))
            .collect();
        let nodes = self.add(&r);
        self.staged.push(nodes);
    }

    /// The staged changes were logged at `seqs` (in order).
    pub fn bind(&mut self, seqs: &[Seq]) {
        debug_assert_eq!(seqs.len(), self.staged.len());
        for (seq, nodes) in seqs.iter().zip(std::mem::take(&mut self.staged)) {
            self.records.insert(*seq, nodes);
        }
    }

    /// Record `seq`'s regions as carried now: `(root, start, end, tainted)`.
    pub fn bounds(&mut self, seq: Seq) -> Option<Vec<(String, usize, usize, bool)>> {
        let nodes = self.records.get(&seq)?.clone();
        Some(
            nodes
                .into_iter()
                .map(|(i, hs, he)| {
                    let (root, a, b) = &mut self.roots[i];
                    let (s, ts) = a.get(hs);
                    let (e, te) = b.get(he);
                    (root.clone(), s, e.max(s), ts || te)
                })
                .collect(),
        )
    }

    /// Forget records (the window moved past them).
    pub fn remove(&mut self, seqs: &[Seq]) {
        for s in seqs {
            if let Some(n) = self.records.remove(s) {
                self.dead += n.len();
            }
        }
        let live: usize = self.records.values().map(Vec::len).sum();
        if self.dead > live.max(64) {
            self.rebuild();
        }
    }

    /// Rebuild the trees from the live records' current bounds (drops dead nodes).
    fn rebuild(&mut self) {
        let seqs: Vec<Seq> = self.records.keys().copied().collect();
        let mut now = Vec::with_capacity(seqs.len());
        for s in seqs {
            now.push((s, self.bounds(s).expect("live record")));
        }
        let mut fresh = Self::new();
        for (s, regions) in now {
            let nodes = fresh.add(&regions);
            fresh.records.insert(s, nodes);
        }
        *self = fresh;
    }

    /// Build the index of doc `id` from the store: records in the window (and pending
    /// ones), each carried from its own change (or from its persisted carry) through the
    /// shapes of every later change. O(L log R) for L changes replayed.
    pub fn build(store: &DocStore, id: DocId) -> Result<Self, StoreError> {
        let index = store.revert_index(id)?;
        let carries = store.revert_carries(id)?;
        let mut ix = Self::new();
        let Some(first) = index.first().map(|x| x.0) else {
            return Ok(ix);
        };
        let fresh: std::collections::HashSet<Seq> = index
            .iter()
            .map(|x| x.0)
            .filter(|s| !carries.contains_key(s))
            .collect();
        let after = carries
            .values()
            .map(|c| c.carried_to)
            .chain(fresh.iter().map(|s| s - 1))
            .min()
            .unwrap_or(first - 1);
        let mut pending: Vec<(Seq, &crate::store::Carry)> =
            carries.iter().map(|(s, c)| (*s, c)).collect();
        pending.sort_by_key(|(s, c)| (c.carried_to, *s));
        let mut next = 0;
        let put_carried = |ix: &mut Self, upto: Seq, next: &mut usize| {
            while let Some((s, c)) = pending.get(*next) {
                if c.carried_to > upto {
                    break;
                }
                let r: Vec<(String, usize, usize, bool)> = c
                    .regions
                    .iter()
                    .map(|(root, a, b, t)| (root.clone(), *a as usize, *b as usize, *t))
                    .collect();
                let nodes = ix.add(&r);
                ix.records.insert(*s, nodes);
                *next += 1;
            }
        };
        for (seq, shape) in store.change_shapes(id, after)? {
            put_carried(&mut ix, seq - 1, &mut next);
            ix.carry_shape(&shape);
            if fresh.contains(&seq) {
                let mut r = Vec::new();
                for rs in &shape {
                    for (s, e) in regions_of_shape(&rs.hunks) {
                        r.push((rs.root.clone(), s, e, false));
                    }
                }
                let nodes = ix.add(&r);
                ix.records.insert(seq, nodes);
            }
        }
        put_carried(&mut ix, Seq::MAX, &mut next);
        Ok(ix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::position::transform;
    use proptest::prelude::*;

    fn hunks_strategy(len: usize) -> impl Strategy<Value = Vec<Hunk>> {
        prop::collection::vec((0..=len, 0..4usize, 0..4usize), 0..4).prop_map(move |raw| {
            // canonical: sorted, non-overlapping, never touching
            let mut pts: Vec<(usize, usize, usize)> = raw
                .into_iter()
                .map(|(s, del, ins)| (s, (s + del).min(len), ins))
                .collect();
            pts.sort();
            let mut out: Vec<Hunk> = Vec::new();
            let mut prev_end = 0usize;
            for (s, e, ins) in pts {
                if (!out.is_empty() && s <= prev_end) || (s == e && ins == 0) {
                    continue;
                }
                prev_end = e;
                out.push(Hunk {
                    start: s,
                    end: e,
                    insert: "x".repeat(ins),
                });
            }
            out
        })
    }

    fn crosses(p: usize, hs: &[Hunk]) -> bool {
        hs.iter().any(|h| h.start < p && p < h.end)
    }

    /// Positions inserted over time, carried through random hunks, read back: the tree
    /// equals `position::transform` and the taint rule, for both biases.
    fn check(bias: Bias, ops: Vec<(Vec<usize>, Vec<Hunk>)>, len0: usize) {
        let mut tree = OffsetTree::new(bias);
        let mut model: Vec<(Handle, usize, bool)> = Vec::new();
        let mut len = len0;
        for (inserts, hunks) in ops {
            for p in inserts {
                let p = p.min(len);
                model.push((tree.insert(p), p, false));
            }
            // hunks were drawn for a text of up to len0; keep those valid for len
            let hs: Vec<Hunk> = hunks.into_iter().filter(|h| h.end <= len).collect();
            for m in &mut model {
                m.2 |= crosses(m.1, &hs);
                m.1 = transform(m.1, &hs, bias);
            }
            let grow: isize = hs
                .iter()
                .map(|h| h.insert.len() as isize - (h.end - h.start) as isize)
                .sum();
            len = (len as isize + grow) as usize;
            tree.apply(&hs);
            for &(h, v, t) in &model {
                assert_eq!(tree.get(h), (v, t), "handle {h}");
            }
        }
        let all = tree.all();
        assert_eq!(all.len(), model.len());
        assert!(all.windows(2).all(|w| w[0].1 <= w[1].1));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn tree_equals_transform_and_taint(
            before in any::<bool>(),
            ops in prop::collection::vec(
                (prop::collection::vec(0..24usize, 0..4), hunks_strategy(24)),
                1..12,
            ),
        ) {
            let bias = if before { Bias::Before } else { Bias::After };
            check(bias, ops, 24);
        }
    }

    /// A doc of about `len` bytes and a deterministic stream of canonical changes: each one
    /// or two small hunks at pseudo-random places (typing, deleting, replacing).
    struct Changes {
        len: usize,
        rng: u64,
    }

    impl Changes {
        fn next_u(&mut self, n: usize) -> usize {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            (self.rng % n.max(1) as u64) as usize
        }
        fn next(&mut self) -> Vec<Hunk> {
            let mut out = Vec::new();
            let mut at = 0;
            for _ in 0..1 + self.next_u(2) {
                let s = at + self.next_u(self.len.saturating_sub(at) / 2 + 1);
                let e = (s + self.next_u(4)).min(self.len);
                let ins = if s == e {
                    1 + self.next_u(3)
                } else {
                    self.next_u(3)
                };
                out.push(Hunk {
                    start: s,
                    end: e,
                    insert: "x".repeat(ins),
                });
                at = e + 1;
                if at >= self.len {
                    break;
                }
            }
            let grow: isize = out
                .iter()
                .map(|h| h.insert.len() as isize - (h.end - h.start) as isize)
                .sum();
            self.len = (self.len as isize + grow) as usize;
            out
        }
    }

    /// Fill an index with `r` records the way a session does (carry, then stage and bind
    /// the change's own regions).
    fn filled(r: usize) -> (RevertIndex, Changes) {
        let mut ix = RevertIndex::new();
        let mut ch = Changes {
            len: 100_000,
            rng: 0x1234_5678,
        };
        for seq in 1..=r as Seq {
            let hs = ch.next();
            ix.carry("source", &hs);
            let mut delta: isize = 0;
            let regions: Vec<(String, usize, usize)> = hs
                .iter()
                .map(|h| {
                    let s = (h.start as isize + delta) as usize;
                    delta += h.insert.len() as isize - (h.end - h.start) as isize;
                    ("source".to_owned(), s, s + h.insert.len())
                })
                .collect();
            ix.stage(&regions);
            ix.bind(&[seq]);
        }
        (ix, ch)
    }

    /// Work (nodes visited) per applied change once `r` records are held, and the p95 time.
    fn per_apply(r: usize, applies: usize) -> (f64, u64, std::time::Duration) {
        let (mut ix, mut ch) = filled(r);
        assert_eq!(ix.len(), r);
        let mut times = Vec::with_capacity(applies);
        let mut max = 0;
        let w0 = ix.work();
        for _ in 0..applies {
            let hs = ch.next();
            let before = ix.work();
            let t = std::time::Instant::now();
            ix.carry("source", &hs);
            times.push(t.elapsed());
            max = max.max(ix.work() - before);
        }
        times.sort();
        let p95 = times[applies * 95 / 100];
        ((ix.work() - w0) as f64 / applies as f64, max, p95)
    }

    /// BM `revert_carry_per_apply` (K-HIST-22), CI size: R = 10^4 records. The cost is held
    /// by counted work, not time: per apply it grows like log R (a tenfold R adds little),
    /// never like R. The p95 time is printed; the 20 µs p95 budget at R = 10^5 is checked by
    /// the ignored test below, in release, on a compute node.
    #[test]
    fn revert_carry_per_apply_is_logarithmic() {
        let (small, _, _) = per_apply(1_000, 1_000);
        let (big, max, p95) = per_apply(10_000, 1_000);
        eprintln!("revert carry per apply: R=10^3 {small:.0} nodes, R=10^4 {big:.0} nodes (max {max}), p95 {p95:?} (this build)");
        assert!(big < small * 2.0, "work grew too fast: {small} -> {big}");
        let log2 = (2.0 * 10_000f64).log2();
        assert!((max as f64) < 60.0 * log2, "max work {max} at R=10^4");
    }

    /// R = 10^5, the spec's size: run on a compute node with
    /// `cargo test --release -p k2-mpc-base --lib -- --ignored revert_carry_per_apply_r1e5`.
    #[test]
    #[ignore = "BM size; release build on a compute node"]
    fn revert_carry_per_apply_r1e5() {
        let (avg, max, p95) = per_apply(100_000, 10_000);
        eprintln!("R=10^5: {avg:.0} nodes/apply (max {max}), p95 {p95:?}");
        assert!(p95 <= std::time::Duration::from_micros(20), "p95 {p95:?}");
    }

    /// The index built from the store equals the one kept up to date change by change.
    #[test]
    fn index_built_from_the_store_equals_the_live_one() {
        use crate::store::{
            ChangeInfo, ChangeKind, DocKey, DocMeta, LogEntry, NewChange, RecordHunk, RecordRoot,
            RevertRecord,
        };
        let dir = std::env::temp_dir().join(format!("k2mb-carry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = DocStore::open(dir.join("d.redb")).unwrap();
        let id = store
            .create(&DocMeta {
                key: DocKey {
                    space: "s".into(),
                    path: "p".into(),
                },
                pack: "t".into(),
                engine: "e".into(),
                engine_format: 1,
                codec: vec![],
                created_ms: 0,
                synced_hash: None,
                synced_seq: 0,
                declined: None,
            })
            .unwrap();
        let mut live = RevertIndex::new();
        let mut ch = Changes {
            len: 2_000,
            rng: 99,
        };
        for _ in 0..400 {
            let hs = ch.next();
            live.carry("source", &hs);
            let mut delta: isize = 0;
            let regions: Vec<(String, usize, usize)> = hs
                .iter()
                .map(|h| {
                    let s = (h.start as isize + delta) as usize;
                    delta += h.insert.len() as isize - (h.end - h.start) as isize;
                    ("source".to_owned(), s, s + h.insert.len())
                })
                .collect();
            live.stage(&regions);
            let rec = RevertRecord {
                author: "a".into(),
                on_behalf_of: None,
                at_ms: 0,
                roots: vec![RecordRoot {
                    root: "source".into(),
                    empty_after: false,
                    hunks: hs
                        .iter()
                        .map(|h| RecordHunk {
                            start: h.start as u64,
                            end: h.end as u64,
                            insert: h.insert.clone(),
                            deleted: "?".repeat(h.end - h.start),
                            redacted: false,
                        })
                        .collect(),
                }],
            };
            let entry = LogEntry {
                author: "a".into(),
                client: 7,
                at_ms: 0,
                via: "t".into(),
                update: vec![1],
                on_behalf_of: None,
            };
            let change = NewChange {
                info: ChangeInfo::new(ChangeKind::Edit, "a", "t", 0),
                record: rec,
            };
            let seqs = store.commit_changes(id, &[entry], &[change], &[]).unwrap();
            live.bind(&seqs);
        }
        let mut built = RevertIndex::build(&store, id).unwrap();
        assert_eq!(built.len(), 400);
        for seq in 1..=400 {
            assert_eq!(built.bounds(seq), live.bounds(seq), "record {seq}");
        }
        // after dropping old records the rebuilt trees still agree
        let drop: Vec<Seq> = (1..=300).filter(|s| s % 3 != 0).collect();
        live.remove(&drop);
        for seq in (1..=400).filter(|s| !drop.contains(s)) {
            assert!(live.bounds(seq).is_some());
            assert_eq!(
                built.bounds(seq),
                live.bounds(seq),
                "record {seq} after removal"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn insertion_at_a_position_respects_bias() {
        for (bias, want) in [(Bias::Before, 5), (Bias::After, 8)] {
            let mut t = OffsetTree::new(bias);
            let h = t.insert(5);
            t.apply(&[Hunk {
                start: 5,
                end: 5,
                insert: "abc".into(),
            }]);
            assert_eq!(t.get(h), (want, false));
        }
    }

    #[test]
    fn deletion_across_collapses_and_taints() {
        let mut t = OffsetTree::new(Bias::Before);
        let hs: Vec<Handle> = [2, 5, 7, 9].iter().map(|&p| t.insert(p)).collect();
        t.apply(&[Hunk {
            start: 3,
            end: 8,
            insert: "Z".into(),
        }]);
        let got: Vec<(usize, bool)> = hs.iter().map(|&h| t.get(h)).collect();
        assert_eq!(got, vec![(2, false), (3, true), (3, true), (5, false)]);
    }
}
