# tools/

Jobs for compute nodes: the undo spec's model check (`docs/MODEL-CHECK.md`). Nothing here is
shipped or needed to build the kit.

## The undo model check

| Script | What the job does |
|---|---|
| `undo-quint-job.sh <commands> <out dir>` | builds `undo_enum` and `undo_itf_replay` (release, mutants on), installs Quint 0.33 (npm; TLC comes with Apalache, fetched by the first `quint verify`), and runs `<commands>` in `spec/quint/` |
| `undo-enum-job.sh <commands> <out dir> [extra files]` | builds the same binaries and runs `<commands>` at the repo root (no Quint, no Java) |
| `undo-bundle.sh` | what both use: writes `<out dir>/job.sh` and `<out dir>/env-args` |

Commands files (in `tools/undo/`):

| File | Length | What |
|---|---|---|
| `smoke.sh` | ≤ 10 min | Quint typecheck; a tiny simulator run exported as ITF and replayed into `spec_undo::model`; TLC's exact count at a tiny bound against the enumerator's; every mutant on TLC and on the enumerator |
| `counts.sh` | ≤ 10 min | TLC's exact distinct-state counts at the small bounds against `undo_enum bfs`; ITF replay of 1,000 traces at 2 × 1 + 1 undo + 1 revert; every mutant on TLC (one worker) and on the enumerator |
| `diff.sh` | ≤ 10 min | the exact state-set comparison: TLC's `-dump` against `undo_enum bfs --dump`, by `statediff.py` (it found the one count mismatch, `docs/MODEL-CHECK.md`) |
| `g1-smoke.sh` | ≤ 10 min | the literal G1 bound, sampled: Quint simulator traces replayed, random walks, mutant walks |
| `g1.sh` | about 1 h (`LONG=1`: hours more; see `docs/MODEL-CHECK.md`) | exhaustive at the G1-shaped bounds that fit, with checkpoints; random walks at the literal G1 bound. **Launched only after review**, never from a smoke run |

### Running a job

From the repo root, with any `K2_*`/`K2SO_*` variables unset. **Every run passes an explicit
`--mem`** (a job without one gets a small default and is stopped above it):

```sh
unset $(env | grep -oE '^K2(SO)?_[A-Za-z0-9_]*')
tools/undo-quint-job.sh tools/undo/smoke.sh /tmp/undo-smoke
k2 compute run k2-arch --no-source --needs node@22,java@21,rust@stable --mem 12G \
  --timeout 15m --env CARGO_BUILD_JOBS=6 $(cat /tmp/undo-smoke/env-args) \
  --wait --max-wait 9m -- sh -ec "$(cat /tmp/undo-smoke/job.sh)"
```

An enumerator-only job is the same with `tools/undo-enum-job.sh` and `--needs rust@stable`.
Long runs (`g1.sh`) may add `--tier batch` (idle-time work) and `--detach`; see
`docs/MODEL-CHECK.md` for the exact command and the runtime estimate. Never `--priority`.

### Why the sources travel in environment variables

The jobs run with `--no-source` (no checkout on the node). The sources travel in the job as a
base64 gzip tarball. A Linux argument is at most 128 KiB, and a runner may cap one
environment variable's value far lower (8 KiB measured on this runner, and at most a few dozen
variables), so the tarball is split into chunks of at most `CHUNK` characters (default 8000),
passed as `UNDO_B64_0`, `UNDO_B64_1`, … (`env-args`, one `--env` argument per chunk). The job
joins them, checks the tarball's sha256 (written into `job.sh`), and unpacks it.

**What travels** (about 76 KB gzip, 101 KB base64, 13 chunks with Quint): the workspace
manifest and lock, `crates/spec-undo` (sources, and of its tests only `mutants.rs`, which the
manifest declares), `spec/quint` (Quint jobs), `tools/undo`, the commands file, and of
`crates/base` **only `src/text`, verbatim**. The oracle calls nothing of the kit but
`text::diff` and `text::lines`, and that module uses only `std` and `thiserror`, so the job
builds it as a minimal `k2-mpc-base` (`lib.rs` = `pub mod text;`, with the real manifest's
version, `thiserror` requirement and `conformance` feature). The diff the job runs is the
kit's own code, byte for byte; the job skips the kit's other dependencies.

If the bundle grows past the runner's limits (more chunks than it takes), lower nothing:
raise `CHUNK` only if the runner allows it, or trim what travels.
