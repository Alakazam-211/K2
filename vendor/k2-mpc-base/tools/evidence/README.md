# Evidence records and citations

Every count in an evidence note (`docs/evidence/*.md`) comes from a run, and says which one.
The tools here keep that true mechanically: a job prints one machine-readable line per run, the
lines become committed run records, and a checker fails any count that is uncited, or does not
match its run, or is a partial (capped) count written as if it were exact.

Self-contained: POSIX sh and Python 3 (stdlib only). Copy the directory as is.

| File | Does |
|---|---|
| `collect.py` | job logs → `docs/evidence/runs/<job>.json` |
| `check.py` | evidence notes + run records → pass / fail |
| `test.sh` | self-test: a fixture that must pass, planted negatives that must fail, the log parser |
| `pre-commit` | git hook: runs `check.py` when a commit touches `docs/evidence/` |

## 1. Run records

A job's summary helpers print, for every run:

```
RUNREC {"job":null,"run":"m_g_2x2","tool":"tlc","status":"COMPLETE","states":5977,"depth":5,"queued":0,"cap_s":180,"generated":15413,"exit":0,"wall_s":12}
```

(`tools/quint-job.sh` defines `tlc_record`, which the TLC helpers call after each run.)
`collect.py` fills in `job` and writes one file per job:

```json
{"job": "3ce98eac", "collected": "2026-10-28", "from": "log command", "runs": [ {record}, ... ]}
```

Record fields (null when unknown):

| Field | Meaning |
|---|---|
| `job` | the job id |
| `run` | the run's name, unique in the job (a repeat gets `#2`, `#3`, ... in log order) |
| `tool` | `tlc` or `enumerator` (`mpce-enum`, which prints its own `RUNREC` lines: `crates/mpce-enum/src/runrec.rs`) |
| `status` | `COMPLETE`: the whole reachable state space, no violation: for TLC, "depth of the complete state graph search" printed and 0 states left on queue; `PARTIAL`: stopped by a cap with states still queued, so `states` is a lower bound; `VIOLATION`: an invariant or property failed; `ERROR`: the tool failed (for TLC, an exit other than 0, 10–14 or 124); `NO_RESULT` / `UNKNOWN`: no count in the log |
| `states` | distinct states (for `PARTIAL`, the count reached when it stopped) |
| `depth` | **One convention, TLC's:** the number of states on the longest shortest path from an initial state, the initial state counting as 1. For `COMPLETE`, TLC's "depth of the complete state graph search", or the enumerator's BFS level count (its levels include the initial state, so it is the same quantity); for `VIOLATION`, TLC's depth reached, or for the enumerator the violation's BFS depth d (d transitions from the initial state) converted to d + 1. Searches without BFS levels (sweep-line, reverse search, sampling) record null. Caveat: TLC with more than one worker is not strictly level-synchronous, so its reported depth can exceed the BFS depth (see `docs/evidence/g1-client-protocol-draft.md`, 2×1 r2), and its first violation need not be a shortest one (§4) |
| `queued` | TLC's states left on queue at the last report |
| `workers` | TLC's worker count, from the log's `Running ... with N worker(s) on ...` line (`tlc_record` and `collect.py` read it). 1 is a strict BFS. Absent when the log does not show it; a run backfilled from its job's commands file (`export TLC_WORKERS=1`) has `workers: 1` and says so in `workers_from` |
| `cap_s` | the time cap in seconds, when known (a backfilled TLC run stopped by its cap: its wall time, which can run a second over) |
| `generated` | TLC's states generated |
| `trace_states` | `VIOLATION`: the counterexample's length in states, initial state included |
| `violation`, `error` | the tool's message |
| `exit`, `wall_s`, `mode`, `source` | exit code, wall time, enumerator mode, `RUNREC` or `log` (parsed from an older log) |
| `profile` | enumerator: the Cargo profile the binary was built with, `release` (no LTO, 16 codegen units: fast builds) or `evidence` (thin LTO, 1 codegen unit; `PROFILE=evidence tools/enum-job.sh`) |
| enumerator extras | `transitions`, `rate` (states/s), `fp_collision_bound` (n²/2^129 for 128-bit fingerprints; 0 for exact runs), `per_level`; dsweep: `pending_records`, `peak_disk_bytes`, `disk_B_per_record`, `peak_ram_bytes`, `checkpoints`, ...; sample: `seed`, `walks`, `steps`, `violating_walks`, `digest`. Cite one as `[run JOB/RUN:field]` |

**Older logs** have no `RUNREC` lines. `collect.py` parses them:
- TLC output before `=== tlc NAME ...: exit RC, wall Ws` (or `=== live NAME ...`). Violation lines that name another run's instance are skipped; they come from a trace printed after that run.
- `### NAME [... Ns cap]: <summary>` lines.
- The enumerator's `SUMMARY ... states= levels= violations= complete=`, followed by `=== LABEL: exit ...`; the run name is LABEL with spaces turned to `_`.

Usage:

```
RUNREC_LOG_CMD='<command that prints a job log> {job}' tools/evidence/collect.py JOB [JOB ...]
tools/evidence/collect.py --log saved.log --job JOB        # from a saved log
tools/evidence/collect.py --print --log saved.log --job JOB  # to stdout, nothing written
```

Commit the record files. A log that can no longer be read gets no record: mark its numbers
`[unverified: log unavailable]` instead.

## 2. Citations

Put the citation right after the number it backs, in the same table cell or sentence:

| Run | Write | Checked |
|---|---|---|
| `COMPLETE` | `5,977 [run 3ce98eac/m_g_2x2]` | the number is exactly `states` |
| `COMPLETE`, with depth | `156,237 / 23 [run 069b15e6/x_2x1_r1]` | and the number after `/` is `depth` |
| `PARTIAL` | `≥ 61,233, partial (cap 180 s) [run 3ce98eac/m_g_4x1]` | this form; the number is `states`; the cap text names `cap_s` in s, min or h, within 5 s or 2 % |
| `VIOLATION` | `trace of 15 states [run c3c883d8/m1_client_no_epoch_fence]` | the number is `trace_states` (`depth` if no trace length) |
| any numeric field | `578,429 [run 069b15e6/x_2x1_r1:generated]` | exactly that field |

Bold (`**5,977** [run …]`) is fine. Not from a run: mark the table cell `[estimate]`,
`[derived]` or `[unverified: <reason>]`.

## 3. The check

```
tools/evidence/check.py [--runs docs/evidence/runs] [docs/evidence/*.md]
```

It fails on any of these:
- a cited run with no record;
- a `COMPLETE` run cited with a number other than `states`, or as a lower bound;
- a `PARTIAL` run cited without `≥ N, partial (cap …)`, with the wrong N, or with the wrong cap;
- a run of any other status cited;
- a count-like number in an evidence table with no citation and no exemption. Count-like means 1,000 or more, written `12,345` or `12345`, and not followed by a unit (s, min, h, %, MB, GiB, M, k, ×, ...). Cells in Job / Commit columns are skipped.
- a least depth backed by a run that is not a strict BFS (§4).

Prose is not scanned for uncited counts, but every citation in it is checked.

## 4. Least depths: one TLC worker, or the enumerator's BFS

TLC with `-workers auto` shares one queue between workers, so a worker can check a state of the
next level before the current level is done. Its counts are exact, but its **first violation
need not be a shortest one**: 6 rich-text mutant cells were 1 or 2 states deeper than the
shortest violation (`docs/evidence/g1-richtext-draft.md`, note 11), and so were 5 of 26 layers
mutants (`docs/evidence/g1-layers-draft.md`) and 3 of 16 map/list teeth (`docs/evidence/g1-lmm-tlc.md`). So:

- A **least depth** (a mutant's first violation depth, its shortest trace) must cite a TLC run
  with `workers: 1` (run it with `TLC_WORKERS=1`; e.g. `tools/quint/depth_w1.sh`) or an
  enumerator BFS run (`bfs`, `bfsm`, `cp`: its records have a `depth`). Reverse search,
  sweep-line and sampling have no BFS levels and do not count.
- A multi-worker trace is fine as a count or a comparison. Label it `-workers auto` (in its
  cell, its column header, or in the sentence before the citation).
- The check finds least-depth claims by their words: a section heading, a table header row or
  a prose line that says "least depth" (also "least violating / failing depth", "shortest
  trace / counterexample") makes every `VIOLATION` citation under it a least-depth citation.
  Write least-depth claims with those words, so the check sees them.

`tools/check.sh` (the repository's check flow) runs `check.py` and `test.sh`. To run the check
before every commit that touches the evidence:

```
ln -s ../../tools/evidence/pre-commit .git/hooks/pre-commit
```

(in a worktree, link it into the common git directory's `hooks/`).
