# k2-mpc-board

The `.k5board` file: a portable whiteboard file, one zip holding a manifest, a board as canonical
JSON, and images named by their hash. Anyone can read it from the spec alone.

- **Spec:** `docs/FORMAT.md`.
- **Schema:** `schema/board-schema.json`, generated from the Rust tables: types, fields, limits,
  defaults, refusal codes.
- **Verbs:** `docs/VERBS.md`: what an agent can do with a file, with no live session. `docs/PARITY.md` lists
  everything a person can do and how an agent does it.
- **Vectors:** `vectors/`: inputs with their expected board or typed refusal, for a reader in any
  language. `vectors/check.mjs` runs the JavaScript side.
- **Crate:** `crates/board` (`k2-mpc-board`): the model, a strict zip reader and writer, canonical
  JSON, and limits decided before parsing. It works on bytes; the host owns files. It refuses,
  typed; it never cuts.

Run the checks with `scripts/check.sh`.

Status: format v1 is frozen as `format-v1.0`. The pack is at v0.1: one writer at a time, on files. Live comes later.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions. Third-party code keeps its
own licence (see `NOTICE.md` and `THIRD-PARTY-NOTICES.md`).
