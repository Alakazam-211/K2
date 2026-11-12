# Notices

This repository is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the
README). It holds no third-party code.

Crates it depends on keep their own licences: all permissive, checked transitively at the locks'
exact versions by `scripts/check-licences.sh`. The shipped crate uses `roxmltree`, `flate2` (on
`miniz_oxide`, `adler2` and `simd-adler32`), `crc32fast` and `cfg-if`. Tests also use the `zip`
crate (MIT) as a second reader. The fuzz crate (`fuzz/`, its own workspace) also uses
`libfuzzer-sys`, `(MIT OR Apache-2.0) AND NCSA`; the gate allows NCSA there alone.

`fuzz/` and the external test checkers (`checkers/`, with `docs/checkers.md`) are private test
tooling: they are kept on this repository's private main line and are not shipped, and a
release leaves them out. The checkers hold manifests, hashes and two short checker programs;
the tools they run (the .NET SDK, the Open XML SDK, Apache POI and its dependencies, and a
Temurin JDK) are fetched at pinned versions for a test run and are never part of this project.

The test corpus in `corpus/` is a manifest only: each file is fetched from its source at a
pinned commit, under that project's licence (named in the manifest), and is never committed.

docx, pptx and xlsx are file formats specified by ECMA-376 and ISO/IEC 29500. This project is
not affiliated with, endorsed by or sponsored by the formats' original vendor.
