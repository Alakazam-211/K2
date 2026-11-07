# The undo model check (`MC text-undo`)

The undo spec's model check (`docs/SPEC-UNDO.md` §8 "Model check"): a Quint model of the spec,
replayed against the oracle of record `spec_undo::model` through ITF traces, and an exhaustive
enumeration of the oracle itself, cross-validated against TLC's exact state counts, with
mutants that must fail in both.

| Piece | Where |
|---|---|
| The Quint model | `spec/quint/undo.qnt` (SPEC-UNDO v1.9, over abstract text) |
| Evidence for G1 | `docs/evidence/g1-undo.md` |
| The pinned diff | `spec/quint/undo_diff.qnt`, reached only by the mutants `f3-rediff` and (i); a hand transcription of it equals `diff::hunks` on every pair of texts up to 4 long (`crates/spec-undo/tests/quint_diff.rs`; The diff, below) |
| The bounded world (the same abstraction, in Rust, over the oracle) | `crates/spec-undo/src/bounded.rs` |
| ITF replay | `crates/spec-undo/src/itf.rs`, `src/bin/undo_itf_replay.rs` |
| The enumerator | `crates/spec-undo/src/bin/undo_enum.rs` |
| Mutants | `crates/spec-undo/src/model/mutant.rs` (feature `mutants`), `MUTANT` in `undo.qnt` |
| Node jobs | `tools/undo/job.sh` (a `--repo` job), `tools/undo/*.sh`; the env-chunk fallback `tools/undo-quint-job.sh`, `tools/undo-enum-job.sh` (`tools/README.md`) |

## Provenance of the cited runs

Each job cited here and in `docs/evidence/` says what source it ran (checked 2026-11-04 against
the job records). **No mark:** the record names a commit that is pushed. **(tag `ev-<job>`):**
the record names a commit that was on no pushed branch; the pushed tag `ev-<job>` now holds
that commit. **indicative (bundle):** the source went to the node as env chunks (the env-chunk
fallback), so the record names no commit and the exact source can't be rebuilt from it;
**source stated as <commit>** is the commit the note names for that run, which the record
can't confirm. **indicative (dirty: ...):** a pushed commit plus files that were not
committed. **rerun at pushed `<commit>`: `<job>`:** the run was repeated at that pushed commit;
the job named is the sha-tied rerun, cited beside the marked run, whose own figures stay as it printed. **K2 Engine's record at `<commit>`:** K2 Engine's job (its
record isn't readable from this repo's side); K2 Engine's record names that commit.
**indicative (K2 Engine, uploaded tree); rerun sha-tied by K2 Engine pending:** K2 Engine's
job ran an uploaded tree with no commit. A mark changes no figure: an indicative run's result
is what it printed.

## One abstraction, two implementations

The Quint model is the oracle transcribed definition by definition (each Quint definition names
the oracle function it transcribes), driven by the same bounded world as the Rust side. A Quint
state `st` is exactly a `bounded::View`, so:

- **ITF replay** compares, after every step of a simulator trace: the action is enabled on the
  Rust side, the cut agrees, the outcome is equal (reports as sets; lines are always (1, 1)), and
  the whole state is equal (text with each character's author and root label, every stack, every revert record
  with its per-edge carry, budgets, violations). A difference is reported, never adjusted.
- **TLC's "distinct states found"** equals `undo_enum bfs`'s distinct count at the same bound
  (without `--sym`): both count distinct values of the same state.

### The bounded world

- **Text:** one root over `{a, b}` (no `\n`); each character carries its **author** (0: the
  seed text, an import; pid `k`: author `k`) and (v1.8) its **provenance root**, as a label.
- **Roots as labels** (v1.8): the rules only compare roots for equality and a fresh root
  equals no other, so the state numbers them `0, 1, …` by first appearance in a fixed walk
  (the doc left to right; each pid's undo then redo batches, oldest first, each region's
  `before` roots then `after` roots; the records by id). The oracle names roots (change id,
  ordinal); the view relabels (`View::relabel`), and `undo.qnt` relabels after every step,
  giving a fresh root the least free label.
- **Pids** `p1..pN`, acting for nobody, every right.
- **Events** per pid (`EVENTS`): an edit or a purge. Edits are explicit canonical hunks: insert
  a letter, delete one, replace one with the other letter, delete two, and (`MULTI`) two
  single-letter deletes or replacements at least one letter apart (two regions). A purge
  deletes every live copy of a letter as the purger's batch (canonical hunks: copies that
  touch are one hunk), then clears the stacks (U19).
- **Undos** (`UNDOS`, shared): `undo` all or nothing or `--partial`, `--steps` 1 or 2;
  `undo --drop`; `redo` all or nothing or `--partial`. A request spends one whatever it finds.
- **Reverts** (`REVERTS`, shared): any recorded change, by any pid, all or nothing or `--partial`.
- **Restarts** (`RESTARTS`, shared; v1.7): every stack lost, records kept with each edge's
  taint; with a restart budget the state also holds who made a change and who did before the
  last restart (the "history lost" notice reads them), otherwise those stay empty.
- **Exact hunk sets** (v1.7): every region holds ours (the inverse of its change's hunk, or of
  what an undo applied). **Theirs is read by root** (v1.8, U8): at merge time, off the live
  window against the region's `after` roots (the greedy rule), so it is no longer state: the
  v1.7 carried theirs (regions and record edges) stays in the oracle as a cross-check only.
  Nothing is diffed, but by the mutants `f3-rediff` and (i) on a tainted region.
- **Not here** (the trace test covers them): the revert window, `write --base`,
  `replace_text`, `restore`, `undo --for`, on-behalf edits, the U6 bounds (never reached),
  purge redaction of records (R9's 8-byte windows never match a one-letter secret). So
  **`erased` is not modelled**: no region is ever erased, and the merge's `erased` branch,
  U9's `!erased` clause and the Erased report are never reached by any model-check run
  (Invariants, U9).

### Reductions (each machine-checked; the cut is a bound, not exact)

| Reduction | Why it is exact | Check |
|---|---|---|
| **The cut**: no state with a text longer than `MAXLEN` | **not exact: a bound**, like the budgets. An action that would make a longer text is not taken, in both implementations (`bounded::State::step` returns `None` before `check`; `undo.qnt`'s `step` disables it, `r.ok`), so **that transition is never checked**. The bfs SUMMARY's `cut=` counts them: 624 at 2 × 1 + 1 undo + 1 revert (`abb`, 4), 18,912 at 3 × 1 + 1 undo, 30,104 at 2 × 2 + 1 undo (`ab`, 3), 17,790 at 1 × 3 + 1 revert + 1 restart (jobs `f57e1522`, `5d514715`, `95a4221d` (tag `ev-95a4221d`)). At `MAXLEN` 5 the 2 × 1 + 1 undo + 1 revert bound takes those 624: 39,790 states, no violation (`95a4221d`; the enumerator only) | `bounded::tests::the_cut_bounds_every_diffed_string` (the states kept stay within `MAXLEN`; it does not check the cut transitions) |
| **Records dropped** once the revert budget is spent (and `head`) | nothing reads a record again | `bounded::tests::dropping_spent_records_is_exact`: with and without it, the same states (minus records). It also asserts the same violations, but it runs the oracle of record, which has none, and no mutant runs there: **that half compares empty with empty** |
| **Fields no rule reads** are not state: a record's original hunks, a batch's `after_partial` and `on_behalf_of`, the authors sets | no rule or outcome reads them | the ITF replay compares every state field the model has |
| **The view** (v1.8): roots as labels, the carried theirs and a reduced head left out | the rules compare roots only for equality; theirs is read by root from the live window; nothing reads a head once the records are dropped | `bounded::tests::the_view_is_exact`: oracle states found with one view (4,000 pairs at 2 × 1 + 2 undos + 1 revert, 1,780 of them under other root names) have the same actions, outcomes and next views |
| **Pid symmetry** (enumerator, `--sym`; not in TLC) | pids have the same budgets and rights | `bounded::tests::pid_symmetry_is_exact_*` (one test per bound): the canonical forms of the plain enumeration's states are exactly the symmetric enumeration's states. The "same violations" half runs the oracle of record, which has none, with no mutant: **it compares empty with empty** |

### The diff (D1–D2)

The merge diffs with `diff::hunks`. Quint can't call it, so `undo_diff.qnt` **pins it**. On
texts over `{a, b}` (no whitespace, no newline) `diff::hunks` takes one path: trim the common
prefix and suffix; if what is left of either side is empty, one hunk; if one side is a
subsequence of the other, the character-level Myers diff is one-sided; otherwise the two
middles are one replacement hunk. The only choice left is which characters a one-sided diff
keeps, and on every pair of texts up to 4 long Myers keeps the **right-greedy embedding**
(the latest matching characters). `undo_diff.qnt` is that algorithm.

**Only mutants reach it.** In `undo.qnt`, `hunks` is called only under mutant `f3-rediff`
and under mutant (i) on a tainted region (`mergeRegions`); the correct model never diffs.
So no count, ITF replay or TLC run of the correct model depends on the pinned diff.

**What is checked is a hand transcription.** `crates/spec-undo/tests/quint_diff.rs`
transcribes the module into Rust by hand and checks that the transcription equals
`diff::hunks` on **every** pair of texts over `{a, b}` up to 4 long (961 pairs), and that
it differs at 5 (4 is a hard limit, `DIFF_MAX`). That part is exact. Nothing mechanically
ties the transcription to the `.qnt` file: the third test
(`the_quint_module_names_the_limit`) only checks that the file names `DIFF_MAX = 4`. The
cut keeps every text the merge diffs (every region's before, after and window) within
`MAXLEN` (`the_cut_bounds_every_diffed_string`), and every instance compared with Quint so
far has `MAXLEN ≤ 4`. **That is a convention, not enforced:** `Bound`, `undo_enum` and
`undo_itf_replay` accept any `--maxlen`, and `undo.qnt` any `MAXLEN`. Under it, on every
state the bound reaches, the transcription **is** `diff::hunks`, and the model's diff is
too as far as the module matches the transcription.
`align_insertions` is not involved (the merge uses `diff::hunks` alone; the bounded world's
edits are explicit hunks). A first version pinned the diff as a generated table of 3,906
entries; Quint's typechecker took over 8 minutes on it, so the table gave way to the
algorithm.

## Invariants (by rule id)

Every transition records the rules it violates in `viol`; each invariant says one rule never
appears.

| Invariant | Rule |
|---|---|
| `own_only` | `hist.undo_own_only` (U10a; by root since v1.8): every character an undo, redo or revert deletes has a root the change it reverses inserted (an undo of a batch: its inserted roots; a redo: the roots the undo put back; a revert of `c`: `c`'s inserted roots), **in the regions the step applied** (v1.9). **Enforced** (stopped at) since v1.7 |
| `I1` | (v1.8) at most one live element per provenance root, in every state, and (v1.9) after every step of an undo or redo: each step is its own change |
| `RT` | restart transparency (U22a v1.6): a restart changes no revert's outcome or text, for every record, all or nothing and `--partial` (v1.7) |
| `U9` | a tainted region is never applied (by construction; its `erased` case is not modelled: below) |
| `U13` | a refused all-or-nothing request, or a blocked one, changes nothing |
| `U15` | all or nothing's conflicts = `--partial`'s reports, first conflicting step |
| `U17` | a step pushes one region per region it applied: redo reapplies exactly the applied regions |
| `U19` | a purge leaves every stack empty |
| `U22a` | revert's conflicts ⊇ undo's on the same state, same bounds, same text where both apply |
| `O5` | O5 lands exactly at the doc's end with a non-empty whole-doc base: never elsewhere, never refused there (U10's `at_end`, both ways) |
| `O5doc` (2026-11-04) | O5 and U10's `at_end` **restated independently** of the merge's own `at_end`: for a request that merged one step (a revert, or an undo or redo of one batch), each untainted region judged from its own exact sides, the doc's length as the pre-state holds it, the whole-doc base flag its batch or record stored, and the text the step left (landed elsewhere; refused at the doc's end; or landed there but the text left doesn't end with ours). `bounded::o5_doc`, `undo.qnt` `o5doc` |
| `U17doc` (2026-11-04) | U17 read off the state an applied undo or redo left: the batches it pushed on the opposite stack hold one region per region each step applied, in order |
| `U12` (Rust only, `--u12`) | a diagnostic: outside taint, the window merge of the exact sets equals the whole-text merge of the same sets (v1.7). Clean; the old re-diff form (F4) is informative only |

### What the runs check

- **A transition past the length cap is never checked**, in the enumerator and in the model
  (Reductions, the cut).
- **`bfs` and TLC** check every transition they take. A violation is part of the state, so
  a violation on a transition into a seen state was recorded when that state was first
  reached.
- **A walk** (`undo_enum walk`, with or without `--bias`) computes every enabled step at each
  state (each runs the checks), picks one, and records **only the picked step's
  violations**. A violation on a step it did not take is not reported.
- **Case-class counts differ by mode.** A walk counts per step taken. `bfs` counts **per
  first visit**: a class is counted on the transition that reaches a new state, and a
  transition into an already-visited state adds nothing. So a bfs class count of 0 does not
  mean no transition hits the class. At the seven exact bounds the bfs counts 0
  all-or-nothing undo refusals at six bounds and 0 blocked requests at all seven, while
  200,000 G1 walks count 12,633 and 7,487 (audit job `95a4221d`, tag `ev-95a4221d`), and TLC reaches both at
  2 × 1 + 2 undos + 1 revert (`240529ae`, tag `ev-240529ae`).

### Six rules hold by construction

**Six of the ten rules hold by construction on the oracle and the model: U9, U13, U15, U17,
U19 and O5.** The code that computes a step already makes each true, so no state of the
correct oracle or model can break it. **Their evidence is their mutants, not their
COMPLETE:** a COMPLETE shows only that the code implementing each rule agrees with itself.

- **U9:** a tainted region takes the conflict branch with `clean` false (`model/mod.rs`
  ~1253-1257; `undo.qnt` `mergeRegions`), so "tainted and clean" can't occur. Mutant (i).
  **U9's `!erased` clause is never reached**: no code in `undo.qnt` sets a region's
  `erased`, and the bounded world never redacts a record (one-letter secrets; R9's 8-byte
  windows), so the merge's `erased` branch, the clause and the Erased report are reached by
  no model-check run: TLC cover COMPLETE (`240529ae`, tag `ev-240529ae`), the oracle 0 at all seven bounds and
  in 400,000 G1 walks (`95a4221d`, tag `ev-95a4221d`). The only evidence for the erased clause is the trace
  test's class `revert_over_erased`: 31 hits in 256 traces (job `38fdc54f`, tag `ev-38fdc54f`, a run of
  `trace_equivalence` with `--nocapture` at main's spec-undo code plus two counters).
- **U13:** undo and redo plan on a scratch copy and return before `*self = work`; revert
  returns before `new_batch` (`undo.qnt`: the conflicts and blocked branches return the
  input state), so a refusal can't change the state. Mutant `u13-refused-revert-applies`
  (added by the audit; results below) shows its check can fail.
- **U15:** both modes plan identical steps up to the first conflicting one and report the
  same set there. Mutant (iii).
- **U17:** `apply_step` pushes exactly one inverse region per clean region. Mutant (iv).
- **U19:** `purge_text` always clears every stack. Mutant skip-U19.
- **O5:** the check's "at the doc's end" (`end == live.len() && base_doc_nonempty`) is the
  same expression as the merge's `at_end` (`model/mod.rs` ~1268, `undo.qnt` `atEndFor`);
  `o5_landed` requires `at_end` and `same_point_at_window_end` requires `!at_end`
  (`merge.rs` ~253-257), so neither failure state is reachable. TLC, with each failure
  state as a cover: on the correct model neither is reached (both COMPLETE at 2 × 1 + 2
  undos + 1 revert, `240529ae`, tag `ev-240529ae`); under mutant (vi) only the landing-off-the-end state is
  reached, under (vii) only the refusal-at-the-end state.

**The genuinely relational checks are `own_only`, `I1`, `RT` and `U22a`.** No TLC or bfs
run at the documented bounds had shown U22a can fail. Its only failing witnesses are
mutant (i) in G1 walks (`48962ffa`) and, now, mutant (i) at 2 pids × 2 events + 1 revert
(`abb`, 4): TLC `Invariant m_u22a2_undo_inv_U22a is violated`, 5 states, and the enumerator
`U22a@4` (job `b9742756`, tag `ev-b9742756`). Its control is job `528c615e` (tag `ev-528c615e`):
TLC on `undo.qnt` at 2 pids × 2 events + 0 undos + 1 revert (`abb`, 4, `MULTI`, purge,
`REDUCE`), no mutant, only `inv_U22a` checked: COMPLETE, 7,059,123 states, depth 6.

## Readings

The oracle's readings (R1–R10, U22a; `crates/spec-undo/src/model/mod.rs`) carry over. Added
here: a request spends the undo budget whatever it finds. A purge's hunks are **canonical**:
copies that touch are one hunk (D2), so one region (U8), and that hunk taints another
region whose edge it deletes across (U9), as `purge_text` and the kit build them. **The
earlier, per-copy reading** (a purge's removal as one hunk per copy of the letter, touching
hunks not joined, so one region each and no taint between them) held in the oracle until
`04487c5` and in the model until the model revision of 2026-10-29 (`cd9840c`). Every count
and walk at a bound with a purge and a revert taken before those commits is under that
reading; the exact counts and the G1 exhaustive pair were re-run under the canonical one
(`docs/evidence/g1-undo.md`).

## Findings

**Both fixed in v1.7** (spec U10 on exact hunk sets; kit v0.9.0). F3 is now mutant
`f3-rediff` (fails `own_only` at depth 3, inside 2 × 1 + 1 undo), and U12 is restated on
hunk sets (clean at every bound run). Kept as found:

**F3: the oracle (and the kit) violate `hist.undo_own_only` per element.** Least depth 3:
seed `ab`; p1 replaces `a` with `b` (`bb`); p2 inserts `b` at 0 (`bbb`), which is pulled into
p1's region at its start (U8). p1's undo merges `base = "b"`, `ours = "a"`, `theirs = "bb"`:
`diff::hunks(base, theirs)` trims the common prefix and reports p2's insertion **after** the
region's own `b`, so ours' replacement lands on p2's `b`. The text is right (`abb`); the
elements are wrong: p2's `b` is deleted and p1's stays, and p2's own undo then conflicts.
Confirmed on the kit on Yrs (char blame). Same class as F1 (v0.8.1), on the theirs side: the
window's foreign changes are found by a text diff, which can't see element identity in
repeated text. A fix needs the merge to know where foreign insertions inside the window are
(element positions the engine reports), a spec change for the kit's owner.

**F4: U12's equivalence note fails in repeated text.** Least depth 3: seed `ab`; p1 inserts
`a` at 0 (`aab`); p2 replaces it with `b` (`bab`). p1's undo conflicts in the window form (O1:
p2 replaced p1's letter), which is right by identity; whole-text merge3 of the same inverse is
clean (its diff deletes the seed's `a` instead). U12 says the region form is the definition,
so the definition stands; the note's claim "U10 equals whole-text merge3 outside the taint
case" needs "outside repeated text" (or to be dropped).

## The audit's additions (2026-11-04)

An audit of the model check found that six of the ten rules hold **by construction** on the
oracle and the model (U9, U13, U15, U17, U19, O5), so their evidence is their mutants alone;
that U13 had no mutant; that `own_only` failed only through undo (`f3-rediff`), never through
redo or revert; that no mutant broke I1 at an intermediate step only (v1.9's per-step flag:
`commit` in `undo.qnt`, `Hits::i1_violated` in the oracle); and that the `O5` check read the
flags the merge computed from its own `at_end` (`end == live.len() && base_doc_nonempty`;
`atEndFor`), so it could not fail on correct code. No behaviour of the oracle changed: the
additions are four mutants, two independently formulated checks and a debug-assertion job.

**Two caveats, stated up front:**

- **`O5doc` covers single-step requests only** (a revert, or an undo or redo of one batch).
  An `undo --steps 2` whose two steps both merge rests on the old `O5` check alone, which
  reads the merge's own flags (below, "Independent formulations").
- **`i1-per-request`'s Rust test is a replayed witness, not a search.** At its own bound a
  full `bfs` of the oracle runs out of memory (20 GB), so `tests/mutants.rs` replays the
  least counterexample and checks it, and its least depth (5) rests on the enumerator's
  `--first` run and TLC's one-worker run (`counts.sh` `PART=m`). **That bound turns off
  two-region edits and purge** (2 × 2 + 1 undo + 1 revert, seed `a`, `MAXLEN` 3), only to
  keep TLC's search small: the witness uses neither. **It is not in the G1 walks**: from
  `abb` with `MAXLEN` 4 its least witness at G1 is six exact steps, which uniform walks
  don't draw (0 of 20,000, job `2668cf01`); the other three are (below).

### Four mutants (`Mutant::ALL`, `MUTANT` in `undo.qnt`, `counts.sh` `PART=m`, `smoke.sh`)

| Mutant | Rule | Bound | Least depth: enumerator / TLC (one worker) | Violating action |
|---|---|---|---|---|
| `u13-refused-revert-applies`: a refused all-or-nothing revert still applies its clean regions | U13 | 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 3 / 3 | a revert (all or nothing) |
| `r3-redo-rewrites-window`: a redo rewrites each clean region's whole window (R3) | own_only | 2 × 1 + 2 undos + 1 revert (`ab`, 4) | 4 / 4 | a redo |
| `r3-revert-rewrites-window`: the same, for revert only | own_only | 2 × 1 + 1 undo + 1 revert (`ab`, 4) | 3 / 3 | a revert |
| `i1-per-request`: v1.8's I1, per request | I1 (per step only) | 2 × 2 + 1 undo + 1 revert (`a`, 3; no two-region edits, no purge) | 5 / 5 | an `undo --steps 2` |

Jobs `66736440` (`counts.sh` `PART=m`) and `03aa049f` (the same on the committed model): the
same depths, TLC's `violated` line naming the mutant's own invariant (`inv_U13`,
`inv_own_only`, `inv_own_only`, `inv_I1`) and the enumerator's `SUMMARY` reading
`violations=[U13@3]`, `[own_only@4]`, `[own_only@3]`, `[I1@5]`. `tests/mutants.rs`:
`refused_revert_applies_fails_u13`, `redo_rewrites_window_fails_own_only_through_redo`,
`revert_rewrites_window_fails_own_only_through_revert` (each asserting the violating
action's kind), `i1_per_request_fails_i1_at_an_intermediate_step`, and the oracle at the two
new bounds (`the_oracle_passes_at_the_revert_bound`, `…_restorations_bound`); job `ba6a024c`.

Each fails its own rule and no other at that depth (the enumerator's `--first` lists every
rule violated at the level it stops on: one each), and each leaves the oracle as it was (the
`--mutants` feature only).

- **`u13-refused-revert-applies`**: p1 deletes the first and the last letter of `abb` (two
  regions: `b`); p2 types `a` at 0 (`ab`, into the first region's window); p1's
  all-or-nothing revert conflicts on the first region and applies the second anyway
  (`abb`), reporting the conflict. TLC: `Invariant m_u13_undo_inv_U13 is violated`, 4
  states; the enumerator: `violations=[U13@3]`. The same trace in both.
- **`r3-redo-rewrites-window`** (and `r3-revert-…`): the step applies, for each clean region,
  one hunk deleting its whole window and putting the merged text back, each character with
  the root the merge gives it. Text and roots come out as the oracle's; a foreign element in
  the window is deleted (and put back as a new element), which U10a forbids. Redo's least
  witness (enumerator and TLC alike): p1 types `a` at 0 (`aab`); p1 undoes (`ab`); p2 reverts
  p1's undo (`aab`, the root back); p1 redoes, rewriting a window that holds p2's
  restoration: `own_only`. In TLC's trace the last transition raises `undos` from 1 to 2,
  moves p1's batch from its redo stack to its undo stack and leaves `events` and `reverts`
  alone: a redo. Revert's (depth 3): p1 types `a` at 0, p2 types `a` at 0 (pulled into p1's
  region, U8), p1 reverts its change; the last transition raises `reverts`.
- **`i1-per-request`**: O4a is waived on every step but the last of a multi-step request
  when the request, so planned all or nothing, applies and leaves each root live at most
  once; otherwise the request is planned as specified. The decision is the same for both
  modes, so U15 can't see it, and no end state ever holds a root twice: only the per-step
  check can fail. Least witness: p1 appends `a` (root R), p1 deletes it, p2 appends `a`, p2
  reverts p1's delete (R back after p2's `a`), p1 `undo --steps 2`. The first step puts R
  back beside p2's `a` at the doc's end (theirs inserts `a` and R, ours R: not identical,
  O5 lands both; O4a, waived, would refuse), two live R; the second undoes p1's append and
  removes one. TLC's last state: `doc` roots `0, 1, 2` (no duplicate), `viol {"I1"}`, `undos`
  0 → 1; the enumerator ends at `aaa`, authors `[0, 2, 1]`. `tests/mutants.rs` replays the
  witness: the oracle refuses the request (O4a, step 1), the mutant applies both steps and
  its end state passes `Model::i1` with `I1` in `viol`. The bound's whole oracle run doesn't
  fit a test (about 10^6 states at depth 5 alone, over 20 GB for a full `bfs`), so its least
  depth is the jobs'. TLC took 496 s with one worker (301,577 states); the enumerator 52 s.

**At the literal G1 bound, on walks.** The rows `g1.sh` and `g1-smoke.sh` declare
(`tools/undo/expect.sh`), with their verdicts, run at the branch head `37c36196`:

| Mutant | Walk row (`g1.sh` walks-mutants, 50,000 walks; `g1-smoke.sh`, 20,000; walks-bias declares the same rows) | Its own rule, at its own bound (`counts.sh` `PART=m`, job `2ee1b04f`) |
|---|---|---|
| `u13-refused-revert-applies` | VIOLATION at U13: `violations=[U13@3]` (jobs `5d33fce7`, `2668cf01`) | U13, depth 3, TLC and the enumerator |
| `r3-redo-rewrites-window` | VIOLATION at own_only: `[own_only@4]` (same jobs) | own_only, depth 4 |
| `r3-revert-rewrites-window` | VIOLATION at own_only: `[own_only@3]` (same jobs) | own_only, depth 3 |
| `i1-per-request` | **not in walks** (six-step least witness; 0 of 20,000 drew it): `g1-smoke.sh` declares its 20,000 walks SAMPLED, `violations=[]` (job `2668cf01`); left out of `g1.sh`'s lists (`NW`, the comment beside `AUDIT_MUTANTS`) | **caught exhaustively** at its own bound: I1, depth 5, TLC (one worker) and the enumerator |
| (iv) `iv-redo-retries-skipped` | stopped by the oracle's own shared-edge assertion at depth 5 ("a shared edge's elements must be foreign"; the enumerator's pseudo-rule for a failed assertion), **not U17**; its walk row accepts either | U17 (and U17doc), depth 3: its own-bound rows `tlc@iv-redo-retries-skipped/U17`, `enum@…/U17` (job `2ee1b04f`) |

`i1-per-request`'s least witness at G1, from `abb` with `MAXLEN` 4: p1 deletes a letter,
appends R, deletes R; p2 appends; p2 reverts p1's delete; p1 `undo --steps 2`. Uniform walks
don't draw those six steps. The three caught on walks fail their own rule only.

**Export check and counts at that head** (job `2ee1b04f`, `counts.sh` `PART=abm`, `=== EXPECT:
GREEN: all 51 declared rows ran`): the export guard compared 113 definitions, 0 differ; TLC =
enumerator, COMPLETE, at all seven bounds (3,889; 35,758; 220,210; 144; 542,418; 199,268;
49,644); ITF 0 mismatches; every mutant at its rule, `i1-per-request` I1 at depth 5 in both
tools (TLC 301,577 states), (iv) U17 and U17doc on its one-rule rows.

### Independent formulations

| Check | Restates | Fails under | Enumerator (`--stop` it alone) | TLC (that invariant alone, one worker) |
|---|---|---|---|---|
| `O5doc` | O5, U10's `at_end` | (vi) `vi-o5-every-window-end` | `violations=[O5doc@3] observed=[O5@3]` | `Invariant m_tiny_undo_inv_O5doc is violated`, 4 states |
| `O5doc` | | (vii) `vii-o5-region-base` | `violations=[O5doc@3] observed=[O5@3]` | the same, 4 states |
| `U17doc` | U17 | (iv) `iv-redo-retries-skipped` | `violations=[U17doc@3] observed=[U17@3]` | `Invariant m_tiny_undo_inv_U17doc is violated`, 4 states |

**`O5doc`** (`bounded::o5_doc`, `undo.qnt` `o5doc`) never reads the merge's `at_end`, its
`o5_landed` or its `same_point_at_window_end`. For a request that merged one step, each
untainted region is judged from its own exact sides (an insertion of ours met by a different
insertion of theirs at the same point, O4 by roots), **the doc's length as the pre-state
holds it**, **the whole-doc base flag its batch or record stored** when the change was made,
and **the text the step left**: such a region violates O5 when it merged clean with the pair
anywhere but the base's end of a window ending at the doc's end with a non-empty base; when
it was refused there though the pair is all there is to merge (one insertion each side) and
O4a didn't refuse it; or when it landed there, applied, and is the step's last region, but
the text the step left doesn't end with ours' letters and roots (theirs first, then ours).
Requests of two steps are left to `O5`: a Quint result doesn't split its region notes by
step, and carrying that would grow the IR. **`U17doc`** reads the batches an applied undo or
redo pushed off the stacks it left, not the step's own count.

**Not restated**, and why: **U9** (the oracle refuses a tainted region in one `if`; an
independent reading needs each region's own edits traced through the step, which neither
side records); **U13** and **U19** (their checks already compare the states before and after,
not anything the code computed; U13 now has its mutant); **U15** (its check already plans
the other mode; a second, independent planner is not cheap).

**Clean on correct code** at the seven exact bounds, on the enumerator (explore job
`57835476`: the counts below, `violations=[]` with both new rules in the stop set) and on
TLC (`PART=a`, `b` with every invariant, both new ones included: jobs `3e209dc0`, `03aa049f`, `9037e1fe`); `the_oracle_passes_at_*` (job `ba6a024c`).
Every older mutant fails as before (job `66736440`: the same rules at the same depths in
both tools; (vi), (vii) also `O5doc@3`, (iv) also `U17doc@3`, TLC naming the earlier
invariant in the configuration's order, `inv_O5` or `inv_U17`).

### Debug assertions (`tools/undo/debug.sh`)

The release jobs build without debug assertions, so the oracle's own `debug_assert!`s never
ran in a model job. `debug.sh` (`sh tools/undo/job.sh enum tools/undo/debug.sh`) rebuilds
`undo_enum` with `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` in its own target dir and runs
the oracle at the seven exact bounds and on walks at the literal G1 bound, with and without a
restart. Job `16f98d6f`: the
seven counts as below, no rule violated, no assertion failed; 100,000 walks at G1 (1,200,000
steps) and 100,000 with a restart (1,300,000 steps), none violated. About 12 minutes on 4
threads (the build 82 s).

### Counts after the changes (TLC = enumerator)

Unchanged at all seven bounds, on the model as committed (job `03aa049f`, and `9037e1fe`
for `PART=b`) and before the IR reduction below (job `3e209dc0`):

| Bound | TLC = enumerator |
|---|---|
| 2 × 1 + 1 undo (`abb`, 4) | 3,889 |
| 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 35,758 |
| 3 × 1 + 1 undo (`abb`, 4) | 220,210 |
| 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 144 |
| 2 × 2 + 1 undo (`ab`, 3) | 542,418 |
| 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 199,268 |
| 2 × 1 + 2 undos + 1 revert (`ab`, 4) | 49,644 |

ITF replay: 1,000 traces at 2 × 1 + 1 undo + 1 revert, 1,000 at 2 × 1 + 2 undos + 1 revert
and the regression traces (`PART=a`), 300 with and 300 without a restart (`smoke.sh`): **0
mismatches**. The Quint export guard: 113 definitions compared, 0 differ.

**The model's IR** (`smoke.sh`, `u_tiny`): **21,334,452 bytes**, against 19,862,706 for main's
model before these changes (`ab16c7e4`, the same instance, job `03aa049f`): +1.47 MB, past
Jackson's default of 20,000,000 (the export check's instance was already past it), so every
Quint job depends on `tools/quint-jvm`'s agent (limit 200,000,000), as it already did. The first
draft was 22,522,104 (job `16f98d6f`): every expression whose type holds the whole state
(`St`, or `Res` around it) carries that type, about 90 KB each. The committed model projects
what `checks` reads once (`nts`, `sts`, `kind`, `post`, the stacks and records; the existing
checks now use them too), passes maps rather than states to `stepRegions` and `oppStack`,
and folds the redo rewrite into `applyStep`, which already holds the window's roots.

**The agent is required, and its absence stops the job** (follow-up, 2026-11-04). The IR's
anatomy (`quint compile --target json` of `u_tiny`, bytes by part; job `c20c1e8a`): `types`
13.0 MB, the name `table` 7.3 MB (an instance's import keeps each name twice, qualified and
not), `effects` 3.1 MB, the expressions themselves 0.6 MB. By definition, `undoRedoW` holds
1.6 MB, `perform` 0.6, `revert` 0.6, `checks` 0.6, `commit` 0.5. **Moving the mutants into a
module of their own was not done**:

- it would not get the base under 20 MB: the mutant-only definitions and branches (the
  `undoRedo` wrapper and `perRequest`, `rewrite`, the old mutants' branches in
  `mergeRegions`, `undoRedoW`, `purge`, `restart`) come to about 1 MB of the 21.3, and the
  export check's instance is a further ~0.14 MB over `u_tiny`;
- Quint has no overriding: every mutant changes a core definition (`mergeRegions`,
  `undoRedoW`, `revert`, `purge`, `restart`), so a mutant module would carry its own copies
  of most of the model, kept equal to the base by hand.

Instead every Apalache server a job starts goes through `tools/quint-jvm/require.sh`
(`qjvm_require`): without the agent in `APA_JAVA_OPTS` the job stops before its first
Apalache request, in one line naming the 20,000,000 limit and the IR's size. The planted run
(job `ce625b9f` at `37c36196`, `APA_JAVA_OPTS` set to a non-agent option) stops at its export check,
exit 1, through `tools/undo/expect.sh` (`=== EXPECT: RED: 1 problem(s)`, the missing agent):

    === quint-jvm: the export check's Apalache server: no Jackson limits agent in APA_JAVA_OPTS
    ('-Dplanted.no.agent=1'). Apalache reads the model's IR as one JSON string of at most
    20,000,000 characters (Jackson's default limit); this model's IR (q_export_check) is
    21471884 bytes. Stopping the job before any Apalache request (tools/quint-jvm/README.md).

Every Quint path, and where it sets or checks the agent:

| Path | Apalache? | Agent |
|---|---|---|
| `tools/undo/job.sh` quint: Apalache's fetch | a server quint starts itself | the one-line module `q_fetch` (`cat > q_fetch.qnt`), never the model's IR |
| `tools/undo/job.sh` quint: the export check's server (`lint.sh --export`, every compile through the port shim) | yes | `APA_JAVA_OPTS=$(sh … agent.sh …)`, then `qjvm_require "the export check's Apalache server"` before `java ${APA_JAVA_OPTS} … server`; stops if the server didn't start (no fallback to quint's own) |
| `tools/undo/tlc.sh` `apa_up` (every `tlc` call: `smoke.sh`, `counts.sh` a/b/m, `diff.sh`) | yes | the same, `qjvm_require "tlc.sh's Apalache server"`, and the same start check |
| `smoke.sh`, `counts.sh`, `g1-smoke.sh`: `q run`, `q typecheck`, `q compile --target json` | no (quint alone) | not needed |
| `tools/undo/g1.sh`, `debug.sh` | no (enumerator only) | not needed |
| `tools/undo-quint-job.sh` / `undo-bundle.sh` (the fallback) | through `tools/undo/job.sh` | as above |

### Jobs (an x86 compute node, `--cpus 4 --mem 20G`)

| Job | What | Result |
|---|---|---|
| `57835476` | enumerator only: the oracle at the seven exact bounds; every older mutant; the new mutants at candidate bounds | the seven counts unchanged, no violation; every older mutant as before; the four caught |
| `1994ea8e` | enumerator only: `i1-per-request`'s bound at `MAXLEN` 3 | caught at depth 5 (with and without purge); the oracle's whole bound didn't finish (a 600 s cap, then the 20 GB memory cap) |
| `66736440` | `counts.sh` `PART=m` | every mutant caught in both tools at the depths above; `O5doc`, `U17doc` alone under (vi), (vii), (iv) |
| `ba6a024c` | `tools/tests/full.sh` `PART=lint`, then `PART=spec-undo` | fmt, clippy (with and without `mutants`) clean; 62 + 27 tests pass |
| `3e209dc0` | `counts.sh` `PART=ab` (the first draft) | the seven counts equal; ITF 2,000 traces and the regressions, 0 mismatches |
| `16f98d6f` | `smoke.sh`, then `debug.sh` (the first draft) | smoke clean (IR 22,522,104); debug assertions as above |
| `03aa049f` | the committed model: `PART=abm`, `smoke.sh`, the IR before | export check 0 differ; `PART=m` as `66736440` (the same states and depths in TLC; `i1-per-request` 301,577 states, 818 s); six exact counts equal, ITF 0 mismatches; 2 × 2 + 1 undo stopped at its 540 s TLC cap on a slower node (PARTIAL); IR 21,334,452 (19,862,706 before) |
| `9037e1fe` | the committed model: `PART=b` | the three counts equal (542,418, 199,268, 49,644; TLC complete), no violation |


## Results v1.9 (fidelity review of v1.8)

### What changed

No change to what the kit does; the rules are read more tightly and the checks follow:

- **U10a over the applied regions.** `own_only`'s set is the base roots the ours of each
  region the step **applied** deletes; v1.8 took every region of the batch, a skipped one
  included, which only I1 made the same. Oracle (`own_roots(regions, applied)`), Quint
  (`ownRoots(rs, applied)`), the enumerator (it reads the oracle's flag) and the trace
  test's kit-side check (the kit's removals against the oracle's applied-region roots, by
  the element correspondence, on top of the change-level check).
- **I1 after every step.** The kit commits each step of an undo or redo as its own change
  (its own change ID, log entry and revert record; `commit` takes one `Step` per planned
  step, in one durable append), and its live-root index asserts I1 as each step's
  restorations enter it. The oracle checks I1 after each applied step
  (`Hits::i1_violated`), the enumerator reads it, and the Quint model flags `I1` in `viol`
  in `commit` (every change, so every step), kept by `nextOf`.
- **O4a's wording**: the live text as the step merges it (after the earlier steps of the
  request), window included. The oracle and the model already read it so.
- **Mutant (xi) `xi-no-o4a`**: O4 on (range, roots) as specified, no O4a.

### Counts: v1.8 → v1.9 (TLC = enumerator)

The changes reach no state at the small bounds (the oracle never breaks I1, so the two
readings of U10a agree and no step-only I1 failure exists): every count is v1.8's and no
violation set changed.

| Bound | v1.8 (TLC = enumerator) | v1.9 enumerator | v1.9 TLC | TLC wall |
|---|---|---|---|---|
| 2 pids × 1 event + 1 undo (`abb`, 4) | 3,889 | 3,889 | 3,889 | 3 s |
| 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 35,792 | 35,792 | 35,792 | 22 s |
| 3 × 1 + 1 undo (`abb`, 4) | 220,210 | 220,210 | 220,210 | 115 s |
| 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 144 | 144 | 144 | 1 s |
| 2 × 2 + 1 undo (`ab`, 3) | 542,418 | 542,418 | 542,418 | 266 s |
| 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 200,530 | 200,530 | 200,530 | 123 s |
| 2 × 1 + 2 undos + 1 revert (`ab`, 4) | 49,656 | 49,656 | 49,656 | 63 s |

ITF replay: 1,000 traces at 2 × 1 + 1 undo + 1 revert (4,000 steps) and 1,000 at 2 × 1 + 2
undos + 1 revert (5,000 steps): **0 mismatches**.
**Current counts (note of 2026-10-31; the rows above are history, as recorded then).** Three of
these bounds count fewer states since the model's purge revision of 2026-10-29 (canonical purge
hunks, as the trace oracle since commit `04487c5`; under the old per-copy reading a purge's removal
was one hunk per copy): 2 × 1 + 1 undo + 1 revert **35,758** (was 35,792), 1 × 3 + 1 revert + 1
restart **199,268** (was 200,530), 2 × 1 + 2 undos + 1 revert **49,644** (was 49,656). The other
four are unchanged: 3,889, 220,210, 144 and 542,418. TLC = enumerator at all seven. Recorded in
`docs/evidence/g1-undo.md` (runs `88555d19`, `ee85bf3c`; the re-runs after the revision `3f360ca1` (tag `ev-3f360ca1`),
`708bd100` (tag `ev-708bd100`)). Latest: v0.10.0's merge commit `260ef4e`, jobs `f57e1522` (`counts.sh` `PART=a`: the
first four bounds; ITF 2,000 traces and the regression traces, 0 mismatches) and `5d514715`
(`PART=b`: the other three), the same seven counts, TLC = enumerator.

**Violation sets.** On the enumerator without `--first` (every rule's least depth, the whole
bound), v0.9.0's binaries against these, at every bound above and at each mutant's bound:
**identical**: the same distinct counts and the same rules at the same least depths
(mutants (i)–(x) each only their own rule; the oracle none; (x) at 1 × 1 + 1 undo + 1
revert, 2 × 1 + 1 undo + 1 revert and 2 × 1 + 2 undos + 1 revert: `I1@3` only, no
`own_only`). (xi), on these binaries only: clean at 1 × 1 + 1 undo + 1 revert (144 states);
`I1@4` only at 2 × 1 + 1 undo + 1 revert (14,697 states) and at 2 × 1 + 2 undos + 1 revert
(49,830). Removing O4a breaks I1 but not U10a: what the second restoration removes is still
its own root's.

### Mutant (xi): least depth

| Mutant | Rule | Bound | Enumerator (depth) | TLC, one worker (actions) |
|---|---|---|---|---|
| **(xi) `xi-no-o4a`** (v1.9) | **I1** | 2 × 1 + 1 undo + 1 revert (`ab`, 4) | 4 | 4 |

Not at 1 × 1 + 1 undo + 1 revert (144 states, clean: the one pid's one event is the delete,
so nothing foreign lands beside the restored letter, and O4 takes the second restoration
once). Every other mutant fails as in v1.8 (same rules, same depths).
**(xi)'s least witness is at the doc's end** (the oracle's `o4a_at_the_end`): p1 deletes `b`
(`a`); p1 undoes (`ab`, the new `b` with the seed `b`'s root); p2 types `a` before it
(`aab`); p1 reverts its delete: theirs inserts `a` and the restored `b`, ours `b`; not
identical by roots (O4), so O5 lands both (`aabb`) and two live `b` share a root. The
**mid-text witness** (the oracle's `o4a_mid_text`) needs two undos: depth 5 at 2 × 1 + 2
undos + 1 revert (inside `PART=exhaustive`'s 2 × 2 + 2 undos + 1 revert); it is in
`tests/mutants.rs` (`no_o4a_fails_i1_in_mid_text`: the oracle blocks p2's `undo --partial`
by O4a, the mutant applies it and fails I1).

### The model's size

Apalache reads the model's IR (`quint compile --target json`) as **one JSON string of at most
20,000,000 characters** (Jackson's default `maxStringLength`; the server gives "Invalid strict
JSON: String value length ... exceeds the maximum allowed"). v1.8's IR is 19,827,223 bytes;
v1.9's 19,868,662. A first v1.9 draft that carried the per-step I1 flag in a new field of the
request's result record, then one that read and rebuilt the state in `undoRedo`, grew it to
about 20.03 MB and every compile failed: every expression whose type holds the whole state
carries that type in the IR. The per-step flag therefore lives in `commit` (one record
update, inside the state). `smoke.sh` prints the size. **Since 2026-11-04** (the audit's additions above):
21,334,452 bytes, every Quint job running its servers with `tools/quint-jvm`'s agent, which is required (a job without it stops; "The audit's additions").

### Jobs (an x86 compute node, `--cpus 4 --mem 8G`, `APA_HEAP=4g TLC_HEAP=2g NODE_HEAP_MB=2048`)

| Job | What | Result |
|---|---|---|
| `5597f6ea` (indicative (bundle)) | enumerator only, v0.9.0 (baseline): every mutant without `--first`, and the oracle at every bound | the v1.8 violation sets and counts |
| `03867f52`, `cbf8cddd` (both indicative (bundle)) | `counts.sh` `PART=m`, first drafts | IR over 20 MB: every compile failed (above) |
| `20db2b75` (indicative (bundle)) | IR sizes (v1.8, v1.9 with and without the per-step flag), then `counts.sh` `PART=m` | every mutant, one TLC worker: the depths above; (xi) 4 and 4 |
| `0b8fedfe` (indicative (bundle)) | `counts.sh` `PART=a` | four bounds equal, as v1.8; ITF 2,000 traces, 0 mismatches |
| `e6427b49` (indicative (bundle)) | `counts.sh` `PART=b` | three bounds equal, as v1.8 |
| `3951cf03` (indicative (bundle)) | enumerator only, this commit: as `5597f6ea`, plus (xi) | the counts and violation sets of `5597f6ea`, exactly; (xi) as above |
| `e4c17c6c` (indicative (bundle)) | `smoke.sh` | typechecks; IR 19,853,379 bytes (`u_tiny`); ITF 0 mismatches (with and without a restart); TLC = enumerator at 2 × 1 + 1 undo (3,889); every mutant fails in both, (xi) at 4 |

## Results v1.8 (provenance roots)

### What changed in the abstraction

- Each character carries a root label; each region the labels of its `before` and `after`
  characters (a change region: what the change deleted and inserted; a redo region: the
  window the undo saw and what it left). A record edge pair is bounds and taints only.
- Theirs is read off the live window by root at merge time (U8, the greedy rule); O4 is
  judged on (range, roots); O4a refuses a kept ours hunk that would put back a live root
  (conflict, overlap); U10a (`own_only`) is checked by root; `I1` is a new stop rule.
- The author-based `own_only` of v1.7 is withdrawn: an undo may now remove another author's
  restoration of its own root (regression (b)).
- New case class `concurrent_restore`: O4 took a restoration once, O4a fired, or a window
  character counted as base by root only. Since v0.9.2 the enumerator (`bfs`, `walk`: its
  progress and `SUMMARY` lines) and the trace test also count each part as its own class:
  `concurrent_restore_o4`, `concurrent_restore_o4a`, `concurrent_restore_base_by_root` (a
  transition may hit more than one; the total stays). CI checks each is hit: the trace test
  (256 traces: 72 / 12 / 102, as v1.8 reported), a tiny `bfs`, and a short walk of the
  binary (`tests/walk_classes.rs`). No behaviour change.

### Counts: v1.7 → v1.8

TLC's distinct states = `undo_enum bfs`'s at every bound (an 8-core x86 node; v1.7's figures from
job `7a899e60`, indicative (bundle); source stated as `a5bc7d0`). Roots add states (a restoration differs from the element it restores only by
being one), the carried theirs leaves them (two windows read the same by root are one state):

| Bound | v1.7 (TLC = enumerator) | v1.8 enumerator | v1.8 TLC | TLC wall |
|---|---|---|---|---|
| 2 pids × 1 event + 1 undo (`abb`, 4) | 3,891 | 3,889 | 3,889 | 4 s |
| 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 34,125 | 35,792 | 35,792 | 27 s |
| 3 × 1 + 1 undo (`abb`, 4) | 218,761 | 220,210 | 220,210 | 133 s |
| 2 × 2 + 1 undo (`ab`, 3) | 500,176 | 542,418 | 542,418 | 267 s |
| 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 193,161 | 200,530 | 200,530 | 92 s |
| 1 × 1 + 1 undo + 1 revert (`ab`, 4) (new: (x)'s bound) | – | 144 | 144 | 2 s |
| 2 × 1 + 2 undos + 1 revert (`ab`, 4) (new: restorations meet) | – | 49,656 | 49,656 | 49 s |

**Current counts:** see the note of 2026-10-31 under v1.9's table (three bounds count fewer
states since the purge revision of 2026-10-29, commit `04487c5`).

Case class `concurrent_restore` (distinct states whose transition hit it): 4,661 at
2 × 1 + 2 undos + 1 revert; 10,654 at 2 × 2 + 1 undo (`--steps 2`: the second step reads the
first's restoration as its base, by root); 0 at the bounds with one undo and no second
restoration. *These are bfs counts, per first visit* (What the runs check): a transition
into a state already visited is not counted, so the 0 says no transition into a new state
hit the class there, not that no transition does.

ITF replay (whole state after every step): 300 traces at 2 × 1 + 1 undo + 1 revert and 300
with a restart (smoke `ac55d0a0`, indicative (bundle); source stated as `c906453`); 1,000 at 2 × 1 + 1 undo + 1 revert (4,000 steps) and 1,000
at 2 × 1 + 2 undos + 1 revert (5,000 steps; 1,422 undos, 578 redos, 1,000 reverts) (`00d8b0dc`, indicative (bundle)):
**0 mismatches**.

### Mutants, least depth inside an exhaustive bound

| Mutant | Rule | Bound | Enumerator (depth) | TLC, one worker (actions) |
|---|---|---|---|---|
| (i), (iii), (iv), (vi), (vii) | U9, U15, U17, O5, O5 | 2 × 1 + 1 undo (`abb`, 4) | 3 each | 3 each |
| skip-U19 | U19 | same | 1 | 1 |
| `f3-rediff` (F3) | own_only | same | 3 | 3 |
| `restart-single-taint` | RT | 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 4 | 4 |
| **(x) `x-provenance-ignored-in-o4`** (v1.8) | **I1** | 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 3 | 3 |

Job `a75a2670` (counts `PART=m`; indicative (bundle)); the smoke job `ac55d0a0` (TLC with every worker; indicative (bundle), source stated as `c906453`) agrees, and
`cargo test --features mutants --test mutants` gives the same rules and depths.

**Latest (2026-11-04):** jobs `66736440` and `03aa049f` (`PART=m`): every mutant above at the
same rule and depth in both tools; the audit's four in "The audit's additions" above.
**Before (2026-10-31):** v0.10.0's merge commit `260ef4e`, job `f292077c` (`counts.sh` `PART=m`):
every mutant above fails its own rule only, at the same depths on the enumerator and on TLC
with one worker; (xi) `xi-no-o4a` at 4 and 4, as v1.9 recorded (`20db2b75`, indicative (bundle)).

**(x)'s witness** (depth 3, 1 × 1 + 1 undo + 1 revert, seed `ab`): p1 deletes `b` at the doc's
end (`a`); p1 undoes (`ab`, the new `b` has the seed `b`'s root); p1 reverts its delete. By
element identity the revert's `b` and the undo's are different insertions at one point, at
the doc's end with a non-empty base, so O5 lands both (`abb`): two live elements with one
root, I1 fails. The oracle takes it once (O4 on roots).

### The O4a witnesses (oracle unit tests)

- **At the doc's end** (`o4a_at_the_end`, depth 4): the same, but p2 types `a` before the
  restored `b`; revert's theirs inserts `ab`, ours `b`, at the end: O5 would land both; O4a
  conflicts.
- **In mid-text** (`o4a_mid_text`, depth 5, the least the bounded world reaches): seed `ab`;
  p1 inserts `a` at 1; p2 deletes the seed's `a` and replaces `b` (two regions); p1 undoes;
  p1 reverts p2's change, whose two hunks touch and are joined into one replacement that
  starts at p2's first, empty region (R1: entirely after it). The seed's `a` comes back
  outside that region's window, so p2's undo of it would put the root back a second time:
  O4a conflicts (and `--partial` must not apply it).

### Readings (v1.8)

- **The greedy rule** with repeated base roots (only a mutant makes them): the least base
  index greater than the last counted.
- **O4:** same range on the base and the same roots in the same order; letters follow the
  roots. Pure deletions of one range are identical (no roots), as before.
- **O4a scope:** the live text of the region's root as the step merges it (the scratch text
  after earlier steps of a multi-step undo), anywhere in it, window included; only when the
  merge is otherwise clean; O5 is then not counted as landed.
- **U10a's set:** the `after` roots at the positions ours deletes, over the step's batch (a
  change region: everything it inserted; a redo region: what the undo put back). *v1.9:
  over the regions the step applied only.*
- **Redo regions:** `before` roots are the whole window the undo saw, `after` roots what it
  left; ours' inserted roots are the window characters the undo removed.
- **Bounds decide the window, roots only classify inside it:** a restored base element that
  lands outside the carried bounds (an insertion at a shared edge, pulled into the left
  region; or the mid-text O4a case) is not read as the region's base; O4a covers it.

**A consequence worth knowing** (`undo_twice_over_own_replacements_applies`): one participant
replaces `a` with `b`, then `b` with `c`, and undoes twice (or `--steps 2`). v1.7 conflicted on
the second step (the first undo's `b` is a new element, foreign by identity); v1.8 applies,
since it has the first change's root. This is regression (b) with one participant.

### Jobs (an 8-core x86 node, `--mem 12G`, commit `cbe337e`'s tools; the bundle as `tools/README.md`)

The jobs ran branch `g1/u4-oracle` (commits `c906453`, `cbe337e`), cherry-picked onto
`g1/exact-hunk-undo` as `f6191b4`, `641f6dc` with the same model, tools and oracle.

| Job | What | Result |
|---|---|---|
| `ac55d0a0` (indicative (bundle); source stated as `c906453`, kept by tag `ev-ac55d0a0`) | `smoke.sh` (commit `c906453`) | Quint typechecks; ITF 0 mismatches (with and without a restart); TLC = enumerator at 2 × 1 + 1 undo; every mutant fails in both |
| `00d8b0dc` (indicative (bundle)) | `counts.sh`, `PART=a` | four bounds equal; ITF 2,000 traces, 0 mismatches |
| `85262144` (indicative (bundle)) | `counts.sh`, `PART=b` | three bounds equal (one quint compile retried, the known flake) |
| `a75a2670` (indicative (bundle)) | `counts.sh`, `PART=m` | every mutant, one TLC worker: the depths above |

The G1-bound estimates below are v1.7's; v1.8's states are a few percent more at the bounds
above.

## Results v1.7 (exact hunk sets, restart)

### Counts: v0.8.2 → v1.7

The state now holds ours and theirs, and `own_only` no longer sits in `viol` (in v0.8.2 the
same state with and without it counted twice), so the counts move:

| Bound | v0.8.2 (TLC = enumerator) | v1.7 enumerator | v1.7 TLC | TLC wall |
|---|---|---|---|---|
| 2 pids × 1 event + 1 undo (`abb`, 4) | 3,891 | 3,891 | 3,891 | 3 s |
| 2 × 1 + 1 undo + 1 revert (`abb`, 4) | 35,635 | 34,125 | 34,125 | 19 s |
| 3 × 1 + 1 undo (`abb`, 4) | 220,660 | 218,761 | 218,761 | 83 s |
| 2 × 2 + 1 undo (`ab`, 3) | 531,261 | 500,176 | 500,176 | 200 s |
| 1 × 3 + 1 revert + 1 restart (`abba`, 4) (new) | – | 193,161 | 193,161 | 75 s |

An 8-core x86 node, job `7a899e60` (`tools/undo/counts.sh`, commit a5bc7d0; indicative (bundle), source stated as `a5bc7d0`): every count equal. ITF
replay in the same job: 1,000 traces at 2 × 1 + 1 undo + 1 revert, 4,000 steps, 0
mismatches; and in the smoke job `0c00aee0` (indicative (bundle)): 300 traces with a restart (1,500 steps), 0
mismatches.

### Mutants, least depth inside an exhaustive bound

| Mutant | Rule | Bound | Enumerator (depth) | TLC, one worker (actions) |
|---|---|---|---|---|
| (i), (iii), (iv), (vi), (vii) | U9, U15, U17, O5, O5 | 2 × 1 + 1 undo (`abb`, 4) | 3 each | 3 each |
| skip-U19 | U19 | same | 1 | 1 |
| `f3-rediff` (F3) | own_only | same | 3 | 3 |
| `restart-single-taint` | RT | 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 4 | 4 |

Job `7a899e60` (indicative (bundle); source stated as `a5bc7d0`); the smoke job `0c00aee0` (TLC with every worker; indicative (bundle)) agrees.

`restart-single-taint`'s witness: from `abba`, a two-region edit (`[1, 2)` → `a`, `[3, 4)`
deleted: `aab`); a delete of what separates the regions (`aa`: they touch); a delete of
`[0, 2)`, across the left region's start only; then the restart. After it the left region
reads as tainted at its end too, the right one inherits that taint (U22a), and the revert's
conflicts change. On walks at the literal G1 bound plus one restart
it fails at depth 8; `f3-rediff` at depth 3.

## Results v1.6 (an 8-core x86 node; jobs from `tools/undo/*.sh`)

### ITF replay: 0 mismatches

Quint simulator traces through the wrapper `tools/undo/itf.qnt.in`, replayed step by step into
`spec_undo::model` (`undo_itf_replay`):

| Bound | Traces | Steps | Mismatches |
|---|---|---|---|
| **G1**: 3 pids × 3 events + 2 undos + 1 revert (seed `abb`, `MAXLEN` 4) | 3,000 | 35,798 (up to 12 actions) | 0 |
| 2 pids × 1 event + 1 undo + 1 revert (`abb`, 4) | 1,000 | 4,000 | 0 |
| *before the fix below:* 2 pids × 1 event + 1 undo + 1 revert | 7,600 | 30,400 | 0 |
| *before the fix below:* 2 pids × 2 events + 1 undo (`ab`, 3) | 6,000 | 30,000 | 0 |

At G1 the replayed actions were 22,311 edits, 4,555 purges, 4,204 undos, 1,759 redos and
2,969 reverts. The replay saw every action kind and the model reproduces F3 (`own_only` in its `viol`, as in the oracle's).

### TLC's exact counts = the enumerator's

TLC's "distinct states found" on `undo.qnt` against `undo_enum bfs` at the same bound (no
symmetry; the same abstraction and the same state):

| Bound | TLC | `undo_enum bfs` | TLC wall |
|---|---|---|---|
| 2 pids × 1 event + 1 undo (`abb`, 4) | 3,891 | 3,891 | 4 s |
| 2 pids × 1 event + 1 undo + 1 revert (`abb`, 4) | 35,635 | 35,635 | 16 s |
| 3 pids × 1 event + 1 undo (`abb`, 4) | 220,660 | 220,660 | 68 s |
| 2 pids × 2 events + 1 undo (`ab`, 3) | 531,261 | 531,261 | 151 s |

**One mismatch found and fixed on the way** (a transcription error in the model's checks,
not in the oracle): TLC first counted 2 states more at the revert bound and 496 more at the
2 × 2 bound. The exact state-set diff (`tools/undo/diff.sh`: TLC's `-dump` against
`undo_enum bfs --dump`, compared by `statediff.py`) showed TLC-only states equal to
enumerator states but for `own_only`; TLC's shortest path to one ended in an all-or-nothing
`undo --steps 2` whose first step deleted another pid's letter on the scratch copy and whose
second conflicted. The model reported the scratch copy's deletions for the refused request;
the oracle's side counts only what is applied. Fixed in `undo.qnt`; every count then matched.

### Mutants: every one fails, in both, at the same depth

At 2 pids × 1 event + 1 undo (`abb`, 4): TLC with one worker (a shortest counterexample) and
`undo_enum bfs --first`:

| Mutant | Rule violated | TLC (actions) | Enumerator (depth) |
|---|---|---|---|
| (i) no taint rule (whole-text merge3) | U9 | 3 | 3 |
| (iii) `--partial` reports a different set | U15 | 3 | 3 |
| (iv) redo re-tries skipped regions | U17 | 3 | 3 |
| (vi) O5 at every window end | O5 | 3 | 3 |
| (vii) O5 on the region's base | O5 | 3 | 3 |
| skip-U19 (purge keeps stacks) | U19 | 1 | 1 |

The same set fails `cargo test -p k2-mpc-spec-undo --features mutants --test mutants`, and on
random walks at the literal G1 bound (20,000 walks each, `tools/undo/g1-smoke.sh`): the same
rule, at the same depth (3; skip-U19 at 1). 150,000 walks of the oracle of record at G1 (1.8M
steps) violate no rule the runs stop at (F3 observed).

## The G1 bound

The spec's G1 bound is 3 pids × 3 events + 2 undos + 1 revert. **In this action space it is
far beyond exhaustive enumeration:** Knuth's estimate of the action-sequence tree is about
10^21 nodes; with pid symmetry and per-seed deduplication, not one of the 875,724 depth-4
seeds finished in 120 s (4 threads, > 23M states), which puts the distinct states at 10^13 or
more: weeks of node time. TLC can't come close. What fits (seed `abb`, `MAXLEN` 4; estimates
from `undo_enum dfs --sample`, which explores random seeds exhaustively and extrapolates;
states counted per seed, with pid symmetry):

| Bound | States | Status |
|---|---|---|
| 3 pids × 1 event + 2 undos + 1 revert | ≈ 8.8 × 10^6 (± 11 %) | `g1.sh` (`PART=exhaustive`), minutes |
| 2 pids × 2 events + 2 undos + 1 revert | ≈ 6.5 × 10^8 (± 15 %) | `g1.sh` (`PART=exhaustive`) |
| 3 pids × 2 events + 2 undos, no revert | ≈ 2.6 × 10^10 (± 32 %, 4 seeds) | `g1.sh` (`PART=long`) |
| 3 pids × 2 events + 1 undo + 1 revert | ≈ 1.4 × 10^11 (± 7 %, seed `ab`) | not planned |
| **G1**: 3 pids × 3 events + 2 undos + 1 revert | ≥ 10^13 | sampled: Quint ITF traces (`g1-smoke.sh`) and random walks (`g1.sh`, `PART=walks`) |

**Throughput measured:** on this Mac (M-series) about 30–60k states/s per thread for `dfs`
with every check, and about 1,000 G1 walks/s per thread; on an 8-core x86 node the walks
ran about 5× slower per thread (1M walks in 655 s on 8 threads), and `dfs` is assumed to
scale the same way (not measured there).

**Runtime estimates:**

| Part | On an M-series node (6 threads) | On an x86 node (12 threads) |
|---|---|---|
| `PART=exhaustive` (≈ 6.6 × 10^8 states) | ≈ 40–60 min | ≈ 1.5–2 h |
| `PART=walks` (5 × 10^6 walks + mutants) | ≈ 15 min | ≈ 1 h |
| `PART=long` (≈ 2.6 × 10^10 states) | ≈ 1–2 days | ≈ 3–5 days |

**Commands** (repo root, with the `k2` CLI's variables intact: unset `K2_*` only around
cargo; launched only after review). An M-series node (the grant there allows long jobs):

```sh
k2 compute run "$NODE" --repo k2-mpc-base --needs rust@stable --cpus 6 --mem 4G \
  --env CARGO_BUILD_JOBS=6 --env THREADS=6 --env PART=exhaustive --timeout 3h --tier batch \
  --detach -- sh tools/undo/job.sh enum tools/undo/g1.sh
```

(`--repo` ships the commit; run from a worktree, `--repo` names it, or is found for you
when you run from inside it. The env-chunk fallback is `tools/undo-enum-job.sh`, `tools/README.md`.)

Then the same with `--env PART=walks` (`--timeout 1h`), and, if wanted, `--env PART=long`
(`--timeout 72h`). On a 12-core x86 node: `--cpus 12 --env THREADS=12`, and twice the
timeouts. Memory is small: per-seed fingerprint sets hold at most a few million states.
Every finished seed writes a checkpoint line (also logged as `ckpt seed ...`), so a stopped
run resumes (`g1.sh` says how).
