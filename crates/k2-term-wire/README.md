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
`k1` module docs, which are the single source of truth. A TypeScript decoder
mirrors it for browser clients.

The intended transport is one websocket per viewer. The server sends a snapshot
on connect and deltas after that as binary messages. Other events and the
client's input stay JSON text. The session protocol (frame vocabulary, ack
pacing) will move into this crate later; today it holds the codec.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
