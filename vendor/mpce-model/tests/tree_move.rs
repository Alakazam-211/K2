//! v0.4.0: the tree reference model (`mpce_model::tree_move`).
//!
//! - Fig. 4 unit tests (the transcription converges over every delivery order).
//! - The fixtures of `spec/quint/tree_fixtures.qnt` (modules `tree_fixtures`, nodes A = 2,
//!   B = 3, C = 4, and `tree_fixtures4`, plus D = 5; all created under ROOT by the seed; agents
//!   1, 2, 3), ported one to one. As in Quint, every invariant (`check_replica` on each replica
//!   and the ghost, with the full graph; and `check_converge` over the replicas, the ghost and an
//!   uncompacted twin of each replica, so a compacted or restarted replica is compared with its
//!   twin at every step) is checked after every step.
//! - Mutants: every `Variant` mutant must fail a fixture or the property test.
//! - Property tests (`MPCE_PROP_SEEDS`, default 100): 3 replicas, 3 seeded nodes (even seeds)
//!   or 4 (odd seeds), random moves, creates (up to 6 nodes), deletes (incl. re-deletes inside
//!   TRASH), restores, non-tree events, random delivery orders and partial syncs, compaction at
//!   random critical points (and events integrated with parents at or below them), restarts
//!   from snapshots; every invariant (incl. `projection_acyclic`) after every step, a full sync
//!   at the end.

use mpce_model::text::Id;
use mpce_model::tree_move::{
    check_converge, check_replica, convergence_view, fig4, seed_events, Causality, ConflictReport, Graph, MoveError, NodeId, Replica, SameSpotOrder, Status, TreeEvent,
    VisibleTree, Variant,
};
use std::collections::{BTreeMap, BTreeSet};

const ROOT: u64 = 0;
const TRASH: u64 = 1;
const A: u64 = 2;
const B: u64 = 3;
const C: u64 = 4;
const D: u64 = 5;

const VIS: Status = Status::Visible;
const DEL: Status = Status::Deleted;
const HID: Status = Status::Hidden;

fn id(a: u64, s: u64) -> Id {
    Id::new(a, s)
}

fn n(x: u64) -> NodeId {
    NodeId(x)
}

macro_rules! ensure {
    ($c:expr) => {
        if !$c {
            return Err(format!("line {}: {}", line!(), stringify!($c)));
        }
    };
}

macro_rules! ensure_eq {
    ($a:expr, $b:expr) => {{
        let (x, y) = (&$a, &$b);
        if x != y {
            return Err(format!("line {}: {} = {:?}, want {:?}", line!(), stringify!($a), x, y));
        }
    }};
}

fn ancestors(g: &Graph, x: Id) -> BTreeSet<Id> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<Id> = g[&x].iter().copied().collect();
    while let Some(y) = stack.pop() {
        if seen.insert(y) {
            stack.extend(g[&y].iter().copied());
        }
    }
    seen
}

// ---------------------------------------------------------------------------------------
// The harness: replicas, a ghost (receives every event when generated), an uncompacted twin
// of every replica, the full event graph, omniscient stability (as in the Quint model)

#[derive(Default, Debug, Clone, Copy)]
struct Stats {
    compactions: usize,
    /// events integrated at a replica with a parent it had compacted
    over_compacted: usize,
    restarts: usize,
    creates: usize,
    rescues: usize,
    skips: usize,
    /// nodes at the end (seeded + created)
    nodes: usize,
    /// rescue roots at the end shown under a node other than ROOT
    rescued_under_node: usize,
}

struct Sim {
    reps: BTreeMap<u64, Replica>,
    twins: BTreeMap<u64, Replica>,
    ghost: Replica,
    events: BTreeMap<Id, TreeEvent>,
    graph: Graph,
    check: bool,
    stats: Stats,
}

impl Sim {
    fn new(variant: Variant, agents: &[u64], nodes: &[u64]) -> Sim {
        let seeds = seed_events(nodes);
        let mut r0 = Replica::with_variant(variant);
        for e in &seeds {
            r0.deliver(e).expect("seed");
        }
        Sim {
            reps: agents.iter().map(|&a| (a, r0.clone())).collect(),
            twins: agents.iter().map(|&a| (a, r0.clone())).collect(),
            ghost: r0,
            events: seeds.iter().map(|e| (e.id, e.clone())).collect(),
            graph: seeds.iter().map(|e| (e.id, e.parents.clone())).collect(),
            check: true,
            stats: Stats::default(),
        }
    }

    fn fixture(v: Variant) -> Sim {
        Sim::new(v, &[1, 2, 3], &[A, B, C])
    }

    /// `tree_fixtures4`: four seeded nodes
    fn fixture4(v: Variant) -> Sim {
        Sim::new(v, &[1, 2, 3], &[A, B, C, D])
    }

    fn note_parents(&mut self, a: u64, e: &TreeEvent) {
        if e.parents.iter().any(|p| self.reps[&a].compacted().contains(p)) {
            self.stats.over_compacted += 1;
        }
    }

    /// a local event of agent a: record it, deliver it to the ghost and a's twin
    fn local(&mut self, a: u64, f: impl FnOnce(&mut Replica) -> Result<TreeEvent, MoveError>) -> Result<Id, String> {
        let before = self.reps[&a].clone();
        let e = f(self.reps.get_mut(&a).unwrap()).map_err(|e| format!("local op at {a}: {e:?}"))?;
        if e.parents.iter().any(|p| before.compacted().contains(p)) {
            self.stats.over_compacted += 1;
        }
        self.ghost.deliver(&e).map_err(|x| format!("ghost: {x:?}"))?;
        self.twins.get_mut(&a).unwrap().deliver(&e).map_err(|x| format!("twin {a}: {x:?}"))?;
        self.graph.insert(e.id, e.parents.clone());
        self.events.insert(e.id, e.clone());
        self.after()?;
        Ok(e.id)
    }

    fn mv(&mut self, a: u64, c: u64, p: u64, i: usize) -> Result<Id, String> {
        self.local(a, |r| r.move_as(a, n(c), n(p), i))
    }

    fn create(&mut self, a: u64, c: u64, p: u64, i: usize) -> Result<Id, String> {
        self.stats.creates += 1;
        self.local(a, |r| r.create_as(a, n(c), n(p), i))
    }

    fn other(&mut self, a: u64) -> Result<Id, String> {
        self.local(a, |r| r.other_as(a))
    }

    fn deliver_quiet(&mut self, a: u64, x: Id) -> Result<(), String> {
        let e = self.events[&x].clone();
        self.note_parents(a, &e);
        match self.reps.get_mut(&a).unwrap().deliver(&e) {
            Ok(true) => {}
            other => return Err(format!("deliver {x:?} to {a}: {other:?}")),
        }
        self.twins.get_mut(&a).unwrap().deliver(&e).map_err(|x| format!("twin {a}: {x:?}"))?;
        Ok(())
    }

    fn deliver(&mut self, a: u64, x: Id) -> Result<(), String> {
        self.deliver_quiet(a, x)?;
        self.after()
    }

    /// what f has that t lacks, in timestamp order (a causal order)
    fn missing(&self, f: &Replica, t: u64) -> Vec<Id> {
        let have = self.reps[&t].received();
        let mut todo: Vec<&TreeEvent> = f.received().iter().filter(|x| !have.contains(x)).map(|x| &self.events[x]).collect();
        todo.sort_by_key(|e| e.ts());
        todo.iter().map(|e| e.id).collect()
    }

    /// fixtures: deliver everything f has that t lacks, in timestamp order
    fn sync(&mut self, f: u64, t: u64) -> Result<(), String> {
        for x in self.missing(&self.reps[&f].clone(), t) {
            self.deliver_quiet(t, x)?;
        }
        self.after()
    }

    /// stable events: every replica has every event with an equal or lower timestamp
    fn stable(&self) -> BTreeSet<Id> {
        self.events.values().filter(|x| self.events.values().all(|y| y.ts() > x.ts() || self.reps.values().all(|r| r.has(y.id)))).map(|x| x.id).collect()
    }

    /// the largest critical version below S: stable events every non-stable event descends from
    fn critical(&self) -> BTreeSet<Id> {
        let st = self.stable();
        let anc: Vec<BTreeSet<Id>> = self.events.keys().filter(|x| !st.contains(x)).map(|&x| ancestors(&self.graph, x)).collect();
        st.into_iter().filter(|x| anc.iter().all(|s| s.contains(x))).collect()
    }

    fn compact_at(&mut self, a: u64, cv: &BTreeSet<Id>) -> Result<(), String> {
        self.reps.get_mut(&a).unwrap().compact(cv).map_err(|e| format!("compact: {e:?}"))?;
        self.stats.compactions += 1;
        self.after()
    }

    /// compact replica a at the current critical version (as the Quint model's `compact`)
    fn compact(&mut self, a: u64) -> Result<(), String> {
        let cv: BTreeSet<Id> = self.critical().into_iter().filter(|x| self.reps[&a].has(*x)).collect();
        self.compact_at(a, &cv)
    }

    /// restart replica a from its snapshot
    fn restart(&mut self, a: u64) -> Result<(), String> {
        let r = &self.reps[&a];
        let snap = r.snapshot();
        if let Some(x) = snap.parents.keys().find(|x| r.compacted().contains(x)) {
            return Err(format!("snapshot keeps the parents of compacted {x:?}"));
        }
        let r2 = Replica::load(&snap).map_err(|e| format!("load: {e:?}"))?;
        let st = self.stable();
        if convergence_view(&r2, &st) != convergence_view(r, &st) || r2.log() != r.log() || r2.frontier() != r.frontier() {
            return Err("restart changed the replica".into());
        }
        for x in r.compacted() {
            if r2.event(*x).is_some() {
                return Err(format!("reloaded replica has the structure of compacted {x:?}"));
            }
        }
        self.reps.insert(a, r2);
        self.stats.restarts += 1;
        self.after()
    }

    fn after(&self) -> Result<(), String> {
        if self.check {
            self.inv()
        } else {
            Ok(())
        }
    }

    fn check_one(&self, who: &str, r: &Replica) -> Result<(), String> {
        check_replica(r, Some(&self.graph)).map_err(|e| format!("{who}: {e}"))
    }

    fn inv(&self) -> Result<(), String> {
        for (a, r) in &self.reps {
            self.check_one(&format!("replica {a}"), r)?;
        }
        self.check_one("ghost", &self.ghost)?;
        let all: Vec<&Replica> = self.reps.values().chain(self.twins.values()).chain(std::iter::once(&self.ghost)).collect();
        check_converge(&all, &self.stable())
    }

    // fixture helpers (the Quint fixtures' `parentIn`, `statusIn`, ...): the projection from
    // the full graph, as `projFull` (inv checks it equals the skeleton's)
    fn proj(&self, a: u64) -> VisibleTree {
        self.reps[&a].projection_with(Causality::Graph(&self.graph))
    }
    fn parent_in(&self, a: u64, x: u64) -> u64 {
        self.reps[&a].parent_of(n(x)).map_or(u64::MAX, |(p, _)| p.0)
    }
    fn status_in(&self, a: u64, x: u64) -> Status {
        self.proj(a).status(n(x)).expect("a node")
    }
    fn vparent_in(&self, a: u64, x: u64) -> u64 {
        self.proj(a).nodes[&n(x)].parent.0
    }
    fn order_in(&self, a: u64, p: u64) -> Vec<u64> {
        self.proj(a).children(n(p)).iter().map(|x| x.0).collect()
    }
    fn skips_in(&self, a: u64) -> BTreeSet<(u64, u64, Id)> {
        self.reps[&a]
            .reports_with(Causality::Graph(&self.graph))
            .into_iter()
            .filter_map(|r| if let ConflictReport::CycleSkip { mover, node, op } = r { Some((mover, node.0, op)) } else { None })
            .collect()
    }
    fn rescues_in(&self, a: u64) -> BTreeSet<(u64, Id)> {
        self.proj(a).rescues().into_iter().map(|(x, d)| (x.0, d)).collect()
    }
    /// each rescue report's (node, delete, walked_past) (`ConflictReport` equality ignores
    /// walked_past, so the fixtures compare these tuples)
    fn walked_in(&self, a: u64) -> BTreeSet<(u64, Id, Vec<u64>)> {
        self.reps[&a]
            .reports_with(Causality::Graph(&self.graph))
            .into_iter()
            .filter_map(|r| if let ConflictReport::Rescue { node, delete, walked_past } = r { Some((node.0, delete, walked_past.iter().map(|x| x.0).collect())) } else { None })
            .collect()
    }
}

fn set<T: Ord + Clone>(xs: &[T]) -> BTreeSet<T> {
    xs.iter().cloned().collect()
}

// ---------------------------------------------------------------------------------------
// Fig. 4

#[test]
fn fig4_get_parent_follows_the_isabelle_definition() {
    let t: fig4::Tree<u32, u32> = set(&[(1, 10, 5)]);
    assert_eq!(fig4::get_parent(&t, &5), Some((1, 10)));
    assert_eq!(fig4::get_parent(&t, &1), None);
    // two metas under one parent: no unique meta for that parent
    let t: fig4::Tree<u32, u32> = set(&[(1, 10, 5), (1, 11, 5)]);
    assert_eq!(fig4::get_parent(&t, &5), None);
    // ∃!parent. ∃!meta: parent 1 has two metas, parent 2 exactly one
    let t: fig4::Tree<u32, u32> = set(&[(1, 10, 5), (1, 11, 5), (2, 12, 5)]);
    assert_eq!(fig4::get_parent(&t, &5), Some((2, 12)));
    assert!(!fig4::unique_parent(&t));
}

/// The paper's theorem on small inputs: apply_ops over every permutation of a set of moves
/// gives one state, which is do_op in timestamp order; unique_parent and acyclic hold.
#[test]
fn fig4_apply_ops_converges_over_every_order() {
    type M = fig4::Move<u32, u32, u32>;
    let mv = |t, p, c| M { time: t, parent: p, meta: t, child: c };
    // nodes 1..=4 under 0; Fig. 1, Fig. 2 and a §3.6 delete (8 = trash)
    let sets: Vec<Vec<M>> = vec![
        vec![mv(1, 0, 1), mv(2, 0, 2), mv(3, 0, 3), mv(4, 2, 1), mv(5, 3, 1)],
        vec![mv(1, 0, 1), mv(2, 0, 2), mv(3, 1, 2), mv(4, 2, 1)],
        vec![mv(1, 0, 1), mv(2, 0, 2), mv(3, 1, 2), mv(4, 8, 2), mv(5, 2, 1), mv(6, 1, 2)],
    ];
    fn perms(v: &[usize]) -> Vec<Vec<usize>> {
        if v.len() <= 1 {
            return vec![v.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..v.len() {
            let mut rest = v.to_vec();
            let x = rest.remove(i);
            for mut p in perms(&rest) {
                p.insert(0, x);
                out.push(p);
            }
        }
        out
    }
    for ops in sets {
        let mut sorted = ops.clone();
        sorted.sort_by_key(|m| m.time);
        let seq_tree = sorted.iter().fold(fig4::Tree::new(), |t, op| fig4::do_op(op, &t).1);
        let want = fig4::apply_ops(&sorted);
        assert_eq!(want.1, seq_tree);
        for p in perms(&(0..ops.len()).collect::<Vec<_>>()) {
            let o: Vec<M> = p.iter().map(|&i| ops[i].clone()).collect();
            let s = fig4::apply_ops(&o);
            assert_eq!(s, want, "order {p:?}");
            assert!(fig4::unique_parent(&s.1) && fig4::acyclic(&s.1));
        }
    }
}

// ---------------------------------------------------------------------------------------
// The fixtures of tree_fixtures.qnt

type Fixture = (&'static str, fn(Variant) -> Result<(), String>);

const FIXTURES: [Fixture; 19] = [
    ("tf_fig1", tf_fig1),
    ("tf_fig2", tf_fig2),
    ("tf_delete_makes_move_safe", tf_delete_makes_move_safe),
    ("tf_simple_rescue", tf_simple_rescue),
    ("tf_move_after_delete_not_rescued", tf_move_after_delete_not_rescued),
    ("tf_nested_inner_after_move", tf_nested_inner_after_move),
    ("tf_nested_outer_after_move", tf_nested_outer_after_move),
    ("tf_nested_both_concurrent", tf_nested_both_concurrent),
    ("tf_restore_cancels_rescue", tf_restore_cancels_rescue),
    ("tf_oldp_loop", tf_oldp_loop),
    ("tf_rescue_across_s", tf_rescue_across_s),
    ("tf_truncation_counterexample", tf_truncation_counterexample),
    ("tf_compaction_keeps_skeleton", tf_compaction_keeps_skeleton),
    ("tf_rescued_despite_seen_delete_up_the_walk", tf_rescued_despite_seen_delete_up_the_walk),
    ("tf_stable_pair_pulled_into_rescue", tf_stable_pair_pulled_into_rescue),
    // tree_fixtures4 (four nodes)
    ("tf_same_spot_subtree_preorder", tf_same_spot_subtree_preorder),
    ("tf_same_spot_oldp_loop", tf_same_spot_oldp_loop),
    ("tf_nested_rescue_beside", tf_nested_rescue_beside),
    ("tf_mutual_rescue_cycle", tf_mutual_rescue_cycle),
];

/// [Move] Fig. 1: concurrent moves of A under B (agent 1) and under C (agent 2); the higher
/// timestamp, (5,2), wins on both replicas.
fn tf_fig1(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, B, 0)?;
    ensure_eq!(s.parent_in(1, A), B);
    s.mv(2, A, C, 0)?;
    ensure_eq!(s.parent_in(2, A), C);
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    ensure_eq!(s.parent_in(1, A), C);
    ensure_eq!(s.parent_in(2, A), C);
    Ok(())
}

/// [Move] Fig. 2: B under A (agent 1, (5,1)) ∥ A under B (agent 2, (5,2)). The lower timestamp
/// applies; the other is a cycle-skip, reported to its mover.
fn tf_fig2(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, B, A, 0)?;
    s.mv(2, A, B, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    for a in [1, 2] {
        ensure_eq!(s.parent_in(a, B), A);
        ensure_eq!(s.parent_in(a, A), ROOT);
        ensure_eq!(s.skips_in(a), set(&[(2, A, id(2, 0))]));
    }
    Ok(())
}

/// [Move] §3.6: a delete that makes a previously unsafe move safe; A is then rescued, its walk
/// (B, then B's oldp A itself: a revisit) falling back to ROOT.
fn tf_delete_makes_move_safe(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, B, A, 0)?;
    s.sync(1, 2)?;
    s.mv(1, B, TRASH, 0)?;
    s.mv(2, A, B, 0)?;
    ensure_eq!(s.parent_in(2, A), ROOT);
    ensure_eq!(s.skips_in(2), set(&[(2, A, id(2, 0))]));
    s.sync(1, 2)?;
    ensure_eq!(s.parent_in(2, A), B);
    ensure_eq!(s.parent_in(2, B), TRASH);
    ensure!(s.skips_in(2).is_empty());
    s.sync(2, 1)?;
    ensure_eq!(s.status_in(1, B), DEL);
    ensure_eq!(s.status_in(1, A), VIS);
    ensure_eq!(s.vparent_in(1, A), ROOT);
    ensure_eq!(s.rescues_in(1), set(&[(A, id(1, 1))]));
    ensure_eq!(s.walked_in(1), set(&[(A, id(1, 1), vec![B])]));
    Ok(())
}

/// A simple rescue: agent 1 deletes A; agent 2 concurrently moves B under A. B is shown under
/// ROOT at A's old position, and reported.
fn tf_simple_rescue(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, TRASH, 0)?;
    s.mv(2, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    ensure_eq!(s.parent_in(1, B), A);
    ensure_eq!(s.status_in(1, A), DEL);
    ensure_eq!(s.status_in(1, B), VIS);
    ensure_eq!(s.vparent_in(1, B), ROOT);
    ensure_eq!(s.order_in(1, ROOT), vec![B, C]);
    ensure_eq!(s.rescues_in(1), set(&[(B, id(1, 0))]));
    ensure_eq!(s.walked_in(1), set(&[(B, id(1, 0), vec![A])]));
    Ok(())
}

/// The same move made after seeing the delete is not rescued.
fn tf_move_after_delete_not_rescued(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.mv(2, B, A, 0)?;
    s.sync(2, 1)?;
    ensure_eq!(s.status_in(1, B), HID);
    ensure_eq!(s.order_in(1, ROOT), vec![C]);
    ensure!(s.rescues_in(1).is_empty());
    Ok(())
}

/// Nested deletes, B under A. (i) C moved under B, then B deleted by the same agent, A deleted
/// concurrently: C stays hidden (B's delete saw the move).
fn tf_nested_inner_after_move(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    s.mv(2, C, B, 0)?;
    s.mv(2, B, TRASH, 0)?;
    s.mv(1, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    ensure_eq!(s.status_in(1, C), HID);
    ensure_eq!(s.status_in(2, C), HID);
    ensure!(s.rescues_in(1).is_empty());
    Ok(())
}

/// (ii) B deleted concurrently with the move of C under B, A deleted by the mover after its
/// move. The rescue is decided on C's core path (C, B, TRASH): B's delete is concurrent, so C
/// is rescued (K2 Cores' ruling; it was hidden under the earlier "every delete on the walk"
/// reading, A's delete having seen the move). It shows under ROOT at A's spot, reported
/// against B's delete only.
fn tf_nested_outer_after_move(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    s.mv(1, B, TRASH, 0)?;
    s.mv(2, C, B, 0)?;
    s.mv(2, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    for a in [1, 2] {
        ensure_eq!(s.status_in(a, C), VIS);
        ensure_eq!(s.vparent_in(a, C), ROOT);
        ensure_eq!(s.order_in(a, ROOT), vec![C]);
        ensure_eq!(s.rescues_in(a), set(&[(C, id(1, 1))]));
        ensure_eq!(s.walked_in(a), set(&[(C, id(1, 1), vec![B, A])]));
    }
    Ok(())
}

/// (iii) both deletes concurrent with the move: C is rescued (its core path has B) and shown
/// under the nearest core-visible node above the outermost deleted one (ROOT, A's oldp); the
/// report names B's delete (the core path's), not A's.
fn tf_nested_both_concurrent(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    s.mv(1, B, TRASH, 0)?;
    s.mv(3, A, TRASH, 0)?;
    s.mv(2, C, B, 0)?;
    s.sync(1, 2)?;
    s.sync(3, 2)?;
    s.sync(2, 1)?;
    s.sync(2, 3)?;
    ensure_eq!(s.status_in(2, C), VIS);
    ensure_eq!(s.vparent_in(2, C), ROOT);
    ensure_eq!(s.rescues_in(2), set(&[(C, id(1, 1))]));
    ensure_eq!(s.walked_in(2), set(&[(C, id(1, 1), vec![B, A])]));
    Ok(())
}

/// Restore cancels rescue: after the simple rescue, A is moved back under ROOT; B is under A
/// again and no rescue is reported.
fn tf_restore_cancels_rescue(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, TRASH, 0)?;
    s.mv(2, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    ensure_eq!(s.vparent_in(1, B), ROOT);
    s.mv(1, A, ROOT, 0)?;
    s.sync(1, 2)?;
    ensure_eq!(s.status_in(2, A), VIS);
    ensure_eq!(s.vparent_in(2, B), A);
    ensure!(s.rescues_in(2).is_empty());
    Ok(())
}

/// The oldp loop (D = A, E = B, X = C): the walk from X goes E -> D -> E (revisit) -> ROOT.
fn tf_oldp_loop(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, B, 0)?;
    s.mv(1, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.mv(2, B, A, 0)?;
    s.mv(2, B, TRASH, 0)?;
    s.mv(3, C, B, 0)?;
    s.sync(1, 3)?;
    s.sync(2, 3)?;
    ensure_eq!(s.parent_in(3, C), B);
    ensure_eq!(s.parent_in(3, B), TRASH);
    ensure_eq!(s.parent_in(3, A), TRASH);
    ensure_eq!(s.status_in(3, C), VIS);
    ensure_eq!(s.vparent_in(3, C), ROOT);
    // B, then B's delete's old parent A, then A's (B again: a revisit, so ROOT)
    ensure_eq!(s.walked_in(3), set(&[(C, id(2, 1), vec![B, A])]));
    Ok(())
}

/// A rescue across the S boundary: the simple rescue becomes stable, every replica compacts;
/// the rescue is unchanged, also after a later unrelated move and after a restore.
fn tf_rescue_across_s(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, TRASH, 0)?;
    s.mv(2, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    s.sync(1, 3)?;
    s.compact(1)?;
    s.compact(2)?;
    s.compact(3)?;
    ensure_eq!(s.status_in(1, B), VIS);
    ensure_eq!(s.vparent_in(3, B), ROOT);
    ensure!(s.reps[&1].compacted().contains(&id(2, 0)));
    ensure_eq!(s.walked_in(3), set(&[(B, id(1, 0), vec![A])]));
    ensure!(s.reps[&1].log().iter().all(|lm| lm.meta != id(2, 0)));
    s.mv(3, C, ROOT, 0)?;
    s.sync(3, 1)?;
    s.sync(3, 2)?;
    ensure_eq!(s.order_in(1, ROOT), vec![C, B]);
    s.mv(1, A, ROOT, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    ensure_eq!(s.vparent_in(3, B), A);
    Ok(())
}

/// The v2 truncation counterexample: A's move's tparents run through the non-tree event's T;
/// A is rescued identically before and after compacting. Under the core-path rule the rescue
/// is decided on B's delete alone (A's core path is A, B, TRASH), so it is reported against
/// B's delete only; `tf_stable_pair_pulled_into_rescue` has a rescue that needs a pair below S.
fn tf_truncation_counterexample(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(3, B, C, 0)?;
    s.mv(2, C, TRASH, 0)?;
    s.sync(3, 2)?;
    s.sync(3, 1)?;
    s.other(1)?;
    s.mv(1, A, B, 0)?;
    s.deliver(2, id(1, 0))?;
    s.mv(2, B, TRASH, 0)?;
    s.sync(1, 3)?;
    s.deliver(3, id(2, 0))?;
    s.deliver(1, id(2, 0))?;
    s.deliver(2, id(1, 1))?;
    for r in s.reps.values().chain(std::iter::once(&s.ghost)) {
        ensure_eq!(r.tparents_of(id(1, 1)).cloned(), Some(set(&[id(3, 0)])));
    }
    ensure_eq!(s.status_in(2, A), VIS);
    ensure_eq!(s.vparent_in(2, A), ROOT);
    ensure_eq!(s.rescues_in(2), set(&[(A, id(2, 1))]));
    // B, then B's delete's old parent C (deleted, old parent ROOT)
    ensure_eq!(s.walked_in(2), set(&[(A, id(2, 1), vec![B, C])]));
    s.compact(2)?;
    ensure!(s.reps[&2].compacted().contains(&id(2, 0)));
    ensure!(!s.reps[&2].compacted().contains(&id(2, 1)));
    ensure_eq!(s.status_in(2, A), VIS);
    ensure_eq!(s.vparent_in(2, A), ROOT);
    Ok(())
}

/// Compaction keeps the skeleton: B moved under A after seeing A's delete stays hidden after
/// compaction; so does C, moved under B later with a parent below the compaction point.
fn tf_compaction_keeps_skeleton(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    s.mv(2, B, A, 0)?;
    s.sync(2, 1)?;
    s.sync(2, 3)?;
    s.compact(1)?;
    ensure_eq!(s.status_in(1, B), HID);
    s.other(1)?;
    s.mv(1, C, B, 0)?;
    ensure_eq!(s.status_in(1, C), HID);
    Ok(())
}

/// The accepted consequence of the core-path rule: B under A; agent 1 deletes A and agent 2
/// sees it; agent 3, not having seen it, deletes B; agent 2 then moves C under B. C's mover
/// had seen A's delete (further up the walk: B's old parent is A), but C's core path (C, B,
/// TRASH) has only B, whose delete is concurrent: C is rescued (the earlier reading hid it),
/// shown under ROOT at A's spot, reported against B's delete.
fn tf_rescued_despite_seen_delete_up_the_walk(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    s.mv(1, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.mv(3, B, TRASH, 0)?;
    s.mv(2, C, B, 0)?;
    for (f, t) in [(3, 1), (3, 2), (2, 1), (1, 3)] {
        s.sync(f, t)?;
    }
    ensure!(s.reps[&1].happened_before(id(1, 1), id(2, 0)));
    ensure!(s.reps[&1].concurrent(id(3, 0), id(2, 0)));
    for a in [1, 2, 3] {
        ensure_eq!(s.parent_in(a, C), B);
        ensure_eq!(s.status_in(a, A), DEL);
        ensure_eq!(s.status_in(a, B), DEL);
        ensure_eq!(s.status_in(a, C), VIS);
        ensure_eq!(s.vparent_in(a, C), ROOT);
        ensure_eq!(s.order_in(a, ROOT), vec![C]);
        ensure_eq!(s.rescues_in(a), set(&[(C, id(3, 0))]));
        ensure_eq!(s.walked_in(a), set(&[(C, id(3, 0), vec![B, A])]));
    }
    Ok(())
}

/// A rescue whose (move, delete) pair is stable before the rescue exists (v2's hole): A moved
/// under B (agent 1) concurrently with C's delete (agent 2), synced everywhere (both stable; no
/// rescue, B is under ROOT); agent 3 then moves B under C (after seeing the delete: B hidden).
/// A's core path is A, B, C, TRASH: A is rescued under ROOT at C's spot, also after every
/// replica compacts.
fn tf_stable_pair_pulled_into_rescue(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture(v);
    s.mv(1, A, B, 0)?;
    s.mv(2, C, TRASH, 0)?;
    for (f, t) in [(1, 2), (1, 3), (2, 1), (2, 3)] {
        s.sync(f, t)?;
    }
    ensure!(s.stable().is_superset(&set(&[id(1, 0), id(2, 0)])));
    ensure_eq!(s.status_in(1, A), VIS);
    ensure_eq!(s.vparent_in(1, A), B);
    s.mv(3, B, C, 0)?;
    ensure_eq!(s.status_in(3, B), HID);
    ensure_eq!(s.status_in(3, A), VIS);
    ensure_eq!(s.vparent_in(3, A), ROOT);
    ensure_eq!(s.order_in(3, ROOT), vec![A]);
    ensure_eq!(s.rescues_in(3), set(&[(A, id(2, 0))]));
    s.compact(3)?;
    ensure!(s.reps[&3].compacted().contains(&id(1, 0)) && s.reps[&3].compacted().contains(&id(2, 0)));
    s.sync(3, 1)?;
    s.sync(3, 2)?;
    for a in [1, 2] {
        s.compact(a)?;
    }
    for a in [1, 2, 3] {
        ensure_eq!(s.status_in(a, A), VIS);
        ensure_eq!(s.vparent_in(a, A), ROOT);
        ensure_eq!(s.rescues_in(a), set(&[(A, id(2, 0))]));
        // B is live (not core-visible): the walk goes on to its core parent C, deleted
        ensure_eq!(s.walked_in(a), set(&[(A, id(2, 0), vec![C])]));
    }
    Ok(())
}

/// Same-spot order (K2 Cores' rule): D under A; A deleted by agent 1; concurrently agent 2
/// moves B under D and C under A before D. Both are rescued at A's spot in ROOT; the rebuilt
/// pre-delete tree's pre-order there is A, C, D, B, so ROOT shows [C, B] (node ID order would
/// give [B, C]).
fn tf_same_spot_subtree_preorder(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture4(v);
    s.mv(1, D, A, 0)?;
    s.sync(1, 2)?;
    s.mv(1, A, TRASH, 0)?;
    s.mv(2, B, D, 0)?;
    s.mv(2, C, A, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    ensure_eq!(s.status_in(1, D), HID);
    ensure_eq!(s.order_in(1, ROOT), vec![C, B]);
    ensure_eq!(s.rescues_in(1), set(&[(B, id(1, 1)), (C, id(1, 1))]));
    ensure_eq!(s.walked_in(1), set(&[(B, id(1, 1), vec![A]), (C, id(1, 1), vec![A])]));
    let mut r = s.reps[&1].clone();
    r.set_same_spot_order(SameSpotOrder::NodeId);
    ensure_eq!(r.projection().children(n(ROOT)).to_vec(), vec![n(B), n(C)]);
    Ok(())
}

/// Same-spot order on the oldp loop: the loop of tf_oldp_loop (A deleted with old parent B; B
/// moved under A and deleted with old parent A), with C moved under B and D under A by agent 3,
/// which saw neither. Both are rescued and both walks fall back to ROOT (no spot: shown last).
/// The rebuilt pre-delete tree cycles (A's old parent is B, B's is A); by the visited-set rule
/// A and B are roots of the rebuild, ordered by node ID: pre-order ROOT, A, D, B, C, so ROOT
/// shows [D, C] (node ID order would give [C, D]).
fn tf_same_spot_oldp_loop(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture4(v);
    s.mv(1, A, B, 0)?;
    s.mv(1, A, TRASH, 0)?;
    s.sync(1, 2)?;
    s.mv(2, B, A, 0)?;
    s.mv(2, B, TRASH, 0)?;
    s.mv(3, C, B, 0)?;
    s.mv(3, D, A, 0)?;
    s.sync(1, 3)?;
    s.sync(2, 3)?;
    ensure_eq!(s.parent_in(3, C), B);
    ensure_eq!(s.parent_in(3, D), A);
    ensure_eq!(s.parent_in(3, A), TRASH);
    ensure_eq!(s.parent_in(3, B), TRASH);
    for x in [C, D] {
        ensure_eq!(s.status_in(3, x), VIS);
        ensure_eq!(s.vparent_in(3, x), ROOT);
        ensure_eq!(s.proj(3).nodes[&n(x)].rescue.as_ref().and_then(|r| r.anchor), None);
    }
    ensure_eq!(s.order_in(3, ROOT), vec![D, C]);
    ensure_eq!(s.rescues_in(3), set(&[(C, id(2, 1)), (D, id(1, 1))]));
    ensure_eq!(s.walked_in(3), set(&[(C, id(2, 1), vec![B, A]), (D, id(1, 1), vec![A, B])]));
    let mut r = s.reps[&3].clone();
    r.set_same_spot_order(SameSpotOrder::NodeId);
    ensure_eq!(r.projection().children(n(ROOT)).to_vec(), vec![n(C), n(D)]);
    Ok(())
}

/// A nested rescue root shows beside the outer rescued subtree: B under A (agent 1, synced);
/// agent 1 deletes D while agent 2 moves A under D (A is rescued: it shows at D's spot in ROOT,
/// B with it); agent 3 deletes B (old parent A) while agent 2 moves C under B. C's core path
/// has B, whose delete is concurrent: C is a rescue root. Its walk passes B, A (visible, but
/// rescued, not core-visible), D, to ROOT: C shows under ROOT beside A, at D's spot, after A
/// (the rebuilt pre-delete tree is D, A, B, C), not under A; reported against B's delete.
fn tf_nested_rescue_beside(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture4(v);
    s.mv(1, B, A, 0)?;
    s.sync(1, 2)?;
    s.sync(1, 3)?;
    s.mv(1, D, TRASH, 0)?;
    s.mv(2, A, D, 0)?;
    s.mv(3, B, TRASH, 0)?;
    s.mv(2, C, B, 0)?;
    for (f, t) in [(1, 2), (3, 2), (2, 1), (2, 3)] {
        s.sync(f, t)?;
    }
    for a in [1, 2, 3] {
        ensure_eq!(s.parent_in(a, A), D);
        ensure_eq!(s.parent_in(a, C), B);
        ensure_eq!(s.status_in(a, B), DEL);
        ensure_eq!(s.status_in(a, D), DEL);
        ensure_eq!((s.status_in(a, A), s.vparent_in(a, A)), (VIS, ROOT));
        ensure_eq!((s.status_in(a, C), s.vparent_in(a, C)), (VIS, ROOT));
        ensure_eq!(s.order_in(a, ROOT), vec![A, C]);
        ensure!(s.order_in(a, A).is_empty());
        ensure_eq!(s.rescues_in(a), set(&[(A, id(1, 1)), (C, id(3, 0))]));
        // C: B, then B's old parent A (live, rescued, not core-visible), its core parent D
        ensure_eq!(s.walked_in(a), set(&[(A, id(1, 1), vec![D]), (C, id(3, 0), vec![B, D])]));
    }
    Ok(())
}

/// The gap K2 Cores' ruling closes (X = A, Y = B, P = C, Q = D): agent 1 puts P under X and Q
/// under Y, syncs, then deletes P (old parent X) and Q (old parent Y); concurrently agent 2
/// moves Y under P and X under Q. Both moves are concurrent with the delete on their core path
/// (X: Q's; Y: P's), so X and Y are rescued. The old walk stopped at the first visible node and
/// showed X under Y and Y under X (a cycle, unreachable from ROOT); the walk now stops only at
/// core-visible nodes: each walk loops back to its start and falls back to ROOT, so X and Y
/// show at the end of ROOT, in the rebuilt pre-delete tree's order (all four nodes are on its
/// cycle, so they are roots of the rebuild by node ID: [X, Y]).
fn tf_mutual_rescue_cycle(v: Variant) -> Result<(), String> {
    let mut s = Sim::fixture4(v);
    s.mv(1, C, A, 0)?;
    s.mv(1, D, B, 0)?;
    s.sync(1, 2)?;
    s.mv(1, C, TRASH, 0)?;
    s.mv(1, D, TRASH, 0)?;
    s.mv(2, B, C, 0)?;
    s.mv(2, A, D, 0)?;
    s.sync(1, 2)?;
    s.sync(2, 1)?;
    for a in [1, 2] {
        ensure_eq!(s.parent_in(a, A), D);
        ensure_eq!(s.parent_in(a, B), C);
        ensure_eq!((s.status_in(a, A), s.vparent_in(a, A)), (VIS, ROOT));
        ensure_eq!((s.status_in(a, B), s.vparent_in(a, B)), (VIS, ROOT));
        ensure_eq!(s.order_in(a, ROOT), vec![A, B]);
        ensure_eq!(s.rescues_in(a), set(&[(A, id(1, 3)), (B, id(1, 2))]));
        ensure_eq!(s.walked_in(a), set(&[(A, id(1, 3), vec![D, C]), (B, id(1, 2), vec![C, D])]));
    }
    ensure_eq!(s.reps[&1].projection(), s.ghost.projection());
    Ok(())
}

#[test]
fn fixtures_pass() {
    let mut bad = Vec::new();
    for (name, f) in FIXTURES {
        if let Err(e) = f(Variant::Correct) {
            bad.push(format!("{name}: {e}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn rescue_report_identity_ignores_walked_past() {
    let (x, d) = (n(C), id(1, 1));
    let r1 = ConflictReport::Rescue { node: x, delete: d, walked_past: vec![n(B), n(A)] };
    let r2 = ConflictReport::Rescue { node: x, delete: d, walked_past: vec![] };
    assert_eq!(r1, r2);
    assert_eq!(r1.key(), r2.key());
    assert_eq!(BTreeSet::from([r1.clone(), r2]).len(), 1);
    assert_ne!(r1, ConflictReport::Rescue { node: x, delete: id(1, 2), walked_past: vec![n(B), n(A)] });
}

// ---------------------------------------------------------------------------------------
// Unit tests: compaction, snapshots, stability

#[test]
fn compaction_keeps_t_and_the_skeleton_and_restart_needs_nothing_else() {
    let mut s = Sim::fixture(Variant::Correct);
    s.mv(1, A, TRASH, 0).unwrap();
    s.other(2).unwrap();
    s.mv(2, B, A, 0).unwrap();
    for (f, t) in [(1, 2), (1, 3), (2, 1), (2, 3)] {
        s.sync(f, t).unwrap();
    }
    s.compact(1).unwrap();
    let r = &s.reps[&1];
    assert!(r.compacted().contains(&id(2, 1)));
    assert!(r.event(id(2, 1)).is_none(), "graph structure discarded");
    assert_eq!(r.t_of(id(2, 0)), Some(&set(&[id(0, 4)])));
    assert_eq!(r.tparents_of(id(2, 1)), Some(&set(&[id(0, 4)])));
    assert_eq!(r.tparents_of(id(1, 0)), Some(&set(&[id(0, 4)])));
    assert!(r.concurrent(id(1, 0), id(2, 1)));
    s.restart(1).unwrap();
    // a new event whose parents are all compacted: T from the persisted column only
    let e = s.mv(1, C, B, 0).unwrap();
    assert_eq!(s.reps[&1].tparents_of(e), Some(&set(&[id(1, 0), id(2, 1)])));
    assert_eq!(s.status_in(1, B), VIS);
    assert_eq!(s.status_in(1, C), VIS);
    assert_eq!(s.vparent_in(1, C), B);
}

#[test]
fn a_move_below_the_truncated_log_is_refused() {
    let seeds = seed_events(&[A, B]);
    let mut r = Replica::new();
    r.deliver_all(&seeds).unwrap();
    let mut other = r.clone();
    let late = other.move_as(2, n(A), n(B), 0).unwrap();
    r.move_as(1, n(B), n(A), 0).unwrap();
    let mine = r.move_as(1, n(A), n(TRASH), 0).unwrap();
    // compacting at a version that is not stable (it lacks `late`) is the caller's error
    r.compact(&set(&[mine.id])).unwrap();
    assert_eq!(r.stability_point(), Some(mine.ts()));
    assert_eq!(r.deliver(&late), Err(MoveError::BelowStability));
}

#[test]
fn local_ops_refuse_bad_moves() {
    let mut r = Replica::new();
    r.deliver_all(&seed_events(&[A, B])).unwrap();
    assert_eq!(r.move_as(1, n(ROOT), n(A), 0), Err(MoveError::BadMove));
    assert_eq!(r.move_as(1, n(A), n(A), 0), Err(MoveError::BadMove));
    assert_eq!(r.move_as(1, n(9), n(A), 0), Err(MoveError::UnknownNode(n(9))));
    assert_eq!(r.move_as(1, n(A), n(B), 1), Err(MoveError::OutOfRange));
    assert_eq!(r.create_as(1, n(A), n(ROOT), 0), Err(MoveError::NodeExists(n(A))));
    let e = r.create_as(1, n(7), n(B), 0).unwrap();
    assert_eq!(r.parent_of(n(7)), Some((n(B), e.id)));
    assert_eq!(r.children(n(B)), vec![n(7)]);
    assert_eq!(r.deliver(&e), Ok(false));
    let mut bad = e.clone();
    bad.lamport += 1;
    assert_eq!(r.deliver(&bad), Err(MoveError::Conflict));
}

// ---------------------------------------------------------------------------------------
// Property tests

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() {
            None
        } else {
            Some(v[self.below(v.len())])
        }
    }
}

fn seeds(default: u64) -> u64 {
    std::env::var("MPCE_PROP_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

const AGENTS: [u64; 3] = [1, 2, 3];
const MAX_NODE: u64 = 7;

fn deliverable(s: &Sim, a: u64) -> Vec<Id> {
    let r = &s.reps[&a];
    s.events.values().filter(|e| !r.has(e.id) && e.parents.iter().all(|p| r.has(*p))).map(|e| e.id).collect()
}

fn random_step(s: &mut Sim, rng: &mut Rng, next_node: &mut u64) -> Result<(), String> {
    let a = AGENTS[rng.below(3)];
    let r = s.reps[&a].clone();
    let nodes: Vec<u64> = r.nodes().into_iter().map(|x| x.0).collect();
    let deleted: Vec<u64> = nodes.iter().copied().filter(|&x| r.parent_of(n(x)).is_some_and(|(p, _)| p.0 == TRASH)).collect();
    let live: Vec<u64> = nodes.iter().copied().filter(|x| !deleted.contains(x)).collect();
    let index = |rng: &mut Rng, p: u64| rng.below(r.children(n(p)).len() + 1);
    let k = rng.below(100);
    match k {
        // any move, including under TRASH (also a re-delete inside TRASH) and cycles
        0..=21 => {
            let c = rng.pick(&nodes).unwrap();
            let ps: Vec<u64> = [ROOT, TRASH].into_iter().chain(nodes.iter().copied()).filter(|&p| p != c).collect();
            let p = rng.pick(&ps).unwrap();
            let i = index(rng, p);
            s.mv(a, c, p, i)?;
        }
        22..=27 if *next_node <= MAX_NODE => {
            let ps: Vec<u64> = std::iter::once(ROOT).chain(nodes.iter().copied()).collect();
            let p = rng.pick(&ps).unwrap();
            let i = index(rng, p);
            let c = *next_node;
            *next_node += 1;
            s.create(a, c, p, i)?;
        }
        28..=37 => {
            if let Some(c) = rng.pick(&live) {
                s.mv(a, c, TRASH, index(rng, TRASH))?;
            }
        }
        38..=44 => {
            if let Some(c) = rng.pick(&deleted) {
                let ps: Vec<u64> = std::iter::once(ROOT).chain(nodes.iter().copied()).filter(|&p| p != c).collect();
                let p = rng.pick(&ps).unwrap();
                s.mv(a, c, p, index(rng, p))?;
            }
        }
        45..=50 => {
            s.other(a)?;
        }
        // one event, in a random causal order
        51..=72 => {
            if let Some(x) = rng.pick(&deliverable(s, a)) {
                s.deliver(a, x)?;
            }
        }
        // a partial sync: a prefix (in timestamp order) of what f has that a lacks
        73..=83 => {
            let f = AGENTS[rng.below(3)];
            let todo = s.missing(&s.reps[&f].clone(), a);
            if !todo.is_empty() {
                let k = 1 + rng.below(todo.len());
                for &x in &todo[..k] {
                    s.deliver_quiet(a, x)?;
                }
                s.after()?;
            }
        }
        // compaction at a random critical point (the closure of a random critical event)
        84..=93 => {
            let crit: Vec<Id> = s.critical().into_iter().filter(|x| r.has(*x)).collect();
            if let Some(x) = rng.pick(&crit) {
                let mut cv = ancestors(&s.graph, x);
                cv.insert(x);
                s.compact_at(a, &cv)?;
            }
        }
        _ => s.restart(a)?,
    }
    Ok(())
}

/// a readable dump of a replica (MPCE_DEBUG=1)
fn dump(r: &Replica) -> String {
    let mut out = String::new();
    let mut evs: Vec<Id> = r.received().into_iter().collect();
    evs.sort_by_key(|x| r.ts_of(*x));
    for x in evs {
        let e = r.event(x);
        let ps = e.as_ref().map(|e| e.parents.clone());
        match r.op(x) {
            Some(mpce_model::tree_move::TreeOp::Move { child, parent, .. }) => {
                out += &format!("{x:?} ts {:?} move {child:?} -> {parent:?} parents {ps:?} tparents {:?}\n", r.ts_of(x).unwrap(), r.tparents_of(x))
            }
            _ => out += &format!("{x:?} ts {:?} other parents {ps:?} T {:?}\n", r.ts_of(x).unwrap(), r.t_of(x)),
        }
    }
    let (seq, _) = r.seq_log();
    for (lm, sk) in &seq {
        out += &format!("seq {:?} {:?}: {:?} -> {:?} (old {:?}){}\n", lm.time, lm.meta, lm.child, lm.new_parent, lm.old_parent, if *sk { " SKIP" } else { "" });
    }
    out += &format!("tree {:?}\n", r.tree());
    let p = r.projection();
    for (n, v) in &p.nodes {
        out += &format!("{n:?}: {v:?}\n");
    }
    out += &format!("children {:?}\n", p.children);
    out
}

fn run_prop_seed(seed: u64, v: Variant) -> Result<Stats, String> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    // odd seeds start with four nodes (the mutual-rescue gap needs four)
    let seeded: &[u64] = if seed % 2 == 1 { &[A, B, C, D] } else { &[A, B, C] };
    let mut s = Sim::new(v, &AGENTS, seeded);
    let mut next_node = seeded.len() as u64 + 2;
    let steps = 30 + rng.below(25);
    for step in 0..steps {
        if let Err(e) = random_step(&mut s, &mut rng, &mut next_node) {
            if std::env::var("MPCE_DEBUG").is_ok() {
                eprintln!("{}", dump(&s.ghost));
            }
            return Err(format!("seed {seed} step {step}: {e}"));
        }
    }
    // full sync in random causal orders, then compact everything stable and restart all
    for &a in &AGENTS {
        loop {
            let d = deliverable(&s, a);
            match rng.pick(&d) {
                Some(x) => s.deliver_quiet(a, x)?,
                None => break,
            }
        }
    }
    s.after().map_err(|e| format!("seed {seed} after full sync: {e}"))?;
    let all = s.ghost.received();
    for (a, r) in &s.reps {
        if r.received() != all {
            return Err(format!("seed {seed}: replica {a} incomplete after full sync"));
        }
    }
    for &a in &AGENTS {
        s.compact(a).map_err(|e| format!("seed {seed} final compact: {e}"))?;
        s.restart(a).map_err(|e| format!("seed {seed} final restart: {e}"))?;
    }
    let p = s.ghost.projection();
    s.stats.rescues = p.rescues().len();
    s.stats.nodes = s.ghost.nodes().len();
    s.stats.rescued_under_node = p.nodes.values().filter(|v| v.rescue.is_some() && v.parent != n(ROOT)).count();
    s.stats.skips = s.ghost.reports().iter().filter(|r| matches!(r, ConflictReport::CycleSkip { .. })).count();
    for r in s.reps.values() {
        if r.projection() != p {
            return Err(format!("seed {seed}: projections differ after full sync"));
        }
    }
    Ok(s.stats)
}

#[test]
#[ignore]
fn debug_one_seed() {
    let seed: u64 = std::env::var("MPCE_DEBUG_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    run_prop_seed(seed, Variant::Correct).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn property_tree_converges_and_projection_survives_compaction() {
    let n = seeds(100);
    let mut tot = Stats::default();
    let mut with_rescue = 0;
    let mut with_skip = 0;
    let mut four_plus = 0;
    let mut under_node = 0;
    let mut by_nodes: BTreeMap<usize, usize> = BTreeMap::new();
    for seed in 0..n {
        let st = run_prop_seed(seed, Variant::Correct).unwrap_or_else(|e| panic!("{e}"));
        tot.compactions += st.compactions;
        tot.over_compacted += st.over_compacted;
        tot.restarts += st.restarts;
        tot.creates += st.creates;
        with_rescue += usize::from(st.rescues > 0);
        with_skip += usize::from(st.skips > 0);
        four_plus += usize::from(st.nodes >= 4);
        under_node += usize::from(st.rescued_under_node > 0);
        *by_nodes.entry(st.nodes).or_default() += 1;
    }
    println!(
        "property: {n} seeds (nodes at the end: {by_nodes:?}; with 4+ nodes {four_plus}); seeds ending with a rescue {with_rescue} (a rescue root under a node, not ROOT: {under_node}), with a cycle-skip {with_skip}; compactions {}, events integrated over compacted parents {}, restarts {}, creates {}; projection_acyclic checked after every step",
        tot.compactions, tot.over_compacted, tot.restarts, tot.creates
    );
    if n >= 20 {
        assert!(with_rescue > 0 && with_skip > 0 && tot.compactions > 0 && tot.over_compacted > 0 && tot.restarts > 0 && tot.creates > 0 && four_plus > 0);
    }
}

// ---------------------------------------------------------------------------------------
// Mutants: each must fail a fixture or the property test

fn fails<T>(f: impl FnOnce() -> Result<T, String>) -> bool {
    !matches!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)), Ok(Ok(_)))
}

#[test]
fn every_mutant_fails_a_fixture_or_a_property() {
    let budget: u64 = std::env::var("MPCE_MUTANT_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    let mut uncaught = Vec::new();
    for v in Variant::MUTANTS {
        let failed: Vec<&str> = FIXTURES.iter().filter(|(_, f)| fails(|| f(v))).map(|(name, _)| *name).collect();
        let first_seed = (0..budget).find(|&seed| fails(|| run_prop_seed(seed, v)));
        println!("mutant {:<26} fixtures failed {:>2}/{} {:?}; property: first failing seed {:?} (of {budget})", v.quint_name(), failed.len(), FIXTURES.len(), failed, first_seed);
        if failed.is_empty() && first_seed.is_none() {
            uncaught.push(v);
        }
    }
    assert!(uncaught.is_empty(), "mutants not caught: {uncaught:?}");
}
