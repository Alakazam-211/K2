# Notices

This repo's own code is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the
README). It holds no code copied from another project; `PROVENANCE.md` says what it was written
from.

Rust dependencies keep their own licences: all permissive, checked transitively at their locked
versions by `scripts/check-licences.sh` (`Cargo.lock` here; the fuzz crate's separately, with
libFuzzer's NCSA allowed there alone, because it is test tooling and never shipped).

Public test files are fetched by reference when the tests run and are never committed here. Each
keeps its own licence, listed with its source and checksum in
`crates/blend/tests/corpus/manifest.tsv`.

`crates/blend/tests/builds/` holds the pack's own test data, under this repo's licence: scenes
built from the pack's own spec (`scene-spec.json`, there) and saved by Blender's releases 5.2.4
and 5.3.0, with the program's report of them (`values.tsv` per release). `MANIFEST.tsv` there
lists each file's size and sha256, and the tests check them.

BLEND is the file format of Blender, a project of the Blender Foundation. "Blender" is a
trademark of its owner, named here only to say whose file format this is. This project is not
affiliated with, endorsed by or sponsored by it.
