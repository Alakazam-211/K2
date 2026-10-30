# The undo model check (`MC text-undo`)

The undo spec's model check (`docs/SPEC-UNDO.md` §8 "Model check"): a Quint model of the spec,
replayed against the oracle of record `spec_undo::model` through ITF traces, and an exhaustive
enumeration of the oracle itself, cross-validated against TLC's exact state counts, with
mutants that must fail in both.

| Piece | Where |
|---|---|
| The Quint model | `spec/quint/undo.qnt` (SPEC-UNDO v1.6, over abstract text) |
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
  the whole state is equal (text with each character's author, every stack, every revert record
  with its per-edge carry, budgets, violations). A difference is reported, never adjusted.
- **TLC's "distinct states found"** equals `undo_enum bfs`'s distinct count at the same bound
  (without `--sym`): both count distinct values of the same state.

### The bounded world

- **Text:** one root over `{a, b}` (no `\n`); each character carries its **author** (0: the
  seed text, an import; pid `k`: author `k`).
- **Pids** `p1..pN`, acting for nobody, every right.
- **Events** per pid (`EVENTS`): an edit or a purge. Edits are explicit canonical hunks: insert
  a letter, delete one, replace one with the other letter, delete two, and (`MULTI`) two
  single-letter deletes or replacements at least one letter apart (two regions). A purge
  deletes every live copy of a letter as the purger's batch, then clears the stacks (U19).
- **Undos** (`UNDOS`, shared): `undo` all or nothing or `--partial`, `--steps` 1 or 2;
  `undo --drop`; `redo` all or nothing or `--partial`. A request spends one whatever it finds.
- **Reverts** (`REVERTS`, shared): any recorded change, by any pid, all or nothing or `--partial`.
- **Not here** (the trace test covers them): restart, the revert window, `write --base`,
  `replace_text`, `restore`, `undo --for`, on-behalf edits, the U6 bounds (never reached),
  purge redaction of records (R9's 8-byte windows never match a one-letter secret).

### Reductions (each machine-checked)

| Reduction | Why it is exact | Check |
|---|---|---|
| **The cut**: no state with a text longer than `MAXLEN` | a bound, like the budgets: an action that would make a longer text is not taken, in both implementations | `bounded::tests::the_cut_bounds_every_diffed_string` |
| **Records dropped** once the revert budget is spent (and `head`) | nothing reads a record again | `bounded::tests::dropping_spent_records_is_exact`: with and without it, the same states (minus records) and the same violations |
| **Fields no rule reads** are not state: a record's original hunks, a batch's `after_partial` and `on_behalf_of`, the authors sets | no rule or outcome reads them | the ITF replay compares every state field the model has |
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
| `own_only` | `hist.undo_own_only` (U10 element identity, U18a): every character an undo or redo by `p` deletes is `p`'s; every one a revert of change `c` deletes is `c`'s author's. **Observed, not stopped at: finding F3** |
| `U9` | a tainted region is never applied |
| `U13` | a refused all-or-nothing request, or a blocked one, changes nothing |
| `U15` | all or nothing's conflicts = `--partial`'s reports, first conflicting step |
| `U17` | a step pushes one region per region it applied: redo reapplies exactly the applied regions |
| `U19` | a purge leaves every stack empty |
| `U22a` | revert's conflicts ⊇ undo's on the same state, same bounds, same text where both apply |
| `O5` | O5 lands exactly at the doc's end with a non-empty whole-doc base: never elsewhere, never refused there (U10's `at_end`, both ways) |
| `U12` (Rust only, `--u12`) | a diagnostic: outside taint, the window merge equals whole-text merge3. **Finding F4** |

## Readings

The oracle's readings (R1–R10, U22a; `crates/spec-undo/src/model/mod.rs`) carry over. Added
here: a request spends the undo budget whatever it finds; a purge's hunks are one per copy,
not joined (as `purge_text` builds them).

## Findings

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

## Results (k2-arch, 8 cores; jobs from `tools/undo/*.sh`)

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
