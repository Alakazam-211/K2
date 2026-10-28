//! The trace-equivalence test (spec §8): `spec_undo::model` against the kit's sessions on
//! Yrs, after every step of seeded traces. 256 traces in the normal run; the large run is
//! `#[ignore]`d (for the compute nodes).

mod trace;

use std::collections::BTreeMap;

/// Traces run on this many threads (each has its own store).
const THREADS: u64 = 4;

fn run_many(seeds: std::ops::Range<u64>, min_per_class: u64) {
    run_many_with(seeds, trace::CLASSES, min_per_class, trace::run);
}

fn run_many_with(
    seeds: std::ops::Range<u64>,
    want: &[&'static str],
    min_per_class: u64,
    run: fn(u64) -> trace::TraceResult,
) {
    let mut classes: BTreeMap<&'static str, u64> = trace::CLASSES.iter().map(|c| (*c, 0)).collect();
    let want: Vec<&'static str> = want.to_vec();
    let mut mismatches = Vec::new();
    let (mut rr, mut rx, mut tr, mut tx, mut reverts) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut hunk_diffs = Vec::new();
    let (mut traces, mut steps) = (0u64, 0usize);
    let results: Vec<trace::TraceResult> = std::thread::scope(|sc| {
        let workers: Vec<_> = (0..THREADS)
            .map(|k| {
                let seeds = seeds.clone();
                sc.spawn(move || {
                    seeds
                        .filter(|s| s % THREADS == k)
                        .map(run)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("a trace worker panicked"))
            .collect()
    });
    for r in results {
        traces += 1;
        steps += r.steps;
        for (c, n) in r.classes {
            *classes.entry(c).or_default() += n;
        }
        rr += r.revert_regions;
        rx += r.revert_extra;
        tr += r.touching_revert_regions;
        tx += r.touching_revert_extra;
        reverts += r.reverts_checked;
        hunk_diffs.extend(r.hunk_diff);
        mismatches.extend(r.mismatch);
    }
    mismatches.sort_by_key(|m: &trace::Mismatch| m.seed);
    println!("{traces} traces, {steps} steps");
    println!("case classes (hits):");
    for (c, n) in &classes {
        println!("  {c:32} {n}");
    }
    let pct = |x: u64, n: u64| {
        if n == 0 {
            0.0
        } else {
            100.0 * x as f64 / n as f64
        }
    };
    println!(
        "U22a's cost: of {rr} revert regions ({reverts} reverts), {rx} conflict where undo of \
         the same change would apply ({:.2}%); in reverts with touching regions: {tx} of {tr} \
         ({:.2}%)",
        pct(rx, rr),
        pct(tx, tr)
    );
    println!(
        "traces whose applied hunks differ (same text; diagnostic): {}",
        hunk_diffs.len()
    );
    for d in hunk_diffs.iter().take(3) {
        println!("  {d}");
    }
    let mut labels: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for m in &mismatches {
        labels.entry(m.label).or_default().push(m.seed);
    }
    println!("mismatching traces by label:");
    for (l, seeds) in &labels {
        println!("  {:3} {l}: seeds {seeds:?}", seeds.len());
    }
    for m in &mismatches {
        println!(
            "\nMISMATCH seed {} step {} [{}]: {}",
            m.seed, m.step, m.label, m.what
        );
        for l in m.log.iter().rev().take(14).rev() {
            println!("    {l}");
        }
    }
    let missing: Vec<_> = classes
        .iter()
        .filter(|(c, n)| want.contains(c) && **n < min_per_class)
        .map(|(c, _)| *c)
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} of {traces} traces disagree with the oracle (first: seed {})",
        mismatches.len(),
        mismatches[0].seed
    );
    assert!(
        missing.is_empty(),
        "case classes under {min_per_class} hits: {missing:?}"
    );
}

/// Regression for finding F1 (fixed in v0.8.1): two participants append the same line;
/// each undoes their own.
/// Undo of the first must delete **its own** line (U10: the merge's hunks, at element
/// identity), so the second's undo finds its line and the doc is back to `x\n`.
#[test]
fn f1_each_undoes_their_own_append() {
    use trace::{edit, script, undo};
    let steps = vec![
        edit(0, 0, &[(2, 2, "a\n")]),
        edit(1, 0, &[(4, 4, "a\n")]),
        undo(0, false),
        undo(1, false),
    ];
    if let Some(m) = script(["x\n", ""], 2, steps) {
        panic!("{m}");
    }
}

/// U22a (v1.5), minimal: a change's two regions; a deletion crosses the left one's end,
/// then another deletes what separated them. Undo of the change conflicts on the left region
/// only (U9); revert also on the right one, which touches the left one's tainted end
/// (U22a's inheritance). The kit and the oracle must agree on both.
#[test]
fn u22a_revert_inherits_a_tainted_neighbour() {
    use trace::{edit, revert, script, undo};
    let steps = vec![
        edit(0, 0, &[(1, 2, "B"), (7, 8, "H")]), // change 2: "aBcdefgHij\n"
        edit(1, 0, &[(1, 3, "")]),               // across B's end: "adefgHij\n"
        edit(1, 0, &[(1, 5, "")]),               // the gap: "aHij\n", the regions touch
        undo(0, false),                          // conflicts on B's region only
        revert(0, 2),                            // both regions (U22a)
    ];
    if let Some(m) = script(["abcdefghij\n", ""], 2, steps) {
        panic!("{m}");
    }
}

/// Undo spec U22a (v1.6), restart transparency: stack-free traces, each also run on a kit
/// that never restarts; a restart at any point changes no outcome (reverts included) and
/// no text. 64 traces; the class `revert_restart_transparent` (a revert whose carry was
/// persisted, with restarts between the change and the revert and between later changes
/// on its root) must be hit.
#[test]
fn restart_transparency() {
    run_many_with(
        0..64,
        &[
            "revert_restart_transparent",
            "revert_after_restart",
            "taint",
        ],
        4,
        trace::run_twin,
    );
}

/// 256 traces, every case class hit at least once.
#[test]
fn trace_equivalence() {
    run_many(0..256, 1);
}

/// The nodes' run: traces until every class has 10^4 hits (spec §8 Volume). Not run here.
#[test]
#[ignore]
fn trace_equivalence_large() {
    run_many(1_000_000..1_000_000 + 400_000, 10_000);
}

/// Replay one trace with its whole log: `SPEC_UNDO_SEED=<n> cargo test -p k2-mpc-spec-undo
/// --test traces replay_one -- --nocapture`. Does nothing without the variable.
#[test]
fn replay_one() {
    let Some(seed) = std::env::var("SPEC_UNDO_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    else {
        return;
    };
    let r = trace::run_with(seed, true);
    match &r.mismatch {
        Some(m) => {
            for l in &m.log {
                println!("{l}");
            }
            panic!("seed {seed} step {}: {}", m.step, m.what);
        }
        None => println!("seed {seed}: {} steps, no mismatch", r.steps),
    }
}
