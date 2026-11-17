//! Replay ITF traces (`quint run --out-itf`) of the Quint list, mlist, map and register models
//! on the reference models built from the two layers (`mpce_model::{list, mlist, map}` and
//! `mpce_reg`).
//!
//! `lmm_itf_replay --model list|mlist|map|reg [MUTANT...] [--gap N] [--digests PATH]
//!  [--expect-mismatch] <trace.itf.json | dir>...`
//!
//! `--gap N`: reg_model's GAP (an op's lamport is at most 1 + its past's maximum + N).
//! `--digests PATH`: append one digest per distinct trace (of its states) to PATH, so that
//! distinct traces can be counted across batches.
//!
//! Mutants (each must mismatch; they give the replay teeth): `--rule fugue` (the order layer's
//! sibling rule), `--read undel-lww|delete-loses-to-set|agent-tiebreak-reversed|
//! holder-covers-every-delete` (the register layer's read), `--variant register-reads-order`
//! (the composition), `--key-rule no-agent-tiebreak|delete-wins` (map).
//!
//! Each step of a trace is classified by its state diff, as `itf_replay` does for text:
//! - a new event in `ev`: its agent makes the same op on its reference replica, from the view
//!   (an index derived from the trace event's slots), and the reference event must equal the
//!   trace's in every field (ID, parents, lamport, kind, element/target/key, value, origins,
//!   `from`, `idx`; reg: parents, past, lamport, kind, value). An mlist op whose parents are not
//!   its replica's frontier is a branch-merge op (`branchFrom`): it is made on the replica's
//!   older version (the closure of its parents) and integrated into the current state;
//! - one replica gains one event: it is delivered to that reference replica (must be new);
//! - no change (re-delivery): every event is delivered again everywhere, which must change
//!   nothing.
//!
//! After every step, every replica and the ghost are compared on every state field the Quint
//! model has: list (`rcv`, `front`, `doc`, `reg`), mlist (`rcv`, `front`, `doc`, `pos`, `vreg`,
//! `ex`, `live`), map (`rcv`, `front`, `cells`, `nested`), reg (`ops`, `last`, the on-arrival
//! `inc`: `pos`, `vreg`, `maxE`, `exAnc` (drops included), `exl`, `dropw` (the highest drop),
//! `exh` (empty: only the existence-holder-by-arrival mutant keeps it), the history store
//! `hist` (every received op's content by ID), and `view`, the replica's read with the
//! existence holder: `winner(elem, existence)`; a dropped element's view shows no
//! content, value "", while the register keeps it), plus `cnt`, plus the visible list
//! (or map) the trace's state implies against the reference view.
//!
//! Prints one `LMM_REPLAY key=value ...` line. Exit 0: every trace replayed with no mismatch
//! (with --expect-mismatch: at least one mismatch). Exit 1 otherwise.
#![forbid(unsafe_code)]

use mpce_model::map::{self, KOp};
use mpce_model::mlist::{self, Config, Event, Intent, MOp, Variant};
use mpce_model::text::Id;
use mpce_model::tree::{Origin, SiblingRule};
use mpce_reg::key::KeyRule;
use mpce_reg::{Existence, Kind, Op as ROp, ReadRule, Reg};
use std::collections::{BTreeMap, BTreeSet};

#[path = "shared/itf_json.rs"]
mod itf_json;
use itf_json::{parse, J};

type T2 = (i64, i64);
const START: T2 = (-1, 0);
const END: T2 = (-2, 0);
const NOID: T2 = (-9, 0);
const TS0: T2 = (-1, -9);

fn tup(j: &J) -> T2 {
    let v = j.items("#tup");
    (v[0].int(), v[1].int())
}
fn id(j: &J) -> Id {
    let (a, s) = tup(j);
    Id::new(a as u64, s as u64)
}
fn t2(x: Id) -> T2 {
    (x.agent as i64, x.seq as i64)
}
fn idset(j: &J) -> BTreeSet<Id> {
    j.items("#set").iter().map(id).collect()
}
fn map_of(j: &J) -> Vec<(&J, &J)> {
    j.items("#map").iter().map(|kv| (&kv.list()[0], &kv.list()[1])).collect()
}
fn origin(o: Origin<Id>) -> T2 {
    match o {
        Origin::Start => START,
        Origin::End => END,
        Origin::Elem(x) => t2(x),
    }
}
fn ts(t: (u64, u64)) -> T2 {
    (t.0 as i64, t.1 as i64)
}
fn oid(x: T2) -> Id {
    Id::new(x.0 as u64, x.1 as u64)
}

macro_rules! ensure {
    ($c:expr, $($fmt:tt)*) => { if !$c { return Err(format!($($fmt)*)); } };
}

/// coverage counters
#[derive(Default)]
struct Cov {
    steps: usize,
    gen: usize,
    deliveries: usize,
    stutters: usize,
    branch: usize,
    kinds: BTreeMap<String, usize>,
    traces_with_hidden: usize,
    traces_with_undel: usize,
}

fn index_of(v: &[Id], x: Id, what: &str) -> Result<usize, String> {
    v.iter().position(|&y| y == x).ok_or(format!("{what} {x:?} not in {v:?}"))
}

// =========================================================================================
// mlist (and list, which is its insert/delete/set fragment)

#[derive(Clone, Debug, PartialEq)]
struct MEv {
    parents: BTreeSet<Id>,
    lam: i64,
    kind: String,
    elem: T2,
    value: String,
    left: T2,
    right: T2,
    from: T2,
    idx: i64,
}

#[derive(Clone, Debug, PartialEq)]
struct MRep {
    rcv: BTreeSet<Id>,
    front: BTreeSet<Id>,
    doc: Vec<Id>,
    /// mlist: elem -> (ts, slot); list: none
    pos: BTreeMap<Id, (T2, Id)>,
    /// mlist `vreg` / list `reg`: elem -> (ts, w, value)
    vreg: BTreeMap<Id, (T2, Id, String)>,
    ex: BTreeMap<Id, BTreeSet<Id>>,
    live: BTreeSet<Id>,
}

#[derive(Clone, Debug)]
struct MState {
    ev: BTreeMap<Id, MEv>,
    cnt: BTreeMap<u64, i64>,
    reps: BTreeMap<u64, MRep>,
    ghost: MRep,
}

fn mrep(j: &J, is_list: bool) -> MRep {
    let vkey = if is_list { "reg" } else { "vreg" };
    let mut r = MRep {
        rcv: idset(j.get("rcv").unwrap()),
        front: idset(j.get("front").unwrap()),
        doc: j.get("doc").unwrap().list().iter().map(id).collect(),
        pos: BTreeMap::new(),
        vreg: BTreeMap::new(),
        ex: BTreeMap::new(),
        live: BTreeSet::new(),
    };
    for (k, v) in map_of(j.get(vkey).unwrap()) {
        r.vreg.insert(id(k), (tup(v.get("ts").unwrap()), id(v.get("w").unwrap()), v.get("value").unwrap().str().to_string()));
    }
    if !is_list {
        for (k, v) in map_of(j.get("pos").unwrap()) {
            r.pos.insert(id(k), (tup(v.get("ts").unwrap()), id(v.get("slot").unwrap())));
        }
        for (k, v) in map_of(j.get("ex").unwrap()) {
            r.ex.insert(id(k), idset(v));
        }
        r.live = idset(j.get("live").unwrap());
    }
    r
}

fn mstate(j: &J, is_list: bool) -> MState {
    let mut ev = BTreeMap::new();
    for (k, e) in map_of(j.var("ev").expect("ev")) {
        let g = |f: &str| e.get(f).unwrap();
        ev.insert(
            id(k),
            MEv {
                parents: idset(g("parents")),
                lam: g("lam").int(),
                kind: g("kind").str().to_string(),
                elem: tup(g(if is_list { "target" } else { "elem" })),
                value: g("value").str().to_string(),
                left: tup(g("left")),
                right: tup(g("right")),
                from: if is_list { START } else { tup(g("from")) },
                idx: g("idx").int(),
            },
        );
    }
    let cnt = map_of(j.var("cnt").expect("cnt")).into_iter().map(|(k, v)| (k.int() as u64, v.int())).collect();
    let reps = map_of(j.var("replicas").expect("replicas")).into_iter().map(|(k, v)| (k.int() as u64, mrep(v, is_list))).collect();
    MState { ev, cnt, reps, ghost: mrep(j.var("ghost").expect("ghost"), is_list) }
}

/// the reference event as the trace would write it
fn mev_of(e: &Event, is_list: bool, from: T2) -> MEv {
    let (kind, elem, value, place, idx) = match &e.op {
        MOp::Insert { value, place } => ("ins", if is_list { START } else { t2(e.id) }, value.clone(), Some(place), -1),
        MOp::Move { elem, place, .. } => ("move", t2(*elem), String::new(), Some(place), -1),
        MOp::Set { elem, value, idx } => ("set", t2(*elem), value.clone(), None, idx.map_or(-1, |i| i as i64)),
        MOp::Delete { elem } => ("del", t2(*elem), String::new(), None, -1),
        MOp::Undel { elem } => ("undel", t2(*elem), String::new(), None, -1),
        MOp::Drop { elem } => ("drop", t2(*elem), String::new(), None, -1),
    };
    let from = match &e.op {
        MOp::Move { from, .. } => t2(*from),
        MOp::Insert { .. } => START,
        _ => from,
    };
    MEv {
        parents: e.parents.clone(),
        lam: e.lamport as i64,
        kind: kind.to_string(),
        elem,
        value,
        left: place.map_or(START, |p| origin(p.left)),
        right: place.map_or(END, |p| origin(p.right)),
        from: if is_list { START } else { from },
        idx,
    }
}

fn mcompare(step: usize, who: &str, d: &mlist::Doc, r: &MRep, ev: &BTreeMap<Id, MEv>, is_list: bool) -> Result<(), String> {
    ensure!(d.event_ids() == r.rcv, "step {step}: {who}: rcv: model {:?}, trace {:?}", d.event_ids(), r.rcv);
    ensure!(*d.frontier() == r.front, "step {step}: {who}: front: model {:?}, trace {:?}", d.frontier(), r.front);
    ensure!(d.slots() == r.doc, "step {step}: {who}: doc: model {:?}, trace {:?}", d.slots(), r.doc);
    let elems = d.elems();
    let keys: BTreeSet<Id> = r.vreg.keys().copied().collect();
    ensure!(elems == keys, "step {step}: {who}: elements: model {elems:?}, trace {keys:?}");
    for &e in &elems {
        let v = d.read(e).unwrap();
        let want = (ts(v.value_ts), v.value_write, v.value.clone());
        ensure!(r.vreg[&e] == want, "step {step}: {who}: value register of {e:?}: model {want:?}, trace {:?}", r.vreg[&e]);
        if !is_list {
            let p = (ts(v.pos_ts), v.pos);
            ensure!(r.pos.get(&e) == Some(&p), "step {step}: {who}: pos of {e:?}: model {p:?}, trace {:?}", r.pos.get(&e));
            let mx = d.max_existence(e);
            ensure!(r.ex.get(&e) == Some(&mx), "step {step}: {who}: ex of {e:?}: model {mx:?}, trace {:?}", r.ex.get(&e));
        }
    }
    if !is_list {
        let pk: BTreeSet<Id> = r.pos.keys().copied().collect();
        let xk: BTreeSet<Id> = r.ex.keys().copied().collect();
        ensure!(pk == elems && xk == elems, "step {step}: {who}: pos/ex keys {pk:?} {xk:?}, elements {elems:?}");
        ensure!(r.live.is_empty(), "step {step}: {who}: live (move-duplicates only) is {:?}", r.live);
    }
    // the visible list the trace's state implies
    let tv: Vec<Id> = if is_list {
        let dead: BTreeSet<Id> = r.rcv.iter().filter(|x| ev[x].kind == "del").map(|x| oid(ev[x].elem)).collect();
        r.doc.iter().copied().filter(|x| !dead.contains(x)).collect()
    } else {
        r.doc
            .iter()
            .copied()
            .filter(|s| {
                let e = oid(ev[s].elem);
                r.pos[&e].1 == *s && !r.ex[&e].iter().any(|m| ev[m].kind == "del")
            })
            .map(|s| oid(ev[&s].elem))
            .collect()
    };
    ensure!(d.visible() == tv, "step {step}: {who}: visible: model {:?}, trace {tv:?}", d.visible());
    Ok(())
}

fn mreplay(states: &[MState], config: Config, is_list: bool, cov: &mut Cov) -> Result<(), String> {
    let s0 = &states[0];
    // the seed (mlist): agent 0's inserts, made through the API
    let mut seed = mlist::Doc::with_config(config);
    let mut events: BTreeMap<Id, Event> = BTreeMap::new();
    let seeds: Vec<Id> = s0.ev.keys().copied().collect();
    for (k, x) in seeds.iter().enumerate() {
        let te = &s0.ev[x];
        let e = seed.insert(0, k, &te.value).map_err(|e| format!("seed {x:?}: {e:?}"))?;
        let me = mev_of(&e, is_list, START);
        ensure!(e.id == *x && me == *te, "seed {x:?}: model {:?} {me:?}, trace {te:?}", e.id);
        events.insert(*x, e);
    }
    let mut docs: BTreeMap<u64, mlist::Doc> = s0.reps.keys().map(|&a| (a, seed.clone())).collect();
    let mut ghost = seed.clone();
    let check_all = |step: usize, s: &MState, docs: &BTreeMap<u64, mlist::Doc>, ghost: &mlist::Doc, events: &BTreeMap<Id, Event>| -> Result<(), String> {
        for (a, r) in &s.reps {
            mcompare(step, &format!("replica {a}"), &docs[a], r, &s.ev, is_list)?;
        }
        mcompare(step, "ghost", ghost, &s.ghost, &s.ev, is_list)?;
        for (a, c) in &s.cnt {
            let n = events.keys().filter(|x| x.agent == *a).count() as i64;
            ensure!(*c == n, "step {step}: cnt of {a}: trace {c}, model {n}");
        }
        Ok(())
    };
    check_all(0, s0, &docs, &ghost, &events)?;
    let (mut hidden_seen, mut undel_seen) = (false, false);
    for (k, w) in states.windows(2).enumerate() {
        let (p, s) = (&w[0], &w[1]);
        let step = k + 1;
        cov.steps += 1;
        let new: Vec<Id> = s.ev.keys().filter(|x| !p.ev.contains_key(x)).copied().collect();
        let changed: Vec<u64> = s.reps.keys().filter(|a| s.reps[a] != p.reps[a]).copied().collect();
        ensure!(new.len() <= 1, "step {step}: {} new events", new.len());
        if let Some(&x) = new.first() {
            let te = s.ev[&x].clone();
            let a = x.agent;
            let d = docs.get_mut(&a).ok_or(format!("step {step}: unknown agent {a}"))?;
            let branch = te.parents != p.reps[&a].front;
            let from = oid(te.from);
            let elem = oid(te.elem);
            // `from`: the slot the op was made at, from the model's own view
            let mut mfrom = START;
            let e = if branch {
                ensure!(!is_list && (te.kind == "del" || te.kind == "undel"), "step {step}: a {} event whose parents are not its replica's frontier", te.kind);
                cov.branch += 1;
                let old = d.fork_at(&te.parents).map_err(|e| format!("step {step}: fork: {e:?}"))?;
                let (pool, intent) = if te.kind == "del" {
                    let vs = old.visible_slots();
                    (vs.clone(), Intent::Delete { index: index_of(&vs, from, "branch delete slot")? })
                } else {
                    let hs = old.hidden_slots();
                    (hs.clone(), Intent::Undel { index: index_of(&hs, from, "branch undel slot")? })
                };
                if let Intent::Delete { index } | Intent::Undel { index } = intent {
                    mfrom = t2(pool[index]);
                }
                d.branch(a, &te.parents, intent, None).map_err(|e| format!("step {step}: branch {}: {e:?}", te.kind))?
            } else {
                let vs = d.visible_slots();
                let intent = match te.kind.as_str() {
                    "ins" => Intent::Insert { index: if te.left == START { 0 } else { index_of(&vs, oid(te.left), "left origin")? + 1 }, value: te.value.clone() },
                    "move" => {
                        let i = index_of(&vs, from, "moved slot")?;
                        let others: Vec<Id> = vs.iter().copied().filter(|&t| t != from).collect();
                        Intent::Move { from: i, to: if te.left == START { 0 } else { index_of(&others, oid(te.left), "move left origin")? + 1 } }
                    }
                    "del" if is_list => Intent::Delete { index: index_of(&d.visible(), elem, "deleted element")? },
                    "del" => {
                        let i = index_of(&vs, from, "deleted slot")?;
                        mfrom = t2(vs[i]);
                        Intent::Delete { index: i }
                    }
                    "set" => {
                        ensure!(te.idx >= 0 && (te.idx as usize) < vs.len(), "step {step}: set with idx {} of {}", te.idx, vs.len());
                        mfrom = t2(vs[te.idx as usize]);
                        Intent::Set { index: te.idx as usize, value: te.value.clone() }
                    }
                    "undel" => {
                        let hs = d.hidden_slots();
                        let i = index_of(&hs, from, "undel slot")?;
                        mfrom = t2(hs[i]);
                        Intent::Undel { index: i }
                    }
                    k => return Err(format!("step {step}: unknown kind {k}")),
                };
                d.local(a, intent, None).map_err(|e| format!("step {step}: {}: {e:?}", te.kind))?
            };
            let me = mev_of(&e, is_list, mfrom);
            ensure!(e.id == x && me == te, "step {step}: event {x:?}: model {:?} {me:?}, trace {te:?}", e.id);
            ghost.deliver(&e).map_err(|e| format!("step {step}: ghost: {e:?}"))?;
            *cov.kinds.entry(if branch { format!("branch-{}", te.kind) } else { te.kind.clone() }).or_default() += 1;
            cov.gen += 1;
            undel_seen |= te.kind == "undel";
            events.insert(x, e);
        } else if changed.len() == 1 {
            let a = changed[0];
            let gained: Vec<Id> = s.reps[&a].rcv.difference(&p.reps[&a].rcv).copied().collect();
            ensure!(gained.len() == 1, "step {step}: replica {a} gained {} events", gained.len());
            let r = docs.get_mut(&a).unwrap().deliver(&events[&gained[0]]);
            ensure!(r == Ok(true), "step {step}: deliver {:?} to {a}: {r:?}", gained[0]);
            cov.deliveries += 1;
        } else if changed.is_empty() {
            for d in docs.values_mut() {
                for e in d.log() {
                    ensure!(d.deliver(&e) == Ok(false), "step {step}: re-delivery of {:?} changed a replica", e.id);
                }
            }
            cov.stutters += 1;
        } else {
            return Err(format!("step {step}: {} replicas changed without a new event", changed.len()));
        }
        check_all(step, s, &docs, &ghost, &events)?;
        hidden_seen |= docs.values().any(|d| d.elems().iter().any(|e| d.read(*e).is_some_and(|v| v.existence == Existence::Hidden)));
    }
    cov.traces_with_hidden += hidden_seen as usize;
    cov.traces_with_undel += undel_seen as usize;
    Ok(())
}

// =========================================================================================
// map

#[derive(Clone, Debug, PartialEq)]
struct KEv {
    parents: BTreeSet<Id>,
    lam: i64,
    kind: String,
    key: String,
    target: T2,
}
#[derive(Clone, Debug, PartialEq)]
struct KRep {
    rcv: BTreeSet<Id>,
    front: BTreeSet<Id>,
    cells: BTreeSet<(String, T2, Id, bool, bool)>,
    nested: BTreeMap<Id, BTreeSet<Id>>,
}
struct KState {
    ev: BTreeMap<Id, KEv>,
    cnt: BTreeMap<u64, i64>,
    reps: BTreeMap<u64, KRep>,
    ghost: KRep,
}

fn krep(j: &J) -> KRep {
    KRep {
        rcv: idset(j.get("rcv").unwrap()),
        front: idset(j.get("front").unwrap()),
        cells: j
            .get("cells")
            .unwrap()
            .items("#set")
            .iter()
            .map(|c| (c.get("key").unwrap().str().to_string(), tup(c.get("ts").unwrap()), id(c.get("w").unwrap()), c.get("live").unwrap().bool(), c.get("root").unwrap().bool()))
            .collect(),
        nested: map_of(j.get("nested").unwrap()).into_iter().map(|(k, v)| (id(k), idset(v))).collect(),
    }
}
fn kstate(j: &J) -> KState {
    let ev = map_of(j.var("ev").expect("ev"))
        .into_iter()
        .map(|(k, e)| {
            let g = |f: &str| e.get(f).unwrap();
            (id(k), KEv { parents: idset(g("parents")), lam: g("lam").int(), kind: g("kind").str().to_string(), key: g("key").str().to_string(), target: tup(g("target")) })
        })
        .collect();
    let cnt = map_of(j.var("cnt").expect("cnt")).into_iter().map(|(k, v)| (k.int() as u64, v.int())).collect();
    let reps = map_of(j.var("replicas").expect("replicas")).into_iter().map(|(k, v)| (k.int() as u64, krep(v))).collect();
    KState { ev, cnt, reps, ghost: krep(j.var("ghost").expect("ghost")) }
}

fn kev_of(e: &map::Event) -> KEv {
    let (kind, target) = match &e.op {
        KOp::Set { .. } => ("set", NOID),
        KOp::Root { .. } => ("root", NOID),
        KOp::Delete { .. } => ("del", NOID),
        KOp::Nest { target, .. } => ("nest", t2(*target)),
    };
    KEv { parents: e.parents.clone(), lam: e.lamport as i64, kind: kind.to_string(), key: e.op.key().to_string(), target }
}

fn kcompare(step: usize, who: &str, d: &map::Doc, r: &KRep) -> Result<(), String> {
    ensure!(d.event_ids() == r.rcv, "step {step}: {who}: rcv: model {:?}, trace {:?}", d.event_ids(), r.rcv);
    ensure!(*d.frontier() == r.front, "step {step}: {who}: front");
    let cells: BTreeSet<(String, T2, Id, bool, bool)> = d.cells().into_iter().map(|c| (c.key, ts(c.ts), c.w, c.live, c.root)).collect();
    ensure!(cells == r.cells, "step {step}: {who}: cells: model {cells:?}, trace {:?}", r.cells);
    ensure!(*d.nested() == r.nested, "step {step}: {who}: nested: model {:?}, trace {:?}", d.nested(), r.nested);
    // the visible map the trace's state implies
    let tv: map::MapView = r.cells.iter().filter(|c| c.3).map(|c| (c.0.clone(), (c.2, if c.4 { Some(r.nested.get(&c.2).cloned().unwrap_or_default()) } else { None }))).collect();
    ensure!(d.view() == tv, "step {step}: {who}: view: model {:?}, trace {tv:?}", d.view());
    Ok(())
}

fn kreplay(states: &[KState], rule: KeyRule, cov: &mut Cov) -> Result<(), String> {
    let s0 = &states[0];
    ensure!(s0.ev.is_empty(), "map traces start empty (init); this one has {} events", s0.ev.len());
    let mut docs: BTreeMap<u64, map::Doc> = s0.reps.keys().map(|&a| (a, map::Doc::with_rule(rule))).collect();
    let mut ghost = map::Doc::with_rule(rule);
    let mut events: BTreeMap<Id, map::Event> = BTreeMap::new();
    let check_all = |step: usize, s: &KState, docs: &BTreeMap<u64, map::Doc>, ghost: &map::Doc, events: &BTreeMap<Id, map::Event>| -> Result<(), String> {
        for (a, r) in &s.reps {
            kcompare(step, &format!("replica {a}"), &docs[a], r)?;
        }
        kcompare(step, "ghost", ghost, &s.ghost)?;
        for (a, c) in &s.cnt {
            let n = events.keys().filter(|x| x.agent == *a).count() as i64;
            ensure!(*c == n, "step {step}: cnt of {a}: trace {c}, model {n}");
        }
        Ok(())
    };
    check_all(0, s0, &docs, &ghost, &events)?;
    let mut hidden_seen = false;
    for (k, w) in states.windows(2).enumerate() {
        let (p, s) = (&w[0], &w[1]);
        let step = k + 1;
        cov.steps += 1;
        let new: Vec<Id> = s.ev.keys().filter(|x| !p.ev.contains_key(x)).copied().collect();
        let changed: Vec<u64> = s.reps.keys().filter(|a| s.reps[a] != p.reps[a]).copied().collect();
        ensure!(new.len() <= 1, "step {step}: {} new events", new.len());
        if let Some(&x) = new.first() {
            let te = s.ev[&x].clone();
            let a = x.agent;
            let d = docs.get_mut(&a).ok_or(format!("step {step}: unknown agent {a}"))?;
            let r = match te.kind.as_str() {
                "set" => d.set(a, &te.key, ""),
                "root" => d.set_root(a, &te.key),
                "del" => d.delete(a, &te.key),
                "nest" => d.nest(a, &te.key),
                k => return Err(format!("step {step}: unknown kind {k}")),
            };
            let e = r.map_err(|e| format!("step {step}: {} {}: {e:?}", te.kind, te.key))?;
            let me = kev_of(&e);
            ensure!(e.id == x && me == te, "step {step}: event {x:?}: model {:?} {me:?}, trace {te:?}", e.id);
            ghost.deliver(&e).map_err(|e| format!("step {step}: ghost: {e:?}"))?;
            *cov.kinds.entry(te.kind.clone()).or_default() += 1;
            cov.gen += 1;
            events.insert(x, e);
        } else if changed.len() == 1 {
            let a = changed[0];
            let gained: Vec<Id> = s.reps[&a].rcv.difference(&p.reps[&a].rcv).copied().collect();
            ensure!(gained.len() == 1, "step {step}: replica {a} gained {} events", gained.len());
            let r = docs.get_mut(&a).unwrap().deliver(&events[&gained[0]]);
            ensure!(r == Ok(true), "step {step}: deliver {:?} to {a}: {r:?}", gained[0]);
            cov.deliveries += 1;
        } else if changed.is_empty() {
            for d in docs.values_mut() {
                for e in d.log() {
                    ensure!(d.deliver(&e) == Ok(false), "step {step}: re-delivery of {:?} changed a replica", e.id);
                }
            }
            cov.stutters += 1;
        } else {
            return Err(format!("step {step}: {} replicas changed without a new event", changed.len()));
        }
        check_all(step, s, &docs, &ghost, &events)?;
        hidden_seen |= docs.values().any(|d| d.cells().iter().any(|c| !c.live));
    }
    cov.traces_with_hidden += hidden_seen as usize;
    Ok(())
}

// =========================================================================================
// reg (reg_model: one element, every op kind at any time after the insert)

#[derive(Clone, Debug, PartialEq)]
struct RTOp {
    id: Id,
    parents: BTreeSet<Id>,
    past: BTreeSet<Id>,
    lam: i64,
    kind: String,
    value: String,
}
#[derive(Clone, Debug, PartialEq)]
struct RRep {
    ops: BTreeMap<Id, RTOp>,
    last: usize,
    pos: (T2, T2),
    vreg: (T2, T2, String),
    max_e: BTreeSet<Id>,
    ex_anc: BTreeMap<Id, BTreeSet<Id>>,
    exl: (T2, String),
    /// the highest drop: (ts, op) (TS0, NOID if none)
    dropw: (T2, T2),
    /// the last-arrived existence op (existence-holder-by-arrival only; NOID otherwise)
    exh: T2,
    /// the history store: every received op's content by ID
    hist: BTreeMap<Id, String>,
    /// the replica's read (`view`: ok, pos, vw, value, hidden, ex, exw)
    view: RView,
}
type RView = (bool, T2, T2, String, bool, String, T2);
struct RState {
    ev: BTreeMap<Id, RTOp>,
    cnt: BTreeMap<u64, i64>,
    reps: BTreeMap<u64, RRep>,
}

fn rtop(j: &J) -> RTOp {
    let g = |f: &str| j.get(f).unwrap();
    RTOp { id: id(g("id")), parents: idset(g("parents")), past: idset(g("past")), lam: g("lam").int(), kind: g("kind").str().to_string(), value: g("value").str().to_string() }
}
fn rrep(j: &J) -> RRep {
    let inc = j.get("inc").unwrap();
    let pos = inc.get("pos").unwrap();
    let vr = inc.get("vreg").unwrap();
    let exl = inc.get("exl").unwrap();
    let dw = inc.get("dropw").unwrap();
    RRep {
        ops: j.get("ops").unwrap().items("#set").iter().map(rtop).map(|o| (o.id, o)).collect(),
        last: j.get("last").unwrap().items("#set").len(),
        pos: (tup(pos.get("ts").unwrap()), tup(pos.get("w").unwrap())),
        vreg: (tup(vr.get("ts").unwrap()), tup(vr.get("w").unwrap()), vr.get("value").unwrap().str().to_string()),
        max_e: idset(inc.get("maxE").unwrap()),
        ex_anc: map_of(inc.get("exAnc").unwrap()).into_iter().map(|(k, v)| (id(k), idset(v))).collect(),
        exl: (tup(exl.get("ts").unwrap()), exl.get("kind").unwrap().str().to_string()),
        dropw: (tup(dw.get("ts").unwrap()), tup(dw.get("w").unwrap())),
        exh: tup(inc.get("exh").unwrap()),
        hist: map_of(j.get("hist").unwrap()).into_iter().map(|(k, v)| (id(k), v.str().to_string())).collect(),
        view: {
            let v = j.get("view").unwrap();
            let g = |f: &str| v.get(f).unwrap();
            (g("ok").bool(), tup(g("pos")), tup(g("vw")), g("value").str().to_string(), g("hidden").bool(), g("ex").str().to_string(), tup(g("exw")))
        },
    }
}
fn rstate(j: &J) -> RState {
    RState {
        ev: map_of(j.var("ev").expect("ev")).into_iter().map(|(k, v)| (id(k), rtop(v))).collect(),
        cnt: map_of(j.var("cnt").expect("cnt")).into_iter().map(|(k, v)| (k.int() as u64, v.int())).collect(),
        reps: map_of(j.var("replicas").expect("replicas")).into_iter().map(|(k, v)| (k.int() as u64, rrep(v))).collect(),
    }
}

fn kind_name(k: &Kind<String>) -> &'static str {
    match k {
        Kind::Insert(_) => "ins",
        Kind::Move => "move",
        Kind::Set(_) => "set",
        Kind::Delete => "del",
        Kind::Undel => "undel",
        Kind::Drop => "drop",
    }
}
fn rt_of(o: &ROp<Id, String>) -> RTOp {
    // parents are filled in by the caller (the reg crate's op has none: its past is enough)
    RTOp { id: o.id, parents: BTreeSet::new(), past: o.past.clone(), lam: o.lamport as i64, kind: kind_name(&o.kind).to_string(), value: o.kind.value().cloned().unwrap_or_default() }
}

fn rcompare(step: usize, who: &str, d: &Reg<Id, String>, parents: &BTreeMap<Id, BTreeSet<Id>>, r: &RRep, rule: ReadRule) -> Result<(), String> {
    let mine: BTreeMap<Id, RTOp> = d.ops().map(|o| (o.id, RTOp { parents: parents[&o.id].clone(), ..rt_of(o) })).collect();
    ensure!(mine == r.ops, "step {step}: {who}: ops: model {mine:?}, trace {:?}", r.ops);
    ensure!(r.last == 0, "step {step}: {who}: last (reg-order-sensitive only) is not empty");
    let v = mpce_reg::read_with(d, rule);
    let (pos, vreg) = match &v {
        Some(v) => ((ts(v.pos_ts), t2(v.pos)), (ts(v.value_ts), t2(v.value_write), v.value.clone())),
        None => ((TS0, NOID), (TS0, NOID, String::new())),
    };
    ensure!(pos == r.pos, "step {step}: {who}: inc.pos: model {pos:?}, trace {:?}", r.pos);
    ensure!(vreg == r.vreg, "step {step}: {who}: inc.vreg: model {vreg:?}, trace {:?}", r.vreg);
    let mx = mpce_reg::max_existence(d);
    ensure!(mx == r.max_e, "step {step}: {who}: inc.maxE: model {mx:?}, trace {:?}", r.max_e);
    // exAnc: per received op, the existence ops (deletes, undels, drops) in its past or itself
    let is_ex_any = |k: &Kind<String>| k.is_existence() || *k == Kind::Drop;
    let anc: BTreeMap<Id, BTreeSet<Id>> = d.ops().map(|o| (o.id, d.ops().filter(|y| is_ex_any(&y.kind) && (y.id == o.id || o.past.contains(&y.id))).map(|y| y.id).collect())).collect();
    ensure!(anc == r.ex_anc, "step {step}: {who}: inc.exAnc: model {anc:?}, trace {:?}", r.ex_anc);
    let exl = d.ops().filter(|o| o.kind.is_existence()).max_by_key(|o| o.ts()).map_or((TS0, "none".to_string()), |o| (ts(o.ts()), kind_name(&o.kind).to_string()));
    ensure!(exl == r.exl, "step {step}: {who}: inc.exl: model {exl:?}, trace {:?}", r.exl);
    // the drop register (the highest drop), the by-arrival holder (unused), the history store
    let dropw = d.ops().filter(|o| o.kind == Kind::Drop).max_by_key(|o| o.ts()).map_or((TS0, NOID), |o| (ts(o.ts()), t2(o.id)));
    ensure!(dropw == r.dropw, "step {step}: {who}: inc.dropw: model {dropw:?}, trace {:?}", r.dropw);
    ensure!(r.exh == NOID, "step {step}: {who}: inc.exh (existence-holder-by-arrival only) is set: {:?}", r.exh);
    let hist: BTreeMap<Id, String> = d.ops().map(|o| (o.id, o.kind.value().cloned().unwrap_or_default())).collect();
    ensure!(hist == r.hist, "step {step}: {who}: hist: model {hist:?}, trace {:?}", r.hist);
    // the read itself, existence holder (winner(elem, existence)) included
    let rv: RView = match &v {
        Some(v) => (
            true,
            t2(v.pos),
            t2(v.value_write),
            // the live view shows no content of a dropped element; the register keeps it
            if v.existence == Existence::Dropped { String::new() } else { v.value.clone() },
            v.hidden(),
            match v.existence {
                Existence::Live => "live",
                Existence::Hidden => "hidden",
                Existence::Dropped => "dropped",
            }
            .to_string(),
            t2(v.existence_holder),
        ),
        None => (false, NOID, NOID, String::new(), true, "none".to_string(), NOID),
    };
    ensure!(rv == r.view, "step {step}: {who}: view: model {rv:?}, trace {:?}", r.view);
    // hidden: the read's rule against the trace's T column (a maximal delete),
    // for an element with no drop (dropped: hidden whatever the T column holds)
    if let Some(v) = v {
        let th = r.max_e.iter().any(|m| r.ops[m].kind == "del");
        if v.existence == Existence::Dropped {
            ensure!(r.dropw.1 != NOID, "step {step}: {who}: dropped, but the trace's drop register is empty");
        } else {
            ensure!(v.hidden() == th, "step {step}: {who}: hidden: model {}, trace (maxE) {th}", v.hidden());
        }
    }
    Ok(())
}

fn rreplay(states: &[RState], rule: ReadRule, gap: i64, cov: &mut Cov) -> Result<(), String> {
    let s0 = &states[0];
    ensure!(s0.ev.is_empty(), "reg traces start empty");
    let mut docs: BTreeMap<u64, Reg<Id, String>> = s0.reps.keys().map(|&a| (a, Reg::new())).collect();
    let mut ops: BTreeMap<Id, ROp<Id, String>> = BTreeMap::new();
    let mut parents: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
    let check_all = |step: usize, s: &RState, docs: &BTreeMap<u64, Reg<Id, String>>, parents: &BTreeMap<Id, BTreeSet<Id>>, ops: &BTreeMap<Id, ROp<Id, String>>| -> Result<(), String> {
        for (a, r) in &s.reps {
            rcompare(step, &format!("replica {a}"), &docs[a], parents, r, rule)?;
        }
        for (a, c) in &s.cnt {
            let n = ops.keys().filter(|x| x.agent == *a).count() as i64;
            ensure!(*c == n, "step {step}: cnt of {a}: trace {c}, model {n}");
        }
        Ok(())
    };
    check_all(0, s0, &docs, &parents, &ops)?;
    let (mut hidden_seen, mut undel_seen) = (false, false);
    for (k, w) in states.windows(2).enumerate() {
        let (p, s) = (&w[0], &w[1]);
        let step = k + 1;
        cov.steps += 1;
        let new: Vec<Id> = s.ev.keys().filter(|x| !p.ev.contains_key(x)).copied().collect();
        let changed: Vec<u64> = s.reps.keys().filter(|a| s.reps[a] != p.reps[a]).copied().collect();
        ensure!(new.len() <= 1, "step {step}: {} new ops", new.len());
        if let Some(&x) = new.first() {
            let te = s.ev[&x].clone();
            let a = x.agent;
            let d = docs.get_mut(&a).ok_or(format!("step {step}: unknown agent {a}"))?;
            // the creator's state: past = everything it has; parents = its maximal ops
            let past = d.ids();
            let front: BTreeSet<Id> = d.ops().filter(|o| !d.ops().any(|y| y.past.contains(&o.id))).map(|o| o.id).collect();
            let top = d.ops().map(|o| o.lamport as i64).max().unwrap_or(-1);
            ensure!(te.lam > top && te.lam <= top + 1 + gap, "step {step}: lamport {} outside [{}, {}]", te.lam, top + 1, top + 1 + gap);
            let seq = ops.keys().filter(|y| y.agent == a).count() as u64;
            let kind = match te.kind.as_str() {
                "ins" => Kind::Insert(te.value.clone()),
                "move" => Kind::Move,
                "set" => Kind::Set(te.value.clone()),
                "del" => Kind::Delete,
                "undel" => Kind::Undel,
                "drop" => Kind::Drop,
                k => return Err(format!("step {step}: unknown kind {k}")),
            };
            let op = ROp { id: Id::new(a, seq), agent: a, lamport: te.lam as u64, past, kind, restores_write: None };
            let mine = RTOp { parents: front.clone(), ..rt_of(&op) };
            ensure!(mine == te, "step {step}: op {x:?}: model {mine:?}, trace {te:?}");
            ensure!(d.apply(op.clone()) == Ok(true), "step {step}: apply at the creator");
            *cov.kinds.entry(te.kind.clone()).or_default() += 1;
            cov.gen += 1;
            undel_seen |= te.kind == "undel";
            parents.insert(x, front);
            ops.insert(x, op);
        } else if changed.len() == 1 {
            let a = changed[0];
            let gained: Vec<Id> = s.reps[&a].ops.keys().filter(|y| !p.reps[&a].ops.contains_key(y)).copied().collect();
            ensure!(gained.len() == 1, "step {step}: replica {a} gained {} ops", gained.len());
            let o = ops[&gained[0]].clone();
            let d = docs.get_mut(&a).unwrap();
            ensure!(o.past.iter().all(|y| d.contains(*y)), "step {step}: delivery of {:?} to {a} is not causal", o.id);
            ensure!(d.apply(o) == Ok(true), "step {step}: deliver {:?} to {a}: not new", gained[0]);
            cov.deliveries += 1;
        } else if changed.is_empty() {
            for d in docs.values_mut() {
                for o in d.ops().cloned().collect::<Vec<_>>() {
                    ensure!(d.apply(o) == Ok(false), "step {step}: re-delivery changed a replica");
                }
            }
            cov.stutters += 1;
        } else {
            return Err(format!("step {step}: {} replicas changed without a new op", changed.len()));
        }
        check_all(step, s, &docs, &parents, &ops)?;
        hidden_seen |= docs.values().any(|d| mpce_reg::read(d).is_some_and(|v| v.hidden()));
    }
    cov.traces_with_hidden += hidden_seen as usize;
    cov.traces_with_undel += undel_seen as usize;
    Ok(())
}

// =========================================================================================

fn main() {
    let mut model = String::new();
    let mut config = Config::default();
    let mut key_rule = KeyRule::Correct;
    let mut gap = 1i64;
    let mut expect_mismatch = false;
    let mut paths = Vec::new();
    let mut args = std::env::args().skip(1);
    let mut mutants = Vec::new();
    let mut digest_out: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--model" => model = args.next().expect("--model list|mlist|map|reg"),
            "--rule" => {
                config.rule = match args.next().as_deref() {
                    Some("fuguemax") => SiblingRule::FugueMax,
                    Some("fugue") => SiblingRule::Fugue,
                    _ => panic!("--rule fuguemax|fugue"),
                };
                mutants.push("rule");
            }
            "--read" => {
                config.read = match args.next().as_deref() {
                    Some("correct") => ReadRule::Correct,
                    Some("undel-lww") => ReadRule::UndelLww,
                    Some("delete-loses-to-set") => ReadRule::DeleteLosesToSet,
                    Some("agent-tiebreak-reversed") => ReadRule::AgentTiebreakReversed,
                    Some("holder-covers-every-delete") => ReadRule::HolderCoversEveryDelete,
                    _ => panic!("--read correct|undel-lww|delete-loses-to-set|agent-tiebreak-reversed|holder-covers-every-delete"),
                };
                mutants.push("read");
            }
            "--variant" => {
                config.variant = match args.next().as_deref() {
                    Some("correct") => Variant::Correct,
                    Some("register-reads-order") => Variant::RegisterReadsOrder,
                    _ => panic!("--variant correct|register-reads-order"),
                };
                mutants.push("variant");
            }
            "--key-rule" => {
                key_rule = match args.next().as_deref() {
                    Some("correct") => KeyRule::Correct,
                    Some("no-agent-tiebreak") => KeyRule::NoAgentTiebreak,
                    Some("delete-wins") => KeyRule::DeleteWins,
                    _ => panic!("--key-rule correct|no-agent-tiebreak|delete-wins"),
                };
                mutants.push("key-rule");
            }
            "--gap" => gap = args.next().and_then(|g| g.parse().ok()).expect("--gap N"),
            "--digests" => digest_out = Some(args.next().expect("--digests PATH")),
            "--expect-mismatch" => expect_mismatch = true,
            _ => paths.push(a),
        }
    }
    assert!(["list", "mlist", "map", "reg"].contains(&model.as_str()), "--model list|mlist|map|reg");
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
    let mut first: Option<String> = None;
    let mut cov = Cov::default();
    let mut digests = BTreeSet::new();
    for f in &files {
        let txt = std::fs::read_to_string(f).expect("read");
        let j = parse(&txt).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let sts = j.get("states").expect("states").list();
        // distinct traces: a cheap digest of the states' text
        digests.insert({
            let mut h: u64 = 0xcbf29ce484222325;
            for b in txt.split("\"states\"").nth(1).unwrap_or("").bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100000001b3);
            }
            h
        });
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut c = Cov::default();
            let r = match model.as_str() {
                "mlist" => mreplay(&sts.iter().map(|s| mstate(s, false)).collect::<Vec<_>>(), config, false, &mut c),
                "list" => mreplay(&sts.iter().map(|s| mstate(s, true)).collect::<Vec<_>>(), config, true, &mut c),
                "map" => kreplay(&sts.iter().map(kstate).collect::<Vec<_>>(), key_rule, &mut c),
                _ => rreplay(&sts.iter().map(rstate).collect::<Vec<_>>(), config.read, gap, &mut c),
            };
            (r, c)
        }))
        .unwrap_or_else(|p| (Err(format!("panic: {:?}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())))), Cov::default()));
        match res.0 {
            Ok(()) => {
                ok += 1;
                let c = res.1;
                cov.steps += c.steps;
                cov.gen += c.gen;
                cov.deliveries += c.deliveries;
                cov.stutters += c.stutters;
                cov.branch += c.branch;
                cov.traces_with_hidden += c.traces_with_hidden;
                cov.traces_with_undel += c.traces_with_undel;
                for (k, n) in c.kinds {
                    *cov.kinds.entry(k).or_default() += n;
                }
            }
            Err(e) => {
                bad += 1;
                if first.is_none() {
                    first = Some(format!("{}: {e}", f.display()));
                }
            }
        }
    }
    if let Some(p) = &digest_out {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).expect("--digests file");
        for d in &digests {
            writeln!(f, "{d:016x}").unwrap();
        }
    }
    if let Some(m) = &first {
        println!("first mismatch: {m}");
    }
    let kinds: Vec<String> = cov.kinds.iter().map(|(k, n)| format!("{k}:{n}")).collect();
    println!(
        "LMM_REPLAY model={model} mutants={} traces={} distinct={} matched={ok} mismatched={bad} steps={} events={} deliveries={} stutters={} branch_ops={} with_hidden={} with_undel={} kinds={}",
        if mutants.is_empty() { "none".to_string() } else { mutants.join(",") },
        files.len(),
        digests.len(),
        cov.steps,
        cov.gen,
        cov.deliveries,
        cov.stutters,
        cov.branch,
        cov.traces_with_hidden,
        cov.traces_with_undel,
        kinds.join(",")
    );
    let pass = if expect_mismatch { bad > 0 } else { bad == 0 && !files.is_empty() };
    std::process::exit(if pass { 0 } else { 1 });
}
