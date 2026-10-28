//! Movable trees (the reference model for trees): Kleppmann, Mulligan, Gomes & Beresford, "A
//! highly-available move operation for replicated trees", IEEE TPDS 33(7), 2021 (cited as
//! [Move]), composed with K2's additions: FugueMax sibling order per parent, the causal
//! skeleton (amendment `g1-tree-causality` r2: E-CORE-12, E-TREE-4) and the rescue projection.
//! The Quint model is `spec/quint/tree.qnt`; this model replays its ITF traces
//! (`src/bin/tree_itf_replay.rs`).
//!
//! ## The core: [Move] Fig. 4
//! [`fig4`] is Fig. 4 transcribed line by line (generic over `'t`, `'n`, `'m`, recursion kept),
//! because that is the code proven in Isabelle/HOL. Here `'t` = [`Timestamp`] `(lamport,
//! agent)`, `'n` = [`NodeId`] and `'m` = the move's own event [`Id`], which is also its element
//! in the new parent's sibling list. Create = a move of a fresh node ID; delete = a move under
//! [`NodeId::TRASH`], its own root (§3.6); restore = a move out of TRASH.
//!
//! ## Events and the causal skeleton
//! Every event (a move or a non-tree event, [`TreeOp::Other`]) has an ID `(agent, seq)`, a
//! Lamport clock (`1 +` the largest the creator has seen) and parents (the creator's frontier);
//! delivery is causal. On integration a replica computes **T(e)**, the maximal tree ops in
//! past-or-self(e): `{e}` for a move, else `max(∪ T(p))` over e's parents; a move's
//! **tparents** = `max(∪ T(p))`. `max` keeps the elements not reachable from another through
//! the skeleton. For tree ops, a happened-before b iff a is reachable from b through tparents
//! ([`Replica::happened_before`]). T and tparents are computed from the persisted T column
//! only, so they work for parents whose graph structure was compacted away.
//!
//! [`Replica::compact`] discards the event-graph structure (parents) of a critical version and
//! truncates the resident log below it (the stability point S); T and the skeleton stay.
//! [`Replica::snapshot`]/[`Replica::load`] persist and rebuild a replica from what an engine
//! keeps (no parents for compacted events).
//!
//! ## Sibling order
//! Each parent has a FugueMax list (the Fugue tree of `crate::tree`) whose elements are the
//! moves into it, tombstones included; a move carries its [`Position`] (Algorithm 1's
//! placement plus the left origin). A parent's core children are its live elements (an element
//! is live when its triple `(p, m, c)` is in the tree), in list order. A move at visible index
//! `i` takes as left origin the `i`-th live element (or start) and as right origin the next
//! element after it, tombstones included, as in the Quint model.
//!
//! ## The projection (E-TREE-4, daemon-side)
//! [`Replica::projection`] is a pure function of the tree-op set and happened-before (from the
//! skeleton); nothing is frozen. Old parents and skip status come from a replay of the op set
//! in timestamp order ([`Replica::seq_log`]), never from the resident log. The rules are K2
//! Cores' ruling of 2026-10-28 (the "core path" rule; it replaced a walk that stopped at the
//! first *visible* node, which with 4 nodes let two rescued roots be each other's visible
//! parent: `tf_mutual_rescue_cycle`).
//! - core parent `cp(n)`: Fig. 4's `get_parent`; n is deleted if `cp(n)` is TRASH; its
//!   effective move `E(n)` is the meta of its triple (for a deleted d, its delete). The
//!   **core-visible** nodes reach ROOT through core parents.
//! - **rescue test, on the core path:** a live node n that is not core-visible has its core
//!   parent chain end at TRASH through a deleted node; n passes iff `E(n)` is concurrent with
//!   the delete of every deleted node on that chain (its strict core ancestors whose core parent
//!   is TRASH; the core tree is a forest, so there is exactly one). Deletes further up the
//!   pre-delete chain (the walk below) do not count.
//! - visible set: least fixpoint from {ROOT} of "live n whose core parent is visible, or that
//!   passes the test". Rescue roots: visible n whose core parent is not; their core subtrees
//!   come with them (minus deleted nodes, which sit in TRASH).
//! - **the walk decides only where a rescue root is shown:** from `x = cp(n)`, stop at the
//!   first CORE-visible node (or ROOT); a deleted x goes to the old parent of the delete that
//!   took it out of the live tree (the latest applied move of x into TRASH, at or before its
//!   effective delete, whose old parent was not TRASH; ROOT if none), a live x to `cp(x)`; on a
//!   revisit (n itself counts as visited) fall back to ROOT. Rescued subtrees therefore always
//!   hang off the core-visible tree, and the visible tree is acyclic. A rescue root nested in
//!   another rescued subtree shows beside it, under the nearest core-visible node.
//! - a rescue root is shown under the walk's stop (recomputed, never frozen) at the tombstoned
//!   element of the chain node directly under it (`anchor`); roots with no spot (the walk fell
//!   back to ROOT) go last. Several roots at one spot (or with no spot) are ordered by the
//!   pre-order of the rebuilt pre-delete tree ([`SameSpotOrder::PreDeletePreorder`], below).
//! - conflict reports ([`ConflictReport`]): cycle-skips per (mover, node, op), rescues per
//!   (node, delete) for the deletes on the rescue root's core path. A rescue also carries
//!   `walked_past` (v0.4.1, informational, not part of its identity): the deleted nodes the
//!   walk passed on its way to the visible parent, nearest first.
//!
//! **Same-spot order, precisely.** The rebuilt pre-delete tree hangs every live node at its
//! core parent and every deleted node at the old parent (and old element) of the delete that
//! took it out of the live tree (ROOT, after its listed children, if none); siblings are
//! ordered by their element's position in the parent's FugueMax list (tombstones included),
//! then node ID. The rebuild can cycle on the oldp loop (d deleted under e, e moved under d, e
//! deleted with old parent d), so it uses the walk's visited-set rule: a node whose
//! pre-delete parent chain comes back to it is a root of the rebuild; the roots are ROOT, then
//! those nodes by node ID. With no nested deletes the rebuilt subtree at a spot is the deleted
//! chain node's core subtree. The Quint model orders the same way; [`SameSpotOrder::NodeId`]
//! orders by node ID instead (for comparison).
//!
//! ## Mutants
//! [`Variant`] mirrors the Quint model's `VARIANT` (all but the Quint-only regression
//! `rescue-records-v2`): each must fail a fixture or a property test.

use crate::check::{def4, Origins};
use crate::text::Id;
use crate::tree::{Origin, Parent, Side, SiblingRule, Tree as FugueTree, TreeError};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

// ---------------------------------------------------------------------------------------
// [Move] Fig. 4

/// [Move] Fig. 4, transcribed (Isabelle/HOL, lines 1-59). Generic over the timestamp `T`
/// (`'t::{linorder}`), the node `N` (`'n`) and the metadata `M` (`'m`). The log is a `Vec` with
/// its head (the newest entry) at index 0; the tree is the set of `(parent, meta, child)`
/// triples.
pub mod fig4 {
    use std::collections::{BTreeMap, BTreeSet};

    /// `datatype ('t, 'n, 'm) operation = Move (move_time: 't) (move_parent: 'n)
    /// (move_meta: 'm) (move_child: 'n)`
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct Move<T, N, M> {
        pub time: T,
        pub parent: N,
        pub meta: M,
        pub child: N,
    }

    /// `datatype ('t, 'n, 'm) log_op = LogMove (log_time: 't) (old_parent: ('n × 'm) option)
    /// (new_parent: 'n) (log_meta: 'm) (log_child: 'n)`
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct LogMove<T, N, M> {
        pub time: T,
        pub old_parent: Option<(N, M)>,
        pub new_parent: N,
        pub meta: M,
        pub child: N,
    }

    /// `('n × 'm × 'n) set`
    pub type Tree<N, M> = BTreeSet<(N, M, N)>;

    /// `type_synonym ('t, 'n, 'm) state = ('t, 'n, 'm) log_op list × ('n × 'm × 'n) set`
    pub type State<T, N, M> = (Vec<LogMove<T, N, M>>, Tree<N, M>);

    /// `get_parent tree child ≡ if ∃!parent. ∃!meta. (parent, meta, child) ∈ tree
    /// then Some (THE (parent, meta). (parent, meta, child) ∈ tree) else None`
    pub fn get_parent<N: Ord + Clone, M: Ord + Clone>(tree: &Tree<N, M>, child: &N) -> Option<(N, M)> {
        let mut metas: BTreeMap<&N, Vec<&M>> = BTreeMap::new();
        for (p, m, c) in tree {
            if c == child {
                metas.entry(p).or_default().push(m);
            }
        }
        let mut unique = metas.into_iter().filter(|(_, ms)| ms.len() == 1);
        match (unique.next(), unique.next()) {
            (Some((p, ms)), None) => Some((p.clone(), ms[0].clone())),
            _ => None,
        }
    }

    /// `inductive ancestor`: `(parent, meta, child) ∈ tree ⟹ ancestor tree parent child` and
    /// `(parent, meta, child) ∈ tree ∧ ancestor tree anc parent ⟹ ancestor tree anc child`
    /// (the least such relation: the transitive closure of the parent relation).
    pub fn ancestor<N: Ord + Clone, M>(tree: &Tree<N, M>, anc: &N, child: &N) -> bool {
        let mut seen: BTreeSet<&N> = BTreeSet::new();
        let mut stack: Vec<&N> = vec![child];
        while let Some(x) = stack.pop() {
            for (p, _, c) in tree {
                if c == x && seen.insert(p) {
                    stack.push(p);
                }
            }
        }
        seen.contains(anc)
    }

    /// `do_op (Move t newp m c, tree) = (LogMove t (get_parent tree c) newp m c,
    /// if ancestor tree c newp ∨ c = newp then tree else {(p', m', c') ∈ tree. c' ≠ c} ∪ {(newp, m, c)})`
    pub fn do_op<T: Clone, N: Ord + Clone, M: Ord + Clone>(op: &Move<T, N, M>, tree: &Tree<N, M>) -> (LogMove<T, N, M>, Tree<N, M>) {
        let Move { time: t, parent: newp, meta: m, child: c } = op;
        let lm = LogMove { time: t.clone(), old_parent: get_parent(tree, c), new_parent: newp.clone(), meta: m.clone(), child: c.clone() };
        let tree2 = if ancestor(tree, c, newp) || c == newp {
            tree.clone()
        } else {
            let mut t2: Tree<N, M> = tree.iter().filter(|(_, _, c2)| c2 != c).cloned().collect();
            t2.insert((newp.clone(), m.clone(), c.clone()));
            t2
        };
        (lm, tree2)
    }

    /// `undo_op (LogMove t None newp m c, tree) = {(p', m', c') ∈ tree. c' ≠ c}`;
    /// `undo_op (LogMove t (Some (oldp, oldm)) newp m c, tree) = {(p', m', c') ∈ tree. c' ≠ c} ∪ {(oldp, oldm, c)}`
    pub fn undo_op<T, N: Ord + Clone, M: Ord + Clone>(lm: &LogMove<T, N, M>, tree: &Tree<N, M>) -> Tree<N, M> {
        let mut t2: Tree<N, M> = tree.iter().filter(|(_, _, c2)| *c2 != lm.child).cloned().collect();
        if let Some((oldp, oldm)) = &lm.old_parent {
            t2.insert((oldp.clone(), oldm.clone(), lm.child.clone()));
        }
        t2
    }

    /// `redo_op (LogMove t _ p m c) (ops, tree) = (let (op2, tree2) = do_op (Move t p m c, tree)
    /// in (op2 # ops, tree2))`
    pub fn redo_op<T: Clone, N: Ord + Clone, M: Ord + Clone>(lm: &LogMove<T, N, M>, state: State<T, N, M>) -> State<T, N, M> {
        let (ops, tree) = state;
        let (op2, tree2) = do_op(&Move { time: lm.time.clone(), parent: lm.new_parent.clone(), meta: lm.meta.clone(), child: lm.child.clone() }, &tree);
        let mut l = Vec::with_capacity(ops.len() + 1);
        l.push(op2);
        l.extend(ops);
        (l, tree2)
    }

    /// `apply_op op1 ([], tree1) = (let (op2, tree2) = do_op (op1, tree1) in ([op2], tree2))`;
    /// `apply_op op1 (logop # ops, tree1) = (if move_time op1 < log_time logop
    /// then redo_op logop (apply_op op1 (ops, undo_op (logop, tree1)))
    /// else let (op2, tree2) = do_op (op1, tree1) in (op2 # logop # ops, tree2))`
    pub fn apply_op<T: Ord + Clone, N: Ord + Clone, M: Ord + Clone>(op1: &Move<T, N, M>, state: State<T, N, M>) -> State<T, N, M> {
        let (log, tree1) = state;
        let mut it = log.into_iter();
        match it.next() {
            None => {
                let (op2, tree2) = do_op(op1, &tree1);
                (vec![op2], tree2)
            }
            Some(logop) => {
                let ops: Vec<LogMove<T, N, M>> = it.collect();
                if op1.time < logop.time {
                    let undone = undo_op(&logop, &tree1);
                    redo_op(&logop, apply_op(op1, (ops, undone)))
                } else {
                    let (op2, tree2) = do_op(op1, &tree1);
                    let mut l = Vec::with_capacity(ops.len() + 2);
                    l.push(op2);
                    l.push(logop);
                    l.extend(ops);
                    (l, tree2)
                }
            }
        }
    }

    /// `apply_ops ops ≡ foldl (λstate oper. apply_op oper state) ([], {}) ops`
    pub fn apply_ops<T: Ord + Clone, N: Ord + Clone, M: Ord + Clone>(ops: &[Move<T, N, M>]) -> State<T, N, M> {
        ops.iter().fold((Vec::new(), BTreeSet::new()), |s, op| apply_op(op, s))
    }

    /// `unique_parent tree ≡ (∀ p1 p2 m1 m2 c. (p1, m1, c) ∈ tree ∧ (p2, m2, c) ∈ tree ⟶ p1 = p2 ∧ m1 = m2)`
    pub fn unique_parent<N: Ord, M: Ord>(tree: &Tree<N, M>) -> bool {
        tree.iter().all(|(p1, m1, c1)| tree.iter().all(|(p2, m2, c2)| c1 != c2 || (p1 == p2 && m1 == m2)))
    }

    /// `acyclic tree ≡ (∄ n. ancestor tree n n)`
    pub fn acyclic<N: Ord + Clone, M>(tree: &Tree<N, M>) -> bool {
        tree.iter().all(|(p, _, c)| !ancestor(tree, p, p) && !ancestor(tree, c, c))
    }
}

// ---------------------------------------------------------------------------------------
// K2's composition: types

/// A node ID. [`NodeId::ROOT`] and [`NodeId::TRASH`] are the two roots; every other node is
/// created by a move of a fresh ID (IDs are chosen by the caller and must be globally unique).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u64);

impl NodeId {
    pub const ROOT: NodeId = NodeId(0);
    /// deleted nodes are moved under TRASH (§3.6)
    pub const TRASH: NodeId = NodeId(1);

    /// ROOT or TRASH
    pub fn is_root(self) -> bool {
        self == NodeId::ROOT || self == NodeId::TRASH
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            NodeId::ROOT => write!(f, "ROOT"),
            NodeId::TRASH => write!(f, "TRASH"),
            NodeId(n) => write!(f, "N{n}"),
        }
    }
}

const ROOT: NodeId = NodeId::ROOT;
const TRASH: NodeId = NodeId::TRASH;

/// `(lamport, agent)`, ordered lexicographically: globally unique and totally ordered (the
/// agent's events have distinct Lamport clocks).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    pub lamport: u64,
    pub agent: u64,
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({},{})", self.lamport, self.agent)
    }
}

/// Fig. 4's `operation` here: `Move t p m c` with m = the move's event ID.
pub type TreeMove = fig4::Move<Timestamp, NodeId, Id>;
/// Fig. 4's `log_op` here.
pub type LogEntry = fig4::LogMove<Timestamp, NodeId, Id>;
/// The tree: `(parent, meta, child)` triples.
pub type Edges = fig4::Tree<NodeId, Id>;
/// An event graph: each event's parents (the full graph; a compacted replica no longer has it).
pub type Graph = BTreeMap<Id, BTreeSet<Id>>;

/// Where a move puts its node among the new parent's children: its element (the move's own
/// ID) in the parent's FugueMax list, as Algorithm 1 of the Fugue paper broadcasts it (the
/// Fugue tree parent and side, and the right origin) plus the left origin (Def. 4 needs it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    pub left: Origin<Id>,
    pub right: Origin<Id>,
    pub parent: Parent<Id>,
    pub side: Side,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeOp {
    /// `Move t parent m child`, t = (lamport, agent of the event), m = the event's ID
    Move { child: NodeId, parent: NodeId, pos: Position },
    /// a non-tree event (e.g. a text edit): it only carries T
    Other,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TreeEvent {
    pub id: Id,
    pub lamport: u64,
    pub parents: BTreeSet<Id>,
    pub op: TreeOp,
}

impl TreeEvent {
    pub fn ts(&self) -> Timestamp {
        Timestamp { lamport: self.lamport, agent: self.id.agent }
    }

    /// The Fig. 4 operation of a move.
    pub fn as_move(&self) -> Option<TreeMove> {
        match self.op {
            TreeOp::Move { child, parent, .. } => Some(TreeMove { time: self.ts(), parent, meta: self.id, child }),
            TreeOp::Other => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MoveError {
    /// a parent of the event is not known yet (causal delivery)
    MissingDeps,
    /// an event whose ID is known with different content (equivocation)
    Conflict,
    /// a node that is not ROOT, TRASH or created here
    UnknownNode(NodeId),
    /// `create_as` with an ID that is already a node
    NodeExists(NodeId),
    /// a move of ROOT or TRASH, or (local) of a node under itself
    BadMove,
    /// an index beyond the parent's live children
    OutOfRange,
    /// a move whose timestamp is below the truncated part of the resident log (S): only
    /// possible if compaction was given a version that was not stable
    BelowStability,
    /// compaction named an event this replica does not have
    UnknownEvent(Id),
    Fugue(TreeError),
}

impl From<TreeError> for MoveError {
    fn from(e: TreeError) -> MoveError {
        MoveError::Fugue(e)
    }
}

/// The model or one of the Quint model's mutants (`VARIANT` in `tree.qnt`). Mutants exist to
/// prove the fixtures and property tests have teeth; real replicas use `Correct`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Variant {
    Correct,
    /// do_op on arrival, the log in arrival order
    NoUndoDoRedo,
    /// do_op never checks for cycles
    NoCycleCheck,
    /// after undo-do-redo, do_op the arriving op again on top
    LwwByArrival,
    /// the cycle rule looks at the tree as it was before the undo
    AncestorCheckStale,
    /// never rescue
    RescueDropsMovedIn,
    /// rescue every live node under a deleted one
    RescueIgnoresCausality,
    /// rescue if concurrent with ANY delete the display walk passes (the core path has one
    /// deleted node, so "any" over it would be the model; this is the walk's pre-delete chain)
    RescueAnyNotEvery,
    /// the display walk has no revisit fallback
    RescueWalkUnbounded,
    /// compaction drops the T column and skeleton entries of the compacted events
    CompactTreeStructure,
}

impl Variant {
    pub const MUTANTS: [Variant; 9] = [
        Variant::NoUndoDoRedo,
        Variant::NoCycleCheck,
        Variant::LwwByArrival,
        Variant::AncestorCheckStale,
        Variant::RescueDropsMovedIn,
        Variant::RescueIgnoresCausality,
        Variant::RescueAnyNotEvery,
        Variant::RescueWalkUnbounded,
        Variant::CompactTreeStructure,
    ];

    /// The name of the variant in `tree.qnt`.
    pub fn quint_name(self) -> &'static str {
        match self {
            Variant::Correct => "correct",
            Variant::NoUndoDoRedo => "no-undo-do-redo",
            Variant::NoCycleCheck => "no-cycle-check",
            Variant::LwwByArrival => "lww-by-arrival",
            Variant::AncestorCheckStale => "ancestor-check-stale",
            Variant::RescueDropsMovedIn => "rescue-drops-moved-in",
            Variant::RescueIgnoresCausality => "rescue-ignores-causality",
            Variant::RescueAnyNotEvery => "rescue-any-not-every",
            Variant::RescueWalkUnbounded => "rescue-walk-unbounded",
            Variant::CompactTreeStructure => "compact-tree-structure",
        }
    }

    pub fn from_quint(s: &str) -> Option<Variant> {
        std::iter::once(Variant::Correct).chain(Variant::MUTANTS).find(|v| v.quint_name() == s)
    }
}

/// How several rescued roots at one spot are ordered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SameSpotOrder {
    /// K2 Cores' rule: the pre-order of the rebuilt pre-delete tree (module docs); the Quint
    /// model orders the same way
    PreDeletePreorder,
    /// by node ID (the Quint model's earlier simplification; kept for comparison)
    NodeId,
}

/// How a projection answers happened-before between tree ops.
#[derive(Clone, Copy, Debug)]
pub enum Causality<'a> {
    /// the causal skeleton (what a replica has; survives compaction)
    Skeleton,
    /// a full event graph (each event's parents), e.g. a test's ghost record of every event
    Graph(&'a Graph),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Visible,
    /// the node's core parent is TRASH
    Deleted,
    /// live, but under a deleted node and not rescued
    Hidden,
    /// a rescue root whose display walk did not end (only a mutant)
    Stuck,
}

/// A rescue root's display information.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rescue {
    /// the tombstoned element (in the visible parent's list) of the deleted chain node directly
    /// under the visible parent (a core-visible node or ROOT); `None` when the walk fell back to
    /// ROOT (shown last)
    pub anchor: Option<Id>,
    /// the deletes of the deleted nodes on its core path (one conflict report each)
    pub deletes: BTreeSet<Id>,
    /// the deleted nodes the display walk passed on its way to the visible parent, nearest
    /// first ("shown under V because B was deleted, and its former parents C, D"); display
    /// only (v0.4.1)
    pub walked_past: Vec<NodeId>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NodeView {
    pub status: Status,
    /// `Visible`: the visible parent; otherwise the core parent
    pub parent: NodeId,
    /// rescue roots only
    pub rescue: Option<Rescue>,
    /// `Hidden`: the deletes on its core path that its effective move is not concurrent with
    pub reasons: BTreeSet<Id>,
}

/// The projected (visible) tree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VisibleTree {
    /// every node of the replica (not ROOT or TRASH)
    pub nodes: BTreeMap<NodeId, NodeView>,
    /// ROOT and every visible node: its visible children in order
    pub children: BTreeMap<NodeId, Vec<NodeId>>,
}

impl VisibleTree {
    pub fn status(&self, n: NodeId) -> Option<Status> {
        self.nodes.get(&n).map(|v| v.status)
    }

    /// the visible parent of a visible node
    pub fn parent(&self, n: NodeId) -> Option<NodeId> {
        self.nodes.get(&n).filter(|v| v.status == Status::Visible).map(|v| v.parent)
    }

    /// the visible children of ROOT or a visible node (empty otherwise)
    pub fn children(&self, p: NodeId) -> &[NodeId] {
        self.children.get(&p).map_or(&[], |v| v.as_slice())
    }

    /// (node, delete) for every rescue
    pub fn rescues(&self) -> BTreeSet<(NodeId, Id)> {
        self.nodes.iter().flat_map(|(n, v)| v.rescue.iter().flat_map(move |r| r.deletes.iter().map(move |d| (*n, *d)))).collect()
    }
}

/// A conflict report (E-TREE-4): cycle-skips per (mover, node, op), rescues per (node, delete).
///
/// Identity is the key only: `CycleSkip` by (mover, node, op), `Rescue` by (node, delete).
/// `PartialEq`, `Ord` and `Hash` compare [`ConflictReport::key`] and ignore `walked_past`
/// (informational), so a `BTreeSet<ConflictReport>` holds one rescue per (node, core-path
/// delete). Compare `walked_past` explicitly where it matters.
#[derive(Clone, Debug)]
pub enum ConflictReport {
    /// the move `op` by agent `mover` of `node` is a no-op (it would have made a cycle)
    CycleSkip { mover: u64, node: NodeId, op: Id },
    /// `node` is shown although `delete` deleted an ancestor (they were concurrent).
    /// `walked_past` (v0.4.1, informational, not part of the identity): the deleted nodes the
    /// projection's walk passed on its way to the visible parent, nearest first
    /// ([`Rescue::walked_past`]).
    Rescue { node: NodeId, delete: Id, walked_past: Vec<NodeId> },
}

/// A conflict report's identity (see [`ConflictReport`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ReportKey {
    CycleSkip { mover: u64, node: NodeId, op: Id },
    Rescue { node: NodeId, delete: Id },
}

impl ConflictReport {
    /// The identity: everything but `walked_past`.
    pub fn key(&self) -> ReportKey {
        match *self {
            ConflictReport::CycleSkip { mover, node, op } => ReportKey::CycleSkip { mover, node, op },
            ConflictReport::Rescue { node, delete, .. } => ReportKey::Rescue { node, delete },
        }
    }
}

impl PartialEq for ConflictReport {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for ConflictReport {}

impl PartialOrd for ConflictReport {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ConflictReport {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl std::hash::Hash for ConflictReport {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.key().hash(h)
    }
}

/// What a replica persists (an engine's snapshot): every event's ID, Lamport clock and op;
/// parents only for events not compacted; the T column, the skeleton, the compaction set and
/// S; the frontier, the tree and the resident log. The per-parent lists are rebuilt on load.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Snapshot {
    pub variant: Variant,
    pub same_spot: SameSpotOrder,
    pub ops: BTreeMap<Id, (u64, TreeOp)>,
    pub parents: Graph,
    pub t_col: BTreeMap<Id, BTreeSet<Id>>,
    pub tparents: BTreeMap<Id, BTreeSet<Id>>,
    pub compacted: BTreeSet<Id>,
    pub truncated: Option<Timestamp>,
    pub frontier: BTreeSet<Id>,
    pub tree: Edges,
    pub log: Vec<LogEntry>,
}

// ---------------------------------------------------------------------------------------
// The replica

/// One replica. Deliberately simple: everything recomputed, linear scans.
#[derive(Clone, Debug)]
pub struct Replica {
    variant: Variant,
    same_spot: SameSpotOrder,
    /// every received event: Lamport clock and op (kept forever; no content)
    ops: BTreeMap<Id, (u64, TreeOp)>,
    /// event-graph structure: parents, for events not compacted
    parents: Graph,
    /// the T column: maximal tree ops in past-or-self, for every event
    t_col: BTreeMap<Id, BTreeSet<Id>>,
    /// the causal skeleton: each tree op's tparents
    tparents: BTreeMap<Id, BTreeSet<Id>>,
    /// events whose graph structure was discarded
    compacted: BTreeSet<Id>,
    /// S: the highest timestamp truncated from the resident log
    truncated: Option<Timestamp>,
    frontier: BTreeSet<Id>,
    /// Fig. 4 state: the resident log (descending timestamps) and the tree
    log: Vec<LogEntry>,
    tree: Edges,
    /// per parent: its FugueMax list (elements = moves into it)
    lists: BTreeMap<NodeId, FugueTree<Id>>,
}

impl Default for Replica {
    fn default() -> Replica {
        Replica::new()
    }
}

/// The seed of the Quint model: agent 0 creates every node under ROOT in node order, node n by
/// event `(0, n)` at Lamport clock n, each event's parent the previous one, appended at the end.
pub fn seed_events(nodes: &[u64]) -> Vec<TreeEvent> {
    let mut sorted: Vec<u64> = nodes.to_vec();
    sorted.sort_unstable();
    let mut scratch = Replica::new();
    let mut out = Vec::new();
    let mut prev: Option<Id> = None;
    for (k, &n) in sorted.iter().enumerate() {
        let pos = scratch.position(ROOT, k).expect("seed position");
        let e = TreeEvent { id: Id::new(0, n), lamport: n, parents: prev.into_iter().collect(), op: TreeOp::Move { child: NodeId(n), parent: ROOT, pos } };
        scratch.deliver(&e).expect("seed");
        prev = Some(e.id);
        out.push(e);
    }
    out
}

struct Walk {
    stop: NodeId,
    prev: NodeId,
    /// the deleted nodes passed, nearest first
    dels: Vec<NodeId>,
    ok: bool,
}

/// happened-before between tree ops, precomputed for one projection
struct Hb {
    /// strict ancestors (tree ops and, for a graph, other events) of each tree op
    anc: BTreeMap<Id, BTreeSet<Id>>,
}

impl Hb {
    fn conc(&self, x: Id, y: Id) -> bool {
        let below = |a: Id, b: Id| self.anc.get(&b).is_some_and(|s| s.contains(&a));
        x != y && !below(x, y) && !below(y, x)
    }
}

/// strict ancestors of `id` in a graph
fn graph_ancestors(g: &Graph, id: Id) -> BTreeSet<Id> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<Id> = g.get(&id).map(|p| p.iter().copied().collect()).unwrap_or_default();
    while let Some(x) = stack.pop() {
        if seen.insert(x) {
            if let Some(p) = g.get(&x) {
                stack.extend(p.iter().copied());
            }
        }
    }
    seen
}

impl Replica {
    pub fn new() -> Replica {
        Replica::with_variant(Variant::Correct)
    }

    pub fn with_variant(variant: Variant) -> Replica {
        Replica {
            variant,
            same_spot: SameSpotOrder::PreDeletePreorder,
            ops: BTreeMap::new(),
            parents: BTreeMap::new(),
            t_col: BTreeMap::new(),
            tparents: BTreeMap::new(),
            compacted: BTreeSet::new(),
            truncated: None,
            frontier: BTreeSet::new(),
            log: Vec::new(),
            tree: BTreeSet::new(),
            lists: BTreeMap::new(),
        }
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    pub fn same_spot_order(&self) -> SameSpotOrder {
        self.same_spot
    }

    pub fn set_same_spot_order(&mut self, o: SameSpotOrder) {
        self.same_spot = o;
    }

    // --- reading -------------------------------------------------------------------------

    pub fn has(&self, id: Id) -> bool {
        self.ops.contains_key(&id)
    }

    /// every received event ID
    pub fn received(&self) -> BTreeSet<Id> {
        self.ops.keys().copied().collect()
    }

    pub fn frontier(&self) -> &BTreeSet<Id> {
        &self.frontier
    }

    pub fn op(&self, id: Id) -> Option<&TreeOp> {
        self.ops.get(&id).map(|(_, o)| o)
    }

    pub fn ts_of(&self, id: Id) -> Option<Timestamp> {
        self.ops.get(&id).map(|(l, _)| Timestamp { lamport: *l, agent: id.agent })
    }

    /// The event as received, or `None` if unknown or if its parents were compacted away.
    pub fn event(&self, id: Id) -> Option<TreeEvent> {
        let (lamport, op) = self.ops.get(&id)?;
        let parents = self.parents.get(&id)?.clone();
        Some(TreeEvent { id, lamport: *lamport, parents, op: op.clone() })
    }

    /// The event-graph structure still held (parents of events not compacted).
    pub fn graph(&self) -> &Graph {
        &self.parents
    }

    /// Every node created here (not ROOT or TRASH).
    pub fn nodes(&self) -> BTreeSet<NodeId> {
        self.ops.values().filter_map(|(_, o)| if let TreeOp::Move { child, .. } = o { Some(*child) } else { None }).collect()
    }

    fn is_node(&self, n: NodeId) -> bool {
        n.is_root() || self.ops.values().any(|(_, o)| matches!(o, TreeOp::Move { child, .. } if *child == n))
    }

    pub fn tree(&self) -> &Edges {
        &self.tree
    }

    /// The resident log (newest first): every move above S.
    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    /// Fig. 4's `get_parent`: the core parent and the meta of its triple.
    pub fn parent_of(&self, n: NodeId) -> Option<(NodeId, Id)> {
        fig4::get_parent(&self.tree, &n)
    }

    /// p's FugueMax list: every move into p, tombstones included.
    pub fn list(&self, p: NodeId) -> Vec<Id> {
        self.lists.get(&p).map_or_else(Vec::new, |l| l.order())
    }

    fn child_of(&self, e: Id) -> Option<NodeId> {
        match self.op(e) {
            Some(TreeOp::Move { child, .. }) => Some(*child),
            _ => None,
        }
    }

    fn live_el(&self, p: NodeId, e: Id) -> bool {
        self.child_of(e).is_some_and(|c| self.tree.contains(&(p, e, c)))
    }

    /// p's live elements (those whose triple is in the tree), in list order.
    pub fn live_elements(&self, p: NodeId) -> Vec<Id> {
        self.list(p).into_iter().filter(|&e| self.live_el(p, e)).collect()
    }

    /// p's core children in sibling order (no projection).
    pub fn children(&self, p: NodeId) -> Vec<NodeId> {
        self.live_elements(p).into_iter().filter_map(|e| self.child_of(e)).collect()
    }

    /// Each element's (left, right) origins, over every list.
    pub fn origins(&self) -> Origins<Id> {
        self.ops.iter().filter_map(|(id, (_, o))| if let TreeOp::Move { pos, .. } = o { Some((*id, (pos.left, pos.right))) } else { None }).collect()
    }

    /// T(e): the maximal tree ops in past-or-self(e).
    pub fn t_of(&self, id: Id) -> Option<&BTreeSet<Id>> {
        self.t_col.get(&id)
    }

    /// A tree op's tparents: the maximal tree ops in its past.
    pub fn tparents_of(&self, id: Id) -> Option<&BTreeSet<Id>> {
        self.tparents.get(&id)
    }

    pub fn compacted(&self) -> &BTreeSet<Id> {
        &self.compacted
    }

    /// S: the highest timestamp truncated from the resident log, if any.
    pub fn stability_point(&self) -> Option<Timestamp> {
        self.truncated
    }

    /// Strict skeleton ancestors of a tree op: the closure of tparents.
    pub fn skeleton_ancestors(&self, id: Id) -> BTreeSet<Id> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<Id> = self.tparents.get(&id).map(|s| s.iter().copied().collect()).unwrap_or_default();
        while let Some(x) = stack.pop() {
            if seen.insert(x) {
                if let Some(s) = self.tparents.get(&x) {
                    stack.extend(s.iter().copied());
                }
            }
        }
        seen
    }

    /// For tree ops a and b: a happened before b, i.e. a is reachable from b through the
    /// skeleton.
    pub fn happened_before(&self, a: Id, b: Id) -> bool {
        a != b && self.skeleton_ancestors(b).contains(&a)
    }

    pub fn concurrent(&self, a: Id, b: Id) -> bool {
        a != b && !self.happened_before(a, b) && !self.happened_before(b, a)
    }

    /// max(S): the elements of S not reachable from another element through the skeleton
    fn max_tree(&self, s: &BTreeSet<Id>) -> BTreeSet<Id> {
        let anc: Vec<BTreeSet<Id>> = s.iter().map(|&y| self.skeleton_ancestors(y)).collect();
        s.iter().copied().filter(|x| !anc.iter().any(|a| a.contains(x))).collect()
    }

    /// Every move, in timestamp order.
    pub fn moves(&self) -> Vec<TreeMove> {
        let mut v: Vec<TreeMove> = self
            .ops
            .iter()
            .filter_map(|(id, (l, o))| match o {
                TreeOp::Move { child, parent, .. } => Some(TreeMove { time: Timestamp { lamport: *l, agent: id.agent }, parent: *parent, meta: *id, child: *child }),
                TreeOp::Other => None,
            })
            .collect();
        v.sort_by_key(|m| m.time);
        v
    }

    /// do_op over every move in timestamp order, from the empty tree: each LogMove (oldest
    /// first) with whether do_op ignored the move (a cycle-skip), and the final tree. A pure
    /// function of the op set; the resident log above S equals its tail.
    pub fn seq_log(&self) -> (Vec<(LogEntry, bool)>, Edges) {
        let mut tree = Edges::new();
        let mut out = Vec::new();
        for mv in self.moves() {
            let skipped = mv.child == mv.parent || fig4::ancestor(&tree, &mv.child, &mv.parent);
            let (lm, t2) = fig4::do_op(&mv, &tree);
            out.push((lm, skipped));
            tree = t2;
        }
        (out, tree)
    }

    // --- local operations ----------------------------------------------------------------

    /// The position of a move to visible index `index` among `parent`'s live children
    /// (Algorithm 1 lines 23-28 on the parent's list).
    pub fn position(&self, parent: NodeId, index: usize) -> Result<Position, MoveError> {
        let live = self.live_elements(parent);
        if index > live.len() {
            return Err(MoveError::OutOfRange);
        }
        let empty = FugueTree::new(SiblingRule::FugueMax);
        let l = self.lists.get(&parent).unwrap_or(&empty);
        let left = if index == 0 { Origin::Start } else { Origin::Elem(live[index - 1]) };
        let right = l.next_after(left);
        let (fp, side) = l.placement(left, right)?;
        Ok(Position { left, right, parent: fp, side })
    }

    fn fresh_id(&self, agent: u64) -> Id {
        let next = self.ops.range(Id::new(agent, 0)..=Id::new(agent, u64::MAX)).next_back().map_or(0, |(id, _)| id.seq + 1);
        Id::new(agent, next)
    }

    fn next_lamport(&self) -> u64 {
        self.ops.values().map(|(l, _)| *l).max().unwrap_or(0) + 1
    }

    fn local(&mut self, agent: u64, op: TreeOp) -> Result<TreeEvent, MoveError> {
        let e = TreeEvent { id: self.fresh_id(agent), lamport: self.next_lamport(), parents: self.frontier.clone(), op };
        self.deliver(&e)?;
        Ok(e)
    }

    /// Move `node` under `parent` at visible index `index` among its live children, as
    /// `agent`; returns the event, already applied here. A move under the node's own
    /// descendant is allowed (it is a cycle-skip, reported to the mover).
    pub fn move_as(&mut self, agent: u64, node: NodeId, parent: NodeId, index: usize) -> Result<TreeEvent, MoveError> {
        if node.is_root() || node == parent {
            return Err(MoveError::BadMove);
        }
        for n in [node, parent] {
            if !self.is_node(n) {
                return Err(MoveError::UnknownNode(n));
            }
        }
        let pos = self.position(parent, index)?;
        self.local(agent, TreeOp::Move { child: node, parent, pos })
    }

    /// Create `node` (a fresh ID) under `parent` at visible index `index`: a move of a fresh ID.
    pub fn create_as(&mut self, agent: u64, node: NodeId, parent: NodeId, index: usize) -> Result<TreeEvent, MoveError> {
        if node.is_root() {
            return Err(MoveError::BadMove);
        }
        if self.is_node(node) {
            return Err(MoveError::NodeExists(node));
        }
        if !self.is_node(parent) {
            return Err(MoveError::UnknownNode(parent));
        }
        let pos = self.position(parent, index)?;
        self.local(agent, TreeOp::Move { child: node, parent, pos })
    }

    /// Delete `node`: a move under TRASH (at its start).
    pub fn delete_as(&mut self, agent: u64, node: NodeId) -> Result<TreeEvent, MoveError> {
        self.move_as(agent, node, TRASH, 0)
    }

    /// A non-tree event (e.g. a text edit) as `agent`.
    pub fn other_as(&mut self, agent: u64) -> Result<TreeEvent, MoveError> {
        self.local(agent, TreeOp::Other)
    }

    // --- integration ---------------------------------------------------------------------

    /// Deliver an event (causal). Ok(true) if new, Ok(false) if already known (unchanged).
    pub fn deliver(&mut self, e: &TreeEvent) -> Result<bool, MoveError> {
        if let Some((l, o)) = self.ops.get(&e.id) {
            let same = *l == e.lamport && *o == e.op && self.parents.get(&e.id).is_none_or(|p| *p == e.parents);
            return if same { Ok(false) } else { Err(MoveError::Conflict) };
        }
        if !e.parents.iter().all(|p| self.ops.contains_key(p)) {
            return Err(MoveError::MissingDeps);
        }
        // T(e) from the T column only (no graph structure needed)
        let mut up = BTreeSet::new();
        for p in &e.parents {
            if let Some(t) = self.t_col.get(p) {
                up.extend(t.iter().copied());
            }
        }
        let tset = self.max_tree(&up);
        match &e.op {
            TreeOp::Move { child, parent, pos } => {
                if child.is_root() {
                    return Err(MoveError::BadMove);
                }
                if !self.is_node(*parent) {
                    return Err(MoveError::UnknownNode(*parent));
                }
                if self.truncated.is_some_and(|s| e.ts() < s) {
                    return Err(MoveError::BelowStability);
                }
                let mut l = self.lists.get(parent).cloned().unwrap_or_else(|| FugueTree::new(SiblingRule::FugueMax));
                l.integrate(e.id, pos.parent, pos.side, pos.right)?;
                self.lists.insert(*parent, l);
                self.tparents.insert(e.id, tset);
                self.t_col.insert(e.id, BTreeSet::from([e.id]));
                self.ops.insert(e.id, (e.lamport, e.op.clone()));
                self.apply(&e.as_move().expect("a move"));
            }
            TreeOp::Other => {
                self.t_col.insert(e.id, tset);
                self.ops.insert(e.id, (e.lamport, e.op.clone()));
            }
        }
        self.parents.insert(e.id, e.parents.clone());
        for p in &e.parents {
            self.frontier.remove(p);
        }
        self.frontier.insert(e.id);
        Ok(true)
    }

    /// Deliver a batch in any order (known events skipped). Atomic. Returns the number of new
    /// events.
    pub fn deliver_all(&mut self, events: &[TreeEvent]) -> Result<usize, MoveError> {
        let mut work = self.clone();
        let mut pending: Vec<&TreeEvent> = events.iter().collect();
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
                return Err(MoveError::MissingDeps);
            }
            pending = rest;
        }
        *self = work;
        Ok(n)
    }

    /// Fig. 4 `apply_op` (or a core mutant) on the resident state
    fn apply(&mut self, op: &TreeMove) {
        let log = std::mem::take(&mut self.log);
        let tree = std::mem::take(&mut self.tree);
        let (log, tree) = match self.variant {
            Variant::NoUndoDoRedo => {
                let (lm, t) = fig4::do_op(op, &tree);
                let mut l = vec![lm];
                l.extend(log);
                (l, t)
            }
            Variant::LwwByArrival => {
                let (l, t) = fig4::apply_op(op, (log, tree));
                let t2 = fig4::do_op(op, &t).1;
                (l, t2)
            }
            Variant::NoCycleCheck => apply_unrolled(op, log, tree, false, false),
            Variant::AncestorCheckStale => apply_unrolled(op, log, tree, true, true),
            _ => fig4::apply_op(op, (log, tree)),
        };
        self.log = log;
        self.tree = tree;
    }

    // --- compaction and snapshots ------------------------------------------------------------

    /// Compact at a critical version `cv` (a set of events below S that every later event
    /// descends from; the caller knows stability): discard their event-graph structure
    /// (parents) and truncate their entries from the resident log. The T column and the
    /// skeleton are kept (the `CompactTreeStructure` mutant drops them). Monotone.
    pub fn compact(&mut self, cv: &BTreeSet<Id>) -> Result<(), MoveError> {
        if let Some(x) = cv.iter().find(|x| !self.ops.contains_key(x)) {
            return Err(MoveError::UnknownEvent(*x));
        }
        for x in cv {
            self.parents.remove(x);
            if self.variant == Variant::CompactTreeStructure {
                self.t_col.remove(x);
                self.tparents.remove(x);
            }
        }
        let gone: Vec<Timestamp> = self.log.iter().filter(|lm| cv.contains(&lm.meta)).map(|lm| lm.time).collect();
        self.log.retain(|lm| !cv.contains(&lm.meta));
        if let Some(&s) = gone.iter().max() {
            self.truncated = Some(self.truncated.map_or(s, |t| t.max(s)));
        }
        self.compacted.extend(cv.iter().copied());
        Ok(())
    }

    /// What this replica persists (no parents for compacted events, no per-parent lists).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            variant: self.variant,
            same_spot: self.same_spot,
            ops: self.ops.clone(),
            parents: self.parents.clone(),
            t_col: self.t_col.clone(),
            tparents: self.tparents.clone(),
            compacted: self.compacted.clone(),
            truncated: self.truncated,
            frontier: self.frontier.clone(),
            tree: self.tree.clone(),
            log: self.log.clone(),
        }
    }

    /// Restart from a snapshot: the per-parent lists are rebuilt by integrating every move's
    /// element in timestamp order (a causal order).
    pub fn load(s: &Snapshot) -> Result<Replica, MoveError> {
        let mut r = Replica {
            variant: s.variant,
            same_spot: s.same_spot,
            ops: s.ops.clone(),
            parents: s.parents.clone(),
            t_col: s.t_col.clone(),
            tparents: s.tparents.clone(),
            compacted: s.compacted.clone(),
            truncated: s.truncated,
            frontier: s.frontier.clone(),
            log: s.log.clone(),
            tree: s.tree.clone(),
            lists: BTreeMap::new(),
        };
        let mut evs: Vec<(Timestamp, Id, NodeId, Position)> = r
            .ops
            .iter()
            .filter_map(|(id, (l, o))| match o {
                TreeOp::Move { parent, pos, .. } => Some((Timestamp { lamport: *l, agent: id.agent }, *id, *parent, *pos)),
                TreeOp::Other => None,
            })
            .collect();
        evs.sort_by_key(|x| x.0);
        for (_, id, parent, pos) in evs {
            r.lists.entry(parent).or_insert_with(|| FugueTree::new(SiblingRule::FugueMax)).integrate(id, pos.parent, pos.side, pos.right)?;
        }
        Ok(r)
    }

    // --- the projection --------------------------------------------------------------------

    fn hb(&self, c: Causality) -> Hb {
        let tree_ops: Vec<Id> = self.ops.iter().filter(|(_, (_, o))| matches!(o, TreeOp::Move { .. })).map(|(id, _)| *id).collect();
        let anc = tree_ops
            .iter()
            .map(|&y| {
                let a = match c {
                    Causality::Skeleton => self.skeleton_ancestors(y),
                    Causality::Graph(g) => graph_ancestors(g, y),
                };
                (y, a)
            })
            .collect();
        Hb { anc }
    }

    /// The old parent of the delete that took x out of the live tree: the latest applied move
    /// of x into TRASH at or before `t_eff` whose old parent was not TRASH.
    fn del_oldp(seq: &[(LogEntry, bool)], x: NodeId, t_eff: Timestamp) -> Option<(NodeId, Id)> {
        let mut acc = None;
        for (lm, skipped) in seq {
            if lm.child == x && !skipped && lm.time <= t_eff && lm.new_parent == TRASH && !matches!(lm.old_parent, Some((TRASH, _))) {
                acc = lm.old_parent;
            }
        }
        acc
    }

    /// x's effective delete's timestamp, if x is deleted
    fn deleted_at(&self, x: NodeId) -> Option<Timestamp> {
        match self.parent_of(x) {
            Some((TRASH, m)) => self.ts_of(m),
            _ => None,
        }
    }

    /// The display walk from n: from `x = cp(n)`, stop at the first core-visible node (in `cv`)
    /// or ROOT; a deleted x goes to its delete's old parent, a live x to `cp(x)`; on a revisit
    /// (n itself counts as visited), at TRASH or with no live old parent, fall back to ROOT. The
    /// deleted nodes passed are `dels`, nearest first (the rescue's `walked_past`; the
    /// `RescueAnyNotEvery` mutant tests them); `prev` is
    /// the chain node directly under the stop.
    fn walk(&self, n: NodeId, seq: &[(LogEntry, bool)], cv: &BTreeSet<NodeId>, unbounded: bool) -> Walk {
        let bound = 2 * (self.nodes().len() + 2) + 2;
        let mut x = self.parent_of(n).map_or(ROOT, |(p, _)| p);
        let mut prev = n;
        let mut dels = Vec::new();
        let mut seen = BTreeSet::from([n]);
        for _ in 0..bound {
            if x == ROOT || cv.contains(&x) {
                return Walk { stop: x, prev, dels, ok: true };
            }
            if x == TRASH || (!unbounded && seen.contains(&x)) {
                return Walk { stop: ROOT, prev, dels, ok: true };
            }
            prev = x;
            seen.insert(x);
            match self.deleted_at(x) {
                Some(t) => {
                    dels.push(x);
                    x = Replica::del_oldp(seq, x, t).map_or(ROOT, |(p, _)| p);
                }
                None => x = self.parent_of(x).map_or(ROOT, |(p, _)| p),
            }
        }
        Walk { stop: ROOT, prev, dels, ok: false }
    }

    /// The deleted nodes on n's core path: its strict core ancestors whose core parent is TRASH
    /// (the core tree is a forest, so a live node has at most one).
    fn core_dels(&self, n: NodeId) -> BTreeSet<NodeId> {
        let mut out = BTreeSet::new();
        let mut x = n;
        for _ in 0..=self.tree.len() {
            match self.parent_of(x) {
                Some((p, _)) if !p.is_root() => {
                    if self.deleted_at(p).is_some() {
                        out.insert(p);
                    }
                    x = p;
                }
                _ => break,
            }
        }
        out
    }

    /// The rescue test (and its mutants), on n's effective move, the deletes of the deleted
    /// nodes on its core path (`core`) and of those its display walk passes (`walked`).
    fn passes_test(&self, hb: &Hb, eff_n: Id, core: &BTreeSet<Id>, walked: &BTreeSet<Id>) -> bool {
        match self.variant {
            Variant::RescueDropsMovedIn => false,
            Variant::RescueIgnoresCausality => !core.is_empty(),
            Variant::RescueAnyNotEvery => walked.iter().any(|&d| hb.conc(eff_n, d)),
            _ => !core.is_empty() && core.iter().all(|&d| hb.conc(eff_n, d)),
        }
    }

    /// the effective move of a node: the meta of its triple
    fn eff(&self, n: NodeId) -> Option<Id> {
        self.parent_of(n).map(|(_, m)| m)
    }

    fn effs<'a>(&self, dels: impl IntoIterator<Item = &'a NodeId>) -> BTreeSet<Id> {
        dels.into_iter().filter_map(|&d| self.eff(d)).collect()
    }

    /// The pre-order of the rebuilt pre-delete tree (module docs): each node's index.
    fn pre_delete_order(&self, seq: &[(LogEntry, bool)], nodes: &BTreeSet<NodeId>) -> BTreeMap<NodeId, usize> {
        // pre-delete parent: the core parent, or a deleted node's delete's old parent (and element)
        let pd: BTreeMap<NodeId, Option<(NodeId, Id)>> = nodes
            .iter()
            .map(|&y| {
                let p = match self.deleted_at(y) {
                    Some(t) => Replica::del_oldp(seq, y, t),
                    None => self.parent_of(y),
                };
                (y, p)
            })
            .collect();
        let up = |y: NodeId| -> NodeId { pd.get(&y).copied().flatten().map_or(ROOT, |(p, _)| p) };
        // the visited-set rule: a node whose pre-delete parent chain comes back to it is a root
        let on_cycle = |y: NodeId| -> bool {
            let mut seen = BTreeSet::from([y]);
            let mut x = up(y);
            loop {
                if x == y {
                    return true;
                }
                if x.is_root() || !seen.insert(x) {
                    return false;
                }
                x = up(x);
            }
        };
        let cyc: BTreeSet<NodeId> = nodes.iter().copied().filter(|&y| on_cycle(y)).collect();
        let mut kids: BTreeMap<NodeId, Vec<(usize, NodeId)>> = BTreeMap::new();
        let mut lists: BTreeMap<NodeId, Vec<Id>> = BTreeMap::new();
        for &y in nodes.iter().filter(|y| !cyc.contains(y)) {
            let q = up(y);
            let i = match pd[&y] {
                Some((_, m)) => lists.entry(q).or_insert_with(|| self.list(q)).iter().position(|&e| e == m).unwrap_or(usize::MAX),
                None => usize::MAX,
            };
            kids.entry(q).or_default().push((i, y));
        }
        for v in kids.values_mut() {
            v.sort();
        }
        // roots: ROOT, then the nodes on a cycle by node ID
        let mut out = BTreeMap::new();
        let mut stack: Vec<NodeId> = cyc.iter().rev().copied().chain(std::iter::once(ROOT)).collect();
        while let Some(y) = stack.pop() {
            if out.contains_key(&y) {
                continue;
            }
            out.insert(y, out.len());
            if let Some(v) = kids.get(&y) {
                stack.extend(v.iter().rev().map(|&(_, k)| k));
            }
        }
        out
    }

    /// The projection, with happened-before from the skeleton.
    pub fn projection(&self) -> VisibleTree {
        self.projection_with(Causality::Skeleton)
    }

    /// The projection, with happened-before answered by `c`.
    pub fn projection_with(&self, c: Causality) -> VisibleTree {
        let (seq, _) = self.seq_log();
        let hb = self.hb(c);
        let nodes = self.nodes();
        let unbounded = self.variant == Variant::RescueWalkUnbounded;
        let cp: BTreeMap<NodeId, Option<(NodeId, Id)>> = nodes.iter().map(|&n| (n, self.parent_of(n))).collect();
        let live: Vec<NodeId> = nodes.iter().copied().filter(|n| matches!(cp[n], Some((p, _)) if p != TRASH)).collect();
        let core = self.core_visible();
        // the rescue test, on the core path; the walk (display only) stops at core-visible nodes
        let cdel: BTreeMap<NodeId, BTreeSet<Id>> = live.iter().map(|&n| (n, self.effs(&self.core_dels(n)))).collect();
        let walks: BTreeMap<NodeId, Walk> = live.iter().copied().filter(|n| !core.contains(n)).map(|n| (n, self.walk(n, &seq, &core, unbounded))).collect();
        let passes: BTreeSet<NodeId> = walks.iter().filter(|(n, w)| self.passes_test(&hb, cp[*n].expect("live").1, &cdel[*n], &self.effs(&w.dels))).map(|(n, _)| *n).collect();
        // the visible set: Jacobi rounds from {ROOT} to the least fixpoint
        let mut vis: BTreeSet<NodeId> = BTreeSet::from([ROOT]);
        loop {
            let add: Vec<NodeId> = live.iter().copied().filter(|n| !vis.contains(n) && (vis.contains(&cp[n].expect("live").0) || passes.contains(n))).collect();
            if add.is_empty() {
                break;
            }
            vis.extend(add);
        }
        let roots: BTreeSet<NodeId> = live.iter().copied().filter(|n| vis.contains(n) && !vis.contains(&cp[n].expect("live").0)).collect();
        let mut views: BTreeMap<NodeId, NodeView> = BTreeMap::new();
        for &n in &nodes {
            let core_p = cp[&n].map_or(ROOT, |(p, _)| p);
            let root = roots.contains(&n);
            let status = if core_p == TRASH && cp[&n].is_some() {
                Status::Deleted
            } else if root && !walks[&n].ok {
                Status::Stuck
            } else if vis.contains(&n) {
                Status::Visible
            } else {
                Status::Hidden
            };
            let eff_n = cp[&n].map(|(_, m)| m);
            let reasons = match (status, cdel.get(&n)) {
                (Status::Hidden, Some(ds)) => ds.iter().copied().filter(|&d| eff_n.is_none_or(|e| !hb.conc(e, d))).collect(),
                _ => BTreeSet::new(),
            };
            let (parent, rescue) = if root {
                let w = &walks[&n];
                let anchor = match self.deleted_at(w.prev) {
                    Some(t) => Replica::del_oldp(&seq, w.prev, t).filter(|(p, _)| *p == w.stop).map(|(_, m)| m),
                    None => None,
                };
                (w.stop, Some(Rescue { anchor, deletes: cdel[&n].clone(), walked_past: w.dels.clone() }))
            } else {
                (core_p, None)
            };
            views.insert(n, NodeView { status, parent, rescue, reasons });
        }
        // same-spot order: the pre-order of the rebuilt pre-delete tree, or node ID
        let pre = if self.same_spot == SameSpotOrder::PreDeletePreorder && roots.len() > 1 { self.pre_delete_order(&seq, &nodes) } else { BTreeMap::new() };
        let sort = |v: &mut Vec<NodeId>| {
            if self.same_spot == SameSpotOrder::PreDeletePreorder {
                v.sort_by_key(|r| (pre.get(r).copied().unwrap_or(usize::MAX), *r));
            }
        };
        // visible children: core children in list order, rescued roots right after their
        // anchor, anchorless roots last
        let mut children: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for p in std::iter::once(ROOT).chain(nodes.iter().copied().filter(|n| vis.contains(n))) {
            let l = self.list(p);
            let at = |e: Option<Id>| -> Vec<NodeId> {
                roots.iter().copied().filter(|r| views[r].parent == p && views[r].rescue.as_ref().map(|x| x.anchor) == Some(e)).collect()
            };
            let mut out = Vec::new();
            for &e in &l {
                let c = self.child_of(e).expect("a move");
                if self.live_el(p, e) && vis.contains(&c) && !roots.contains(&c) {
                    out.push(c);
                }
                let mut here = at(Some(e));
                sort(&mut here);
                out.extend(here);
            }
            let mut last: Vec<NodeId> = roots.iter().copied().filter(|r| views[r].parent == p && views[r].rescue.as_ref().and_then(|x| x.anchor).is_none_or(|e| !l.contains(&e))).collect();
            sort(&mut last);
            out.extend(last);
            children.insert(p, out);
        }
        VisibleTree { nodes: views, children }
    }

    /// Conflict reports, with happened-before from the skeleton.
    pub fn reports(&self) -> BTreeSet<ConflictReport> {
        self.reports_with(Causality::Skeleton)
    }

    pub fn reports_with(&self, c: Causality) -> BTreeSet<ConflictReport> {
        self.reports_of(&self.projection_with(c))
    }

    /// The reports, given this replica's projection: every cycle-skip of the op set and every
    /// rescue of the projection (with its root's `walked_past`).
    pub fn reports_of(&self, p: &VisibleTree) -> BTreeSet<ConflictReport> {
        let (seq, _) = self.seq_log();
        let skips = seq.iter().filter(|(_, s)| *s).map(|(lm, _)| ConflictReport::CycleSkip { mover: lm.meta.agent, node: lm.child, op: lm.meta });
        let rescues = p.nodes.iter().flat_map(|(n, v)| {
            v.rescue.iter().flat_map(move |r| r.deletes.iter().map(move |&delete| ConflictReport::Rescue { node: *n, delete, walked_past: r.walked_past.clone() }))
        });
        skips.chain(rescues).collect()
    }

    /// the visible set of the core tree alone: nodes reaching ROOT through core parents
    fn core_visible(&self) -> BTreeSet<NodeId> {
        let mut v = BTreeSet::from([ROOT]);
        loop {
            let add: Vec<NodeId> = self.nodes().into_iter().filter(|n| !v.contains(n) && self.parent_of(*n).is_some_and(|(p, _)| v.contains(&p))).collect();
            if add.is_empty() {
                return v;
            }
            v.extend(add);
        }
    }
}

/// apply_op unrolled (undo every newer entry, do_op, redo oldest first), with the two core
/// mutants' cycle rules: `cycle_check` false = no-cycle-check; `stale` = the cycle rule looks
/// at the tree as it was before the undo (ancestor-check-stale).
fn apply_unrolled(op: &TreeMove, log: Vec<LogEntry>, tree: Edges, cycle_check: bool, stale: bool) -> (Vec<LogEntry>, Edges) {
    let do_checked = |mv: &TreeMove, t: &Edges, check: &Edges| -> (LogEntry, Edges) {
        let lm = LogEntry { time: mv.time, old_parent: fig4::get_parent(t, &mv.child), new_parent: mv.parent, meta: mv.meta, child: mv.child };
        let skip = mv.child == mv.parent || (cycle_check && fig4::ancestor(check, &mv.child, &mv.parent));
        let t2 = if skip {
            t.clone()
        } else {
            let mut t2: Edges = t.iter().filter(|(_, _, c)| *c != mv.child).cloned().collect();
            t2.insert((mv.parent, mv.meta, mv.child));
            t2
        };
        (lm, t2)
    };
    let later: Vec<LogEntry> = log.iter().filter(|lm| op.time < lm.time).cloned().collect();
    let rest: Vec<LogEntry> = log.iter().filter(|lm| op.time >= lm.time).cloned().collect();
    let mut t1 = tree.clone();
    for lm in &later {
        t1 = fig4::undo_op(lm, &t1);
    }
    let (lm0, t2) = do_checked(op, &t1, if stale { &tree } else { &t1 });
    let mut acc_tree = t2;
    let mut acc_log: Vec<LogEntry> = Vec::new();
    for lm in later.iter().rev() {
        let mv = TreeMove { time: lm.time, parent: lm.new_parent, meta: lm.meta, child: lm.child };
        let (lm2, t3) = do_checked(&mv, &acc_tree, if stale { &tree } else { &acc_tree });
        acc_log.insert(0, lm2);
        acc_tree = t3;
    }
    acc_log.push(lm0);
    acc_log.extend(rest);
    (acc_log, acc_tree)
}

// ---------------------------------------------------------------------------------------
// Invariants (the Quint model's `inv`), for fixtures, property tests and the ITF replay

/// The single-replica invariants: Fig. 4's `unique_parent` and `acyclic`; the tree is do_op
/// over the moves in timestamp order (§3.4); the resident log is that replay's tail above S;
/// `move_lww`; Def. 4 on every parent's list; the projection is total and acyclic, lists each
/// visible node exactly once (and nothing else), hides only with a reason, rescues soundly and
/// completely on the core path, and hangs every rescued subtree off the core-visible tree. With a full graph `g` also: the skeleton is happened-before between tree ops,
/// and the projection (and reports) through the skeleton equal the ones through `g`.
pub fn check_replica(r: &Replica, g: Option<&Graph>) -> Result<(), String> {
    if !fig4::unique_parent(&r.tree) {
        return Err("unique_parent".into());
    }
    if !fig4::acyclic(&r.tree) {
        return Err("acyclic".into());
    }
    let (seq, seq_tree) = r.seq_log();
    if r.tree != seq_tree {
        return Err(format!("timestamp_order: tree {:?}, replay {:?}", r.tree, seq_tree));
    }
    let want: Vec<LogEntry> = seq.iter().rev().map(|(lm, _)| lm.clone()).filter(|lm| !r.compacted.contains(&lm.meta)).collect();
    if r.log != want {
        return Err(format!("resident log {:?}, replay above S {:?}", r.log, want));
    }
    for c in r.nodes() {
        let applied: Vec<&LogEntry> = seq.iter().filter(|(lm, s)| lm.child == c && !s).map(|(lm, _)| lm).collect();
        if let Some(last) = applied.iter().max_by_key(|lm| lm.time) {
            if !r.tree.contains(&(last.new_parent, last.meta, c)) {
                return Err(format!("move_lww: {c:?}"));
            }
        }
    }
    let origins = r.origins();
    for p in r.lists.keys() {
        def4(&r.list(*p), &origins).map_err(|e| format!("sibling_order under {p:?}: {e:?}"))?;
    }
    let causality = g.map_or(Causality::Skeleton, Causality::Graph);
    let pr = r.projection_with(causality);
    let hb = r.hb(causality);
    let nodes = r.nodes();
    let core = r.core_visible();
    let mut listed: BTreeMap<NodeId, usize> = BTreeMap::new();
    for (p, kids) in &pr.children {
        if *p != ROOT && pr.status(*p) != Some(Status::Visible) {
            return Err(format!("children listed for non-visible {p:?}"));
        }
        for k in kids {
            *listed.entry(*k).or_default() += 1;
            if pr.nodes[k].parent != *p {
                return Err(format!("{k:?} listed under {p:?}, its visible parent is {:?}", pr.nodes[k].parent));
            }
        }
    }
    for &n in &nodes {
        let v = &pr.nodes[&n];
        let want = usize::from(v.status == Status::Visible);
        if listed.get(&n).copied().unwrap_or(0) != want {
            return Err(format!("no_duplicate: {n:?} ({:?}) listed {:?} times", v.status, listed.get(&n)));
        }
        match v.status {
            Status::Stuck => return Err(format!("projection_total: {n:?} stuck")),
            Status::Hidden if v.reasons.is_empty() => return Err(format!("hidden_with_reason: {n:?}")),
            Status::Visible => {
                let mut x = n;
                for _ in 0..=nodes.len() {
                    if x == ROOT || pr.status(x) != Some(Status::Visible) {
                        break;
                    }
                    x = pr.nodes[&x].parent;
                }
                if x != ROOT {
                    return Err(format!("projection_acyclic: {n:?} does not reach ROOT through visible parents"));
                }
            }
            _ => {}
        }
        if let Some(rs) = &v.rescue {
            // rescue_sound, on the core path: the reported deletes are those of the deleted nodes
            // on n's core path, each concurrent with n's effective move
            let e = r.eff(n).ok_or("rescued node without parent")?;
            let want = r.effs(&r.core_dels(n));
            if rs.deletes != want {
                return Err(format!("rescue_sound: {n:?} reports {:?}, its core path's deletes are {want:?}", rs.deletes));
            }
            if let Some(d) = rs.deletes.iter().find(|&&d| !hb.conc(e, d)) {
                return Err(format!("rescue_sound: {n:?} {d:?}"));
            }
            // rescue_walked_past (v0.4.1): distinct deleted nodes, nearest first, starting with
            // the deleted node on n's core path (the walk follows core parents until it meets it)
            let cd = r.core_dels(n);
            let distinct: BTreeSet<NodeId> = rs.walked_past.iter().copied().collect();
            if distinct.len() != rs.walked_past.len()
                || rs.walked_past.iter().any(|&d| pr.status(d) != Some(Status::Deleted))
                || rs.walked_past.first().is_none_or(|d| !cd.contains(d))
            {
                return Err(format!("rescue_walked_past: {n:?} walked past {:?}, core path's deleted nodes {cd:?}", rs.walked_past));
            }
            // rescue_under_core_visible: a rescued subtree hangs off the core-visible tree
            if !core.contains(&v.parent) {
                return Err(format!("rescue_under_core_visible: {n:?} shown under {:?}", v.parent));
            }
        }
    }
    // rescue_complete: a live node, not core-visible, whose effective move is concurrent with
    // the delete of every deleted node on its core path is visible
    for &n in &nodes {
        if let Some((p, m)) = r.parent_of(n) {
            let ds = r.effs(&r.core_dels(n));
            if p != TRASH && !core.contains(&n) && !ds.is_empty() && ds.iter().all(|&d| hb.conc(m, d)) && pr.status(n) != Some(Status::Visible) {
                return Err(format!("rescue_complete: {n:?}"));
            }
        }
    }
    if let Some(g) = g {
        let tops: Vec<Id> = r.ops.iter().filter(|(_, (_, o))| matches!(o, TreeOp::Move { .. })).map(|(id, _)| *id).collect();
        for &b in &tops {
            let ga = graph_ancestors(g, b);
            let sa = r.skeleton_ancestors(b);
            for &a in &tops {
                if a != b && ga.contains(&a) != sa.contains(&a) {
                    return Err(format!("skeleton_is_happened_before: {a:?} before {b:?}: graph {}, skeleton {}", ga.contains(&a), sa.contains(&a)));
                }
            }
        }
        let ps = r.projection();
        if ps != pr {
            return Err(format!("projection_unchanged_by_compaction: skeleton {ps:?}, graph {pr:?}"));
        }
    }
    Ok(())
}

/// What replicas with the same events must agree on: the resident log above S (`stable`: the
/// events below S), the tree, every parent's core children, the projection and the reports.
pub fn convergence_view(r: &Replica, stable: &BTreeSet<Id>) -> String {
    let log: Vec<&LogEntry> = r.log.iter().filter(|lm| !stable.contains(&lm.meta)).collect();
    let kids: BTreeMap<NodeId, Vec<NodeId>> = [ROOT, TRASH].into_iter().chain(r.nodes()).map(|p| (p, r.children(p))).collect();
    let p = r.projection();
    format!("{log:?}\n{:?}\n{kids:?}\n{p:?}\n{:?}", r.tree, r.reports_of(&p))
}

/// tree.converge: replicas with the same event set have the same convergence view.
pub fn check_converge(reps: &[&Replica], stable: &BTreeSet<Id>) -> Result<(), String> {
    for (i, a) in reps.iter().enumerate() {
        for b in &reps[i + 1..] {
            if a.ops.keys().eq(b.ops.keys()) {
                let (va, vb) = (convergence_view(a, stable), convergence_view(b, stable));
                if va != vb {
                    return Err(format!("converge: same events, different state:\n{va}\n---\n{vb}"));
                }
            }
        }
    }
    Ok(())
}
