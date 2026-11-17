# Provenance

What each part of this repo was written from. Everything here is our own Rust, written by us;
no code, text, tables or figures are copied from another project or document, but for one table
of data: the colour-number palette (its row below).

| Part | Written from |
|---|---|
| `crates/cad-model` (the drawing model, its structure index, the shared reads, box geometry) | our own design; the kit's documented index and result shapes (`k2-mpc-base`). The box geometry (arcs, bulged segments, the arbitrary-axis frame, ellipses) is worked out from the geometry itself, in our own code |
| `crates/dxf/src/token.rs` (the tokenizer) | the DXF reference, read as a specification for the file's code/value line structure; our own code |
| `crates/dxf/src/codes.rs` (the group-code value types) | our own table, written in our own words from the DXF reference's group-code ranges, read as a specification |
| `crates/dxf/src/structure.rs`, `convert.rs` (sections, tables, blocks, entities, followers, each kind's geometry codes) | the DXF reference, read as a specification; our own code and tables |
| `crates/dxf/src/verbs.rs` (the DXF reads and `check`) | our own design |
| `crates/dxf/src/testgen.rs` (the test drawing generator) | our own code; it writes neutral test drawings with no user, machine or path names |
| `crates/dwg/src/bits.rs` (the bit-coded values) | the Open Design Specification for .dwg files 5.4.1, its chapter on bit codes and data definitions, read as a source of facts; our own code |
| `crates/dwg/src/crc.rs` (the CRC-16, the CRC-32, the page checksum) | the same specification, its chapters on the 2004 file format and the object map, as facts; our own code, checked against published check values |
| `crates/dwg/src/lz77.rs` (the container's decompression) | the same specification, its chapter on the 2004 file format's compression, as facts; our own code |
| `crates/dwg/src/container.rs` (the file header, the page and section maps, the data pages) | the same specification, its chapter on the 2004 file format, as facts; our own code |
| `crates/dwg/src/objects.rs`, `classes.rs` (the object map, objects' heads, the class list) | the same specification, its chapters on the object map, the objects' common data and the classes section, as facts; our own code |
| `crates/dwg/src/frame.rs`, `entities.rs`, `tables.rs` (objects' streams, extension data, the entities' common data, each entity kind's and table object's fields, the type numbers) | the same specification, its chapters on the objects' and entities' common data, the extension data and the object prescriptions, as facts; our own code and tables. Where the public sample drawings disagree with it, the samples' facts are followed and each is noted in the code |
| `crates/dwg/src/drawing.rs` (the format-neutral model and drawing tables from a DWG file) | our own code, mapping to the drawing model the way the DXF reader does |
| `crates/dwg/tests/support/object.rs` (hand-made object bytes for the rows) | our own test code: one object's size fields and CRC-16 around the bit fields each row packs; never a drawing, a section or a file |
| `crates/dwg/tests/support/container.rs` (the test container builder) | our own test code: container structure only around bytes the tests choose, no drawing objects; its app-info holds only our own generic writer name |
| `crates/dwg` as a whole | written without reading or running any other DWG reader's code; no other project's DWG code is a source |
| `crates/cad-model/src/draw` (the drawing-model read: style resolution, text placement, the multiline text parser, spline and hatch tessellation, paging) | our own design and code; the inheritance rules and inline codes from the DXF reference, read as a specification |
| `crates/cad-model/src/draw/aci.rs` (`TABLE`, the colour-number palette) | **data taken from ezdxf** (MIT licence, notice in `NOTICE.md`): `DXF_DEFAULT_COLORS` in `src/ezdxf/colors.py`, ezdxf 1.4.2, commit 8240df5793daf2fd7db95d118a78e17ec92043ec (the file's SHA-256 2dc20b764a7ca766f9719f954974e46348595669a4ae8eb7dd4da512e0ed16fb). The 256 values only; no code. The drawn-values referee checks every entry against ezdxf when it runs |
| `crates/dxf/src/draw.rs` (each kind's raw drawing values, the tables the drawing-model read uses) | the DXF reference, read as a specification; our own code |
| tests, plants, fuzz properties | our own |

**Not used as sources:** no other DXF or DWG reader's or writer's source code is read or ported
for this repo. Test referees, where used, run as separate processes on test machines, are pinned
by version and checksum, and are never shipped or vendored; only their outputs are compared.

A DWG reader, when it comes, gets its own lines here, with what it was written from.
