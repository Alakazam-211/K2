k2-term-wire is a compact binary wire format ("k1") for sending terminal grid snapshots and deltas over a websocket.

# k2-term-wire

> Internal name. This crate will be published as **kessel-wire**, part of the
> Kessel family of open-source terminal crates by Alakazam Labs. It isn't
> published yet (`publish = false`).

## What's in it

`k1` encodes and decodes the snapshot and delta types from the grid crate
(`k2-term-grid`, to be published as kessel-grid):

- `encode_snapshot` / `encode_delta` produce one binary frame each;
- `decode_frame` reads one back into a `WireFrame`.

A k1 frame carries exactly what the JSON form carries, about 2.5× smaller, and
decoding it reproduces the same object. Integers are little-endian with a fixed
layout, run text is length-prefixed UTF-8, a sentinel stands for "terminal
default" colour, and the style flags fit in one byte. The full layout is in the
`k1` module docs, which are the single source of truth.

The intended transport is one websocket per viewer. The server sends a snapshot
on connect and deltas after that as binary messages. Other events and the
client's input stay JSON text. This crate holds the codec only.

## What "k1" means

"k1" is wire format version 1. Every frame starts with three bytes:

| Byte | Value | Meaning |
|---|---|---|
| 0 | `0x6B` (ASCII `k`) | magic byte (`WIRE_MAGIC`) |
| 1 | `1` | format version (`WIRE_VERSION`) |
| 2 | `1` or `2` | frame kind: snapshot or delta |

`decode_frame` returns a `WireDecodeError` for any other magic byte, version
or kind, and for a truncated frame or invalid UTF-8. It never guesses. The
next incompatible layout gets version 2, so a decoder that only knows k1
refuses it instead of misreading it. One caveat: a style bit (`has-cols`, for
runs with wide or zero-width characters) was added within version 1, so k1
assumes the encoder and decoder ship together.

**Other languages.** Browsers need a decoder in their own language. None is
published with this crate yet. The module docs are the spec to write one
from, and the test `fixture_hex_and_json_dump` prints a fixture (hex bytes
plus the matching JSON) to check it against.

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

An encode/decode round trip. This is the crate-level doc example, so it runs
with `cargo test`.

```rust
use k2_term_grid::grid_snapshot::{CellRun, CursorSnapshot, TermGridSnapshot};
use k2_term_wire::k1::{decode_frame, encode_snapshot, WireFrame};

let run = CellRun {
    text: "hello".into(),
    fg: Some(0xff8800), // 0xRRGGBB; None = the terminal's default colour
    bg: None,
    bold: true,
    italic: false,
    underline: false,
    inverse: false,
    dim: false,
    strikeout: false,
    wrapped: None,
    cols: None,
};
let snapshot = TermGridSnapshot {
    pane_id: "pane-1".into(),
    cols: 80,
    rows: 24,
    grid: vec![vec![run]],
    scrollback: vec![],
    cursor: CursorSnapshot { row: 0, col: 5, visible: true },
    version: 1,
    display_offset: 0,
    mouse_report: false,
    sgr_mouse: false,
    alt_screen: false,
};

let bytes = encode_snapshot(&snapshot); // send as one websocket binary message
let WireFrame::Snapshot(back) = decode_frame(&bytes).unwrap() else {
    panic!("a snapshot decodes as a snapshot");
};
assert_eq!(back.grid[0][0].text, "hello");
assert_eq!(back.grid[0][0].fg, Some(0xff8800));
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
