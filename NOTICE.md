# Notices

This repo's own code is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the
README). It holds no code copied from another project, and no text, tables or figures from the
specifications it reads: it cites their section numbers only. They are, as citations: the
*Gerber Layer Format Specification*, revision 2026.05 (the photoplot reader), and the *XNC Format
Specification*, revision 2021.11 (the drill reader).

Rust dependencies keep their own licences: all permissive, checked transitively at their locked
versions by `scripts/check-licences.sh` (`Cargo.lock` here; the fuzz crate's separately, with
libFuzzer's NCSA allowed there alone, because it is test tooling and never shipped).

Public test files are fetched by reference when the tests run and are never committed here. Each
keeps its own licence, listed with its source and checksum in `crates/fab/tests/corpus/manifest.tsv`.
The test files committed here (`crates/fab/tests/data/`) were written by hand for these tests.

Gerber Format is a registered trademark of Ucamco NV; the photoplot reader reads files in that
format and is not affiliated with, endorsed by or sponsored by Ucamco. XNC is Ucamco's subset of
the IPC-NC-349 drill format; Excellon is a trademark of its owner. These names are used here only
to name the file formats.
