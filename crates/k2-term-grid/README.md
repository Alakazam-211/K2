k2-term-grid is a terminal grid model for remote rendering: it turns a live `alacritty_terminal` Term into snapshots and deltas that viewers can render, and reflows a screen to another width.

# k2-term-grid

> Internal name. This crate will be published as **kessel-grid**, part of the
> Kessel family of open-source terminal crates by Alakazam Labs. It isn't
> published yet (`publish = false`).

## What's in it

- **`grid_snapshot`**: `TermGridSnapshot` (the grid, scrollback, cursor, and
  mouse-reporting and alt-screen modes) and `TermGridDelta` (damaged rows plus
  appended scrollback), built from an `alacritty_terminal::Term`. Rows are
  lists of style-homogeneous runs (`CellRun`), with soft-wrap and wide-char
  column hints. `build_emit` reads the Term's damage and decides whether the
  next frame is a full snapshot, a delta or nothing; `drain_damage` keeps that
  state right while nobody is watching.
- **`grid_types`**: a compact line model (`GridUpdate`): per-row plain text,
  sparse style spans and a `wrapped` flag.
- **`reflow`**: re-wraps a `GridUpdate` at another width. It joins
  soft-wrapped rows into logical lines and wraps them again, keeping styles,
  so a narrow viewer gets real line breaks. The algorithm follows WezTerm's
  `rewrap_lines` (MIT).

Everything serializes with `serde` as camelCase JSON. The companion wire crate
(`k2-term-wire`, to be published as kessel-wire) has a compact binary
encoding of the snapshot and delta types.

`build_emit` reads and resets the Term's damage, so call it after each batch
of PTY output, with one `EmitState` per viewer connection. The first call on
an `EmitState` is always a full snapshot.

## How the crates fit

Three crates make a terminal that runs on a server and that any number of
viewers can watch and type into over a websocket:

1. **kessel-pty** (`k2-ptyd` until the rename) keeps the programs alive: it
   owns their PTYs in its own process and keeps their output in a ring.
2. Their output feeds an [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal)
   `Term`, which runs the terminal emulation.
3. **kessel-grid** (`k2-term-grid`) turns that `Term` into snapshots and
   deltas (and reflows them to another width).
4. **kessel-wire** (`k2-term-wire`) encodes the snapshots and deltas as
   compact binary websocket messages.

Each crate works on its own. kessel-crdt and kessel-sync are unrelated
sibling crates in the Kessel family; nothing here depends on them.

Used in [K2](https://k2.dev) (Alakazam Labs), an AI workspace tool, for its
terminal and agent sessions.

## Example

Build a `Term`, feed it bytes, take a snapshot and then a delta, and
serialise them. This is the crate-level doc example, so it runs with
`cargo test`.

```rust
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use k2_term_grid::grid_snapshot::{build_emit, EmitDecision, EmitState};

struct Size { cols: usize, rows: usize }
impl Dimensions for Size {
    fn total_lines(&self) -> usize { self.rows }
    fn screen_lines(&self) -> usize { self.rows }
    fn columns(&self) -> usize { self.cols }
}

let mut term = Term::new(Config::default(), &Size { cols: 40, rows: 5 }, VoidListener);
let mut parser: vte::ansi::Processor = vte::ansi::Processor::new();
let mut state = EmitState::default();

// Bytes as they come from a PTY.
parser.advance(&mut term, b"hello \x1b[1;31mworld\x1b[0m\r\n");
let EmitDecision::Full(snapshot) = build_emit("pane-1", &mut term, &mut state) else {
    panic!("the first frame is a full snapshot");
};
let json = serde_json::to_string(&snapshot).unwrap(); // camelCase JSON

// More output: the next frame carries only the damaged rows.
parser.advance(&mut term, b"second line");
let EmitDecision::Delta(delta) = build_emit("pane-1", &mut term, &mut state) else {
    panic!("a small change is a delta");
};
let json = serde_json::to_string(&delta).unwrap();
```

## Dependencies

`alacritty_terminal` 0.26 (Apache-2.0), `vte` 0.15 (Apache-2.0 OR MIT) and
`serde`. Because `alacritty_terminal` is Apache-2.0 only, a build that uses
this crate carries an Apache-2.0 dependency whichever licence you take this
crate under.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
