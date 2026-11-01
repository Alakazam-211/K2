# k2-mpc-markdown

The Markdown type pack for the K2 multiplayer kit
([k2-mpc-base](https://github.com/Alakazam-211/k2-mpc-base)). People and agents
edit the same `.md` file live, with presence and attribution, and the file on disk stays plain
Markdown, byte for byte.

- **Schema:** one text root (`source`) over the Markdown source, so every dialect survives.
- **Codec:** an exact BOM/CRLF round trip, a 4 MiB limit, and decline rules; fidelity tests
  run on a corpus.
- **Agent parity:** `docs/PARITY.md` maps every person action to a command-line verb, and
  `tests/agent_parity.rs` runs each through the kit's parity harness (person path vs agent path).
- **Its own verb:** `replace-section` (replace the body under a heading), through the kit's
  `Pack::verbs`.

- **Demo:** `cargo run -p k2-mpc-markdown --example multiplayer_demo`, with three agents and
  one person on one doc (`docs/DEMO.md`, which also has a script to replay it on a host).

Check this repo out next to `k2-mpc-base` (sibling directories).
Status: early. Licence: MIT OR Apache-2.0 (below).

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions. The kit it builds on has
the same licence; third-party code keeps its own (see `NOTICE.md`).
