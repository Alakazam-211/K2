# events_500 between v0.10.2 and v0.11.0: layout variance, not attributable by wall time

**Status: parked, with an open item.** The `events_500` read (and `blame_doc`) measured 4–7% slower in v0.11.0 than in v0.10.2 on interleaved pairs. A first-parent bisect, a merge-content check, a `timed!`-compiled-out pair and a static comparison of five builds did not find a code change that explains it. **Conclusion: layout variance plus possibly `timed!`, not attributable by wall time; instruction counts pending approval of an instruction-counting tool on the nodes.**

Every ratio cites the run record that printed it (`[run JOB/RUN:field]`, checked by `tools/evidence/check.py`); B/A ratios are ×1000, the ratio of the medians of per-run p50; times are in ns. Node loads are the job logs' own lines (`host at start/end`, and the harness overlay's `perf: load before <step>` line), not run records. Measurements only: the harness (`tools/perf/`, `tools/README.md`, "Performance harness") at main `ab16c7e4`'s `tools/perf`, both sides interleaved A, B, A, B on one node, one fixture grown by side A and read by both, 2,000 samples a row or fewer at a cap (so every p99 is indicative). Node classes: `x86_64-linux-quiet` (no other job running or queued and a 1-minute load under 1.0 at submit) and `x86_64-linux-shared` (the least-loaded allowed node).

## 1. Where it was seen

| Job | A commit | B commit | Node at submit | Size | events_500 B/A p50 | events_500 B/A p95 | blame_doc B/A p50 |
|---|---|---|---|---|---|---|---|
| `dffd1402` | `39954167` | `2aa7a8a0` | shared, 5 runs | 10k | 1,082 [run dffd1402/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,055 [run dffd1402/perfh_ratio_h10000_events_500:ratio_p95_milli] | 1,046 [run dffd1402/perfh_ratio_h10000_blame_doc:ratio_p50_milli] |
| `dffd1402` | `39954167` | `2aa7a8a0` | shared, 5 runs | 100k | 1,073 [run dffd1402/perfh_ratio_h100000_events_500:ratio_p50_milli] | 952 [run dffd1402/perfh_ratio_h100000_events_500:ratio_p95_milli] | 1,035 [run dffd1402/perfh_ratio_h100000_blame_doc:ratio_p50_milli] |
| `ea5329f6` | `77bda2d2` (v0.10.2) | `49e94018` (v0.11.0) | quiet (load 0.02 at start, 1.00 at end, no other job), 9 runs | 10k | 1,060 [run ea5329f6/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,059 [run ea5329f6/perfh_ratio_h10000_events_500:ratio_p95_milli] | 1,040 [run ea5329f6/perfh_ratio_h10000_blame_doc:ratio_p50_milli] |
| `ea5329f6` | `77bda2d2` | `49e94018` | quiet, 9 runs | 100k | 1,055 [run ea5329f6/perfh_ratio_h100000_events_500:ratio_p50_milli] | 1,036 [run ea5329f6/perfh_ratio_h100000_events_500:ratio_p95_milli] | 1,031 [run ea5329f6/perfh_ratio_h100000_blame_doc:ratio_p50_milli] |

`ea5329f6`'s events_500 medians at 10k: A 1,016,104 [run ea5329f6/perfh_med_h10000_events_500_A:p50] ns, B 1,077,375 [run ea5329f6/perfh_med_h10000_events_500_B:p50] ns. Per-run p50 ranges were disjoint (A 990.3–1,022.8 µs, B 1,065.5–1,086.2 µs), with run spreads of 3.2% (A) and 1.9% (B) [derived]. The pair commit for it put main's `tools/perf` at the root: `77bda2d2`, on the v0.10.2 release line, has no harness of its own.

## 2. The first-parent bisect

**The rule, fixed before the first step:** events_500 at 10k, A fixed at `77bda2d2`, B = the step's commit, 5 interleaved runs a side (9 on a shared node from 2026-11-04T15:07Z). SLOW = r ≥ 1,030 and the per-run ranges disjoint; FAST = r < 1,030 and the ranges overlap; anything else is AMBIGUOUS: rerun once and call the rerun on r alone. The range: the 43 first-parent commits that touch `crates/base` from `77bda2d2` to `49e94018`, numbered #1 to #43 (#43's kit is `49e94018`'s).

| Step | B commit | Job | Node at submit (loads at the runs' starts) | events_500 B/A | blame_doc B/A | Call |
|---|---|---|---|---|---|---|
| #21 | `22f8f3fa` | `c1ff4410` | quiet (1.0–5.3) | 1,058 [run c1ff4410/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,040 [run c1ff4410/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | AMBIGUOUS (one A run at load 3.7) |
| #21 rerun | `22f8f3fa` | `7fc5dee8` | shared (2.1–3.8) | 1,073 [run 7fc5dee8/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,036 [run 7fc5dee8/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | SLOW |
| #11 | `a7224c7f` | `7744cc46` | quiet (1.1–4.9) | 1,002 [run 7744cc46/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,011 [run 7744cc46/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | FAST |
| #32 | `cc2b2670` | `fdcfe917` | quiet (1.1–5.5) | 1,042 [run fdcfe917/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,038 [run fdcfe917/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | AMBIGUOUS; moot after #21 |
| #12 | `aa1319f7` | none | — | — | — | FAST by identity with #11: `git diff --quiet a7224c7f aa1319f7 -- crates/base/src crates/base/Cargo.toml Cargo.lock vendor` passes |
| #16 | `8ab9bbf9` | `a0df1eae` | quiet (1.1–3.9) | 1,077 [run a0df1eae/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,053 [run a0df1eae/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | AMBIGUOUS |
| #16 rerun | `8ab9bbf9` | `ca1defcd` | shared (1.0–2.5) | 1,064 [run ca1defcd/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,043 [run ca1defcd/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | SLOW |
| #13 | `db08bf5d` | `e04fe5c0` | shared (1.1–3.5) | 1,096 [run e04fe5c0/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,069 [run e04fe5c0/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | AMBIGUOUS |
| #13 rerun | `db08bf5d` | `522db6d5` | quiet (1.0–3.1) | 1,060 [run 522db6d5/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,056 [run 522db6d5/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | SLOW |

So the first-parent culprit was #13, `db08bf5d`, "Merge main ef511130 into feat/sl-s4". Its main side, from `77bda2d2` (the parent of its first commit) to `ef511130`, has seven first-parent commits m#1–m#7. Only three change the compiled kit (`crates/base/src`, `crates/base/Cargo.toml`, `Cargo.lock`, `vendor/yrs`): m#2 `de956b17` (grid key counters), m#4 `3263a3e0` (Merge kit main `5d037528`, which brings the production timers and the native engine's updates), m#6 `6d49128c` (Merge feat/sl-s3, Board slice 3). The others are identical to their predecessors.

| Step | B commit | Job | Node at submit (loads at the runs' starts) | events_500 B/A | blame_doc B/A | Call |
|---|---|---|---|---|---|---|
| m#4 | `3263a3e0` | `98e42f2c` | shared (2.3–5.4) | 1,053 [run 98e42f2c/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,064 [run 98e42f2c/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | AMBIGUOUS |
| m#4 rerun | `3263a3e0` | `fa77477f` | shared (2.1–5.6) | 1,055 [run fa77477f/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,054 [run fa77477f/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | SLOW |
| m#2 | `de956b17` | `e652ee74` | shared (1.5–6.9) | 1,032 [run e652ee74/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,065 [run e652ee74/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | AMBIGUOUS; not rerun (m#6 below) |
| m#6 | `6d49128c` | `10c010fd` | shared (2.1–6.3) | 996 [run 10c010fd/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,016 [run 10c010fd/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | FAST |
| m#6, 9 runs | `6d49128c` | `32ec6752` | shared (2.4 rising to 20) | 1,032 [run 32ec6752/perfh_ratio_h10000_events_500:ratio_p50_milli] | 1,011 [run 32ec6752/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | **INVALID FOR NOISE**: the node's load rose mid-job, runs 7–9 doubled on both sides; per-run B/A 0.945, 0.939, 1.068, 0.830, 0.978, 0.958, 1.817, 0.990, 0.963 |
| m#4 vs m#6, one job | A `3263a3e0`, B `6d49128c` | `08b685cb` | shared (3.2–7.3), 9 runs | 959 [run 08b685cb/perfh_ratio_h10000_events_500:ratio_p50_milli] | 983 [run 08b685cb/perfh_ratio_h10000_blame_doc:ratio_p50_milli] | m#6 about 4% faster than m#4; per run B was 14–21% slower in runs 2–4 and 1–6% faster in the other six |

m#6 contains m#4 and adds only Board slice 3, which is on neither path, yet it read as fast as v0.10.2 (`10c010fd`) and faster than m#4 in one job (`08b685cb`), while m#4 (twice), #13 and v0.11.0 read 5–6% slow. That is the pattern of build-to-build layout variance, not of a code change on the path.

## 3. The merge's content (`db08bf5d`, local git only)

- The automatic merge of its parents (`aa1319f7`, `ef511130`) in a scratch worktree: every non-conflicted file under `crates/base/src`, `crates/base/Cargo.toml`, `Cargo.lock` and `vendor/yrs` equals `db08bf5d`'s.
- Seven kit files conflicted. `git show --cc db08bf5d` shows one hunk differing from both parents: the module list (`crates/base/src/session/live.rs`, the `mod elem_claims;` and `mod elem_presence;` lines). Every other hunk took one side's text; `store/mod.rs` as main's drops only doc-comment lines.
- On the `events_since` path, the function bodies are the same in the merge base, both parents and the merge (`Session::window`, `DocStore::window_rows`, `DocStore::client_owned_by`, `Session::app_viewer`, `Session::admit` and its callees), or the same in main's parent and the merge (the roster's `flush`). The only edits on these paths since v0.10.2 are main's timer scopes: `timed!(EventsSince)` and `timed!(EventsSinceWindow)` in `session/live/at_base.rs`, `timed!(Blame)` in `session/live/blame.rs`, `timed!(StoreTxnCommit)` in `store/history.rs`.
- What the merge adds over main (`git diff --stat ef511130 db08bf5d -- crates/base/src`: 15 files) is slice 4, claims on elements; none of it is on these paths. The new `SessionError::ElemsClaimed` is boxed, so `SessionError` does not grow.

## 4. `timed!` compiled out

`timed!(X)` expands to `let _timed = Scope::new(Path::X);`. Off (the default), a scope is one relaxed load of `ENABLED` and a branch at creation and one compare and a branch at drop (`crates/base/src/timers.rs`: `Scope::new_in` and `Drop for Scope`, both `#[inline(always)]`; the `HISTS` index is a constant after inlining; no clock read; a 24-byte guard). `events_since` has two scopes, `blame` one; none is inside a per-row loop.

Pair `d4176489`: A `49e94018`, B = `49e94018` with `timed!` expanding to nothing (a scratch commit, never merged; B's build warns that `Scope::new` is never used). Shared node, loads 15–24 at the runs' starts (over-subscribed), 9 runs a side; absolute times about twice a quiet node's.

| Size | events_500 B/A p50 | events_500 B/A p95 | blame_doc B/A p50 |
|---|---|---|---|
| 10k | 963 [run d4176489/perfh_ratio_h10000_events_500:ratio_p50_milli] | 974 [run d4176489/perfh_ratio_h10000_events_500:ratio_p95_milli] | 985 [run d4176489/perfh_ratio_h10000_blame_doc:ratio_p50_milli] |
| 100k | 992 [run d4176489/perfh_ratio_h100000_events_500:ratio_p50_milli] | 997 [run d4176489/perfh_ratio_h100000_events_500:ratio_p95_milli] | 1,002 [run d4176489/perfh_ratio_h100000_blame_doc:ratio_p50_milli] |

At 100k, the six runs at steady load (20–21 on both sides) read 0.988–0.998 for events_500. Compiling `timed!` out recovers perhaps 1–4%, less at 100k, on a loaded node: not the 5–6%.

## 5. Static comparison of five builds

One job (`0a2c3824`; a first pass, `9f204532`, gave the same sizes) built the harness's plain release binary from each tree, in its own target directory, with one toolchain: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, LLVM 23.1.1. Every measuring job above also used rustc 1.99.1, so no toolchain drift. Features as the pairs build them: none for A, `kit-timers` (harness-side only) for the others. `Cargo.lock` is the same from `77bda2d2` to `db08bf5d`.

Function sizes:

| Function | A `77bda2d2` | m#4 `3263a3e0` | m#6 `6d49128c` | #13 `db08bf5d` | v0.11.0 `49e94018` |
|---|---|---|---|---|---|
| `Session::events_since` | 1,393 B | 1,709 B | 1,709 B | 1,709 B | 1,709 B |
| `Session::window` | 5,444 B | 5,444 B | 5,444 B | 5,444 B | 5,444 B |
| `DocStore::window_rows` | 7,500 B | 7,500 B | 8,371 B | 8,411 B | 8,411 B |
| `DocStore::client_owned_by` | 2,133 B | 2,133 B | 2,431 B | 2,431 B | 2,431 B |
| `Session::blame` | 6,806 B | 6,923 B | 6,924 B | 6,927 B | 6,900 B |

- `Session::window` is a function of its own in all five, never inlined into `events_since`, and its instructions are the same in all five but one immediate.
- `events_since` gains its scopes at m#4 (with a new register allocation, 282 to 350 instructions) and is then the same code: identical from m#6 to #13, one line apart elsewhere.
- `window_rows` and `client_owned_by` are recompiled differently at m#6 (and `window_rows` again at #13) from source that is byte-identical in every tree and an unchanged `Cargo.lock`: the compiler's inlining of the store's dependencies changes as the crate around them grows.
- The timings don't follow the code: m#6 and #13 run identical `events_since` and `window` code, yet m#6 read fast and #13 slow; m#4 has A's `window_rows` exactly, yet read slow.
- Addresses (and their alignment to 64 bytes) differ in every build, but they belong to that job's binaries, not to the measured ones: each job builds at its own path, which feeds the crates' metadata hashes and so symbol and section order. The job's loop-head parser read no loop heads (its pattern missed the disassembler's `0x`-prefixed targets), so hot-loop alignment is not reported here.

## 6. Conclusion and open item

**Layout variance plus possibly `timed!`, not attributable by wall time; instruction counts pending approval of an instruction-counting tool on the nodes.** The nodes' hardware counters are closed to jobs (`perf_event_paranoid` 4, no capabilities), and valgrind is not on the nodes' approved package list; both are the machine owner's to change.

Prepared for v0.11.1, measured only once counts are possible: `timers::finish` (the on path) marked `#[cold]` (`6d9ba0fa`, branch `perf/timers-finish-cold`), so the compiler knows the timed branch is the unlikely one; the off path is otherwise unchanged.

**Open item, when valgrind is allowed:** run the harness's events_500 row alone (an overlay-only switch, `PERF_ONLY=events_500`: the same warm-up and `fast_round`, other rows skipped) under `valgrind --tool=callgrind --toggle-collect='*events_since*'` on six builds, A `77bda2d2`, m#4 `3263a3e0`, m#6 `6d49128c`, #13 `db08bf5d`, v0.11.0 `49e94018` and v0.11.0 with `#[cold]` `6d9ba0fa`, at 10k on one fixture, with `--cache-sim=yes`: instructions, i-cache and branch-miss counts per `events_since` call. Equal instructions with different times confirm layout; more instructions in the slow builds point to real code, and `callgrind_annotate` names the functions.
