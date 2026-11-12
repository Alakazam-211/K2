# Changelog

## v0.1.2 (2026-11-12): pairs with kit v0.15.1 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.15.1** (the carrier row's judge, `test_support::carrier_judge`,
which this pack's tests call) and **`k2-mpc-ooxml` v0.1.1** (`k2-mpc-opc` 0.1.1), each checked
out beside this repo. The pack's behaviour is unchanged from v0.1.1. As for v0.1.1, the tag is a
cut commit that leaves out the fuzz crate (`fuzz/`).

- **The kit v0.15.1** (its cut `e31ed4d9`): a keyed doc's reads, field verbs and one-element
  list changes cost the kit what they touch, and the pack contract's carrier row. `Cargo.toml`'s
  comment and the README name the tag and its cut. `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.15.1, k2-mpc-opc 0.1.1 (ooxml's v0.1.1, whose cut `2362f7c1` they now name; the
  opc crate's code is the same as v0.1.0's) and the crate version 0.1.2.
- `scripts/check.sh` runs the kit's `scripts/pack-checks/engine-free.sh` (the pack template's
  rule: no CRDT engine dependency or import) after the carrier-row check.
- M1's one-element list lines print one bracket each: `M1: shape add (the verb alone; a text
  box in front of a slide's others), ...`.
- **The carrier row, the kit's** (the pack contract for a pack that keeps a carrier):
  `tests/live_carrier.rs`, `carrier_judge_byte_exact_over_the_corpus`, runs
  `session::test_support::carrier_judge` over the byte-exact corpus (every fixture, stored and
  deflated, and the golden deck; `splice_rows::corpus`, which the pack's own untouched row now
  shares) and requires a byte-exact verdict for every file: the hook, the file opened live
  through the kit, and the same after a restart. The planted `EncodeReserialises` is caught by
  it too (`tests/mutants.rs`). The dev-dependency on the kit turns on its `test-support`
  feature (test builds only). `scripts/check.sh` runs the kit's
  `scripts/pack-checks/carrier-row.sh` after fmt, which refuses a carrier pack without the row.
- **M1 times the one-element list changes** (`tests/m1.rs`, a measurement): on the 2,500-shape
  deck, after the write-backs, 200 calls each of `shape add` (a text box in front of a slide's
  others), `shape order` (a shape to the front, then the same shape to the back; every call
  moves one) and `shape delete` (each added box, so the deck ends with the shapes it started
  with), each printing p50, p99 and max of the verb alone, in the same run as the move, read
  and show lines; then a write-back of them before the phase timers.
- Wording only, no code or test logic changed: internal planning labels in the docs, comments,
  this changelog and one test's failure message are replaced by plain statements of what they
  meant; one private function in `live::splice` is renamed (`plan` to `diff_steps`).
- The README's "Tests and checks" gives neutral contributor guidance (the cargo commands), and
  the README, `Cargo.toml` and `docs/checkers.md` say plainly that `fuzz/` and the external
  checkers (`k2-mpc-ooxml`'s `checkers/`) are private test tooling, kept on the private main
  line and not shipped.

## v0.1.1 (2026-11-11): pairs with k2-mpc-ooxml v0.1.0 and kit v0.15.0

**Requires `k2-mpc-ooxml` v0.1.0** (`k2-mpc-opc` 0.1.0, its first tag) and `k2-mpc-base`
v0.15.0, each checked out beside this repo. The pack's code and tests are unchanged from
v0.1.0. As for v0.1.0, the tag is a cut commit that leaves out the fuzz crate (`fuzz/`).

- `Cargo.lock` and `fuzz/Cargo.lock`: k2-mpc-opc 0.1.0 (it was 0.0.0, ooxml's untagged main)
  and the crate version 0.1.1. With ooxml v0.1.0 beside it, v0.1.0's locks no longer match, so
  `--locked` builds need this tag.
- `Cargo.toml`'s and README's ooxml pin name its v0.1.0 cut `5ed01a00` (they named its main
  `790a044f`; the tagged crate's code is the same). `docs/checkers.md`: the external checkers
  run from ooxml's main branch, since its tags leave `checkers/` out.

## v0.1.0 (2026-11-11): the first tag, pairs with kit v0.15.0

**Requires `k2-mpc-base` v0.15.0** (the first kit tag with the carrier hook K1, the keyed
limits and the keyed outside-save rule) and the shared package layer `k2-mpc-ooxml`
(`k2-mpc-opc`, at `790a044f`), each checked out beside this repo. The crate version is `0.1.0`;
`Cargo.lock` and `fuzz/Cargo.lock` record k2-mpc-base 0.15.0. The tag is a cut commit that
leaves out the fuzz crate (`fuzz/`), test tooling a host never builds.

What the first tag holds: reading decks and their structure index, the merge of an app's save
by shape (tier 0.5), agent verbs that splice only what they change, and a deck as a live
keyed doc on the kit's structured live mode, written back through the kit's carrier.
The two parts below are the history of the work before this tag.

### A deck as a live keyed doc

- **The kit at v0.15.0** (`07a2fa63`, both locks; written against its main `91df58ea`): the
  keyed limits (`MPC-E415` on import, `MPC-E822` on growth), the keyed outside-save rule, and
  the carrier hook K1 (`Pack::wants_carrier`, `Pack::encode_from`; version carriers; keyed
  structured reads that render nothing).
- **`PptxLive`** (`live`): the kit's `Pack` for a live deck. Two MovableLists (`slides`,
  `shapes`) for existence and order, two composite maps (`slide_props`, `shape_props`) of one
  field each, a `deck` map of facts and id marks. Keys are the file's own ids (`s<sldId>`,
  `s<sldId>-<cNvPr>`), for imported elements and for ones a verb adds. Geometry is one field
  (`geom`). Declines `live_ids` and `live_text_too_large`; the kit's cap gives
  `live_too_large`. Outside saves reconciled by key. **B1:** a slide's id mark is the highest
  `cNvPr` in its whole tree, a group's children included, and a live verb mints past it.
- **The splice** (`live::splice::encode_from`): the live content written back into the file
  by splicing the difference into the carrier with the file verbs' code; an unchanged deck is
  the carrier byte for byte; a guard refuses a write that doesn't read back as the live
  content.
- **The carrier through the kit (K1)**: `PptxLive::wants_carrier` is true and
  `encode_from` splices into the carrier the kit hands it (the bytes it last read from the
  file or wrote to it, kept in its store), decoding it each time; the pack's in-memory carrier
  is gone. A deck writes back after a restart with no read of the file, and `file_at` of an
  earlier version is that version's deck byte for byte. `encode` refuses (no carrier: a deck
  isn't built from the doc alone). The codec settings are one format byte (a doc stored by
  an earlier, untagged build of the live pack holds a digest too, ignored).
  `live::splice::encode_from` takes the carrier alone (no decoded content). Rows:
  `tests/live_carrier.rs`.
- **One-pass splice**: `encode_from` opens the carrier's package once, decodes it on that
  open, applies every step on it (`verbs::Batch`: each step sees the parts, slides and notes
  slides the steps before it made) and writes the package once; the read-back guard runs once,
  on the final bytes. Before, each step reopened, rewrote and re-checked the package. A
  test-only counter (`live::splice::take_counts`, the `testgen` feature) and the row
  `row_one_pass_per_encode_from` hold one open, one decode and one guard per call; planted
  fault `SpliceReopensPerStep`.
- **`scripts/check.sh`**: the mutants run prints each plant and the summary with the test's own
  exit; the fuzz targets' build is a step; the private-names check is skipped with a printed
  `check-private: SKIPPED` line where no patterns are set, instead of ending the script.
- **Live verbs** (`live::verbs`, the kit's structured verbs): `slide list`, `slide show`,
  `shape show` (bounded and paged), `slide add`, `slide move`, `slide delete`, `slide hide`,
  `shape add`, `shape text` (a formatted shape needs `--drop-formatting`), `shape move`,
  `shape order`, `shape delete`, `notes set`; typed refusals with a next step.
- **The rendering-data contract r1.4's pack items** (`view`, `geom`, `render`): transform
  boxes in page space (group children by the true affine image, flips, `approx_box`,
  inherited placeholder boxes), the render hash over each slide's render closure, fonts,
  field kinds, `name_is_generic`, layout names and placeholder types; checked against the
  contract's goldens (`testgen::golden`).
- Planted faults: `GroupRotNotComposed`, `FlipRotAdded`, `ApproxNotInherited`,
  `HashMissesSlideNumber`, `HashMissesTableStyle`, `HashMissesLayout`,
  `HashMissesNothingButOverreaches`, `EncodeReserialises`, `FidReused`, `ReadUnbounded`,
  `ZOrderGlobal`. A fuzz target, `live`. M1, a measurement (`tests/m1.rs`, ignored).

### pptx tier 0.5, merged by shape

- Reading: slides by `sldId` in presentation order, shapes by `cNvPr` id (kind, name, transform,
  text, placeholder, alt text), masters and layouts, notes; typed refusals. The structure index
  of slides, shapes, notes, masters and layouts (`PptxPack::index`).
- The merge of an app's save by shape, on `k2-mpc-opc`'s package merge and `PartMerge`: slide
  lists by `sldId`, shape trees by `cNvPr` id, colliding slide and shape ids re-minted on ours'
  side with references rewritten; a shape both sides changed keeps the person's and holds the
  agent's (`ShapeConflict`, `Conflicts::take`); whole-file conflicts where nothing can be placed
  for sure. `PptxPack::merge3_bytes` with the stale-buffer guard until the kit's outside-save bridge
  supplies the app's base.
- **No silent loss through the kit: an app's save over a version the kit may have written
  keeps both.** The kit's call (`merge3_bytes`) keeps
  both whenever both sides changed the deck: the kit doesn't say whether it wrote the file
  since the last outside save, and an app's buffer from before an agent's write would read as
  the person undoing it. `PptxPack::merge3_since` takes that fact (`Since`); the kit's
  outside-save bridge plugs in there. A new mutant, `pack_no_kit_write_guard`.
- **A clash is a whole-file conflict until the kit holds conflicts durably.** A held clash
  lived in pack memory only, so a restart lost the clash record and its `conflict take`. Until
  the kit's durable held conflicts land, the pack returns a merge that would hold the agent's
  side as a whole-file conflict (the kit keeps both). The held path stays behind
  `HOLD_CLASHES` (off). A new mutant, `pack_holds_clashes`.
- Agent verbs as file-verb registry data and as splicing edits: `slide list`, `slide add`,
  `slide move`, `shape add`, `shape text`, `shape move`, `shape delete`, `notes set`,
  `conflicts list`, `conflict take`.
- Tests: read, refusals, round trips byte-exact in two zip layouts, merge rows, the 169-pair
  merge matrix against a rules oracle, verbs, mutants; the external checkers k2-mpc-ooxml
  shares (Open XML SDK validator, Apache POI) on every written file, with the reader's slide
  and picture counts; fuzz targets `open_index`, `merge3`, `verbs`.
- The repository: README, NOTICE, the licence gate at exact versions, the licence files check,
  the private-names check, `docs/checkers.md`.
