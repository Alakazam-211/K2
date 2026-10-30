# k2-mpc-base

The multiplayer kit that file-type packs build on. Everything that repeats across file types
lives here, so a new type takes days or weeks, not months:

- docs on **K2-defined primitives** (text today; rich text, list, map, tree with a real move,
  table and blob specified), with edits as plain data;
- text algorithms: minimal diff to edits, and a three-way merge with no last-writer-wins;
- reusable file codecs (plain text with an exact BOM/line-ending round trip);
- sessions, presence, permission hooks, persistence and snapshots, disk sync;
- tier 0 for any file: presence, a lease, versions and restore, conflict copies, on a keyed
  content-addressed blob store;
- text tools agents run (`cat`, `grep`, `sed -i`, `patch`...), history (versions, undo, revert,
  blame per line at the character's grain), erasure (purge);
- the agent-parity framework: a verb registry and a harness proving each person action has a
  verb with the same effect;
- planned: sync transport.

The CRDT engine (Yrs, Yjs-compatible, today) is private to this crate. Packs never touch it,
so it can be replaced without changing them.

See `docs/PACK_TEMPLATE.md` for what a pack contains and which checks it must pass.

Layout:
- `crates/base` (`k2-mpc-base`): the kit. The only crate a host or pack depends on.
- `crates/conformance` (`k2-mpc-conformance`): the primitive conformance rules, generic over
  engines, run on the kit's `Doc` and on the reference model. Test-only: never published,
  nothing depends on it.
- `vendor/`: other repos copied in by git subtree, test-only (`vendor/README.md`).

Status: early. `spike/` holds the D0 engine-choice spike (Yrs vs Loro). Licence: not yet chosen. See `docs/ARCHITECTURE.md` and `docs/PRIMITIVES.md`.
