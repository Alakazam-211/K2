# Evidence: the wall time of a chunked write, history-only chunk against brand-new

A connection without the history right must not learn from a write whether a chunk of its file
is held by a version it can't read (`docs/design/version-storage-r2.md`, section 6;
`docs/SESSIONS.md`, "Chunked versions"). The suite asserts that the counted work is equal
(`tests/chunked_versions.rs`: segment bytes, file and folder syncs, rows written, commits).
This note publishes the wall time beside it: **a figure, not a pass or fail**, published with
each release that changes the chunked write path.

## Setup

- **The test:** `tests/chunked_timing.rs`, `a_history_only_chunk_against_a_brand_new_one_wall_time`
  (ignored by default), run as
  `cargo test --release -p k2-mpc-base --test chunked_timing -- --ignored --nocapture`: a
  release build, debug assertions off. The kit's test-only hooks (planted faults, work
  counters) are compiled in, as in every test build, and cost both classes the same.
- **The doc:** a 1,572,864-byte file, chunked versions on (64 KiB average chunks). Its history
  holds 2,000 versions, each with its own chunk at one place (and its own edit at a second
  place, so no write below equals a stored version). The writer is a connection with read and
  edit but not history.
- **The writes:** 2,000 of each class, interleaved in one random order (fixed seed
  `0x5eedc0de`), each differing from the version before it by 64 bytes at the first place:
  - **history-only:** the bytes there are one stored version's (each used once), so the chunk
    is held only by a version the writer can't read;
  - **brand-new:** fresh bytes.
- **The figure:** each write's wall time (`write_bytes`, from the call to its return: the cut,
  the segment's write and syncs, the commit), per class p10, p50, p90 and p99 (nearest rank),
  and the two-sample Kolmogorov-Smirnov D with its asymptotic p-value (Stephens' correction).
- **Where:** a Linux build node shared with two other jobs (load 10.43 at the start, 9.28 at
  the end, on 16 CPUs), so the tails are indicative.

## Figures

| Run | Class | p10 | p50 | p90 | p99 |
|---|---|---|---|---|---|
| the published run | history-only chunk | 13.563 ms | 21.622 ms | 22.063 ms | 22.372 ms |
| the published run | brand-new chunk | 13.565 ms | 21.614 ms | 22.041 ms | 22.355 ms |

Kolmogorov-Smirnov over the two samples: **D 0.0195, p 0.8381**: no difference between the two
distributions shows at this size.

Segment bytes per write, from the same job's lines: 68,538 (history-only) and 68,473
(brand-new). They differ by a few dozen bytes because the cut is by content: the 64
bytes a write changes move the cut near them by a little, differently for each class. Each
class writes its one changed chunk and its manifest in full; the counted-work rows assert that
the same bytes cost the same, field for field.

Two earlier runs of the same test, on the same kit code (before the kit was rebased onto a
lint-only change of the workspace): the first (load about 5) gave p50 13.650 ms and 13.623 ms,
p99 20.077 ms and 17.635 ms, D 0.0265 with p 0.4787; the second gave p50 14.134 ms and
14.120 ms, p99 23.117 ms and 22.223 ms, D 0.0245 with p 0.5807. The p50 moves with the node's
load (about 13.6 ms at load 5, 21.6 ms at load 10); within each run the two
classes stay together.

## What it doesn't cover

- No quiet-node run: every run shared its node, and the node's load moves the p50 by several
  milliseconds between runs. Within one run, a difference smaller than the two classes' spread
  would not show.
- The one existing difference at whole-file grain is outside this figure: a version whose
  bytes equal an older one's skips its structure index build.
