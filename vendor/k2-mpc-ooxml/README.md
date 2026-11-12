# k2-mpc-ooxml

Reads and writes docx, pptx and xlsx packages, for the multiplayer kit's packs for those formats
(`k2-mpc-docx`, and the pptx and xlsx packs to come). One crate, `k2-mpc-opc` (`crates/opc`), the
Open Packaging Conventions layer they share:

- **The zip layer** (`zip`): a bounded reader that accepts only a package it can reproduce
  exactly, and a writer that copies every untouched entry byte for byte.
  - Stored and deflate only; zip64, encryption and split archives are refused.
  - Names that escape the package, repeat (case folded) or hold control characters are refused.
  - Any gap or overlap between records is refused; entry counts, sizes and ratios are capped.
  - Every CRC is checked.
- **The package** (`OpcPackage`): every part's content, the content types and the relationships,
  with the byte ranges of what they hold.
  - A `DOCTYPE` in any XML part, UTF-16 XML, an encrypted package and a package with no
    `[Content_Types].xml` are refused. A pack adds its own refusals (macros, the wrong main part).
  - **Writing splices:** `OpcPackage::write` replaces, removes or adds only the parts it is
    given. Every other entry keeps its bytes, its place in the zip and its timestamps.
- **A three-way merge of two saves** (`merge`), at the package level:
  - **Two saves that each add a part pick the same next name and id** (`image3.png`,
    `slide7.xml`, `rId9`). Ours' colliding part is renamed in its own numbering, ours'
    colliding relationship id is re-minted, and every reference to them on ours' side
    (relationship targets, content-type overrides, `r:` attributes in the source part) is
    rewritten. Theirs keeps its names: it is the file an app saved.
  - Content types merge by key (`Default` by extension, `Override` by part name), and a
    relationships part by id. Any other part both sides changed goes to the format pack
    (`PartMerge`); with no pack, it is a conflict.
  - The result may not hold a problem neither side had (a part with no content type, a
    relationship to a missing part, a repeated id, a reference to a missing relationship).
  - **On any doubt, a conflict:** an id the merge can't tell is only a reference, a key or
    relationship both sides changed differently, a part one side changed and the other removed.
  - A merge equal to one side is that side's file, byte for byte; otherwise it is laid out as
    theirs, with ours' new parts after, and every part a side's merge left alone keeps its raw
    record.

Native ids inside content (a slide's `sldId`, a shape's `cNvPr`, a sheet's `sheetId`) are the
format packs' to re-mint, with the same rule, through `PartMerge`.

## What a byte-exact round trip needs

A zip's local headers, central records and end record carry fields each writer chooses: the
versions, the flags (bits 1-2 record the deflate level asked for; bit 3 a data descriptor, with or
without its signature; bit 11 a UTF-8 name), DOS times, extra fields (often different between
the local and the central record), comments and attributes. The compressed bytes depend on the
compressor and its level. So nothing is re-encoded: the reader accepts only records that tile the
file in directory order, and the writer copies each untouched entry's local record and central
record whole, patching only the central record's offset, and the end record's counts and
offsets. A changed entry keeps its headers and gets new content, deflated again by this crate
(its content is exact; its compressed bytes are ours). A new entry has a fixed layout, so the same
content gives the same bytes. The `zip` module's documentation has the detail.

This is the docx pack's package layer, generalised: the docx pack already read and wrote its
corpus byte for byte this way (200 files), and now depends on this crate for it.

## Tests and checks

Every check runs with cargo and a stable Rust toolchain (1.88 or later). `sh scripts/check.sh`
runs every step and exits non-zero on the first failure: fmt, clippy with `-D warnings` (with the
mutants feature too), the tests, the mutants, the licence gate, the licence files and the
private-names check. The main steps by hand:

```sh
cargo fmt -p k2-mpc-opc -- --check
cargo clippy --locked -p k2-mpc-opc --all-targets -- -D warnings
cargo test --locked -p k2-mpc-opc --all-targets
cargo test --locked -p k2-mpc-opc --features mutants --test mutants
```

The private-names check reads its patterns from outside the tree (`scripts/check-private.sh`
says where); with none set it stops with exit 2, so `scripts/check.sh` ends there.

- **Our own generated packages are the default fixtures:** docx, pptx and xlsx shapes, each in
  eight zip layouts (stored, three deflate levels, data descriptors with and without a
  signature, differing extra fields and comments, UTF-8 names and folder entries, mixed and
  reversed), written by a test-only zip writer that shares no code with the crate. Each must
  round-trip byte for byte, and a second reader (the `zip` crate, test-only) must agree on every
  entry.
- **The refusal rows** (`tests/refusals.rs`): a bad zip in each of its forms, a missing content
  types part, duplicate parts (also by case), hostile XML, every limit; and no damage panics.
- **The merge rows** (`tests/merge.rs`): two saves each adding an image (docx), a slide (pptx)
  and a sheet (xlsx); content types and relationships merged by key; deletes; refusals to guess;
  the soundness check; the identities.
- **Mutants** (`--features mutants`, `tests/mutants.rs`) plant bugs in the reader, the writer
  and the merge; each must be caught by the leg written for it.
- **The rust_xlsxwriter test set** (`corpus/xlsxwriter.tsv`): over a thousand files saved by a
  spreadsheet app, fetched by reference (source, commit, licence and each file's sha256 in the
  manifest; never committed) with `scripts/fetch-corpus.sh DIR`, then
  `OOXML_XLSXWRITER_CORPUS=DIR cargo test --test corpus_xlsxwriter`: each one byte-exact, spliced
  and merged. Its output names files only.
- **Fuzz targets** (`fuzz/`, its own workspace, `publish = false`): `package_reader` and `merge3`.
  Private test tooling, not shipped (below).
- **Private test tooling, not shipped:** `fuzz/`, `checkers/` and `docs/checkers.md` are kept on
  this repository's private main line and are not part of any release. A release tag is a cut
  commit that leaves them out, and nothing else here needs them, so a release builds and tests
  without them. The rust_xlsxwriter manifest and its fetch script stay: the test reads them only
  when `OOXML_XLSXWRITER_CORPUS` is set.
- **The licence gate** (`scripts/check-licences.sh`): every crate in the locks, transitively, at
  its exact version, must be MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib or Unicode-3.0
  (libFuzzer's NCSA in the fuzz crate alone).
- `scripts/check-private.sh` keeps internal host names out of the tree; its patterns live outside
  it.
- **External checkers** (`checkers/`: private test tooling, kept on the private main line and
  not shipped), shared by the packs: the Open XML SDK's schema validator and Apache POI, run as
  separate test checkers over packages a pack's rows produce, never linked or shipped. Every
  download is pinned by version and hash (the .NET SDK and the jars by sha512, the Temurin JDK by
  the sha256 its publisher lists; the SDK and the JDK only once a release is two weeks old), the
  validator's packages by a locked NuGet restore, and each run installs them in its own folder
  with .NET's telemetry off. `checkers/self-test.sh` holds them to the rust_xlsxwriter test set
  and a planted bad package. `docs/checkers.md` (private too) has the detail.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions.
