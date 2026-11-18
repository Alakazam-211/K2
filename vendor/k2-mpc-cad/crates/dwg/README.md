# k2-mpc-dwg

Reads DWG files, read-only: the 2004 to 2018 family (`AC1018`, `AC1024`, `AC1027`, `AC1032`).

- `read_container`: the file header (its masked part, its CRC-32, its second copy), the page
  map with its gaps, and the section map;
- `read_section`: one section, each of its pages checked (type, section, data checksum, header
  checksum, start offset) and decompressed;
- `objects::object_map` and `objects::object_head`: the object map (each block's CRC-16) and
  each object's size, type and own handle, its CRC-16 checked; `classes::classes`: the class
  list, its CRC-16 and closing sentinel checked;
- `frame::open`: one object's data, string and handle streams and its extension data;
  `frame::entity_common`: the data and handles every entity begins with;
- `entities::entity`: an entity's kind and fields (LINE, ARC, CIRCLE, POINT, ELLIPSE, SOLID,
  TRACE, 3DFACE, LWPOLYLINE, TEXT, MTEXT, INSERT and MINSERT, ATTRIB, ATTDEF, SEQEND, BLOCK,
  ENDBLK, VIEWPORT, HATCH, the dimension kinds, SPLINE, RAY, XLINE, LEADER, MULTILEADER, the
  old 2D and 3D polylines, face meshes and meshes with their vertices, IMAGE and WIPEOUT (their
  geometry and clip; an image file is never opened); every other kind for its common data); a
  kind's read must end exactly at its data's end;
- `leaders`: a leader's fields (its points, path type, arrowhead flag, what it annotates and its
  dimension style; its own style overrides come from its extension data, as in DXF); a
  multileader's fields with its annotation context (its leaders, then its text or block
  content), into the drawing model's leader content, the type the DXF read fills;
- `tables`: the control objects, layers, linetypes, text styles, registered applications,
  dimension styles (every setting read; the ones a leader's arrowhead is drawn by kept), block
  records, layouts, and image definitions (their image files' paths, as written);
- `drawing::DwgDrawing`: the format-neutral model and the drawing tables, as a source for the
  drawing-model read (`draw header`, `draw page`, through `drawing::draw`);
- `header::header`: the header variables (settings, units, model space's extents, the next
  handle), read in their order by version; `header::measurement`: the measurement system;
- the bit-coded values the sections are written in, the CRC-16, CRC-32 and page checksum, and
  the decompression, within hard bounds.

Where the public sample drawings disagree with the specification, the reader follows the
samples, and the code says so at each place: no links bit in an entity's common data, a
graphic's size in three plain bits, a book colour carrying no true colour, no external
reference index in a table entry from 2007, a layout's viewport count as a `BL`, a count of
further handles in the dimension style table, a linetype element's text and shape bits (2 and
4, as DXF has them), a text style's shape-file flag before its vertical flag, a 2018 multiline
text's column height stored as minus the text's extent where it isn't set (read as 0, as the
program's own DXF save writes it), a multileader's arrowhead list up to 2007 only (its block
labels in every version), a leader's text box height and width up to 2007 only, a
dimension style's three dimension colours as object colours, a 2018 multi-line attribute's
embedded multiline text beginning with the data every entity has from its entity mode (no
graphic flag). The 2004 header
ends with the four shorts the specification lists last, then fewer than 8 bits of padding; from
2007 the header's read ends exactly at its data's end.

Every damaged or hostile input is refused with a typed `DWG-E…` code, and a refusal never
quotes file content. Encrypted or password-protected drawings (the security flags set, an
encrypted section, a security section) are refused before any section is read. Other versions
are refused as unsupported.

The format's facts come from the Open Design Specification for .dwg files (version 5.4.1), used
as a source of facts only. The code and its tables are our own, and no text or tables are copied
from it (`PROVENANCE.md`).

## Known limits

- **A true-colour layer's colour number:** the file stores none beside the true colour (its
  colour record's number is 0), so the reader gives the palette's nearest; a program may give
  another nearby number.
- **A spline stored by fit points only:** the file holds its fit points, end tangents and knot
  parameter; the drawing model draws it by interpolating the fit points, where a program
  computes control points of its own, so the two curves can differ slightly (converting the fit
  points and end tangents to control points is a planned follow-up).
- **A viewport's on flag:** read from its status bits; a drawing can mark a viewport on without
  that bit, and where else the file keeps it isn't known yet.
- **A leader's hook line:** the file doesn't store it; the program computes it (a segment of the
  arrow's size along the leader's direction, just before its last point) and its DXF save writes
  it as a vertex. The reader draws the leader's stored points only.

## Tests

The rows run on hand-written bytes: bit fields packed in the test, published check values for
the checks, compressed streams written by hand, synthetic containers from the test builder in
`tests/support/container.rs`, and single objects whose fields each row packs itself
(`tests/support/object.rs` puts the size fields and CRC-16 around them). Each row has a planted bug that it must catch
(`cargo test -p k2-mpc-dwg --features mutants --test mutants`). The hostile-input rows measure
the heap with a counting allocator and print `peak_heap_bytes`.

Test inputs are held to these conditions:

- **Public sample drawings** are used only as test inputs, and only on our build machines:
  - fetched there, pinned by the source repository's commit;
  - listed in a manifest giving each file's sha256, licence and source URL, with the source's
    licence file kept beside them;
  - never committed, shipped or attached to a job's output: only counts and pass or fail leave
    the machine.
  - A sample folder's own licence or note overrides its repository's licence.
  - Left out: any sample made from someone's real drawing under other terms, and any sample
    with no licence where its folder needs one.
  - The source project's code is not read or copied.
- **A synthetic container builder** may exist in test code only (`cfg(test)` or an unpublished
  dev crate, never in a published crate's files):
  - it writes container structure only (the header, pages, sections and checksums), never
    drawing objects, and never grows into a whole-file generator;
  - it never writes another program's trusted-file marker, or anything that imitates another
    program's writer. A row checks its output for the marker, and a planted bug that writes the
    marker must be caught;
  - its fixtures are generated when the tests run, or committed only as small synthetic files
    we made.
- **One DWG fixture**, `tests/data/draw_sampler.dwg` (the test generator's sampler drawing as
  a 2018 DWG), was made once by our own test tooling, which is not published, and cleared by its
  sha256 before it was committed (`tests/data/MANIFEST.md`). Nothing in this repo writes DWG; a
  new file is a new fixture, cleared again.
- **Another DWG reader's command line** may be used only as a referee on our build machines,
  comparing counts.
- **No client or user drawing** is ever a test input in this repo.
