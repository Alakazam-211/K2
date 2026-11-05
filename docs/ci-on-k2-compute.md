# CI and release builds on K2 Compute

K2 runs its own CI and its Linux release builds on its compute nodes
instead of paid GitHub runners. GitHub stores the files and shows a
commit status. The GitHub workflows stay as the fallback.

## One Rust version

`rust-toolchain.toml` pins the compiler for every machine that builds K2:
the Macs, the z3mbpZ release, the Linux nodes, the Windows box and
GitHub. rustup reads the file; Homebrew's cargo does not, so a build Mac
needs rustup. `release.sh` Step 0.4 installs the pinned toolchain if it's
missing and stops if `rustc -V` names another version
(`K2_SKIP_RUST_PIN_CHECK=1` skips the check, loudly).

On a compute node an explicit `--needs rust@X` sets `RUSTUP_TOOLCHAIN`,
which **overrides** the checkout's `rust-toolchain.toml`. So for K2's own
repo, send the pinned version, never `rust@stable`:

```
k2 compute run <node> --sha <sha> --needs rust@1.99.1,cmake@3 … -- …
```

1.99.1 is today's pin: read it from `rust-toolchain.toml` at the commit
you send, or in a script `$(k2_rust_need <sha>)` from
`scripts/lib/rust-pin.sh`.

The scripts read the pin at the commit they send (`scripts/lib/rust-pin.sh`:
`k2_rust_pin`, `k2_rust_need`). `tests/packaging/rust-pin.sh` fails when
the file, the workflows' `toolchain:` lines or a script disagree.

To move to a new Rust: change `channel` in `rust-toolchain.toml` and the
four `toolchain:` lines in `.github/workflows`, then run the full gate on
a node.

## The CI gate

```
push to main / integ/*
  └─ z3mbpZ: k2 job k2-ci-poll (cron every 2 min, workspace k2-ci)
       scripts/ci/k2-ci-poll.sh --once            (never waits for a job)
         1. reap: run-checks.sh --reap <sha> for every run in flight
              all jobs ended → final statuses; past the deadline → error
         2. submit: the first tip not seen yet (main first), while fewer
            than K2_CI_MAX_INFLIGHT (2) runs are in flight:
              run-checks.sh <sha> --submit-only --ref <branch>
                k2 compute run --any --os linux --arch x86_64 --tier ci \
                  --sha <sha> --needs rust@<pin at sha>,… --mem 24G \
                  --supersede k2-ci/<branch>/<part> -- scripts/ci/checks.sh <part>
                for each part: web, rust-check, rust-test, audit
       scripts/ci/post-status.sh → POST /repos/Alakazam-211/K2/statuses/<sha>
           context k2-compute/<part>
           pending → success | failure (checks failed) | error (infra)
```

- **`scripts/ci/checks.sh <part>`** holds `checks.yml`'s steps.
  `tests/packaging/ci-gate.sh` fails when a `run:` command in
  `checks.yml` is missing from `checks.sh`.
- **`--tier ci`** puts each job first in the node's queue. No slots are
  reserved for CI. The workspace's grants need `--ci` (only the owner or
  an Admin can give that).
- **Where jobs run:** no node is named, so K2 picks one of the Linux
  nodes the submitting workspace is granted. For the poller that is
  `k2-ci`, and its grants are the fence: k2n7-vin, k2n8-vin, k2n9-vin and
  k2n10-vin. k2n12-vin (the fuzz node) never takes CI: `k2-ci` has no
  grant there, so even naming it is refused (`not_granted`). To pin a part
  to a node, set `K2_CI_NODE_WEB`, `K2_CI_NODE_RUST_CHECK`,
  `K2_CI_NODE_RUST_TEST` or `K2_CI_NODE_AUDIT` to its name.
- **A newer push replaces an older one.** If a branch moves before its
  older jobs start, those queued jobs are cancelled and the older commit
  gets `error` "superseded by a newer push". A job that is already
  running is never stopped.
- **Statuses carry no link.** The description names the job and the
  node. The logs stay in K2: `k2 compute logs <job> --failures`.
- **By hand:** `scripts/ci/run-checks.sh <full 40-character sha> [--parts rust-test]`
  submits, waits and posts, for any commit, pushed or not.
  - It runs as the workspace you call it from (a K2 terminal of that
    workspace, or `K2_CI_WORKSPACE=<name>`), on that workspace's granted
    nodes.
  - It posts real statuses under the same `k2-compute/*` names as the
    poller. `K2_CI_DRY_RUN=1` prints them instead. Without a token the
    checks still run, and each status says "not posted".
  - From a workspace whose grants lack `--ci`, add `K2_CI_TIER=standard`.
  - A K2 CLI older than 0.46.5 has no `--tier`; the scripts then send
    `--gate`, which is the same CI tier.
- **Not a heartbeat, not a git hook:** a heartbeat wakes an LLM and costs
  tokens, and a git hook misses pushes made from other machines.
- **Tests run under cargo-nextest**, profile `ci` in
  `.config/nextest.toml`: each test in its own process, no retries,
  failures at the end of the log, JUnit for
  `k2 compute logs --failures`.
- **Whole suites, split:** `scripts/test-on-nodes.sh [--crate C]...` runs
  `k2 compute test --shards 3 --profile ci --os linux` per crate.

State lives in `~/.local/state/k2-ci/`:
- `seen.tsv`;
- `inflight/<sha>.tsv` (a run whose jobs haven't all ended);
- `runs/<sha>.json`;
- `logs/<sha>.log`.

Nothing goes in `~/.k2`.

### The statuses token

The token is a fine-grained PAT with only *Commit statuses: write* on
Alakazam-211/K2. Never use the owner's `gh` login. `post-status.sh` looks
for it in this order:

1. `CI_STATUSES_TOKEN`: the job's vault grant. A vault env grant can't
   use a `K2_*` name (`env_name_not_allowed`), so grant it like this:
   `k2 vault grant K2_CI_STATUSES_TOKEN --to job:k2-ci-poll --as env --env-name CI_STATUSES_TOKEN`
2. `K2_CI_STATUSES_TOKEN`, set by hand for a run outside the job.
3. The macOS login keychain item `k2-ci-statuses` (account `github`).

The token reaches curl on stdin, never in argv. To put the PAT into the
vault in the first place, the owner fills a secret card
(`k2 thread secret <addr> --name K2_CI_STATUSES_TOKEN --for api.github.com`);
the value never passes through a chat or a file.

### Setting it up on z3mbpZ (owner steps)

1. **A CI checkout** (not the release checkout):
   `git clone git@github.com:Alakazam-211/K2.git ~/k2-ci/K2`, then add it
   as workspace `k2-ci`:
   `k2 agent hire ~/k2-ci/K2 --name k2-ci --mode off --no-wiki --no-agents-md`
2. **Grants**, exactly these four nodes:
   `k2 compute grant <node> --workspace k2-ci --ci --max-time 2h --max-parallel 2 --max-queued 20`
   for k2n7-vin, k2n8-vin, k2n9-vin and k2n10-vin. z3mbpZ's K2 must be a
   server on each of them: `k2 compute nodes` there lists the nodes it
   can use.
3. **Seed, then try it.** `--dry-run` really runs the jobs; it only
   prints the statuses instead of posting them. From `~/k2-ci/K2`, in a
   K2 terminal of `k2-ci`:
   - run `scripts/ci/k2-ci-poll.sh --mark-seen` (it records today's tips
     without checking them);
   - push a test commit to an `integ/` branch;
   - run `scripts/ci/k2-ci-poll.sh --once --dry-run` every few minutes,
     until it prints `→ pass` or `→ fail` for that commit. The statuses
     it would have posted are in `~/.local/state/k2-ci/logs/<sha>.log`.
4. **The job, then at once its token** (before its first tick, 2 minutes
   later; until then its statuses aren't posted):
   `k2 job add k2-ci-poll --cron '*/2 * * * *' --timeout 10m --workspace k2-ci -- bash scripts/ci/k2-ci-poll.sh --once`
   `k2 vault grant K2_CI_STATUSES_TOKEN --to job:k2-ci-poll --as env --env-name CI_STATUSES_TOKEN`
   Watch it with `k2 job logs k2-ci-poll`.
5. **Later:** once the statuses have been reliable for a while,
   `gh variable set CHECKS_ON_GITHUB -b off` stops `checks.yml`'s paid
   push and PR runs (a manual dispatch still runs). Branch protection on
   `k2-compute/*` is a separate decision.

A Mac without `k2 job` can run the same poller from the LaunchAgent in
`scripts/ci/dev.k2.ci-poll.plist` (its header says how to install it).

### Why there is no shared sccache (measured 2026-10-24)

Nodes already keep a warm target dir per project slot, so a job at a
nearby commit recompiles only what changed. sccache only helps when that
dir is cold. On k2n4-vin, a cold `cargo build -p k2-daemon` (dev) took
643 s without sccache, 696 s while filling an empty local cache and 549 s
from a warm one in a different checkout path (jobs get a fresh path each
time): 80% of Rust units and 27% of C/C++ units hit, and the long poles
(k2-core, k2-daemon, the links, cmake builds) never do. So: about 15% off
a cold build, 8% on to fill, nothing on a warm slot. A cache shared across
jobs would also be shared across the K2 servers that use a node, and a
CI-tier release build must never read objects another tenant could have
written. Not added; revisit only as a per-workspace, node-managed cache
dir that CI-tier jobs never mount.

## Linux release builds (release.sh)

`K2_LINUX_BUILDS` picks who builds the Linux x86 files. The repo variable
`LINUX_BUILDS` must agree with it; the pre-flight refuses a mismatch
either way.

| `K2_LINUX_BUILDS` | Repo variable `LINUX_BUILDS` | What happens |
|---|---|---|
| `github` (default) | unset or `github` | Pushing the tag starts `daemon-binaries.yml` and `app-linux.yml`. |
| `shadow` | unset or `github` | As `github`, plus release.sh builds the x86 set on K2 Compute before the tag, checks it, and compares it with what GitHub built (Step 10.7). Publishes nothing from compute; a compute problem only warns. |
| `compute` | `compute` | The compute-built x86 files are the release. release.sh signs them on z3mbpZ and publishes them in the one `gh release create`. On the tag, GitHub builds only aarch64 (`daemon-binaries.yml`) and skips `app-linux.yml`. Arch still builds on k2-arch. |

**The jobs** (Step 1.6c, at the release commit C, `--tier ci`, the pinned
Rust). L is a Linux build, V the verify build:

| Job | Node | What it builds |
|---|---|---|
| L1 | k2n7-vin | `scripts/ci/build-linux-bins.sh x86_64`: the daemon, k2-node, the root helper and the App engine |
| L3 | k2n8-vin | `scripts/ci/build-linux-app.sh`: deb, rpm and AppImage |
| V1 | k2n9-vin | the same as L1, a second time on another node |
| L2 | — | aarch64. Waits for the ARM Linux VM node (`K2_COMPUTE_ARM_NODE`); until then GitHub builds it. |

`K2_COMPUTE_X86_NODE`, `K2_COMPUTE_APP_NODE` and `K2_COMPUTE_VERIFY_NODE`
change the nodes. `K2_COMPUTE_VERIFY_NODE=none` drops V1, with a loud
warning.

**The checks** (Step 7.9c): after `k2 compute fetch --over-relay`,
`scripts/ci/compute_release.py check-job` checks each job:
- it ended done, with exit 0;
- the controller verified its receipt;
- it ran on a release node (`K2_COMPUTE_RELEASE_NODES`, default
  k2n7-vin, k2n8-vin, k2n9-vin, k2n10-vin);
- the receipt's commit and tree are C's, with nothing uncommitted;
- argv and `argv_sha256` match the command that was sent;
- every fetched byte matches the signed receipt and `build-info.json`;
- the version is right.

It then runs read-only byte checks: ELF arch, exactly one build marker,
no test keys, ptyd embedded, and the OAuth mode (`compute` requires
`baked`).

V1 must equal L1 byte for byte for the root helper, k2-node and the App
engine, which run as root on customer boxes. The daemon is compared and
reported only.

**Running a compute release.** Before the first one:
- the release checkout's workspace on z3mbpZ (`pod-leader`) needs grants
  with `--ci` on k2n7-vin, k2n8-vin and k2n9-vin;
- the OAuth vault grants below exist;
- the repo variable is set:
  `gh variable set LINUX_BUILDS -b compute --repo Alakazam-211/K2`.

Then run `K2_LINUX_BUILDS=compute ./scripts/release.sh <version>`.

Every problem stops the release before anything is pushed or tagged, and
prints how to fall back. Then:
- **Step 8.5** stages the files, signs the x86 daemon with the updater
  key, and writes the `linux-x86_64` and x86 engine entries into
  `daemon-latest.json`.
- **Before Step 8.9 pushes**, the release commit's tree must equal C's.
- **Step 9** publishes everything in one `gh release create`.
- **Step 10.5** signs the root helper, node and engine with the
  root-helper key, as before.

A `K2_RELEASE_RESUME` run submits the jobs again at Step 7.9c.

**Fallback:**
1. `gh variable set LINUX_BUILDS -b github --repo Alakazam-211/K2`
2. `K2_LINUX_BUILDS=github ./scripts/release.sh <v>`. If the Mac build
   and notarization already finished, add `K2_RELEASE_RESUME=8`
   (publish only; release.sh prints the right line).

**OAuth** (`K2_COMPUTE_OAUTH`):
- **`secret`** (the default): the Gmail client id and secret, plus the
  Microsoft client id when the release Mac's `.env` has one, come from
  the vault with `--secret NAME=BUILD_*`. Only the names are in the job
  plan; the values are sealed to the node when the job starts.
- **Vault names and grants:** `K2_GMAIL_CLIENT_ID`,
  `K2_GMAIL_CLIENT_SECRET` and (optional) `K2_MICROSOFT_CLIENT_ID`, in
  the release workspace (`pod-leader`). Only a person grants them, per
  node:
  `k2 vault grant K2_GMAIL_CLIENT_ID --to compute:pod-leader/k2n7-vin --as env --env-name BUILD_GMAIL_CLIENT_ID`
  (and the same for the others, as `BUILD_GMAIL_CLIENT_SECRET` and
  `BUILD_MICROSOFT_CLIENT_ID`). A grant can't use a `K2_*` env name; the
  job asks for the `BUILD_*` name and the build script maps it back.
- **`env`:** this Mac's values with `--env`. They are stored in the job
  plan, so use it for tests only.
- **`placeholder`:** builds without the values, for a shadow run by
  hand. Such a daemon can't match GitHub's and never ships.
- Job env names never start with `K2_`: K2 Compute refuses those
  (`env_k2_reserved`).

**Paths and the compiler:** every Linux builder remaps its paths
(`scripts/ci/repro-build-env.sh`) and uses the pinned Rust, so two
builders produce the same bytes. If they don't match, the shadow report
(`shadow-report.json`) lists each differing ELF's toolchain lines
(`rustc version …`, `GCC: …`). The deb, rpm and AppImage carry archive
timestamps, so they aren't expected to match.

**Audit:** `~/k2-release-audit/<v>/compute/` keeps the receipts,
verdicts, `summary.txt` and the shadow report.
`scripts/ci/shadow-linux-builds.sh <tag>` runs the shadow steps after a
tag.

## K2 Compute features this uses

- **Relay cap:** `k2 compute fetch --over-relay` pulls files over 100 MB
  through the relay, up to the 500 MB the node will collect.
- **CI tier:** `--tier ci` (the same as `--gate`) runs first in every
  queue. The node policy `gate_reserved_slots` exists but isn't used
  (Rosson 2026-10-29).
- **Disk guard:** a CI-tier job is stopped for `disk_low` only when no
  other job is left to stop.
