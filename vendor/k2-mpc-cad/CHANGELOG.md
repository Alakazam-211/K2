# Changelog

## Unreleased

Built against the kit (k2-mpc-base) v0.21.0.

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
