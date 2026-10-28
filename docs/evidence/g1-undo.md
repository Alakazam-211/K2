# Evidence: G1 undo model

**Status 2026-10-28:** DRAFT. The G1-run numbers are TODO; they are filled in after the node
runs that follow this slice. One place for an external reviewer: what was checked, at which
bounds, on which commit, by which job. Requirements: E-HIST-5, E-HIST-13, E-HIST-15,
E-HIST-16, E-PROOF-1 of the MPC requirements PRD.

## What is specified
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
`I1` (v1.8: at most one live element per root), `own_only` (U10a, **enforced**; by root since v1.8), `U9` (a tainted region is never applied), `U13` (a refused
request changes nothing), `U15` (identical reports), `U17` (redo reapplies exactly the applied
regions), `U19` (a purge empties every stack), `U22a` (revert's conflicts ⊇ undo's, same
bounds, same text), `RT` (a restart changes no revert's outcome or text, every record, both
modes), `O5` (the end-of-text rule exactly at the doc's end with a non-empty base). `U12` (the
restated equivalence) is a diagnostic (`--u12`), clean at every bound run.

## Teeth (mutants: each must fail, minimal counterexample inside an exhaustive bound)
Enumerator `undo_enum bfs --first` (exhaustive, so the depth found is the least):

| Mutant | Fails | Bound (exhaustive) | Least depth | TLC (one worker) |
|---|---|---|---|---|
| (i) no taint rule | U9 | 2 pids × 1 event + 1 undo (`abb`, 4) | 3 | 3 |
| (iii) `--partial` hides taint | U15 | same | 3 | 3 |
| (iv) redo retries skipped | U17 | same | 3 | 3 |
| (vi) O5 at every window end | O5 | same | 3 | 3 |
| (vii) O5 on the region's base | O5 | same | 3 | 3 |
| skip-U19 | U19 | same | 1 | 1 |
| **(viii) `f3-rediff`**: sides by re-diffing strings (F3) | own_only | same | 3 | 3 |
| **(ix) `restart-single-taint`**: one taint flag read back at both edges | RT | 1 pid × 3 events + 1 revert + 1 restart (`abba`, 4) | 4 | 4 |
| **(x) `provenance-ignored-in-O4`** (v1.8): O4 on element identity, no O4a | I1 | 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 3 | 3 |

TLC with one worker (a shortest counterexample): v1.8, k2-arch job `a75a2670` (every mutant,
the same depths as v1.7 for (i)–(ix)); the smoke job `ac55d0a0` agrees. v1.7: `7a899e60`,
`0c00aee0`. (x)'s witness: seed `ab`, p1 deletes `b`, undoes, reverts its delete: by element
identity both `b`s land at the doc's end (O5), two live elements with one root.

The same set fails `cargo test -p k2-mpc-spec-undo --features mutants --test mutants`. On
walks at the literal G1 bound (20,000 walks each, this Mac, smoke size): f3-rediff fails
`own_only` at depth 3; restart-single-taint (with one restart added) fails `RT` at depth 8.
G1-run mutant walks: TODO.

## Model checking
| Check | Tool | Bound | States | Result | Job / commit |
|---|---|---|---|---|---|
| **TLC = enumerator, exact** (v1.8) | TLC / `undo_enum bfs` | 2 × 1 + 1 undo (`abb`, 4) | 3,889 / 3,889 | equal, no violation | k2-arch `00d8b0dc`, `85262144`; branch `g1/u4-oracle` `cbe337e` |
| | | 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 35,792 / 35,792 | equal | same |
| | | 3 × 1 + 1 undo (`abb`, 4) | 220,210 / 220,210 | equal | same |
| | | 2 × 2 + 1 undo (`ab`, 3) | 542,418 / 542,418 | equal | same |
| | | 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 200,530 / 200,530 | equal | same |
| | | 1 × 1 + 1 undo + 1 revert (`ab`, 4) (new) | 144 / 144 | equal | same |
| | | 2 × 1 + 2 undos + 1 revert (`ab`, 4) (new: restorations meet) | 49,656 / 49,656 | equal | same |
| Exhaustive, G1-shaped | `undo_enum dfs --sym` | 3 × 1 + 2 undos + 1 revert | TODO | TODO | TODO |
| | | 2 × 2 + 2 undos + 1 revert | TODO | TODO | TODO |
| Walks at the literal G1 bound | `undo_enum walk` | 3 × 3 + 2 undos + 1 revert (`abb`, 4) | TODO walks | TODO | TODO |
| Walks, G1 + 1 restart | `undo_enum walk --restarts 1` | same + 1 restart | TODO walks | TODO | TODO |

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
summary prints them). Smoke, this Mac, 20,000 walks at G1 + 1 restart: eof_append 584,
edge_insert 7,100, taint 1,927, partial 1,712, redo_after_partial 4, identical_reports 2,450,
import_kept 12,846, purge_drops 19,406, restart 15,584, exact_vs_rediff 1,900,
touching_regions 2,288, revert_extra_conflict 145. G1 run: TODO.

## ITF replay
Quint simulator traces exported as ITF and replayed step by step into the oracle (action
enabled, cut, outcome, whole state):
- v1.8: 1,000 traces at 2 × 1 + 1 undo + 1 revert (4,000 steps) and 1,000 at 2 × 1 + 2 undos +
  1 revert (5,000 steps), **0 mismatches** (`00d8b0dc`); 300 traces with and 300 without a
  restart, 0 (smoke `ac55d0a0`).
- v1.7: 2 × 1 + 1 undo + 1 revert: 1,000 traces, 4,000 steps, **0 mismatches** (job `7a899e60`);
  300 traces, 1,200 steps, 0 (smoke `0c00aee0`).
- The same with 1 restart: 300 traces, 1,500 steps, **0 mismatches** (`0c00aee0`).
- At G1 (`g1-smoke.sh`): TODO.

## The trace test (oracle vs the kit on Yrs)
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

## What this does not cover (stated for the reviewer)
- **The literal G1 bound is sampled, not exhausted.** 3 pids × 3 events + 2 undos + 1 revert
  is ≥ 10^13 states even with pid symmetry: the walks and Quint simulator traces sample it;
  exhaustive runs cover the G1-shaped bounds that fit (`docs/MODEL-CHECK.md`, "The G1 bound").
- **The concurrency class** (client batches integrated by CRDT merge between undo steps) is
  later; the bounded world and the trace test here are sequential.
- **Native engine**: the trace test compares the oracle with the kit on Yrs only; the native
  engine's run is later.
- Not in the bounded world (the trace test covers them): leave and rejoin, multi-byte text,
  `write --base`, `replace_text`, `restore`, `undo --for`, on-behalf edits, the revert window
  and its persisted carry, the U6 bounds, purge redaction of records.

## Log
- 2026-10-28: v1.8 (provenance): spec, kit, oracle, Quint, counts and mutants filled in;
  the trace test's v1.8 run below; G1-run numbers still pending.
- 2026-10-28: drafted with the v1.7 slice (kit v0.9.0 prep); G1-run numbers pending.
