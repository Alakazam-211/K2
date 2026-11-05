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

**The Mac nodes** (k2n1-m4, k2n2-m1, k2n3-m1) list rustc 1.99.0 in
`k2 compute nodes`: that is the node's default toolchain, not what a K2
job gets. A job that sends `--needs rust@1.99.1`, or sends no rust need at
all (the checkout's `rust-toolchain.toml` then applies), gets 1.99.1: the
node's own rustup installs it on first use, and its `cargo/bin` comes
before Homebrew's on the job's PATH (checked 2026-11-05, jobs 23653d45 on
k2n1-m4 and fa715dba on k2n2-m1: `rustc 1.99.1`). No exception is needed.
The same holds for the Linux nodes.

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
  `checks.yml` is missing from `checks.sh`. `checks.yml` itself runs only
  by hand (`workflow_dispatch`, since 2026-11-05): GitHub's runners no
  longer check pushes or PRs, and these `k2-compute/*` statuses are the
  check.
- **The parts:** `web` (packaging tests, bun, typecheck, vitest, the SDK,
  the hosted build), `rust-check` (`cargo check -D warnings`, all
  targets, plus the airgap feature), `rust-test` (the nextest subsets) and
  `audit` (`cargo audit` and osv-scanner over `Cargo.lock` and `bun.lock`,
  pinned tools, a canary that proves the scanners still catch a known
  advisory). Rust parts ask the node for `cargo-nextest`, `cmake@3`,
  `pkg-config`, `zlib1g-dev`, `libssl-dev` and `libasound2-dev` (the
  native-voice crates link ALSA).
- **Allowed advisories:** each one is listed in `osv-scanner.toml` with a
  reason and a review-by date at most 90 days out, and by ID in
  `.cargo/audit.toml`; `scripts/ci/audit-ignores.py` fails the part on an
  expired or unexplained entry. Warnings (yanked versions) still print
  and don't fail. Example: RUSTSEC-2026-0441, `pin-utils` unmaintained,
  reached only through `portable-pty 0.8 -> nix 0.25`; allowed until
  2027-01-15, fixed by moving to portable-pty 0.9.
- **`--tier ci`** puts each job first in the node's queue. No slots are
  reserved for CI. The workspace's grants need `--ci` (only the owner or
  an Admin can give that).
- **Where jobs run:** no node is named, so K2 picks one of the Linux
  nodes the submitting workspace is granted. For the poller that is
  `k2-ci`, and its grants are the fence: k2n7-vin, k2n8-vin, k2n9-vin and
  k2n10-vin. k2n12-vin (the fuzz node) never takes CI: `k2-ci` has no
  grant there, so even naming it is refused (`not_granted`). k2-arch is
  an Arch node, so the Debian `sys:` packages fail there: don't grant it
  to `k2-ci`. If the submitting workspace is granted it anyway, the
  scripts add `--avoid k2-arch` (K2 0.46.8+; `K2_COMPUTE_AVOID` changes
  the list). To pin a part
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
5. **Later:** branch protection on `k2-compute/*` is a separate decision
   (Rosson's). `checks.yml` already runs only by hand.

The full, in-order list for z3mbpZ is in "Ops checklist" at the end.

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

## The Linux build gate (release.sh Step 0.6, build-app.sh Step 0)

`scripts/linux-build-gate.sh` proves the whole workspace, the `k2` Tauri
crate too, compiles warning-free on Linux before a release builds
anything. It runs as a K2 Compute job (`K2_LINUX_GATE=compute`, the
default): `cargo check --workspace --all-targets` at HEAD (plus
uncommitted changes, `--dirty`), on a Linux x86 node K2 picks among the
release workspace's grants (never k2-arch), `--tier ci`, the pinned Rust,
the GTK/WebKit and ALSA headers, `--mem 24G` (peak 17.7G, job 20fbaea6), and it waits for the result.
No GitHub runner and no ssh box is involved. `K2_LINUX_GATE=ssh` is the
old way (rsync and ssh to the ship box) while that box exists;
`K2_SKIP_LINUX_GATE=1` skips it loudly. Step 0.5 (cargo, typecheck,
vitest, the web build) still runs on z3mbpZ itself, the release Mac.

## Linux release builds (release.sh)

`K2_LINUX_BUILDS` picks who builds the Linux x86 files. The repo variable
`LINUX_BUILDS` must agree with it; the pre-flight refuses a mismatch
either way.

| `K2_LINUX_BUILDS` | Repo variable `LINUX_BUILDS` | What happens |
|---|---|---|
| `github` (default) | unset or `github` | Pushing the tag starts `daemon-binaries.yml` and `app-linux.yml`. |
| `shadow` | unset or `github` | As `github`, plus release.sh builds the x86 set on K2 Compute before the tag, checks it, and compares it with what GitHub built (Step 10.7). Publishes nothing from compute; a compute problem only warns. |
| `compute` | `compute` | The compute-built x86 files are the release. release.sh publishes them, and signs the daemon on z3mbpZ at Step 10.5 after checking it against the compute verdicts. On the tag, GitHub builds only aarch64 (`daemon-binaries.yml`) and skips `app-linux.yml`. Arch still builds on k2-arch. Needs a signer that accepts compute builds (see "Signing" below). |

**The jobs** (Step 1.6c, at the release commit C, `--tier ci`, the pinned
Rust). L is a Linux build, V the verify build:

| Job | Node | What it builds |
|---|---|---|
| L1 | k2n7-vin | `scripts/ci/build-linux-bins.sh x86_64`: the daemon, k2-node, the root helper and the App engine (`--mem 20G`) |
| L3 | k2n8-vin | `scripts/ci/build-linux-app.sh`: deb, rpm and AppImage. The app links the native voice stack (on by default since 0.46.9), so the job asks for `sys:libasound2-dev` and `--mem 24G` (a release link needs at least 20G) |
| V1 | k2n9-vin | the same as L1, a second time on another node |
| L2 | — | aarch64. Waits for the ARM Linux VM node (`K2_COMPUTE_ARM_NODE`); until then GitHub builds it. |

`K2_COMPUTE_X86_NODE`, `K2_COMPUTE_APP_NODE` and `K2_COMPUTE_VERIFY_NODE`
change the nodes (`K2_COMPUTE_NODE` moves L1 and L3 together).
`K2_COMPUTE_VERIFY_NODE=none` drops V1, with a loud warning.
`K2_COMPUTE_BINS_MEM` and `K2_COMPUTE_APP_MEM` change the memory. The jobs
name their nodes on purpose (the dual build needs two different ones), so
they never land on k2-arch.

**The checks** (Step 7.9c): after `k2 compute fetch --over-relay`,
`scripts/ci/compute_release.py check-job` checks each job:
- it ended done, with exit 0;
- the controller verified its receipt;
- it ran on a release node (`K2_COMPUTE_RELEASE_NODES`, default
  k2n7-vin to k2n11-vin: the Debian Ryzens; not k2-arch, not k2n12);
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
  `gh variable set LINUX_BUILDS -b compute --repo Alakazam-211/K2`;
- a signer that accepts compute builds (below).

Then run `K2_LINUX_BUILDS=compute ./scripts/release.sh <version>`.

Every problem up to Step 9 stops the release before anything is pushed
or tagged, and prints how to fall back. Then:
- **Step 8.5** stages the verified files in `daemon-dist` (an earlier
  run's staged files are removed first, in every mode).
- **Before Step 8.9 pushes**, the release commit's tree must equal C's.
- **Step 9** publishes every staged x86 file but the daemon in the one
  `gh release create`.
- **Step 10.5** runs `scripts/sign-daemon.sh <v> --from-dir <daemon-dist>
  --compute-verdict <audit dir>`: the x86 daemon is signed (U1 and U2)
  only if an ok verdict lists its exact bytes for the tag's tree. Then
  it, its signatures and the `linux-x86_64` manifest keys go up; aarch64
  still comes from GitHub's attested build. The root helper, node and
  engine are signed with the root-helper key, as before.

A `K2_RELEASE_RESUME` run submits the jobs again at Step 7.9c.

**Signing.** Since 0.46.6 the update keys live with the signing user
(`K2_SIGNER=k2sign`, `scripts/k2sign/`). That signer doesn't accept
compute builds yet: compute verdicts are written by the release user and
trust the controller, so they are no evidence for it (prd-signing-user-v1
A4). It needs S3b: the signer checks each node-signed receipt itself
against node keys pinned in its policy. Until S3b ships, release.sh
refuses `K2_LINUX_BUILDS=compute` under `K2_SIGNER=k2sign` in the
pre-flight, before any build, rather than at Step 10.5 with the release
already live. `shadow` needs no signer change (nothing from compute is
signed).

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

## 0.46.9 shadow, then 0.46.10 compute

1. **0.46.9: `K2_LINUX_BUILDS=shadow ./scripts/release.sh 0.46.9`.** The
   repo variable `LINUX_BUILDS` stays unset (or `github`), so GitHub builds
   and publishes Linux as before. release.sh also builds L1, L3 and V1 on
   the Ryzens at the release commit, verifies them (receipts, bytes, V1 =
   L1), and at Step 10.7 compares every file with GitHub's by hash. Shadow
   never holds the release up: a compute problem only warns, and jobs
   still running after `K2_COMPUTE_WAIT_MIN` (60) are collected at 10.7.
   Read `~/k2-release-audit/0.46.9/compute/summary.txt` and
   `shadow-report.json`. Needs the OAuth grants (or
   `K2_COMPUTE_OAUTH=placeholder`, which can't match GitHub's daemon).
2. **What to look for:** the root helper, k2-node and engine identical in
   V1 and L1; the daemon identical to GitHub's (same pin, same remap, same
   OAuth values); deb, rpm and AppImage differ (archive timestamps; not
   expected to match). A differing ELF lists its `rustc`/`GCC` lines.
3. **0.46.10: compute mode,** once S3b is in and the 0.46.9 report is
   clean: `gh variable set LINUX_BUILDS -b compute`, then
   `K2_LINUX_BUILDS=compute ./scripts/release.sh 0.46.10`. Fallback as
   above.

A dry run of the shadow builds for any commit, with no bump, tag, signing
or publishing: `bash scripts/lib/compute-release.sh --shadow-only <sha>`.

## Ops checklist (z3mbpZ, for 0.46.9 and 0.46.10)

Run in this order. "Owner" steps need Rosson (a person) in a K2 terminal
or GUI shell on z3mbpZ. Nothing here needs a GitHub runner.

**A. Rust and the release checkout**
1. `rustup toolchain install 1.99.1 --profile minimal -c rustfmt,clippy`
   (release.sh Step 0.4 also does this).
2. Grant the release workspace the Linux nodes, with CI (owner):
   `for n in k2n7-vin k2n8-vin k2n9-vin k2n10-vin; do k2 compute grant $n --workspace pod-leader --ci --max-time 2h --max-parallel 3 --max-queued 20 --max-disk 200G; done`
   Don't grant it k2-arch or k2n12-vin. `k2 compute nodes` (from the
   release checkout) must list them.

3. Node packages (K2 Web, on every CI and release node, k2n7-vin to
   k2n11-vin): the `sys:` packages the jobs ask for must be approved, or
   a job fails at once with `package_not_approved`. CI and the gate:
   `pkg-config zlib1g-dev libssl-dev libasound2-dev` plus, for the gate
   and L3, `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
   librsvg2-dev libxdo-dev patchelf file xdg-utils`. Also approve
   `shellcheck` on every CI node, so a shellcheck step can ask for
   `sys:shellcheck`. Until then, `--needs aqua:koalaman/shellcheck@v0.10.0`
   works today (job 288ebba1).

**B. OAuth values for the release builds (owner, release checkout)**
1. `(set -a; . ./.env; set +a; printf %s "$K2_GMAIL_CLIENT_ID" | k2 vault set K2_GMAIL_CLIENT_ID --scope ws:pod-leader; printf %s "$K2_GMAIL_CLIENT_SECRET" | k2 vault set K2_GMAIL_CLIENT_SECRET --scope ws:pod-leader)`
   and the same for `K2_MICROSOFT_CLIENT_ID` if `.env` has a real one.
2. For each of k2n7-vin, k2n8-vin, k2n9-vin:
   `k2 vault grant K2_GMAIL_CLIENT_ID --to compute:pod-leader/<node> --as env --env-name BUILD_GMAIL_CLIENT_ID`
   `k2 vault grant K2_GMAIL_CLIENT_SECRET --to compute:pod-leader/<node> --as env --env-name BUILD_GMAIL_CLIENT_SECRET`
   (and `K2_MICROSOFT_CLIENT_ID` as `BUILD_MICROSOFT_CLIENT_ID`).
3. Try it with no release:
   `bash scripts/lib/compute-release.sh --shadow-only "$(git rev-parse HEAD)"`
   must end with every job verified.

**C. The CI poller (owner)**
1. `git clone git@github.com:Alakazam-211/K2.git ~/k2-ci/K2` (gh or ssh
   auth; no token in the URL).
2. `k2 agent hire ~/k2-ci/K2 --name k2-ci --mode off --no-wiki --no-agents-md --dry-run`,
   then the same without `--dry-run`.
3. `for n in k2n7-vin k2n8-vin k2n9-vin k2n10-vin; do k2 compute grant $n --workspace k2-ci --ci --max-time 2h --max-parallel 2 --max-queued 20 --max-disk 200G; done`
   then `k2 compute grants --workspace k2-ci`: exactly those four. Never
   k2n12-vin or k2-arch.
4. In a K2 terminal of `k2-ci`, from `~/k2-ci/K2`:
   `bash scripts/ci/k2-ci-poll.sh --mark-seen`, push a test commit to an
   `integ/ci-smoke` branch, then `bash scripts/ci/k2-ci-poll.sh --once --dry-run`
   every few minutes until it prints `→ pass` or `→ fail`.
5. The job, then at once its token. The vault item lives in workspace
   `cortanax`; if a grant across workspaces is refused, first
   `k2 vault send K2_CI_STATUSES_TOKEN --to k2-ci`.
   `k2 job add k2-ci-poll --cron '*/2 * * * *' --timeout 10m --workspace k2-ci -- bash scripts/ci/k2-ci-poll.sh --once`
   `k2 vault grant K2_CI_STATUSES_TOKEN --to job:k2-ci-poll --as env --env-name CI_STATUSES_TOKEN`
6. `k2 job run k2-ci-poll`, then `k2 job logs k2-ci-poll`. On the next
   push to main, `k2-compute/web`, `rust-check`, `rust-test` and `audit`
   show on the commit.
7. When the CI scripts change on main: `git -C ~/k2-ci/K2 pull --ff-only`.
   The poller runs that checkout's scripts; each check runs its own
   commit's `scripts/ci/checks.sh`.

**D. GitHub repository variables**
- `LINUX_BUILDS`: leave unset (or `github`) for 0.46.9 (shadow). For
  0.46.10: `gh variable set LINUX_BUILDS -b compute --repo Alakazam-211/K2`
  just before the release; back with `-b github` to fall back.
- `CHECKS_ON_GITHUB`: no longer used (`checks.yml` runs only by hand). If
  it was set: `gh variable delete CHECKS_ON_GITHUB --repo Alakazam-211/K2`.

**E. Turning the poller off**
- Pause: `k2 job disable k2-ci-poll` (resume: `k2 job enable k2-ci-poll`).
- Stop a run in progress: `k2 job kill k2-ci-poll`.
- Remove it and its token grant: `k2 job rm k2-ci-poll`.
- Jobs already sent keep running; cancel them with `k2 compute cancel <job>`
  (list: `k2 compute jobs --workspace k2-ci`). In-flight records stay in
  `~/.local/state/k2-ci/inflight/`; a later pass reaps them, or delete
  that folder to forget them.

## K2 Compute features this uses

- **Relay cap:** `k2 compute fetch --over-relay` pulls files over 100 MB
  through the relay, up to the 500 MB the node will collect.
- **CI tier:** `--tier ci` (the same as `--gate`) runs first in every
  queue. The node policy `gate_reserved_slots` exists but isn't used
  (Rosson 2026-10-29).
- **Disk guard:** a CI-tier job is stopped for `disk_low` only when no
  other job is left to stop.
