# k2-mpc-cad

2D drawings for the multiplayer kit (`k2-mpc-base`): a tier-0 type pack with a structure index,
so people and agents can read a drawing by handle, see what changed between versions and who
changed it, and comment on or claim parts of it.

The repo holds:
- **`crates/cad-model`** (`k2-mpc-cad-model`): a format-neutral model of a drawing (entities by
  handle, layers, blocks, attributes, external references), its structure index, and the reads
  every drawing format shares;
- **`crates/dxf`** (`k2-mpc-dxf`): reads ASCII DXF files into that model, byte-exact;
- **`crates/dwg`** (`k2-mpc-dwg`): reads DWG files (the 2004 to 2018 family), read-only, with
  typed refusals: the container, the objects, the entities and tables, and the drawing-model
  read (`draw header`, `draw page`) (`crates/dwg/README.md`).

## What it reads

- **Every ASCII DXF version** from R12 (AC1009) to 2018 (AC1032). Binary DXF is refused, typed.
- **The tokens, byte-exact.** The tokenizer keeps each group code and value with its exact byte
  range: line endings per line, code padding, `999` comments, a byte-order mark and bytes after
  the end marker. Writing the tokens back gives the input's bytes, always.
- **The structure index** (one per version):

  | Node | Key | Parent | Name |
  |---|---|---|---|
  | the drawing | `@doc:<format>` (`@doc:dxf`) | none | the file's version (`AC1027`) |
  | a layer | `@h:<handle>` | the drawing | the layer's name |
  | model space, a layout, a block | `@h:<block record handle>` | the drawing | its name |
  | an entity | `@h:<handle>` | its owner | its kind (`LINE`) |

  Defaults are left out of what is stored (an entity's `bbox_q` when `exact`, its `edit` when
  `1`, its kind, which is its name); `k2_mpc_cad_model::index::with_defaults` gives them back.
  An entity's own facts are its fields: `layer`, `layer_key` (the layer record it names),
  `color`, `rgb`, `ltype`, `lw`, `alpha`, `geom` (a digest of its geometry, followers
  included), `bbox`, and its text. A node never carries counts over other records: those are
  computed by the read verbs on each read.
- **Handles** are normalised: leading zeros dropped, upper case. A file without handles gets keys
  minted by the kit.

- **Block references, resolved.** A reference to the anonymous block that draws a dynamic block
  as it is set goes by the dynamic block's own name (`block`), the anonymous one beside it
  (`anon_block`); `attrib find --block` matches either, and `block list --anon` names the
  dynamic block each anonymous one draws (`dynamic_of`). An external reference's block carries
  its path as written (`path`). A reference's scale and rotation are kept, so its block's
  geometry can be placed (`Doc::place`, `Doc::placed_axis`).
- **Leaders.** A leader's points, its arrow first, and the handle of the text it points to; a
  multileader's every leader line from its arrow to its landing (at most 64 lines and 4,096
  points), and its own text as written (found by `text find`; `leader::plain` reads it without
  its inline codes) and where it sits.

## Read verbs

Each is a pure function of one version's bytes, with typed data. The shape's name says whether it
is shared by every drawing format (`cad.drawing/1`, `cad.layer-list/1`, `cad.block-list/1`,
`cad.entity-find/1`, `cad.text-find/1`) or the DXF reader's own (`dxf.header/1`, `dxf.check/1`):

| Verb | Answers |
|---|---|
| `drawing info` | version, encoding, line endings, units, extents (the header's and computed), counts, handle health, what is editable |
| `layer list [--unused]` | each layer with its switches and the entities on it |
| `block list [--unused] [--anon]` | each block with its base point and its inserts, direct and nested |
| `entity find [--layer L] [--kind K] [--in model\|paper\|@h:B] [--window x0,y0,x1,y1 \| --crossing x0,y0,x1,y1] [--after @h:H] [--limit N]` | handles, kind, layer, box |
| `text find --contains S [--layer L] [--case] [--after @h:H] [--limit N]` | text, multiline text and attribute matches |
| `header get $VAR…` | header variables as written |
| `check [--writeback]` | structure, handle, reference and header findings; with `--writeback`, a byte-exact write-back self-test |
| `draw header --version N` | everything a viewer needs before drawing (`cad.drawing.header/1`), below |
| `draw page --version N --space S [--cursor C]` | one page of a space's entities, resolved for drawing (`cad.drawing.page/1`), below |

Every read that returns coordinates gives the drawing's units with them.

## The drawing-model read

`draw header` and `draw page` give a viewer what it needs to draw the drawing, with the format's
inheritance rules already applied, so it never parses the file. Both live in the shared model
crate (`k2_mpc_cad_model::draw`): a format's reader supplies only the raw values of one entity at
a time. Every response carries `version`, the doc version the host read (`--version`), so a
change shape applies from it.

- **Coordinates** are the world plan's x and y in the drawing's units; **angles** are radians,
  counter-clockwise from +x. An entity in a frame of its own is converted to the plan (a
  mirrored frame exactly; a tilted one as a `path`, flagged `ocs: "tilted"`).
- **Header** (`cad.drawing.header/1`): units, extents, settings, layers (resolved colour,
  linetype, lineweight in mm, transparency, switches), linetypes (dash lengths), text styles,
  block definitions (base point, count, box, a `digest` to cache a definition by), layouts
  (paper, viewports with their frozen layers), external references, `whys` and the page budget.
- **Pages** (`cad.drawing.page/1`): a space's entities in draw order (by its draw-order table
  when it has one, else file order), at most 5,000 entities or 4 MiB of JSON a page; `next` is
  the exact next call. A cursor from another version is refused (`CAD-E013 stale_cursor`).
- **Each entity**: `h`, `kind`, `layer`, `owner`, `z` (its draw-order place), `style` (`rgb`,
  `fg` for colour 7, `alpha`, `ltype`, `lt_scale`, `lw_mm`), `visible`, `drawn`, `bbox`,
  `bbox_q`, and `geom` when drawn.
- **Inside a block definition**, a value by block, and a value by layer on layer `0`, read
  `"insert"`: the referencing block reference's value. Block references are references, never
  expansions; a block's own entities come from `draw page --space @h:<block>`.
- **Text**: the alignment point and alignment as the format defines them (for fit and aligned,
  both points); the viewer measures advance widths with its own font. A text's box is laid
  around its alignment point (a multiline text's around its attachment point, its height
  grown by its runs' own heights and by wrapping at its reference width). **Text boxes are
  estimates** (`bbox_q: "approx"`, in the pages and in the index): characters are taken at 0.6
  of the height, and real fonts run wider, so an agent must not use a text's box to check
  whether text fits. `%%` codes are decoded.
  Multiline text comes as paragraphs of formatting runs, with no inline code left in any run.
- **Splines** come with a `path` within a stated tolerance (1e-4 of the control points' box,
  guaranteed by the curve's hull, not sampled), at most 8,192 points; `tol_achieved` says when
  the cap made it coarser. A spline with fit points only is the polyline through them
  (`interpolated: false`).
- **Hatches**: boundary loops as paths; a pattern's lines clipped to the boundary, or `dense`
  when over 50,000 segments (estimated before any is made). A pattern's definition lines (base
  point, offset, dash lengths) are in **drawing units**, its angles in radians, the hatch's own
  angle and scale already applied.
- **Leaders** carry their arrow: shown or not, its kind (`closed_filled`, or its block's name)
  and its size in drawing units (the arrow size times the overall scale, from the leader's own
  overrides, else its dimension style's).
- **Multileaders** carry their leaders (each one's landing and lines, from the arrow), their
  arrow (its kind and size), their text as written and plain, where it sits, its attachment
  and height: the DXF reader's typed multileader (`k2_mpc_dxf::mleader`).
- **Closed outlines** (a wipeout's `mask`, an image's `clip`, a hatch loop's `path`) come with
  `closed: true` and their first point **not** repeated at the end.
- **Images** carry their corners, clip outline and their definition's file path as written
  (`resolved: null`: resolving a path is the host's).
- **Not drawn** entities say `drawn: false` with a `why` from a closed set of stable words:
  `not_modelled`, `attdef`, `spline_weights`, `over_cap`, `unreadable`, `viewport`. The header
  lists the set.

## Privacy

`$LASTSAVEDBY` usually holds a login name. It is never copied into the index or `drawing info`;
`header get $LASTSAVEDBY` reads it when asked by name.

## Building and testing

The kit is a path dependency: check out `k2-mpc-base` beside this repo, at the commit or tag
`CHANGELOG.md` names. `scripts/check.sh` runs the repo's checks: formatting, lints, the docs with
warnings denied, every test (the planted bugs too), licences, the licence files, the private-name
guard and the label guard. The private-name guard reads its patterns from outside the tree
(`MPC_PRIVATE_PATTERNS`, `MPC_PRIVATE_PATTERNS_FILE` or `~/.config/k2-mpc/private-patterns.txt`).

## Licence

MIT OR Apache-2.0, at your option (`LICENSE-MIT`, `LICENSE-APACHE`). See `NOTICE.md` and
`PROVENANCE.md`.
