# Changelog

## Unreleased

Built against the kit (k2-mpc-base) **v0.22.0** (its release commit bfdb6217; after v0.21.0: the tier-0 verb path,
the pack's refusals by id, a hold's reach per node kind, pairing across an outside save, and
`AgentWrites::VerbsOnly`). `Cargo.lock` and `fuzz/Cargo.lock` name that kit.

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
