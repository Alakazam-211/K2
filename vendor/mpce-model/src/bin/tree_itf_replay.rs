//! Replay ITF traces of `spec/quint/tree.qnt` (exported through the wrapper
//! `tools/refmodel/itf_tree.qnt`) on the tree reference model (`mpce_model::tree_move`).
//!
//! `tree_itf_replay [--variant NAME] [--same-spot preorder|node-id] [--inv] [--expect-mismatch]
//! <trace.itf.json | dir>...`
//!
//! Same-spot rescues are ordered by the rebuilt pre-delete tree's pre-order (`preorder`, the
//! default, as the Quint model does); `node-id` orders them by node ID (differs from Quint only
//! on same-spot groups).
//!
//! The seed is rebuilt with `seed_events` and checked against the trace's seed ops. Each step
//! is classified by its state diff:
//! - a new op x of agent a: a move is made with `move_as` at the visible index just after the
//!   trace op's left origin among the parent's live elements; the reference event must have
//!   the same ID, Lamport clock, left and right origins and parents, and the replica's T and
//!   tparents for it must equal the trace's. A non-tree event is made with `other_as` (ID,
//!   Lamport, parents, T). The event goes to the ghost too.
//! - one replica gains one event: it is delivered there;
//! - one replica's compaction set grows: `compact` with the trace's set.
//!
//! After every step, for every replica and the ghost: received set, frontier, tree, resident
//! log (incl. each entry's skip status, from the replay of the op set) and compaction set
//! equal the trace's. The wrapper's variable `pv` in state k holds, for every replica (key 0:
//! the ghost), the projection of state k-1 as the Quint model computes it from the full graph
//! (`projFull`: status, visible parent, anchor, hidden reasons, visible order of every
//! parent, rescues), the cycle-skip reports, every parent's FugueMax list and core children;
//! the reference model's projection of state k-1 through the skeleton AND through the full
//! graph must both equal it. `--inv` also runs `check_replica` (with the full graph) on every
//! replica after every step. Exit 0: all traces replayed with no mismatch (or, with
//! --expect-mismatch, at least one mismatch).
#![forbid(unsafe_code)]

use mpce_model::text::Id;
use mpce_model::tree::Origin;
use mpce_model::tree_move::{check_replica, seed_events, Causality, ConflictReport, Graph, NodeId, Replica, SameSpotOrder, Status, TreeEvent, TreeOp, Variant};
use std::collections::{BTreeMap, BTreeSet};

#[path = "shared/itf_json.rs"]
mod itf_json;
use itf_json::{parse, J};

const START: (i64, i64) = (-1, 0);
const END: (i64, i64) = (-2, 0);
const NOID: (i64, i64) = (-3, 0);

// ---------------------------------------------------------------------------------------
// The trace's view of a state

#[derive(Clone, Debug, PartialEq)]
struct TOp {
    is_move: bool,
    ts: (i64, i64),
    p: u64,
    c: u64,
    left: (i64, i64),
    right: (i64, i64),
    parents: BTreeSet<Id>,
    t: BTreeSet<Id>,
    tparents: BTreeSet<Id>,
}

/// a LogMove: (t, old parent, new parent, meta, child, skipped)
type TLog = ((i64, i64), Option<(u64, Id)>, u64, Id, u64, bool);

#[derive(Clone, Debug, PartialEq)]
struct TRep {
    rcv: BTreeSet<Id>,
    front: BTreeSet<Id>,
    log: Vec<TLog>,
    tree: BTreeSet<(u64, Id, u64)>,
    comp: BTreeSet<Id>,
}

/// the projection and the derived views of one replica
#[derive(Clone, Debug, PartialEq, Default)]
struct TProj {
    status: BTreeMap<u64, String>,
    vparent: BTreeMap<u64, u64>,
    anchor: BTreeMap<u64, (i64, i64)>,
    reasons: BTreeMap<u64, BTreeSet<Id>>,
    order: BTreeMap<u64, Vec<u64>>,
    rescues: BTreeSet<(u64, Id)>,
    skips: BTreeSet<(u64, u64, Id)>,
    lists: BTreeMap<u64, Vec<Id>>,
    core: BTreeMap<u64, Vec<u64>>,
}

#[derive(Clone, Debug)]
struct TState {
    ops: BTreeMap<Id, TOp>,
    reps: BTreeMap<u64, TRep>,
    ghost: TRep,
    pv: BTreeMap<u64, TProj>,
}

fn tup(j: &J) -> (i64, i64) {
    let v = j.items("#tup");
    (v[0].int(), v[1].int())
}
fn id(j: &J) -> Id {
    let (a, s) = tup(j);
    Id::new(a as u64, s as u64)
}
fn idset(j: &J) -> BTreeSet<Id> {
    j.items("#set").iter().map(id).collect()
}
fn f<'a>(j: &'a J, k: &str) -> &'a J {
    j.get(k).unwrap_or_else(|| panic!("field {k} missing"))
}
fn nat(j: &J) -> u64 {
    j.int() as u64
}
fn map<'a>(j: &'a J) -> impl Iterator<Item = (&'a J, &'a J)> {
    j.items("#map").iter().map(|kv| {
        let kv = kv.list();
        (&kv[0], &kv[1])
    })
}

fn rep(j: &J) -> TRep {
    let log = f(j, "log")
        .list()
        .iter()
        .map(|l| {
            let old = if f(l, "oldHas").bool() { Some((nat(f(l, "oldP")), id(f(l, "oldM")))) } else { None };
            (tup(f(l, "t")), old, nat(f(l, "p")), id(f(l, "m")), nat(f(l, "c")), f(l, "skipped").bool())
        })
        .collect();
    let tree = f(j, "tree").items("#set").iter().map(|e| (nat(f(e, "p")), id(f(e, "m")), nat(f(e, "c")))).collect();
    TRep { rcv: idset(f(j, "rcv")), front: idset(f(j, "front")), log, tree, comp: idset(f(j, "comp")) }
}

fn proj(j: &J) -> TProj {
    TProj {
        status: map(f(j, "status")).map(|(k, v)| (nat(k), v.str().to_string())).collect(),
        vparent: map(f(j, "vparent")).map(|(k, v)| (nat(k), nat(v))).collect(),
        anchor: map(f(j, "anchor")).map(|(k, v)| (nat(k), tup(v))).collect(),
        reasons: map(f(j, "reasons")).map(|(k, v)| (nat(k), idset(v))).collect(),
        order: map(f(j, "order")).map(|(k, v)| (nat(k), v.list().iter().map(nat).collect())).collect(),
        rescues: f(j, "rescues")
            .items("#set")
            .iter()
            .map(|t| {
                let v = t.items("#tup");
                (nat(&v[0]), id(&v[1]))
            })
            .collect(),
        skips: f(j, "skips")
            .items("#set")
            .iter()
            .map(|t| {
                let v = t.items("#tup");
                (nat(&v[0]), nat(&v[1]), id(&v[2]))
            })
            .collect(),
        lists: map(f(j, "lists")).map(|(k, v)| (nat(k), v.list().iter().map(id).collect())).collect(),
        core: map(f(j, "core")).map(|(k, v)| (nat(k), v.list().iter().map(nat).collect())).collect(),
    }
}

fn tstate(j: &J) -> TState {
    let mut ops = BTreeMap::new();
    for (k, o) in map(j.var("ops").expect("ops")) {
        ops.insert(
            id(k),
            TOp {
                is_move: f(o, "kind").str() == "move",
                ts: tup(f(o, "ts")),
                p: f(o, "p").int().max(0) as u64,
                c: f(o, "c").int().max(0) as u64,
                left: tup(f(o, "left")),
                right: tup(f(o, "right")),
                parents: idset(f(o, "parents")),
                t: idset(f(o, "T")),
                tparents: idset(f(o, "tparents")),
            },
        );
    }
    let reps = map(j.var("replicas").expect("replicas")).map(|(k, v)| (nat(k), rep(v))).collect();
    let pv = j.var("pv").map(|p| map(p).map(|(k, v)| (nat(k), proj(v))).collect()).unwrap_or_default();
    TState { ops, reps, ghost: rep(j.var("ghost").expect("ghost")), pv }
}

fn origin(o: Origin<Id>) -> (i64, i64) {
    match o {
        Origin::Start => START,
        Origin::End => END,
        Origin::Elem(x) => (x.agent as i64, x.seq as i64),
    }
}

fn idt(x: Id) -> (i64, i64) {
    (x.agent as i64, x.seq as i64)
}

// ---------------------------------------------------------------------------------------
// The reference model's view

fn my_rep(r: &Replica) -> TRep {
    let (seq, _) = r.seq_log();
    let skipped: BTreeMap<Id, bool> = seq.iter().map(|(lm, s)| (lm.meta, *s)).collect();
    TRep {
        rcv: r.received(),
        front: r.frontier().clone(),
        log: r
            .log()
            .iter()
            .map(|lm| {
                (
                    (lm.time.lamport as i64, lm.time.agent as i64),
                    lm.old_parent.map(|(p, m)| (p.0, m)),
                    lm.new_parent.0,
                    lm.meta,
                    lm.child.0,
                    skipped.get(&lm.meta).copied().unwrap_or(false),
                )
            })
            .collect(),
        tree: r.tree().iter().map(|(p, m, c)| (p.0, *m, c.0)).collect(),
        comp: r.compacted().clone(),
    }
}

fn my_proj(r: &Replica, c: Causality) -> TProj {
    let p = r.projection_with(c);
    let nodes = r.nodes();
    let alln: Vec<NodeId> = [NodeId::ROOT, NodeId::TRASH].into_iter().chain(nodes.iter().copied()).collect();
    let mut t = TProj::default();
    for (n, v) in &p.nodes {
        let s = match v.status {
            Status::Visible => "visible",
            Status::Deleted => "deleted",
            Status::Hidden => "hidden",
            Status::Stuck => "stuck",
        };
        t.status.insert(n.0, s.to_string());
        t.vparent.insert(n.0, v.parent.0);
        t.anchor.insert(n.0, v.rescue.as_ref().and_then(|x| x.anchor).map_or(NOID, idt));
        t.reasons.insert(n.0, v.reasons.clone());
    }
    for &q in &alln {
        t.order.insert(q.0, p.children(q).iter().map(|x| x.0).collect());
        t.lists.insert(q.0, r.list(q));
        t.core.insert(q.0, r.children(q).iter().map(|x| x.0).collect());
    }
    t.rescues = p.rescues().into_iter().map(|(n, d)| (n.0, d)).collect();
    t.skips = r.reports_of(&p).into_iter().filter_map(|x| if let ConflictReport::CycleSkip { mover, node, op } = x { Some((mover, node.0, op)) } else { None }).collect();
    t
}

fn diff_proj(want: &TProj, got: &TProj) -> String {
    let mut out = Vec::new();
    macro_rules! field {
        ($f:ident) => {
            if want.$f != got.$f {
                out.push(format!("{}: quint {:?}, model {:?}", stringify!($f), want.$f, got.$f));
            }
        };
    }
    field!(status);
    field!(vparent);
    field!(anchor);
    field!(reasons);
    field!(order);
    field!(rescues);
    field!(skips);
    field!(lists);
    field!(core);
    out.join("; ")
}

#[derive(Default)]
struct Stats {
    steps: usize,
    moves: usize,
    others: usize,
    deliveries: usize,
    compactions: usize,
    proj_checks: usize,
    states_with_rescue: usize,
    states_with_skip: usize,
    states_with_hidden: usize,
    same_spot_groups: usize,
    traces_with_rescue: usize,
    traces_with_compaction: usize,
}

/// groups of rescue roots sharing (visible parent, anchor or no spot) with another root
fn same_spot(r: &Replica) -> usize {
    let p = r.projection();
    let mut g: BTreeMap<(NodeId, Option<Id>), usize> = BTreeMap::new();
    for v in p.nodes.values() {
        if let Some(rs) = &v.rescue {
            *g.entry((v.parent, rs.anchor)).or_default() += 1;
        }
    }
    g.values().filter(|&&k| k > 1).count()
}

fn replay(states: &[TState], variant: Variant, spot: SameSpotOrder, inv: bool, st: &mut Stats) -> Result<(), String> {
    let s0 = &states[0];
    let nodes: Vec<u64> = s0.ops.iter().filter(|(x, _)| x.agent == 0).map(|(_, o)| o.c).collect();
    let seeds = seed_events(&nodes);
    for e in &seeds {
        let o = s0.ops.get(&e.id).ok_or(format!("seed {:?} not in the trace", e.id))?;
        let TreeOp::Move { child, parent, pos } = e.op else { unreachable!() };
        if (o.ts, o.c, o.p, o.left, o.right, &o.parents) != ((e.lamport as i64, 0), child.0, parent.0, origin(pos.left), origin(pos.right), &e.parents) {
            return Err(format!("seed {:?}: model {e:?}, trace {o:?}", e.id));
        }
    }
    let mut r0 = Replica::with_variant(variant);
    r0.set_same_spot_order(spot);
    for e in &seeds {
        r0.deliver(e).map_err(|e| format!("seed: {e:?}"))?;
    }
    let mut reps: BTreeMap<u64, Replica> = s0.reps.keys().map(|&a| (a, r0.clone())).collect();
    reps.insert(0, r0); // key 0: the ghost
    let mut events: BTreeMap<Id, TreeEvent> = seeds.iter().map(|e| (e.id, e.clone())).collect();
    let trep = |s: &TState, a: u64| -> TRep { if a == 0 { s.ghost.clone() } else { s.reps[&a].clone() } };
    let graph_of = |s: &TState| -> Graph { s.ops.iter().map(|(x, o)| (*x, o.parents.clone())).collect() };
    let snap = |reps: &BTreeMap<u64, Replica>, g: &Graph| -> Result<BTreeMap<u64, TProj>, String> {
        let mut out = BTreeMap::new();
        for (a, r) in reps {
            let ps = my_proj(r, Causality::Skeleton);
            let pg = my_proj(r, Causality::Graph(g));
            if ps != pg {
                return Err(format!("replica {a}: skeleton and graph projections differ: {}", diff_proj(&pg, &ps)));
            }
            out.insert(*a, ps);
        }
        Ok(out)
    };
    let compare = |k: usize, s: &TState, reps: &BTreeMap<u64, Replica>| -> Result<(), String> {
        for (a, r) in reps {
            let (want, got) = (trep(s, *a), my_rep(r));
            if want != got {
                return Err(format!("step {k}: replica {a}: trace {want:?}\nmodel {got:?}"));
            }
        }
        Ok(())
    };
    compare(0, s0, &reps)?;
    let mut prev = snap(&reps, &graph_of(s0))?;
    let (mut any_rescue, mut any_comp) = (false, false);
    for (k0, w) in states.windows(2).enumerate() {
        let (p, s) = (&w[0], &w[1]);
        let k = k0 + 1;
        if !s.pv.is_empty() {
            for (a, want) in &s.pv {
                let got = prev.get(a).ok_or(format!("step {k}: no replica {a}"))?;
                if want != got {
                    return Err(format!("step {k}: projection of state {} at replica {a}: {}", k - 1, diff_proj(want, got)));
                }
                st.proj_checks += 1;
            }
        }
        let new: Vec<Id> = s.ops.keys().filter(|x| !p.ops.contains_key(x)).copied().collect();
        let changed: Vec<u64> = s.reps.keys().filter(|a| s.reps[a] != p.reps[a]).copied().collect();
        if new.len() > 1 {
            return Err(format!("step {k}: {} new ops", new.len()));
        }
        if let Some(&x) = new.first() {
            let o = &s.ops[&x];
            let a = x.agent;
            let r = reps.get_mut(&a).ok_or(format!("step {k}: unknown agent {a}"))?;
            let e = if o.is_move {
                st.moves += 1;
                let live = r.live_elements(NodeId(o.p));
                let index = if o.left == START {
                    0
                } else {
                    live.iter().position(|y| idt(*y) == o.left).ok_or(format!("step {k}: left origin {:?} not live", o.left))? + 1
                };
                let e = r.move_as(a, NodeId(o.c), NodeId(o.p), index).map_err(|e| format!("step {k}: move_as: {e:?}"))?;
                let TreeOp::Move { pos, .. } = e.op else { unreachable!() };
                if (origin(pos.left), origin(pos.right)) != (o.left, o.right) {
                    return Err(format!("step {k}: move {x:?}: model origins ({:?}, {:?}), trace ({:?}, {:?})", origin(pos.left), origin(pos.right), o.left, o.right));
                }
                if r.tparents_of(x) != Some(&o.tparents) {
                    return Err(format!("step {k}: tparents of {x:?}: model {:?}, trace {:?}", r.tparents_of(x), o.tparents));
                }
                e
            } else {
                st.others += 1;
                r.other_as(a).map_err(|e| format!("step {k}: other_as: {e:?}"))?
            };
            if e.id != x || e.lamport as i64 != o.ts.0 || e.parents != o.parents || r.t_of(x) != Some(&o.t) {
                return Err(format!("step {k}: event {x:?}: model {e:?} T {:?}, trace {o:?}", r.t_of(x)));
            }
            reps.get_mut(&0).unwrap().deliver(&e).map_err(|e| format!("step {k}: ghost: {e:?}"))?;
            events.insert(x, e);
        } else if changed.len() == 1 {
            let a = changed[0];
            let (pr, sr) = (&p.reps[&a], &s.reps[&a]);
            let gained: Vec<Id> = sr.rcv.difference(&pr.rcv).copied().collect();
            let r = reps.get_mut(&a).unwrap();
            if gained.len() == 1 {
                st.deliveries += 1;
                match r.deliver(&events[&gained[0]]) {
                    Ok(true) => {}
                    other => return Err(format!("step {k}: deliver {:?} to {a}: {other:?}", gained[0])),
                }
                let ts = &s.ops[&gained[0]];
                if ts.is_move && r.tparents_of(gained[0]) != Some(&ts.tparents) {
                    return Err(format!("step {k}: tparents of {:?} at {a}: model {:?}, trace {:?}", gained[0], r.tparents_of(gained[0]), ts.tparents));
                }
            } else if gained.is_empty() && sr.comp != pr.comp {
                st.compactions += 1;
                any_comp = true;
                r.compact(&sr.comp).map_err(|e| format!("step {k}: compact: {e:?}"))?;
            } else {
                return Err(format!("step {k}: replica {a} changed in an unknown way"));
            }
        } else {
            return Err(format!("step {k}: {} replicas changed without a new op", changed.len()));
        }
        compare(k, s, &reps)?;
        let g = graph_of(s);
        if inv {
            for (a, r) in &reps {
                check_replica(r, Some(&g)).map_err(|e| format!("step {k}: replica {a}: {e}"))?;
            }
        }
        prev = snap(&reps, &g)?;
        let gh = &prev[&0];
        if !gh.rescues.is_empty() {
            st.states_with_rescue += 1;
            any_rescue = true;
        }
        st.states_with_skip += usize::from(!gh.skips.is_empty());
        st.states_with_hidden += usize::from(gh.status.values().any(|v| v == "hidden"));
        st.same_spot_groups += reps.values().map(same_spot).sum::<usize>();
        st.steps += 1;
    }
    st.traces_with_rescue += usize::from(any_rescue);
    st.traces_with_compaction += usize::from(any_comp);
    Ok(())
}

fn main() {
    let mut variant = Variant::Correct;
    let mut spot = SameSpotOrder::PreDeletePreorder;
    let mut inv = false;
    let mut expect_mismatch = false;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--variant" => {
                let v = args.next().expect("--variant NAME");
                variant = Variant::from_quint(&v).unwrap_or_else(|| panic!("unknown variant {v}"));
            }
            "--same-spot" => {
                spot = match args.next().as_deref() {
                    Some("node-id") => SameSpotOrder::NodeId,
                    Some("preorder") => SameSpotOrder::PreDeletePreorder,
                    _ => panic!("--same-spot preorder|node-id"),
                }
            }
            "--inv" => inv = true,
            "--expect-mismatch" => expect_mismatch = true,
            _ => paths.push(a),
        }
    }
    let mut files = Vec::new();
    for p in paths {
        let md = std::fs::metadata(&p).expect("path");
        if md.is_dir() {
            let mut v: Vec<_> = std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().ends_with(".itf.json")).collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(p.into());
        }
    }
    let (mut ok, mut bad) = (0usize, 0usize);
    let mut st = Stats::default();
    let mut first: Option<String> = None;
    for f in &files {
        let txt = std::fs::read_to_string(f).expect("read");
        let j = parse(&txt).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let states: Vec<TState> = j.get("states").expect("states").list().iter().map(tstate).collect();
        match replay(&states, variant, spot, inv, &mut st) {
            Ok(()) => ok += 1,
            Err(e) => {
                bad += 1;
                if first.is_none() {
                    first = Some(format!("{}: {e}", f.display()));
                }
            }
        }
    }
    if let Some(m) = &first {
        let m: String = m.chars().take(4000).collect();
        println!("first mismatch: {m}");
    }
    println!(
        "SUMMARY tree_itf_replay variant={} same_spot={:?} traces={} matched={} mismatched={} steps={} moves={} others={} deliveries={} compactions={} projection_checks={} states_with_rescue={} states_with_skip={} states_with_hidden={} same_spot_groups={} traces_with_rescue={} traces_with_compaction={}",
        variant.quint_name(),
        spot,
        files.len(),
        ok,
        bad,
        st.steps,
        st.moves,
        st.others,
        st.deliveries,
        st.compactions,
        st.proj_checks,
        st.states_with_rescue,
        st.states_with_skip,
        st.states_with_hidden,
        st.same_spot_groups,
        st.traces_with_rescue,
        st.traces_with_compaction
    );
    let pass = if expect_mismatch { bad > 0 } else { bad == 0 && !files.is_empty() };
    std::process::exit(if pass { 0 } else { 1 });
}
