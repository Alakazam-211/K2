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

BLEND is the file format of Blender, a project of the Blender Foundation. "Blender" is a
trademark of its owner, named here only to say whose file format this is. This project is not
affiliated with, endorsed by or sponsored by it.
