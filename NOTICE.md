# Notices

This repo's own code is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the
README). It holds no code copied from another project.

Rust dependencies keep their own licences: all permissive, checked transitively at their locked
versions by `scripts/check-licences.sh` (`Cargo.lock` here; the fuzz crate's separately, with
libFuzzer's NCSA allowed there alone, because it is test tooling and never shipped).

Public test files are fetched by reference when the tests run and are never committed here. Each
keeps its own licence, listed with its source and checksum in
`crates/timeline/tests/corpus/manifest.tsv`.

OTIO is the file format of OpenTimelineIO, a project of the Academy Software Foundation. FCPXML and
PRPROJ are the file formats of other companies' video editing products. This project is not
affiliated with, endorsed by or sponsored by any of them; their names and marks belong to their
owners and are used here only to name the file formats.
