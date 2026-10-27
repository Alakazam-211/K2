# D0 spike: Yrs vs Loro

Throwaway harness that picked the engine for the Markdown core. It is kept as the seed of the
benchmarks.

- `rust/`: the bench (`bench <trace>`, `paste`, `blame`, `forge`, `empty`) and a stdio `peer` that
  plays the server for the interop tests. A counting allocator gives exact heap bytes.
- `js/interop.mjs`: Yjs, y-protocols and y-codemirror.next (in jsdom) against the Rust peer:
  framing in both directions, a 10-minute 3-typist + agent convergence run with reconnects,
  forged client IDs, cursors and per-user undo.
- `tools/git_history_trace.py`: turns a file's git history into an editing trace.
- `traces/fetch.sh`: downloads the public automerge-paper trace.

Run:

    (cd traces && ./fetch.sh)
    (cd rust && cargo build --release)
    rust/target/release/d0-spike bench traces/automerge-paper.json --label paper
    rust/target/release/d0-spike paste
    (cd js && npm ci --ignore-scripts && node interop.mjs --minutes 10 --seed 1)

Every check asserts and exits non-zero on failure.
