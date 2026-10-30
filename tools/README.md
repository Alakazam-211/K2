# tools/

Jobs for compute nodes: the kit's heavy tests ([Test jobs](#test-jobs): the full suite, the
32-participant soak, the live GNU parity), the undo spec's model check
(`docs/MODEL-CHECK.md`) and the fuzz targets ([Fuzzing](#fuzzing)). Nothing here is shipped or needed to build the kit.

**Build machines are the host's.** The jobs are self-contained shell scripts for any Linux
runner; nothing here names a machine. The examples submit them with one compute CLI
(`k2 compute run "$NODE" …`): set `NODE` (or `FUZZ_NODES` for the fuzz nightly) to your own
runner. Any Linux box with a checkout runs a job the same way (`sh tools/job.sh <commands
file>` at the repo root). Figures in the notes say "a build node" with the node's size where it matters.
`tools/check-private.sh` keeps internal host names out of the tree.

## Test jobs

**The default: the runner ships the commit (`--repo`).** Every part runs in a checkout of
the repo at a commit, and nothing travels in the environment:

```sh
k2 compute run <node> --repo <repo> [--sha REV] --needs rust@stable,cargo-nextest --cpus 4 \
  --mem 8G --timeout 40m --env PART=base-a1 --artifact 'junit/*/junit.xml' --detach \
  -- sh tools/job.sh tools/tests/full.sh
```

- **`tools/job.sh <commands file>`** is the harness: it sources the commands file at the repo
  root. The file runs each step as `timed LABEL CMD...`, which prints `=== LABEL: exit N,
  wall Ss`; the job exits non-zero if any step failed (the later steps still run). It prints
  a heartbeat every 4 minutes (a long step isn't stopped as stalled), the job's memory peak
  at the end, and stops any process the job left behind.
- **What `--repo` needs** (`k2 compute run --help`, `k2 study compute`, k2-node 0.46.6):
  - `<repo>` is the repo's folder or a worktree of it, **relative to the workspace folder**
    (an absolute path is refused, `repo_invalid`); run from inside a checkout or worktree and
    it is found for you.
  - **A commit**: the job checks out `HEAD` of that checkout (a worktree's own HEAD), or
    `--sha REV`, in a fresh tree on the node. The commit need not be pushed: an unpushed
    commit of a local branch or worktree travels. Uncommitted changes don't, unless
    `--dirty` (the diff and untracked, not-ignored files on top of `HEAD`): a dirty tree is
    not refused, just not shipped.
  - The whole tree travels, `vendor/yrs/src` included, so nothing is rebuilt from the
    public crate (the fallback's `K2.patch` step) and `tools/tests/full-paths.sh` isn't used.
  - On the node, git refuses to run in the checkout ("dubious ownership"): no job step uses
    git. The runner sets `CARGO_TARGET_DIR` to a warm target dir per repo and the CPU hints
    (`CARGO_BUILD_JOBS`, test threads) from `--cpus`; the harness keeps both.
  - Sibling repos a pack's path dependencies need go with `--with DIR[@SHA]` (committed
    only), laid out as in the workspace.
- **JUnit:** nextest's files (`.config/nextest.toml`) are copied to `junit/<profile>/junit.xml`
  in the checkout at the end (the warm target dir is outside it), so `--artifact
  'junit/*/junit.xml'` keeps them: the job's test list (`k2 compute get JOB --json`, `logs
  --failures`).
- **fmt and clippy** run as the `lint` part with `--needs rust@stable+clippy+rustfmt` (the
  component form needs k2-node 0.46.7 on the node; an older node refuses it at submit,
  `node_upgrade_required`). On an older node, submit with `--needs rust@stable`: the
  toolchain has neither and its rustup home is read-only (`rustup component add` fails), so
  the harness's `lint` function installs a job-local stable toolchain with both (rustup-init,
  minimal profile, under the job dir; about 15 s) for those steps only, and says so; test
  steps keep the node's.

### The fallback: env-chunk bundles

Only for a runner that can't ship a checkout. `test-bundle.sh` writes a self-contained job
for a node with no checkout (`--no-source`), for any part of the repo:

```sh
tools/test-bundle.sh [--rev REV] [--env NAME=VALUE]... <commands file> <out dir> <path>...
```

- **The hard check:** a runner caps a job at **64 environment pairs** (k2-node 0.46.6; more
  is refused, `env_too_many`) and 8 KiB a value. The bundler counts every pair the job will
  carry (the chunks, and each `--env` given to it: PART, SOAK_SECS, ...) and **refuses to
  write the job above 60** (`ENV_LIMIT`, a margin), with a message that gives the `--repo`
  command. Give every variable to the bundler's `--env`, never as an extra `--env` on the
  runner's command line, so the count is the job's (`env-args` carries them all).
- **What travels:** the listed paths (`tools/tests/full-paths.sh $PART`), from a clean `git
  archive` of `REV` (default `HEAD`), never the working tree: commit first; `tools/job.sh`
  always. When `Cargo.toml` travels and a workspace member doesn't, the job narrows the
  manifest's `members` line to the members shipped (its only change).
- **Over the limit** (with k2-yrs k2.6's larger engine patch every cargo part needs more than
  the 60 pairs): `INLINE=120000 tools/test-bundle.sh …` puts the base64 past the chunks
  inside `job.sh`, i.e. in the job's command argument (120,000 characters checked on a build
  node; Linux allows 128 KiB per argument).
- **`STRIP_DOCS=1`** (v0.9.7: D1 put `spec-undo` at 657,656 base64 characters, 145,656
  past the variables, over what one argument carries): every shipped crate's `src/` and
  `tests/` travel with their doc-comment lines (`///`, `//!`) **blanked, not deleted**, so
  every line number a panic or a test failure names is the repository's
  (`tools/fuzz/strip-comments.py --docs`, which refuses when such a line sits in a string
  literal). The code is unchanged; doctests are left behind, so use it only for a part that
  runs none (`spec-undo`, `conformance`, `base-c`), never `base-a1`.
- **How it travels:** an xz tarball, base64, in chunks of at most `CHUNK` characters
  (default 8000) passed as `BUNDLE_B64_0`, `BUNDLE_B64_1`, ...; the job joins them, checks
  the sha256 written into `job.sh`, unpacks, and runs `tools/job.sh`.
- **`vendor/yrs` travels without `src/`** (v0.9.3: the kit plus yrs is over a job's limit):
  the job downloads the public `yrs-0.28.0` crate, checks its sha256, applies
  `vendor/yrs/FORK.patch` and checks the result against `vendor/yrs/VENDORED`'s `src-sha256`
  before anything builds (the node needs `curl` and `patch`).

### Both

- **Run records:** the kit's runs print one `RUNREC` line each (`tools/evidence/README.md`);
  after a job, `RUNREC_LOG_CMD='k2 compute logs {job}' tools/evidence/collect.py JOB` writes
  `docs/evidence/runs/JOB.json`, and the evidence notes cite it.
- **Threads follow the job's CPU quota:** a node limits a job's CPUs with a cgroup quota, and
  `nproc` there counts the whole machine (16 in a `--cpus 4` job). The job sets
  `CARGO_BUILD_JOBS`, `RUST_TEST_THREADS` and `NEXTEST_TEST_THREADS` to the quota (a value
  already set wins) and prints its cgroup's memory peak at the end, to size `--mem`.

| Commands file | Fallback ships | Length | What |
|---|---|---|---|
| `tests/full.sh` | `tools/tests/full-paths.sh $PART` | ≈ 1–4 min a part (build included; [Build settings](#build-settings)) | the workspace's tests (debug; under nextest when the node has it) in ten parts, `--env PART=`: `base-a1` (the kit's unit tests and doctests; the `disk`, `if_version` and `blame` integration tests were here from exports P0, when base-a2 reached 117,788 of a job's 120,000 inline characters, until the v0.9.7 merge put base-a1 at 130,240: they are base-d's now) and `base-a2` (every other integration test but tier 0's, base-c's, base-d's and errors', `client_vectors` among them; on the kit without its unit-test modules): one part, `base-a`, until the v0.9.5 merge put it over a job's limit, `base-b` (tier 0's, promotion's, both parity harnesses and erasure; live_edit since v0.9.8), `base-c` (the soak's CI variant, `edit_paths`, `edit_cost`, history, presence, sessions, roster, app_view, overflow; v0.9.3), `base-d` (disk, `if_version`, blame, `blame_incremental` (v0.9.7), `blame_cost`, and `position_index` with its planted engine bugs; v0.9.7), `errors` (`error_ids`, `tools_session`, `pack_verbs`, the `mpc` example; v0.10), `conformance` and `spec-undo` (with its mutant tests; both `INLINE=120000` since v0.10, `STRIP_DOCS=1` since v0.9.7; `base-c` too since v0.9.7, when the K5 `run_tool` test put it 600 characters past 120,000 inline), `evidence` (`tools/evidence/`'s self-test and check of `docs/evidence/`; Python, no cargo), `wire-guard` (`tools/tests/wire-guard.sh`, v0.9.8: the `untrusted-wire` feature refused without `--cfg k2_mpc_untrusted_wire_ack`, in the repo and as a host builds; `docs/HOST-WIRE.md`). The whole workspace is over one job's limit (about 61 of 64 chunks a part) |
| `tests/soak.sh` | `tools/tests/full-paths.sh soak` | `SOAK_SECS` + build (default 10 min) | the 32-participant soak in release (`crates/base/tests/soak.rs`: progress every 30 s, a summary with peak memory), then its CI variant (`soak_short`), then the D1 replay regression (`soak_replay_seed_3_no_d1`: seed 3, 27,000 calls, no known D1 violation; about a minute) |
| `tests/gnu-parity.sh` | `scripts/gnu-parity.py crates/base/tests/golden` | < 1 min | the golden corpus's `gnu: same` cases against the real GNU tools (K-PAR-7); needs python3 and the GNU tools, no Rust |

A run, **always with an explicit `--mem`**. Run cargo with the host's `K2_*`/`K2SO_*`
variables unset (a subshell that runs only cargo); run the `k2` CLI with them **intact**:
they carry the caller's identity, and without them it falls back to another identity. From
inside the repo's checkout or worktree (else `--repo <its folder, relative to the
workspace>`):

```sh
# a part of the full suite (PART: base-a1 base-a2 base-b base-c errors conformance spec-undo
# wire-guard lint evidence)
k2 compute run <node> --needs rust@stable,cargo-nextest --cpus 4 --mem 8G --timeout 40m \
  --env PART=base-a1 --artifact 'junit/*/junit.xml' --detach -- sh tools/job.sh tools/tests/full.sh
# fmt and clippy (rust@stable on a node older than k2-node 0.46.7)
k2 compute run <node> --needs rust@stable+clippy+rustfmt --cpus 4 --mem 8G --timeout 40m \
  --env PART=lint --detach -- sh tools/job.sh tools/tests/full.sh
# the soak
k2 compute run <node> --needs rust@stable --cpus 4 --mem 8G --timeout 40m \
  --env SOAK_SECS=600 --detach -- sh tools/job.sh tools/tests/soak.sh
```

**A release soak runs `tools/tests/soak.sh` whole** (the command above: `soak_32`, `soak_short`
and the seed-3 replay), never `soak_32` alone by hand. v0.9.7 and v0.9.7.1's soaks ran
`soak_32` only, so the replay's stale expectation (its D1 violation, gone since v0.9.7) went
unseen until a later full run.

The GNU parity job is the same with `tools/tests/gnu-parity.sh` and `--needs python@3.12`.
`--needs cargo-nextest` has the node install nextest into its own toolchain home before the
job starts, cached for the next job (no `cargo install` in the job; 0.9.150 on 10-28;
Apache-2.0 OR MIT); without it the part runs under `cargo test`. The JUnit
artifacts become the job's test list (`k2 compute get JOB --json`, `logs --failures`; one
file a nextest profile: spec-undo's mutant tests run under `mutants`).

The fallback for a part (refused when it would carry more than 60 environment pairs):

```sh
INLINE=120000 tools/test-bundle.sh --env PART=base-a1 tools/tests/full.sh /tmp/base-a1 \
  $(tools/tests/full-paths.sh base-a1)
k2 compute run <node> --no-source --needs rust@stable,cargo-nextest --cpus 4 --mem 8G \
  --timeout 40m $(cat /tmp/base-a1/env-args) --detach -- sh -ec "$(cat /tmp/base-a1/job.sh)"
```

The licence check (`scripts/check-licences.sh`, `cargo metadata` only) runs locally.

## Build settings

The kit's own debug builds and node test runs (since the build-speed trial, 10-28):

- **`Cargo.toml`:** `[profile.dev] debug = "line-tables-only"` (backtraces keep file:line)
  and `[profile.dev.package."*"] opt-level = 1` (every dependency, the vendored engine
  among them; debug assertions and overflow checks stay on). Cargo applies a workspace's
  profiles only when it is the workspace being built, so a host that vendors the kit keeps
  its own. **Release is unchanged:** the soak, `edit_cost` and `edit_paths` test binaries and
  the kit's rlib are byte-identical with and without these lines (job `17eddcb6`), so the
  soak's and the measurements' numbers can't move.
- **cargo-nextest** for the suite's parts (`.config/nextest.toml`; `tools/tests/full.sh` falls
  back to `cargo test`). It runs each test in its own process, several binaries at once.
- **`.cargo/config.toml`** (v0.9.8): `--cfg k2_mpc_untrusted_wire_ack` in `rustflags` and
  `rustdocflags`, for builds started inside the repo: the kit's own tests enable
  `untrusted-wire`, which doesn't compile without it (K10, `docs/HOST-WIRE.md`). Every part's
  bundle ships the file. A `RUSTFLAGS` in the environment, even empty, replaces it.

**The trial.** `k2-mpc-base` v0.9.3, each part's node bundle, on a build node (`--cpus 4 --mem 10G`,
4 build jobs and test threads), median of 3. In each rep every configuration ran in turn
(rotated per rep): a fresh `target/`, the cold build (`cargo test --no-run`, its process
tree's peak RSS sampled every 0.5 s), `touch crates/base/src/session/live.rs` and the warm
rebuild, then the part's tests. The node was shared (load 6–19 on 12 CPUs), so single figures
are rough; the interleaving keeps the comparison fair. Jobs: base-a `d89d242f`, base-b
`ca3d567b`, base-c `37331b39`, conformance `437636f3`, spec-undo `0ed2f024` `2d30b374`
`3b3788a5` (one rep each).

| Configuration | Cold build (s) | Warm rebuild (s) | Suite (s) | Cold + suite (s) | `target/` (MB) | Build peak RSS (MB) |
|---|---|---|---|---|---|---|
| baseline | 111 | 61 | 387 | 498 | 5,013 | 1,213–1,261 |
| nextest | 112 | 59 | 345 | 457 | 5,014 | 1,240–1,256 |
| deps opt-level 1 | 244 | 63 | 238 | 482 | 5,634 | 1,126–1,297 |
| line-tables-only | 103 | 56 | 385 | 488 | 2,847 | 1,084–1,225 |
| **all three** | 183 | 47 | **201** | **384** | **2,941** | 1,126–1,207 |

Sums over the five cargo parts of each part's median; RSS is the range of the parts'
medians (the build's whole process tree; the largest single process 600–830 MB).

| Part (suite s, cold s) | baseline | all three |
|---|---|---|
| base-a | 61.5, 30.6 | 14.7, 37.0 |
| base-b | 41.7, 18.3 | 13.3, 30.9 |
| base-c | 26.2, 22.5 | 7.6, 41.7 |
| conformance | 16.6, 19.4 | 2.3, 40.2 |
| spec-undo | 240.8, 19.9 | 163.0, 33.0 |

- **spec-undo** was two long single tests in the trial: `bounded::tests::pid_symmetry_is_exact`
  (about 80 s) and `mutants::every_mutant_fails_its_rule` (about 130 s), which no runner can
  spread over threads, so nextest alone gained nothing there. Since then they are one test per
  bound and one per mutant (the mutant tests hold a lock, so under `cargo test` they take
  turns: the active mutant is process-wide). On a build node, `--cpus 4`, median of 3 under nextest:
  the mutant tests 85 s → 53 s (jobs `9bf7c12e` → `c1d41696`); the rest of the part 66 s →
  69 s, unchanged (its tests are CPU-bound at 4 threads now, not waiting on one test). The
  longest is now `the_oracle_passes_at_the_restart_bound` (about 50 s, one enumeration).
- **Same tests, same results:** every run of every configuration passed with the same
  counts: base-a 256 + 3 doctests (1 ignored), base-b 65, base-c 99 (3 ignored),
  conformance 96, spec-undo 42 + 2 mutants (1 ignored). No test failed in any
  configuration. nextest runs no doctests, so `full.sh` runs them with `cargo test --doc`.
- **Process state:** each test's temp dir carries the process id and a per-process counter;
  the process-wide state (the mutant switch, the counting allocators of `tier0_large` and
  `ephemeral_replicas`, thread-locals) is only more isolated under nextest.
- **Property tests:** case counts are set in code (256, 48, 3000 or `PROPTEST_CASES`) and
  seeds are proptest's (random per run, regressions replayed from `proptest-regressions/`)
  under either runner; neither runner changes them.
- **Validation** of the adopted scripts, every part at `--mem 8G` under nextest, JUnit kept:
  base-a `a48ac20a` (22 s; peak 1.5 GiB), base-b `30a6c39b`, base-c `2d649672`,
  conformance `c482dff9`, spec-undo `32082436` (82 + 133 s; peak 2.2 GiB), evidence `7e3c796c`.
  Again on main with the errors part, at `--mem 8G`: base-a `c42a76cf` (243 + 3 doctests),
  base-b `5e7bf323`, base-c `64429c2f`, errors `f155264c` (30 and the example), conformance
  `65187f92` (`INLINE`), spec-undo `88446691` (42 + 2, both JUnit files kept), evidence
  `09a0b3dc`; every part passed, peaks 1.0–2.2 GiB.

## The undo model check

| Script | What the job does |
|---|---|
| `undo/job.sh quint <commands>` | a `--repo` job (the default): builds `undo_enum` and `undo_itf_replay` (release, mutants on), installs Quint 0.33 (npm; TLC comes with Apalache, fetched by the first `quint verify`), and runs `<commands>` in `spec/quint/` |
| `undo/job.sh enum <commands>` | builds the same binaries and runs `<commands>` at the repo root (no Quint, no Java) |
| `undo-quint-job.sh`, `undo-enum-job.sh` `[--env N=V]... <commands> <out dir> [extra files]` | the fallback: an env-chunk bundle (`undo-bundle.sh`) whose job unpacks and runs `undo/job.sh`; refused above 60 environment pairs |

**The Quint export guard** (`tools/quint/`, v0.9.3): a guard against a Quint→TLA+ export
issue, where the exported TLA+ can say something other than the Quint source. Every Quint job
runs it in `spec/quint/` **before anything else touches the model**, the TLC warm-up included:
`lint.sh undo.qnt` (a source lint: `qnt_lint.py`, Quint's parser, no Java), then `lint.sh
--export` on an instance of `undo.qnt` (`export_check.py`: the model compiled to TLA+, read
back and compared with the Quint IR, definition by definition). A finding (exit 1) or a tool
error (exit 2) **stops the job**; `tools/undo/tlc.sh` refuses to run TLC unless the check
passed. Needs Python 3 on the node (stdlib only; `--needs python@3.12`). The three files are
self-contained (POSIX sh, Python 3 stdlib) and kept as received: update them as a set.

Commands files (in `tools/undo/`):

| File | Length | What |
|---|---|---|
| `smoke.sh` | ≤ 15 min | Quint typecheck; tiny simulator runs exported as ITF and replayed into `spec_undo::model` (one bound with a restart); TLC's exact count at a tiny bound against the enumerator's; every mutant on TLC and on the enumerator (the restart mutant at its own bound) |
| `counts.sh` | ≤ 15 min per `PART` (`a`, `b`, `m`; v1.8: all three no longer fit one job) | TLC's exact distinct-state counts at the small bounds (one with a restart, one where restorations meet) against `undo_enum bfs`; ITF replay of 1,000 traces at 2 × 1 + 1 undo + 1 revert and 1,000 at 2 × 1 + 2 undos + 1 revert; every mutant on TLC (one worker) and on the enumerator |
| `diff.sh` | ≤ 10 min | the exact state-set comparison: TLC's `-dump` against `undo_enum bfs --dump`, by `statediff.py` (it found the one count mismatch, `docs/MODEL-CHECK.md`) |
| `g1-smoke.sh` | ≤ 10 min | the literal G1 bound, sampled: Quint simulator traces replayed, random walks, mutant walks |
| `g1.sh` | about 1 h (`LONG=1`: hours more; see `docs/MODEL-CHECK.md`) | exhaustive at the G1-shaped bounds that fit, with checkpoints; random walks at the literal G1 bound (and with one restart), per-class coverage in the summary; `PART=walks` is over a 2 h job on 8 threads, so it also runs as `walks-main`, `walks-mutants` and `walks-restart`; `walks-bias` runs walks weighted toward `redo_after_partial` (`undo_enum walk --bias`), with every mutant again. **Launched only after review**, never from a smoke run |

`inst` and `itfinst` take an optional 11th argument, `RESTARTS` (default 0); `undo_enum` and
`undo_itf_replay` take `--restarts N`.

### Running a job

From inside the repo's checkout or worktree (else `--repo <its folder, relative to the
workspace>`; [Test jobs](#test-jobs) says what `--repo` needs), with the `k2` CLI's variables
intact (unset `K2_*`/`K2SO_*` only around cargo, never around `k2`). **Every run passes an
explicit `--mem`** (a job without one gets a small default and is stopped above it):

```sh
k2 compute run "$NODE" --needs node@22,java@21,rust@stable,python@3.12 --mem 12G \
  --timeout 15m --env CARGO_BUILD_JOBS=6 --wait --max-wait 9m \
  -- sh tools/undo/job.sh quint tools/undo/smoke.sh
```

An enumerator-only job is the same with `enum` and `--needs rust@stable`. A file a commands
file reads that isn't committed (a checkpoint, `g1.sh`) travels with `--dirty` (untracked
files on top of the commit). The fallback: `tools/undo-quint-job.sh [--env N=V]...
tools/undo/smoke.sh /tmp/undo-smoke`, then `k2 compute run "$NODE" --no-source ...
$(cat /tmp/undo-smoke/env-args) -- sh -ec "$(cat /tmp/undo-smoke/job.sh)"`.
Long runs (`g1.sh`) may add `--tier batch` (idle-time work) and `--detach`; see
`docs/MODEL-CHECK.md` for the exact command and the runtime estimate. Never `--priority`.

### Apalache and TLC in a job

`tlc.sh` (sourced after the warm-up `quint verify`, which fetches Apalache) starts **one
Apalache server** for the job (`APA_HEAP`, default 6g) and every `quint compile` reuses it.
On the v1.7 model a compile that started its own server failed at random (`error:` with
nothing after it); with one server about one compile in six still fails cold, so `tlc`
retries a failed compile up to 3 times and logs each failure. TLC runs with `TLC_HEAP`
(default 3g), so both fit a `--mem 12G` job. The warm-up itself may report exit 1: it is
only there to fetch Apalache.

**Each job has its own Apalache port.** Jobs on one node share its network, and on Quint's
default (`localhost:8822`) one job's compile could reach another job's server and fail with
"Received RST_STREAM with code 0" when that job stopped it. The prelude picks a port from the
job shell's PID (checked free by a listen probe, else stepped; `QJ_APA_PORT` overrides), puts a
`quint` shim first on `PATH` that adds `--server-endpoint` to every compile and verify, and
starts its own servers on that port. **At exit** the job lists the processes left in its
session (`=== job epilogue: ...`; an Apalache server is normal there) and stops them; the
test jobs (`tools/job.sh`) do the same.

### Long runs and the stall timeout

A node stops a job that prints **nothing** for its stall timeout (`--stall-timeout`, 30 min by
default) and reports it as stalled. A pre-v1.7 G1 walks job was stopped that way: `undo_enum
walk` printed only its header until the end. Every long mode now prints a progress line at
least every `--progress` seconds (default 60): `dfs` (states, seeds, rate, memory), `walk`
(walks and steps done, violations so far, the per-class counts so far). So:

- keep the progress output on in long runs (`--progress 60` or less), and don't pipe a long
  command through anything that buffers it to the end (`| tail`, `| sort`); `timed` doesn't;
- give `--stall-timeout` well above the progress interval only if a step is known to be
  silent for long (a TLC run on a big bound prints nothing between its own reports);
- **never rely on turning stall detection off**: a job that hangs then holds a node for its
  whole timeout. A silent long step is a bug in the job: add progress output instead.

### The fallback: sources in environment variables

Only for a runner that can't ship a checkout: the job runs with `--no-source` and the sources
travel in it as a base64 gzip tarball, split into chunks of at most `CHUNK` characters
(default 8000; a runner caps one value at 8 KiB) passed as `UNDO_B64_0`, `UNDO_B64_1`, …
(`env-args`, one `--env` argument per chunk, after the `--env` pairs given to the bundler).
The job joins them, checks the tarball's sha256 (written into `job.sh`), unpacks it, and runs
`tools/undo/job.sh`. **A runner caps a job at 64 environment pairs**: the bundler counts the
chunks and every `--env` given to it and refuses above 60 (`ENV_LIMIT`), pointing to
`--repo`.

**What travels** (about 145 KB gzip, 25 chunks with Quint, at v0.9.6): the workspace
manifest and lock, `crates/spec-undo` (sources, and of its tests only `mutants.rs`, which the
manifest declares), `spec/quint` (Quint jobs), `tools/undo`, the commands file, and of
`crates/base` **only `src/text`, verbatim**. The oracle calls nothing of the kit but
`text::diff` and `text::lines`, and that module uses only `std` and `thiserror`, so the job
builds it as a minimal `k2-mpc-base` (`lib.rs` = `pub mod text;`, with the real manifest's
version, `thiserror` requirement and `conformance` feature). The diff the job runs is the
kit's own code, byte for byte; the job skips the kit's other dependencies.

In a `--repo` job the whole tree travels and the real kit is built: the same `text` module,
with the kit's other dependencies.

## Fuzzing

`fuzz/` is a cargo-fuzz crate (libFuzzer, nightly Rust, Linux): its own workspace,
excluded from the kit's, so a normal build never pulls it. It reaches the store's private
record decoders through the kit's test-only feature `fuzzing` (`k2_mpc_base::fuzzing`, off
by default, like `differential`). Every target asserts properties: **a violated property is
a finding, like a panic.** Targets are built with debug assertions on (`-a`), so the kit's
own `debug_assert!`s count too.

| Target | Input | Properties |
|---|---|---|
| `text_merge` | base, ours, theirs: mostly a small vocabulary (repeats, `\n`, multi-byte, combining, empty), sometimes any UTF-8 | `diff::hunks` valid, canonical (no touching or no-op hunks), pure, and gives `new`; `align_insertions` keeps that. `merge3`'s identities; symmetric up to O5's append order; O5 only adds clean merges; a clean merge is exactly both sides' changes applied to the base in base order (insertions before replacements at one point, theirs' first); conflicts disjoint, in order and well formed; `merge3_hunks` on the diffs' hunks is `merge3_window` |
| `store_records` | a record kind, then bytes | every store record decoder (doc key, sync state, meta, log entry, approval, version and its head, snapshot, change, revert record, carry and taint, provenance row, revert index and stats, the blob records, the file stat) refuses with `Corrupt` or decodes to a record whose encoding is a fixpoint; canonical records re-encode to the input. Allocation is bounded by `-malloc_limit_mb` (256). Engine bytes inside a record are carried, **never decoded**: no fuzzed byte reaches the engine |
| `outside_edit` | a file's lines, then up to 24 steps | a file-backed session under two participants' edits, changes that move no byte (edit + undo, delete + retype, rewrite and back, restore, revert), `write --base`, ticks, flushes, restarts (a crash) and outside edits: an outside edit made while the doc is the file is the doc right after `file_changed` and the file after the write-back; a flush makes the file the doc and the session clean; a restart keeps the doc; `write --base` is `merge3` (applied with its text, refused exactly when it conflicts); and, once per process, its fixed regressions (`outside::regressions`, readable inputs: an outside delete and retype over unwritten edits never yields a line twice) |

**One run:** `tools/fuzz/bundle.sh <repo> <rev> <out dir> <corpus root> <secs> <target>...`
writes the job. **The code travels with the runner** (`FUZZ_SHIP=repo`, the default: the
job is submitted from inside `<repo>`, a repo or worktree in the workspace, with `--sha
<rev>`, unpushed commits too; [Test jobs](#test-jobs) says what that needs), and only the
private corpus travels in the job's environment, cut to fit 60 pairs (a runner caps a job at
64). `FUZZ_SHIP=env` is the fallback: the code travels in the job too (full-line comments
blanked, line numbers kept: `strip-comments.py` refuses when that would change a string),
and the bundler refuses when the code alone is over 60 pairs. The job runs every target at
once for `<secs>`, then minimizes any crash (`cargo fuzz tmin`, `cargo fuzz fmt` renders it)
and the corpus (`cmin`), and keeps `out/<target>.tgz`:

```sh
FLAGS="outside_edit=-len_control=0" MALLOC="store_records=256" \
  tools/fuzz/bundle.sh . HEAD /tmp/fz "$FUZZ_HOME" 1800 text_merge store_records outside_edit
(cd "$(cat /tmp/fz/run-from)" && k2 compute run <node> $(cat /tmp/fz/sha-args) \
  --needs rust@nightly --artifact 'out/*.tgz' --cpus 3 --mem 8G --timeout 60m \
  $(cat /tmp/fz/env-args) --detach -- sh -ec "$(cat /tmp/fz/job.sh)")
k2 compute fetch <job> --out /tmp/fz-out
```

**A night:** `tools/fuzz/nightly.sh` runs the plan in `$FUZZ_HOME/plan` (one job per line:
repo | rev | node or `auto` (the allowed node in `$FUZZ_NODES` that is free now), targets, `|` between fields), waits, merges each target's corpus and crashes into
`$FUZZ_HOME/<target>/` and writes `$FUZZ_HOME/runs/<stamp>/report.txt`; `--dry-run` only
bundles. `FUZZ_WINDOW` (e.g. `01-05`) keeps a run inside its hours: nothing is submitted outside them and no job starts too late to end in them; `FUZZ_TIMEOUT`, `FUZZ_MAX_JOBS` and `FUZZ_SUMMARY_DIR` cap and record a night. Nothing here schedules it: whoever runs it picks the night.

**The corpus and crashes are private:** they live in `$FUZZ_HOME`, outside every repo, and
are never committed. A minimized crash becomes a plain regression test here only after its
fix, written as a readable input.
