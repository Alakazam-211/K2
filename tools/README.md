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
| `smoke.sh` | ≤ 15 min | Quint typecheck; tiny simulator runs exported as ITF and replayed into `spec_undo::model` (one bound with a restart); TLC's exact count at a tiny bound against the enumerator's; every mutant on TLC and on the enumerator (the restart mutant at its own bound) |
| `counts.sh` | ≤ 15 min per `PART` (`a`, `b`, `m`; v1.8: all three no longer fit one job) | TLC's exact distinct-state counts at the small bounds (one with a restart, one where restorations meet) against `undo_enum bfs`; ITF replay of 1,000 traces at 2 × 1 + 1 undo + 1 revert and 1,000 at 2 × 1 + 2 undos + 1 revert; every mutant on TLC (one worker) and on the enumerator |
| `diff.sh` | ≤ 10 min | the exact state-set comparison: TLC's `-dump` against `undo_enum bfs --dump`, by `statediff.py` (it found the one count mismatch, `docs/MODEL-CHECK.md`) |
| `g1-smoke.sh` | ≤ 10 min | the literal G1 bound, sampled: Quint simulator traces replayed, random walks, mutant walks |
| `g1.sh` | about 1 h (`LONG=1`: hours more; see `docs/MODEL-CHECK.md`) | exhaustive at the G1-shaped bounds that fit, with checkpoints; random walks at the literal G1 bound (and with one restart), per-class coverage in the summary. **Launched only after review**, never from a smoke run |

`inst` and `itfinst` take an optional 11th argument, `RESTARTS` (default 0); `undo_enum` and
`undo_itf_replay` take `--restarts N`.

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

### Apalache and TLC in a job

`tlc.sh` (sourced after the warm-up `quint verify`, which fetches Apalache) starts **one
Apalache server** for the job (`APA_HEAP`, default 6g) and every `quint compile` reuses it.
On the v1.7 model a compile that started its own server failed at random (`error:` with
nothing after it); with one server about one compile in six still fails cold, so `tlc`
retries a failed compile up to 3 times and logs each failure. TLC runs with `TLC_HEAP`
(default 3g), so both fit a `--mem 12G` job. The warm-up itself may report exit 1: it is
only there to fetch Apalache.

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
