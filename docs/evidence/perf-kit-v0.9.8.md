# v0.9.8 investigation: what the kit's edit path costs as one doc's history grows

Every figure cites the run that printed it (`[run JOB/RUN:field]`: the run records in
`docs/evidence/runs/`, checked by `tools/evidence/check.py`). Measurements only; **no fix is
made here**. The bench is `crates/base/tests/perf_kit.rs` (ignored; release, on a node).

## What was measured

- **The doc:** one Markdown doc opened with a file, as a host opens it. 4 people and 4 agents
  grow it through the public session API with a seeded mix:
  - people's plain `edit`s (a line replaced, added or deleted);
  - agents' `run_tool` verbs (`replace --lines`, `insert --line`, `delete --lines`, `sed -i`)
    and `edit_with`;
  - undo and redo, `cat` reads;
  - the host's `tick` every 25 calls and `flush` every 250.
- **Doc size:** the text stays about 200 lines (about 1.8 kB), so only the history grows:
  1,807 [run 6491b3f9/perfkit_10000_sizes:text_bytes] bytes at 10k versions,
  1,810 [run 6491b3f9/perfkit_100000_sizes:text_bytes] at 100k.
- **What is timed:** nothing while the doc grows, except the kit's own profile scopes per
  window of 5,000 versions. At 10k, 50k and 100k versions the bench takes these measurements:
  - **Cold:** a reopen (a fresh store handle, manager and session), then the first calls.
  - **Warm samples:** 500 rounds, each call timed alone: `edit`, `edit_with`,
    `run_tool replace`, `run_tool sed -i`, `undo`, `cat`, and a store probe. The probe is
    one durable `host_put` of 5 bytes: the store's fixed cost per commit at that file size.
  - **Every 10 rounds:** a `flush`, then a `tick` one minute of host time later.
  - **At 100k:** 500 more edits with the store's sync to disk off (test-only), to separate
    the sync from the store's own work.
- **Profile scopes:** v0.9.7's test-only `prof!` scopes, inclusive and nested. On the v0.9.7
  measurement branch, scopes were also added inside the store's commit (`store.*`), the
  write-back (`wb.*`), the history tick (`history.*`) and the provenance rebuild (`prov.*`).
  These are test-only and no behaviour changes.
- **Which kit:**
  - **v0.9.7** (`v097/merge-main` 7059125, which has the soak's profile scopes): local
    branch `perf/kit-investigation-v097`, measurement only, not for merge.
  - **kit main** (3ada409): branch `perf/kit-investigation`.
  - The two kits measure the same except for the write-back (§4).
  - File:line references below are at 7059125.
- **Nodes and runs** (x86_64 Linux; `--cpus 4 --mem 8G`, release):

| Job | Kit | Node | Exclusive | Note |
|---|---|---|---|---|
| `6491b3f9` | v0.9.7 | A | yes | the main tables |
| `3820966c` | main 3ada409 | A | yes | the same bench on kit main |
| `9f3081e5` | v0.9.7 + more scopes | B | yes | the breakdowns of flush, tick, prov rebuild, store |
| `c89e6b1f` | v0.9.7 (an earlier bench revision) | A | no | the shared-node spike (§0) |

Soak logs cited (not mine): `1699c679` (v0.9.7 soak, shared node, the 9× step) and `01e766d0`
(the gate soak, kit a47d616, exclusive). Their per-scope profile lines are log lines, not run
records: those numbers are marked `[unverified: job … log line]`.

## Summary: per-call cost, warm (v0.9.7, node A, exclusive)

p50 in µs (500 samples each); the doc's text is the same size at every point.

| Call | 10k versions | 50k | 100k |
|---|---|---|---|
| `edit` | 382 [run 6491b3f9/perfkit_10000_edit:p50_us] | 429 [run 6491b3f9/perfkit_50000_edit:p50_us] | 492 [run 6491b3f9/perfkit_100000_edit:p50_us] |
| `edit_with` | 414 [run 6491b3f9/perfkit_10000_edit_with:p50_us] | 464 [run 6491b3f9/perfkit_50000_edit_with:p50_us] | 536 [run 6491b3f9/perfkit_100000_edit_with:p50_us] |
| `run_tool replace` | 425 [run 6491b3f9/perfkit_10000_tool_replace:p50_us] | 471 [run 6491b3f9/perfkit_50000_tool_replace:p50_us] | 536 [run 6491b3f9/perfkit_100000_tool_replace:p50_us] |
| `run_tool sed -i` | 615 [run 6491b3f9/perfkit_10000_tool_sed:p50_us] | 667 [run 6491b3f9/perfkit_50000_tool_sed:p50_us] | 742 [run 6491b3f9/perfkit_100000_tool_sed:p50_us] |
| `undo` | 410 [run 6491b3f9/perfkit_10000_undo:p50_us] | 457 [run 6491b3f9/perfkit_50000_undo:p50_us] | 512 [run 6491b3f9/perfkit_100000_undo:p50_us] |
| `cat` (read) | 51 [run 6491b3f9/perfkit_10000_tool_cat:p50_us] | 50 [run 6491b3f9/perfkit_50000_tool_cat:p50_us] | 56 [run 6491b3f9/perfkit_100000_tool_cat:p50_us] |
| store probe (1 durable commit) | 98 [run 6491b3f9/perfkit_10000_store_probe:p50_us] | 99 [run 6491b3f9/perfkit_50000_store_probe:p50_us] | 102 [run 6491b3f9/perfkit_100000_store_probe:p50_us] |
| **`flush` (write-back)** | 2,278 [run 6491b3f9/perfkit_10000_flush:p50_us] | 7,539 [run 6491b3f9/perfkit_50000_flush:p50_us] | **13,997 [run 6491b3f9/perfkit_100000_flush:p50_us]** |
| **`tick` (history duties)** | 1,004 [run 6491b3f9/perfkit_10000_tick:p50_us] | 4,572 [run 6491b3f9/perfkit_50000_tick:p50_us] | **8,900 [run 6491b3f9/perfkit_100000_tick:p50_us]** |
| **first edit after a reopen** | 9,040 [run 6491b3f9/perfkit_10000_cold:first_edit_us] | 50,847 [run 6491b3f9/perfkit_50000_cold:first_edit_us] | **112,591 [run 6491b3f9/perfkit_100000_cold:first_edit_us]** |
| reopen (`open_file`) | 19,489 [run 6491b3f9/perfkit_10000_cold:open_us] | 97,275 [run 6491b3f9/perfkit_50000_cold:open_us] | 209,080 [run 6491b3f9/perfkit_100000_cold:open_us] |

- **Warm calls grow slowly:** about 1.3× [derived] from 10k to 100k, at the same text size.
- **Four paths grow linearly with history:** the write-back, the history tick, the first
  edit after a load, and the open.
- **The growth phase's cost per call** (all calls and the host's ticks and flushes, averaged):
  - 439 [run 6491b3f9/perfkit_window_10000:us_per_call] µs in the window to 10k;
  - 1,060 [run 6491b3f9/perfkit_window_100000:us_per_call] µs in the window to 100k;
  - smooth in between, every 5,000 versions.

## 0. The commit.store step in soak `1699c679` is a shared-node artefact, not the store

- **The step:** in soak `1699c679` (v0.9.7, shared node), `commit.store` went from 1,074 µs
  per commit in the window to v60k to 6,603 µs in the window to v70k
  [unverified: job 1699c679 log line]. That is a 6× step in one window.
- **It does not reproduce on an exclusive node.** On node A, `commit.store` per growth call
  grows smoothly across v55k–v80k:
  - 324,031 [run 6491b3f9/perfkit_window_55000:commit_store_ns] ns at v55k;
  - 350,558 [run 6491b3f9/perfkit_window_70000:commit_store_ns] ns at v70k;
  - 355,197 [run 6491b3f9/perfkit_window_80000:commit_store_ns] ns at v80k.
  - Nothing steps where the store file grows (57 → 86 MB at v70k:
    85,803,008 [run 6491b3f9/perfkit_window_70000:store_file_bytes] bytes).
  - Nothing steps where redb's B-tree gains a level (4 → 5 at v80k:
    5 [run 6491b3f9/perfkit_window_80000:redb_tree_height]).
  - Compactions run 5 per window throughout.
- **The gate soak agrees.** `01e766d0` (exclusive, kit a47d616) shows `commit.store`
  311 → 430 → 467 µs at v10k → v70k → v90k [unverified: job 01e766d0 log line].
- **Contention reproduces the step.** My own non-exclusive run on node A had one window at
  10,596,062 [run c89e6b1f/perfkit_window_20000:commit_store_ns] ns per call (v15k–v20k).
  - That is about 30× [derived] the window before it:
    331,112 [run c89e6b1f/perfkit_window_15000:commit_store_ns].
  - The window after it was back to normal: 768,365 [run c89e6b1f/perfkit_window_25000:commit_store_ns].
  - The file size was the same before and after.
- **In `1699c679` the step matches a co-tenant, not the doc.**
  - It starts at about 390 s of wall time.
  - That is about when another job on the same node started its run phase
    [unverified: job start times from the compute log; no record].
  - The soak's CPU-only phases (the line audit, presence) did not step
    [unverified: job 1699c679 log line].
- **Verdict:** each change's commit waits on one `fsync`, so a co-tenant's writes on a shared
  disk inflate `commit.store`.
  - It is not O(history). Drop it from the ranking.
  - Time the kit on exclusive nodes only.
  - The kit's exposure is one sync per change, and three for a write-back (§3). Group
    commit would cut that, but it is a durability design change, not a v0.9.8 fix.

## Ranked causes (biggest win first)

Ranked by how much of the smooth growth each explains (the gate soak's `session.commit`
402 → 1,161 µs and `apply_local` 74 → 633 µs from v10k to v90k, `01e766d0`), plus the
calls outside the edit path that grow linearly. **The largest single growing term per edit
is not the kit's:** it is the engine (§E), listed after the kit's causes for K2 Engine.

### 1. The provenance map is rebuilt from a throwaway replica on every load: O(history)

- **Cause:**
  - `Session::prov` (`session/live.rs:3226`) builds the map on first need after a load.
    The rebuild is `prov_from` (`session/live.rs:3247-3277`).
  - `prov_from` calls `Doc::blame_runs` (`engine/yrs.rs:1201-1237`), which encodes the whole
    engine state, loads it into a throwaway replica and walks every item with `diff_range`.
  - So every load (a restart, a pool eviction and reload, `recover`, a purge) pays O(state
    with its tombstones) on its first edit, undo or blame.
- **Evidence** (node B; nearly all of it is `blame_runs`; the store's provenance rows are flat):

| | 10k | 50k | 100k |
|---|---|---|---|
| `prov_build` on the first edit (ns) | 7,206,339 [run 9f3081e5/perfkit_10000_cold:first_edit_session_prov_build_ns] | 40,172,773 [run 9f3081e5/perfkit_50000_cold:first_edit_session_prov_build_ns] | 90,455,341 [run 9f3081e5/perfkit_100000_cold:first_edit_session_prov_build_ns] |
| of which `blame_runs` (ns) | 6,779,426 [run 9f3081e5/perfkit_10000_cold:first_edit_prov_blame_runs_ns] | 39,563,158 [run 9f3081e5/perfkit_50000_cold:first_edit_prov_blame_runs_ns] | 89,536,403 [run 9f3081e5/perfkit_100000_cold:first_edit_prov_blame_runs_ns] |
| of which `provenance_rows` (ns) | 385,763 [run 9f3081e5/perfkit_10000_cold:first_edit_prov_rows_ns] | 484,214 [run 9f3081e5/perfkit_50000_cold:first_edit_prov_rows_ns] | 493,174 [run 9f3081e5/perfkit_100000_cold:first_edit_prov_rows_ns] |
| warm `prov_build` per edit (ns) | 45 [run 6491b3f9/perfkit_10000_edit:session_prov_build_ns] | 54 [run 6491b3f9/perfkit_50000_edit:session_prov_build_ns] | 56 [run 6491b3f9/perfkit_100000_edit:session_prov_build_ns] |

- **What the soak's prov_build number is:** warm, the map is kept exact by every change.
  `prov_build` is a lookup costing tens of ns. The soak's per-edit `prov_build` is this
  rebuild, amortised over its restarts (one every 3,000 calls):
  - 4 → 119 µs per `apply_local` from v10k to v90k [unverified: job 01e766d0 log line];
  - about 21 % [derived] of `apply_local`'s growth there.
- **Fix:**
  - **(a) No format bump:** build from the live doc without a replica. Use an engine read
    that lists each live item's (client, clock, length) without splitting blocks (K2 Engine's
    side). It is still O(items), but it removes the encode, the decode and the second walk.
  - **(b) Format bump (STORE_FORMAT 5 → 6):**
    - Persist the map (each root's runs and the live restorations by root; about 300 runs
      here, 296 [run 6491b3f9/perfkit_100000_sizes:prov_runs]) in a new table, as of a
      version, written at unload or release and at compaction.
    - A load whose version matches uses it, and anything else rebuilds as today. The
      sampled check against a rebuild (blame's) stays.
- **Expected gain:**
  - (b): the first edit after a load goes from about 112 ms to about 1 ms at 100k
    [estimate], and the soak's `prov_build` goes to about 0.
  - (a): about 3–5× [estimate] less.
- **Risk:**
  - (b): a stale persisted map. Guard it with the version and the doc's state vector, and
    keep the differential test.
  - (a): engine work.
- **Format bump:** (a) no, (b) yes.

### 2. Every commit copies the whole snapshot to read one number: O(state)

- **Cause:** `DocStore::append_in` (`store/mod.rs:1184-1208`) computes the next log
  position from the last update and from the snapshot:
  - `next.max(dec_snapshot(v.value())?.0 + 1)` (`store/mod.rs:1198-1199`).
  - `dec_snapshot` (`store/mod.rs:888-896`) copies the whole engine state (`r.bytes()?`
    into a `Vec`) just to read the snapshot's seq.
  - It runs on every commit of every change.
  - The snapshot is needed only when the log is empty: right after a compaction. Otherwise
    the last update's seq is already past it.
- **Evidence** (`store.append` per edit; the engine state: 194,244 [run 6491b3f9/perfkit_10000_sizes:state_bytes] →
  982,332 [run 6491b3f9/perfkit_50000_sizes:state_bytes] →
  1,974,265 [run 6491b3f9/perfkit_100000_sizes:state_bytes] bytes):

| | 10k | 50k | 100k |
|---|---|---|---|
| `store.append` per edit (ns) | 21,186 [run 6491b3f9/perfkit_10000_edit:store_append_ns] | 46,878 [run 6491b3f9/perfkit_50000_edit:store_append_ns] | 82,829 [run 6491b3f9/perfkit_100000_edit:store_append_ns] |
| the same, node B (ns) | 18,190 [run 9f3081e5/perfkit_10000_edit:store_append_ns] | 42,572 [run 9f3081e5/perfkit_50000_edit:store_append_ns] | 76,333 [run 9f3081e5/perfkit_100000_edit:store_append_ns] |

- **How it was confirmed:**
  - It scales with the state, not with the log, which compaction keeps under 1,000 entries.
  - It is the only O(state) step in `append_in`.
  - It is the largest part of `commit.store`'s growth: 308,667 [run 6491b3f9/perfkit_10000_edit:commit_store_ns] →
    420,234 [run 6491b3f9/perfkit_100000_edit:commit_store_ns] ns, of which `store.append`
    is +61,643 ns [derived].
- **Fix:** read the snapshot only when the update log is empty, or read only its 9-byte
  header (`u8` version, `u64` seq) without copying the state. No other change.
- **Expected gain:**
  - About −60 to −75 µs per commit at 100k [derived]: 12–15 % of an `edit`'s p50.
  - It grows with the state, so more at larger histories.
- **Risk:** very low (the seq rule is unchanged; a unit test with an empty log after a
  compaction).
- **Format bump:** no.

### 3. Each write-back encodes the whole state and writes it into the store twice: O(state)

- **Cause:** `Session::write_back` (`session/live.rs:5065-5145`) does these steps every time
  the file is written:
  - encodes the doc's full state (`encode_state`);
  - stores it as the pending sync state (`set_pending_sync`, a durable commit of the whole
    state);
  - writes the file (`atomic_write`, flat);
  - promotes the pending state to synced (`promote_pending_sync`, a second durable commit
    that moves the whole state).
  - The session lock is held throughout, so every participant waits.
- **Evidence** (node B, per flush, ns):

| | 10k | 50k | 100k |
|---|---|---|---|
| `flush` p50 (µs) | 2,061 [run 9f3081e5/perfkit_10000_flush:p50_us] | 6,933 [run 9f3081e5/perfkit_50000_flush:p50_us] | 13,045 [run 9f3081e5/perfkit_100000_flush:p50_us] |
| `wb.encode_state` | 721,823 [run 9f3081e5/perfkit_10000_flush:wb_encode_state_ns] | 3,657,545 [run 9f3081e5/perfkit_50000_flush:wb_encode_state_ns] | 7,693,206 [run 9f3081e5/perfkit_100000_flush:wb_encode_state_ns] |
| `wb.set_pending_sync` | 357,976 [run 9f3081e5/perfkit_10000_flush:wb_set_pending_sync_ns] | 1,302,961 [run 9f3081e5/perfkit_50000_flush:wb_set_pending_sync_ns] | 2,434,134 [run 9f3081e5/perfkit_100000_flush:wb_set_pending_sync_ns] |
| `wb.promote` | 390,142 [run 9f3081e5/perfkit_10000_flush:wb_promote_ns] | 1,282,091 [run 9f3081e5/perfkit_50000_flush:wb_promote_ns] | 2,343,444 [run 9f3081e5/perfkit_100000_flush:wb_promote_ns] |
| `wb.atomic_write` (the file) | 470,854 [run 9f3081e5/perfkit_10000_flush:wb_atomic_write_ns] | 483,847 [run 9f3081e5/perfkit_50000_flush:wb_atomic_write_ns] | 480,821 [run 9f3081e5/perfkit_100000_flush:wb_atomic_write_ns] |
| `wb.replica_to` (v0.9.7's catch-up) | 139,159 [run 9f3081e5/perfkit_10000_flush:wb_replica_to_ns] | 186,853 [run 9f3081e5/perfkit_50000_flush:wb_replica_to_ns] | 210,996 [run 9f3081e5/perfkit_100000_flush:wb_replica_to_ns] |

- **Kit main (3ada409) is far worse.** It reloads the disk replica on every write-back
  (v0.9.7's catch-up fixes that), so its `flush` p50 is:
  - 9,143 [run 3820966c/perfkit_10000_flush:p50_us] µs at 10k;
  - 41,758 [run 3820966c/perfkit_50000_flush:p50_us] µs at 50k;
  - 90,731 [run 3820966c/perfkit_100000_flush:p50_us] µs at 100k.
  - Its growth phase per call reaches 2,535 [run 3820966c/perfkit_window_100000:us_per_call] µs,
    against v0.9.7's 1,060.
- **Fix:**
  - **(a) No format bump:**
    - Make the promote a rename of the pending row's role, not a second write of the bytes.
    - Encode the state once per write-back and reuse it (it is already reused for
      `replica_to`).
  - **(b) Format bump:**
    - Persist the sync point as (seq, hash) plus the catch-up since the last snapshot,
      instead of a full state per write-back.
    - The replica is then rebuilt at load from the snapshot and the log up to the synced
      seq. Compaction must keep that seq reachable, or write the synced state only then.
- **Expected gain:**
  - (a): about −2.3 ms of 13 ms at 100k [derived].
  - (b): about −12 ms [derived]: what is left is the file write and the catch-up.
- **Risk:**
  - (a): low.
  - (b): the crash-safety protocol of the write-back (K-AUD-1) must be re-proved. It needs
    the disk crash tests.
- **Format bump:** (a) no, (b) yes.

### 4. The history tick reads every revert record each minute: O(revert records), which here is O(history)

- **Cause:** `history_tick` (`session/live/history.rs:548`) calls `prune_reverts`
  (`session/live/history.rs:580-630`) every `history_tick_ms` (60 s).
  - Once the store holds more than `revert_keep_changes` (10,000) records,
    `prune_reverts` reads the whole revert index (`store.revert_index`,
    `store/history.rs:1268-1278`), every record.
  - It then plans the cut (`window_plan`, `store/history.rs:961-983`).
  - A record stays while it is in the last 10,000 changes **or** younger than
    `revert_keep_ms` (30 days). So in any doc edited within 30 days nothing is dropped, and
    the full read repeats every minute, forever.
  - The records pile up with the history: 100,000 [run 6491b3f9/perfkit_100000_sizes:revert_records],
    12,952,311 [run 6491b3f9/perfkit_100000_sizes:revert_bytes] bytes.
- **Evidence** (node B, per tick):
  - `history.revert_index_read` per tick:
    - 843,827 [run 9f3081e5/perfkit_10000_tick:history_revert_index_read_ns] ns at 10k;
    - 4,099,173 [run 9f3081e5/perfkit_50000_tick:history_revert_index_read_ns] ns at 50k;
    - 8,029,656 [run 9f3081e5/perfkit_100000_tick:history_revert_index_read_ns] ns at 100k.
    - That is nearly all of the tick: 8,184 [run 9f3081e5/perfkit_100000_tick:p50_us] µs.
  - Version thinning and the disk tick are flat:
    8,721 [run 9f3081e5/perfkit_100000_tick:history_thin_versions_ns] and
    67,653 [run 9f3081e5/perfkit_100000_tick:disk_tick_ns] ns.
  - The soaks' `us_per_tick` follows it: 3,890 [run 01e766d0/soak_32_v10000:us_per_tick] µs
    at v10k, 45,564 [run 01e766d0/soak_32_v90000:us_per_tick] µs at v90k.
- **Fix:**
  - Before reading the index, check whether anything can leave the window. Read the oldest
    record (one `range().next()`): when it is younger than `keep_ms`, nothing can go (the
    rule is an "or"), so return. That is O(1).
  - Or keep the index in memory, built once per load.
  - No change to what is dropped.
- **Expected gain:** about 8 ms → under 0.1 ms per tick at 100k [estimate]. It holds the
  session lock, so it also removes an 8 ms stall per minute per doc.
- **Risk:** low (the window rule is unchanged; a test with records past and inside the window).
- **Format bump:** no.

### 5. Compaction every 1,000 entries encodes and writes the whole state: O(state) per 1,000 changes

- **Cause:** `maybe_compact` (`session/live.rs:3515-3520`) compacts when the log reaches
  `compact_entries` (1,000) or `compact_bytes` (2 MiB).
  - `compact` (`session/live.rs:3522-3549`) encodes the full state and stores it as the
    snapshot (`store/mod.rs:1258-1277`).
  - The trigger counts entries, so the cost per change grows with the state.
- **Evidence** (`commit.compact` per growth call, which is amortised):
  - 2,106 [run 6491b3f9/perfkit_window_10000:commit_compact_ns] ns at 10k;
  - 14,930 [run 6491b3f9/perfkit_window_100000:commit_compact_ns] ns at 100k.
  - The gate soak shows 2 → 23 µs per commit [unverified: job 01e766d0 log line].
- **Fix:** compact when the log's bytes reach a fraction of the state's (for example log ≥
  state / 2), with the entry count as a floor. Each compaction then costs O(log bytes)
  amortised, constant per change.
- **Expected gain:** about −10 to −15 µs per change at 100k [derived].
- **Risk:**
  - Loads replay a longer log, still bounded by the state's size.
  - It interacts with §2's seq read (the log is empty less often).
- **Format bump:** no.

### 6. A reopen decodes the full state twice (the doc and the disk replica): O(state)

- **Cause:**
  - `Session::load` (`session/live.rs:941-989`) decodes the snapshot and replays the log
    (`Doc::from_store`, `doc.rs:176-185`).
  - Attaching the file then decodes the stored sync state into the disk replica
    (`session/live.rs:4174-4177`): a second full decode.
- **Evidence:**
  - `open_file` takes 16,466 [run 9f3081e5/perfkit_10000_cold:open_us] µs at 10k and
    175,671 [run 9f3081e5/perfkit_100000_cold:open_us] µs at 100k.
  - The same on node A: 19,489 [run 6491b3f9/perfkit_10000_cold:open_us] and
    209,080 [run 6491b3f9/perfkit_100000_cold:open_us] µs.
  - The `open_wb_*` fields in the cold records are the write-back that `release` does
    before the reopen. They are not part of `open_us`.
- **Fix:** load the replica lazily (it is first needed at the next write-back or outside
  save), or, when the synced seq is the doc's version, make it from the doc's own state.
- **Expected gain:** about half of the open [estimate].
- **Risk:** low.
- **Format bump:** no.

### 7. Each change carries every participant's undo stack: O(participants × undo depth), flat in history

- **Cause:** `Session::carry` (`session/live.rs:3285-3325`) transforms every batch on every
  pid's undo and redo stacks through each change (`Batch::transform`). It also carries the
  revert index when built (O(log R), cheap).
  - The stacks hold up to `undo_depth` (100) per pid: 800 [run 6491b3f9/perfkit_100000_sizes:undo_batches]
    batches here, about 3,200 in the 32-participant soak.
  - Then `push_batch` (`session/live.rs:2952-2986`) re-sums every batch's bytes on each push.
- **Evidence:** `session.carry` per edit:
  - 35,728 [run 6491b3f9/perfkit_10000_edit:session_carry_ns] ns at 10k;
  - 54,577 [run 6491b3f9/perfkit_100000_edit:session_carry_ns] ns at 100k.
  - It grows mildly because regions' ID runs fragment.
  - The soaks show 113–183 µs [unverified: job 1699c679 log line] and 33–54 µs
    [unverified: job 01e766d0 log line] per `apply_local`.
- **Fix:**
  - Keep batch region bounds in a shared offset tree, as `RevertIndex` does for revert
    records (`session/carry.rs:376-584`): O(hunks × log B) per change.
  - Or carry lazily, replaying the hunks since a batch's last carry only when it is undone.
  - Keep a running byte total instead of re-summing.
- **Expected gain:** about −40 µs per edit with 8 participants, about −150 µs with 32 [estimate].
- **Risk:** medium. The undo model's evidence (U10 carries, D1) must be re-run against the
  new representation: the spec-undo part and the soak.
- **Format bump:** no (memory only).

### 8. The store commit per change: seven tables, no count bound on revert records

- **Cause:** each change's commit (`DocStore::commit_changes`, `store/history.rs:988-1002`;
  `put_changes`, `store/history.rs:826-885`) writes the update, the change summary, the
  revert record, its index entry, the revert stats, the clock span and any provenance rows,
  then syncs. Revert records are never dropped while younger than 30 days (§4), so the file
  only grows:
  - 85,803,008 [run 6491b3f9/perfkit_100000_sizes:store_file_bytes] bytes at 100k;
  - of which 43,310,316 [run 6491b3f9/perfkit_100000_sizes:redb_fragmented_bytes] bytes are
    fragmented;
  - B-tree height 5 [run 6491b3f9/perfkit_100000_sizes:redb_tree_height].
- **Evidence** (per edit, ns):

| | 10k | 50k | 100k |
|---|---|---|---|
| `store.txn_commit` | 244,606 [run 6491b3f9/perfkit_10000_edit:store_txn_commit_ns] | 262,984 [run 6491b3f9/perfkit_50000_edit:store_txn_commit_ns] | 279,518 [run 6491b3f9/perfkit_100000_edit:store_txn_commit_ns] |
| `store.put_changes` | 41,221 [run 6491b3f9/perfkit_10000_edit:store_put_changes_ns] | 48,447 [run 6491b3f9/perfkit_50000_edit:store_put_changes_ns] | 56,035 [run 6491b3f9/perfkit_100000_edit:store_put_changes_ns] |

  - With the sync off, `store.txn_commit` is 166,579 [run 6491b3f9/perfkit_100000_edit_nosync:store_txn_commit_ns] ns.
  - So about 113 µs [derived] of a commit at 100k is the sync. The rest is redb's own commit
    work: its copy-on-write pages over seven tables.
  - The bare one-row probe is 102 [run 6491b3f9/perfkit_100000_store_probe:p50_us] µs.
- **Fix (modest):**
  - Merge the per-change rows (summary, revert index entry, clock span) into one record per
    change, so fewer B-trees are dirtied per commit (format bump).
  - Bound revert records by count as well as by time (a retention-policy change for the product owner).
- **Expected gain:** −50 to −100 µs per commit [estimate].
- **Risk:** medium (a new record encoding and its migration).
- **Format bump:** yes for the merge.

### E. For K2 Engine (not the kit): the engine's edit grows with history and is the largest term

- **Inside `apply_local`,** the engine's transaction ops (`engine.txn_ops`: the Yrs
  `remove_range` and `insert` calls in `main_doc_update`, `engine/yrs.rs:465-525`; nothing
  of the kit's runs in that scope in a release build) grow per edit:
  - 11,844 [run 6491b3f9/perfkit_10000_edit:engine_txn_ops_ns] ns at 10k;
  - 57,076 [run 6491b3f9/perfkit_50000_edit:engine_txn_ops_ns] ns at 50k;
  - 119,372 [run 6491b3f9/perfkit_100000_edit:engine_txn_ops_ns] ns at 100k.
  - That is the largest per-edit growth in the bench.
  - The gate soak: 9 → 311 µs per edit from v10k to v90k [unverified: job 01e766d0 log line].
    `engine.edit_as` is +418 µs of `apply_local`'s +559 µs growth [unverified: job 01e766d0 log line].
- **The first edit after a load** pays 5,685,987 [run 9f3081e5/perfkit_100000_cold:first_edit_engine_txn_ops_ns] ns
  in it at 100k, which suggests the position index starts cold.
- **The canary:** its sampled full-state check is a further 5–19 µs amortised per call.

## run_tool and edit_with

- **Nothing in the tools path is O(history).**
- **`run_tool`'s overhead over `edit_with`** (verb parse, output, the App lens) is a few µs
  and flat: `tools.run_tool` minus `session.edit_with` is about 3.9 µs [derived] at 100k.
- **`edit_with`'s own work** is the closure, the diff and the claim and note checks. The
  diff, `session.compute_diff`, is O(text) (`lines::align_insertions(live,
  diff::hunks(live, new))`, `session/live.rs:2182-2196`) and flat here at the same text size.
- **Their growth is the commit's:** §2, §5, §7, §8 and the engine. `sed -i` costs about
  200 µs more than `replace` at every size: the regex compile and run, flat.

## The soak numbers, mapped to these causes

The scopes are nested and inclusive: `tools.run_tool` ⊃ `session.edit_with` ⊃
`session.commit` ⊃ `session.apply_local` ⊃ `engine.edit_as`. They are not additive.

K2 Engine's figures, as relayed, are per 10k-version window at v100k
[unverified: K2 Engine's report as relayed, no job id]:
- `session.commit` + store 8.6 s;
- `tools.run_tool` 6.7 s;
- `edit_with` 6.6 s;
- `apply_local` 4.9 s, 9× over the history;
- `prov_build` 3 → 106 µs per edit.

| Soak figure | What it is |
|---|---|
| `commit.store` step (6.6 ms per commit at v70k, `1699c679`) | §0: shared-node fsync contention, not the store. The exclusive gate soak `01e766d0` and the bench grow smoothly |
| `commit.store`'s smooth growth (311 → 467 µs, `01e766d0`) | §2, the snapshot copy per commit (the most growth); §8, redb's commit over a growing file; §5, compaction |
| `apply_local` 9× (74 → 633 µs, `01e766d0`) | about 75 % [derived]: the engine's edit (§E, K2 Engine). About 21 %: `prov_build`, i.e. the rebuild per restart (§1). The rest: the undo carry (§7, flat but large with 32 participants) |
| `prov_build` 3 → 106 µs per edit | §1: the throwaway-replica rebuild after each of the soak's restarts (one every 3k calls, `restart_every` in `tests/soak.rs`), amortised. Warm it is about 50 ns |
| `session.edit_with` and `tools.run_tool` | the commit inside them, so the rows above. `run_tool` adds a few µs flat |
| the soak's per-call slope (`us_per_call` 904 [run 01e766d0/soak_32_v10000:us_per_call] → 6,479 [run 01e766d0/soak_32_v90000:us_per_call]) | the above, plus the history tick (§4: `us_per_tick` 3,890 [run 01e766d0/soak_32_v10000:us_per_tick] → 45,564 [run 01e766d0/soak_32_v90000:us_per_tick]), the write-back on its flushes (§3), and the reopen at each restart (§6, not in the kit's edit scopes) |

## Not done or uncertain

- **Write-back attribution:** the split rests on test-only scopes added on the measurement
  branch only (`perf/kit-investigation-v097`, 9cafb64). Kit main has no profile scopes, so
  its run (`3820966c`) gives whole-call times only.
- **Expected gains are estimates** until each fix is measured with this bench. Rerun it at
  the same node and settings, before and after.
- **The engine-side growth (§E)** was not investigated beyond the scope boundary.
- **The co-tenant timing** for `1699c679` comes from the compute service's job times, not a
  run record.

## v0.9.8 quick fixes: before/after

Five of the ranked fixes, made with **no store-format change and no behaviour change**, on
branch `perf/kit-quick`. It is rebased on the v0.9.7 candidate `a150e7d`, so the `prof!`
scopes are there; the bench and its test-only scopes are `a29c0e0`–`0df6ab6`.

| Fix | Commit | What |
|---|---|---|
| #2 | `2dd1c8f` | a commit reads the snapshot's position in place (`dec_snapshot_ref`), not a copy of the whole state |
| #4 | `adfd51d` | the history tick checks the window's oldest record first (`DocStore::first_revert`) and returns when nothing can go |
| #3a | `5753c25` | the write-back writes the synced row from the bytes it just recorded (`promote_written_sync`); a re-sync is one transaction |
| #5 | `a8f3ee7` | compaction: from `compact_entries` entries once the log's bytes reach a quarter of the snapshot's; always at 8 × `compact_entries` entries or `compact_bytes` |
| #6 | `92ac38d` | a reopened doc keeps its disk replica encoded until the disk path first needs it |

- **Rebased onto main `88d53eb` (v0.9.7)** after these runs. The fixes are now:
  - #2 `8128d72`;
  - #4 `fd831aa`;
  - #3a `43a883b`;
  - #5 `3579eaa`;
  - #6 `c05614b`;
  - #6's follow-up `e689c36` (a damaged sync state fails the first write-back, before
    anything is written).
- **The runs below** were measured at the commits named in the table, before the rebase.
- **After the rebase, `77c0f36` vendors k2-yrs k2.6.4.**

### How it was measured

- **The bench:** `tests/perf_kit.rs` as in the sections above, at 10k, 50k and 100k
  versions. Each commit has its own job, and the jobs ran one after another on **one node**
  (node B, x86_64 Linux, `--cpus 4 --mem 8G`, release).
- **Not exclusive:** exclusive runs now need a compute grant, which was requested and not
  yet given.
  - So each "before" is the job for the commit just before the fix, run minutes before the
    "after".
  - Differences under about 5 % are noise; the warm per-call rows show it.
- **Jobs, in order:**
  - `9e5225b5`: `0df6ab6`, the base;
  - `c630cc39`: #2;
  - `d4829ee6`: #4;
  - `0341d9dc`: #3a;
  - `d1f0ead6`: #5;
  - `72821787`: #6.

### #2: the snapshot's position read in place

- `store.append` no longer grows with the state: it is flat from 10k to 100k.
- At 100k an `edit`'s p50 is about 13 % [derived] lower.

| #2: the snapshot's seq read in place | 10k | 50k | 100k |
|---|---|---|---|
| `store.append` per edit (ns), before | 18,685 [run 9e5225b5/perfkit_10000_edit:store_append_ns] | 42,374 [run 9e5225b5/perfkit_50000_edit:store_append_ns] | 80,082 [run 9e5225b5/perfkit_100000_edit:store_append_ns] |
| after | 12,879 [run c630cc39/perfkit_10000_edit:store_append_ns] | 14,169 [run c630cc39/perfkit_50000_edit:store_append_ns] | 14,860 [run c630cc39/perfkit_100000_edit:store_append_ns] |
| `commit.store` per edit (ns), before | 276,009 [run 9e5225b5/perfkit_10000_edit:commit_store_ns] | 334,815 [run 9e5225b5/perfkit_50000_edit:commit_store_ns] | 386,563 [run 9e5225b5/perfkit_100000_edit:commit_store_ns] |
| after | 271,403 [run c630cc39/perfkit_10000_edit:commit_store_ns] | 287,847 [run c630cc39/perfkit_50000_edit:commit_store_ns] | 312,003 [run c630cc39/perfkit_100000_edit:commit_store_ns] |
| `edit` p50 (µs), before | 340 [run 9e5225b5/perfkit_10000_edit:p50_us] | 382 [run 9e5225b5/perfkit_50000_edit:p50_us] | 440 [run 9e5225b5/perfkit_100000_edit:p50_us] |
| after | 335 [run c630cc39/perfkit_10000_edit:p50_us] | 360 [run c630cc39/perfkit_50000_edit:p50_us] | 384 [run c630cc39/perfkit_100000_edit:p50_us] |

### #4: the history tick checks the oldest record first

The tick is O(1) now: about 100× [derived] faster at 100k, and flat across the sizes.

| #4: the tick checks the oldest record first | 10k | 50k | 100k |
|---|---|---|---|
| `tick` p50 (µs), before | 976 [run c630cc39/perfkit_10000_tick:p50_us] | 4,243 [run c630cc39/perfkit_50000_tick:p50_us] | 8,344 [run c630cc39/perfkit_100000_tick:p50_us] |
| after | 68 [run d4829ee6/perfkit_10000_tick:p50_us] | 81 [run d4829ee6/perfkit_50000_tick:p50_us] | 88 [run d4829ee6/perfkit_100000_tick:p50_us] |
| `history.prune_reverts` per tick (ns), before | 909,031 [run c630cc39/perfkit_10000_tick:history_prune_reverts_ns] | 4,215,489 [run c630cc39/perfkit_50000_tick:history_prune_reverts_ns] | 8,248,044 [run c630cc39/perfkit_100000_tick:history_prune_reverts_ns] |
| after | 6,752 [run d4829ee6/perfkit_10000_tick:history_prune_reverts_ns] | 8,882 [run d4829ee6/perfkit_50000_tick:history_prune_reverts_ns] | 9,891 [run d4829ee6/perfkit_100000_tick:history_prune_reverts_ns] |

### #3a: the synced row written from the recorded bytes

**Within noise.**
- The promotion no longer reads the pending row back and copies it, but it still writes the
  whole state once more as the synced row.
- Dropping that write needs the sync point stored differently: #3b, a format bump.
- The write-back's cost is the full-state encode and the two durable writes of the state.

| #3a: the synced row from the recorded bytes | 10k | 50k | 100k |
|---|---|---|---|
| `wb.promote` per flush (ns), before | 413,542 [run d4829ee6/perfkit_10000_flush:wb_promote_ns] | 1,233,330 [run d4829ee6/perfkit_50000_flush:wb_promote_ns] | 2,321,071 [run d4829ee6/perfkit_100000_flush:wb_promote_ns] |
| after | 392,272 [run 0341d9dc/perfkit_10000_flush:wb_promote_ns] | 1,295,921 [run 0341d9dc/perfkit_50000_flush:wb_promote_ns] | 2,160,173 [run 0341d9dc/perfkit_100000_flush:wb_promote_ns] |
| `flush` p50 (µs), before | 2,435 [run d4829ee6/perfkit_10000_flush:p50_us] | 7,108 [run d4829ee6/perfkit_50000_flush:p50_us] | 12,895 [run d4829ee6/perfkit_100000_flush:p50_us] |
| after | 2,259 [run 0341d9dc/perfkit_10000_flush:p50_us] | 7,107 [run 0341d9dc/perfkit_50000_flush:p50_us] | 12,945 [run 0341d9dc/perfkit_100000_flush:p50_us] |

### #5: compaction by the log's size against the snapshot's

- **Compactions stop coming every 1,000 entries once the snapshot is large.** At 100k the log
  reaches the hard bound (8,000 entries) before a quarter of the snapshot, and
  `commit.compact` amortised per call drops to tens of ns.
- **Up to 50k a window still compacts:** the log reaches a quarter of the snapshot first.
- **Trade-off:** a load after a crash can replay up to 8 × `compact_entries` entries
  instead of `compact_entries`. An unload still compacts at `unload_compact_entries` (100),
  so a clean restart replays little.

| #5: compaction by the log's size against the snapshot's | 10k | 50k | 100k |
|---|---|---|---|
| `commit.compact` per growth call (ns), before | 1,972 [run 0341d9dc/perfkit_window_10000:commit_compact_ns] | 7,812 [run 0341d9dc/perfkit_window_50000:commit_compact_ns] | 13,951 [run 0341d9dc/perfkit_window_100000:commit_compact_ns] |
| after | 1,925 [run d1f0ead6/perfkit_window_10000:commit_compact_ns] | 2,229 [run d1f0ead6/perfkit_window_50000:commit_compact_ns] | 26 [run d1f0ead6/perfkit_window_100000:commit_compact_ns] |
| compactions in the window, before | 5 [run 0341d9dc/perfkit_window_10000:compactions] | 5 [run 0341d9dc/perfkit_window_50000:compactions] | 5 [run 0341d9dc/perfkit_window_100000:compactions] |
| after | 5 [run d1f0ead6/perfkit_window_10000:compactions] | 1 [run d1f0ead6/perfkit_window_50000:compactions] | 0 [run d1f0ead6/perfkit_window_100000:compactions] |
| `edit` p50 (µs), before | 331 [run 0341d9dc/perfkit_10000_edit:p50_us] | 359 [run 0341d9dc/perfkit_50000_edit:p50_us] | 387 [run 0341d9dc/perfkit_100000_edit:p50_us] |
| after | 332 [run d1f0ead6/perfkit_10000_edit:p50_us] | 361 [run d1f0ead6/perfkit_50000_edit:p50_us] | 391 [run d1f0ead6/perfkit_100000_edit:p50_us] |

### #6: the disk replica decoded on first need

- The reopen is about 24 % [derived] faster at 100k.
- The decode moves to the first disk need. Here that is the first write-back after the
  reopen, inside the timed `flush` samples, so the flush p50 rises slightly.
- An edit is unaffected.
- **A damaged stored sync state** is found at the first disk need after the reopen instead of
  at the open (commit `25d2b7d`).
  - That write-back, and every later one, fails before writing anything, with the open's
    error (the same variant and the same id). So do outside saves.
  - The file, the synced row and the log are untouched, and nothing is left in flight.
  - Before this follow-up, the write-back's catch-up fell back to reloading the replica from
    the new state, which would have replaced the damaged row unseen.
  - `session::disk_crash_tests::a_damaged_sync_state_fails_the_first_write_back_as_the_open_did`
    checks it: it passed in node job `cde36708` [unverified: a test result, not a run record].

| #6: the disk replica decoded on first need | 10k | 50k | 100k |
|---|---|---|---|
| reopen `open_us`, before | 17,980 [run d1f0ead6/perfkit_10000_cold:open_us] | 88,354 [run d1f0ead6/perfkit_50000_cold:open_us] | 197,545 [run d1f0ead6/perfkit_100000_cold:open_us] |
| after | 13,592 [run 72821787/perfkit_10000_cold:open_us] | 61,777 [run 72821787/perfkit_50000_cold:open_us] | 151,114 [run 72821787/perfkit_100000_cold:open_us] |
| first edit after the reopen (µs), before | 7,638 [run d1f0ead6/perfkit_10000_cold:first_edit_us] | 44,175 [run d1f0ead6/perfkit_50000_cold:first_edit_us] | 99,137 [run d1f0ead6/perfkit_100000_cold:first_edit_us] |
| after | 8,267 [run 72821787/perfkit_10000_cold:first_edit_us] | 43,723 [run 72821787/perfkit_50000_cold:first_edit_us] | 107,904 [run 72821787/perfkit_100000_cold:first_edit_us] |
| `flush` p50 (µs), before | 2,219 [run d1f0ead6/perfkit_10000_flush:p50_us] | 6,918 [run d1f0ead6/perfkit_50000_flush:p50_us] | 13,364 [run d1f0ead6/perfkit_100000_flush:p50_us] |
| after | 2,310 [run 72821787/perfkit_10000_flush:p50_us] | 7,152 [run 72821787/perfkit_50000_flush:p50_us] | 13,862 [run 72821787/perfkit_100000_flush:p50_us] |

### All five together

| all five: before (`0df6ab6`) → after (`92ac38d`) | 10k | 50k | 100k |
|---|---|---|---|
| `edit` p50 (µs), before | 340 [run 9e5225b5/perfkit_10000_edit:p50_us] | 382 [run 9e5225b5/perfkit_50000_edit:p50_us] | 440 [run 9e5225b5/perfkit_100000_edit:p50_us] |
| after | 331 [run 72821787/perfkit_10000_edit:p50_us] | 371 [run 72821787/perfkit_50000_edit:p50_us] | 400 [run 72821787/perfkit_100000_edit:p50_us] |
| `run_tool replace` p50 (µs), before | 378 [run 9e5225b5/perfkit_10000_tool_replace:p50_us] | 423 [run 9e5225b5/perfkit_50000_tool_replace:p50_us] | 481 [run 9e5225b5/perfkit_100000_tool_replace:p50_us] |
| after | 372 [run 72821787/perfkit_10000_tool_replace:p50_us] | 410 [run 72821787/perfkit_50000_tool_replace:p50_us] | 446 [run 72821787/perfkit_100000_tool_replace:p50_us] |
| `undo` p50 (µs), before | 363 [run 9e5225b5/perfkit_10000_undo:p50_us] | 406 [run 9e5225b5/perfkit_50000_undo:p50_us] | 460 [run 9e5225b5/perfkit_100000_undo:p50_us] |
| after | 355 [run 72821787/perfkit_10000_undo:p50_us] | 397 [run 72821787/perfkit_50000_undo:p50_us] | 426 [run 72821787/perfkit_100000_undo:p50_us] |
| growth phase per call (µs), before | 397 [run 9e5225b5/perfkit_window_10000:us_per_call] | 664 [run 9e5225b5/perfkit_window_50000:us_per_call] | 955 [run 9e5225b5/perfkit_window_100000:us_per_call] |
| after | 396 [run 72821787/perfkit_window_10000:us_per_call] | 631 [run 72821787/perfkit_window_50000:us_per_call] | 899 [run 72821787/perfkit_window_100000:us_per_call] |
| `tick` p50 (µs), before | 956 [run 9e5225b5/perfkit_10000_tick:p50_us] | 4,688 [run 9e5225b5/perfkit_50000_tick:p50_us] | 8,495 [run 9e5225b5/perfkit_100000_tick:p50_us] |
| after | 63 [run 72821787/perfkit_10000_tick:p50_us] | 80 [run 72821787/perfkit_50000_tick:p50_us] | 86 [run 72821787/perfkit_100000_tick:p50_us] |
| `flush` p50 (µs), before | 2,241 [run 9e5225b5/perfkit_10000_flush:p50_us] | 7,563 [run 9e5225b5/perfkit_50000_flush:p50_us] | 13,205 [run 9e5225b5/perfkit_100000_flush:p50_us] |
| after | 2,310 [run 72821787/perfkit_10000_flush:p50_us] | 7,152 [run 72821787/perfkit_50000_flush:p50_us] | 13,862 [run 72821787/perfkit_100000_flush:p50_us] |
| reopen (µs), before | 17,226 [run 9e5225b5/perfkit_10000_cold:open_us] | 82,805 [run 9e5225b5/perfkit_50000_cold:open_us] | 180,203 [run 9e5225b5/perfkit_100000_cold:open_us] |
| after | 13,592 [run 72821787/perfkit_10000_cold:open_us] | 61,777 [run 72821787/perfkit_50000_cold:open_us] | 151,114 [run 72821787/perfkit_100000_cold:open_us] |

### The soak gate pair: not comparable (shared node), to be rerun exclusive

- **The bench above is the evidence for each fix; the soak only checks for regressions.**
- **The pair below is not comparable:** both ran on a node other jobs shared.
- **The gate comparison still to run:** the gate soak's two runs (incremental write-back
  `SOAK_MAX_SLOWDOWN=8`, then `SOAK_FULL_WRITEBACK=1 SOAK_MAX_SLOWDOWN=8.5`, as `01e766d0`
  ran them), at `a150e7d` and at the branch head, both exclusive on one node.
- **Waiting on:** the compute grant for exclusive runs, requested and not yet given.

- **The runs:** the 32-participant soak (`soak_32`, 600 s, seed 1), before (`a150e7d`) and
  after (`92ac38d`), one after the other on node B.
- **Not exclusive:** exclusive runs need the compute grant, which is still pending.
- **Both fail the soak's own throughput check, and so would the baseline.** The check
  allows at most 2× (`SOAK_MAX_SLOWDOWN`, default 2). The two runs slowed by:
  - before: 482 [run 056fc5ec/soak_32:slowdown_percent] %;
  - after: 638 [run b088ae13/soak_32:slowdown_percent] %.
- **The after run went further in the same 600 s,** so its tail is at a longer history:
  - before: 73,749 [run 056fc5ec/soak_32:version] versions in 138,000 [run 056fc5ec/soak_32:calls] calls;
  - after: 89,865 [run b088ae13/soak_32:version] versions in 168,000 [run b088ae13/soak_32:calls] calls.
  - The gate soak `01e766d0` (kit a47d616, exclusive) reached 96,870 [run 01e766d0/soak_32:version].
- **The before run hit shared-node contention (§0) in two windows:** `commit.store` was
  3,070 and 7,298 µs per commit at v20k and v30k [unverified: job 056fc5ec log line].
- **Per call at equal history** (the soak's 1,000-call milestones):

| History | before `056fc5ec` (µs per call) | after `b088ae13` | gate `01e766d0` (a47d616, exclusive) |
|---|---|---|---|
| 10k | 850 [run 056fc5ec/soak_32_v10000:us_per_call] | 1,081 [run b088ae13/soak_32_v10000:us_per_call] | 904 [run 01e766d0/soak_32_v10000:us_per_call] |
| 40k | 3,014 [run 056fc5ec/soak_32_v40000:us_per_call] | 2,729 [run b088ae13/soak_32_v40000:us_per_call] | 2,756 [run 01e766d0/soak_32_v40000:us_per_call] |
| 70k | 3,833 [run 056fc5ec/soak_32_v70000:us_per_call] | 3,844 [run b088ae13/soak_32_v70000:us_per_call] | 4,086 [run 01e766d0/soak_32_v70000:us_per_call] |

- **Per window** (µs per entry), the after run [unverified: job b088ae13 log line]:
  - `session.commit` 479 → 1,026 from v10k to v80k;
  - `commit.store` 341 → 406;
  - `session.apply_local` 121 → 579;
  - `session.edit_with` 437 → 925;
  - `tools.run_tool` 441 → 866.
- **The before run's same scopes** reached [unverified: job 056fc5ec log line]:
  - `commit.store` 811–1,213 µs from v40k to v70k (after: 355–623);
  - `session.commit` 1,149–1,975 (after: 762–1,116).
- **What the soak can show of these fixes is limited.**
  - **Its growth is mostly not theirs:** the engine's edit (§E) and the provenance rebuild
    at each of its restarts (§1) are what grow.
  - **Its `tick` is mostly its write-back** (the disk tick writes after the quiet time), so
    #4's gain is hidden there. The bench's tick, timed with the disk clean, shows it.
  - **The gain that should show is `commit.store`** (#2, #5). This pair points that way, but
    one run of each on a shared node is not evidence.
- **To do:** rerun the pair exclusive on one node once the grant is given.
