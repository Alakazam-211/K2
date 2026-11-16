# Provenance

Where every layout this reader knows comes from. The reader is written from the sources below
and from the files themselves; nothing in it is copied from another project.

## Not read

No GPL source code was read in writing this code, and none may be read in reviewing it: not
the source of the program that writes BLEND files, not its asset tracer, and no diffs or
commits of either. Where
the public release notes link to source files for the details, those links were not followed.
The Rust crates that read BLEND files were not used, studied or copied, and are not
dependencies.

## The container (the classic header, file blocks, the schema block)

- **The Kaitai Struct description `blender_blend.ksy`** (CC0-1.0), read as a reference only:
  <https://formats.kaitai.io/blender_blend/>. It gives the 12-byte classic header (magic,
  pointer-size character, byte-order character, three-digit version), the classic block header
  (code, body length, old address of the file's pointer size, schema index, count), and the
  schema block's sections (`SDNA`, `NAME`, `TYPE`, `TLEN`, `STRC`, each on a 4-byte boundary of
  the body). It is not vendored, copied in, or used to generate code.
- **The file's own schema**: every struct, field, size and offset this reader uses is read
  from the schema block of the file in hand at read time. A field's size follows from its
  written name (`*` and `(*...)` pointers, `[n]` dimensions) and its type's length; every
  struct's fields must add up to its length. The field names the scene model looks up (`id`,
  `name`, `lib`, `loc`, `rot`, `quat`, `size`, `restrictflag`, `data`, `parent`, `mat`,
  `totcol`, `gobject`, `children`, `ob`, `object`, `collection`, `base`, `master_collection`,
  `camera`, `world`, `lens`, `energy`...) were found by listing the schemas of the sample files
  below.

## The newer header form

From two sources:

- **The public release notes for version 5.0**,
  <https://developer.blender.org/docs/release_notes/5.0/core/> (section "Large Buffers in
  Blend-Files"): the file header and every block header changed, to allow larger blocks; 4.5
  reads the new form; data-block names grew from 63 to 255 bytes; big-endian support is
  removed.
- **The sample files' bytes** (below), which give the layout itself:
  - the header is 17 bytes: `BLENDER`, `17` (the header's length), `-` (8-byte pointers), `01`
    (the block-header format), `v` (little-endian), and a four-digit version (`0500`, `0501`);
  - each block header is 32 bytes: the 4-byte code, a 4-byte schema index, the 8-byte old
    address, an 8-byte body length and an 8-byte count. Checked by walking every block of the
    5.00 and 5.01 samples to the end block, with the schema block parsing to exactly its
    length and every data-block's `id.name` reading as its type code and name;
  - names are `name[258]` in those files' schemas (two code bytes, 255 bytes, a NUL).

Any other header length, pointer size, block-header format or byte order in the newer form is
refused as unknown: nothing beyond what the samples show is assumed.

## Storage

- **zstd**: the frame format of RFC 8878. The samples are runs of single-segment frames
  followed by a skippable frame (a seek table, which the reader skips); none carries a
  checksum or a dictionary. Decoded with `ruzstd` (MIT).
- **gzip**: the 2.73 sample. Decoded with `flate2` (MIT OR Apache-2.0).

## What the samples showed, and the reader does

- Old addresses repeat: the same address on blocks of different data-blocks, and on blocks
  owned by none. A pointer resolves to the one block with its address, else the one owned by
  the same data-block, else the one data-block, else it is ambiguous.
- Schema index 0 marks raw (untyped) data, whatever struct 0 is in that file.
- The schema block comes second to last, before the end block, so the reader keeps every
  data-block's own struct (and small data blocks) until it has the schema.

## The sample files

Demo files their publisher lists as CC0 on <https://www.blender.org/download/demo-files/>,
fetched by reference (`crates/blend/tests/corpus/manifest.tsv`) and never committed.

| File | Title, author (as listed) | Version, form, storage | sha256 of the download |
|---|---|---|---|
| `fishy_cat.blend` (member of `splash_fishy_cat_2.zip`) | the 2.74 splash, Fishy Cat; Manu Jarvinen | 2.73, classic, gzip | zip `22769b4b003330eccd13de3450555bb50f2500827c4cf1997eefa365acfec275`; member `4b5a97bed6361f1a1f888e02eda8ebda338276ff0afe46c5a170dac55bba36d5` |
| `mesh_fracturing.blend` | Mesh Fracturing; Simon Thommes | 3.06, classic, plain | `2bd3bcbad59ce61481d5c5ed3879d6f7f9ed7bf564858584d01e19bffb51485a` |
| `jiggly_pudding.blend` | Jiggly Pudding; Mingwei Lim | 3.06, classic, zstd | `1338daffaf54d521fe851514be2ba10c2ec93769393e4dd2c79419a68cf9727d` |
| `gizmo_array.blend` | Gizmo Array; Simon Thommes | 4.03, classic, zstd | `4262fd55588aa9c03fc09b6726917371c71173446545ad8e945ab3d72960e0d4` |
| `repeat_zone_fractal_raymarch.blend` | Repeat Zone Raymarch Fractal; Cartesian Caramel | 5.00, newer, zstd | `5178cd88e6518fcacc27984282ade0d988794e15d11a8dda0321ddd01708a376` |
| `mandelbrot_grow.blend` | Mandelbrot Grow; Shahzod Boyhonov | 5.00, newer, zstd | `08b1928c3b8c6c2ed120a470cdc08277a706e624c8133e72bc2f0e0c6c4255f9` |
| `skeleton-arm-xray.blend` | X-Ray Skeleton Arm; Cartesian Caramel | 5.01, newer, zstd | `6556cf6ee4e2cbcd9376fd15136601eb8ed8553a3f53ffaa94dbb3f08ffffba5` |

The zip of the 2.74 splash holds other files (a font archive, images); only the `.blend`
member is taken.

## Flags and values: learnt from saved files

The meaning of the flag bits and values the scene model names (`crates/blend/src/scene.rs`,
`NAMES_FROM` and the tables beside it) was learnt from file structure: the program that writes
these files was run headless on a build node, at three pinned releases (3.6.23, 4.5.9, 5.1.2,
each the publisher's Linux archive checked against its published sha256), to save small scenes
with known states, one state per file; the saved fields were then diffed with the reader in
this repo. Only the saved files came back; nothing of the program was linked, copied or
shipped, and the script that drove it is not in this repo (its checksum is under "Private
notes"). The files stay on the node and are never committed; `crates/blend/tests/saved/`
pins them by sha256 and its rows check every state on them.

What every one of the three releases saved alike (each type only where the release has it:
3.6.23 has no light probe object of the kind asked for and saves its grease pencil as 26; the
later two save 30, and have no 26 to save):

| Field | Values |
|---|---|
| an object's `restrictflag` | bit 1 disabled in viewports, bit 2 not selectable, bit 4 disabled in renders |
| a collection's `flag` | bit 1 disabled in viewports, bit 2 not selectable, bit 8 disabled in renders |
| an object's `type` | 0 empty, 1 mesh, 2 curve, 3 surface, 4 text, 5 metaball, 10 light, 11 camera, 12 speaker, 13 light probe, 22 lattice, 25 armature, 26 the older grease pencil, 27 curves, 28 point cloud, 29 volume, 30 grease pencil |
| an object's `rotmode` | -1 axis angle, 0 quaternion, 1 XYZ, 2 XZY, 3 YXZ, 4 YZX, 5 ZXY, 6 ZYX |
| a light's `type` | 0 point, 1 sun, 2 spot, 4 area |
| a camera's `type` | 0 perspective, 1 orthographic, 2 panoramic |

Seen but not named: a view layer's own states. An object hidden in a view layer changes
several bits of its base's flag at once (state the program derives), and a layer collection's
`flag` takes 16 when excluded and 128 when hidden; the model doesn't read view layers. Files
older than 3.06 keep their numbers only: no release before it was run.

## What drives a transform (the live pack)

The live pack (`crates/blend/src/live/model.rs`) marks an object whose transform something
else sets (`edit` `a`), so `object place` is refused on it. It reads, by name from the file's
own schema, an object's `adt` (its animation data), `constraints` (a list) and `action`, and the
animation data's `action`, `nla_tracks` and `drivers`. Those names were found by listing the
`Object` and `AnimData` structs of the 3.06, 4.03 and 5.01 samples above, where all six are
present. Their meaning (that the program recomputes such a transform on load) is a hypothesis
from what the names say, not yet checked on saved files; refusing is the safe side of it.

## Placements (the live pack)

The live pack's placements (`crates/blend/src/live/place.rs`) read, by name from the file's own
schema, an object's `loc`, `rot`, `quat`, `rotAxis`, `rotAngle`, `size` (or `scale`), `rotmode`,
`parent`, `partype` and `parentinv`, its delta transforms (`dloc`, `drot`, `dquat`, `dscale` or
`dsize`, `drotAngle`), and a scene's `unit.scale_length`. How they compose (the Euler order
words, the parent inverse between the parent's world matrix and the object's own transform,
scale before rotation, the matrices stored column by column) was settled against the world
matrix the program caches in each object of the files it saved up to the 3.x versions (`obmat`):
the public corpus's 57 objects that cache one match the pack's placements within `f32` rounding
(`crates/blend/tests/corpus_place.rs`). Those files hold only `xyz` rotations and no parenting
other than to an object, no driven transform and no delta transform; files from 4.x on cache no
matrix. The other rotation modes, an axis and angle, and what gives no placement (a parenting
other than to the object itself, a delta that isn't identity) rest on the kit's shared vectors
and on what the field names say. `partype` 0 is a parent object itself (every parented object
of those files that caches a matrix has it, and matches).

The doc's space gives metres per unit (the scene map's `unit_scale`) only when every scene's
unit system (`unit.system`) is 1 and their unit scales (`unit.scale_length`) agree; anything
else gives none (unknown), never an assumed 1 (`crates/blend/src/live/model.rs`,
`METRIC_SYSTEM`). Two meanings rest on a **hypothesis**, held until a saved file confirms them:
that system 1 is metric, with the scale as metres per scene unit, and that system 0 is a scene
with no real-world scale (the user manual lists the unit systems as None, Metric, Imperial; the
numbers are read from files, not from the program). No permissively licensed file found holds a
scale other than 1 or system 2, so none can confirm them:

- the public corpus (`tests/corpus_place.rs` prints each scene's unit settings): the 2.73
  file's two scenes hold system 0, every later file's scene system 1 (with `unit.length_unit`
  3); every scale is 1;
- two more demo files, by reference (`crates/blend/tests/corpus/unit-probes.tsv`, never
  committed): Classroom (Christophe Seux, listed CC0) and Barcelona Pavilion (eMirage, listed
  CC BY, version not stated by the publisher). Each mixes systems: the classroom's main scene
  holds 1 and its two other scenes 0, the pavilion's five render and benchmark scenes 1 and its
  `assets` scene 0 (the classroom's eleven asset files, probed once and not listed, hold 0);
  every scale is 1. Neither gives metres per unit.

## Meshes (the mesh read)

The mesh read (`crates/blend/src/meshread/mesh.rs`) finds a mesh's positions and polygons by
name in the file's own schema. The names, and which of three layouts each version saves, were
found by listing the `Mesh` structs and every mesh's arrays in the sample files above
(`crates/blend/tests/corpus_mesh.rs`, `mesh_schema_survey`; names and numbers only):

- **2.73 and 3.06:** the vertex, polygon and corner structs. `mvert` points at `MVert` blocks
  (`co`, three floats), `mpoly` at `MPoly` blocks (`loopstart`, `totloop`), `mloop` at `MLoop`
  blocks (`v`); the mesh's `totvert`, `totpoly` and `totloop` count them. No position layer.
- **4.03:** named attribute layers. The mesh's `vdata` (a `CustomData`: `layers`, `totlayer`)
  holds a `CustomDataLayer` named `position` whose `data` is a block of `vec3f`, one per vertex;
  `ldata` holds one named `.corner_vert` whose `data` is a block of `MIntProperty`, one per
  corner; `poly_offset_indices` is an untyped block of `totpoly + 1` ints, from 0 to
  `totloop`. No `mvert`, `mpoly` or `mloop` block.
- **5.00 and 5.01:** the attribute store. The mesh's `attribute_storage` (an
  `AttributeStorage`: `dna_attributes`, `dna_attributes_num`) lists `Attribute`s, each with a
  `name` (a string block) and `data`, which points at an `AttributeArray` (`data`, `size`)
  holding untyped blocks of three floats per vertex for `position` and one int per corner for
  `.corner_vert`; `poly_offset_indices` as in 4.03; the `CustomData` layers are empty. An
  attribute may instead point at an `AttributeSingle`; `position` and `.corner_vert` never did.

No sample file saves two layouts for one mesh. The read takes the first it finds whole, in
this order: the attribute layers, the structs, the attribute store; a row on test files
written in two layouts checks they read alike. The layers' and attributes' type numbers seen (position 48, `.corner_vert` 11 in 4.03;
data types 7 and 3, domains 0 and 3 in 5.x) are recorded here only: the read goes by names,
struct sizes and block lengths, and never by these numbers. One 5.01 mesh has vertices and no
polygon; it has nothing to draw.

## Cameras (the mesh read)

The mesh read (`crates/blend/src/meshread/camera.rs`) reads each camera's `type`, `lens`,
`sensor_x`, `sensor_y`, `sensor_fit` and `clipsta` by name (the scene read's camera record), and
the drawn scene's render size from its `r` (`RenderData`: `xsch`, `ysch`, `xasp`, `yasp`). The
listing of the sample files above found these names in every version (2.73 to 5.01), with
`sensor_fit` a `char`, `xsch` and `ysch` ints and `xasp` and `yasp` floats.

- **`sensor_fit`'s values are provisional:** `0` auto, `1` horizontal, `2` vertical. Only `0` is
  seen in the sample files (four cameras); the other two meanings are taken from the order the
  program's documentation lists the fit's choices in, not from a file saved in a known state.
  They stay provisional until a re-save of a camera in each fit, read back by the program
  itself, pins them; any other value leaves the camera out (`camera_sensor_fit_unknown`).
- **`lens` is read as millimetres** whatever unit the program shows it in; `type` `0` is a
  perspective camera (`1` orthographic, `2` panoramic: left out).
- **Camera shift** (`shiftx`, `shifty`, all `0` in the samples) is not read.

## The writer's round trip

The writer (`crates/blend/src/write.rs`) was checked the same way, on the same node and
releases: files the program saved there were patched by `examples/blend_patch.rs`, the program
opened each patched file headless, reported what it read of every object, and saved it again;
`crates/blend/tests/roundtrip.rs` checks both that the program read the values written and that
our reader finds them held in the program's save. The patched and re-saved files stay on the
node. A second driver script, kept outside every repo, ran the program (checksum below).

## Running the saved-file and round-trip rows, by name

Both rows read files that stay on the build node, so the gate's everyday run
(`scripts/check.sh`) leaves them out (`#[ignore]`d), and each fails without its folder. On the node, after the runs above:

- `every_saved_state_indexes_as_it_was_set` (`crates/blend/tests/saved.rs`): `BLEND_SAVED_DIR`
  names the saved files' folder, one subfolder per release;
  `BLEND_SAVED_DIR=DIR cargo test --locked -p k2-mpc-blend --test saved -- --ignored --exact
  every_saved_state_indexes_as_it_was_set`.
- `the_program_reads_our_writes_and_keeps_them` (`crates/blend/tests/roundtrip.rs`):
  `BLEND_ROUNDTRIP_DIR` names the round trip's folder, one subfolder per release, each with
  `values.tsv` and `resaved/`; `BLEND_ROUNDTRIP_DIR=DIR sh scripts/roundtrip.sh`, the gate's
  round-trip part. It refuses a folder or a run that checked nothing (a release's folder, report
  or saves missing or empty; no test run; a release not checked, or with no objects seen), and
  `scripts/check.sh` runs it when a change lands (`CHECK_TARGETS_LANDING=1`), so a landing
  can't skip it. Its self-test, `scripts/test-roundtrip.sh`, runs in every gate.

Nothing these runs read or print is committed.

## Private notes

By hash only. The driver scripts (kept outside every repo): the saves, sha256
`ecb92a089e3a324893e00e401ec4928e6eb4f9b19b9250f9a5cba08e5433d4a2`; the writer's round trip,
sha256 `0cccd50492267302f36a553aa2c9c6d1aea54770c78eedfb8875fca2848ede74`. The releases it ran on:
3.6.23 `0e9a18af4d0060b825e9617e24a775f759e0f9f67271c062f3d53a539030af00`, 4.5.9
`dcdc3eca6c9825bb35a8033b689c053f3cb5a9b0cd2a61b2eac2a49436b4ad3d`, 5.1.2
`aaccb355f50183979b698bcce7467103a76261b5fa59f4972295842662a285fb`.
