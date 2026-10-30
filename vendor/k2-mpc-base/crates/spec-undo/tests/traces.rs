//! The trace-equivalence test (spec §8): `spec_undo::model` against the kit's sessions on
//! Yrs, after every step of seeded traces. 256 traces in the normal run; the large run is
//! `#[ignore]`d (for the compute nodes).

mod trace;

use std::collections::BTreeMap;

/// Traces run on this many threads (each has its own store).
const THREADS: u64 = 4;

fn run_many(name: &str, seeds: std::ops::Range<u64>, min_per_class: u64) {
    run_many_with(name, seeds, trace::CLASSES, min_per_class, trace::run);
}

/// Run `seeds` through `run`; print the summary and one `RUNREC` line named `name` (the
/// traces, steps, mismatching traces, each wanted class's hits and the U10a-on-the-kit
/// counts); fail on any mismatch or a wanted class under `min_per_class` hits.
fn run_many_with(
    name: &str,
    seeds: std::ops::Range<u64>,
    want: &[&'static str],
    min_per_class: u64,
    run: fn(u64) -> trace::TraceResult,
) {
    let mut classes: BTreeMap<&'static str, u64> =
        trace::CLASSES.iter().chain(want).map(|c| (*c, 0)).collect();
    let want: Vec<&'static str> = want.to_vec();
    let mut mismatches = Vec::new();
    let (mut rr, mut rx, mut tr, mut tx, mut reverts) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut hunk_diffs = Vec::new();
    let (mut traces, mut steps) = (0u64, 0usize);
    let mut restore_parts: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut own_only: BTreeMap<&'static str, u64> = ["own_only_checked", "own_only_unresolved"]
        .into_iter()
        .map(|k| (k, 0))
        .collect();
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
        for (k, n) in r.restore_parts {
            *restore_parts.entry(k).or_default() += n;
        }
        for (k, n) in r.own_only {
            *own_only.entry(k).or_default() += n;
        }
        mismatches.extend(r.mismatch);
    }
    mismatches.sort_by_key(|m: &trace::Mismatch| m.seed);
    println!("{traces} traces, {steps} steps");
    println!("case classes (hits):");
    for (c, n) in &classes {
        println!("  {c:32} {n}");
    }
    println!("concurrent restores of one root, by part (v1.8): {restore_parts:?}");
    println!("U10a by root on the kit (v1.8): {own_only:?}");
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
        "traces whose applied hunks differ (a mismatch since v1.7): {}",
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
    let mut fields: Vec<String> = vec![
        format!("\"traces\":{traces}"),
        format!("\"steps\":{steps}"),
        format!("\"mismatches\":{}", mismatches.len()),
    ];
    for c in &want {
        fields.push(format!("\"{c}\":{}", classes.get(c).copied().unwrap_or(0)));
    }
    for (k, n) in &own_only {
        fields.push(format!("\"{k}\":{n}"));
    }
    println!(
        "RUNREC {{\"job\":null,\"run\":\"{name}\",\"tool\":\"spec-undo\",\"status\":\"{}\",{}}}",
        if mismatches.is_empty() {
            "COMPLETE"
        } else {
            "VIOLATION"
        },
        fields.join(",")
    );
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

/// The trace count: `SPEC_UNDO_TRACES`, else `default`.
fn traces_or(default: u64) -> u64 {
    std::env::var("SPEC_UNDO_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
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
        "restart_transparency",
        0..traces_or(64),
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
///
/// `SPEC_UNDO_TRACES=<n>` runs `n` traces instead (here and in [`restart_transparency`]):
/// a quick local look; the class minimum still applies.
#[test]
fn trace_equivalence() {
    run_many("trace_equivalence", 0..traces_or(256), 1);
}

/// The nodes' run: traces until every class has 10^4 hits (spec §8 Volume). Not run here.
#[test]
#[ignore]
fn trace_equivalence_large() {
    run_many(
        "trace_equivalence_large",
        1_000_000..1_000_000 + 400_000,
        10_000,
    );
}

/// D1's run on the nodes (v0.9.7, undo spec v1.10): 50,000 traces of the main class mix,
/// D1's classes (`d1_widened`, `d1_widened_then_undone`) among them. Not run here.
#[test]
#[ignore]
fn d1_traces_large() {
    run_many_with(
        "d1_traces_large",
        2_000_000..2_050_000,
        trace::CLASSES,
        1,
        trace::run,
    );
}

fn group_trace(seed: u64) -> trace::TraceResult {
    trace::run_group(seed, false)
}

/// The grouping class (undo spec U3, §8 Volume "grouping with the (g) split"): people typing
/// with group tokens on the hunk channel, interleaved with others' changes, idle gaps, undos
/// and redos, against the oracle's grouping (`Model::edit_grouped`), after every step. 256
/// traces (`SPEC_UNDO_GROUP_TRACES=<n>` for another count), every grouping class hit.
#[test]
fn group_trace_equivalence() {
    let n = std::env::var("SPEC_UNDO_GROUP_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    run_many_with("group_traces", 0..n, trace::GROUP_CLASSES, 1, group_trace);
}

/// The nodes' run of the grouping class: 10^4 traces, every grouping class hit at least 100
/// times. Not run here.
#[test]
#[ignore]
fn group_trace_equivalence_large() {
    run_many_with(
        "group_traces_large",
        3_000_000..3_010_000,
        trace::GROUP_CLASSES,
        100,
        group_trace,
    );
}

/// The kit's three planted grouping faults (`differential::GroupMutant`, K4's) are each caught
/// by the grouping class within `GROUP_MUTANT_TRACES` traces (default 256), and the catching
/// trace passes without its fault.
#[test]
fn group_mutants_are_caught() {
    use k2_mpc_base::differential::{with_group_mutant, GroupMutant};
    let n: u64 = std::env::var("GROUP_MUTANT_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    for (m, name) in [
        (GroupMutant::NeverClosesOnForeign, "never_closes_on_foreign"),
        (GroupMutant::NeverClosesOnIdle, "never_closes_on_idle"),
        (
            GroupMutant::NeverClosesOnBoundary,
            "never_closes_on_boundary",
        ),
    ] {
        let caught = (0..n).find_map(|seed| {
            with_group_mutant(m, || trace::run_group(seed, false))
                .mismatch
                .map(|x| (seed, x))
        });
        let (seed, mm) = caught.unwrap_or_else(|| panic!("{name}: never caught in {n} traces"));
        if let Some(x) = trace::run_group(seed, false).mismatch {
            panic!(
                "{name}: trace {seed} fails without the fault too: {}",
                x.what
            );
        }
        println!(
            "[group mutant {name}] caught by trace {seed}, step {}: {}",
            mm.step, mm.what
        );
        println!(
            "RUNREC {{\"job\":null,\"run\":\"group_mutant_{name}\",\"tool\":\"spec-undo\",\"status\":\"CAUGHT\",\"seed\":{seed},\"traces\":{}}}",
            seed + 1
        );
    }
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
    // SPEC_UNDO_GROUP=1: a grouping trace
    let r = if std::env::var("SPEC_UNDO_GROUP").is_ok() {
        trace::run_group(seed, true)
    } else {
        trace::run_with(seed, true)
    };
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
