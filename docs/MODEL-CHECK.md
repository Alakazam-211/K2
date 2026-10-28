# The undo model check (`MC text-undo`)

The undo spec's model check (`docs/SPEC-UNDO.md` §8 "Model check"): a Quint model of the spec,
replayed against the oracle of record `spec_undo::model` through ITF traces, and an exhaustive
enumeration of the oracle itself, cross-validated against TLC's exact state counts, with
mutants that must fail in both.

| Piece | Where |
|---|---|
| The Quint model | `spec/quint/undo.qnt` (SPEC-UNDO v1.9, over abstract text) |
| Evidence for G1 | `docs/evidence/g1-undo.md` |
| The pinned diff | `spec/quint/undo_diff.qnt`, equal to `diff::hunks` on every pair of texts up to 4 long (`crates/spec-undo/tests/quint_diff.rs`) |
| The bounded world (the same abstraction, in Rust, over the oracle) | `crates/spec-undo/src/bounded.rs` |
| ITF replay | `crates/spec-undo/src/itf.rs`, `src/bin/undo_itf_replay.rs` |
| The enumerator | `crates/spec-undo/src/bin/undo_enum.rs` |
| Mutants | `crates/spec-undo/src/model/mutant.rs` (feature `mutants`), `MUTANT` in `undo.qnt` |
| Node jobs | `tools/undo-quint-job.sh`, `tools/undo-enum-job.sh`, `tools/undo/*.sh` (`tools/README.md`) |

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
  deletes every live copy of a letter as the purger's batch, then clears the stacks (U19).
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
  purge redaction of records (R9's 8-byte windows never match a one-letter secret).

### Reductions (each machine-checked)

| Reduction | Why it is exact | Check |
|---|---|---|
| **The cut**: no state with a text longer than `MAXLEN` | a bound, like the budgets: an action that would make a longer text is not taken, in both implementations | `bounded::tests::the_cut_bounds_every_diffed_string` |
| **Records dropped** once the revert budget is spent (and `head`) | nothing reads a record again | `bounded::tests::dropping_spent_records_is_exact`: with and without it, the same states (minus records) and the same violations |
| **Fields no rule reads** are not state: a record's original hunks, a batch's `after_partial` and `on_behalf_of`, the authors sets | no rule or outcome reads them | the ITF replay compares every state field the model has |
| **The view** (v1.8): roots as labels, the carried theirs and a reduced head left out | the rules compare roots only for equality; theirs is read by root from the live window; nothing reads a head once the records are dropped | `bounded::tests::the_view_is_exact`: oracle states found with one view (4,000 pairs at 2 × 1 + 2 undos + 1 revert, 1,780 of them under other root names) have the same actions, outcomes and next views |
| **Pid symmetry** (enumerator, `--sym`; not in TLC) | pids have the same budgets and rights | `bounded::tests::pid_symmetry_is_exact`: the canonical forms of the plain enumeration's states are exactly the symmetric enumeration's states, same violations |

### The diff (D1–D2)

The merge diffs with `diff::hunks`. Quint can't call it, so `undo_diff.qnt` **pins it**. On
texts over `{a, b}` (no whitespace, no newline) `diff::hunks` takes one path: trim the common
prefix and suffix; if what is left of either side is empty, one hunk; if one side is a
subsequence of the other, the character-level Myers diff is one-sided; otherwise the two
middles are one replacement hunk. The only choice left is which characters a one-sided diff
keeps, and on every pair of texts up to 4 long Myers keeps the **right-greedy embedding**
(the latest matching characters). `undo_diff.qnt` is that algorithm.

The proof is exhaustive: `crates/spec-undo/tests/quint_diff.rs` transcribes the module and
checks it equals `diff::hunks` on **every** pair of texts over `{a, b}` up to 4 long (961
pairs), and that it differs at 5 (4 is a hard limit, `DIFF_MAX`). The cut keeps every text the
merge diffs (every region's before, after and window) within `MAXLEN`
(`the_cut_bounds_every_diffed_string`), and every instance compared with Quint has
`MAXLEN ≤ 4`, so on every state the bound reaches the model's diff **is** `diff::hunks`.
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
| `U9` | a tainted region is never applied |
| `U13` | a refused all-or-nothing request, or a blocked one, changes nothing |
| `U15` | all or nothing's conflicts = `--partial`'s reports, first conflicting step |
| `U17` | a step pushes one region per region it applied: redo reapplies exactly the applied regions |
| `U19` | a purge leaves every stack empty |
| `U22a` | revert's conflicts ⊇ undo's on the same state, same bounds, same text where both apply |
| `O5` | O5 lands exactly at the doc's end with a non-empty whole-doc base: never elsewhere, never refused there (U10's `at_end`, both ways) |
| `U12` (Rust only, `--u12`) | a diagnostic: outside taint, the window merge of the exact sets equals the whole-text merge of the same sets (v1.7). Clean; the old re-diff form (F4) is informative only |

## Readings

The oracle's readings (R1–R10, U22a; `crates/spec-undo/src/model/mod.rs`) carry over. Added
here: a request spends the undo budget whatever it finds; a purge's hunks are one per copy,
not joined (as `purge_text` builds them).

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
update, inside the state). `smoke.sh` prints the size.

### Jobs (an x86 compute node, `--cpus 4 --mem 8G`, `APA_HEAP=4g TLC_HEAP=2g NODE_HEAP_MB=2048`)

| Job | What | Result |
|---|---|---|
| `5597f6ea` | enumerator only, v0.9.0 (baseline): every mutant without `--first`, and the oracle at every bound | the v1.8 violation sets and counts |
| `03867f52`, `cbf8cddd` | `counts.sh` `PART=m`, first drafts | IR over 20 MB: every compile failed (above) |
| `20db2b75` | IR sizes (v1.8, v1.9 with and without the per-step flag), then `counts.sh` `PART=m` | every mutant, one TLC worker: the depths above; (xi) 4 and 4 |
| `0b8fedfe` | `counts.sh` `PART=a` | four bounds equal, as v1.8; ITF 2,000 traces, 0 mismatches |
| `e6427b49` | `counts.sh` `PART=b` | three bounds equal, as v1.8 |
| `3951cf03` | enumerator only, this commit: as `5597f6ea`, plus (xi) | the counts and violation sets of `5597f6ea`, exactly; (xi) as above |
| `e4c17c6c` | `smoke.sh` | typechecks; IR 19,853,379 bytes (`u_tiny`); ITF 0 mismatches (with and without a restart); TLC = enumerator at 2 × 1 + 1 undo (3,889); every mutant fails in both, (xi) at 4 |

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
  character counted as base by root only.

### Counts: v1.7 → v1.8

TLC's distinct states = `undo_enum bfs`'s at every bound (k2-arch; v1.7's figures from
job `7a899e60`). Roots add states (a restoration differs from the element it restores only by
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

Case class `concurrent_restore` (distinct states whose transition hit it): 4,661 at
2 × 1 + 2 undos + 1 revert; 10,654 at 2 × 2 + 1 undo (`--steps 2`: the second step reads the
first's restoration as its base, by root); 0 at the bounds with one undo and no second
restoration.

ITF replay (whole state after every step): 300 traces at 2 × 1 + 1 undo + 1 revert and 300
with a restart (smoke `ac55d0a0`); 1,000 at 2 × 1 + 1 undo + 1 revert (4,000 steps) and 1,000
at 2 × 1 + 2 undos + 1 revert (5,000 steps; 1,422 undos, 578 redos, 1,000 reverts) (`00d8b0dc`):
**0 mismatches**.

### Mutants, least depth inside an exhaustive bound

| Mutant | Rule | Bound | Enumerator (depth) | TLC, one worker (actions) |
|---|---|---|---|---|
| (i), (iii), (iv), (vi), (vii) | U9, U15, U17, O5, O5 | 2 × 1 + 1 undo (`abb`, 4) | 3 each | 3 each |
| skip-U19 | U19 | same | 1 | 1 |
| `f3-rediff` (F3) | own_only | same | 3 | 3 |
| `restart-single-taint` | RT | 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 4 | 4 |
| **(x) `x-provenance-ignored-in-o4`** (v1.8) | **I1** | 1 × 1 + 1 undo + 1 revert (`ab`, 4) | 3 | 3 |

Job `a75a2670` (counts `PART=m`); the smoke job `ac55d0a0` (TLC with every worker) agrees, and
`cargo test --features mutants --test mutants` gives the same rules and depths.

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

### Jobs (k2-arch, `--mem 12G`, commit `cbe337e`'s tools; the bundle as `tools/README.md`)

The jobs ran branch `g1/u4-oracle` (commits `c906453`, `cbe337e`), cherry-picked onto
`g1/exact-hunk-undo` as `f6191b4`, `641f6dc` with the same model, tools and oracle.

| Job | What | Result |
|---|---|---|
| `ac55d0a0` | `smoke.sh` (commit `c906453`) | Quint typechecks; ITF 0 mismatches (with and without a restart); TLC = enumerator at 2 × 1 + 1 undo; every mutant fails in both |
| `00d8b0dc` | `counts.sh`, `PART=a` | four bounds equal; ITF 2,000 traces, 0 mismatches |
| `85262144` | `counts.sh`, `PART=b` | three bounds equal (one quint compile retried, the known flake) |
| `a75a2670` | `counts.sh`, `PART=m` | every mutant, one TLC worker: the depths above |

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

k2-arch, job `7a899e60` (`tools/undo/counts.sh`, commit a5bc7d0): every count equal. ITF
replay in the same job: 1,000 traces at 2 × 1 + 1 undo + 1 revert, 4,000 steps, 0
mismatches; and in the smoke job `0c00aee0`: 300 traces with a restart (1,500 steps), 0
mismatches.

### Mutants, least depth inside an exhaustive bound

| Mutant | Rule | Bound | Enumerator (depth) | TLC, one worker (actions) |
|---|---|---|---|---|
| (i), (iii), (iv), (vi), (vii) | U9, U15, U17, O5, O5 | 2 × 1 + 1 undo (`abb`, 4) | 3 each | 3 each |
| skip-U19 | U19 | same | 1 | 1 |
| `f3-rediff` (F3) | own_only | same | 3 | 3 |
| `restart-single-taint` | RT | 1 × 3 + 1 revert + 1 restart (`abba`, 4) | 4 | 4 |

Job `7a899e60`; the smoke job `0c00aee0` (TLC with every worker) agrees.

`restart-single-taint`'s witness: from `abba`, a two-region edit (`[1, 2)` → `a`, `[3, 4)`
deleted: `aab`); a delete of what separates the regions (`aa`: they touch); a delete of
`[0, 2)`, across the left region's start only; then the restart. After it the left region
reads as tainted at its end too, the right one inherits that taint (U22a), and the revert's
conflicts change. On walks at the literal G1 bound plus one restart
it fails at depth 8; `f3-rediff` at depth 3.

## Results v1.6 (k2-arch, 8 cores; jobs from `tools/undo/*.sh`)

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
with every check, and about 1,000 G1 walks/s per thread; on k2-arch (x86, 8 cores) the walks
ran about 5× slower per thread (1M walks in 655 s on 8 threads), and `dfs` is assumed to
scale the same way (not measured there).

**Runtime estimates:**

| Part | On an M-series node (6 threads) | On k2-arch-class x86 (12 threads) |
|---|---|---|
| `PART=exhaustive` (≈ 6.6 × 10^8 states) | ≈ 40–60 min | ≈ 1.5–2 h |
| `PART=walks` (5 × 10^6 walks + mutants) | ≈ 15 min | ≈ 1 h |
| `PART=long` (≈ 2.6 × 10^10 states) | ≈ 1–2 days | ≈ 3–5 days |

**Commands** (repo root, `K2_*` unset; launched only after review). An M-series node (the
grant there allows long jobs):

```sh
unset $(env | grep -oE '^K2(SO)?_[A-Za-z0-9_]*')
tools/undo-enum-job.sh tools/undo/g1.sh /tmp/undo-g1
k2 compute run k2n1-m4 --no-source --needs rust@stable --cpus 6 --mem 4G \
  --env CARGO_BUILD_JOBS=6 --env THREADS=6 --env PART=exhaustive --timeout 3h --tier batch \
  --detach $(cat /tmp/undo-g1/env-args) -- sh -ec "$(cat /tmp/undo-g1/job.sh)"
```

Then the same with `--env PART=walks` (`--timeout 1h`), and, if wanted, `--env PART=long`
(`--timeout 72h`). On a 12-core x86 node: `--cpus 12 --env THREADS=12`, and twice the
timeouts. Memory is small: per-seed fingerprint sets hold at most a few million states.
Every finished seed writes a checkpoint line (also logged as `ckpt seed ...`), so a stopped
run resumes (`g1.sh` says how).
