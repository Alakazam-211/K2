# Changelog

## v0.1.2

The crate version is `0.1.2`; `Cargo.lock` and `fuzz/Cargo.lock` record it. It names no kit. It
pairs with the pptx pack's v0.1.10, which names it beside `../k2-mpc-ooxml`. As for v0.1.1, the tag
is a cut commit that leaves out `checkers/`, `docs/checkers.md` and `fuzz/`.

- **`OpcPackage::write_plan`**: a write as ranges of the file's bytes and what replaces each
  (`zip::Span`: `at`, `len`, `with`), in order, apart and inside the file: a removed or replaced
  entry's record, new entries inserted before the directory, and the directory and end record
  replaced by the new ones. Every other byte is the file's own, so a host can store the result as
  the old bytes with those ranges replaced. `OpcPackage::write` is the plan applied
  (`zip::apply`); `zip::write_plan` does the same for any write of a package's own entries in
  their order. `OpcPackage::write_by_records` builds the same package record by record.
- Tests: `write_plan_applied_equals_write` on every fixture and change set (none, a part
  replaced, the first entry replaced, a part removed, the last entry removed, a part added, all
  three at once), the plan applied independently of the crate and held equal to the record by
  record write; the same on every file of the xlsxwriter set; the mutant `opc_plan_skips_gap`
  (two ranges joined across the file's bytes between them), caught by the plan leg.

## v0.1.1 (2026-11-12)

The crate version is `0.1.1`; `Cargo.lock` and `fuzz/Cargo.lock` record it. No code or test
change since v0.1.0: the package layer, its API and its behaviour are as in v0.1.0. What moves
into the tag is wording in the README, NOTICE and this changelog. It pairs with the pptx pack's
v0.1.2, which names it beside `../k2-mpc-ooxml`. As for v0.1.0, the tag is a cut commit that
leaves out `checkers/`, `docs/checkers.md` and `fuzz/`.

- **The checkers' JDK is pinned** (`checkers/pins.txt`, `checkers/run.sh`): Eclipse Temurin
  21.0.13+9, Linux x64, from Adoptium's release, checked against the sha256 Adoptium publishes
  for it before it is unpacked, under the SDK's two-week rule. `run.sh` puts it first on the
  run's `PATH` and refuses any other version; a JDK on the host is no longer used.
- Wording: the README, NOTICE and `docs/checkers.md` say plainly that `checkers/` and `fuzz/`
  are private test tooling, kept on the private main line and not shipped. The README's
  "Tests and checks" gives the cargo commands, naming no build machines.

## v0.1.0 (2026-11-11): the first tag

The first tag of `k2-mpc-opc`, the package layer the docx, pptx and xlsx packs share: the zip
layer, `OpcPackage` and the package-level three-way merge, below. The crate version is `0.1.0`
(it was 0.0.0); `Cargo.lock` and `fuzz/Cargo.lock` record it. It pairs with the pptx pack's
v0.1.1, which names it beside `../k2-mpc-ooxml`. It depends on no kit crate. The tag is a cut
commit that leaves out the test tooling a host never builds or runs: the external checkers
(`checkers/`, .NET and Java, and `docs/checkers.md`, their note) and the fuzz crate (`fuzz/`).
The part below is the history of the work before this tag.

### The shared package layer

- **External checkers** (`checkers/`, moved here from the docx pack so the pptx and xlsx packs
  share them): `run.sh` runs the Open XML SDK's schema validator and Apache POI over a folder of
  docx, pptx and xlsx packages, on build nodes only, as test checkers never linked or shipped.
  Pins in `pins.txt` (the .NET SDK 8.0.427, at least two weeks old when pinned, by its publisher's
  sha512; 13 jars by sha512), the validator's NuGet lock restored in locked mode,
  everything installed per run in its own folder with .NET's telemetry off. POI's counts per
  format (docx pictures, comments, footnotes; pptx slides and pictures; xlsx sheets and
  pictures) against a folder's `expect*.tsv`. `self-test.sh`: the rust_xlsxwriter test set with
  its recorded findings (`known-findings.tsv`) and a planted bad package both checkers must
  fail. `docs/checkers.md` rewritten: they run now.

- `k2-mpc-opc` (`crates/opc`): the zip layer, extracted from the docx pack with its behaviour
  and codes kept: a bounded reader that accepts only what it can reproduce exactly, and a writer
  that copies untouched entries byte for byte. New: `Source::New`, an added entry with a fixed
  layout.
- `OpcPackage`: open (the pack's per-entry check as a hook), the content types and relationships
  parsed with their byte ranges, part names (keys, relationships parts, target resolution,
  minting in a part's own numbering), and `write`, which splices only the parts that changed.
- `merge`: the package-level three-way merge. Colliding adds are reconciled on ours' side (parts
  renamed, relationship ids re-minted, every reference rewritten); content types merge by key and
  relationships by id; other parts both sides changed go to the format pack's `PartMerge`; the
  result is checked for problems neither side had. Two saves that each add an image, a slide or
  a sheet now merge.
- Tests: 48 generated packages byte-exact in eight zip layouts with a second reader agreeing; the
  refusal rows; the merge rows; mutants per leg; the rust_xlsxwriter test set by reference
  (1,049 files: byte-exact, spliced, merged); fuzz targets `package_reader` and `merge3`.
- The repository: README, NOTICE, the licence gate at exact versions, the licence-files check,
  the private-names check with its patterns kept outside the tree, `docs/checkers.md`.
