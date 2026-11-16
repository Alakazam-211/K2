# Changelog

## v0.1.8: pairs with kit v0.21.0 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.21.0** (its cut `9e6f6419`) and **`k2-mpc-ooxml` v0.1.1** (`k2-mpc-opc`
0.1.1, unchanged), each checked out beside this repo. As for v0.1.7, the tag is a cut commit that
leaves out the fuzz crate (`fuzz/`).

- Pairs with kit v0.21.0. Its pack API additions have defaults this pack keeps (`Pack::errors`,
  `Pack::check_merged_view`, `BlobPack::merge3_known`, `BlobPack::save_lineage`); its other
  changes are the host's (the version an app opened, a call's result as data, kept pairs and
  claim bytes on events). The kit's registry takes a deck's structured verbs under its new keyed
  tier (`Tier::Keyed`), so `capabilities` lists them; an outside save merged from an opened
  version keeps both, as with no pin, since this pack keeps `Pack::check_merged_view`'s default.
  So decks behave as in v0.1.7.
- `VERBS`' doc says the kit's registry takes these verbs under the keyed tier.
- **A row for the kit's registry** (`tests/registry.rs`): `Registry::for_pack` takes every deck
  verb under the keyed tier, and `capabilities` lists each one there.
- `Cargo.toml`'s comment and the README name the kit's v0.21.0 and its cut. `Cargo.lock` and
  `fuzz/Cargo.lock`: k2-mpc-base 0.21.0 and the crate version 0.1.8 (only those versions move);
  `k2-mpc-opc` stays 0.1.1.

## v0.1.7: pairs with kit v0.20.0 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.20.0** (its cut `475a1755`) and **`k2-mpc-ooxml` v0.1.1** (`k2-mpc-opc`
0.1.1, unchanged), each checked out beside this repo. As for v0.1.6, the tag is a cut commit that
leaves out the fuzz crate (`fuzz/`).

- Pairs with kit v0.20.0, whose changes are the host's: a store holding native-engine docs opens and
  refuses only those docs; writer IDs follow people, devices and agents (person-writer reuse on by
  default, used only when the host sends a device key); a writers cap per doc, off by default, with
  a near-cap audit notice. No pack API changes, so decks behave as in v0.1.6.
- `Cargo.toml`'s comment and the README name the kit's v0.20.0 and its cut. `Cargo.lock` and
  `fuzz/Cargo.lock`: k2-mpc-base 0.20.0 and the crate version 0.1.7 (only those versions move);
  `k2-mpc-opc` stays 0.1.1.

## v0.1.6: pairs with kit v0.19.0 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.19.0** (its cut `b8db699e`) and **`k2-mpc-ooxml` v0.1.1**
(`k2-mpc-opc` 0.1.1, unchanged), each checked out beside this repo. As for v0.1.5, the tag is a
cut commit that leaves out the fuzz crate (`fuzz/`).

- Pairs with kit v0.19.0. For this pack, the kit release adds: an element claim whose last
  element an outside save removed ends `gone_outside_save`, and a claim's end events carry its
  element addresses (by this pack's `address_of`); the keyed watch's `outside_save` and
  `verb_refused` frames for a live deck; new view-schema container fields (none set here; see
  below); and chunked tier-0 versions, off by default.
- **The view schema's container fields are built from `ContainerFields::NONE`**
  (`..ContainerFields::NONE`), setting the same fields as before, so a field the kit adds to
  `ContainerFields` (a container's parent, after kit v0.18.0) doesn't break the build. No change
  in what the pack declares. Every literal of `ContainerFields`, `BlobView` and `ViewSchema` in
  the pack is in that form now.
- Public text: expected values are in the tests, not in golden files a reader can't see; golden
  variant and case names as code spans; `scripts/check-labels.sh` also blocks parity-row, finding
  and run labels, branch names and engine codes.
- `Cargo.toml`'s comment and the README name the kit's v0.19.0 and its cut. `Cargo.lock` and
  `fuzz/Cargo.lock`: k2-mpc-base 0.19.0 and the crate version 0.1.6 (only those versions move);
  `k2-mpc-opc` stays 0.1.1.

## v0.1.5 (2026-11-12): pairs with kit v0.18.0 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.18.0** (its cut `efc6377e`) and **`k2-mpc-ooxml` v0.1.1**
(`k2-mpc-opc` 0.1.1, unchanged), each checked out beside this repo. As for v0.1.4, the tag is a
cut commit that leaves out the fuzz crate (`fuzz/`).

- Pairs with kit v0.18.0. Its changes for a pack: build the view index's structs with
  `..X::default()` (and the schema's parts with `..X::NONE`); a keyed doc's `MPC-E822` is
  `SessionError::KeyedAtLimit`; an optional per-pack keyed limit (`Pack::keyed_entries_default`);
  verb state, derived values per unit and placements in 3D, all additive with defaults.
- The kit's view structs this pack builds (a unit, its font uses, boxes, flips, texts and
  placeholders) take `..X::default()`, so a field the kit adds (a unit's derived values) doesn't
  break the build; the two files allow `clippy::needless_update` for it. The limits row checks
  the refusal's id (`MPC-E822`) through the kit's error mapping, then that it is the keyed limit
  (`SessionError::KeyedAtLimit`) from the host-wide limit. No behaviour change.
- `Cargo.toml`'s comment and the README name the kit's tag and its cut. `Cargo.lock` and
  `fuzz/Cargo.lock`: k2-mpc-base 0.18.0 and the crate version 0.1.5 (only those versions move).
- Wording only, no behaviour change: internal plan, requirement and ask labels in the docs,
  comments, test messages, `scripts/oss-check.sh` and this changelog are now plain words.
- Tests named by those labels are renamed for what they check (the same rows and asserts): the
  measurement `tests/m1.rs` is now `tests/measure.rs` (`--test measure`; `MEASURE_SLIDES=N` for
  another size; its lines start `measure:`), and the live rows' `f1_`, `f2_`, `f5_` and `b1_`
  prefixes are dropped (`f5_through_the_kits_write_back` is now
  `untouched_through_the_kits_write_back`).

## v0.1.4 (2026-11-12): pairs with kit v0.17.0 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.17.0** (its cut `0a78505a`) and **`k2-mpc-ooxml` v0.1.1**
(`k2-mpc-opc` 0.1.1, unchanged), each checked out beside this repo. The pack's behaviour is
unchanged from v0.1.3. As for v0.1.3, the tag is a cut commit that leaves out the fuzz crate
(`fuzz/`).

- Pairs with kit v0.17.0. Its changes: text → tier 0 (`demote_text_file`, the host's call for a text
  doc whose type now routes to tier 0), the carrier's pairing (opt-in by `Pack::wants_pairing`, new
  error `MPC-E521`), the live grid (wire 3, additive), and an outside save on unwritten changes
  keeping both on a keyed doc that isn't a table. This pack doesn't pair (its shape ids are the
  file's own), so its decks render as in v0.1.3. One kit rule reaches a deck: an outside save while
  the deck holds changes the file doesn't now keeps both (the save becomes the live deck; the kit's
  state a version and a conflict copy), where v0.1.3's kit merged it by key.
- The deck's view schema (`live::view_schema::VIEW`) ends with `..ViewSchema::NONE`, so it
  builds with kit v0.17.0's two new `ViewSchema` fields (`field_class_prefix`, `grid`) at
  their defaults, and with fields the kit adds later. No behaviour change.
- Two internal labels in doc comments and this changelog are now plain words.
- `Cargo.toml`'s comment and the README name the kit's tag and its cut. `Cargo.lock` and
  `fuzz/Cargo.lock`: k2-mpc-base 0.17.0 and the crate version 0.1.4 (only those versions move).

## v0.1.3 (2026-11-12): pairs with kit v0.16.0 and k2-mpc-ooxml v0.1.1

**Requires `k2-mpc-base` v0.16.0** (its cut `d2245444`: the view schema, addresses and view
index, the derive hook with the carrier, the keyed watch's `Pack::watch_extras` with its view
argument, and the structure index's row format 2) and **`k2-mpc-ooxml` v0.1.1** (`k2-mpc-opc`
0.1.1, unchanged), each checked out beside this repo. As for v0.1.2, the tag is a cut commit
that leaves out the fuzz crate (`fuzz/`).

- **The kit v0.16.0:** `Cargo.toml`'s comment and the README name the tag and its cut.
  `Cargo.lock` and `fuzz/Cargo.lock`: k2-mpc-base 0.16.0 and the crate version 0.1.3 (only
  those versions move; k2-mpc-opc stays 0.1.1).

### The blob tier's view items (index format 2)

**Requires the kit's row format 2** (kit v0.16.0: `NodeView`, `IndexSpec::with_older_format`).

- **The index reader carries the view items** (`index::draft`): what the live tier reads from
  the carrier, worked out where the file is parsed (the host's index worker), by the same code
  the live tier's derive hook uses (`view::read_open`, `DeckRead::index`, `view::place`), so the
  two tiers agree by construction: the deck's fonts; per slide its render digest (keyed by the
  kit), its fonts and field kinds; per shape its box on the page (a group's children composed
  through the group, a placeholder's box inherited from its layout or master), rotation, flips,
  `approx_box`, field kinds, the text a view shows (a table's rows tab-separated, a slide
  number at the slide's position) and `editable_text` (`plain`, `formatted` or `never`, as
  `live::model::text_edit` gives the live store's `edit`; a group's child `never`). A shape with
  no id, or with one another shape on its slide shares, gets none, and so does a deck the view
  can't read: fewer details, never another shape's. `view::ShapeRead::edit` is new.
- **Index format 2** (`index::FORMAT`), reading format 1's rows (`index::OLDER_FORMAT`, the same
  nodes) with the items absent until a version is indexed again
  (`IndexSpec::with_older_format`).
- `tests/kit_view.rs`: the blob tier's index of the golden deck is the live tier's with no gap
  listed (each tier's render hashes as the golden's labels, since each keys them under its own
  key), and the draft's raw render digests are the pack's own view's. The gaps it listed before
  (fonts, render hashes, field kinds, the 7 inherited boxes, `@sld:257/sp:5` and `sp:6`'s
  composed boxes and rotations, `sp:7`'s flip, `editable_text` on all 12 units, the table's
  text) are closed.
- **What changes for K2:** re-indexing old versions is optional: the kit offers a job for a
  version whose index is of format 1 (`BlobSession::index_job`), and until it runs those
  versions show fewer details in the blob tier's view (no fonts, render hashes, inherited or
  composed boxes, flips, field kinds or `editable_text`; a table's text one cell per line),
  never wrong ones.

### The deck's view schema, addresses and the live view from the carrier

**Requires the kit's view index** (kit v0.16.0: `Pack::view_schema`, `Pack::address_of`,
`k2_mpc_base::view_index`).

- **The deck's view schema**: `live::view_schema::VIEW`, which `PptxLive` and `PptxPack` both
  give the kit. Slides are the containers and shapes the units; a shape's `slide` field holds
  its slide's `sid`; the page is the deck map's `cx` × `cy` in EMU; `geom` is the one geometry
  field; `edit` gives `editable_text` (`1` plain, `fmt` formatted, `0` never). On the blob tier
  the structure index's `slide` nodes are the containers and every node under one but `notes` a
  unit. The kit builds a deck's `ViewIndex` from it on both tiers.
- **Addresses**: `@sld:<sldId>` for a slide and `@sld:<sldId>/sp:<cNvPr>` for a shape
  (`view_schema::address_of` and `key_of`, the structure index's own keys), so the live store's
  keys never reach the wire. `PptxLive::container_of` names a shape's slide by its key, the kit's
  hint before it checks the shape's `slide` field: a range on a shape, an agent's claim and its
  verbs now focus the shape's slide (before, the kit's default focused the shape itself).
- **What the pack derives on the kit's index** (`view_schema::derive`): each slide's title, the
  layouts (from the deck map), `name_is_generic`, a placeholder with no type as `obj`.
- `tests/kit_view.rs`: the kit's index of the golden deck against the hand-made expected index,
  field by field (the expected index is kept in the test, `tests/common/kit_view_golden.rs`), and
  the blob tier's against the live tier's; any difference not listed fails.
- **The live view reads the carrier** (`live::view_derive`; needs the kit's carrier hand-off,
  `ViewSource::Live(state, carrier)`, and `Pack::watch_extras`' view argument, kit v0.16.0):
  what only the file's bytes hold is set on the kit's live index,
  so the live index of the golden deck is the expected index with no gap left (render hashes
  compared as the golden's labels). From the carrier, decoded once per carrier
  (`ViewCarrier::decoded`, the pack's `view::read_open` and the live model of it):
  - a group's children (`live: false`), placed by `view::place`, the code that places every
    shape of the pack's own view of a file, from the group's **live** box: a group the store
    moved or resized moves or scales them (the true centre);
  - the box a placeholder inherits from its layout, else its master, while the store holds none
    (`bbox_inherited`), a new slide's from its layout;
  - field kinds (`fields`, `has_fields`), and a slide number's text at the slide's live
    position;
  - fonts, per slide and per deck: a shape's from the carrier while its text is the carrier's,
    its style's and first run's format's once the store changed its text, the layout's
    defaults for a shape the file doesn't hold yet;
  - render hashes: the carrier's render digest for a slide the store didn't change as drawn (the
    digest over the written bytes), else a digest over the carrier's id and the slide as the
    store holds it, which changes with every change to the drawing. The rule:
    the live store wins, and the carrier supplies only what the store doesn't model.
- **`view` in two phases:** `view::read_open` reads a file into a `DeckRead` (each shape as the
  file holds it, each slide's render digest) and `DeckRead::index` places it (`view::place`);
  `view::index` is the two, unchanged in what it gives.
- **Change frames** (`PptxLive::watch_extras`): a keyed watch's `derived` holds, for each slide
  a change touched, its derived values after it (title, fonts, field kinds, unit count) and its
  units carrying anything derived (a group's children, an inherited box, field kinds); the kit
  adds the render hashes that changed.
- Rows in `tests/kit_view.rs`: N reads of an unchanged deck decode the carrier zero times (one
  decode per carrier, `live::view_derive::take_decodes`, test-only); a group the agent moved
  moves its children, unwritten and written, as the pack's view of the written file places
  them; a change frame carries the touched slide's new render hash. The blob tier's gaps were
  listed there: its structure index carried no fonts, render hashes, field kinds, inherited
  boxes, composed children's boxes, flips, `edit`, and a table's text one cell per line (closed
  by index format 2, above).
- The measurement (`tests/m1.rs`) prints the view index's reads: the first (the carrier
  decoded), 100 unchanged (kept) and 50 each after a move (built from the kept decode).

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
- The measurement's one-element list lines print one bracket each: `shape add (the verb alone;
  a text box in front of a slide's others), ...`.
- **The carrier row, the kit's** (the pack contract for a pack that keeps a carrier):
  `tests/live_carrier.rs`, `carrier_judge_byte_exact_over_the_corpus`, runs
  `session::test_support::carrier_judge` over the byte-exact corpus (every fixture, stored and
  deflated, and the golden deck; `splice_rows::corpus`, which the pack's own untouched row now
  shares) and requires a byte-exact verdict for every file: the hook, the file opened live
  through the kit, and the same after a restart. The planted `EncodeReserialises` is caught by
  it too (`tests/mutants.rs`). The dev-dependency on the kit turns on its `test-support`
  feature (test builds only). `scripts/check.sh` runs the kit's
  `scripts/pack-checks/carrier-row.sh` after fmt, which refuses a carrier pack without the row.
- **The measurement times the one-element list changes** (`tests/m1.rs`): on the 2,500-shape
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

**Requires `k2-mpc-base` v0.15.0** (the first kit tag with the carrier hook, the keyed
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
  the carrier hook (`Pack::wants_carrier`, `Pack::encode_from`; version carriers; keyed
  structured reads that render nothing).
- **`PptxLive`** (`live`): the kit's `Pack` for a live deck. Two MovableLists (`slides`, `shapes`)
  for existence and order, two composite maps (`slide_props`, `shape_props`) of one field each, a
  `deck` map of facts and id marks. Keys are the file's own ids (`s<sldId>`, `s<sldId>-<cNvPr>`),
  for imported elements and for ones a verb adds. Geometry is one field (`geom`). Declines
  `live_ids` and `live_text_too_large`; the kit's cap gives `live_too_large`. Outside saves
  reconciled by key. **Id marks:** a slide's id mark is the highest `cNvPr` in its whole tree, a
  group's children included, and a live verb mints past it.
- **The splice** (`live::splice::encode_from`): the live content written back into the file
  by splicing the difference into the carrier with the file verbs' code; an unchanged deck is
  the carrier byte for byte; a guard refuses a write that doesn't read back as the live
  content.
- **The carrier through the kit**: `PptxLive::wants_carrier` is true and
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
- **The view items a host's renderer reads** (`view`, `geom`, `render`): transform
  boxes in page space (group children by the true affine image, flips, `approx_box`,
  inherited placeholder boxes), the render hash over each slide's render closure, fonts,
  field kinds, `name_is_generic`, layout names and placeholder types; checked against the
  hand-made goldens on the golden deck (`testgen::golden`).
- Planted faults: `GroupRotNotComposed`, `FlipRotAdded`, `ApproxNotInherited`,
  `HashMissesSlideNumber`, `HashMissesTableStyle`, `HashMissesLayout`,
  `HashMissesNothingButOverreaches`, `EncodeReserialises`, `FidReused`, `ReadUnbounded`,
  `ZOrderGlobal`. A fuzz target, `live`. A measurement (`tests/m1.rs`, ignored).

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
