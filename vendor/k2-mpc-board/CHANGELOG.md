# Changelog

## Unreleased

Format v1, for its first tag.

- `docs/FORMAT.md`: the spec. `schema/board-schema.json`: its fields, limits and refusal codes as data.
- `vectors/`: conformance vectors, with `vectors/check.mjs` for the layers JavaScript can run.
- `crates/board` (`k2-mpc-board`): the model, a strict zip reader and writer, canonical JSON, limits
  held before and while inflating, typed refusals, a save guard over the canonical manifest, object
  and settings revisions, and `apply_group`, which applies a group of edits all or nothing.
- `docs/FREEZE-v1.md`: the evidence for this freeze.
