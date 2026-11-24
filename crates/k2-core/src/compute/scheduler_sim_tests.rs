//! Simulations of the duration-aware queue (design-compute-duration-aware-
//! queue-v1) over a recorded mix: the night of 2026-11-22/23's shape, many
//! 28 GB daemon test runs (25–45 min) and 16 GB suites (15–25 min) beside
//! quick 1–5 minute checks (2–4 GB). Each prints the mean and p95 wait per
//! size class before (today's rules) and after, and checks what the change
//! promises: quick checks wait less, nothing starves, and the longest wait
//! is bounded.
//!
//! Two models: one solo node through [`decide_opts`] (the real scheduler),
//! and a fleet of nodes fed by the central hold's offer order
//! ([`dispatch_key`]) with the hour-old reservation ([`ends_within`]), as
//! `compute_place::sweep` runs it every 5 s.

use super::*;
use crate::compute::proto::frames::{Availability, Caps, Control, Free};

const GB: u64 = 1 << 30;
const TICK: i64 = 15;

fn offer(parallel: u32, mem_gb: u64, used_gb: u64, running: u32) -> Offer {
    Offer {
        share: None,
        revision: 1,
        control: Control::Active,
        control_by: None,
        control_at: None,
        availability: Availability { ok: true, reasons: vec![] },
        caps: Caps { cpu_millis: 16_000, mem_bytes: mem_gb * GB, disk_budget_bytes: 500 * GB, hard: true, gpus: 0 },
        free: Free { cpu_millis: 16_000, mem_bytes: (mem_gb - used_gb) * GB, disk_free_bytes: 900 * GB, gpus: 0 },
        slots: vec![],
        running,
        parallel_max: parallel,
        refusal: None,
        labels: BTreeMap::new(),
        tools: BTreeMap::from([("git".into(), "2".into())]),
        protocol: crate::compute::proto::PROTOCOL,
        boot_id: "b".into(),
        ledger_id: "l".into(),
        node_version: "0.1.0".into(),
        features: Vec::new(),
        host: None,
        pkg: None,
        sim: None,
    }
}

#[derive(Debug, Clone)]
struct SimJob {
    q: QueuedJob,
    class: &'static str,
    /// How long it really runs (its estimate ±20 %).
    actual: i64,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

/// The recorded mix, scaled: `big28`/`big16`/`quick` jobs over `hours`,
/// from 8 builders of one workspace; every job's family has history (its
/// estimate is the family median, the real run ±20 %).
fn mix(seed: u64, big28: usize, big16: usize, quick: usize, hours: i64) -> Vec<SimJob> {
    let mut r = Rng(seed);
    let span = (hours * 3600) as u64;
    let mut jobs = Vec::new();
    let mut add = |r: &mut Rng, n: usize, class: &'static str, mem: (u64, u64), mins: (u64, u64)| {
        for _ in 0..n {
            let est = r.range(mins.0 * 60, mins.1 * 60);
            let actual = (est as i64) * (80 + r.range(0, 40) as i64) / 100;
            let at = r.range(0, span) as i64;
            let id = format!("{class}-{}", jobs.len());
            let sub = format!("k2/s/b{}", r.range(0, 7));
            jobs.push(SimJob {
                q: QueuedJob {
                    id,
                    workspace_id: "k2".into(),
                    created_at: at,
                    exclusive: false,
                    cpu_millis: None,
                    mem_bytes: Some(r.range(mem.0, mem.1) * GB),
                    disk_bytes: 0,
                    needs_git: true,
                    tier: Tier::Normal,
                    submitter: Some(sub),
                    gpus: 0,
                    est_secs: Some(est),
                },
                class,
                actual: actual.max(30),
            });
        }
    };
    add(&mut r, big28, "28G", (28, 28), (25, 45));
    add(&mut r, big16, "16G", (16, 16), (15, 25));
    add(&mut r, quick, "quick", (2, 4), (1, 5));
    jobs.sort_by_key(|j| j.q.created_at);
    jobs
}

/// Waits per class: (mean, p95, max) in minutes.
fn stats(waits: &HashMap<&'static str, Vec<i64>>, class: &str) -> (f64, f64, f64) {
    let mut v = waits.get(class).cloned().unwrap_or_default();
    assert!(!v.is_empty(), "no {class} jobs started");
    v.sort_unstable();
    let mean = v.iter().sum::<i64>() as f64 / v.len() as f64 / 60.0;
    let p95 = v[((v.len() as f64 * 0.95).ceil() as usize).clamp(1, v.len()) - 1] as f64 / 60.0;
    (mean, p95, *v.last().unwrap() as f64 / 60.0)
}

fn report(model: &str, before: &HashMap<&'static str, Vec<i64>>, after: &HashMap<&'static str, Vec<i64>>) {
    println!("SIM {model}: wait in minutes, before → after (mean / p95 / max)");
    for class in ["28G", "16G", "quick"] {
        let (bm, bp, bx) = stats(before, class);
        let (am, ap, ax) = stats(after, class);
        println!("SIM {model} {class:>5}: mean {bm:6.1} → {am:6.1} · p95 {bp:6.1} → {ap:6.1} · max {bx:6.1} → {ax:6.1}");
    }
    let all = |w: &HashMap<&'static str, Vec<i64>>| {
        let v: Vec<i64> = w.values().flatten().copied().collect();
        v.iter().sum::<i64>() as f64 / v.len() as f64 / 60.0
    };
    println!("SIM {model}   all: mean {:6.1} → {:6.1}", all(before), all(after));
}

/// One solo node (64 GB, 4 slots) through the real scheduler.
fn run_solo(jobs: &[SimJob], opts: Opts) -> HashMap<&'static str, Vec<i64>> {
    let mut waits: HashMap<&'static str, Vec<i64>> = HashMap::new();
    let mut pending: VecDeque<SimJob> = jobs.iter().cloned().collect();
    let mut queue: Vec<QueuedJob> = Vec::new();
    let mut class_of: HashMap<String, (&'static str, i64)> = HashMap::new();
    // (end, mem GB, started, est)
    let mut running: Vec<(i64, u64, i64, u64)> = Vec::new();
    let mut turns = Turns::default();
    let caps = Limits { grant_max_parallel: &|_| 4, submitter_max_running: &|_| 0 };
    let mut t = 0;
    while !(pending.is_empty() && queue.is_empty()) {
        assert!(t < 72 * 3600, "the solo simulation never drained");
        running.retain(|r| r.0 > t);
        while pending.front().is_some_and(|j| j.q.created_at <= t) {
            let j = pending.pop_front().unwrap();
            class_of.insert(j.q.id.clone(), (j.class, j.actual));
            queue.push(j.q);
        }
        loop {
            let used: u64 = running.iter().map(|r| r.1).sum();
            let o = offer(4, 64, used, running.len() as u32);
            let mut load = Load::default();
            load.running_by_workspace.insert("k2".into(), running.len() as u32);
            load.running_left = running.iter().map(|r| Some(r.3.saturating_sub((t - r.2) as u64))).collect();
            let d = decide_opts(t, &queue, &turns, &load, &caps, Some(&o), None, &opts);
            let Some(id) = d.place else { break };
            let i = queue.iter().position(|j| j.id == id).unwrap();
            let j = queue.remove(i);
            let (class, actual) = class_of[&j.id];
            waits.entry(class).or_default().push(t - j.created_at);
            running.push((t + actual, j.mem_bytes.unwrap() / GB, t, j.est_secs.unwrap()));
            turns.served(&j);
        }
        t += TICK;
    }
    waits
}

#[derive(Clone)]
struct Node {
    mem_gb: u64,
    slots: u32,
    /// (end, mem GB, started, est)
    running: Vec<(i64, u64, i64, u64)>,
}

impl Node {
    fn used(&self) -> u64 {
        self.running.iter().map(|r| r.1).sum()
    }
    fn starts(&self, mem: u64) -> bool {
        (self.running.len() as u32) < self.slots && self.used() + mem <= self.mem_gb
    }
    /// The soonest expected end of a running job, from now.
    fn shadow(&self, t: i64) -> Option<u64> {
        self.running.iter().map(|r| r.3.saturating_sub((t - r.2) as u64)).min()
    }
}

/// A fleet (`nodes`) fed by the central hold: each tick the held jobs are
/// offered in order to the node that starts them now with the most free
/// memory; after: the duration order, and an hour-old job that starts
/// nowhere reserves the node with the fewest jobs it fits, which then
/// starts only jobs that end before its soonest end (else within the idle
/// bound).
fn run_fleet(jobs: &[SimJob], nodes: &[Node], after: bool) -> HashMap<&'static str, Vec<i64>> {
    let mut nodes = nodes.to_vec();
    let mut waits: HashMap<&'static str, Vec<i64>> = HashMap::new();
    let mut pending: VecDeque<SimJob> = jobs.iter().cloned().collect();
    let mut held: Vec<SimJob> = Vec::new();
    let mut t = 0;
    while !(pending.is_empty() && held.is_empty()) {
        assert!(t < 72 * 3600, "the fleet simulation never drained");
        for n in &mut nodes {
            n.running.retain(|r| r.0 > t);
        }
        while pending.front().is_some_and(|j| j.q.created_at <= t) {
            held.push(pending.pop_front().unwrap());
        }
        // Oldest first, then the offer order (a stable sort).
        held.sort_by_key(|j| j.q.created_at);
        if after {
            let timed = held.iter().any(|j| j.q.est_secs.is_some());
            held.sort_by_key(|j| dispatch_key(j.q.tier, j.q.created_at, j.q.est_secs, t, timed));
        }
        // node index → (reserved for, bound seconds)
        let mut reserved: HashMap<usize, (String, u64)> = HashMap::new();
        let mut i = 0;
        while i < held.len() {
            let j = &held[i];
            let mem = j.q.mem_bytes.unwrap() / GB;
            let free_for = |k: usize, n: &Node| {
                n.starts(mem) && reserved.get(&k).is_none_or(|(id, bound)| *id == j.q.id || ends_within(j.q.est_secs, Some(*bound)))
            };
            // Most free memory first; after: a long job (30 min or more)
            // the bigger node first (placement v2's long-job key).
            let long = after && j.q.est_secs.is_some_and(|e| e >= 1800);
            let pick = nodes
                .iter()
                .enumerate()
                .filter(|(k, n)| free_for(*k, n))
                .max_by_key(|(k, n)| (if long { n.mem_gb } else { 0 }, n.mem_gb - n.used(), std::cmp::Reverse(*k)))
                .map(|(k, _)| k);
            if let Some(k) = pick {
                let j = held.remove(i);
                waits.entry(j.class).or_default().push(t - j.q.created_at);
                nodes[k].running.push((t + j.actual, mem, t, j.q.est_secs.unwrap()));
                continue;
            }
            if after && aged(&j.q, t) {
                let target = nodes
                    .iter()
                    .enumerate()
                    .filter(|(k, n)| n.mem_gb >= mem && !reserved.contains_key(k))
                    .min_by_key(|(k, n)| (n.running.len(), *k))
                    .map(|(k, n)| (k, n.shadow(t).unwrap_or(AGED_IDLE_BOUND_SECS)));
                if let Some((k, bound)) = target {
                    reserved.insert(k, (j.q.id.clone(), bound));
                }
            }
            i += 1;
        }
        t += TICK;
    }
    waits
}

/// One solo node busy most of the night: ~88 % of its slots and memory.
#[test]
fn solo_node_over_the_recorded_mix_quick_checks_wait_less_and_nothing_starves() {
    let jobs = mix(0x51ed_270b_a3c1_9e37, 8, 16, 60, 4);
    let before = run_solo(&jobs, Opts { by_duration: false });
    let after = run_solo(&jobs, Opts::default());
    report("solo node (64 GB, 4 slots)", &before, &after);
    for w in [&before, &after] {
        assert_eq!(w.values().map(Vec::len).sum::<usize>(), jobs.len(), "every job started");
    }
    let (bq, ..) = stats(&before, "quick");
    let (aq, ..) = stats(&after, "quick");
    assert!(aq < bq, "quick checks wait less: {aq:.1} vs {bq:.1} min");
    // The longest wait stays bounded: an hour, then what runs when it ages
    // (45 min +20 % at most), then a minute of slack.
    let bound = (3600 + 54 * 60 + 60) as f64 / 60.0;
    for class in ["28G", "16G", "quick"] {
        let (_, _, max) = stats(&after, class);
        assert!(max <= bound, "{class}: longest wait {max:.1} min over the bound {bound:.1}");
    }
}

/// The fleet as it was that night (a 110 GB node, two 56 GB, a 52 GB and
/// a 48 GB; 15 slots), ~95 % of its slots asked for over four hours.
/// Shorter-first costs the long jobs a little; their longest wait stays
/// bounded.
#[test]
fn fleet_over_the_recorded_mix_quick_checks_wait_less_and_the_longest_wait_is_bounded() {
    let node = |mem_gb, slots| Node { mem_gb, slots, running: Vec::new() };
    let nodes = vec![node(110, 4), node(56, 3), node(56, 3), node(52, 3), node(48, 2)];
    let jobs = mix(0x9e37_79b9_7f4a_7c15, 40, 55, 300, 4);
    let before = run_fleet(&jobs, &nodes, false);
    let after = run_fleet(&jobs, &nodes, true);
    report("fleet (110+56+56+52+48 GB, 15 slots)", &before, &after);
    for w in [&before, &after] {
        assert_eq!(w.values().map(Vec::len).sum::<usize>(), jobs.len(), "every job started");
    }
    let (bq, bqp, _) = stats(&before, "quick");
    let (aq, aqp, _) = stats(&after, "quick");
    assert!(aq < bq && aqp <= bqp, "quick checks wait less: mean {aq:.1} vs {bq:.1}, p95 {aqp:.1} vs {bqp:.1} min");
    // Bounded: an hour, then what runs on its reserved node when it ages
    // (45 min +20 % at most), then a minute of slack.
    let bound = (3600 + 54 * 60 + 60) as f64 / 60.0;
    for class in ["28G", "16G", "quick"] {
        let (_, _, max) = stats(&after, class);
        assert!(max <= bound, "{class}: longest wait {max:.1} min over the bound {bound:.1}");
    }
}
