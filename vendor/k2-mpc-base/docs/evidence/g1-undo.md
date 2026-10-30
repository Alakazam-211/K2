# Evidence: G1 undo model

**Status 2026-10-28:** the G1 runs are in: the exhaustive pair, the walks at the literal G1
bound (with and without a restart) and every mutant on walks. The exact counts (TLC against
the enumerator, and the ITF replay) are re-run with run records (`85045092`); the TLC mutants too
(`a885a72a`): every mutant caught by both, at the same least depth. Requirements: E-HIST-5, E-HIST-13, E-HIST-15, E-HIST-16, E-PROOF-1 of the MPC
requirements PRD.

Every figure from a run with a record cites it (`[run JOB/RUN]` or `[run JOB/RUN:field]`,
records in `docs/evidence/runs/`, checked by `tools/evidence/check.py`). Jobs from before run
records (`6a6c1557`, `41f256f3`) now have records collected from their logs: the two exhaustive
counts cite them; what those records don't carry (seeds, depth, walk counts, per-class counts)
stays marked `[unverified: ...]`. Their summary lines are quoted verbatim
[below](#log-lines-of-the-runs-before-run-records), and
`k2 compute logs <job>` prints the whole log.

## The claim

The undo spec v1.9 (`docs/SPEC-UNDO.md`), as the oracle of record `spec_undo::model` reads it,
holds in the bounded world (seed `abb`, `MAXLEN` 4; every invariant below checked on every
transition) in exactly these senses:

- **Exhaustive at 3 pids × 1 event + 2 undos + 1 revert**, and **exhaustive at 2 pids × 2
  events + 2 undos + 1 revert** (pid symmetry, per-seed deduplication): no violation, every
  seed finished.
- **5,000,000 random walks at the literal G1 bound** (3 pids × 3 events + 2 undos + 1 revert),
  each to full depth: no violation.
- **1,000,000 random walks at the literal G1 bound plus 1 restart**: no violation.
- **1,000,000 biased walks at the literal G1 bound** (`walk --bias redo-after-partial`: the
  steps toward a redo of a partial undo weigh more; no step is left out and every step is
  checked): no violation, and the class `redo_after_partial` hit 28,568 times.
- **1,000,000 biased walks at the literal G1 bound plus 1 restart**: no violation, and
  `redo_after_partial` hit 28,208 times.
- **Every mutant is caught** on 50,000 walks at the literal G1 bound (the restart mutant with
  1 restart added): nine by an invariant, one (iv) by the oracle's own assertion. Under the
  bias every mutant is caught again, all ten by an invariant ((iv) by `U17`).

Nothing more: the literal G1 bound is sampled, not exhausted (≥ 10^13 states,
`docs/MODEL-CHECK.md`, "The G1 bound").

## Runs at the G1 bound

The oracle is the same in every run below: from kit v0.9.1 (spec v1.9) to v0.9.3,
`crates/spec-undo/src/model/` changed only by a harness-only mutant hook, and the bounded world
by the walk's case-class counters (v0.9.2) and the panic catch (v0.9.3, `42146d2`).

| Run | Bound (seed `abb`, `MAXLEN` 4) | Mode | States or walks | Status | Violations | Job | Record |
|---|---|---|---|---|---|---|---|
| `3x1u2r1` | 3 pids × 1 event + 2 undos + 1 revert | `dfs --sym`, exhaustive | 8,516,359 [run 6a6c1557/enumerator_3x1u2r1] states, 502 / 502 seeds, max depth 6 [unverified: seeds and depth, before run records] | COMPLETE | 0 | `6a6c1557` (kit v0.9.2) | log line L1 |
| `2x2u2r1` | 2 pids × 2 events + 2 undos + 1 revert | `dfs --sym`, exhaustive | 717,352,254 [run 6a6c1557/enumerator_2x2u2r1] states, 28,886 / 28,886 seeds, max depth 7 [unverified: seeds and depth, before run records] | COMPLETE | 0 | `6a6c1557` (kit v0.9.2) | log line L2 |
| walks at G1 | 3 pids × 3 events + 2 undos + 1 revert | `walk`, sampled | 5,000,000 walks, 60,000,000 steps [unverified: before run records] | SAMPLED: every walk asked ran | 0 | `41f256f3` (kit v0.9.1) | log line L3 |
| walks at G1 + 1 restart | the same + 1 restart | `walk`, sampled | 1,000,000 [run d810536a/walk_3x3_u2_r1_rs1_none:walks] walks, 13,000,000 [run d810536a/walk_3x3_u2_r1_rs1_none:steps] steps | SAMPLED: every walk asked ran | 0 | `d810536a` (`bbbfc17`) | RUNREC |
| biased walks at G1 | 3 pids × 3 events + 2 undos + 1 revert | `walk --bias redo-after-partial`, sampled with weights | 1,000,000 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:walks] walks, 12,000,000 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:steps] steps | SAMPLED: every walk asked ran | 0 | `f51260dc` (`194048f`) | RUNREC |
| biased walks at G1 + 1 restart | the same + 1 restart | `walk --bias redo-after-partial`, sampled with weights | 1,000,000 [run d9b8d20f/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:walks] walks, 13,000,000 [run d9b8d20f/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:steps] steps; `redo_after_partial` 28,208 [run d9b8d20f/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:redo_after_partial] | SAMPLED: every walk asked ran | 0 | `d9b8d20f` (`ab3bc8a`) | RUNREC |
| biased walks at G1 + 1 restart (smoke) | the same + 1 restart | `walk --bias redo-after-partial` | 1,000 [run f51260dc/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:walks] walks, 13,000 [run f51260dc/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:steps] steps | SAMPLED: every walk asked ran | 0 | `f51260dc` (`194048f`) | RUNREC |
| exact counts, ITF replay | small bounds (`tools/undo/counts.sh`, `PART=ab`) | TLC, `bfs`, ITF replay | 7 bounds, TLC = enumerator ([The exact counts](#the-exact-counts)) | COMPLETE | 0 | `85045092` (`73a4073`) | RUNREC |
| TLC mutants | `tools/undo/counts.sh`, `PART=m` | TLC (one worker), `bfs --first` | every mutant ([Teeth](#teeth-mutants-each-must-fail-minimal-counterexample-inside-an-exhaustive-bound)) | VIOLATION, each its rule | as expected | `a885a72a` | RUNREC |

Every walk runs to full depth (12 actions at G1, 13 with the restart: the steps are exactly 12
and 13 times the walks). The walks are independent draws, not deduplicated: the same path can
recur.

## Mutants at the literal G1 bound

Each on 50,000 walks at 3 pids × 3 events + 2 undos + 1 revert (restart-single-taint with 1
restart added), job `5cfc0edf` (`bbbfc17`, `g1.sh PART=walks-mutants`). A mutant is caught only
if the walk reports a violation; the depth is the shortest witness the walks found (a walk is
not a least witness: the exhaustive least depths are under Teeth, below).

| Mutant | What it breaks | Caught | How (shortest witness seen) | Walks run | Record |
|---|---|---|---|---|---|
| (i) `i-no-taint` | whole-text merge3, no taint rule | yes | `U9` and `own_only` at depth 3, `U22a` at 7; trace of 4 [run 5cfc0edf/walk_3x3_u2_r1_rs0_i-no-taint] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_i-no-taint:walks] | RUNREC |
| (iii) `iii-partial-hides-taint` | `--partial` leaves tainted regions out of its report | yes | `U15` at depth 3; trace of 4 [run 5cfc0edf/walk_3x3_u2_r1_rs0_iii-partial-hides-taint] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_iii-partial-hides-taint:walks] | RUNREC |
| (iv) `iv-redo-retries-skipped` | a partial undo's redo entry also carries the regions it skipped | yes, **by a panic** | `panic` at depth 5: `window.rs:199`, "a shared edge's elements must be foreign"; trace of 6 [run 5cfc0edf/walk_3x3_u2_r1_rs0_iv-redo-retries-skipped] states | 0 (the run stops at the panic; 40 steps) | RUNREC |
| (vi) `vi-o5-every-window-end` | O5 at every window end | yes | `O5` at depth 3; trace of 4 [run 5cfc0edf/walk_3x3_u2_r1_rs0_vi-o5-every-window-end] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_vi-o5-every-window-end:walks] | RUNREC |
| (vii) `vii-o5-region-base` | O5 on the region's base, not the doc's | yes | `O5` at depth 3; trace of 4 [run 5cfc0edf/walk_3x3_u2_r1_rs0_vii-o5-region-base] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_vii-o5-region-base:walks] | RUNREC |
| `skip-u19` | a purge keeps the stacks | yes | `U19` at depth 1; trace of 2 [run 5cfc0edf/walk_3x3_u2_r1_rs0_skip-u19] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_skip-u19:walks] | RUNREC |
| (viii) `f3-rediff` | sides by re-diffing strings (F3) | yes | `own_only` at depth 3; trace of 4 [run 5cfc0edf/walk_3x3_u2_r1_rs0_f3-rediff] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_f3-rediff:walks] | RUNREC |
| (ix) `restart-single-taint` | a restart reads one taint flag back at both edges | yes | `RT` at depth 8; trace of 9 [run 5cfc0edf/walk_3x3_u2_r1_rs1_restart-single-taint] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs1_restart-single-taint:walks] | RUNREC |
| (x) `x-provenance-ignored-in-o4` | O4 on element identity, no O4a | yes | `I1` at depth 3; trace of 4 [run 5cfc0edf/walk_3x3_u2_r1_rs0_x-provenance-ignored-in-o4] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_x-provenance-ignored-in-o4:walks] | RUNREC |
| (xi) `xi-no-o4a` | O4 on (range, roots), no O4a | yes | `I1` at depth 4; trace of 5 [run 5cfc0edf/walk_3x3_u2_r1_rs0_xi-no-o4a] states | 50,000 [run 5cfc0edf/walk_3x3_u2_r1_rs0_xi-no-o4a:walks] | RUNREC |

**(iv) is caught by a panic, not by `U17`.** Its witness: from `abb`, p1 appends `a`
(`abba`); p1 deletes the first and last letters (`bb`); p2 reverts that change with
`--partial` (`abba`); p2 replaces the first letter and deletes the last (`bbb`); p1 undoes 2
steps, and the oracle's assertion in `window.rs:199` (`drop_front`: the elements at a shared
edge must be foreign) fails. Under (iv) every undo step also pushes the regions it skipped
into its redo entry (`model/mod.rs`, code that runs only under that mutant). The assertion
never fired without a mutant: the oracle ran this path's bound exhaustively (`2x2u2r1` above)
and through 6,000,000 walks with no panic. Its invariant still catches it: `undo_enum bfs
--first` finds `U17` at depth 3 (Teeth, below),
and the earlier walks of (iv) (`41f256f3`, `7fc87257`, before the panic stopped them) had
reported `U17` at depth 5 [unverified: a progress line, before run records].

The first two mutants were also run in `41f256f3` and `7fc87257` (the same rules at the same
depths); job `5cfc0edf` re-ran them for records. Those two jobs then hung on (iv)'s panic until
their time limit (the harness bug fixed in `42146d2`), so no other walk of theirs finished.
`7fc87257`'s main walks were a 1,000-walk smoke run, not counted here.

## Biased walks: the class redo_after_partial

In uniform walks a redo of a partial undo's entry is rare (1,289 hits in 5,000,000 walks): it
needs an `undo --partial` that skips a region, then a redo by the same pid, inside a budget of
2 undos and redos in all. `undo_enum walk --bias redo-after-partial` (`194048f`) draws each
step with a weight instead of uniformly: 256 for a step that redoes such an entry, 64 for a step
that leaves a new one (an undo that skipped regions), 4 for any other `undo --partial`, 1 for
every other step. Every enabled step keeps a weight of at least 1, so no path of the bound is
left out, and every step is checked against every invariant as in a plain walk. The biased walks
are not a uniform sample of paths: they add depth on this class, they do not replace the
uniform walks. Job `f51260dc`, `g1.sh PART=walks-bias`:

- **1,000,000 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:walks] biased walks at the literal G1 bound:** 0 violations; `redo_after_partial` hit
  **28,568 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:redo_after_partial]** times (all classes in the coverage table below).
- **1,000 [run f51260dc/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:walks] biased walks at G1 + 1 restart (a smoke run):** 0 violations; `redo_after_partial` hit 27 [run f51260dc/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:redo_after_partial] times.
- **1,000,000 [run d9b8d20f/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:walks] biased walks at G1 + 1 restart** (job `d9b8d20f`, `g1.sh PART=walks-bias-restart`): 0
  violations; `redo_after_partial` hit **28,208 [run d9b8d20f/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:redo_after_partial]** times, and `restart` 780,299 [run d9b8d20f/walk_3x3_u2_r1_rs1_none_bias-redo-after-partial:restart].

**The bias hides no mutant.** Every mutant again on 50,000 biased walks (restart-single-taint
with 1 restart added):

| Mutant | Caught | How (shortest witness seen) | Walks run | redo_after_partial hits |
|---|---|---|---|---|
| (i) `i-no-taint` | yes | `U9` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_i-no-taint_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_i-no-taint_bias-redo-after-partial:walks] | 1,359 [run f51260dc/walk_3x3_u2_r1_rs0_i-no-taint_bias-redo-after-partial:redo_after_partial] |
| (iii) `iii-partial-hides-taint` | yes | `U15` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_iii-partial-hides-taint_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_iii-partial-hides-taint_bias-redo-after-partial:walks] | 1,041 [run f51260dc/walk_3x3_u2_r1_rs0_iii-partial-hides-taint_bias-redo-after-partial:redo_after_partial] |
| (iv) `iv-redo-retries-skipped` | yes | `U17` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_iv-redo-retries-skipped_bias-redo-after-partial] states; then a `panic` at depth 4 (`window.rs:199`), which stopped the run | 153 [run f51260dc/walk_3x3_u2_r1_rs0_iv-redo-retries-skipped_bias-redo-after-partial:walks] (the run stops at the panic) | 0 [run f51260dc/walk_3x3_u2_r1_rs0_iv-redo-retries-skipped_bias-redo-after-partial:redo_after_partial] |
| (vi) `vi-o5-every-window-end` | yes | `O5` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_vi-o5-every-window-end_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_vi-o5-every-window-end_bias-redo-after-partial:walks] | 1,195 [run f51260dc/walk_3x3_u2_r1_rs0_vi-o5-every-window-end_bias-redo-after-partial:redo_after_partial] |
| (vii) `vii-o5-region-base` | yes | `O5` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_vii-o5-region-base_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_vii-o5-region-base_bias-redo-after-partial:walks] | 1,382 [run f51260dc/walk_3x3_u2_r1_rs0_vii-o5-region-base_bias-redo-after-partial:redo_after_partial] |
| `skip-u19` | yes | `U19` at depth 1; trace of 2 [run f51260dc/walk_3x3_u2_r1_rs0_skip-u19_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_skip-u19_bias-redo-after-partial:walks] | 1,430 [run f51260dc/walk_3x3_u2_r1_rs0_skip-u19_bias-redo-after-partial:redo_after_partial] |
| (viii) `f3-rediff` | yes | `own_only` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_f3-rediff_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_f3-rediff_bias-redo-after-partial:walks] | 1,384 [run f51260dc/walk_3x3_u2_r1_rs0_f3-rediff_bias-redo-after-partial:redo_after_partial] |
| (ix) `restart-single-taint` | yes | `RT` at depth 8; trace of 9 [run f51260dc/walk_3x3_u2_r1_rs1_restart-single-taint_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs1_restart-single-taint_bias-redo-after-partial:walks] | 1,420 [run f51260dc/walk_3x3_u2_r1_rs1_restart-single-taint_bias-redo-after-partial:redo_after_partial] |
| (x) `x-provenance-ignored-in-o4` | yes | `I1` at depth 3; trace of 4 [run f51260dc/walk_3x3_u2_r1_rs0_x-provenance-ignored-in-o4_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_x-provenance-ignored-in-o4_bias-redo-after-partial:walks] | 1,315 [run f51260dc/walk_3x3_u2_r1_rs0_x-provenance-ignored-in-o4_bias-redo-after-partial:redo_after_partial] |
| (xi) `xi-no-o4a` | yes | `I1` at depth 4; trace of 5 [run f51260dc/walk_3x3_u2_r1_rs0_xi-no-o4a_bias-redo-after-partial] states | 50,000 [run f51260dc/walk_3x3_u2_r1_rs0_xi-no-o4a_bias-redo-after-partial:walks] | 1,461 [run f51260dc/walk_3x3_u2_r1_rs0_xi-no-o4a_bias-redo-after-partial:redo_after_partial] |

(iii) and (iv) touch partial undos and redo. Under the bias (iv) is caught by its invariant,
`U17` at depth 3 (the least depth of the exhaustive check): p1 deletes `a` and turns the last
`b` into `a`, deletes again, then undoes 2 steps with `--partial`; one region is skipped
(overlap) and the redo entry carries more regions than were applied. The walks then reached
the oracle's panic at depth 4, as in the uniform run, and stopped.

## Per-class coverage at G1

Transitions per case class (E-HIST-13), from the clean walks. The biased column is not a
uniform sample (weighted steps, above).

| Class | Walks at G1 (`41f256f3`) [unverified: before run records] | Walks at G1 + 1 restart | Biased walks at G1 (`--bias redo-after-partial`) |
|---|---|---|---|
| eof_append | 215,358 [unverified: before run records] | 42,272 [run d810536a/walk_3x3_u2_r1_rs1_none:eof_append] | 41,305 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:eof_append] |
| edge_insert | 1,708,451 [unverified: before run records] | 333,934 [run d810536a/walk_3x3_u2_r1_rs1_none:edge_insert] | 360,586 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:edge_insert] |
| taint | 494,519 [unverified: before run records] | 96,633 [run d810536a/walk_3x3_u2_r1_rs1_none:taint] | 101,191 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:taint] |
| partial | 443,021 [unverified: before run records] | 85,204 [run d810536a/walk_3x3_u2_r1_rs1_none:partial] | 163,897 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:partial] |
| redo_after_partial | 1,289 [unverified: before run records] | 242 [run d810536a/walk_3x3_u2_r1_rs1_none:redo_after_partial] | **28,568** [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:redo_after_partial] |
| identical_reports | 630,349 [unverified: before run records] | 118,720 [run d810536a/walk_3x3_u2_r1_rs1_none:identical_reports] | 152,741 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:identical_reports] |
| import_kept | 3,207,597 [unverified: before run records] | 636,841 [run d810536a/walk_3x3_u2_r1_rs1_none:import_kept] | 778,697 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:import_kept] |
| purge_drops | 5,043,303 [unverified: before run records] | 968,317 [run d810536a/walk_3x3_u2_r1_rs1_none:purge_drops] | 1,006,462 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:purge_drops] |
| restart | 0 (no restart in this bound) | 779,665 [run d810536a/walk_3x3_u2_r1_rs1_none:restart] | 0 (no restart in this bound) |
| exact_vs_rediff | 355,050 [unverified: before run records] | 68,876 [run d810536a/walk_3x3_u2_r1_rs1_none:exact_vs_rediff] | 68,093 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:exact_vs_rediff] |
| touching_regions | 582,025 [unverified: before run records] | 115,253 [run d810536a/walk_3x3_u2_r1_rs1_none:touching_regions] | 124,865 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:touching_regions] |
| revert_extra_conflict | 31,880 [unverified: before run records] | 6,279 [run d810536a/walk_3x3_u2_r1_rs1_none:revert_extra_conflict] | 5,642 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:revert_extra_conflict] |
| concurrent_restore | 272,790 [unverified: before run records] | 52,395 [run d810536a/walk_3x3_u2_r1_rs1_none:concurrent_restore] | 76,083 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:concurrent_restore] |
| concurrent_restore_o4 (O4 took a restoration once) | not counted (v0.9.1) | 24,780 [unverified: in the record as `concurrent_restore_o4`, a field name the checker cannot cite (a digit)] | 41,232 [unverified: in the record as `concurrent_restore_o4`, a field name the checker cannot cite (a digit)] |
| concurrent_restore_o4a (O4a fired) | not counted (v0.9.1) | 3,914 [unverified: in the record as `concurrent_restore_o4a`, a field name the checker cannot cite (a digit)] | 5,841 [unverified: in the record as `concurrent_restore_o4a`, a field name the checker cannot cite (a digit)] |
| concurrent_restore_base_by_root | not counted (v0.9.1) | 24,424 [run d810536a/walk_3x3_u2_r1_rs1_none:concurrent_restore_base_by_root] | 30,681 [run f51260dc/walk_3x3_u2_r1_rs0_none_bias-redo-after-partial:concurrent_restore_base_by_root] |

## The exact counts

TLC's exact distinct-state counts on `spec/quint/undo.qnt` against `undo_enum bfs` at the same
bounds (no `--sym`: both count the same states), and the ITF replay. Node job `85045092`
(`tools/undo/counts.sh`, `PART=ab`, after the Quint export guard passed): every count equal,
every run COMPLETE. The mutants (`PART=m`) are in [Teeth](#teeth-mutants-each-must-fail-minimal-counterexample-inside-an-exhaustive-bound).

| Check | Bound | States (TLC / enumerator) | Result | Job |
|---|---|---|---|---|
| TLC = enumerator, exact | 2 × 1 + 1 undo (`abb`, 4) | 3,889 [run 85045092/c_2x1u1] / 3,889 [run 85045092/e_c_2x1u1] | equal, no violation | `85045092` |
|  | 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 35,792 [run 85045092/c_2x1u1r1] / 35,792 [run 85045092/e_c_2x1u1r1] | equal, no violation | `85045092` |
|  | 3 × 1 + 1 undo (`abb`, 4) | 220,210 [run 85045092/c_3x1u1] / 220,210 [run 85045092/e_c_3x1u1] | equal, no violation | `85045092` |
|  | 2 × 2 + 1 undo (`ab`, 3) | 542,418 [run 85045092/c_2x2u1_l3] / 542,418 [run 85045092/e_c_2x2u1_l3] | equal, no violation | `85045092` |
|  | 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 200,530 [run 85045092/c_1x3r1s1] / 200,530 [run 85045092/e_c_1x3r1s1] | equal, no violation | `85045092` |
|  | 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 144 [run 85045092/c_1x1u1r1] / 144 [run 85045092/e_c_1x1u1r1] | equal, no violation | `85045092` |
|  | 2 × 1 + 2 undos + 1 revert (`ab`, 4) | 49,656 [run 85045092/c_2x1u2r1] / 49,656 [run 85045092/e_c_2x1u2r1] | equal, no violation | `85045092` |

The same counts as v1.8 and v1.9's runs (`00d8b0dc`, `85262144`; `0b8fedfe`, `e6427b49`):
the oracle didn't change.

**ITF replay** (Quint simulator traces replayed step by step into the oracle), same job:
- 2 × 1 + 1 undo + 1 revert: 1,000 [run 85045092/itf_2x1u1r1:traces] traces, 4,000 [run 85045092/itf_2x1u1r1:steps] steps, 0 [run 85045092/itf_2x1u1r1:mismatches] mismatches;
- 2 × 1 + 2 undos + 1 revert (restorations meet): 1,000 [run 85045092/itf_2x1u2r1:traces] traces, 5,000 [run 85045092/itf_2x1u2r1:steps] steps, 0 [run 85045092/itf_2x1u2r1:mismatches] mismatches.

## What is not covered

- **The literal G1 bound is sampled, not exhausted.** 6,000,000 walks (5,000,000 without and
  1,000,000 with a restart) are a sample of ≥ 10^13 states; a rule broken only on paths the
  walks rarely take could be missed.
- **Exhaustive only up to** 3 pids × 1 event + 2 undos + 1 revert and 2 pids × 2 events + 2
  undos + 1 revert. 3 × 2 + 2 undos (`g1.sh PART=long`, ≈ 2.6 × 10^10 states) has not been run;
  neither exhaustive run has a restart (restarts are exhaustive only at the small bound of the
  counts, 1 × 3 + 1 revert + 1 restart).
- **`redo_after_partial` past 10^4 only in biased walks:** 28,568 hits in 1,000,000 biased walks
  (cited above) and 28,208 in 1,000,000 biased walks with a restart, against 1,289 in
  5,000,000 uniform walks and 242 with a restart. The biased walks weigh their steps, so they
  are not a uniform sample of the bound's paths.
- **One seed, one length cap:** every run starts from `abb` with `MAXLEN` 4 (texts longer than 4
  are cut). Other seeds appear only in the counts' small bounds.
- **The main walks ran on kit v0.9.1,** before the panic catch: a panic there would have hung
  the run, not passed it, and the run finished; the 1,000,000 restart walks ran on v0.9.3.
- **The concurrency class** (client batches integrated by CRDT merge between undo steps) is
  later; the bounded world and the trace test are sequential.
- **Native engine:** the trace test compares the oracle with the kit on Yrs only.
- Not in the bounded world (the trace test covers them): leave and rejoin, multi-byte text,
  `write --base`, `replace_text`, `restore`, `undo --for`, on-behalf edits, the revert window
  and its persisted carry, the U6 bounds, purge redaction of records.

## Log lines of the runs before run records

Verbatim from `k2 compute logs 6a6c1557` (L1, L2) and `k2 compute logs 41f256f3` (L3); each
is followed in the log by `=== <label>: exit 0`. (`tools/evidence/collect.py` would misread
these older lines: it takes `violations=[]` for a violation, so no record file was made.)

```
L1  SUMMARY mode=dfs states=8516359 seeds=502/502 complete=true max_depth=6 dedup=seed violations=[] observed=[] wall=168.9s
    === enumerator 3x1u2r1: exit 0, wall 169s
L2  SUMMARY mode=dfs states=717352254 seeds=28886/28886 complete=true max_depth=7 dedup=seed violations=[] observed=[] wall=7463.8s
    === enumerator 2x2u2r1: exit 0, wall 7464s
L3  SUMMARY mode=walk walks=5000000 steps=60000000 violations=[] observed=[] classes=[eof_append:215358,edge_insert:1708451,taint:494519,partial:443021,redo_after_partial:1289,identical_reports:630349,import_kept:3207597,purge_drops:5043303,restart:0,exact_vs_rediff:355050,touching_regions:582025,revert_extra_conflict:31880,concurrent_restore:272790] wall=6473.0s
    === walks at the literal G1 bound: exit 0, wall 6473s
```

Re-running: `tools/undo-enum-job.sh tools/undo/g1.sh <out>`, then a job with `PART=exhaustive`
(127 min on 8 threads: it needs a grant over 2 h), `walks-main` (108 min on 8 threads),
`walks-mutants` (8 min), `walks-restart` (34 min on 6 threads) or `walks-bias` (16 min on 8
threads, with `WALKS_BIAS=1000000`), `walks-bias-restart` (13 min on 8 threads); `tools/README.md` has the
command.

# Background

## What is specified
- **v1.9 (fidelity review of v1.8; no change to the kit's behaviour):** U10a is read over the
  regions a step **applies** (a skipped region lends its roots to none); I1 is checked
  **after every step** of an undo or redo (the kit commits each step as its own change); O4a
  looks for the live root in the text as the step merges it (after earlier steps), window
  included. New mutant (xi) `no-o4a`.
- **v1.8 (provenance):** an element an undo, redo or revert puts back records the **root** it
  restores (U23, path-compressed); windows are read **by root** (U8); U10a and revert follow
  the root; O4 is judged on (position, root); O4a refuses to put back a live root; invariant
  **I1** (at most one live element per root); blame by root (U18a). It undoes two v1.7
  regressions: (a) a revert after undo and redo, (b) another participant's edit and undo
  inside your region, both of which conflicted, now apply.
- **The undo spec** `docs/SPEC-UNDO.md`, **v1.7**: undo, redo and revert over plain values
  (text and hunks), implemented once in the kit (`session/undo.rs`) for every engine. v1.7:
  the merge's sides are **exact hunk sets** (U10): ours is the inverse of the change's own
  hunks, theirs the foreign hunks from the applied-hunk stream that touched the region's
  carried window (U8); **U10a** (`hist.undo_own_only`) is normative; U12 is restated on hunk
  sets.
- **The oracle of record** `spec_undo::model` (`crates/spec-undo`), written from the spec text
  with its own element-level window (`model/window.rs`) and merge; it calls the kit only for
  `diff::hunks` / `align_insertions` where the spec still uses them (`restore`,
  `write --base`).
- **The Quint model** `spec/quint/undo.qnt`, transcribed definition by definition from the
  oracle and driven by the same bounded world (`crates/spec-undo/src/bounded.rs`), so a Quint
  state is exactly a `bounded::View`. The diff it still needs (mutants only) is pinned by
  `spec/quint/undo_diff.qnt`, equal to `diff::hunks` on every pair of texts up to 4 long.

## Readings
The oracle's readings R1–R11 and U22a (`crates/spec-undo/src/model/mod.rs`), carried by the
Quint model:
- R1: a bound strictly inside a deleted range moves to the range's start (tainted, U9).
- R2: one step's regions merge against the step's starting text.
- R3: a clean region applies the merge's own hunks.
- R4 (v1.7): a redo region's ours is the inverse of the hunks the undo applied there.
- R5–R10: bounds, notices, `--steps` with fewer batches, revert as one undo step, purge
  redaction, the window.
- **R11** (v1.7): a later edit is inside a window when within the carried bounds (a pure
  insertion at an edge included); at a shared edge the right region drops the foreign
  elements the left one took; a tainted region keeps no theirs.
- U22a: revert's per-edge carry, inheritance at touching regions, then the clamp; restart
  transparent (v1.6).
- The bounded world's own: an undo or redo request spends the budget whatever it finds.

## Invariants checked (every transition; each a rule id in `viol`)
`I1` (v1.8: at most one live element per root; after every step of an undo or redo since v1.9), `own_only` (U10a, **enforced**; by root since v1.8; over the applied regions since v1.9), `U9` (a tainted region is never applied), `U13` (a refused
request changes nothing), `U15` (identical reports), `U17` (redo reapplies exactly the applied
regions), `U19` (a purge empties every stack), `U22a` (revert's conflicts ⊇ undo's, same
bounds, same text), `RT` (a restart changes no revert's outcome or text, every record, both
modes), `O5` (the end-of-text rule exactly at the doc's end with a non-empty base). `U12` (the
restated equivalence) is a diagnostic (`--u12`), clean at every bound run.

## Teeth (mutants: each must fail, minimal counterexample inside an exhaustive bound)
Enumerator `undo_enum bfs --first` (exhaustive, so the depth found is the least) and TLC with one worker (a shortest counterexample), both re-run with run records in `a885a72a` (`tools/undo/counts.sh`, `PART=m`): TLC's trace is one state longer than the enumerator's depth (it counts the initial state), at every mutant:

| Mutant | Fails | Bound (exhaustive) | Least depth (enumerator) | TLC's shortest trace (states, one worker) |
|---|---|---|---|---|
| (i) no taint rule | U9 | 2 pids × 1 event + 1 undo (`abb`, 4) | 3 [run a885a72a/e_m_i-no-taint:violation_depth] | 4 [run a885a72a/m_tiny] |
| (iii) `--partial` hides taint | U15 | same | 3 [run a885a72a/e_m_iii-partial-hides-taint:violation_depth] | 4 [run a885a72a/m_tiny#2] |
| (iv) redo retries skipped | U17 | same | 3 [run a885a72a/e_m_iv-redo-retries-skipped:violation_depth] | 4 [run a885a72a/m_tiny#3] |
| (vi) O5 at every window end | O5 | same | 3 [run a885a72a/e_m_vi-o5-every-window-end:violation_depth] | 4 [run a885a72a/m_tiny#4] |
| (vii) O5 on the region's base | O5 | same | 3 [run a885a72a/e_m_vii-o5-region-base:violation_depth] | 4 [run a885a72a/m_tiny#5] |
| skip-U19 | U19 | same | 1 [run a885a72a/e_m_skip-u19:violation_depth] | 2 [run a885a72a/m_tiny#6] |
| **(viii) `f3-rediff`**: sides by re-diffing strings (F3) | own_only | same | 3 [run a885a72a/e_m_f3-rediff:violation_depth] | 4 [run a885a72a/m_tiny#7] |
| **(ix) `restart-single-taint`**: one taint flag read back at both edges | RT | 1 pid × 3 events + 1 revert + 1 restart (`abba`, 4) | 4 [run a885a72a/e_m_restart-single-taint:violation_depth] | 5 [run a885a72a/m_rst] |
| **(x) `provenance-ignored-in-O4`** (v1.8): O4 on element identity, no O4a | I1 | 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 3 [run a885a72a/e_m_x-provenance-ignored-in-o4:violation_depth] | 4 [run a885a72a/m_prov] |
| **(xi) `no-o4a`** (v1.9): O4 on (range, roots), no O4a | I1 | 2 × 1 + 1 undo + 1 revert (`ab`, 4) | 4 [run a885a72a/e_m_xi-no-o4a:violation_depth] | 5 [run a885a72a/m_xi] |

TLC with one worker (a shortest counterexample): v1.8, job `a75a2670` (every mutant,
the same depths as v1.7 for (i)–(ix)); the smoke job `ac55d0a0` agrees. v1.7: `7a899e60`,
`0c00aee0`. (x)'s witness: seed `ab`, p1 deletes `b`, undoes, reverts its delete: by element
identity both `b`s land at the doc's end (O5), two live elements with one root.
v1.9: job `20db2b75` (every mutant, one TLC worker: (i)–(x) as v1.8, (xi) at 4). (xi)'s
least witness is at the doc's end: p1 deletes `b`, undoes, p2 types `a` before the restored
`b`, p1 reverts its delete; theirs (`a` and the restored `b`) and ours (`b`) differ by roots,
so O5 lands both: two live `b` with one root. Its mid-text witness (two undos, depth 5,
inside 2 × 2 + 2 undos + 1 revert) is a test in `tests/mutants.rs`.

The same set fails `cargo test -p k2-mpc-spec-undo --features mutants --test mutants`. On
walks at the literal G1 bound (20,000 walks each, a local smoke run): f3-rediff fails
`own_only` at depth 3; restart-single-taint (with one restart added) fails `RT` at depth 8.
G1-run mutant walks: [Mutants at the literal G1 bound](#mutants-at-the-literal-g1-bound).

## Model checking: history
**v1.8 → v1.9:** unchanged at every bound above (TLC = enumerator; jobs `0b8fedfe`,
`e6427b49`), and no violation set changed (enumerator without `--first`, v0.9.0 against
this, `5597f6ea` and `3951cf03`): the oracle never breaks I1, so U10a's two readings agree
and no step-only I1 failure exists.

**Old vs new counts (v1.7 → v1.8):** 3,891 → 3,889; 34,125 → 35,792; 218,761 → 220,210;
500,176 → 542,418; 193,161 → 200,530 (TLC = enumerator at each, v1.7 job `7a899e60`). Roots
add states (a restoration differs from what it restores only by being one); the carried
theirs leaves the state (two windows read the same by root are one state; machine-checked
exact, `bounded::tests::the_view_is_exact`). The class `concurrent_restore`: 4,661 states at
2 × 1 + 2 undos + 1 revert, 10,654 at 2 × 2 + 1 undo.

**v1.7 history.** v0.8.2's model (re-diffed sides, `own_only` observed) had 3,891 /
35,635 / 220,660 / 531,261 distinct states at the first four bounds. v1.7's are 3,891 /
34,125 / 218,761 / 500,176: the state now holds each region's ours and theirs (more states),
and `own_only` no longer appears in `viol` (in v0.8.2 the same state with and without it were
two states; fewer now).

**Per-class coverage in the walks** (E-HIST-13, counts of transitions per class; the walk
summary prints them). Smoke (local), 20,000 walks at G1 + 1 restart: eof_append 584,
edge_insert 7,100, taint 1,927, partial 1,712, redo_after_partial 4, identical_reports 2,450,
import_kept 12,846, purge_drops 19,406, restart 15,584, exact_vs_rediff 1,900,
touching_regions 2,288, revert_extra_conflict 145. G1 run: [the coverage table](#per-class-coverage-at-g1).

## ITF replay
Quint simulator traces exported as ITF and replayed step by step into the oracle (action
enabled, cut, outcome, whole state):
- v1.9: the same 1,000 + 1,000 traces, **0 mismatches** (`0b8fedfe`).
- v1.8: 1,000 traces at 2 × 1 + 1 undo + 1 revert (4,000 steps) and 1,000 at 2 × 1 + 2 undos +
  1 revert (5,000 steps), **0 mismatches** (`00d8b0dc`); 300 traces with and 300 without a
  restart, 0 (smoke `ac55d0a0`).
- v1.7: 2 × 1 + 1 undo + 1 revert: 1,000 traces, 4,000 steps, **0 mismatches** (job `7a899e60`);
  300 traces, 1,200 steps, 0 (smoke `0c00aee0`).
- The same with 1 restart: 300 traces, 1,500 steps, **0 mismatches** (`0c00aee0`).
- At G1: 3,000 Quint simulator traces (35,798 steps), 0 mismatches (v1.6, `docs/MODEL-CHECK.md`); not re-run on v1.9.

## The trace test (oracle vs the kit on Yrs)
- **v1.9:** 256 traces (28,632 steps) and the restart twin: **0 mismatches**, the same figures
  as v1.8 (the kit is unchanged). The kit-side U10a check now also holds the kit's removals to
  the roots of the **applied** regions (the oracle's, by the element correspondence): 2,874
  steps checked, 9,543 removals, 0 unresolved.
- **v1.8:** 256 traces (28,632 steps) and the 64-trace restart twin (6,580 steps): **0 mismatches**, run locally (about 39 s). New since v1.8: each live character's root and root author, I1, and U10a **by root** are compared on both sides (the kit side from per-character blame and a mirror of each pid's stacks: 2,874 steps checked, 9,543 removals, 0 unresolved; a by-element check fails 31 of 64 traces). Class `concurrent_restores_of_one_root`: 186 hits (O4 took a restoration once 72, O4a 12, a base element counted by root only 102), plus 31 in the twin. U22a's cost: 8 of 2,286 revert regions (0.35%); with touching regions 8 of 408 (1.96%). Commit `0ad8d4c`.
- 256 seeded traces (28,632 steps), every case class hit, **0 mismatches**; the restart twin
  (64 stack-free traces, 6,580 steps): **0 mismatches** (commit b90c172 and later).
- Since v1.7 a mismatch also when: the applied hunks differ (0 here: kit and oracle remove
  the same elements), the oracle's or the kit's undo, redo or revert removes an element not
  its own (per-element blame on the kit).
- The class `exact_sides_differ_from_rediff` (F3's: exact sides differ from what a re-diff
  would give): 877 hits in 256 traces.
- **U22a's cost** (revert conflicting where undo of the same change would apply): 7 of 2,304
  revert regions (0.30%); with touching regions 7 of 443 (1.58%).
- Large run (≥ 10^4 per class): TODO.

## Conformance
v1.8: `hist.undo_own_only` reads roots and asserts the expected outcomes of (a) and (b); the
kit's session tests `revert_of_a_change_undone_and_redone_applies` and
`undo_after_a_foreign_edit_inside_and_its_undo_applies` (with a restart variant),
`concurrent_restores_of_one_root_keep_i1` (both orders, mid-text and at the end),
`o4a_refuses_a_live_root_at_the_doc_end`, `o4_a_fresh_identical_text_is_not_the_restored_root`,
`blame_by_root_survives_a_restart` and the format 4 → 5 upgrade pass.
`sec.client_provenance_refused` is declared (kit: by construction; native: G2).

`hist.undo_own_only` (per element, through blame) passes on the kit, with F3's case; the
session tests `undo_exact_hunks_*` (F3, repeated lines, a run; undo and revert) fail on
v0.8.2's kit and pass here.


## Log
- 2026-10-28: the exact counts, the ITF replay and the TLC mutants re-run with run records (`85045092`, `a885a72a`); the exhaustive pair's counts cite records collected from its log (`6a6c1557`).
- 2026-10-29: biased walks with 1 restart (`d9b8d20f`): `redo_after_partial` at 28,208 hits
  in 1,000,000 walks, 0 violations.
- 2026-10-28: biased walks (`walk --bias redo-after-partial`, `f51260dc`): `redo_after_partial`
  at 28,568 hits in 1,000,000 walks, 0 violations; every mutant caught under the bias.
- 2026-10-28: the G1 runs: exhaustive pair (`6a6c1557`), 5,000,000 walks at G1 (`41f256f3`),
  1,000,000 walks with a restart (`d810536a`), every mutant on walks (`5cfc0edf`: (iv) by a
  panic); run records for the last two; the exact counts pending.
- 2026-10-28: v1.9 (fidelity review): U10a over applied regions, I1 per step, O4a wording,
  mutant (xi); counts unchanged, (xi) at depth 4 in both.
- 2026-10-28: v1.8 (provenance): spec, kit, oracle, Quint, counts and mutants filled in;
  the trace test's v1.8 run below; G1-run numbers still pending.
- 2026-10-28: drafted with the v1.7 slice (kit v0.9.0 prep); G1-run numbers pending.
