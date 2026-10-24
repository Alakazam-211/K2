# CI and release builds on K2 Compute

K2 runs its own CI and its Linux release builds on its compute nodes
instead of paid GitHub runners. GitHub stores the files and shows a
commit status. The GitHub workflows stay as the fallback until this path
has been proven (stage 0 is shadow builds).

## The CI gate

```
push to main / integ/*
  └─ z3mbpZ LaunchAgent dev.k2.ci-poll (every 60 s)
       scripts/ci/k2-ci-poll.sh --once
         git ls-remote origin  → the first tip it hasn't seen (main first)
         scripts/ci/run-checks.sh <sha>
           ├─ k2 compute run k2n4-vin --sha <sha> -- scripts/ci/checks.sh web
           ├─ k2 compute run k2n5-vin --sha <sha> -- scripts/ci/checks.sh rust-check
           └─ k2 compute run k2n6-hil --sha <sha> -- scripts/ci/checks.sh rust-test
           k2 compute wait …; k2 compute get <job> --json
           scripts/ci/post-status.sh → POST /repos/Alakazam-211/K2/statuses/<sha>
               context k2-compute/web | rust-check | rust-test
               pending → success | failure (checks failed) | error (infra)
```

- **`scripts/ci/checks.sh <part>`** holds `checks.yml`'s steps. Keep the
  two in step: `tests/packaging/ci-gate.sh` fails when a `run:` command in
  `checks.yml` is missing from `checks.sh`. Once compute statuses are the
  gate (stage 1), `checks.yml` becomes dispatch-only and calls `checks.sh`.
- **The poller isn't a K2 heartbeat** (that wakes an LLM and costs tokens)
  **or a git hook** (that misses pushes from other machines). It checks
  one commit at a time. A branch that moves during a run is checked at its
  newest tip only.
- **Statuses carry no link.** The description names the job and node;
  logs stay in K2: `k2 compute logs <job> --failures`.
- **By hand:** `scripts/ci/run-checks.sh <sha> [--parts rust-test]`
  checks any commit, pushed or not. `K2_CI_DRY_RUN=1` prints the statuses
  instead of posting them.
- **Nodes:** `K2_CI_NODE_WEB`, `K2_CI_NODE_RUST_CHECK` and
  `K2_CI_NODE_RUST_TEST` set them (`any` means `--any --os linux --arch
  x86_64`). A wider run (`k2 compute test --shards N` over whole crates,
  `scripts/test-on-nodes.sh`) can be added as another part later.

### Setting it up on z3mbpZ (Rosson approves first; see the open questions)

1. **A CI checkout** (not the release checkout):
   `git clone git@github.com:Alakazam-211/K2.git ~/k2-ci/K2`. Add it to
   z3mbpZ's K2 as a workspace, and grant it the three nodes
   (`k2 compute grant <node> --workspace <ws>`). z3mbpZ's K2 must already
   be a server on k2n4/5/6.
2. **A statuses-only token:** a fine-grained PAT on Alakazam-211/K2 with
   only *Commit statuses: write*. Store it in the login keychain:
   `security add-generic-password -s k2-ci-statuses -a github -w`.
   Don't use the owner's `gh` login.
3. **The LaunchAgent:** `mkdir -p ~/.local/state/k2-ci`, then install
   `scripts/ci/dev.k2.ci-poll.plist` as its header says. To try it first
   without posting, set `K2_CI_DRY_RUN=1` in the plist environment.
4. **Branch protection:** not part of this. GitHub can require the
   `k2-compute/*` contexts later if Rosson wants that.

State lives in `~/.local/state/k2-ci/`: `seen.tsv`, `runs/<sha>.json`
and `logs/<sha>.log`. Nothing goes in `~/.k2`.

## Linux release builds (release.sh)

`K2_LINUX_BUILDS` picks the path:

| Value | What happens |
|---|---|
| `github` (default) | Today: pushing the tag starts `daemon-binaries.yml` and `app-linux.yml`. |
| `shadow` | GitHub stays the source of truth. release.sh also builds the Linux x86 set on K2 Compute before the tag and checks it, then compares it with what GitHub built. |
| `compute` | Stage 1, not built yet. The compute files become the release: release.sh signs them on z3mbpZ and publishes everything in one go. |

What shadow mode does:

- **Step 1.6c:** refreshes Cargo.lock's workspace versions, makes the
  release commit C (the same preview commit the Arch gate builds), and
  submits two jobs with `--sha C --gate --detach --artifact 'dist/*'`:
  - L1 `scripts/ci/build-linux-bins.sh x86_64` on k2n4-vin;
  - L3 `scripts/ci/build-linux-app.sh` on k2n5-vin.

  Two more jobs are optional: V1, a second x86 build on another node
  (`K2_COMPUTE_VERIFY_NODE`), and L2, aarch64 (`K2_COMPUTE_ARM_NODE`).
  L2 waits for the ARM Linux VM on a Mac mini; until then GitHub builds
  aarch64.
- **Step 7.9c:** waits for the jobs, then runs `k2 compute fetch
  --over-relay`. The AppImage is over the relay's 100 MB default. For
  each job, `scripts/ci/compute_release.py check-job` checks:
  - the job ended done with exit 0;
  - the controller verified the receipt;
  - it ran on a node from the release list;
  - the receipt's commit and tree are C's, with nothing uncommitted;
  - argv and `argv_sha256` match the command that was sent;
  - every fetched byte matches the signed receipt and `build-info.json`;
  - the version is right.

  Then it runs read-only byte checks: ELF arch, exactly one build marker,
  no test-keys, ptyd embedded, OAuth mode, and deb/rpm/AppImage magic.
- **After Step 8.9:** the tagged tree must equal C's tree.
- **Step 10.7:** the shadow report compares the files, by exact asset
  name, with the GitHub-built ones.

Nothing is published or signed from compute in shadow mode. A compute
problem only prints a warning. The audit record (receipts, verdicts,
report) goes to `~/k2-release-audit/<v>/compute/`.
`scripts/ci/shadow-linux-builds.sh <tag>` runs the same steps after a
tag.

**OAuth:** shadow builds use `BUILD_OAUTH=placeholder`, so their daemon
can't match GitHub's byte for byte. Passing the real Gmail/MS client
values to nodes (`K2_COMPUTE_OAUTH=env`) stores them in the job plan;
that is an open question for Rosson. Job env names never start with
`K2_`, because K2 Compute refuses those (`env_k2_reserved`).

## K2 Compute changes this needs (0.46.4)

- **Relay cap:** `k2 compute fetch --over-relay` pulls files over 100 MB
  through the relay, up to the 500 MB the node will collect. A big pull
  runs on the server and the CLI polls it, so no request is held open
  longer than 60 s.
- **Reserved gate slot:** node policy `gate_reserved_slots` (0 to 8,
  default 0). Normal work never fills those slots, so a `--gate` job
  starts at once. Set it on the release nodes with
  `k2 compute node policy <node> set gate_reserved_slots=1`.
- **Disk guard:** a `--gate` job is stopped for `disk_low` only when no
  other job is left to stop.
