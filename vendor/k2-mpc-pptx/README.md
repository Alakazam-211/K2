# k2-mpc-pptx

Reads and writes pptx packages for the multiplayer kit (`k2-mpc-base`), so a person saving from
their presentation app and agents working through verbs edit the same deck: merged slide by
slide and shape by shape, never last-writer-wins. Tier 0.5: no live editor and no engine. A
`.pptx` gets the kit's tier 0 (presence, a lease, versions, restore, conflict copies), and this
pack adds:

- **Reading** (`deck`, `slide`): slides in presentation order by their own `sldId`; shapes per
  slide by their own `cNvPr` id, with kind, name, position and size, text, placeholder and alt
  text; masters and their layouts (carried, read only); speaker notes. A file this pack can't
  hold is declined with a typed code (macros, Strict OOXML, a template, a bad slide list, a
  slide without a shape tree...) and stays at tier 0.
- **The structure index** (`index`, `PptxPack::index`): `@sld:<sldId>` per slide,
  `@sld:<sldId>/sp:<id>` per shape (groups nested, front to back), `@notes:<sldId>`,
  `@master:<id>`, `@layout:<id>`. The kit's index verbs read it as `shape list`, `shape show`,
  `shape diff` and `shape blame`.
- **The merge of an app's save** (`merge`, `PptxPack::merge3_bytes`), on the shared package
  layer (`k2-mpc-ooxml`'s `k2-mpc-opc`) and its `PartMerge` hook. See below.
- **Agent verbs** (`verbs`): definitions as data for the kit's file-verb registry (the host
  wires the command line), and the edits themselves, which splice only what they change.

The package layer underneath reads a package only if it can write it back byte for byte, and
copies every untouched entry's records raw; this pack splices inside the parts it changes and
never re-serialises XML.

## Live (a deck as a live keyed doc)

`PptxLive` (`live`) is the kit's `Pack` for a deck on the kit's structured live mode, so a
person can watch an agent's verbs land. The host turns it on per type; a deck it declines
stays at tier 0.5.

- **The store:** `slides` and `shapes` (MovableLists: existence and order; a slide's z-order is
  the subsequence of its shapes), `slide_props` and `shape_props` (one field per entry:
  `geom` is one field, `x,y,w,h,rot,fh,fv`), and `deck` (the page, layout facts, id marks).
  Masters, layouts, themes, pictures, tables, charts and animations stay in the file. Keys
  are the file's own ids: `s<sldId>`, `s<sldId>-<cNvPr>`.
- **New ids** are fixed inside the verb's change: one past the higher of the slide's id mark
  (the highest `cNvPr` in its whole tree, a group's children included) and every id the
  store holds for it, so a new shape never takes a group child's id.
- **Writing the file:** the live changes are spliced into the **carrier**, the bytes the kit
  last read from the file or wrote to it (`live::splice::encode_from`, through the kit's
  carrier hook: `wants_carrier` is true), every other part a raw record copy; an untouched
  deck writes back byte for byte, and a write that wouldn't read back as the live content is
  refused. One pass per write: the carrier's package is opened and decoded once, every change
  is applied on that open, the package is written once and read back once. The kit keeps the
  carrier in its store, so a deck writes back after a restart with
  no read of the file, and `file_at` gives an earlier version's deck byte for byte. The pack
  holds no file bytes; with no carrier, `encode` refuses (a deck isn't built from the doc
  alone).
- **Verbs:** `slide list`, `slide show`, `shape show` (bounded: 64 shapes a page, text cut at
  1,024 bytes; `shape show` at 16,384), `slide add`, `slide move`, `slide delete`, `slide hide`,
  `shape add`, `shape text` (a shape with more than one format needs `--drop-formatting`),
  `shape move`, `shape order`, `shape delete`, `notes set`. An agent's edit names `--base`;
  refusals print `refused: <code>` and a `next:` verb.
- **Declines:** `live_ids` (a shape without an id, or one id twice on a slide),
  `live_text_too_large` (over 32 KiB), and the kit's keyed cap (`live_too_large`).
- **For the renderer and the view** (`view`, `geom`, `render`): each unit's transform box in
  page space (group children composed, flips, `approx_box`, boxes inherited from the layout),
  a render hash per slide over its render closure, fonts, field kinds, generic names, layout
  names and placeholder types.

## The merge, by shape

`ours` is the doc's current version (where agents' writes land), `theirs` what a person's app
saved, `base` what both started from. The rules below are the merge's (`merge::merge3`, and
`PptxPack::merge3_since` for a caller that knows the save's base). **Through the kit's call
today, a save both sides changed is a whole-file conflict** (Stated limits, the base).

| Ours (the agent) | Theirs (the person) | Result |
|---|---|---|
| edit shape A | edit shape B, same slide | both |
| edit shape A | edit shape A (text, position, anything) | the person's in the file; the agent's **held** |
| move or edit A | delete A | deleted; the agent's held |
| delete A | edit A | the person's kept; the agent's delete held |
| add a slide | add a slide | both land; ours gets the next `sldId`, part name and relationship id |
| add a shape | add a shape, same slide, same id | both land; ours gets a new id, with its connectors and animations following |
| reorder slides or shapes | edit, add or delete | the agent's order, with the person's changes in it |
| reorder | reorder, differently | whole-file conflict |
| edit a slide | delete that slide | whole-file conflict |
| change a slide's timing, transition or background | change it too | whole-file conflict |
| change a master, layout or theme | change it too | whole-file conflict |
| make notes for a slide | make notes for it too | whole-file conflict |
| anything | `docProps/*`, `ppt/viewProps.xml` both changed | the person's (metadata and view state) |

- **Held** means the merged file keeps the person's shape (their app may still have the deck
  open) and the pack keeps the agent's as a `ShapeConflict` (`PptxPack::conflicts`): `conflicts
  list` shows it, `conflict take ID --agent` writes it only if the file still holds the
  person's, `--person` drops it. **Off for now** (`HOLD_CLASHES`): that record lives in pack
  memory only and a restart would lose it, so until the kit's durable held conflicts land, the
  pack returns a held row as a whole-file conflict, and `conflicts list` shows nothing. The
  held path is kept as it was, to turn back on once it moves onto the kit's tables.
- **Whole-file conflict** means the kit keeps both: the save stays current, and what it replaced
  is a version and a conflict copy. Nothing is lost.
- A shape is the unit of an app's save: a shape both sides changed is never merged inside. Finer
  merging (one property, one text range) comes through targeted verbs and, later, live editing.
- Each shape in a merged slide is the bytes of the side it came from (with only re-minted ids
  rewritten); every part one side changed is that side's raw record.

### Stated limits

- **The base.** The kit hands the pack what the file last held, not the version the app opened,
  and doesn't say whether it wrote the file since the last outside save. An app's buffer may
  predate an agent's change the kit wrote to the file, and a merge from what the file holds
  would read that save as the person undoing the change, and drop it. So until the kit's
  outside-save bridge supplies the app's base, the kit's call (`merge3_bytes`) keeps
  both whenever both sides changed the deck, and when `base == ours` (as in the docx pack).
  The merge runs for a caller that knows the kit wrote nothing since the last outside save
  (`PptxPack::merge3_since` with `Since::NoKitWrite`), which is where that bridge plugs in.
- **Renumbered parts.** A save in which a slide's part holds a different slide (an app that
  renumbers its parts on save) is a whole-file conflict, never a mis-pairing. Whether apps do
  this is to be measured; pairing by `sldId` across renamed parts would come later if they do.
- **Sections.** A deck with sections merges slide adds and removes only when one side changed
  the slides; `slide move` is refused on a deck with sections.
- **Groups** are one shape: a change inside a group is a change of the group.
- New ids are one past the highest the deck shows; ids an app retired and doesn't show can't be
  seen, which is why the merge re-mints on any collision.
- After an agent changes text, an autofit scale an app computed is stale until the app re-fits;
  `docProps/app.xml` counts are the person's; thumbnails go stale.

## Verbs

| Verb | What |
|---|---|
| `slide list [--shapes]` | slides (address, position, layout, title, hidden, notes) and their shapes |
| `slide add --layout NAME\|@layout:ID [--after @sld:ID \| --first]` | a slide from a layout's placeholders (not date, footer or number) |
| `slide move @sld:ID (--after @sld:ID \| --first)` | the slide list entry only |
| `shape add @sld:ID --kind textbox --at X,Y,W,H --text T` | a text box in front of the others |
| `shape text @sld:ID/sp:N --text T` | the whole text body, one paragraph per line, the first paragraph's and run's formatting kept |
| `shape move @sld:ID/sp:N [--x] [--y] [--w] [--h]` | the transform's values only (EMU) |
| `shape delete @sld:ID/sp:N` | one shape (an animated or connected one is refused) |
| `notes set @sld:ID --text T` | the notes body, making the notes slide from the notes master if needed |
| `conflicts list`, `conflict take ID --agent\|--person` | the merge's held clashes (none while `HOLD_CLASHES` is off) |

## Tests and checks

Every check runs with cargo and a stable Rust toolchain (1.88 or later), with the kit and the
package layer checked out beside this repo, where `Cargo.toml`'s path dependencies point:
`../k2-mpc-base` at the kit's v0.23.0 (`905515d9`, its public cut) and `../k2-mpc-ooxml` at
its v0.1.2 (`306d3938`, its cut).

```sh
sh scripts/check.sh            # every step below; exits non-zero on the first failure
# the main steps by hand:
cargo fmt -p k2-mpc-pptx -- --check
cargo clippy --locked -p k2-mpc-pptx --all-targets -- -D warnings
cargo test --locked -p k2-mpc-pptx --all-targets
cargo test --locked -p k2-mpc-pptx --features mutants --test mutants -- --nocapture
```

`scripts/check.sh` runs fmt, clippy with `-D warnings` (the mutants feature too), the tests, the
mutants, the fuzz targets' build, the licence gate, the licence files and the private-names
check; its exit is the checks' exit, and its last line, `repo checks: ok`, prints only when all
passed. The private-names check's patterns live off the tree: where none are set it prints
`check-private: SKIPPED` and must be run where they are.

- **Our own generated decks are the fixtures** (`testgen`, test-only): synthetic names and text,
  every shape kind, notes, animations, sections, compact and pretty-printed. The generator's
  model is the truth the reading rows compare against.
- **Read and round trip** (`tests/read.rs`): slides, shapes, layouts, notes and the index read
  back; every generated deck, stored and deflated by a second writer (the `zip` crate), writes
  back byte for byte, with a second reader agreeing; every shape tree splits back to its bytes.
- **Refusals** (`tests/refusals.rs`): each malformed input with its code; no damage panics.
- **Merge** (`tests/merge.rs`) and **the merge matrix** (`tests/matrix.rs`): every pair of 13
  agent edits and 13 person edits (169), against an oracle written from the rules above: both
  land, the person's lands and the agent's is held at the predicted address, or a whole-file
  conflict; every result opens, indexes and reads back with each side's edit. Through the pack,
  every held pair is a whole-file conflict and every other pair the same answer. The rows
  for the base: a stale save after a kit write keeps both through the kit's call.
- **Verbs** (`tests/verbs.rs`): each verb's effect, the parts it changes and nothing else (every
  other part's raw record in every fixture), ids, refusals and exits.
- **Mutants** (`--features mutants`, `tests/mutants.rs`): planted bugs in the merge and the pack,
  each caught by its leg.
- **Live** (`tests/live.rs`, `live_verbs.rs`, `live_reads.rs`, `splice.rs`, `live_carrier.rs`): a
  deck opens live and reads back byte for byte; each verb's ops and refusals; the agent's base; the
  differential oracle (live verbs written back equal the file verbs on the same file); every kind of
  change spliced with every other part its raw record; bounded reads. Through the kit's carrier: an
  untouched deck byte for byte by the kit's write-back, a restart then a verb with no read of the
  file, `file_at` of an earlier version byte for byte, an outside save then a verb spliced into the
  save, and the pack contract's byte-exact row: the kit's judge (`test_support::carrier_judge`) over
  every fixture, stored and deflated, and the golden deck. `scripts/check.sh` runs the kit's
  `carrier-row.sh`, which fails a pack that keeps a carrier and has no such row.
- **What a renderer reads** (`tests/view.rs`, `render.rs`): the view index and the render hash
  against the hand-made goldens, on a generated copy of the golden sample deck. The kit's view index
  (`tests/kit_view.rs`): the live tier against the expected index kept in the test, with no gap (the
  carrier's items: fonts, render hashes, field kinds, inherited boxes, a group's children), the blob
  tier against the live one with no gap either (the structure index's view items, the kit's row
  format 2), N reads of an unchanged deck decoding the carrier zero times, a group move moving its
  children, and a keyed watch's change frame carrying the touched slide's new render hash.
- **The measurement** (`tests/measure.rs`, ignored): heap, verb p99, the agent's read and
  `slide show` time at 2,500 shapes, the view index's reads (the first, unchanged, after a
  move), the write-back's time, and the one-element list changes (`shape add`, `shape order`
  and `shape delete`, p50 and p99 of the verb alone), release build:
  `cargo test --release -p k2-mpc-pptx --test measure -- --ignored --nocapture`
  (`MEASURE_SLIDES=N` for another size; 50 by default).
- **External checkers** (`docs/checkers.md`): every file this pack writes (`tests/emit.rs`:
  fixtures, verb outputs, every merged matrix result) passes the Open XML SDK validator and
  opens in Apache POI with the slide and picture counts this pack reads, through the checkers
  `k2-mpc-ooxml` shares (`../k2-mpc-ooxml/checkers/run.sh`). Private test tooling, kept on
  `k2-mpc-ooxml`'s private main line and not shipped: test-only, hash-pinned, on a pinned
  open JDK.
- **Fuzz targets** (`fuzz/`, its own workspace, `publish = false`): `open_index`, `merge3`
  (structured, from the verbs), `verbs` and `live`. Private test tooling, kept on this
  repository's private main line and not shipped: a release tag is a cut commit that leaves
  `fuzz/` out, and nothing else here needs it.
- **The licence gate** (`scripts/check-licences.sh`): every crate in the locks, transitively, at
  its exact version: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib or Unicode-3.0 (libFuzzer's
  NCSA in the fuzz crate alone).
- `scripts/check-private.sh` keeps internal host names out of the tree; its patterns live
  outside it.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions.
