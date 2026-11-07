# tools/

Jobs for compute nodes: the kit's heavy tests ([Test jobs](#test-jobs): the full suite, the
32-participant soak, the live GNU parity), the undo spec's model check
(`docs/MODEL-CHECK.md`).
Nothing here is shipped or needed to build the kit.

**Build machines are the host's.** The jobs are self-contained shell scripts for any Linux
runner; nothing here names a machine. The examples submit them with one compute CLI
(`k2 compute run "$NODE" …`): set `NODE` to your own
runner. Any Linux box with a checkout runs a job the same way (`sh tools/job.sh <commands
file>` at the repo root). Figures in the notes say "a build node" with the node's size where it matters.
`tools/check-private.sh` keeps internal host names out of the tree.
`tools/check-windows-names.sh` fails on any path Windows can't check out: a reserved name, a
refused or control character, a trailing dot or space, or a case collision, in every path component,
or a whole path over 200 bytes.
`tools/test-windows-names.sh` is its self-test.
`tools/oss-check.sh` (K-OSS-3) fails unless the repo carries its licence: `LICENSE-MIT` and
`LICENSE-APACHE` at the root, `license = "MIT OR Apache-2.0"` and `publish = false` on every
crate of ours, the same licence on every `package.json` of ours (and its lockfile's root entry),
and the client's copies of both licence files byte-identical to the root's.
`tools/test-oss-check.sh` plants each failure. Both run in `scripts/check.sh` and the evidence
part.

**Build machines are the host's.** The jobs are self-contained shell scripts for any Linux
runner; nothing here names a machine. The examples submit them with one compute CLI
(`k2 compute run "$NODE" …`): set `NODE` to your own runner, or run `sh -ec "$(cat <out
dir>/job.sh)"` with the `env-args` variables set on any Linux box. Figures in the notes say "a
build node" with the node's size where it matters. `tools/check-private.sh` keeps internal
host names out of the tree.

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
  at the end, and stops any process the job left behind. A commands file that ran no step
  fails the job, and a step that would run nothing (a count of 0) is skipped and says so, or
  fails, never `exit 0`. Every count or size a commands file reads from the environment is
  printed near the top, defaults included (`settings NAME=DEFAULT...`, as `=== settings:
  NAME=VALUE ...`): a job's record shows env names, not values.
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
  (`tools/strip-comments.py --docs`, which refuses when such a line sits in a string
  literal). The code is unchanged; doctests are left behind, so use it only for a part that
  runs none (`spec-undo`, `conformance`, `base-c`), never `base-a1`.
- **How it travels:** an xz tarball, base64, in chunks of at most `CHUNK` characters
  (default 8000) passed as `BUNDLE_B64_0`, `BUNDLE_B64_1`, ...; the job joins them, checks
  the sha256 written into `job.sh`, unpacks, and runs `tools/job.sh`.
- **`vendor/yrs` travels without `src/`** (v0.9.3: the kit plus yrs is over a job's limit):
  the job downloads the public `yrs-0.28.0` crate, checks its sha256, applies
  `vendor/yrs/FORK.patch` and checks the result against `vendor/yrs/VENDORED`'s `src-sha256`
  before anything builds (the node needs `curl` and `patch`).

### The gate: before any round of evidence

**Every round goes in only after one green job at its exact head**, whatever else ran on
earlier commits: the `lint` part (`fmt --check` and clippy, `-D warnings`), then `cargo nextest
run -p k2-mpc-base --lib --tests`, then `cargo nextest run -p k2-mpc-conformance -p
k2-mpc-spec-undo`. About five minutes on a warm node. A property or a unit test failing in a round's first part costs the whole round; this
is what it costs to know first. A later commit, even tests or formatting only, means a new
gate.

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
| `n1/n1.sh <tag> <dir>` | `old` (the tag's kit) and `n1` (`n1/consumer` + two stores this build wrote) | < 2 min | store format 6's N−1 check (K-FMT-10 item 22): the previous release opens, reads, writes, reverts and rewrites a store this build used for text docs only, and refuses one holding a grid doc with one line (`N-1 OK`) |
| `n1/n1.sh layout-refs <tag> <dir>`, then `node-refs <dir>` | `old` (the tag's kit) and `refs` (`n1/consumer-refs` + a store this build wrote) | < 3 min | the blob references' N−1 check: the previous release's gc keeps a blob this build took a reference on (`HELD`), collects one named only by a value it wrote (`LIMIT`, the documented limit), refuses a rewrite and leaves rows on a delete; this build's open gives those back and a load takes a hold on what's still there (`N-1 REFS OK`) |

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

### Client jobs

The JS client is a library with no DOM (since v0.10.2): its whole suite runs in Node, with no
browser tests. **The client gate runs in a full checkout** (any runner with the kit), so the
shared-vector test also reads the kit's copy of the vectors:

```sh
k2 compute run "$NODE" --needs node@22 --cpus 4 --mem 8G --detach -- sh -ec 'cd client &&
  npm ci && node scripts/check-licences.mjs && npx tsc --noEmit && npx vitest run'
```

`tools/client-ship.sh <kit-repo> <kit-sha> <out-dir>` writes a fresh one-commit repo holding
only `client/` at that commit, checks that nothing of the kit came with it (no path outside
`client/`, no crates, vendored engine, Rust or Cargo files, no history), and prints `ship <sha>
<- kit <sha>`. Use it for a client-only job on a runner that must not get kit source (a second
run of the suite, a seed scan): ship that repo (`--repo`), and quote the proof line with the
job id. Without the kit beside it, the shared-vector test checks the client's own copy only.

## The public cut

A host pulls the kit's release tags (squashed, by git subtree) into a **public** repo, so the
tag it pulls must carry no engine source that isn't public yet. Each such release gets a
**public cut**: on a release branch, the pre-cut release commit, then one more commit,
`release: public cut`, made by `public-cut.sh`. The tag goes on the cut. **The cut is never
merged into main:** main merges the pre-cut commit (`tag^`), which keeps everything.

| Script | What it does |
|---|---|
| `public-cut.sh [--no-commit] [--lock FILE] [--trailer TEXT]...` | Run at the root of a clean checkout of the release branch (never main): removes every `vendor/mpce*` but the allowlist's, the crates the allowlist drops (the conformance suite), the engine-only features (`native`, and every feature derived from removed ones, to a fixed point), engine dependencies from every dependency table, the trees in `DROPPED_TREES` (tooling no host builds or runs, and the evidence run records, `docs/evidence/runs`: main keeps and checks them), the code under cfgs on removed features, the tooling's parts for them, what `public-cut: drop` marker lines enclose in a Markdown file or a `Cargo.toml`, and the lockfile's removed packages (removals only); then runs `check-public-cut.sh` and commits. Idempotent: on a cut tree it changes nothing and commits nothing. `--no-commit` is a dry run (allowed detached and on a dirty tree). Its header has the rules; a case it can't decide (a feature mixing engine parts with an optional dependency, an unknown cfg form) stops it, to decide by hand |
| `check-public-cut.sh [ROOT]` | Fails closed (exit 1, naming every offending path or entry) unless the tree may be a public cut: both licence files; no engine path, manifest entry, lock entry or engine identifier outside `PUBLIC_ENGINE_CRATES`; no engine-only feature declared, requested or left enabled; every cfg names a declared feature; no dropped crate or engine feature in the tooling; every pinned regression present; no file under `DROPPED_TREES` and no reference to one (`DROPPED_REFS`, in any kept text file but a CHANGELOG, `vendor/` or the two cut scripts); no line added since the previous host release uses a term in `TEXT_TERMS`, unless reviewed (below); `check-private.sh` clean. The allowlist (`PUBLIC_ENGINE_CRATES`, `NATIVE_FEATURES`, `DROPPED_CRATES`, `LICENCE_FILES`, `DROPPED_TREES`, `DROPPED_REFS`, `TEXT_TERMS`) is at its top, the one place to change it; `--print-config` prints it |
| `public-text-base` | The text rule's base: the previous host release's pre-cut commit (`tag^`), one full sha. Missing, not tracked or not an ancestor of HEAD blocks the tag. A job's checkout has the history and no tags, so a sha it is |
| `public-text-exceptions.txt` | The lines the text rule found and a person reviewed and kept, one `PATH: TEXT` each (the whole added line, trimmed). An entry covers that exact text in that file, so a changed line or another line of the file still blocks, and a moved one needs no update. Line numbers would go stale with each edit above them, and a hash would hide what was reviewed. An entry matching no added line blocks too, so none outlives its base |
| `public-cut-regressions.txt` | The pinned regression tests of the host's earlier tags, which every cut must still carry (a hotfix line is cherry-picked, not an ancestor, so they are checked by name). One per line: `NAME` (a Rust `fn NAME` with a test attribute, not `#[ignore]`d) or `PATH IDENT` (the word in that file). **At each hotfix tag, append its regression tests** under a comment naming the tag and the fix |
| `tests/public-cut-gate.sh` | The cut's gate: a commands file for `job.sh`, run at the cut commit, one part per job (`--env PART=`): `full.sh`'s parts on the cut (`lint`, `base-a1` to `base-d`, `errors`, `spec-undo`, `wire-guard`, `evidence` with oss), `licence`, `lockproof` (cargo's own resolution leaves the pruned lock unchanged), `checker` (the cut is clean; planted copies of it are blocked), `checker-precut` (the pre-cut commit is blocked; the script on planted copies drops derived features and stops on a mixed one) and `reproduce` (the script run on the node makes exactly the cut's tree: the node's awk gates, other awks are informational). Planted and reproduced trees are `git archive` copies beside the checkout, never a worktree of its repository. Each part ends with `=== RESULT <part>: GREEN` or `RED` |
| `tests/public-cut-dryrun.sh` | A public cut's dry run at a pre-cut commit, for `job.sh`: `public-cut.sh --no-commit`, then `check-public-cut.sh` on the staged tree, in a shared clone; each step's failure is the job's exit (`=== RESULT dry run: GREEN` or `RED`) |

The procedure for a release:

```sh
# on the release branch, at the pre-cut release commit (the one main will merge)
sh tools/public-cut.sh --trailer '...'          # makes `release: public cut`; a `text:` finding stops it
sh tools/public-cut.sh                          # again: "nothing to cut" (idempotent)
# the gate, every part, at the cut commit (a --repo job each; Test jobs above)
k2 compute run <node> --repo <repo> --sha <cut> --needs rust@stable+clippy+rustfmt,cargo-nextest \
  --cpus 4 --mem 16G --env PART=reproduce -- sh tools/job.sh tools/tests/public-cut-gate.sh
```

Then tag the cut, and merge `tag^` (never the tag) into main. Then, in a commit on main, write
`tag^`'s full sha in `tools/public-text-base` and clear the entries of
`tools/public-text-exceptions.txt`: the next release's text rule scans from there. A release on an
older line does the same on that line, with the `tag^` it starts from. Forgetting costs a longer
scan, not a missed line.

**The text rule** scans every tracked text file's added lines on the cut's tree: `.md`, code
comments, test names and messages, `tools/` and the vendored trees that stay in the cut,
which reach the public repo like the rest. Files the cut removes are out of its reach. Hits are
expected, as the kit's own changes are described plainly in the CHANGELOG: read each one, and
either reword it or add it to the exceptions, in the pre-cut commit (`git reset --hard` drops the
stopped cut's staged edits), then cut again. Files git treats as binary aren't scanned. The rule
reads prose, not code: in `.rs` and `.ts` files the tokens in `TEXT_CODE_TOKENS` (a macro call, a
path, a member, a std name) are skipped, outside comments and string literals on that line, so a
macro's message is still read. Test names and plain words in code still count, and nothing but
those tokens is skipped, so a line misread by this one-line filter is only scanned more. The gate's
`checker` part plants a copy for each case, each asserting its exact finding; a gate copy with its
own `git init` reads the history through an alternate, so the rule runs on it too.

## Timers check

`tools/timers-check/` (crate `k2-mpc-timers-check`, its own workspace, never a member of the
kit's) builds against the kit with no features, as a host builds it. It checks that the
production timers (`docs/SESSIONS.md`, "Timers") are there, off by default and not built with
the test-only hooks, and that once switched on they count a session's edits and join. The
release run prints a `RUNREC` line with what a scope costs on the node (`calibrate`, off and
on) and the store commit's p50.

```sh
k2 compute run "$NODE" --repo . --sha <rev> --needs rust@stable --cpus 4 --mem 8G \
  --timeout 40m -- sh tools/job.sh tools/timers-check/job.sh
```

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
runs it in `spec/quint/` **before anything else touches the model**, any TLC run included:
`lint.sh undo.qnt` (a source lint: `qnt_lint.py`, Quint's parser, no Java), then `lint.sh
--export` on an instance of `undo.qnt` (`export_check.py`: the model compiled to TLA+, read
back and compared with the Quint IR, definition by definition). A finding (exit 1) or a tool
error (exit 2) **stops the job**; `tools/undo/tlc.sh` refuses to run TLC unless the check
passed. Needs Python 3 on the node (stdlib only; `--needs python@3.12`). The three files are
self-contained (POSIX sh, Python 3 stdlib) and kept as received: update them as a set.

**The Apalache servers' JVM options** (`tools/quint-jvm/`): Apalache reads a request's
configuration, which carries the whole Quint IR as one JSON string, with Jackson's default
limit of 20,000,000 characters. The export check's instance of the undo model is just past it
since the purge revision (20,001,757 bytes; 19,992,163 before). `undo/job.sh` builds a small
Java agent (`JacksonLimits.java`, needs a JDK: `java@21` has one) that raises the limit, and
starts both its servers with it (`APA_JAVA_OPTS`): the export check's and `undo/tlc.sh`'s. The
guard itself (`tools/quint/`) is unchanged, and the TLA+ it checks is the same with and
without the agent. **The agent is required** (2026-11-04, `tools/quint-jvm/require.sh`): a job
without it stops before its first Apalache server, in one line naming the 20,000,000 limit
and the model's IR size (`tools/quint-jvm/README.md`).

Commands files (in `tools/undo/`):

| File | Length | What |
|---|---|---|
| `smoke.sh` | ≤ 15 min | Quint typecheck; tiny simulator runs exported as ITF and replayed into `spec_undo::model` (one bound with a restart), and the ITF regression traces (`undo/itf-regress/`, each must replay clean); TLC's exact count at a tiny bound against the enumerator's; every mutant on TLC and on the enumerator (the restart mutant at its own bound) |
| `counts.sh` | ≤ 15 min per `PART` (`a`, `b`, `m`; v1.8: all three don't fit one job; `m` about 18 min since 2026-11-04) | TLC's exact distinct-state counts at the small bounds (one with a restart, one where restorations meet) against `undo_enum bfs`; ITF replay of 1,000 traces at 2 × 1 + 1 undo + 1 revert and 1,000 at 2 × 1 + 2 undos + 1 revert, and the ITF regression traces (`PART=a`); every mutant on TLC (one worker) and on the enumerator, the audit's four (2026-11-04) with each counterexample printed (`TLC_TRACE`), and the independent checks `O5doc`, `U17doc` alone under the mutants they restate (`PART=m`: about 18 min since then, 8 of them TLC's one-worker run of `i1-per-request` to its depth 5) |
| `debug.sh` (an `enum` job) | about 12 min on 4 threads | **debug assertions on**: `undo_enum` rebuilt with `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` in its own target dir (`target-dbg`; the release binaries every other file uses are built without them, so the oracle's own `debug_assert!`s never run there), then the oracle of record at the seven exact bounds (the counts must be `counts.sh`'s, no rule violated; a failed assertion is the enumerator's abort pseudo-rule) and random walks at the literal G1 bound with and without a restart (`WALKS`, default 100,000 each; `THREADS`, default 4). Part of every release's model suite since 2026-11-04 |
| `diff.sh` | ≤ 10 min | the exact state-set comparison: TLC's `-dump` against `undo_enum bfs --dump`, by `statediff.py` (it found the one count mismatch, `docs/MODEL-CHECK.md`) |
| `g1-smoke.sh` | ≤ 10 min | the literal G1 bound, sampled: Quint simulator traces replayed, random walks, mutant walks |
| `g1.sh` | about 1 h (`LONG=1`: hours more; see `docs/MODEL-CHECK.md`) | exhaustive at the G1-shaped bounds that fit, with checkpoints; random walks at the literal G1 bound (and with one restart), per-class coverage in the summary; `PART=walks` is over a 2 h job on 8 threads, so it also runs as `walks-main`, `walks-mutants` and `walks-restart`; `walks-bias` runs walks weighted toward `redo_after_partial` (`undo_enum walk --bias`), with every mutant again. **Launched only after review**, never from a smoke run |

`inst` and `itfinst` take an optional 11th argument, `RESTARTS` (default 0); `undo_enum` and
`undo_itf_replay` take `--restarts N`.

**Row verdicts** (`undo/expect.sh`, sourced by `undo/job.sh`): each commands file declares,
before it runs anything, every row (TLC run, `E`, `R`) it must end with and its verdict
(`expect KEY COMPLETE|SAMPLED|VIOLATION [RULE]`, `expect_mutants tlc|enum|walk`). Each row is
checked as it ends (`=== row KEY: ... as expected`), and the job exits 1 (`=== EXPECT: RED`,
with each problem) when a declared row didn't run, a row ended with another verdict or at
another rule, a row isn't declared, a tool printed no `RUNREC`, a TLC compile failed, or a
`timed` step that ran no row exited non-zero. A new mutant goes into `UNDO_MUTANT_RULES`
there with its rule. `expect_mutants` prints its variant (`=== expect: variant=walk ...`) and
refuses an unknown one, or an argument that isn't `-` and a mutant there; `expect.sh` is sourced
once a job (by `job.sh`), and a commands file that sources it again is refused, its rows kept.
Each refusal is one of the job's problems: the job ends `=== EXPECT: RED`, exit 1.

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

`tlc.sh` (sourced after `need_apalache`, which stops the job unless the export check's compile
fetched Apalache) starts **one
Apalache server** for the job (`APA_HEAP`, default 6g) and every `quint compile` reuses it.
On the v1.7 model a compile that started its own server failed at random (`error:` with
nothing after it); with one server about one compile in six still fails cold, so `tlc`
retries a failed compile up to 3 times and logs each failure. TLC runs with `TLC_HEAP`
(default 3g), so both fit a `--mem 12G` job. The export check retries a tool error the same
way (up to 3 attempts, each logged as the known random compile failure on a cold server); a
finding is never retried. Until v0.10.0's follow-up a warm-up `quint verify` on quint's own
server ran before `tlc.sh`: it checked nothing, and since v1.9 it exited 1 on every job
(`error: error` after SanyParser; that server has 4 GiB and no Jackson limits agent). It is
gone: `need_apalache` checks for what it was there to fetch.

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


## Performance harness

`tools/perf/` (crate `k2-mpc-perf`, its own workspace, never a member of the kit's): the kit's
paths timed at 10k, 100k and 1M versions, p50/p95/p99 per path, two builds paired in one job.

- **The builds.** The plain build depends on the kit with **no features**, as a host builds
  it: every row but one runs there. The kit's dev-dependency features (`differential`,
  `conformance`) never reach it (`differential` changes hot paths: its profile scopes, the
  engine's edit-path switch and the planted-fault checks). The `nosync` build adds
  `conformance` for two things only: the `edit_nosync` row (the store's test-only `unsynced`
  switch, redb's `Durability::None`) and the live-run count (`Session::char_blame`). It
  refuses every other row. Both sides of a pair build with the same harness source, the
  same flags and their own tree's `Cargo.lock`, and every record names its build and its
  features.
- **Fixtures** (generated in the job, never committed): `grow` grows one Markdown doc through
  the public session API with `perf_kit.rs`'s seeded mix (4 people, 4 agents; edits, tools,
  `edit_with`, undo/redo, reads, a tick every 25 calls and a write-back every 250). It copies
  the store and file at each size. The text stays at about 200 lines, so only the history
  grows. The large-doc fixture (`--lines 39000 --line-len 80`, about 3 MiB, under Markdown's
  4 MiB live limit, at 10k versions) carries the content-scaled rows.
- **Rows** (`--rows history`): `edit`, `edit_with`, `tool_replace`, `tool_sed`,
  `doc_edit_as` (the engine doc alone, `Doc::edit_as`, loaded from the fixture's store),
  `undo`, `revert` (by root), `blame_line`, `blame_doc`, `cat`, `events_500`, `store_probe`,
  `write_base` (3 others' edits after the read), then `flush`, `tick`, `ingest` (an outside
  save, `file_changed`) and `notes` (`take_notes` by the replaced line's author), then the
  cold rows `open`, `join` and `first_edit`, and the memory rows (`mem_open_bytes`,
  `mem_warm_bytes`: heap held after a reopen and after its first edit, from a counting
  allocator; `mem_estimate_bytes`: the session's own estimate; `engine_doc_bytes`: one
  engine doc loaded from the store). `--rows content` (the large doc): `edit`,
  `doc_edit_as`, `blame_doc`, `flush`, the cold rows and memory. `--rows nosync`:
  `edit_nosync`, `live_runs`, `prov_runs`.
- **Samples:** each call is timed alone. A row stops at `--samples` (10,000) or at its timed
  seconds per run (`--fast-cap-s` 900, `--slow-cap-s` 60, `--cold-cap-s` 60). Every record
  has `samples` and `p99_indicative` (1 below 10,000 samples): such a p99 is written
  "indicative (n=…)", never extrapolated. The fixture is copied back in after every
  `--batch` mutating calls (default a tenth of the size, at least 1,000), so a run at 10k
  stays near 10k (`version_max` in the run's record).
- **The pair:** `tools/perf/pair-commit.sh A B` makes a local commit holding A's tree with
  B's under `.perf-b/`. Submit that commit (never merged or pushed); a commit without
  `.perf-b/` runs side A alone, or A/A with `PERF_AA=1`. The job builds both sides, grows
  one fixture on side A (both sides read it; `PERF_OWN_FIXTURES=1`: each side grows its own
  from the same seed, for a build that can't read A's store) and runs each size `PERF_RUNS` times per side,
  interleaved A, B, A, B (plain rows, then the `nosync` rows, per run). `summarize`
  prints the median per side (`perfh_med_*`), the B/A ratio of the medians
  (`perfh_ratio_*`, ×1000), and growth from the smallest size to the largest
  (`perfh_growth_*`). `fork(V)` and `changed_since(V)` have no Yrs path: the summary names
  them as empty slots, never zeros.

```sh
SHA=$(tools/perf/pair-commit.sh <A> <B>)
k2 compute run "$NODE" --repo . --sha "$SHA" --needs rust@stable --cpus 4 --mem 8G --timeout 2h \
  --env PERF_SIZES=10000,100000 --artifact 'perf-out/samples.tsv' -- sh tools/job.sh tools/perf/pair.sh
```

**Known issue: same-seed fixtures aren't byte-identical.** The seed fixes the calls (same call
count, same version, same content bytes), not the store file: 10k fixtures came out at about
10.0, 13.2, 17.4 and 19.5 MB on one binary. `tools/perf/store-spread.sh` shows it is free space,
not data: the data in those files is the same to about 30 bytes and the pages in use to about 3 in
2,160; only the file's free space differs, and the file's length moves in steps. So `store_bytes`
is the file's length, not the data's size: quote it with `store_data_bytes` (the `nosync` run's
row, with `store_free_bytes`). Why the free space differs from run to run isn't established. A figure belongs to its
fixture, so every record gives `store_bytes`. Between jobs, even on one binary with the timers
off, `events_500`'s p99 at 100k moved about 50 %. So: compare builds on one shared fixture (the
default); sides with their own fixtures compare the fixtures too; and read an absolute figure
from one job as that fixture's, not as the kit's across fixtures.

**Which binary was measured.** A target dir outlives a job, so a build that finds nothing to do would measure an earlier job's binary. `pair.sh` keeps a side's binary only if cargo compiled the harness, the kit and its vendored engine from that side's tree in this job (else `STALE BUILD`, nothing measured), prints `perfh_build_<side>-<kind>` with `bin_sha` and `src_sha` (the source by content: a binary embeds its paths), tags each run's `build` with its `bin=`, and fails on identical binaries or one source on a run with two trees (`TWO BUILDS EXPECTED`; `PERF_SAME_SOURCE=1` for a two-build floor). A/A is one binary by design, says so, and so its floor has no build-to-build difference in it. A cached fixture records its grower's source (`grown-src`), and another source reading it fails (`FIXTURE MISMATCH`).

Give the node's class on every submit (`--env PERF_NODE_CLASS=...`, a disk and CPU class,
never a host name): every record carries it as `node_class`. Absolute figures are the node's
as much as the kit's (one durable commit, the `store_probe` row, differed about 30× between
two classes of build node), so a baseline names its class, and each pair stays on one node.
The job's knobs are at the top of `tools/perf/pair.sh`. The records are `perfh_*` (tool
`perf_harness`, `tools/evidence/README.md`). Times are in ns (`unit: ns`), memory and
counts as is (`unit: count`).
