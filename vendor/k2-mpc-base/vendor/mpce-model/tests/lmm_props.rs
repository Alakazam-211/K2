//! Property tests (PT) for the layered composition, on generated runs
//! (`MPCE_PROP_SEEDS` runs, default 300).
//!
//! A run: 2 or 3 agents, a seed of 0-3 elements, then 10-40 steps, each one of: a local op of
//! any kind (insert, move, set, delete, undel, drop) from the agent's view; a branch-merge
//! delete or undel made on an older version of the agent's replica; or a causal
//! delivery of one event to one replica (re-delivery included). After every step, for every
//! replica:
//! - **`projection`** (the Quint mlist model's, in Rust): the composed view (elements, value writes,
//!   values) equals order ∘ read, where the order is rebuilt from the replica's slot events
//!   *alone*, integrated in a fresh random causal order, and each element's read is of its op
//!   set rebuilt from the event graph in a random order. So the view is a function of the two
//!   layers' states, each of which is a function of its own events only;
//! - **`slot_order_independent`**: the replica's slot list equals the rebuilt one;
//! - **`winner()`** for every element and facet equals the rebuilt register's;
//! - **the applied stream**: the entry the step appended reports the element's visible index
//!   and value after it, and `effect: none` iff the view did not change;
//! - **converge**: replicas holding the same event set have the same view and slot list.
//!
//! Planted negative: the composition mutant `register-reads-order` (a set lands on the
//! element at its generation index in the receiver's view) must fail `projection` on some run.

use mpce_model::mlist::{Config, Doc, Effect, Event, Intent, MOp, Variant};
use mpce_model::text::Id;
use mpce_order::SlotOrder;
use mpce_reg::{Facet, Kind, Op as ROp, Reg};
use std::collections::{BTreeMap, BTreeSet};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

fn seeds(default: u64) -> u64 {
    std::env::var("MPCE_PROP_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn closure(events: &BTreeMap<Id, Event>, roots: &BTreeSet<Id>) -> BTreeSet<Id> {
    let mut c = BTreeSet::new();
    let mut st: Vec<Id> = roots.iter().copied().collect();
    while let Some(x) = st.pop() {
        if c.insert(x) {
            st.extend(events[&x].parents.iter().copied());
        }
    }
    c
}

/// a random causal order of `ids` (a random topological sort)
fn random_causal_order(rng: &mut Rng, events: &BTreeMap<Id, Event>, ids: &BTreeSet<Id>) -> Vec<Id> {
    let mut done: BTreeSet<Id> = BTreeSet::new();
    let mut out = Vec::new();
    while out.len() < ids.len() {
        let ready: Vec<Id> = ids.iter().copied().filter(|x| !done.contains(x) && events[x].parents.iter().all(|p| done.contains(p) || !ids.contains(p))).collect();
        let x = ready[rng.below(ready.len())];
        done.insert(x);
        out.push(x);
    }
    out
}

/// order ∘ read, rebuilt from the replica's event set alone: (view, slot list, registers)
type Rebuilt = (Vec<(Id, Id, String)>, Vec<Id>, BTreeMap<Id, Reg<Id, String>>);
fn rebuild(rng: &mut Rng, d: &Doc) -> Rebuilt {
    let events: BTreeMap<Id, Event> = d.log().into_iter().map(|e| (e.id, e)).collect();
    let ids: BTreeSet<Id> = events.keys().copied().collect();
    // the order layer from the slot events alone, in a fresh causal order (a random causal
    // order of every event, restricted to the slot events: causality between two slot events
    // can run through other events)
    let mut order: SlotOrder<Id> = SlotOrder::new();
    let mut slot_elem = BTreeMap::new();
    for x in random_causal_order(rng, &events, &ids).into_iter().filter(|x| events[x].op.placement().is_some()) {
        let e = &events[&x];
        order.insert(x, e.op.placement().unwrap()).unwrap();
        slot_elem.insert(x, e.op.elem(x));
    }
    // the register layer: each element's op set, in a random order, pasts from the graph
    let mut by_elem: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
    for e in events.values() {
        by_elem.entry(e.op.elem(e.id)).or_default().push(e.id);
    }
    let mut regs = BTreeMap::new();
    for (el, mut ops) in by_elem {
        rng.shuffle(&mut ops);
        let mine: BTreeSet<Id> = ops.iter().copied().collect();
        let mut r = Reg::new();
        for x in ops {
            let e = &events[&x];
            let kind = match &e.op {
                MOp::Insert { value, .. } => Kind::Insert(value.clone()),
                MOp::Move { .. } => Kind::Move,
                MOp::Set { value, .. } => Kind::Set(value.clone()),
                MOp::Delete { .. } => Kind::Delete,
                MOp::Undel { .. } => Kind::Undel,
                MOp::Drop { .. } => Kind::Drop,
            };
            let past = closure(&events, &e.parents).into_iter().filter(|p| mine.contains(p)).collect();
            r.apply(ROp { id: x, agent: x.agent, lamport: e.lamport, past, kind, restores_write: e.restores_write }).unwrap();
        }
        regs.insert(el, r);
    }
    let reads: BTreeMap<Id, _> = regs.iter().filter_map(|(e, r)| mpce_reg::read(r).map(|v| (*e, v))).collect();
    let shown = order.visible(|s| reads.get(&slot_elem[&s]).is_some_and(|v| v.pos == s && !v.hidden()));
    let view = shown
        .into_iter()
        .map(|s| {
            let e = slot_elem[&s];
            (e, reads[&e].value_write, reads[&e].value.clone())
        })
        .collect();
    (view, order.slots(), regs)
}

/// One generated run; Err(description) on the first property that fails.
fn run(seed: u64, variant: Variant) -> Result<usize, String> {
    let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let config = Config { variant, ..Config::default() };
    let n_agents = 2 + rng.below(2) as u64;
    let mut seed_doc = Doc::with_config(config);
    for k in 0..rng.below(4) {
        seed_doc.insert(0, k, &format!("s{k}")).map_err(|e| format!("seed: {e:?}"))?;
    }
    seed_doc.take_stream();
    let mut docs: Vec<Doc> = (0..n_agents).map(|_| seed_doc.clone()).collect();
    let mut all: BTreeMap<Id, Event> = seed_doc.log().into_iter().map(|e| (e.id, e)).collect();
    let steps = 10 + rng.below(31);
    let mut checks = 0;
    for step in 0..steps {
        let a = rng.below(docs.len());
        let agent = a as u64 + 1;
        let before = docs[a].view();
        let len0 = docs[a].stream().len();
        let choice = rng.below(10);
        let d = &mut docs[a];
        let vis = d.visible().len();
        let made: Option<Event> = match choice {
            0 | 1 => d.insert(agent, rng.below(vis + 1), &format!("i{step}")).ok(),
            2 | 3 if vis > 0 => d.move_elem(agent, rng.below(vis), rng.below(vis)).ok(),
            4 if vis > 0 => d.set(agent, rng.below(vis), &format!("v{step}")).ok(),
            5 if vis > 0 => d.delete(agent, rng.below(vis)).ok(),
            6 => {
                let h = d.hidden_slots().len();
                if h > 0 { d.undel(agent, rng.below(h)).ok() } else { None }
            }
            7 if rng.below(4) == 0 => {
                let el: Vec<Id> = d.elems().into_iter().collect();
                if el.is_empty() { None } else { d.drop_elem(agent, el[rng.below(el.len())]).ok() }
            }
            7 => {
                // branch merge: an older version holding all of this agent's own events
                let log = d.log();
                let cut = rng.below(log.len() + 1);
                let mut v: BTreeSet<Id> = log[..cut].iter().map(|e| e.id).collect();
                v.extend(log.iter().filter(|e| e.id.agent == agent).map(|e| e.id));
                let old = d.fork_at(&v).map_err(|e| format!("fork: {e:?}"))?;
                let (vs, hs) = (old.visible().len(), old.hidden_slots().len());
                if rng.below(2) == 0 && vs > 0 {
                    d.branch(agent, &v, Intent::Delete { index: rng.below(vs) }, None).ok()
                } else if hs > 0 {
                    d.branch(agent, &v, Intent::Undel { index: rng.below(hs) }, None).ok()
                } else {
                    None
                }
            }
            _ => {
                // deliver one event this replica can take (or a re-delivery)
                let ready: Vec<&Event> = all.values().filter(|e| e.parents.iter().all(|p| d.has(*p))).collect();
                if !ready.is_empty() {
                    let e = ready[rng.below(ready.len())].clone();
                    d.deliver(&e).map_err(|x| format!("step {step}: deliver {:?}: {x:?}", e.id))?;
                }
                None
            }
        };
        if let Some(e) = made {
            all.insert(e.id, e);
        }
        // the applied stream: the step's entries end at the current view
        let d = &docs[a];
        if let Some(last) = d.stream()[len0..].last() {
            let vis = d.visible();
            if last.index_after != vis.iter().position(|&x| x == last.elem) {
                return Err(format!("step {step}: stream index_after {:?}, view {vis:?}", last.index_after));
            }
            if last.value_after != d.read(last.elem).map(|v| v.value) {
                return Err(format!("step {step}: stream value_after"));
            }
            if d.stream().len() == len0 + 1 && (last.effect == Effect::None) != (before == d.view()) {
                return Err(format!("step {step}: stream effect {:?}, view changed {}", last.effect, before != d.view()));
            }
        }
        for (i, d) in docs.iter().enumerate() {
            let (view, slots, regs) = rebuild(&mut rng, d);
            if d.view_writes() != view {
                return Err(format!("seed {seed} step {step} replica {i}: projection: composed {:?}, order∘read {view:?}", d.view_writes()));
            }
            if d.slots() != slots {
                return Err(format!("seed {seed} step {step} replica {i}: slot_order_independent"));
            }
            for (el, r) in &regs {
                for f in [Facet::Value, Facet::Position, Facet::Existence] {
                    if d.winner(*el, f) != mpce_reg::winner(r, f) {
                        return Err(format!("seed {seed} step {step} replica {i}: winner({el:?}, {f:?})"));
                    }
                }
            }
            checks += 1;
        }
        for x in &docs {
            for y in &docs {
                if x.event_ids() == y.event_ids() && (x.view() != y.view() || x.slots() != y.slots()) {
                    return Err(format!("seed {seed} step {step}: converge"));
                }
            }
        }
    }
    Ok(checks)
}

#[test]
fn mlist_projection_on_generated_runs() {
    let n = seeds(300);
    let mut checks = 0;
    for seed in 0..n {
        checks += run(seed, Variant::Correct).unwrap_or_else(|e| panic!("{e}"));
    }
    println!("PT mlist projection: runs={n} replica_checks={checks} failures=0");
}

#[test]
fn planted_register_reads_order_fails_projection() {
    let n = seeds(300);
    let failed: Vec<u64> = (0..n).filter(|&s| run(s, Variant::RegisterReadsOrder).is_err()).collect();
    println!("PT planted register-reads-order: runs={n} failing={}", failed.len());
    assert!(!failed.is_empty(), "the mutant passed the projection property on {n} runs");
}
