//! The trace-equivalence test (spec §8): `spec_undo::model` against the kit's sessions on
//! Yrs, after every step of seeded traces. 256 traces in the normal run; the large run is
//! `#[ignore]`d (for the compute nodes).

mod trace;

use std::collections::BTreeMap;

/// Traces run on this many threads (each has its own store).
const THREADS: u64 = 4;

fn run_many(name: &str, seeds: std::ops::Range<u64>, min_per_class: u64, floors: &Floors) {
    let (conflicts, regions) = run_many_with(
        name,
        seeds,
        trace::CLASSES,
        min_per_class,
        trace::run,
        floors,
    );
    // the comparison of write --base's conflict regions was reached
    assert!(
        conflicts > 0 && regions > 0,
        "no write --base conflict regions compared ({conflicts} conflicts, {regions} regions)"
    );
}

/// Run `seeds` through `run`; print the summary and one `RUNREC` line named `name` (the
/// traces, steps, mismatching traces, each wanted class's hits and the U10a-on-the-kit
/// counts, and the refused `write --base` steps whose conflict regions were compared, with
/// their regions); fail on any mismatch or a wanted class under `min_per_class` hits.
/// Returns those two counts.
fn run_many_with(
    name: &str,
    seeds: std::ops::Range<u64>,
    want: &[&'static str],
    min_per_class: u64,
    run: fn(u64) -> trace::TraceResult,
    floors: &Floors,
) -> (u64, u64) {
    let seeds_len = seeds.end - seeds.start;
    let mut classes: BTreeMap<&'static str, u64> =
        trace::CLASSES.iter().chain(want).map(|c| (*c, 0)).collect();
    let want: Vec<&'static str> = want.to_vec();
    let mut mismatches = Vec::new();
    let (mut rr, mut rx, mut tr, mut tx, mut reverts) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut hunk_diffs = Vec::new();
    let (mut traces, mut steps) = (0u64, 0usize);
    let (mut wb_conflicts, mut wb_regions) = (0u64, 0u64);
    let (mut wb_multi, mut wb_multi_regions) = (0u64, 0u64);
    let mut wb_labels: BTreeMap<String, u64> = [trace::WB_EXACT, trace::WB_COALESCED]
        .into_iter()
        .map(|k| (k.to_owned(), 0))
        .collect();
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
        wb_conflicts += r.write_conflicts;
        wb_regions += r.write_regions;
        wb_multi += r.write_multi;
        wb_multi_regions += r.write_multi_regions;
        for (k, n) in r.write_labels {
            *wb_labels.entry(k).or_default() += n;
        }
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
    println!(
        "write --base conflict regions compared (root, base range, lines in base, ours, \
         theirs): {wb_conflicts} refused writes, {wb_regions} regions"
    );
    println!("write --base regions by label (kit against the oracle): {wb_labels:?}");
    println!(
        "write --base refused in several separate regions (oracle, no two touching): \
         {wb_multi} writes, {wb_multi_regions} regions"
    );
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
    fields.push(format!("\"write_base_conflicts_compared\":{wb_conflicts}"));
    fields.push(format!("\"write_base_regions_compared\":{wb_regions}"));
    let wb_other: u64 = wb_labels
        .iter()
        .filter(|(k, _)| k.starts_with("other"))
        .map(|(_, n)| n)
        .sum();
    fields.push(format!("\"wb_exact\":{}", wb_labels[trace::WB_EXACT]));
    fields.push(format!(
        "\"wb_coalesced_touching\":{}",
        wb_labels[trace::WB_COALESCED]
    ));
    fields.push(format!("\"wb_other\":{wb_other}"));
    fields.push(format!("\"wb_multi_separate\":{wb_multi}"));
    fields.push(format!("\"wb_multi_separate_regions\":{wb_multi_regions}"));
    println!(
        "RUNREC {{\"job\":null,\"run\":\"{name}\",\"tool\":\"spec-undo\",\"status\":\"{}\",{}}}",
        if mismatches.is_empty() {
            "COMPLETE"
        } else {
            "VIOLATION"
        },
        fields.join(",")
    );
    // every refused write a class counts is one compared (the runner checks each step's side);
    // the two classes never meet (an empty base against a whole base line): so the compared
    // count is at least their sum. `write_base_shared_start_was_conflict` counts clean writes
    // only, never one compared: no relation to assert. Asserted with the floors below.
    let refused_classes =
        classes["write_base_words_same_line"] + classes["empty_base_double_write"];
    println!(
        "write --base refused writes compared {wb_conflicts} >= write_base_words_same_line + \
         empty_base_double_write {refused_classes}"
    );
    // ---- the floors: every count, asserted at or above its floor (one assert, at the end) ----
    let n = seeds_len;
    let mut counts: Vec<(String, u64, u64)> = vec![
        ("traces".into(), traces, plant(n)),
        ("steps".into(), steps as u64, floors.at("steps", n)),
        (
            "wb_conflicts_compared >= words_same_line + empty_base_double_write".into(),
            wb_conflicts,
            plant(refused_classes),
        ),
    ];
    for (c, v) in &classes {
        let f = floors.at(c, n);
        let f = if want.contains(c) {
            f.max(plant(min_per_class))
        } else {
            f
        };
        counts.push(((*c).into(), *v, f));
    }
    for k in ["base_by_root", "o4_restore", "o4a"] {
        restore_parts.entry(k).or_default();
    }
    for (k, v) in &restore_parts {
        let k = format!("restore_part:{k}");
        counts.push((k.clone(), *v, floors.at(&k, n)));
    }
    for k in [
        "own_only_checked",
        "own_only_removed",
        "own_only_removed_root_author_not_actor",
        "own_only_unresolved",
    ] {
        own_only.entry(k).or_default();
    }
    for (k, v) in &own_only {
        counts.push(((*k).into(), *v, floors.at(k, n)));
    }
    for (k, v) in [
        ("reverts_checked", reverts),
        ("revert_regions", rr),
        ("revert_extra", rx),
        ("touching_revert_regions", tr),
        ("touching_revert_extra", tx),
        ("wb_conflicts_compared", wb_conflicts),
        ("wb_regions_compared", wb_regions),
        ("wb_exact", wb_labels[trace::WB_EXACT]),
        ("wb_coalesced_touching", wb_labels[trace::WB_COALESCED]),
        ("wb_multi_separate", wb_multi),
        ("wb_multi_separate_regions", wb_multi_regions),
    ] {
        counts.push((k.into(), v, floors.at(k, n)));
    }
    println!(
        "floors ({n} traces; stated for {} traces, scaled):",
        floors.per
    );
    let mut unmet = Vec::new();
    for (k, v, f) in &counts {
        let ok = *v >= *f && *f != NO_FLOOR;
        println!(
            "  FLOOR {k:70} {v:>9} >= {:>9} {}",
            if *f == NO_FLOOR {
                "none".into()
            } else {
                f.to_string()
            },
            if ok { "ok" } else { "UNMET" }
        );
        if !ok {
            unmet.push(format!("{k} {v} < {f}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {traces} traces disagree with the oracle (first: seed {})",
        mismatches.len(),
        mismatches[0].seed
    );
    assert!(unmet.is_empty(), "{} floors unmet: {unmet:?}", unmet.len());
    (wb_conflicts, wb_regions)
}

/// A floor no table states (`Floors::at`): printed as `none` and failing, so every count
/// must have one.
const NO_FLOOR: u64 = u64::MAX;

/// The floors' plant multiplier (1: none).
const FLOOR_PLANT: u64 = 1;

/// A floor under the plant: as it is when `FLOOR_PLANT` is 1.
fn plant(f: u64) -> u64 {
    if FLOOR_PLANT == 1 {
        f
    } else {
        f.max(1) * FLOOR_PLANT
    }
}

/// A run's floors: each count's floor at `per` traces, scaled to the run's traces (rounded
/// up; at least 1 when the stated floor is). A floor guards against a vacuous run, not
/// against drift: each is half of what the default run (its fixed seeds, at `per` traces)
/// gives, rounded down, at least 1 (a rare class too); 0 only for a count that run gives 0
/// (its generator can't make it: a stack-free trace has no undo, grouping traces no `write
/// --base`, revert or restart; or not reached, the twin's `concurrent_restore_o4a`), or for a
/// cost or a diagnostic (a revert's extra conflicts, unresolved U10a checks, touching
/// coalescences of `write --base` regions).
struct Floors {
    per: u64,
    /// `of` holds measured counts (rates per `per` traces), not floors: a floor is half the
    /// count expected at the run's traces, rounded down, at least 1 where the rate is above 0
    rates: bool,
    of: &'static [(&'static str, u64)],
}

impl Floors {
    fn at(&self, name: &str, n: u64) -> u64 {
        let Some(&(_, f)) = self.of.iter().find(|(k, _)| *k == name) else {
            return NO_FLOOR;
        };
        let f = if f == 0 {
            0
        } else if self.rates {
            // half the count expected at n traces, rounded down, at least 1
            ((f * n) / (2 * self.per)).max(1)
        } else {
            ((f * n).div_ceil(self.per)).max(1)
        };
        plant(f)
    }
}

/// `trace::run`'s floors, at 256 traces: half of what seeds 0..256 give (28632 steps; the
/// write --base refused writes 790, regions 1107, 227 refused in separate regions with 544).
const MAIN: Floors = Floors {
    per: 256,
    rates: false,
    of: &[
        ("steps", 14316),
        ("append_at_end", 185),
        ("insert_inside_another_range", 21),
        ("multi_hunk_partial", 28),
        ("stays_on_top", 108),
        ("redo_after_partial", 2),
        ("revert_after_restart", 203),
        ("undo_across_purge", 98),
        ("multi_connection_pid", 166),
        ("empty_base_double_write", 60),
        ("last_line_delete_after_append", 93),
        ("whole_doc_delete_then_append", 83),
        ("touching_regions_one_step", 101),
        ("revert_beyond_compaction", 439),
        ("revert_outside_window", 76),
        ("taint", 402),
        ("revert_over_erased", 20),
        ("undo_for_person", 34),
        ("steps_n", 85),
        ("undo_drop", 41),
        ("history_lost_after_restart", 153),
        ("multi_root_batch", 40),
        ("repeated_text_identity", 168),
        ("revert_restart_transparent", 22),
        ("exact_sides_differ_from_rediff", 751),
        ("concurrent_restores_of_one_root", 82),
        ("concurrent_restore_o4", 20),
        ("concurrent_restore_o4a", 7),
        ("concurrent_restore_base_by_root", 55),
        ("d1_widened", 2808),
        ("d1_widened_then_undone", 1215),
        ("end_break_a", 43),
        ("end_break_b", 38),
        ("end_break_c", 36),
        ("end_break_d", 38),
        ("write_base_shared_start", 165),
        ("write_base_shared_start_was_conflict", 39),
        ("write_base_words_same_line", 243),
        ("write_base_multiline_rewrite", 71),
        ("restore_part:base_by_root", 55),
        ("restore_part:o4_restore", 20),
        ("restore_part:o4a", 7),
        ("own_only_checked", 1214),
        ("own_only_removed", 8545),
        ("own_only_removed_root_author_not_actor", 1333),
        ("own_only_unresolved", 0),
        ("reverts_checked", 649),
        ("revert_regions", 857),
        ("revert_extra", 0),
        ("touching_revert_regions", 117),
        ("touching_revert_extra", 0),
        ("wb_conflicts_compared", 395),
        ("wb_regions_compared", 553),
        ("wb_exact", 395),
        ("wb_coalesced_touching", 0),
        ("wb_multi_separate", 113),
        ("wb_multi_separate_regions", 272),
    ],
};

/// `trace::run_twin`'s floors (restart transparency, stack-free, with its own scenario
/// weight), at 128 traces: half of what seeds 0..128 give (13538 steps; write --base 211
/// compared, 297 regions, 63 refused in separate regions with 149). Undo-only classes are 0.
const TWIN: Floors = Floors {
    per: 128,
    rates: false,
    of: &[
        ("steps", 6769),
        ("append_at_end", 48),
        ("insert_inside_another_range", 7),
        ("multi_hunk_partial", 0),
        ("stays_on_top", 0),
        ("redo_after_partial", 0),
        ("revert_after_restart", 677),
        ("undo_across_purge", 0),
        ("multi_connection_pid", 0),
        ("empty_base_double_write", 21),
        ("last_line_delete_after_append", 13),
        ("whole_doc_delete_then_append", 7),
        ("touching_regions_one_step", 72),
        ("revert_beyond_compaction", 798),
        ("revert_outside_window", 46),
        ("taint", 262),
        ("revert_over_erased", 19),
        ("undo_for_person", 0),
        ("steps_n", 0),
        ("undo_drop", 0),
        ("history_lost_after_restart", 0),
        ("multi_root_batch", 0),
        ("repeated_text_identity", 119),
        ("revert_restart_transparent", 69),
        ("exact_sides_differ_from_rediff", 244),
        ("concurrent_restores_of_one_root", 73),
        ("concurrent_restore_o4", 36),
        ("concurrent_restore_o4a", 2),
        ("concurrent_restore_base_by_root", 35),
        ("d1_widened", 931),
        ("d1_widened_then_undone", 472),
        ("end_break_a", 21),
        ("end_break_b", 18),
        ("end_break_c", 11),
        ("end_break_d", 13),
        ("write_base_shared_start", 36),
        ("write_base_shared_start_was_conflict", 6),
        ("write_base_words_same_line", 64),
        ("write_base_multiline_rewrite", 16),
        ("restore_part:base_by_root", 35),
        ("restore_part:o4_restore", 36),
        ("restore_part:o4a", 2),
        ("own_only_checked", 630),
        ("own_only_removed", 1709),
        ("own_only_removed_root_author_not_actor", 921),
        ("own_only_unresolved", 0),
        ("reverts_checked", 1058),
        ("revert_regions", 1229),
        ("revert_extra", 0),
        ("touching_revert_regions", 152),
        ("touching_revert_extra", 0),
        ("wb_conflicts_compared", 105),
        ("wb_regions_compared", 148),
        ("wb_exact", 105),
        ("wb_coalesced_touching", 0),
        ("wb_multi_separate", 31),
        ("wb_multi_separate_regions", 74),
    ],
};

/// The grouping traces' floors, at 256 traces: half of what seeds 0..256 give (25188 steps).
/// They make no `write --base`, revert, purge or restart: those counts are 0.
const GROUP: Floors = Floors {
    per: 256,
    rates: false,
    of: &[
        ("steps", 12594),
        ("append_at_end", 12),
        ("insert_inside_another_range", 9),
        ("multi_hunk_partial", 1),
        ("stays_on_top", 33),
        ("redo_after_partial", 0),
        ("revert_after_restart", 0),
        ("undo_across_purge", 0),
        ("multi_connection_pid", 155),
        ("empty_base_double_write", 0),
        ("last_line_delete_after_append", 12),
        ("whole_doc_delete_then_append", 1),
        ("touching_regions_one_step", 1),
        ("revert_beyond_compaction", 0),
        ("revert_outside_window", 0),
        ("taint", 127),
        ("revert_over_erased", 0),
        ("undo_for_person", 0),
        ("steps_n", 78),
        ("undo_drop", 0),
        ("history_lost_after_restart", 39),
        ("multi_root_batch", 0),
        ("repeated_text_identity", 0),
        ("revert_restart_transparent", 0),
        ("exact_sides_differ_from_rediff", 504),
        ("concurrent_restores_of_one_root", 20),
        ("concurrent_restore_o4", 0),
        ("concurrent_restore_o4a", 0),
        ("concurrent_restore_base_by_root", 20),
        ("d1_widened", 3032),
        ("d1_widened_then_undone", 1287),
        ("end_break_a", 0),
        ("end_break_b", 0),
        ("end_break_c", 0),
        ("end_break_d", 0),
        ("write_base_shared_start", 0),
        ("write_base_shared_start_was_conflict", 0),
        ("write_base_words_same_line", 0),
        ("write_base_multiline_rewrite", 0),
        ("group_joined", 878),
        ("group_closed_idle", 47),
        ("group_closed_noncontiguous", 235),
        ("group_closed_boundary", 338),
        ("group_closed_newline", 162),
        ("group_closed_token", 109),
        ("group_closed_foreign", 216),
        ("group_self_undone", 54),
        ("group_undone", 148),
        ("group_redone", 9),
        ("group_agent_token", 296),
        ("restore_part:base_by_root", 20),
        ("restore_part:o4_restore", 0),
        ("restore_part:o4a", 0),
        ("own_only_checked", 1275),
        ("own_only_removed", 3732),
        ("own_only_removed_root_author_not_actor", 221),
        ("own_only_unresolved", 0),
        ("reverts_checked", 0),
        ("revert_regions", 0),
        ("revert_extra", 0),
        ("touching_revert_regions", 0),
        ("touching_revert_extra", 0),
        ("wb_conflicts_compared", 0),
        ("wb_regions_compared", 0),
        ("wb_exact", 0),
        ("wb_coalesced_touching", 0),
        ("wb_multi_separate", 0),
        ("wb_multi_separate_regions", 0),
    ],
};

/// `trace_equivalence_large`'s rates, seeds 1,000,000..1,004,000 (kit 2bbe1e50): the counts job
/// 301c3906 measured in 4000 traces (a RATE, not a floor: each floor is half the count expected at
/// the traces a run checks, `Floors::at`; 0 where the generator gives none, and for the costs and
/// diagnostics, as in the tables above).
const LARGE_MAIN: Floors = Floors {
    per: 4000,
    rates: true,
    of: &[
        ("steps", 444910),
        ("append_at_end", 5487),
        ("concurrent_restore_base_by_root", 1699),
        ("concurrent_restore_o4", 807),
        ("concurrent_restore_o4a", 155),
        ("concurrent_restores_of_one_root", 2647),
        ("d1_widened", 86771),
        ("d1_widened_then_undone", 38278),
        ("empty_base_double_write", 2148),
        ("end_break_a", 1525),
        ("end_break_b", 1245),
        ("end_break_c", 1114),
        ("end_break_d", 1142),
        ("exact_sides_differ_from_rediff", 24223),
        ("history_lost_after_restart", 5231),
        ("insert_inside_another_range", 604),
        ("last_line_delete_after_append", 2581),
        ("multi_connection_pid", 5939),
        ("multi_hunk_partial", 956),
        ("multi_root_batch", 1327),
        ("redo_after_partial", 50),
        ("repeated_text_identity", 6217),
        ("revert_after_restart", 7076),
        ("revert_beyond_compaction", 14751),
        ("revert_outside_window", 1965),
        ("revert_over_erased", 761),
        ("revert_restart_transparent", 711),
        ("stays_on_top", 3830),
        ("steps_n", 2454),
        ("taint", 13443),
        ("touching_regions_one_step", 3437),
        ("undo_across_purge", 3265),
        ("undo_drop", 1667),
        ("undo_for_person", 1160),
        ("whole_doc_delete_then_append", 2617),
        ("write_base_multiline_rewrite", 2082),
        ("write_base_shared_start", 4973),
        ("write_base_shared_start_was_conflict", 921),
        ("write_base_words_same_line", 6713),
        ("restore_part:base_by_root", 1699),
        ("restore_part:o4_restore", 807),
        ("restore_part:o4a", 155),
        ("own_only_checked", 38557),
        ("own_only_removed", 265585),
        ("own_only_removed_root_author_not_actor", 39019),
        ("own_only_unresolved", 0),
        ("reverts_checked", 21619),
        ("revert_regions", 28434),
        ("revert_extra", 0),
        ("touching_revert_regions", 3680),
        ("touching_revert_extra", 0),
        ("wb_conflicts_compared", 11674),
        ("wb_regions_compared", 16039),
        ("wb_exact", 11674),
        ("wb_coalesced_touching", 0),
        ("wb_multi_separate", 3157),
        ("wb_multi_separate_regions", 7522),
    ],
};

/// `d1_traces_large`'s and `end_break_trace_class_large`'s rates (one generator, one seed range),
/// seeds 2,000,000..2,004,000 (kit 2bbe1e50): the counts job 841f1cba measured in 4000 traces (a
/// RATE, not a floor: each floor is half the count expected at the traces a run checks,
/// `Floors::at`; 0 where the generator gives none, and for the costs and diagnostics, as in the
/// tables above).
const LARGE_D1: Floors = Floors {
    per: 4000,
    rates: true,
    of: &[
        ("steps", 437549),
        ("append_at_end", 5431),
        ("concurrent_restore_base_by_root", 1743),
        ("concurrent_restore_o4", 837),
        ("concurrent_restore_o4a", 154),
        ("concurrent_restores_of_one_root", 2715),
        ("d1_widened", 86443),
        ("d1_widened_then_undone", 37883),
        ("empty_base_double_write", 2099),
        ("end_break_a", 1440),
        ("end_break_b", 1209),
        ("end_break_c", 1135),
        ("end_break_d", 1063),
        ("exact_sides_differ_from_rediff", 24236),
        ("history_lost_after_restart", 4865),
        ("insert_inside_another_range", 583),
        ("last_line_delete_after_append", 2597),
        ("multi_connection_pid", 6140),
        ("multi_hunk_partial", 981),
        ("multi_root_batch", 1401),
        ("redo_after_partial", 63),
        ("repeated_text_identity", 6462),
        ("revert_after_restart", 6733),
        ("revert_beyond_compaction", 14478),
        ("revert_outside_window", 1921),
        ("revert_over_erased", 682),
        ("revert_restart_transparent", 618),
        ("stays_on_top", 3882),
        ("steps_n", 2470),
        ("taint", 13205),
        ("touching_regions_one_step", 3305),
        ("undo_across_purge", 3085),
        ("undo_drop", 1567),
        ("undo_for_person", 1187),
        ("whole_doc_delete_then_append", 2600),
        ("write_base_multiline_rewrite", 2040),
        ("write_base_shared_start", 5223),
        ("write_base_shared_start_was_conflict", 1024),
        ("write_base_words_same_line", 6757),
        ("restore_part:base_by_root", 1743),
        ("restore_part:o4_restore", 837),
        ("restore_part:o4a", 154),
        ("own_only_checked", 38257),
        ("own_only_removed", 263877),
        ("own_only_removed_root_author_not_actor", 38623),
        ("own_only_unresolved", 0),
        ("reverts_checked", 21202),
        ("revert_regions", 27832),
        ("revert_extra", 0),
        ("touching_revert_regions", 3496),
        ("touching_revert_extra", 0),
        ("wb_conflicts_compared", 11597),
        ("wb_regions_compared", 15949),
        ("wb_exact", 11597),
        ("wb_coalesced_touching", 0),
        ("wb_multi_separate", 3142),
        ("wb_multi_separate_regions", 7494),
    ],
};

/// `group_trace_equivalence_large`'s rates, seeds 3,000,000..3,002,000 (kit 2bbe1e50): the counts
/// job f45b9156 measured in 2000 traces (a RATE, not a floor: each floor is half the count expected
/// at the traces a run checks, `Floors::at`; 0 where the generator gives none, and for the costs
/// and diagnostics, as in the tables above).
const LARGE_GROUP: Floors = Floors {
    per: 2000,
    rates: true,
    of: &[
        ("steps", 201714),
        ("append_at_end", 200),
        ("concurrent_restore_base_by_root", 261),
        ("concurrent_restore_o4", 0),
        ("concurrent_restore_o4a", 0),
        ("concurrent_restores_of_one_root", 261),
        ("d1_widened", 49578),
        ("d1_widened_then_undone", 19770),
        ("empty_base_double_write", 0),
        ("end_break_a", 0),
        ("end_break_b", 0),
        ("end_break_c", 0),
        ("end_break_d", 0),
        ("exact_sides_differ_from_rediff", 8069),
        ("group_agent_token", 5052),
        ("group_closed_boundary", 5437),
        ("group_closed_foreign", 3552),
        ("group_closed_idle", 736),
        ("group_closed_newline", 2406),
        ("group_closed_noncontiguous", 4026),
        ("group_closed_token", 1936),
        ("group_joined", 14431),
        ("group_redone", 245),
        ("group_self_undone", 946),
        ("group_undone", 2376),
        ("history_lost_after_restart", 694),
        ("insert_inside_another_range", 125),
        ("last_line_delete_after_append", 197),
        ("multi_connection_pid", 2328),
        ("multi_hunk_partial", 39),
        ("multi_root_batch", 0),
        ("redo_after_partial", 3),
        ("repeated_text_identity", 0),
        ("revert_after_restart", 0),
        ("revert_beyond_compaction", 0),
        ("revert_outside_window", 0),
        ("revert_over_erased", 0),
        ("revert_restart_transparent", 0),
        ("stays_on_top", 671),
        ("steps_n", 1225),
        ("taint", 2059),
        ("touching_regions_one_step", 32),
        ("undo_across_purge", 0),
        ("undo_drop", 0),
        ("undo_for_person", 0),
        ("whole_doc_delete_then_append", 6),
        ("write_base_multiline_rewrite", 0),
        ("write_base_shared_start", 0),
        ("write_base_shared_start_was_conflict", 0),
        ("write_base_words_same_line", 0),
        ("restore_part:base_by_root", 261),
        ("restore_part:o4_restore", 0),
        ("restore_part:o4a", 0),
        ("own_only_checked", 19753),
        ("own_only_removed", 58770),
        ("own_only_removed_root_author_not_actor", 2783),
        ("own_only_unresolved", 0),
        ("reverts_checked", 0),
        ("revert_regions", 0),
        ("revert_extra", 0),
        ("touching_revert_regions", 0),
        ("touching_revert_extra", 0),
        ("wb_conflicts_compared", 0),
        ("wb_regions_compared", 0),
        ("wb_exact", 0),
        ("wb_coalesced_touching", 0),
        ("wb_multi_separate", 0),
        ("wb_multi_separate_regions", 0),
    ],
};

/// The trace count: `SPEC_UNDO_TRACES`, else `default`.
fn traces_or(default: u64) -> u64 {
    std::env::var("SPEC_UNDO_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// A large run's seeds and class minimum, or a part of them: `SPEC_UNDO_SEEDS=START..END`
/// (tools/undo/traces-large.sh, one node job per part) runs the seeds START..END of `range`
/// (inside it, or the test fails), named `NAME_START_END`, its class minimum scaled to the
/// part (rounded up), so parts that cover `range` assert at least the whole run's minimum
/// between them. The floors scale to the traces run as always (`Floors::at`).
fn large_part(
    name: &str,
    range: std::ops::Range<u64>,
    min_per_class: u64,
) -> (String, std::ops::Range<u64>, u64) {
    let Ok(s) = std::env::var("SPEC_UNDO_SEEDS") else {
        return (name.to_owned(), range, min_per_class);
    };
    let (a, b) = s
        .split_once("..")
        .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)))
        .unwrap_or_else(|| panic!("SPEC_UNDO_SEEDS={s}: START..END"));
    assert!(
        range.start <= a && a < b && b <= range.end,
        "SPEC_UNDO_SEEDS={s}: not a part of {name}'s seeds {range:?}"
    );
    let min = (min_per_class * (b - a)).div_ceil(range.end - range.start);
    (format!("{name}_{a}_{b}"), a..b, min)
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

/// Regression (job f77bab22, `trace_equivalence_large` seed 1001280, step 53): a refused
/// `write --base` whose region holds a live insertion that clashes with nothing. The
/// region's lines in the live text count it (live lines 6 to 15); the kit said 6 to 13, the
/// oracle 6 to 11.
#[test]
fn regression_write_base_lines_seed_1001280() {
    let r = trace::run(1_001_280);
    assert!(
        r.mismatch.is_none(),
        "{}",
        r.mismatch.map_or(String::new(), |m| m.what)
    );
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
/// no text. 128 traces (64 before v0.11.1, whose small classes moved by half with any
/// change to the generator); the class `revert_restart_transparent` (a revert whose carry
/// was persisted, with restarts between the change and the revert and between later
/// changes on its root) must be hit.
#[test]
fn restart_transparency() {
    run_many_with(
        "restart_transparency",
        0..traces_or(128),
        &[
            "revert_restart_transparent",
            "revert_after_restart",
            "taint",
        ],
        4,
        trace::run_twin,
        &TWIN,
    );
}

/// 256 traces, every case class hit at least once.
///
/// `SPEC_UNDO_TRACES=<n>` runs `n` traces instead (here and in [`restart_transparency`]):
/// a quick local look; the class minimum still applies.
#[test]
fn trace_equivalence() {
    run_many("trace_equivalence", 0..traces_or(256), 1, &MAIN);
}

/// The nodes' run (spec §8 Volume): 400,000 traces, seeds 1,000,000..1,400,000, in parts
/// (tools/undo/traces-large.sh PART=main1..main5). Each class at least half its measured
/// rate (`LARGE_MAIN`, job 301c3906) at the traces a part checks: over the 400,000, enforced
/// minimums of about 2,500 for `redo_after_partial` and about 7,750 for
/// `concurrent_restore_o4a` (and its `restore_part:o4a`), every other class above 10^4.
/// Not run here.
#[test]
#[ignore]
fn trace_equivalence_large() {
    let (name, seeds, min) = large_part("trace_equivalence_large", 1_000_000..1_400_000, 1);
    run_many(&name, seeds, min, &LARGE_MAIN);
}

/// D1's run on the nodes (v0.9.7, undo spec v1.10): 50,000 traces of the main class mix,
/// D1's classes (`d1_widened`, `d1_widened_then_undone`) among them, each class at least half
/// its measured rate (`LARGE_D1`, job 841f1cba). One job (tools/undo/traces-large.sh
/// PART=d1). Not run here.
#[test]
#[ignore]
fn d1_traces_large() {
    let (name, seeds, min) = large_part("d1_traces_large", 2_000_000..2_050_000, 1);
    run_many_with(&name, seeds, trace::CLASSES, min, trace::run, &LARGE_D1);
}

/// Seeds of `d1_traces_large` whose class counts differed between two runs of the same
/// seeds (jobs 89adae85 and 05dcd2f2, a scan of 2,000,000..2,050,000 run twice): the
/// session's client IDs, drawn at random, change the sizes compaction reads (an ID's varint
/// length in the log, or the IDs' order in the snapshot's encoding), so a compaction moved
/// by a step and `revert_beyond_compaction` with it.
const REPEAT_SEEDS: [u64; 11] = [
    2_002_488, 2_004_968, 2_005_922, 2_006_543, 2_008_968, 2_011_790, 2_015_468, 2_015_894,
    2_017_200, 2_018_420, 2_040_571,
];

/// A trace is a function of its seed: [`REPEAT_SEEDS`] run `REPEAT_RUNS` times (default 8)
/// give the same steps, class counts and other counts every time (the trace runner draws
/// the kit's client IDs from the seed).
#[test]
fn trace_counts_repeat() {
    let runs: usize = std::env::var("REPEAT_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let counts = |r: &trace::TraceResult| {
        format!(
            "steps {} mismatch {:?} classes {:?} reverts {} {} {} {} {} restores {:?} \
             own_only {:?} write_base {} {} {:?} {} {}",
            r.steps,
            r.mismatch.as_ref().map(|m| (m.step, &m.what)),
            r.classes,
            r.revert_regions,
            r.revert_extra,
            r.touching_revert_regions,
            r.touching_revert_extra,
            r.reverts_checked,
            r.restore_parts,
            r.own_only,
            r.write_conflicts,
            r.write_regions,
            r.write_labels,
            r.write_multi,
            r.write_multi_regions
        )
    };
    let mut differ = Vec::new();
    for seed in REPEAT_SEEDS {
        let first = counts(&trace::run(seed));
        for k in 1..runs {
            let again = counts(&trace::run(seed));
            if again != first {
                differ.push(format!("seed {seed}, run {k}:\n  {first}\n  {again}"));
                break;
            }
        }
    }
    assert!(
        differ.is_empty(),
        "{} of {} seeds gave other counts on another run:\n{}",
        differ.len(),
        REPEAT_SEEDS.len(),
        differ.join("\n")
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
    run_many_with(
        "group_traces",
        0..n,
        trace::GROUP_CLASSES,
        1,
        group_trace,
        &GROUP,
    );
}

/// The nodes' run of the grouping class: 10^4 traces, every grouping class hit at least 100
/// times, and each class at least half its measured rate (`LARGE_GROUP`, job f45b9156; the
/// rarest grouping class, `group_redone`, at about 610). One job (tools/undo/traces-large.sh
/// PART=group). Not run here.
#[test]
#[ignore]
fn group_trace_equivalence_large() {
    let (name, seeds, min) = large_part("group_traces_large", 3_000_000..3_010_000, 100);
    run_many_with(
        &name,
        seeds,
        trace::GROUP_CLASSES,
        min,
        group_trace,
        &LARGE_GROUP,
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

/// `write --base`'s trace classes on a node (v0.9.8): 50,000 traces (`SPEC_UNDO_TRACES`
/// overrides), every trace compared with the oracle after every step, each class hit at
/// least 10^4 times: the end of the text's cases A–D (`end_break_a`…`end_break_d`, amendment
/// O5 end-of-text §4); a clean write over adjacent lines sharing their first character
/// (`write_base_shared_start`), the part of it v0.9.7's sides refused
/// (`write_base_shared_start_was_conflict`); two different changes of one line, refused
/// (`write_base_words_same_line`); a line rewritten into several against the other side's
/// change (`write_base_multiline_rewrite`). Every other class at least half its measured
/// rate (`LARGE_D1`, job 841f1cba: the same generator and seeds as `d1_traces_large`). One
/// job (tools/undo/traces-large.sh PART=endbreak). Not run here.
#[test]
#[ignore]
fn end_break_trace_class_large() {
    let (name, seeds, min) = large_part(
        "end_break_traces_large",
        2_000_000..2_000_000 + traces_or(50_000),
        10_000,
    );
    run_many_with(
        &name,
        seeds,
        &[
            "end_break_a",
            "end_break_b",
            "end_break_c",
            "end_break_d",
            "write_base_shared_start",
            "write_base_shared_start_was_conflict",
            "write_base_words_same_line",
            "write_base_multiline_rewrite",
        ],
        min,
        trace::run,
        &LARGE_D1,
    );
}

/// `write --base`'s planted faults in the kit (`differential::MergeMutant`) are each caught by
/// the trace classes within `END_BREAK_MUTANT_TRACES` traces (default 512), and the catching
/// trace passes without its fault: (i) v0.9.7's character-trimmed merge, (ii) the final line
/// break always as theirs left it.
#[test]
fn end_break_mutants_are_caught() {
    use k2_mpc_base::differential::{with_merge_mutant, MergeMutant};
    let n: u64 = std::env::var("END_BREAK_MUTANT_TRACES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(512);
    for (m, name) in [
        (MergeMutant::CharSides, "char_sides"),
        (MergeMutant::EndFollowsTheirs, "end_follows_theirs"),
    ] {
        let caught = (0..n).find_map(|seed| {
            with_merge_mutant(m, || trace::run(seed))
                .mismatch
                .map(|x| (seed, x))
        });
        let (seed, mm) = caught.unwrap_or_else(|| panic!("{name}: never caught in {n} traces"));
        if let Some(x) = trace::run(seed).mismatch {
            panic!(
                "{name}: trace {seed} fails without the fault too: {}",
                x.what
            );
        }
        println!(
            "[end-break mutant {name}] caught by trace {seed}, step {}: {}",
            mm.step, mm.what
        );
        println!(
            "RUNREC {{\"job\":null,\"run\":\"end_break_mutant_{name}\",\"tool\":\"spec-undo\",\"status\":\"CAUGHT\",\"seed\":{seed},\"traces\":{}}}",
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
