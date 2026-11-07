# Evidence: C1e grid undo, redo and revert

**Mutant numbering (renumbered at the C1 merge with kit v0.9.7):** the undo spec's D1a
amendment took (xii)–(xiv) on main (v1.10), so the structured kinds became v1.11 and their
mutants moved up by three. The runs cited below were collected under the old ids and keep
them (historical); runs from the v0.10.0 release candidate on print the new ones
(`docs/SPEC-UNDO.md`, "Every mutant and its code id").

| Old | Old code id | New | New code id |
|---|---|---|---|
| (xii) | `xii-compare-values` | (xv) | `xv-compare-values` |
| (xiii) | `xiii-compare-indices` | (xvi) | `xvi-compare-indices` |
| (xiv) | `xiv-delete-undone-by-new-key` | (xvii) | `xvii-delete-undone-by-new-key` |
| (xv) | `xv-redo-retries-skipped` | (xviii) | `xviii-redo-retries-skipped` |
| (xvi) | `xvi-own-by-pid` | (xix) | `xix-own-by-pid` |
| (xvii) | `xvii-restores-not-resolved` | (xx) | `xx-restores-not-resolved` |
| (xviii) | `xviii-undo-restore-by-new-key` | (xxi) | `xxi-undo-restore-by-new-key` |
| (xix) | `xix-drop-revived` | (xxii) | `xxii-drop-revived` |
| (xx) | `xx-sort-undo-whole-order` | (xxiii) | `xxiii-sort-undo-whole-order` |
| (xxi) | `xxi-header-split` | (xxiv) | `xxiv-header-split` |
| (xxii) | `xxii-no-identity-is-own` | (xxv) | `xxv-no-identity-is-own` |
| (xxiii) | `xxiii-already-by-holder` | (xxvi) | `xxvi-already-by-holder` |
| (xxiv) | `own-by-holder-only` (model-and-native only) | (xxvii) | `own-by-holder-only` |
| (xxv) | `xxv-anchor-on-nearest-live` | (xxviii) | `xxviii-anchor-on-nearest-live` |

Below, numerals in prose are the old ones unless marked new.

**Provenance marks** on the cited jobs (indicative (bundle), tag `ev-<job>`, K2 Engine's record at a
commit, indicative (K2 Engine, uploaded tree); rerun sha-tied by K2 Engine pending) are explained in
`docs/MODEL-CHECK.md`, Provenance of the cited runs. A job with no mark ran a
pushed commit its record names.

**Status 2026-10-29:** the evidence baseline is **`20e65f7`** on `c1/structured` (k2-mpc-base
v0.9.4 merged, plus fix (a) below). Every run here is at that commit. Spec: v1.10 §6a
(`docs/SPEC-UNDO.md`, S1–S12, reviewed by K2 Engine), §8 (the structured case classes and
mutants (xii)–(xxiii)).

Every figure from a run with a record cites it (`[run JOB/RUN:field]`, records in
`docs/evidence/runs/`, checked by `tools/evidence/check.py`). Sums of cited figures are marked
`[derived]`. Parts that print no run record (the plain `cargo test` parts) are listed by job and
exit status only.

## The claim

- **Model equals kit on Yrs, 170,000 traces:** `spec_undo::grid` (the keyed-grid model, written
  from §6a) against the kit's sessions, **10,000 traces in each of the 17 case classes**, after
  every step: the table by key (live order, deleted keys, cells, names, meta), the version, the
  outcome (kind and the conflicting regions as an exact set), every participant's stack depths,
  the holder of every facet the step wrote (`blame_elem`: the resolved change and its author),
  every holder at the end, and K-AUD-1's sync check after every outside save that came in.
  **0 mismatches.**
- **Every mutant (xii)–(xxiii) is caught:** each by the all-class run (300 traces per class),
  and each by at least one of the two trace shards.
- **The grid twin of v0.9.4's no-byte property** (a change that moves no byte, then an outside
  edit) holds on 10,000 cases, keys and blame included (section below).
- The rest of the kit's suite is green at the same commit (table at the end).

Nothing more. The traces are seeded random walks from scripted starts, not an exhaustive
enumeration (see [the enumerator](#the-enumerator-undo_enum)).

**What those traces did not cover** (corrected for v0.10.0; `SPEC-UNDO.md` §8 had listed more than
the generator ran): `write --base` on a grid, column moves and restores, block sets and moves, and
`undo --for`. The v0.10.0 generator adds them, and a test fails if a phrase of the spec's generator
line has no step or no trace reached it. Two gaps remain, each covered elsewhere:

| Not in the trace generator | What covers it |
|---|---|
| a version restore | the kit test `a_version_restore_wins_whole_with_a_note`, a kit-level mutant, and K2 Engine's kit diff |
| purge | `tests/grid_erasure.rs` |

**A gap closed at v0.10.0's head:** the re-identification oracle (`grid_store`,
`ident_case`) didn't track cells a column re-insert hides, so earlier runs, the trace runs among
them, didn't check what `rows drop-deleted` does with them. The kit then gave such a cell no note;
it now gives a `dropped_col` note, and the oracle asserts the exact rule per cell.

**A splice found by the row-pairing property's new generator (column deletes):** under `write --base
--partial`, a text's column delete held by a live write into that column left the text's other rows
applied beside the held column's old cells (24 of the 300-per-form trial's cases, every `--partial`
form). It predates item 3's ruling and needs only `--partial` and a deleted column with a live write.
K2 Engine's model doesn't cover `--partial` here. Fixed: `--partial` applies nothing of a text when a
column layout change on either side conflicts with it. The conflicts that count:

| Conflict | Region | Whose change it holds |
|---|---|---|
| `edited_col` | the column | the text's delete (the live side wrote a cell or name in it, or moved it) |
| `deleted_col` | the column | the text's move (the live side deleted it) |
| `moved_col` | the column | the text's move (both moved it, to different places) |
| `deleted_col` | a cell | the live side's delete (the text wrote into the column) |
| `deleted_col` | a name | the live side's delete (the text renamed the column) |

A text's column insert on its own never conflicts. Against a live column delete it can: the text's
fields still meet the deleted column, as `deleted_col` conflicts at its cells. K2 Engine's probe
(job `c3d226ea` at `6886198`, K2 Engine's record at `114abcd7`; base [`,`, `,a`], text [`x,,`, `x,,a`], the live side deleted column
1) showed `--partial` applying the insert anyway: the text's `x` into the deleted column was held,
but its new empty column went in where column 1 had been, so column 1 looked as if it had come back.
By key it was a new column, and the deleted one stayed deleted. Since ruling (a), any `deleted_col`
conflict holds the whole text under `--partial`. That is the only reason the insert case passes
now: at `d761eb9`, just before the ruling, it still applied (job `fc91204b`). K2 Engine's three
probes (delete, insert, move) are pinned as `engine_partial_*` in `grid_store`. They fail at
`6886198` (job `fe6b24a4`) and pass at the head (job `a167d073`): `--partial` applies nothing,
the doc is unchanged, and its conflicts equal plain `write --base`'s. No text path brings back a
column the live side deleted. `reconcile` maps a text's fields only to live columns, and a
revival (`plan.restore`) happens only when the file wins: an outside save, or a live write from a
stale `seen`, which takes the outside-save path (§2.9.1).

**Two kit-level mutants for the rows both sides changed (§2.9.1).** They are `differential`
variants: `k2_mpc_base::differential::MergeMutant` (`OutsideWholeRowLiveCols` and
`ByCellLiveCols`), planted per thread by `with_merge_mutant`. A build without the `differential`
feature has no hook. In `grid_store`, `MPC_MERGE_MUTANT=outside-whole-row-live-cols` (xxxii) or
`MPC_MERGE_MUTANT=by-cell-live-cols` (xxxiii) runs the bar
(`row_pairing::a_row_rewritten_like_another_never_takes_its_live_cells`) and its named tests
(`a_file_bringing_back_a_live_deleted_column_wins_its_rows_whole`,
`write_base_a_live_write_then_delete_of_a_column_the_text_kept_is_edited_row`) under that fault.
The first runs below used throwaway branches; with the bar at 300 cases per form and
`row_rule_review`:

- **(xxxii) `xxxii-outside-whole-row-live-cols`:** puts back the old outside-save whole-row loop.
  Caught by the bar in job `b18d0e4b` (tag `ev-b18d0e4b`): Rewrite/Outside, 2 failures of 300. First failure:
  `width 3, rows 4, a 2, b 1, picks [Keep, Borrow, Borrow], live_col Some((true, 182))`.
  `a_file_bringing_back_a_live_deleted_column_wins_its_rows_whole` also fails.
- **(xxxiii) `xxxiii-by-cell-live-cols`:** puts back the old both-changed test. Caught by the bar
  in job `d0eac34c` (tag `ev-d0eac34c`): Rewrite/Outside, 1 failure of 300. First failure:
  `width 4, rows 4, a 0, b 2, twin_deleted, live_col Some((true, 224))`. Under `write --base`,
  only the named test `write_base_a_live_write_then_delete_of_a_column_the_text_kept_is_edited_row`
  catches it. There the old test merged the row by cell and the live value stayed hidden in the
  deleted column, so the bar sees no row that nobody wrote. The difference is the whole-row rule
  (`edited_row`), not a lost value.

**Empty and absent, per cell:** a live switch between empty and absent in one cell isn't a live
edit of that cell when the base row, with only that cell switched, renders the same bytes. A blank
row given a `""` cell (blank to `,`) changes bytes and is an edit. The outside-merge property found
the old whole-row test's fault (base-a2 `54b45362` at `f5931e3`): r1/c1 `""` unset live, r1/c2 edited
live, r1/c1 set by the file gave a note and an audit event for r1/c1. The shrunk case is the named
test `a_byte_neutral_live_unset_in_a_row_both_changed_is_not_noted`, and the property replays it first.

**Two K2 Engine findings, fixed:**
- **A restore's silent loss.** p2 set r1/c1 := z and deleted r1. A version restore from before
  the set brought r1 back with the version's value and noted no one: `restore_overrides` skipped
  cells under a deleted row. It now includes them.
  - Tests: `restore_over_a_hidden_value_in_a_deleted_row_is_noted`, with the deleted-column and
    no-delete controls.
- **A false conflict.** The live side undid q to absent and set c2 := z, so it rendered `,z`.
  The text's row was `,z` too, and `write --base` refused it.
  - The rule now: rows that render the same decoded fields are the same row.
  - Tests: `write_base_a_text_row_rendering_as_the_live_row_is_no_conflict`, and the
    outside-save form `an_outside_save_rendering_as_the_live_row_overwrites_nothing`.

**K2 Engine's final set at 617343c, fixed:**
- **(376 cases) A hidden live value beside the file's whole row.** Base `a,b,c/d,e,f`; the live
  side writes T into c3 and deletes c3; the file saves `A,c/d,f`. The outside save left T hidden
  and noted no one, so a restore showed `A,T`. Now the whole row covers the hidden cell (noted).
  - Test: `a_file_over_a_live_write_then_column_delete_under_a_non_exact_mapping_wins_whole`.
- **(12 + 3 cases) A row that renders the same still conflicted.** The cause was `reconcile`'s
  column pairing by content, which read a coincidental match as a column swap. Columns now pair
  by position with no header at the same width (K2 Cores' option 1).
  - Tests: `engine_renders_the_same_is_the_same_row_*` (K2 Engine's literal sequence),
    `a_headerless_column_swap_at_the_same_width_reads_as_value_rewrites` and
    `a_ragged_text_width_is_its_widest_row`.
- **(8,372 + 440 cases) A false note.** A live `""` that the file's row renders anyway is no
  override.
  - Test: `a_live_empty_the_files_row_renders_anyway_is_not_noted`.
- **The header-less mirror shape is pinned.** `a_headerless_file_edit_of_a_live_deleted_column_falls_back_to_position`.
  After a width change, an inexact content match falls back to position, so the file never
  touches c3. A column both sides deleted restores what its cells last held:
  `a_column_deleted_by_both_sides_restores_what_its_cells_last_held`.
- **The C1k case is un-ignored.** `an_outside_rename_after_a_header_switch_and_an_unwritten_edit_is_kept`
  was fixed by c080489 and 828faf8.
- **A false `DiskReplicaMismatch`, fixed.** It was raised by every dirty grid outside save.
  - Tests: `a_dirty_outside_save_raises_no_replica_mismatch` and
    `a_planted_replica_mismatch_is_still_reported`.

**K2 Engine's verdict at 582b8b8 (PASS on safety, its job 097cc248, K2 Engine's record at `d2f1d82c`), and the item fixed after it:**
- **12 false conflicts on `write --base`.** A live undo, revert or restore took c1 from `""` back
  to absent, which renders the same. The text `q,a`, `q,`, `q,z` or `b,a` got a cell conflict.
  - The per-cell `neutral_live` rule now reaches `write --base`'s cell-conflict path.
  - Tests: `write_base_after_a_live_step_back_to_absent_applies_the_text` (K2 Engine's 12 cases)
    and `write_base_over_a_live_{empty_to_absent,absent_to_empty}_switch_applies`.
  - The control, `write_base_over_a_real_live_change_of_the_cell_conflicts`, still gives one
    `edited_row`.
  - Mutant (xxxiv) restores the old path. **The bar does not reach this shape:** its generator
    never makes a byte-neutral live switch under `write --base` (its empty/absent switch is only
    an outside save's neutral change), so all 9 of its forms pass under (xxxiv) (job ebe60d45).
    (xxxiv) is caught by its named tests only: the 12-case table and the two switch tests.
- **K2 Engine's stated gaps, and the tests that cover them** (`crates/base/tests/grid_store.rs`
  unless named):
  - **Column rename conflicts:** `a_column_renamed_on_both_sides` (write --base: `name`; an
    outside save: the file's name, the live name noted), `partial_renaming_a_live_deleted_column_applies_nothing`,
    and `table::tests::merge3_name_and_columns` (src/table/tests.rs).
  - **Restart persistence:** `revert_works_after_a_restart`,
    `a_mark_survives_a_later_exact_pair_and_a_restart`, `the_overwritten_note_survives_a_restart`
    and `a_drops_note_and_version_survive_a_restart`.
  - **A hidden cell removed by a reconcile map delete:** `a_text_cell_delete_over_a_hidden_live_value`
    (write --base: `deleted_row`, nothing applied; an outside save: the file's row back, T noted).
  - **A hidden cell removed by a revert's cell delete:** `a_revert_over_a_hidden_value_written_since_conflicts`
    (`written since`, the value kept hidden).

**K2 Engine's final reconcile evidence.** Source: k2-mpc-engine main 04905c1,
`docs/evidence/g1-reconcile-pairing-draft.md`.
- **The model at our final rules,** all COMPLETE with 0 violations (7e4cd8af, K2 Engine's record at `9252e171`):
  - merge: 15,498,046,224 + 5,219,134,080 cases;
  - livegrid: 1,305,748,920;
  - named: 272,898;
  - bounce: 2,797,589.

  All 37 of its mutants fail.
- **The kit verdict:** PASS on safety at 582b8b8 (job 097cc248, K2 Engine's record at `d2f1d82c`). The session test is green at
  aaf9d5e (98664f62, K2 Engine's record at `61727028`).
- **The kit differential test** is on branch `engine/reconcile-diff-aaf9d5e` (6172702). It is
  **not** in v0.10.0. It's a v0.10.1 candidate, to be vendored as a kit test, with the other
  v0.10.1 items (restore from a `BeforeDrop` version; a clearer refusal for restoring a dropped
  row or column).

**The last fixes before the tag (code head `6a7a2ce`).** Every job below ran on a K2 Compute node
and is read from its result lines.
- **The grid-trace mismatches were a stale harness, not the kit.** At e0e6a81 the grid traces
  gave 114 mismatches: 105 `--partial` `write --base` (kit version N, model N+1) and 9 revert
  errors (job a59858c8). At its parent bc08883 they gave 0 `--partial` and the same 9 (8e67f1c3).
  The `--partial` ones start exactly at ruling (a)'s layout hold, which the model lacked.
  Cross-check at 617343c: 216 = 207 `--partial` + 9 (bdb6fb55). The harness amendment (84bc74c)
  gives the model `layout_held`, stops a trace on a kit/model version skew after a `write --base`,
  and adds the class `partial_write_over_a_column_change` (SPEC-UNDO v1.13).
- **The 9 revert errors: a reversal over a re-paired row the doc has dropped.** S13's whole-row
  revert (`whole_row_back`) restored a row after `rows drop-deleted` had dropped it: an engine
  refusal (`UnknownKey`) instead of an outcome. An undo or redo of the same shape said
  `rewritten_row` instead of `dropped`. All three verbs plan each step through `plan_step`, whose
  S13 check runs before the per-op `plan_region` (so redo takes S13's branch too, not the per-op
  path). Fixed at 60ccbf9: S11 before S13, every reversal reports `dropped`, and `--partial` skips
  it. Mutants (xxxv) (the whole-row revert) and (xxxvi) (the undo and redo kind).
  - Dating: the six tests fail at aaf9d5e and at 876489e (jobs 58c4f1a7, 11537aaf, 91e51618).
    `whole_row_back` arrived with S13 at 659a7e9, where the controls don't compile (b35814f3,
    9b8bc308); 876489e is the earliest point a run reached.
  - Runs at the fix: off 6 passed (f9998f72); (xxxv) fails exactly revert and revert --partial
    (9bd977a3); (xxxvi) fails exactly undo, undo --partial, redo and redo --partial (0d1056bf).
- **The notes-path sweep** (every note, hint or pointer built from a recorded row or column key):
  - Overwrite notes (`plan_overrides`, `finish_overrides`): built when the change is made, against
    the live table; no restore verb; their version is the one before the change, still valid if
    the key is dropped later.
  - Drop notes: carry the values and name no version (`Overwrite::version_of`).
  - The verbs' hint (`table/verbs.rs` `by_key`): a restore hint for a deleted key only; an
    unknown key says `unknown`.
  - S13's undo and redo branch: a conflict region only, no note.
  - **The gap: the hidden-write message.** A raw keyed cell write under a dropped (or never
    existing) row or column was taken as a hidden write: an orphan cell nobody could see, restore
    or sweep, reported with `row restore --row r2`, which refuses a dropped key (probe 0f164b53, tag `ev-0f164b53`).
    Fixed at 6a7a2ce (K2 Cores' ruling A): `edit_ops` refuses the whole edit as `MPC-E805
    no_element` (exit 4, the grid verbs' unknown-key id), naming the drop while its record is in
    the revert window; a deleted but known key stays a hidden write (§4.3); a key the same edit
    inserts takes the write; a delete under an unknown key stays a no-op. Fail-first on
    assertions at 6fc1cd5 (286e74e2), off 6 passed (1cea91d1), mutant (xxxvii) fails exactly the
    four refusal tests (af09fbdb).
- **K2 Engine's evidence at k2-mpc-engine main 04905c1:**
  - the model's S11-over-S13 rule: gate_drop_r4 at 32,646,240 and gate_named with redo at
    272,903, both COMPLETE, the rule's mutant failing;
  - kit 6a7a2ce's session diff green at 10/10 (job 52e6b13c, K2 Engine's record at `e15f27a8`): the revert over a dropped row
    passes on every path (revert, undo and redo, each with and without `--partial`; the
    text-marked shape and its per-op twin), and (xxxv), (xxxvi) and (xxxvii) are caught;
  - `MPC-E805` for unknown keys; text undo unchanged.

- **K2 Engine's grid model evidence at k2-mpc-engine main d03217f**
  (`docs/evidence/g1-table-enum-draft.md` §9): ≤4 ops 25,632,883 (SORT3: 32,469,595), ≤5 ops
  2,637,547,571, all COMPLETE with 0 violations, cross-checked by a checkpoint audit (82868ba7, indicative (K2 Engine, uploaded tree); rerun sha-tied by K2 Engine pending);
  it includes the cell-delete rule (E-MAP-1) and our rulings. Its stated gap: TLC can't reach
  the batches at the bounds it finishes, so those rest on the enumerator, fixtures and unit
  tests.

  K2 Engine's side of v0.10.0 is closed.
- **The traces at the code head: the harness model was stale again, now amended (85830f4,
  test-only).** At 6a7a2ce grid-a gave 6 mismatches (b9288f51) and grid-b 33 (28aa2970); grid-m
  passed (a8a862af). Every one is the kit following S11 over S13 where the model didn't:
  - undo or redo of an op on a dropped re-paired row: kit `dropped`, model `rewritten_row`
    (grid-a: sort_vs_foreign_move 1888, 2245 (redo), 2594; move_back_past_deleted_neighbour
    2246, 5614, the 6th, printed by control e1fea25 in 607e51d4);
  - a revert's whole-row rewrite of a dropped row: kit `rows … dropped`, model without it
    (sort_vs_foreign_move 2214; grid-b write_with_no_identity 3312, 9897, the old revert-error
    seed).
  - grid-b by class: column_and_block_verbs 8, revert_after_text_pair 8, text_row_both_changed
    4, write_with_no_identity 4, partial_write_over_a_column_change 3, undo_after_text_pair 3,
    undo_for_a_person 2, own_later_write_off_stack 1 (33), all printed and classified by control
    e1fea25 (5b382833); the three in partial_write_over_a_column_change are undos and a redo
    `--partial` of the same kind.
  - The amendment (SPEC-UNDO v1.14: S11 takes precedence over S13 in undo, redo and revert)
    gives the model both rules. At 85830f4: grid-a 0 mismatches (3fa6fde0), grid-b 0
    (202427c3), grid-m ok (dbe16369), spec-undo with mutants (3840ed43) and lint (62cd2bb8)
    green.
  - The 11 printed seeds replayed at 85830f4 (`GRID_ONE`): all match with the kit as is
    (98d62198); with (xxxvi) planted every undo and redo seed mismatches (e63ff98b), and at
    10,000 traces per class (xxxvi) gives 7 mismatches in sort_vs_foreign_move,
    move_back_past_deleted_neighbour and write_with_no_identity (08161fc4); with (xxxv) planted
    the revert seeds 2214, 3312 and 531 mismatch (e912c07c), and **(xxxv) is caught at 85830f4 by
    a test failure, not a counted mismatch**: the kit's revert returns the old engine refusal and
    the harness's unwrap panics (grid_traces.rs:1205; seed 9897, 6362ec38). Counting it as a
    mismatch is listed for v0.10.1.
- **The soak at the code head.** The held soak at 6a7a2ce (b6418744) failed on speed only:
  `slowdown_percent 802` against 800 (1187 → 148 calls/s), started at load 9.54 on 16 CPUs with
  2 other jobs; soak_short and the seed-3 replay passed. Rerun on one node, idle, back to back
  with v0.9.8 (1d4b72a) as the control (`SOAK_SECS=600 SOAK_SEED=1 SOAK_MAX_SLOWDOWN=8`):

  | | 6a7a2ce (53880c61) | v0.9.8 (6eb5c809) |
  |---|---|---|
  | status, slowdown / max | COMPLETE, 180% / 800% | COMPLETE, 177% / 800% |
  | calls/s, first → tail | 272 → 151 | 275 → 155 |
  | load at start → end | 0.03 → 1.02, no other job | 1.11 → 1.00, no other job |
  | calls, version | 106,943, v57715 | 110,693, v59752 |

  µs per call by kind, at 30 s → 600 s (ratio):

  | kind | 6a7a2ce | v0.9.8 |
  |---|---|---|
  | append | 3649 → 4107 (1.1×) [unverified: progress line] | 3697 → 3532 (1.0×) [unverified: progress line] |
  | insert | 3659 → 4134 (1.1×) [unverified: progress line] | 3554 → 3827 (1.1×) [unverified: progress line] |
  | delete | 3557 → 3857 (1.1×) [unverified: progress line] | 3497 → 3342 (1.0×) [unverified: progress line] |
  | write | 2496 → 2681 (1.1×) [unverified: progress line] | 2459 → 2715 (1.1×) [unverified: progress line] |
  | undo | 3121 → 3477 (1.1×) [unverified: progress line] | 3027 → 3193 (1.1×) [unverified: progress line] |
  | tick | 13545 → 45625 (3.4×) [unverified: progress line] | 13680 → 47028 (3.4×) [unverified: progress line] |
  | outside insert | 7002 → 40871 (5.8×) [unverified: progress line] | 7468 → 29433 (3.9×) [unverified: progress line] |
  | outside delete | 7298 → 31472 (4.3×) [unverified: progress line] | 7337 → 39016 (5.3×) [unverified: progress line] |
  | outside delete-retype | 13658 → 70583 (5.2×) [unverified: progress line] | 14148 → 44054 (3.1×) [unverified: progress line] |
  | outside stale | 24496 → 40754 (1.7×) [unverified: progress line] | 25681 → 47277 (1.8×) [unverified: progress line] |
  | outside add-undo | 17115 → 58787 (3.4×) [unverified: progress line] | 18765 → 63433 (3.4×) [unverified: progress line] |

  The same growth on both: the known history-proportional cost of outside saves and the tick
  (few calls per kind, so their ratios are noisy), listed for v0.10.1. b6418744's 802% was the
  node's load (9.5, 2 other jobs).
- **The tag rule at the 6a7a2ce candidate** (superseded by the merge with main, below). `crates/base` and the packs were frozen at 6a7a2ce. `git diff --stat 6a7a2ce
  <tag>` may touch only `crates/spec-undo/`, `tools/tests/grid.sh`, `docs/SPEC-UNDO.md` and docs.
  At the tag:

  ```
   CHANGELOG.md                          |   8 ++
   crates/spec-undo/src/grid.rs          |  18 ++++-
   crates/spec-undo/tests/grid_traces.rs |  25 ++++++-
   docs/SPEC-UNDO.md                     |   5 ++
   docs/evidence/c1e-grid-undo.md        | 134 +++++++++++++++++++++++++++++++++-
   5 files changed, 186 insertions(+), 4 deletions(-)
  ```
- **The Yjs-peer path is not reached today.** A client update under a dropped key over sync would
  bypass this check, but untrusted-wire decoding is off and native merges don't reach grid docs
  (csv contract, "the claim holds only while the server's checks apply", ~line 452). No test for
  it now.
- **The final set at 6a7a2ce** (all parts, the 300-case trial, every `MergeMutant` variant plus
  off, the hook check, the csv pack at 876c969) is green: jobs f3b7a6ae, b6c234d6, a2d17efb,
  336f9681, 882bf4be, 76e722bf, 6802b305, e16b4a3e, 46738683, 20953acf, 653c86ac, 0dcc2a5b,
  fb8e6b69, b59e253d, 228f5789, 61ae96d6, c11f9fc9, f26b2658, b9e4f43f, ee87790c, 0ea42dac.

**Replaying a random grid property.** Every random grid property prints its seed as the first
line of its output (`PROPTEST_RNG_SEED=<seed> replays <name>`); a failure's captured output
shows it, and that env var replays the run exactly. These are the three random outside-save
properties in `grid_store`, `grid_disk`'s no-byte property and `table`'s five. Failing cases
also persist next to the test file, in proptest's regressions format, and run first next time.
The bar (`row_pairing`) uses a fixed seed already.

**The bar's oracle under `write --base` and `--partial`:** a live write the text's column layout
dropped is a failure, whether or not it carries a note. This follows from option 2 and ruling (a):
a column change that meets a live write is held, never applied over it. A noted drop passes only
in an outside save.

**K2 Engine's kit diff doesn't cover** `--partial`, column renames' conflicts, the persistence of
marks and notes, and a live insert (being added): for those the evidence is the kit's own tests
and these traces only.

**`write --base` in the traces is a real run through the kit, not a check of the merge rule.** The
harness takes the merge from the kit's own `merge3_text`, and checks everything downstream against
the model: the state by key, holders, stacks, marks, undo, redo and revert after it. The traces do
**not** check the merge rule itself. Its independent checks are the row-pairing property
(`grid_store`, `outside_merge::row_pairing`) and K2 Engine's model and kit diff.

## v0.10.0 at the merge with main (M = 260ef4e)

K2 Cores didn't tag 6ec1c6c: it held v0.9.7.1 but not v0.9.8, v0.9.8.1, the wire contract or the
editor. v0.10.0 is the C1 line merged with main.

**From 6a7a2ce to M**, first parent, in order:
- f33a584: merges main eebf5f8 (v0.9.8, v0.9.7.2, v0.9.8.1, the wire contract, the editor);
- 30532e0: merges main 32e1870 (v0.9.8.2, the wire goldens);
- 941e733: merges main 74fd561.

  The three merges are worker 7's, reviewed against the list of grid-side items that had to
  survive.
- 6be6467: the grid `overwritten` wire golden, test-only.
- 5ff7680, 4252dbe and fdbc4ef: the grid-note fixes (K2 Cores' rulings), each failing first on
  its assertions (a458d5c7). The changes:
  - a purge on a grid now also withholds the pushes not yet sent;
  - a withheld note names no version and no `restore_from`;
  - a grid reader without the history right gets no version on the push, the snapshot or taken
    notes;
  - `notes_for` asserts it is never reached for a grid;
  - a text note is unchanged, including the v0.9.8.2 text golden.
- 260ef4e: the client golden types gate, test-only.

**The merge's resolutions:**
- One `MergeMutant` holds main's (i) `CharSides` and (ii) `EndFollowsTheirs` and our
  (xxxii)–(xxxvii).
- One `Overwrite` and one `take_notes`.
- Main's text merge hook gains `_ => None` for our variants.
- Main's `enum Planted` (text/merge.rs) is now gated behind the `differential` feature, as ours
  are. v0.9.8 shipped with `CharSides` and `EndFollowsTheirs` in plain builds. That was harmless
  (its hook returned `None`), so the gap predates this round, and the merge closes it.

**The tag rule.** Kit code is frozen at M (260ef4e). The diff from M to the tag is docs only,
plus anything K2 Cores approves.
`git diff --stat 260ef4e <tag>`:

```
 docs/evidence/c1e-grid-undo.md | 88 +++++++++++++++++++++++++++++++++++++++++-
 1 file changed, 87 insertions(+), 1 deletion(-)
```

**The final set at M,** each job read from its result lines:
- the compile check (plain and `differential`, df742a9e);
- every `full.sh` part: base-a2 7ee51f66, base-a1 5fa041c5, lint fbd1059c, spec-undo with
  mutants d3be7701, conformance d6cafe29, base-b cc600968, base-c 4436cad7, base-d 000a75af,
  errors 522c2942, wire-guard 8a8f7bf4, evidence 5877377d;
- the 300 trial (21bc645d);
- `MergeMutant` (xxxii)–(xxxvii) and off: 535f5e86, 5486eecc, 90d5db58, 739a7b22, bf25601d,
  cd0c8c21, 4428e926. Each variant fails exactly its named tests, as at 6a7a2ce. Main's (i) and
  (ii) run in the base parts and spec-undo.
- the hook check (83fec1da): none of the hook names in the plain build, main's
  `CharSides`/`EndFollowsTheirs`/`NoteMutant` included;
- grid-a 82ecc20c, grid-b 2b78ff26, grid-m 693fa68c, all with 0 mismatches. (xxv) survives the
  grid-a shard only, as before.
- the 10^4 bar (7803cddb);
- the csv 876c969, markdown f660571 and text 107d4fe packs (2ae5c7d5, 1b10964c, 50dd50a6), each
  `pack checks: ok`;
- N−1 against v0.9.8.1, laid out from b77920b5 (c235a27c). Text docs open, read, write,
  restore, blame and `rewrite_file` there. The format-6 grid store is refused with one line:
  `store is format 6; this build reads up to 5`.

**The soak pair**, on one idle node, back to back (`SOAK_SECS=600 SOAK_SEED=1
SOAK_MAX_SLOWDOWN=8`):
- M (f5efbe5f): COMPLETE, slowdown 179% (max 800%), 276 → 154 calls/s.
- v0.9.8.1 (0f2371a7): COMPLETE, slowdown 177% (max 800%), 276 → 156 calls/s.
- Load: 0.12 → 1.11 for M and 1.11 → 1.10 for v0.9.8.1, with no other job during either.

µs per call by kind, at 30 s → 600 s:

| kind | M | v0.9.8.1 |
|---|---|---|
| append | 3641 → 3556 [unverified: progress line] | 3559 → 3667 [unverified: progress line] |
| insert | 3593 → 3841 [unverified: progress line] | 3561 → 3771 [unverified: progress line] |
| delete | 3505 → 3371 [unverified: progress line] | 3493 → 3352 [unverified: progress line] |
| write | 2400 → 2532 [unverified: progress line] | 2512 → 2611 [unverified: progress line] |
| undo | 2995 → 2860 [unverified: progress line] | 3010 → 2951 [unverified: progress line] |
| tick | 13347 → 48148 [unverified: progress line] | 13631 → 43891 [unverified: progress line] |
| outside insert | 7494 → 30390 [unverified: progress line] | 7052 → 24149 [unverified: progress line] |
| outside delete | 7685 → 34330 [unverified: progress line] | 8416 → 37945 [unverified: progress line] |
| outside delete-retype | 14596 → 57851 [unverified: progress line] | 15098 → 45422 [unverified: progress line] |
| outside stale | 24567 → 39886 [unverified: progress line] | 24674 → 45170 [unverified: progress line] |
| outside add-undo | 19972 → 59798 [unverified: progress line] | 19744 → 59676 [unverified: progress line] |

Both grow the same way. It's the known history-proportional cost of outside saves and the tick,
listed for v0.10.1.

## Model against kit, per class

Node jobs `6676b43b` (grid-a) and `49943ef3` (grid-b) (both indicative (bundle); source stated as `20e65f7`, the baseline above), release build,
`GRID_TRACES=10000 GRID_CLASSES=<shard>`, `grid_traces_model_equals_kit`. Each trace is a class's
scripted start and a random tail of grid verbs, undos (all-or-nothing, `--partial`, `--drop`),
redos, reverts, outside saves (by key through the pack's reconcile) and restarts; pids `p0`
(owner rights), `p1`, `p2`.

| Class | Traces | Steps | Mismatches | Job |
|---|---|---|---|---|
| `delete_foreign_cell_in_hidden_row` | 10,000 [run 6676b43b/grid_traces_delete_foreign_cell_in_hidden_row:traces] | 108,383 [run 6676b43b/grid_traces_delete_foreign_cell_in_hidden_row:steps] | 0 [run 6676b43b/grid_traces_delete_foreign_cell_in_hidden_row:mismatches] | `6676b43b` |
| `identical_value_write` | 10,000 [run 6676b43b/grid_traces_identical_value_write:traces] | 108,701 [run 6676b43b/grid_traces_identical_value_write:steps] | 0 [run 6676b43b/grid_traces_identical_value_write:mismatches] | `6676b43b` |
| `insert_then_foreign_cell` | 10,000 [run 6676b43b/grid_traces_insert_then_foreign_cell:traces] | 112,168 [run 6676b43b/grid_traces_insert_then_foreign_cell:steps] | 0 [run 6676b43b/grid_traces_insert_then_foreign_cell:mismatches] | `6676b43b` |
| `move_back_past_deleted_neighbour` | 10,000 [run 6676b43b/grid_traces_move_back_past_deleted_neighbour:traces] | 110,760 [run 6676b43b/grid_traces_move_back_past_deleted_neighbour:steps] | 0 [run 6676b43b/grid_traces_move_back_past_deleted_neighbour:mismatches] | `6676b43b` |
| `own_lifo_chain` | 10,000 [run 6676b43b/grid_traces_own_lifo_chain:traces] | 145,749 [run 6676b43b/grid_traces_own_lifo_chain:steps] | 0 [run 6676b43b/grid_traces_own_lifo_chain:mismatches] | `6676b43b` |
| `redo_after_partial_sort_undo` | 10,000 [run 6676b43b/grid_traces_redo_after_partial_sort_undo:traces] | 113,006 [run 6676b43b/grid_traces_redo_after_partial_sort_undo:steps] | 0 [run 6676b43b/grid_traces_redo_after_partial_sort_undo:mismatches] | `6676b43b` |
| `revert_after_restart` | 10,000 [run 6676b43b/grid_traces_revert_after_restart:traces] | 113,599 [run 6676b43b/grid_traces_revert_after_restart:steps] | 0 [run 6676b43b/grid_traces_revert_after_restart:mismatches] | `6676b43b` |
| `sort_vs_foreign_move` | 10,000 [run 6676b43b/grid_traces_sort_vs_foreign_move:traces] | 107,164 [run 6676b43b/grid_traces_sort_vs_foreign_move:steps] | 0 [run 6676b43b/grid_traces_sort_vs_foreign_move:mismatches] | `6676b43b` |
| `already_by_value` | 10,000 [run 49943ef3/grid_traces_already_by_value:traces] | 126,712 [run 49943ef3/grid_traces_already_by_value:steps] | 0 [run 49943ef3/grid_traces_already_by_value:mismatches] | `49943ef3` |
| `drop_between_batch_and_undo` | 10,000 [run 49943ef3/grid_traces_drop_between_batch_and_undo:traces] | 117,158 [run 49943ef3/grid_traces_drop_between_batch_and_undo:steps] | 0 [run 49943ef3/grid_traces_drop_between_batch_and_undo:mismatches] | `49943ef3` |
| `header_switch_partial` | 10,000 [run 49943ef3/grid_traces_header_switch_partial:traces] | 110,230 [run 49943ef3/grid_traces_header_switch_partial:steps] | 0 [run 49943ef3/grid_traces_header_switch_partial:mismatches] | `49943ef3` |
| `own_cell_clear_then_undo` | 10,000 [run 49943ef3/grid_traces_own_cell_clear_then_undo:traces] | 120,638 [run 49943ef3/grid_traces_own_cell_clear_then_undo:steps] | 0 [run 49943ef3/grid_traces_own_cell_clear_then_undo:mismatches] | `49943ef3` |
| `own_later_write_off_stack` | 10,000 [run 49943ef3/grid_traces_own_later_write_off_stack:traces] | 108,763 [run 49943ef3/grid_traces_own_later_write_off_stack:steps] | 0 [run 49943ef3/grid_traces_own_later_write_off_stack:mismatches] | `49943ef3` |
| `random` | 10,000 [run 49943ef3/grid_traces_random:traces] | 69,542 [run 49943ef3/grid_traces_random:steps] | 0 [run 49943ef3/grid_traces_random:mismatches] | `49943ef3` |
| `revert_vs_undo` | 10,000 [run 49943ef3/grid_traces_revert_vs_undo:traces] | 118,135 [run 49943ef3/grid_traces_revert_vs_undo:steps] | 0 [run 49943ef3/grid_traces_revert_vs_undo:mismatches] | `49943ef3` |
| `undo_across_outside_save` | 10,000 [run 49943ef3/grid_traces_undo_across_outside_save:traces] | 115,121 [run 49943ef3/grid_traces_undo_across_outside_save:steps] | 0 [run 49943ef3/grid_traces_undo_across_outside_save:mismatches] | `49943ef3` |
| `write_with_no_identity` | 10,000 [run 49943ef3/grid_traces_write_with_no_identity:traces] | 114,412 [run 49943ef3/grid_traces_write_with_no_identity:steps] | 0 [run 49943ef3/grid_traces_write_with_no_identity:mismatches] | `49943ef3` |
| **all 17** | **170,000** [derived] | **1,920,241** [derived] | **0** | |

`write_with_no_identity` (KC7) is the Yrs-only case (spec S2): an outside save whose writes the
kit records with no identity, through the conformance hook
`Session::set_outside_without_identity_for_tests` (the kit has no merge path that makes such a
write yet). The model marks those changes (`Grid::anon`); a holder with no identity is someone
else's. It runs on Yrs only: a pinned deviation, not a rule a native engine reproduces.

## Mutants

`grid_mutants_each_fail`: the model runs as each mutant, stopping at the first trace where it
disagrees with the kit. "Traces" is how many traces had run when the run stopped (4 threads, so
it is not the index of the first failing seed). A shard records CAUGHT or SURVIVED and doesn't
fail; the all-class run (no `GRID_CLASSES`: CI and the node part `grid-m`, job `3dbc09a0`)
fails if any mutant survives. `6676b43b`, `49943ef3` and `3dbc09a0` are each indicative (bundle) (source stated as `20e65f7`).

| Mutant | grid-a shard (`6676b43b`) | grid-b shard (`49943ef3`) | All classes (`3dbc09a0`, 300 per class) | Caught by at least one shard |
|---|---|---|---|---|
| `xii-compare-values` | CAUGHT after 4 [run 6676b43b/grid_mutant_xii-compare-values:traces] traces | CAUGHT after 216 [run 49943ef3/grid_mutant_xii-compare-values:traces] traces | CAUGHT after 4 [run 3dbc09a0/grid_mutant_xii-compare-values:traces] traces | yes |
| `xiii-compare-indices` | CAUGHT after 17 [run 6676b43b/grid_mutant_xiii-compare-indices:traces] traces | CAUGHT after 61 [run 49943ef3/grid_mutant_xiii-compare-indices:traces] traces | CAUGHT after 17 [run 3dbc09a0/grid_mutant_xiii-compare-indices:traces] traces | yes |
| `xiv-delete-undone-by-new-key` | CAUGHT after 6 [run 6676b43b/grid_mutant_xiv-delete-undone-by-new-key:traces] traces | CAUGHT after 4 [run 49943ef3/grid_mutant_xiv-delete-undone-by-new-key:traces] traces | CAUGHT after 4 [run 3dbc09a0/grid_mutant_xiv-delete-undone-by-new-key:traces] traces | yes |
| `xv-redo-retries-skipped` | CAUGHT after 212 [run 6676b43b/grid_mutant_xv-redo-retries-skipped:traces] traces | CAUGHT after 1,197 [run 49943ef3/grid_mutant_xv-redo-retries-skipped:traces] traces | CAUGHT after 317 [run 3dbc09a0/grid_mutant_xv-redo-retries-skipped:traces] traces | yes |
| `xvi-own-by-pid` | CAUGHT after 80 [run 6676b43b/grid_mutant_xvi-own-by-pid:traces] traces | CAUGHT after 4 [run 49943ef3/grid_mutant_xvi-own-by-pid:traces] traces | CAUGHT after 80 [run 3dbc09a0/grid_mutant_xvi-own-by-pid:traces] traces | yes |
| `xvii-restores-not-resolved` | CAUGHT after 13 [run 6676b43b/grid_mutant_xvii-restores-not-resolved:traces] traces | CAUGHT after 61 [run 49943ef3/grid_mutant_xvii-restores-not-resolved:traces] traces | CAUGHT after 13 [run 3dbc09a0/grid_mutant_xvii-restores-not-resolved:traces] traces | yes |
| `xviii-undo-restore-by-new-key` | CAUGHT after 4 [run 6676b43b/grid_mutant_xviii-undo-restore-by-new-key:traces] traces | CAUGHT after 4 [run 49943ef3/grid_mutant_xviii-undo-restore-by-new-key:traces] traces | CAUGHT after 7 [run 3dbc09a0/grid_mutant_xviii-undo-restore-by-new-key:traces] traces | yes |
| `xix-drop-revived` | CAUGHT after 14 [run 6676b43b/grid_mutant_xix-drop-revived:traces] traces | CAUGHT after 27 [run 49943ef3/grid_mutant_xix-drop-revived:traces] traces | CAUGHT after 13 [run 3dbc09a0/grid_mutant_xix-drop-revived:traces] traces | yes |
| `xx-sort-undo-whole-order` | CAUGHT after 4 [run 6676b43b/grid_mutant_xx-sort-undo-whole-order:traces] traces | CAUGHT after 10 [run 49943ef3/grid_mutant_xx-sort-undo-whole-order:traces] traces | CAUGHT after 6 [run 3dbc09a0/grid_mutant_xx-sort-undo-whole-order:traces] traces | yes |
| `xxi-header-split` | SURVIVED 1,600 [run 6676b43b/grid_mutant_xxi-header-split:traces] traces | CAUGHT after 307 [run 49943ef3/grid_mutant_xxi-header-split:traces] traces | CAUGHT after 2,867 [run 3dbc09a0/grid_mutant_xxi-header-split:traces] traces | yes |
| `xxii-no-identity-is-own` | SURVIVED 1,600 [run 6676b43b/grid_mutant_xxii-no-identity-is-own:traces] traces | CAUGHT after 1,178 [run 49943ef3/grid_mutant_xxii-no-identity-is-own:traces] traces | CAUGHT after 4,163 [run 3dbc09a0/grid_mutant_xxii-no-identity-is-own:traces] traces | yes |
| `xxiii-already-by-holder` | CAUGHT after 18 [run 6676b43b/grid_mutant_xxiii-already-by-holder:traces] traces | CAUGHT after 4 [run 49943ef3/grid_mutant_xxiii-already-by-holder:traces] traces | CAUGHT after 15 [run 3dbc09a0/grid_mutant_xxiii-already-by-holder:traces] traces | yes |

(xxi) and (xxii) survive the grid-a shard because their classes (`header_switch_partial`,
`write_with_no_identity`) are in grid-b; both are caught there and by the all-class run.

## The regression found on the way (fixed at `20e65f7`)

The first grid-a run at `3910b73` (job `fe1b0fb0`, indicative (bundle); source stated as `3910b73`) found
1 [run fe1b0fb0/grid_traces_revert_after_restart:mismatches] mismatch:
`revert_after_restart` seed 7420, step 13. A map delete of an empty cell moves no byte and only
deletes, so v0.9.4's re-sync marker (the state vector) left the disk replica holding the cell;
the next outside save, aligned on that replica, paired two blank rows the other way round (right
bytes, wrong keys and blame). The merge of v0.9.4 had replaced C1's `live_is_synced` with
`resync_if_synced`. Fix (a), `20e65f7`: grid ingest aligns on the live doc whenever it renders to
the synced bytes, never on the replica. Seed 7420 is kept as the IT
`regression_revert_after_restart_seed_7420`. The marker itself moves to the change counter in
v0.9.5 (worker 2); until then:

- the mid-way sync audit in the no-byte twin skips the two delete-only changes, and
  `the_replica_follows_a_change_that_only_deleted` is ignored with that reason;
- the trace harness audits after an outside save that came in, not after one that left the file
  as it was.

Both come back when v0.9.5 is merged.

## The grid no-byte property (v0.9.4's grid twin)

Node job `86462d2b` (indicative (bundle); source stated as `20e65f7`), release: **10,000 [run 86462d2b/nobyte_grid:cases] cases passed** (`grid_store`, `no_byte::grid_outside_edit_after_any_change_that_moved_no_byte_is_kept`, through proptest's `TestRunner`), plus the planted stale-replica test. CI runs 256.

A grid doc (1–3 columns, 2–6 rows from a small vocabulary, so values repeat); one change that
moves no byte (header on; header on and off; a cell set and set back; undo, revert or version
restore of a cell set; a row delete or move undone; an empty cell unset; a row deleted, written,
then `drop-deleted`); then nothing, a tick, a flush, a restart or a flush and a restart; then a
plain editor changes any line (a cell, a new row, a row deleted, a row moved). The doc and the
file end as the outside edit; the row and column keys are the outside edit reconciled on the
live table; every cell the edit didn't change keeps its author, the others are the outside
edit's; K-AUD-1 and the store audit are clean.

## Costs (node job `66075ba8`, release)

`66075ba8` is indicative (bundle) (source stated as `20e65f7`).

**A cell set on the doc** (`structured_mem`, `measure_cell_set_latency_at_the_cap`; times in
tenths of a millisecond, as recorded):

| Cells | Import (tenths of ms) | Cell set p50 | p99 | max |
|---|---|---|---|---|
| 10,000 [estimate] | 271 [run 66075ba8/measure_import_10000_cells:value_x10] | 0 [run 66075ba8/measure_cell_set_10000_p50:value_x10] | 1 [run 66075ba8/measure_cell_set_10000_p99:value_x10] | 1 [run 66075ba8/measure_cell_set_10000_max:value_x10] |
| 25,000 [estimate] | 629 [run 66075ba8/measure_import_25000_cells:value_x10] | 0 [run 66075ba8/measure_cell_set_25000_p50:value_x10] | 0 [run 66075ba8/measure_cell_set_25000_p99:value_x10] | 3 [run 66075ba8/measure_cell_set_25000_max:value_x10] |
| 50,000 [estimate] | 1,380 [run 66075ba8/measure_import_50000_cells:value_x10] | 1 [run 66075ba8/measure_cell_set_50000_p50:value_x10] | 1 [run 66075ba8/measure_cell_set_50000_p99:value_x10] | 5 [run 66075ba8/measure_cell_set_50000_max:value_x10] |
| 100,000 (the cap) [estimate] | 3,426 [run 66075ba8/measure_import_100000_cells:value_x10] | 2 [run 66075ba8/measure_cell_set_100000_p50:value_x10] | 4 [run 66075ba8/measure_cell_set_100000_p99:value_x10] | 10 [run 66075ba8/measure_cell_set_100000_max:value_x10] |

(The cell counts are the test's sizes, not measured: marked `[estimate]` for the checker.) At the
cap, a cell set's p99 is 0.4 ms, and listing the 10,000 rows after the sets takes
55 [run 66075ba8/measure_list_rows_100000_cells:value_x10] tenths of a ms.

**Through a session at 25,000 cells** (`grid_cost`, `grid_costs_at_the_cell_limit`; µs):

| Operation | n | p50 | p99 | max |
|---|---|---|---|---|
| person cell set (limit check, edit, store) | 300 [run 66075ba8/grid_cost_person_cell_set:n] | 292 [run 66075ba8/grid_cost_person_cell_set:p50_us] | 500 [run 66075ba8/grid_cost_person_cell_set:p99_us] | 12,126 [run 66075ba8/grid_cost_person_cell_set:max_us] |
| agent cell set with `--base` | 300 [run 66075ba8/grid_cost_agent_cell_set_base:n] | 1,287 [run 66075ba8/grid_cost_agent_cell_set_base:p50_us] | 24,508 [run 66075ba8/grid_cost_agent_cell_set_base:p99_us] | 51,761 [run 66075ba8/grid_cost_agent_cell_set_base:max_us] |
| agent row delete with `--base` | 50 [run 66075ba8/grid_cost_agent_row_delete_base:n] | 12,163 [run 66075ba8/grid_cost_agent_row_delete_base:p50_us] | 13,083 [run 66075ba8/grid_cost_agent_row_delete_base:p99_us] | 13,083 [run 66075ba8/grid_cost_agent_row_delete_base:max_us] |
| `edit_ops_with` (keyed state for the view) | 100 [run 66075ba8/grid_cost_edit_ops_with:n] | 10,246 [run 66075ba8/grid_cost_edit_ops_with:p50_us] | 11,153 [run 66075ba8/grid_cost_edit_ops_with:p99_us] | 11,153 [run 66075ba8/grid_cost_edit_ops_with:max_us] |
| read (content, encode, base's keyed state) | 100 [run 66075ba8/grid_cost_read:n] | 38,051 [run 66075ba8/grid_cost_read:p50_us] | 41,998 [run 66075ba8/grid_cost_read:p99_us] | 41,998 [run 66075ba8/grid_cost_read:max_us] |
| `Doc::keyed_state` alone | 30 [run 66075ba8/grid_cost_keyed_state:n] | 10,134 [run 66075ba8/grid_cost_keyed_state:p50_us] | 11,785 [run 66075ba8/grid_cost_keyed_state:p99_us] | 11,785 [run 66075ba8/grid_cost_keyed_state:max_us] |
| `Doc::map(cells)` alone | 30 [run 66075ba8/grid_cost_map_cells:n] | 9,293 [run 66075ba8/grid_cost_map_cells:p50_us] | 11,238 [run 66075ba8/grid_cost_map_cells:p99_us] | 11,238 [run 66075ba8/grid_cost_map_cells:max_us] |

The node was shared with other jobs; the max column carries their noise.

## The rest of the suite at `20e65f7`

Plain `cargo test` parts (debug, as CI runs them, except `pt`), every one exit 0 (every job indicative (bundle), source stated as `20e65f7`):

| Part | What | Job |
|---|---|---|
| lib | unit tests, 254 passed | `2cbebf88` (indicative (bundle)) |
| doc | doctests | `55f5508a` (indicative (bundle)) |
| int1 | structured, structured_mem, convergence, disk (sim with K-AUD-1 after every outside save and tick), edit_check(_structured, with the scalar-only wire guard), … | `56ffb7d1` (indicative (bundle)) |
| int2 | history, sessions, presence, tools, soak (short) | `8f66c9ab` (indicative (bundle)) |
| int3 | tier0, promote, edit_paths, edit_cost | `6d892678` (indicative (bundle)) |
| int4 | parity, erasure | `2106eaac` (indicative (bundle)) |
| int5 | grid_store (the undo class ITs, the no-byte twin at 256) | `5ea753cf` (indicative (bundle)) |
| nobyte | the grid no-byte twin at 10^4 (section above) | `86462d2b` (indicative (bundle)) |
| conformance-a, -b | the conformance suite | `9a35e187`, `64985f36` (both indicative (bundle)) |
| spec-undo | the text model's suite and mutants; grid traces at CI size (256), the seed-7420 IT | `116648d5` (indicative (bundle)) |
| pt | table property tests, `PROPTEST_CASES` at 10^4, release | `67d8751f` (indicative (bundle)) |

## The enumerator (`undo_enum`)

The grid model is not added to `undo_enum`. The enumerator explores `spec_undo::bounded`, the
text model's transition system, checking its invariants (U*, O*, I*) on every transition, with
TLC parity against `spec/quint/undo.qnt`. The grid model has no invariants of its own: it
executes §6a's rules, and the check is the difference against the kit. Enumerating it alone
would check nothing until grid invariants are written (and a Quint twin, for TLC parity). That
is work for a later stage, not a fit for C1e.

## v1.10 additions (2026-10-29, `5fb84d6`)

Two S4 additions after the C1e review (spec text in `docs/SPEC-UNDO.md`, agreed by K2 Engine,
m_mnjjaNtJfm):

- **Restore → delete, the existence own-test** (G1-8 §3): a conflict unless every undel covering
  one of the element's deletes that isn't own is in the batch's causal past. Class "concurrent
  undels, then undo" and mutant (xxiv) `own-by-holder-only` are **model-and-native only**: a
  session's history is linear, so on Yrs an undel outside the batch's past came later and holds
  the facet (the holder test already conflicts), and a merged one has no identity (KC7). The
  native oracle for (xxiv) is K2 Engine's: engine `main` `0c73d6f`, mpce-reg
  `restore_undo_is_own` with a two-concurrent-undels fixture and `own-by-holder-only` planted.
  On the kit, the linear form is the IT `undo_class_undels_covering_different_deletes`
  (`d17bf4c`).
- **Move → move back, the anchor**: right after the element's left neighbour in place order,
  deleted or not; the record keeps the neighbours up to the nearest live one (at most 8), and a
  dropped one is skipped (else the start). Class `move_back_then_restore_left_neighbour` (K2's
  assertion, also the kit IT `undo_class_move_back_then_restore_left_neighbour`: every row back
  at its index from before the delete) and mutant (xxv) `anchor-on-nearest-live` (`b0e703d`,
  `5fb84d6`). Native follows the same rule by construction (the kit picks the anchor; native's
  `move(elem, after)` takes a live or hidden anchor) and refuses a move anchored on an element
  whose drop is in the op's causal past (a typed MPCE refusal): Addendum **G1-9**, approved by K2,
  in the approved requirements PRD.

**The anchor, exhaustive on the native engine (v0.10.0; external citation).** K2 Engine has
checked the anchored move-back (S4, G1-9) exhaustively on its enumerator: k2-mpc-engine `main`
`8645866`, `docs/evidence/g1-table-enum-draft.md` §4. The 3 × (1 + 1) case ("3×2", seed 2) is
complete at 40,323,105,331 states [unverified: k2-mpc-engine 8645866 records], with every
invariant (`move_after_anchor`, `move_after_anchor_target` and `move_after_dropped_refused`
among them) and no violation. It ran as 14 resumable slices, job `8d31fd70` first and `24049f4d`
last, and job `59a5c699` audited the checkpoint (every seed counted once, totals matching)
[unverified: k2-mpc-engine 8645866 records]. With the 2×3 case, both splits complete
(2 × (2 + 1): 3,664,616,698 states; 2 × (1 + 2): 2,822,953,870 states [unverified: k2-mpc-engine
8645866 records]), the anchored grid move is exhaustive at both bounds G1-9 §2 asks for.
`8d31fd70`, `24049f4d` and `59a5c699` are each indicative (K2 Engine, uploaded tree); rerun sha-tied by K2 Engine pending. The run
records stay in the engine's repo; none are copied into `runs/`.

**Model against kit at `5fb84d6`**, 10^4 traces per class, grid-a and grid-b on build nodes (`f9a4c72b`,
`fed30e62`: both indicative (bundle); source stated as `5fb84d6`):

| Class | Traces | Steps | Mismatches | Job |
|---|---|---|---|---|
| `delete_foreign_cell_in_hidden_row` | 10,000 [run f9a4c72b/grid_traces_delete_foreign_cell_in_hidden_row:traces] | 108,381 [run f9a4c72b/grid_traces_delete_foreign_cell_in_hidden_row:steps] | 0 [run f9a4c72b/grid_traces_delete_foreign_cell_in_hidden_row:mismatches] | `f9a4c72b` |
| `identical_value_write` | 10,000 [run f9a4c72b/grid_traces_identical_value_write:traces] | 108,699 [run f9a4c72b/grid_traces_identical_value_write:steps] | 0 [run f9a4c72b/grid_traces_identical_value_write:mismatches] | `f9a4c72b` |
| `insert_then_foreign_cell` | 10,000 [run f9a4c72b/grid_traces_insert_then_foreign_cell:traces] | 112,168 [run f9a4c72b/grid_traces_insert_then_foreign_cell:steps] | 0 [run f9a4c72b/grid_traces_insert_then_foreign_cell:mismatches] | `f9a4c72b` |
| `move_back_past_deleted_neighbour` | 10,000 [run f9a4c72b/grid_traces_move_back_past_deleted_neighbour:traces] | 110,766 [run f9a4c72b/grid_traces_move_back_past_deleted_neighbour:steps] | 0 [run f9a4c72b/grid_traces_move_back_past_deleted_neighbour:mismatches] | `f9a4c72b` |
| `move_back_then_restore_left_neighbour` | 10,000 [run f9a4c72b/grid_traces_move_back_then_restore_left_neighbour:traces] | 111,160 [run f9a4c72b/grid_traces_move_back_then_restore_left_neighbour:steps] | 0 [run f9a4c72b/grid_traces_move_back_then_restore_left_neighbour:mismatches] | `f9a4c72b` |
| `own_lifo_chain` | 10,000 [run f9a4c72b/grid_traces_own_lifo_chain:traces] | 145,747 [run f9a4c72b/grid_traces_own_lifo_chain:steps] | 0 [run f9a4c72b/grid_traces_own_lifo_chain:mismatches] | `f9a4c72b` |
| `redo_after_partial_sort_undo` | 10,000 [run f9a4c72b/grid_traces_redo_after_partial_sort_undo:traces] | 113,007 [run f9a4c72b/grid_traces_redo_after_partial_sort_undo:steps] | 0 [run f9a4c72b/grid_traces_redo_after_partial_sort_undo:mismatches] | `f9a4c72b` |
| `revert_after_restart` | 10,000 [run f9a4c72b/grid_traces_revert_after_restart:traces] | 113,600 [run f9a4c72b/grid_traces_revert_after_restart:steps] | 0 [run f9a4c72b/grid_traces_revert_after_restart:mismatches] | `f9a4c72b` |
| `sort_vs_foreign_move` | 10,000 [run f9a4c72b/grid_traces_sort_vs_foreign_move:traces] | 107,162 [run f9a4c72b/grid_traces_sort_vs_foreign_move:steps] | 0 [run f9a4c72b/grid_traces_sort_vs_foreign_move:mismatches] | `f9a4c72b` |
| `already_by_value` | 10,000 [run fed30e62/grid_traces_already_by_value:traces] | 126,712 [run fed30e62/grid_traces_already_by_value:steps] | 0 [run fed30e62/grid_traces_already_by_value:mismatches] | `fed30e62` |
| `drop_between_batch_and_undo` | 10,000 [run fed30e62/grid_traces_drop_between_batch_and_undo:traces] | 117,160 [run fed30e62/grid_traces_drop_between_batch_and_undo:steps] | 0 [run fed30e62/grid_traces_drop_between_batch_and_undo:mismatches] | `fed30e62` |
| `header_switch_partial` | 10,000 [run fed30e62/grid_traces_header_switch_partial:traces] | 110,228 [run fed30e62/grid_traces_header_switch_partial:steps] | 0 [run fed30e62/grid_traces_header_switch_partial:mismatches] | `fed30e62` |
| `own_cell_clear_then_undo` | 10,000 [run fed30e62/grid_traces_own_cell_clear_then_undo:traces] | 120,639 [run fed30e62/grid_traces_own_cell_clear_then_undo:steps] | 0 [run fed30e62/grid_traces_own_cell_clear_then_undo:mismatches] | `fed30e62` |
| `own_later_write_off_stack` | 10,000 [run fed30e62/grid_traces_own_later_write_off_stack:traces] | 108,765 [run fed30e62/grid_traces_own_later_write_off_stack:steps] | 0 [run fed30e62/grid_traces_own_later_write_off_stack:mismatches] | `fed30e62` |
| `random` | 10,000 [run fed30e62/grid_traces_random:traces] | 69,311 [run fed30e62/grid_traces_random:steps] | 0 [run fed30e62/grid_traces_random:mismatches] | `fed30e62` |
| `revert_vs_undo` | 10,000 [run fed30e62/grid_traces_revert_vs_undo:traces] | 117,605 [run fed30e62/grid_traces_revert_vs_undo:steps] | 0 [run fed30e62/grid_traces_revert_vs_undo:mismatches] | `fed30e62` |
| `undo_across_outside_save` | 10,000 [run fed30e62/grid_traces_undo_across_outside_save:traces] | 115,121 [run fed30e62/grid_traces_undo_across_outside_save:steps] | 0 [run fed30e62/grid_traces_undo_across_outside_save:mismatches] | `fed30e62` |
| `write_with_no_identity` | 10,000 [run fed30e62/grid_traces_write_with_no_identity:traces] | 114,411 [run fed30e62/grid_traces_write_with_no_identity:steps] | 0 [run fed30e62/grid_traces_write_with_no_identity:mismatches] | `fed30e62` |
| **all 18** | **180,000** [derived] | **2,030,642** [derived] | **0** | |

**Mutants at `5fb84d6`** (13; (xxiv) runs on the engine's oracle, above; `f9a4c72b`, `fed30e62` and
`b6d0f9b6` are each indicative (bundle), source stated as `5fb84d6`):

| Mutant | grid-a (`f9a4c72b`) | grid-b (`fed30e62`) | All classes (`b6d0f9b6`) |
|---|---|---|---|
| `xii-compare-values` | CAUGHT after 4 [run f9a4c72b/grid_mutant_xii-compare-values:traces] | CAUGHT after 223 [run fed30e62/grid_mutant_xii-compare-values:traces] | CAUGHT after 4 [run b6d0f9b6/grid_mutant_xii-compare-values:traces] |
| `xiii-compare-indices` | CAUGHT after 16 [run f9a4c72b/grid_mutant_xiii-compare-indices:traces] | CAUGHT after 70 [run fed30e62/grid_mutant_xiii-compare-indices:traces] | CAUGHT after 18 [run b6d0f9b6/grid_mutant_xiii-compare-indices:traces] |
| `xiv-delete-undone-by-new-key` | CAUGHT after 5 [run f9a4c72b/grid_mutant_xiv-delete-undone-by-new-key:traces] | CAUGHT after 4 [run fed30e62/grid_mutant_xiv-delete-undone-by-new-key:traces] | CAUGHT after 6 [run b6d0f9b6/grid_mutant_xiv-delete-undone-by-new-key:traces] |
| `xv-redo-retries-skipped` | CAUGHT after 207 [run f9a4c72b/grid_mutant_xv-redo-retries-skipped:traces] | CAUGHT after 1,220 [run fed30e62/grid_mutant_xv-redo-retries-skipped:traces] | CAUGHT after 317 [run b6d0f9b6/grid_mutant_xv-redo-retries-skipped:traces] |
| `xvi-own-by-pid` | CAUGHT after 81 [run f9a4c72b/grid_mutant_xvi-own-by-pid:traces] | CAUGHT after 4 [run fed30e62/grid_mutant_xvi-own-by-pid:traces] | CAUGHT after 83 [run b6d0f9b6/grid_mutant_xvi-own-by-pid:traces] |
| `xvii-restores-not-resolved` | CAUGHT after 14 [run f9a4c72b/grid_mutant_xvii-restores-not-resolved:traces] | CAUGHT after 61 [run fed30e62/grid_mutant_xvii-restores-not-resolved:traces] | CAUGHT after 13 [run b6d0f9b6/grid_mutant_xvii-restores-not-resolved:traces] |
| `xviii-undo-restore-by-new-key` | CAUGHT after 7 [run f9a4c72b/grid_mutant_xviii-undo-restore-by-new-key:traces] | CAUGHT after 4 [run fed30e62/grid_mutant_xviii-undo-restore-by-new-key:traces] | CAUGHT after 5 [run b6d0f9b6/grid_mutant_xviii-undo-restore-by-new-key:traces] |
| `xix-drop-revived` | CAUGHT after 13 [run f9a4c72b/grid_mutant_xix-drop-revived:traces] | CAUGHT after 24 [run fed30e62/grid_mutant_xix-drop-revived:traces] | CAUGHT after 13 [run b6d0f9b6/grid_mutant_xix-drop-revived:traces] |
| `xx-sort-undo-whole-order` | CAUGHT after 5 [run f9a4c72b/grid_mutant_xx-sort-undo-whole-order:traces] | CAUGHT after 41 [run fed30e62/grid_mutant_xx-sort-undo-whole-order:traces] | CAUGHT after 5 [run b6d0f9b6/grid_mutant_xx-sort-undo-whole-order:traces] |
| `xxi-header-split` | SURVIVED 1,800 [run f9a4c72b/grid_mutant_xxi-header-split:traces] | CAUGHT after 327 [run fed30e62/grid_mutant_xxi-header-split:traces] | CAUGHT after 2,807 [run b6d0f9b6/grid_mutant_xxi-header-split:traces] |
| `xxii-no-identity-is-own` | SURVIVED 1,800 [run f9a4c72b/grid_mutant_xxii-no-identity-is-own:traces] | CAUGHT after 1,182 [run fed30e62/grid_mutant_xxii-no-identity-is-own:traces] | CAUGHT after 4,090 [run b6d0f9b6/grid_mutant_xxii-no-identity-is-own:traces] |
| `xxiii-already-by-holder` | CAUGHT after 18 [run f9a4c72b/grid_mutant_xxiii-already-by-holder:traces] | CAUGHT after 4 [run fed30e62/grid_mutant_xxiii-already-by-holder:traces] | CAUGHT after 18 [run b6d0f9b6/grid_mutant_xxiii-already-by-holder:traces] |
| `xxv-anchor-on-nearest-live` | CAUGHT after 59 [run f9a4c72b/grid_mutant_xxv-anchor-on-nearest-live:traces] | CAUGHT after 11 [run fed30e62/grid_mutant_xxv-anchor-on-nearest-live:traces] | CAUGHT after 55 [run b6d0f9b6/grid_mutant_xxv-anchor-on-nearest-live:traces] |

(xxi) and (xxii) survive the grid-a shard (their classes are in grid-b), as before; every mutant
is caught by at least one shard and by the all-class run. The CI-size check (`f4476017`, indicative (bundle), source stated as `5fb84d6`:
grid_store, the grid traces at 256 and the mutants) is green at the same commit.

## Open for later stages

- **The per-facet extras for sort** (C1i). (S4's move-back anchor is done: v1.10 additions.)
- **Purge of structured docs** (C1f): ELEM_WRITES rows, v3 records and versions by key.
- **Grid claims in `GridConflict.claimed_by`** (C1g).
- **v0.9.5:** the re-sync marker (the change counter) and three-way ingest against the live doc;
  then the skips listed under the regression come out.

## Log lines

`k2 compute logs <job>` prints any job's whole log; the records above were collected from those
logs by `tools/evidence/collect.py`.
