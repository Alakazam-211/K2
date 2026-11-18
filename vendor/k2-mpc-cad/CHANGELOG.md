# Changelog

## v0.4.0: pairs with kit v0.22.0

**Requires `k2-mpc-base` v0.22.0**, checked out beside this repo, as v0.3.0 does; no kit change.
The crates are 0.4.0; `Cargo.lock` and `fuzz/Cargo.lock` name that kit. A minor bump: two
changes are breaking under 0.x (below). **Unchanged:** the structure index's format (4, nothing
to re-index), the schema tags (`cad.drawing.header/1`, `cad.drawing.page/1`, `cad.change/1`)
and the verbs' argument parsing.

### Breaking
- **A DWG `draw page` row's `edit` is `read-only`.** v0.3.0 gave a DWG row the edit word the
  model's facts gave it, as a DXF row has; DWG is read-only (no edit verb changes a DWG file),
  so every DWG row now says `read-only`, a new word of the closed set: `EDIT_WORDS` has 19 words
  (18 in v0.3.0). `cad-measure dwg-draw-compare` names a DWG row's `read-only` edit as a known,
  format-specific item (counted). Row `dwg_rows_read_only`, plant `DwgEditNull`.
- **`k2-mpc-dwg`'s Rust API: public fields added to structs a caller can build.** A struct
  literal of these outside the crate no longer compiles: `entities::Entity::extra_handle_bits`,
  `entities::MTextData::redundant`, `tables::Entry::state`, `header::HeaderVars::clayer` and
  `header::HeaderVars::extra_string_bits`. `cargo-semver-checks` against v0.3.0 reports them
  (`constructible_struct_adds_field`).

### Fixed: the DWG reader
- **An external reference's block record was left out of the drawing.** An external
  reference's block record (attached or overlaid) owns no entities and writes no count of them.
  v0.3.0 read a count for every block record, so from such a record's base point on every field
  was off, the record missed its data's exact end, and the drawing left it out: the external
  reference was missing from the blocks (its name, kind, base point and path) and from the
  drawing's external references, and an insert of it named no block. The file still opened.
  Now the count is read only for a record that is not an external reference. Row
  `xref_block_records` (a 2018 attached and a 2004 overlaid reference), plant
  `XrefOwnedCountRead`.

### Changed: the DWG reader reads every field and handle the format names
- **A 2007 and later table entry's short** after its name is read as a short (`Entry::state`),
  where v0.3.0 read two bits (the same bits for the short's two-bit forms, 0 and 256; any other
  value would have missed the entry's exact end). From 2007, a layer's xref dependence is read
  from its entry state (bit 256) as well as from a `|` in its name. Rows `entry_states`,
  `entry_state_dependent`; plants `EntryStateAsTwoBits`, `EntryDependentBitIgnored`.
- **Whole handle streams**: a 2013 and later layer's visual style, a dimension style's
  arrowheads (and from 2007 its linetypes), a viewport's coordinate systems (and from 2007 its
  background, visual style, shade plot and sun), a 2018 multiline text's application and an
  image's or wipeout's reactor are read. Data past them is never refused;
  `DwgDrawing::tolerated()` counts the objects that carry it. Rows
  `layer_without_visual_style_refused`,
  `dimension_styles`, `hatches_dimensions_viewports`; plants `LayerVisualStyleSkipped`,
  `TrailingHandlesSkipped`, `EntityTrailingHandlesSkipped`.
- **The header's texts from 2007** include `MENUNAME`, read in its place, so every later text
  is the one named; texts past the format's are tolerated and counted
  (`HeaderVars::extra_string_bits`). Row `header_variables_2018`, plant
  `HeaderMenuNameSkipped`.
- **What the stricter reads refuse.** A table entry whose handle stream stops before the
  handles the format names is refused cut short (`DWG-E002`): a layer, linetype, text style,
  registered application or block record so refused is left out of the drawing, and a
  dimension style keeps its name alone (without its arrowhead settings); an entity so refused
  keeps its common data alone (without geometry). **A 2007 and later header whose string
  stream stops before the texts the format names refuses the whole file** (`DWG-E002`).
- The rows' hand-packed objects are packed to these layouts; the rows whose inputs or
  assertions changed are listed in the commit that changed them.

### Added
- **The read report counts what the drawing leaves out or keeps in part**, so a host can say
  so: `DwgDrawing::tolerated()` gives `entries_left_out` (registered applications, linetypes,
  text styles, layers and block records whose read refused, and dimension styles whose name
  doesn't read either), `dimstyles_name_only`, `entities_common_only` and `entities_left_out`
  (an entity whose common data refused too), beside the data past the format it already
  counted. Rows `left_out_entries_counted`, `fixture_read_report`; plant `LeftOutUncounted`.
- **The draw header, for block insert and adds.** Each of `draw header`'s `blocks` states
  `dynamic` and `annotative` (`true`/`false`, judged exactly as `block insert` refuses them: one
  function, `DXF-E044` and `DXF-E051`), and `attdefs`: its attribute definitions in the file's
  order, `{tag, default, constant, prompt}` (an insert form asks for the settable ones). Row
  `header_blocks_match_block_insert`, plant `HeaderDynamicMissed`.
- The header states `current_layer`: `$CLAYER` by the layers list's address (`@h:<HEX>`), the
  layer `entity add` and `block insert` put an entity on when no `--layer` is named; null when
  the file names none. Row `entity_add_lands_on_current_layer`, plant `CurrentLayerAsZero`.
- **The draw header from a DWG file states what a DXF file's does:** `current_layer` (the
  header's `CLAYER`), each block's `dynamic` (from the model), `attdefs` (its attribute
  definitions in its own order) and `annotative` (its record's, its `BLOCK`'s or a settable
  attribute definition's `AcadAnnotative` extension data, its second short not 0), as the DXF
  read judges them. Rows `header_variables_2018`, `attdef_infos`, `block_annotative_mark`;
  plants `CurrentLayerUnread`, `AttdefsUnread`, `AnnotativeEedIgnored`.
- A negative value is a value: `entity rotate --by -90` (and `--by=-90`) through the kit is a
  turn of -90 degrees, the same bytes as `--by 270`; neither the kit nor the verb takes `-90` for
  a flag (the kit's parsing, unchanged; a row now holds it). Row
  `rotate_negative_through_the_kit`.
- `block insert` now judges a block's annotative mark before its attribute definitions'
  multi-line and field refusals.

### Tests and notices
- One DWG fixture: `crates/dwg/tests/data/draw_sampler.dwg` (the sampler's own drawing as a
  2018 DWG, 11852 bytes), made once by our own test tooling, not by any vendor's software, and
  cleared by its sha256; `MANIFEST.md` beside it and `PROVENANCE.md` say what it is and where it
  came from, and `NOTICE.md` names it. Row `dwg_fixture_draws_as_its_dxf`
  (`crates/cad-measure/tests/dwg_fixture.rs`) reads it and compares its drawing with the
  sampler's DXF: every row matched, no difference, the known items a viewport's on flag and
  every row's `read-only` edit. Nothing in this repo writes DWG.
- Test tooling: `cad-measure dwg-strict [--detail] FILE...` reads every object to its data's
  end and through its handle stream and prints counts only, each file named by its place in
  the arguments (`F1`, `F2`, ...), never by its name: what each file's reads tolerated and left
  out; for each 2007 entry short, the entries with each value and how many of them have a `|`
  in their name; the block records read as external references; and a failing block record's
  flag bits. Row `dwg_strict_names_files_by_index`.

## v0.3.0: pairs with kit v0.22.0

**Requires `k2-mpc-base` v0.22.0**, checked out beside this repo, as v0.2.0 does; no kit change.
The crates are 0.3.0 (a breaking release: the structure index's format is 4); `Cargo.lock` and
`fuzz/Cargo.lock` name that kit. **Breaking first:** a host re-indexes the versions v0.2.0
indexed (below). New: the edit verbs `entity rotate`, `entity scale`, `block insert` and
`text set`; attributes as fields of their own; each draw row's `edit`; `model_space_block`; flag
values after `=`.

### Breaking: the structure index, format 4 (attributes as fields)
- **A block reference's attributes are fields of its node**, `att.<tag>`
  (`k2_mpc_cad_model::index::attrib_fields`: the tag in lower case, other bytes `-xx`, a repeat
  `.2`), and no longer its text; the merge splits an entity by the same names. Two attribute
  values of one reference **land** under `--base` and in the merge when their tags differ, and
  overlap when the tag is the same (`MPC-E723` naming `att.<tag>`). A reference whose tags can't
  each be named (a name over the kit's 64 bytes, or over 200 attributes) keeps them as its text,
  as before. `text find` and `cad.change/1` are unchanged (an attribute change is the
  reference's `text` there).
- **What a host or a stored index does on 3 → 4: both a re-index and a typed refusal.** The kit
  keeps index rows by pack and format and reads only its pack's own format, so a version indexed
  only by v0.2.0 (format 3) is not ready under v0.3.0: a `--base` call on it is refused
  `MPC-E713 index_pending` (exit 1, `details.retry_after_ms`), and the host's index job indexes
  it again under format 4, whose row replaces the format-3 one. Rolling back to v0.2.0 does the
  same the other way (it never reads a format-4 row). Node keys are handles and don't change;
  keys the kit minted (entities without a handle) are minted again. Format 4 also leaves a
  block record's list of its references (`102 {BLKREFS`) out of the block's node. Row `index_format_n1`
  (`tests/edit_session.rs`) runs both directions on one store.
- **Tests**: `tests/index.rs` `attribs_are_fields`, `tests/merge.rs` `merge_attribute_tags`, and
  in `tests/edit_session.rs` the attribute pairs of `merge_and_base_agree` (two tags land in
  both, one tag overlaps in both), `two_agents_one_drawing` (two tags now land) and
  `index_format_n1`; plants `AttribsAsOneText`, `MergeAttribsAsOneText`.

### More edit verbs: rotate, scale, block insert, text set (DXF)
- `entity rotate @h:E… --by DEG --about x,y --unit U` (`rotated`): points turn about the
  centre in plan, directions turn (an ellipse's major axis, a multiline text's direction, a
  spline's end tangents), angles gain DEG in `[0, 360)` (an arc's ends, a text's, a reference's
  and its attributes' rotation, written in where missing); quarter turns are exact in decimal on
  the file's tokens, other angles computed in `f64` and written as the shortest decimal that
  reads back.
- `entity scale @h:E… --factor F --about x,y --unit U` (`scaled`, F above 0): points from the
  centre by F (z from 0 by F), radii, text heights, a multiline text's widths, polyline widths,
  an ellipse's major axis, a reference's three scales (written in where missing); exact decimal
  products where they can be (3.0 by 0.1 is 0.3).
- `block insert --block NAME|@h:B --at x,y --unit U [--rotate DEG] [--scale S]
  [--attr TAG=VALUE]…` (`inserted`): a new reference with an attribute per settable definition
  (constant ones have none), placed as the reference places its block (`Doc::place`), heights
  scaled, rotations turned, the definition's style, flags and alignment kept, layer `0`
  becoming the reference's; the block record's reference list (`BLKREFS`) gains it. Refused:
  a block missing, an external reference, a layout, anonymous or annotative (`DXF-E051`, new),
  a dynamic block (`DXF-E044`), a tag it can't set (`DXF-E047`), a field-driven (`DXF-E048`)
  or multi-line (`DXF-E049`) definition.
- `text set @h:TEXT --value S` (`edited`): a single-line text's value, written as the file
  writes text; anything but a TEXT refuses `DXF-E021` (a block reference: `attrib set`).
- `block insert` also refuses a block whose definition holds another application's object
  (`DXF-E043 proxy`); a missing block is refused (`DXF-E051`), never created. DWG files have no
  edit verbs (a verb on one is the kit's unknown-verb usage refusal).
- A turn by an angle that isn't a quarter writes a coordinate whose magnitude is below 1e-12 of
  the drawing's scale (the drawing-model read's rule) as `0.0`: the sine's and cosine's
  last-bit noise is the same on every platform. Row `rotate_noise_snapped`, plant
  `RotateNoiseKept`.
- The edit referee prints its case count per verb and fails when a verb has none.
- The self-check reads a text's value back (c) and checks a new reference's attribute and end
  handles (d) too.
- The structure index leaves a block record's list of its references (`102 {BLKREFS`) out of
  the block's node, as it leaves counts out: a new reference changes no block node (index
  format 4). The merge still takes that list whole, so an app's save that adds a reference of
  the same block as a verb's insert is kept as both files.
- **Tests**: `tests/edit.rs` `rotate_turns_points_and_angles`, `scale_sizes_and_points`,
  `block_insert_places_attribs`, `text_set_written`, and the four verbs in
  `untouched_units_exact`, `style_kept`, `honest_edits_land` and `refusals_named`;
  `tests/edit_session.rs` the four in `verbs_through_the_kit` and `person_edits_with_base`, and
  `merge_and_base_agree`'s rotate/colour, rotate/scale and text/layer pairs; 10 plants, each
  caught by its row; the referee's rotate, scale, insert and text calls.

### The drawing-model read
- The drawing-model sampler is as complete as a real drawing, so a drawing saved from it in
  another format compares fact for fact: it states its extents (the computed ones), its
  multileader has its arrowhead size, and its layout has its own sheet viewport (id 1) beside
  the viewport onto model space. Row `draw_sampler_as_complete_as_a_drawing`. The DWG reader
  keeps taking a layout's first viewport as the sheet's: a DWG viewport stores no id (a DXF
  writer numbers them as it writes).
- Each `draw page` row has `edit`: null when the edit verbs take the entity, else why they
  refuse it, one word of a closed set (`k2_mpc_cad_model::draw::EDIT_WORDS`: `version`,
  `no-handle`, `xref`, `dynamic`, `in-block`, `proxy`, `locked-layer`, `multiline-attribute`,
  `embedded`, `solid3d`, `own-frame`, `reactors`, `hatch`, `dimension`, `leader`, `viewport`,
  `image`, `unknown`). For DXF it is the verbs' own rules, so a viewer greys out exactly what
  a verb would refuse; a DWG drawing's comes from the model's facts. Row
  `draw_edit_matches_the_verbs`, plant `InBlockDrawnEditable`.
- `draw header` gains `model_space_block`: model space's own block record by its address
  (`@h:<HEX>`, the key a hold on it has), null only in a file without handles; a DWG read gives
  it too. Row `model_space_block_is_the_hold_address`.
- A verb's flag takes its value after `=` as well (`--value=--foo`: a value that starts with
  `--`, or is one of the kit's own flags, reaches the verb), and a lone `--` ends the flags:
  what follows is words. Row `verb_values_that_look_like_flags`.

## v0.2.0: pairs with kit v0.22.0

**Requires `k2-mpc-base` v0.22.0**, checked out beside this repo (a host vendors both side by
side: `vendor/k2-mpc-base` beside `vendor/k2-mpc-cad`). After v0.21.0 that kit adds what this
release uses: the tier-0 verb path, a pack's refusals by id, a hold's reach per node kind, pairing
across an outside save, and `AgentWrites::VerbsOnly`. The crates are 0.2.0; `Cargo.lock` and
`fuzz/Cargo.lock` name that kit.

### Provenance
- `PROVENANCE.md` rows for the edit verbs (`crates/dxf/src/edit/`: our own code from the DXF
  reference; byte patches keyed by handle; no header field written but `$HANDSEED`, no identity
  written) and the edit referee (`scripts/referee/ezdxf-edits.py`: test-only, never shipped).

### Agents change a drawing through the verbs only
- `DxfPack::agent_writes` is `AgentWrites::VerbsOnly`: an agent's whole-file `write` and
  `write --base` are refused (`MPC-E715`, the file unchanged), and its verbs are admitted (v0.1.0
  refused both, as a read-only release). A person's write and any restore are unaffected.
- Row `agent_writes_refused_verbs_land` (`tests/edit_session.rs`), reconciling v0.1.0's
  `an_agents_whole_file_write_is_refused` and the verbs' `agent_verbs_land`; plants
  `AgentWritesAllowed` and `AgentWritesRefused`.

### The pack's verbs inside the kit
- The read verbs are the kit's blob verbs (`BlobPack::verbs`): each runs on a blob doc through
  `run_tool`, its data naming its shape (`schema`: `cad.drawing/1`, `dxf.header/1`…) and, for a
  paged read, the next call (`next`). A drawing read (`draw header`, `draw page`) reads the version
  the kit hands it: `--version` may be left out, and another version than that one is refused.
- The pack's refusals by id (`BlobPack::errors`), under the pack's name: the reader's
  (`DXF-E001` to `DXF-E003`) and the shared model's, `CAD-E0nn` read as `DXF-E0nn` (an address
  that names nothing is `DXF-E010`, exit 4). A usage refusal is the kit's (exit 2).
- A hold's reach (`BlobPack::hold_reach`): a hold on a layer reaches every entity on it, on a block
  definition, model space or a layout everything in it, on anything else the node.
- Another program's save (`BlobPack::save_pairing`) is `Unknown`: no writer is proven to keep
  handles yet, so a `--base` call across one is refused and the agent reads again.

### The drawing-model read: platform-identical bytes
- Draw output floats rounded to 9 significant digits for platform-identical bytes: every number
  of `draw header` and `draw page` is written rounded to 9 significant digits, as the fewest
  digits that read back as that (`k2_mpc_cad_model::draw::out_float`), and `-0` as `0`, so a
  value whose last bits a platform's maths library computes otherwise is written the same on
  every platform. Part of the two shapes' contract; the goldens are blessed again with it.
  Row `draw_floats_platform_identical`, plant `DrawFloatsUnrounded`.
- Near-zero noise snapped: a number whose magnitude is below 1e-12 of the drawing's scale (its
  computed extents' larger span, or 1) is written `0`, so a value that is zero but for a
  platform's last-bit noise (a box edge at `-2.22e-16`) is the same everywhere. Row
  `draw_noise_snapped_to_zero`, plant `DrawNoiseKept`.

### The edit verbs (DXF)
- `attrib set`, `entity set`, `entity move`, `entity add line|polyline|text` and `entity delete`:
  blob verbs whose plans the kit applies, each with its past-tense word (`set`, `changed`,
  `moved`, `added`, `deleted`), `merge: pack`. README, "Edit verbs".
- **Byte-exact by construction**: value lines replaced, whole pairs inserted or removed, new
  pairs in the file's style; a move adds in decimal on the file's tokens; new handles above
  every owned or pointed-at handle, `$HANDSEED` raised; nothing else in the header written.
- **A self-check** of every plan before it returns (`DXF-E090`): pair count, every other unit
  byte for byte, every meant value, handles, no dangling pointer, the rules on the output.
- **The rules**, the same for people and agents, refusing by id (`DXF-E020` to `DXF-E054`,
  README): versions before DXF 2000, files without handles, external references, proxies,
  dynamic blocks' drawn forms and definitions, block definitions, locked layers, field-driven and
  multi-line attributes, and the kinds the verbs don't change.
- A delete takes the entity's followers, its owned objects and its draw-order row with it, and
  refuses (`DXF-E027`) when another object points at it.
- **Known**: two attribute values of one block reference overlap under `--base` and in the
  merge (they are one text).
- **Tests**: `tests/edit.rs` (what each verb writes and refuses, the self-check) and
  `tests/edit_session.rs` (through the kit: dry run and `--expect`, two agents from one version,
  `--base` agreeing with the merge, an app's save after a verb, pinned and not); 17 plants, each
  caught by its row; the referee (ezdxf, a test checker) reads every call's output in its own job
  (`cad-measure edits`, `scripts/referee/ezdxf-edits.py`, `cad-measure edit-referee`) and holds
  it to the call's intent, with no new audit errors.

## v0.1.0: pairs with kit v0.21.0

The first release of the drawing pack. It builds with **`k2-mpc-base` v0.21.0** checked out
beside this repo (a host vendors both side by side: `vendor/k2-mpc-base` beside
`vendor/k2-mpc-cad`); `Cargo.lock` and `fuzz/Cargo.lock` name that kit.

**This release is read-only.** It reads ASCII DXF files (every version from R12 to 2018) and
DWG files (the 2004 to 2018 family: `AC1018`, `AC1024`, `AC1027`, `AC1032`) into one
format-neutral drawing model, and answers read verbs and the drawing-model read (`draw header`,
`draw page`) from it. It has no edit verbs yet: nothing here edits a drawing for a person or an
agent, and a host must not offer whole-file writes to DXF or DWG files through this pack until
the edit verbs ship. **Agents' whole-file writes are refused**: `DxfPack` refuses an agent's
`write` and `write --base` of a DXF file (the kit's `MPC-E715`), whatever rights the host
grants, and the file is unchanged; people's reads are unaffected. A DWG file is never written.

- **What a host gets:** `k2_mpc_dxf::DxfPack`, the kit's tier-0 blob pack for DXF with a
  structure index (index name `dxf`, format 3; the noun `entity`); `k2_mpc_dxf::read_doc` and
  `k2_mpc_dxf::verbs::run` for the DXF reads; `k2_mpc_dwg::drawing::DwgDrawing` and
  `k2_mpc_dwg::drawing::draw` for the DWG drawing-model read. A refusal is a typed code
  (`CAD-E…`, `DXF-E…`, `DWG-E…`) that never quotes file content.
- **`k2-mpc-cad-measure`** is test tooling (the test drawings, the speed and memory
  measurements, the referee comparisons): run in test jobs, never shipped, and not a dependency
  of the readers.

### Known limits

- **DWG, named known differences** from what a program's own DXF save of the same drawing
  writes, each counted and printed by the comparison on every run:
  - a true-colour layer's colour number: the file stores none beside the true colour, so the
    reader gives the palette's nearest; a program may give another nearby number;
  - a viewport's on flag, read from its status bits: a drawing can mark a viewport on without
    that bit, and where else the file keeps it isn't known yet;
  - a spline stored by fit points only is drawn by interpolating its fit points, where a
    program computes control points of its own, so the two curves can differ slightly;
  - a leader's hook line: the file doesn't store it (a program computes it); the reader draws
    the leader's stored points only.
- **LEADER from 2010 on:** a leader's text box height and width are read up to 2007 only; from
  2010 they are not read.
- **The old 2D polyline's layout in a DWG file** is checked on hand-made rows only.
- Binary DXF is refused, typed. Encrypted or password-protected DWG drawings are refused, as are
  DWG versions outside the 2004 to 2018 family.
- Text boxes are estimates (`bbox_q: "approx"`); a viewer measures text with its own font.

### Test-only referees

The readers are checked against other programs run as separate processes, on test machines
only, pinned by version and checksum, never shipped, vendored or linked; only their outputs are
compared, and nothing from them is committed:

- **ezdxf** (MIT, a Python library): the DXF structure and the drawn values (`cad-measure
  referee`, `cad-measure referee-draw`, `scripts/referee/`);
- **LibreDWG's command-line tools** (GPL, run only as programs, never read as a source): DWG
  entity counts by kind and a DXF conversion to compare drawing-model reads against;
- **a DXF program's re-saves**, where used (`cad-measure h1` judges save pairs from any
  writer): run by a person, or on a test machine under the same terms; the re-saves are private
  test data, never committed.

Public sample drawings are fetched on the test machines at a pinned commit, checked by sha256,
and never committed, shipped or attached to a job's output (`crates/dwg/README.md`).


### A format-neutral drawing model
- `k2-mpc-cad-model`: entities by handle, layers, blocks, attributes and external references;
  the structure index built from them; and the reads every drawing format shares, with typed
  data (`cad.*` shapes). No node carries counts over other records.

### Block references and leaders, resolved
- A dynamic block's references go by its own name: the anonymous block that draws one as set
  is followed to the dynamic block it names (`attrib find`, `block show`, `block list`), the
  anonymous name kept beside it.
- An external reference's path; a block reference's scale and rotation, and its block's
  geometry as placed: its long axis (texts and attribute definitions left out) and its drawn
  box (attribute definitions left out).
- Leaders and multileaders: their points from the arrow, and their text (a leader's by the
  handle it points to, a multileader's its own).
- Multileaders, typed (`k2_mpc_cad_model::leader`, read from DXF by `k2_mpc_dxf::mleader`):
  the text with where it sits, its attachment and height, each leader's landing and lines
  (each from its arrow; bounded), and the arrowhead's block and size. The model keeps every
  line; `leader::plain` reads a text without its inline codes; the drawing read draws
  multileaders from the same reader.
- The structure probe lists a dynamic block's references under its own name (anonymous blocks
  in counts only), gives `check`'s findings by code and count (`check` also gives them as
  `by_code`), and no longer guesses flags from one sheet: over a set, the attribute tags whose
  values pair up across it are the flag candidates, by structure. Never a value.

### Reading DWG files: the container's parts
- `k2-mpc-dwg`: the bit-coded values (every code of each coded type, handle references
  relative to their object, entity colours, texts, object types), the CRC-16, CRC-32 and page
  checksum, and the container's decompression, each refusing damaged or hostile input with a
  typed `DWG-E…` code within a bounded heap.
- The container: the file header (its masked part, CRC-32 and second copy), the page map with
  its gaps, the section map, and each section's data pages checked and decompressed. Encrypted
  or password-protected drawings are refused, as are versions outside the 2004 to 2018 family.
- The object map (each block's CRC-16), each object's size, type and own handle (its CRC-16
  checked, in the 2004 and the 2010 layouts), and the class list (names from a string stream
  or inline; its CRC-16 and closing sentinel checked). An object map over the object limit is
  refused as it reaches the limit.

### Reading DWG files: images and wipeouts
- IMAGE and WIPEOUT are decoded (where they sit, their pixel axes and size, their clip
  boundary) and drawn as the DXF read draws them; an image definition gives its image file's
  path as written. No image file is ever opened.

### Reading DWG files: the old polylines
- The old 2D and 3D polylines, face meshes and meshes are decoded with their vertices (each
  vertex an object the polyline owns, then its SEQEND) and drawn as the DXF read draws them: a
  2D polyline at its elevation with its widths and bulges, a 3D polyline in world coordinates,
  a spline frame's control points left out, face meshes and meshes not modelled.

### Reading DWG files: leaders and multileaders
- LEADER is decoded (its points from the arrow, path type, arrowhead flag, what it annotates
  and its dimension style, with its own style overrides from its extension data) and drawn as
  the DXF read draws it; dimension styles give the arrowhead's size, scale and block. A leader's
  hook line, which the file doesn't store, is not drawn.
- MULTILEADER is decoded with its annotation context (its leaders and lines, its text or block
  content) into the drawing model's leader content, the same type the DXF read fills, so a
  multileader draws from a DWG file as from a DXF file: its text with where it sits, its
  attachment and height, each leader's landing and lines (bounded as the DXF read bounds them),
  and its arrowhead's block and size.

### Reading DWG files: splines, rays and construction lines
- SPLINE (by control points and knots, or by fit points), RAY and XLINE are decoded and drawn,
  as the DXF read draws them.

### Reading DWG files: entities, tables and the drawing-model read
- Each object's data, string and handle streams, its extension data, and the data every entity
  begins with (entity mode, reactors, extension dictionary, colour with its book colour and
  transparency, linetype scale and flags, plot style, material and visual style flags,
  invisibility, lineweight) with its owner, layer and linetype handles, in the 2004 and the 2010
  layouts. A kind's read must end exactly at its object's data end, so a field read out of place
  refuses the entity instead of giving wrong values.
- The fields of LINE, ARC, CIRCLE, POINT, ELLIPSE, SOLID, TRACE, 3DFACE, LWPOLYLINE, TEXT,
  MTEXT, INSERT and MINSERT with their ATTRIBs and SEQEND, ATTDEF, BLOCK, ENDBLK, VIEWPORT,
  HATCH and the seven dimension kinds; every other entity is read for its common data.
- The tables: the control objects (each table's order), layers, linetypes (with their text and
  shape elements), text styles, registered applications, dimension style names, block records
  (their entities in order) and layouts.
- `drawing::DwgDrawing` gives the drawing-model read a DWG file: `draw header` and `draw page`
  read it as they read a DXF file (`drawing::draw`). The header variables give the drawing's
  settings, units, extents and next handle.
- 2018 multi-line attributes and attribute definitions are read, their embedded multiline
  text with them; the DXF read takes an attribute's width factor, generation flags and
  alignment from its text's own codes, not its embedded text's or its attribute type's.
- A 2018 multiline text's column data (type, count, width, gutter, automatic height, flow,
  heights) is kept for the drawing model; a column height the file leaves unset (stored as
  minus the text's extent) is read as 0, as the program's own DXF save of the drawing writes it.

### Reading DXF drawings, byte-exact
- A tokenizer of our own: every group code and value with its exact byte range, line endings
  per line, code padding, comments, a byte-order mark and anything after the end marker carried
  as they are. Writing the tokens back gives the same bytes. Binary DXF and files that aren't
  code/value pairs are refused, typed, with the byte offset.
- The structure index: the drawing, its layers, its blocks (model space and layouts included)
  and every entity, keyed by handle (`@h:2B1`). Each entity's own facts are separate fields
  (layer, colour, linetype, lineweight, transparency, geometry, text), so a change to one is told
  apart from a change to another. Followers (attributes after an insert, vertices after a
  polyline) belong to their parent. No node carries counts over other records.
- Read verbs, each returning typed data: `drawing info`, `layer list`, `block list`,
  `entity find`, `text find`, `header get` and `check`. Every read that returns coordinates
  states the drawing's units.

### The structure index, format 3
- A text's box (single-line, multiline, attribute definition) is laid out around its alignment
  or attachment point, by the same computation the drawing-model read uses, so a change's
  highlight lands on the text. Node keys are unchanged by the format change.

### The drawing-model read
- `draw header` (`cad.drawing.header/1`) and `draw page` (`cad.drawing.page/1`): what a viewer
  needs to draw a drawing, the format's rules applied (colour, linetype, lineweight and
  transparency by layer and by block, text alignment, multiline text as paragraphs of runs,
  splines and hatch patterns as bounded paths), paged in draw order, each response carrying
  the doc version it reflects. Shared by every drawing format: a reader supplies one entity's
  raw values at a time, and no geometry is kept on the model.
