# Changelog

## v0.1.0: pairs with kit v0.21.0

The first release of the fabrication-output pack. It builds with **`k2-mpc-base` v0.21.0**
checked out beside this repo (a host vendors both side by side: `vendor/k2-mpc-base` beside
`vendor/k2-mpc-fab`); `Cargo.lock` and `fuzz/Cargo.lock` name that kit. It reads photoplot files
and drill files, and reads common drill exports. Nothing here writes a photoplot or drill file,
and agents can't write one through it.

- **What a host gets:** `PhotoplotPack`, `DrillPack` and `CommonDrillPack`, the kit's tier-0 blob
  packs, each with a structure index (index names `photoplot`, `drill`, `drill-common`; format
  2). The kit keeps each version of the file; the pack indexes it for comments, diffs and blame.
- **What an agent gets:** the kit's index verbs on these files: `feature list`, `feature show`,
  `feature diff`, `feature blame` on a photoplot file; `hole list`, `hole show`, `hole diff`,
  `hole blame` on a drill file. They are the kit's verbs: with `--json` they print the kit's own
  JSON, and a file a reader refuses is the kit's index refusal (`MPC-E711`, `MPC-E714`) with the
  reader's code and section as its reason. The pack registers no refusal ids of its own.

### The photoplot reader

- `photoplot`: a strict reader for the current revision (2026.05) of the photoplot format's
  specification, into a model of the unit and format, apertures (standard, macro, block), macros
  (parsed, and evaluated and checked for each aperture's values), objects (draws, arcs, flashes,
  regions with their contours) with their polarity, aperture, transform, attributes and container,
  step and repeats, and file attributes. Coordinates are integers in the file's units; decimals
  are exact. Refusals are typed (`error::Code`) with the section they enforce and a byte offset.
- Deprecated constructs read only as the deprecated section describes them, with typed warnings
  (`error::Deprecated`); those it lets a reader leave out (incremental notation, single-quadrant
  arcs, image transforms other than the default) are refused. Unknown commands are warned and
  skipped and the file reported as not conforming (`error::Conformance`).

### The drill readers

- `drill`: a strict reader for the XNC subset (revision 2021.11): header and unit, tools (two
  digits, a positive diameter), drill hits, rout paths (lines, and arcs of at most half a turn),
  and attributes in standard comments. Lines outside the subset are refused with their kind
  (`error::OutsideXnc`). Coordinates are kept exactly in millionths of the unit; more decimals
  are refused, never rounded.
- `drill::common`: reads common drill exports, as a reading separate from strict XNC: the
  constructs `docs/common-drill.md` documents (each with its permissively licensed samples, by
  reference, and the rule), and nothing else (`error::Code::NotCommon`). Every file it reads is
  reported not conforming, with the constructs it relied on (`CommonDrill::relied`). A coordinate
  without a decimal point reads only when the header fixes the number format in one of three
  documented ways, else it is refused (`error::Code::FormatNotFixed`), never guessed.
  `CommonDrillPack` reads strictly first, then as a common export; its index carries the verdict,
  the number format, `relies.<construct>` counts and the strict reader's refusal.

### Limits

- Both readers work under resource limits (`Limits`): size, objects, expansion, points,
  apertures, macros, expression depth, macro work, block nesting and attribute states; a file
  over one is declined with a typed code.

### The structure index

- Families of apertures, macros, objects by aperture and by attribute value, regions, step and
  repeats, tools with their hits and rout paths, and file attributes; counts and exact point
  bounds.
- **Apertures and tools are keyed by their definitions**, never by the numbers an exporter
  reissues: an aperture by its template (a macro by name), parameters and `.AperFunction`
  (`@ap:circle,0.25;Conductor`), a block aperture by a digest of what it holds
  (`@ap:block,<16 hex>`), a tool by its diameter and `.AperFunction` (`@tool:0.3;ViaDrill`); two
  identical definitions take `~2`, `~3` in number order. Their objects, hits and rout paths take
  the same key (`@objects:…`, `@hits:…`, `@routs:…`). The number is the node's name (`D10`,
  `T01`) and its `number` attribute (an objects group's `aperture`). So a fresh export that
  renumbers them keeps every key, and its diff shows only the new names and numbers.
- **Digests hold what a node means**: an object's shape, polarity, aperture by key, transform and
  attributes by value; a hit's or rout's points, tool by key and attributes; never a byte offset,
  a number or a list position. Within a family the units are in key order.

### Tests

- Rows for each reader's rules and refusals, the index of a hand-written board and drill file,
  and the packs inside a kit session (through the kit's job, in process and on the worker's wire
  form).
- **Outside saves** (`tests/outside_saves.rs`, `tests/support/saves.rs`), in a kit session kept
  in sync with the file: the same bytes written again make no version and move no blame; a
  fresh export that renumbers every aperture keeps every key and only renames; one that swaps
  two apertures' numbers keeps each definition on its key; a person's write through the kit
  between two outside saves is credited with what it changed.
- **Planted bugs** (`--features mutants`, `tests/mutants.rs`), each caught by its row, among them
  `KeyByNumber` (keys by number, which the swap row fails), also run alone
  (`key_by_number_alone`).
- Fuzz targets for the readers (`fuzz/`, `publish = false`, never shipped), their properties in
  `fuzzing` (feature `fuzzing`, test-only).
- **The public corpus** by reference (`tests/corpus/manifest.tsv`: source, licence, sha256;
  never committed): `scripts/public-corpus.sh [DIR]` fetches it (`scripts/fetch-corpus.sh`) and
  runs its test by name; without the files that test fails, so the gate leaves it out
  (`#[ignore]`d).
- **The target scan:** `scripts/targets.txt` lists every test target of the pack and its fuzz
  crate, every feature of their manifests, and the gate part that runs each, or why none does;
  `scripts/check-targets.sh` checks it with no build, and `scripts/test-check-targets.sh` plants
  one case of each in a copy of the tree.
- Repo checks: `scripts/check.sh` (formatting, lints, tests, licences, the licence files, no
  internal hostnames, no internal labels).
