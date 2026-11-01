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
