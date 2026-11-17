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

**Files of 5.02 and 5.03** (`NAMES_TO` is 503; the mesh read and the live tier read them,
`scene::READ_TO`, and the writer writes them, `write::WRITE_TO`: "The program's own saves,
patched" below) are named by the same
tables. What pins them is the round trip ("The round trip on our own scenes"), not one saved
state per value: on the program's own saves at 5.2.4 and 5.3.0 (its builds of our scene spec
and its re-saves of our inputs), every object's type, rotation mode and three flags (disabled
in viewports, not selectable, disabled in renders) as the program reports them equal what our
reader names for the same object in the save; the types seen are empty, mesh, camera, light and
armature, the rotation modes XYZ and quaternion, and each of the three flags is set on some
object. A collection's viewport bit is pinned by the drawn objects' `hidden`, and a camera's type
1 by the program's own orthographic camera, which the mesh read leaves out. The other values in
the tables (the other object types, the light types, camera type 2) rest on the 3.6.23, 4.5.9
and 5.1.2 saves and on the schema: a listing of those 5.02 and 5.03 saves' schemas against the
5.01 sample's (structs and fields by name and type only, never committed) found every struct and
field the reader names under the same name and type, but one: `Object.restrictflag` is an `int`
in 5.02 and 5.03 (a `short` up to 5.01), which the reader reads as the file's schema types it.
The `Lamp`, `Camera` fields read, `RenderData` sizes and aspects, `CollectionObject.ob`,
`UnitSettings`, `AnimData`, the mesh's attribute store and the `ID` header are the same; the
fields 5.02 and 5.03 add (a collection's `importer`, a collection element's `sort_index`, a
scene's compositor effects) are not read.

Seen but not named: a view layer's own states. An object hidden in a view layer changes
several bits of its base's flag at once (state the program derives), and a layer collection's
`flag` takes 16 when excluded and 128 when hidden; the model doesn't read view layers. Files
older than 3.06 keep their numbers only: no release before it was run.

## What drives a transform (the live pack, the writer)

`crates/blend/src/anim.rs` reads, by name from the file's own schema, an object's `adt` (its
animation data), `constraints` (a list) and `action`; the animation data's `action`, `tmpact`,
`slot_handle`, `tmp_slot_handle`, `nla_tracks` and `drivers`; a track's `strips`, a strip's
`act`, `action_slot_handle` and `strips`; an action's `curves` list and its
`strip_keyframe_data_array`, each keyframe data's `channelbag_array`, each bag's `slot_handle`
and `fcurve_array` (each array with its `_num`); and an F-curve's `rna_path` and
`array_index`. The `Object` and `AnimData` names were found by listing those structs of the
3.06, 4.03 and 5.01 samples above. The rest were found by listing the `bAction`, `AnimData`,
`NlaTrack`, `NlaStrip`, `ActionStripKeyframeData`, `ActionChannelBag` and `FCurve` structs of
the program's own saves kept here (`crates/blend/tests/builds/`, 5.2.4 and 5.3.0) and following
the blocks of their two animated objects: in both releases an action's F-curve is in its
channel bag for the object's slot and in its `curves` list too, its path a block of its own
(`location`, `rotation_euler`) ending in a NUL. The row
`the_programs_saves_read_per_channel_through_bags_and_list` reads them so, through either
alone.

What the reading means was checked on the program, not assumed: a build of the writer that
writes 5.02 and 5.03 (a trial, not this release's) patched the program's own save of
`driven_quat.blend` (`Driven`, its location's z animated by an action
with keyframes 0 at frame 1 and 2 at frame 60) to the location (-3, 2, 1), and the program,
opening that file, kept x and y (-3, 2) and re-evaluated z from the action to 0, in its 5.2.4
and 5.3.0 alike. So a value written to an animated channel doesn't hold, and one written to a
channel nothing animates does; the writer refuses the first and writes the second. That a
driver's channel and an NLA strip's are re-evaluated the same way is taken from their being
evaluated as the action is (not checked on the program); refusing them is the safe side. A
constraint acts on the placement the channels give, not on the stored values: the live tier,
which places objects as the program shows them, refuses it whole; the writer writes under it.

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

- **`sensor_fit`'s values:** `0` auto, `1` horizontal, `2` vertical, pinned by the round trip
  ("The round trip on our own scenes"): the program, at 5.2.4 and 5.3.0, opened our
  `camera_fits_wide` (1920 x 1080, square pixels) and `camera_fits_tall` (1080 x 1920, pixels
  twice as tall as wide) scenes, one camera in each fit with its own lens and sensor, named each
  fit as written and reported each camera's vertical field of view, which equals the mesh
  read's for all six cameras within 1e-6 rad (the image's aspect at or over 1 and under 1). Only
  `0` is seen in the sample files. Any other value leaves the camera out
  (`camera_sensor_fit_unknown`).
- **`lens` is read as millimetres** whatever unit the program shows it in; `type` `0` is a
  perspective camera (`1` orthographic, `2` panoramic: left out).
- **Camera shift** (`shiftx`, `shifty`, all `0` in the samples) is not read.

## Generated test files

The test scenes (`testgen::scenes`, written by `SceneBuilder` from a schema of our own) are
opened and saved again by the program that writes BLEND files, so they hold what that program
expects of a saved scene, learned from the sample files above by struct and field name only
(their own schemas and values, listed by a test survey), never from the program's source:

- **Embedded data-blocks are flagged.** In every sample from 3.06 on, a scene's master
  collection is a `DATA` block of struct `Collection`, and every node tree inside another
  data-block a `DATA` block of struct `bNodeTree`; each carries the ID flag 1024
  (`testgen::EMBEDDED_ID_FLAG`), and each scene has one master collection. The program warns
  about a master collection without it and repairs it while loading, which is where it failed
  on our files. The generator writes it on every master collection; the row
  `public_corpus_embedded_ids_carry_the_embedded_flag` checks the samples keep the rule.
- **What every sample scene holds beyond the scene graph** (`SceneBuilder::saved`): its tool
  settings (`toolsettings`, a `ToolSettings`), its render views (`r.views`: two
  `SceneRenderView`s, `left` with suffix `_L` and `right` with `_R`), one view layer
  (`view_layers`: a `ViewLayer` named `ViewLayer`) whose layer collections (`LayerCollection`)
  mirror the master collection and its children, and whose bases (`Base`) are the objects the
  scene holds, each once. A master collection's `owner_id` names its scene (a field of
  `Collection` from 4.03 on; in 3.06 it sits in the collection's runtime struct). The row
  `public_corpus_scenes_hold_view_layers_tool_settings_and_render_views` checks the samples
  hold all of it.
- **The values written** are those the samples agree on: a view layer's `flag` 5, `layflag`
  32767, `passflag` 1, `pass_alpha_threshold` 0.5, `cryptomatte_flag` 8, `cryptomatte_levels`
  6; a layer collection's and a base's `local_collections_bits` -1; a base's `flag` and
  `flag_from_collection` 214 (an object not selected); a master collection's `flag` 32 and
  `color_tag` -1; a handful of tool settings (`vgroup_weight` 1, `doublimit` 0.001,
  `object_flag` 1, `selectmode` 1, `uvcalc_flag` 5, `uv_selectmode` 1, `uvcalc_margin` 0.001,
  `uvcalc_iterations` 10, `uvcalc_weight_factor` 1, `proportional_size` 1, `select_thresh`
  0.01, `autokey_mode` 2). Every other field is left out of the schema, which the program reads
  as absent. What these numbers mean beyond that the samples hold them is not claimed.

## The round trip on our own scenes

The writer and the mesh read are checked against the program that writes these files, run as
a separate process on a build node: its official Linux builds 5.2.4 and 5.3.0, each checked
against its published sha256 before every run, started without user settings or scripts from
the files and without audio, one file per process, under a time cap, as an unprivileged user
with no network and no credentials. Its inputs: the scenes `testgen::scenes` generates
(`examples/blend_scenes.rs`) but the living room's family (the program files their local data
as library data, because their library block comes before it; the program's own build of that
scene takes their place), and the writer's patches (`examples/blend_patch.rs`) of four of the
public CC0 sample files above, which the program itself saved (3.06, 4.03, 5.00 and 5.01; fetched by sha256, never
committed, their saves never committed), all listed with their sha256 in the input folder's
manifest, which the run checks first. The program opens each input, reports what it reads of every object (one line
per object, matched here by file and name), and saves it again; only those saves and its logs
come back, kept on the node or in private storage, recorded by sha256 with the program's version,
and never committed. `crates/blend/tests/roundtrip.rs` checks that the program saw every input
and object, that each mesh object's evaluated world matrix equals our composition (driven
objects left out), that its triangle count equals ours, that each camera's vertical field of
view equals ours (which pins the sensor-fit meanings, "Cameras"), that the patched values were
read, that our reader finds them in the program's saves, and that it names each object's type,
rotation mode and flags in those saves as the program reports them ("Flags and values"). The
same run has the program build each scene of a spec of our test scenes itself, save it and
report it: on those saves the same checks run, with the live tier's (each save opens live: a
write-back of nothing is the file's own bytes, and a change, the first object the live tier
places moved, is written back and found by our reader), and each object the program links from a library, which its save holds as a
bare `ID` header whose stored name starts with `OB`, is an object linked in our reader (its
kind from that type code) and never drawn.
The program evaluates no object disabled in viewports (by its own flag, or by every collection
holding it) and reports its world matrix as exactly the identity: the row requires that, and
checks our matrix for such an object against the driver's own composition of every object's
world matrix (the parent's, the parent inverse, the object's transform with its delta
transforms; no constraint applied), and against one composed in the test alone, in `f64`, from
the program's own location, Euler rotation in its rotation mode's order, scale and parent chain.
For each object the program does evaluate, the driver's composition is checked against the
program's evaluated matrix (a parent that isn't an object is skipped, and counted).
The path scan reads the mesh read's text, K2SCENE1's JSON header, never its data section of
numbers.
The driver, kept outside every repo, is recorded by its sha256 in the run's record.

**The program's own saves, patched.** The writer writes 5.02 and 5.03 files on this evidence:
the writer, taking 5.02 and 5.03 as it does now, patched the program's own saves at 5.2.4 and 5.3.0 (its
builds of nine scenes of the spec, and its re-saves of the four public samples), each file as
`crates/blend/tests/roundtrip.rs` lists it (`OWN_SAVES`): objects moved, a scale, each of the
three visibility bits set or cleared, renames, among them a camera, an object in a hidden
collection, the last of 2,000 objects, an object with a quaternion rotation and one whose
location z an action animates. Each input was checked by its sha256 against the run that made
it before it was patched; the patched files are recorded by sha256 and never committed. The
program, run as above, reopened each patched file, reported every object and saved it again.
`the_program_reopens_our_writes_of_its_own_saves` checks, per release and file: every patched
value as the program reports it after reopening; nothing else of any object changed against
the program's report of the same file before the patches (the round trip's reports, matched by
name, renames mapped), every column compared as printed, but the evaluated world matrix and
`world_eval` of a moved object and its descendants and of an object whose viewport bit was
patched, which are checked instead by the referee rule after the patches (the evaluated world
equal to the driver's composition, the identity for an object not evaluated) and, for one not
evaluated, `world_eval` against a composition made in the row alone from the program's values;
the one value the program doesn't keep, the animated z, re-evaluated from its action to 0 (the
writer refuses that change, `blend_driven`); and our reader finding every patched value in the
program's re-save. The run's outputs are recorded by sha256 (their archive's
`ae18518aeb48916ba464d24e25b20dc79a8685f7efe9888d1082c83177056c79`) and stay private. The live tier writes back through the
same writer (`live::splice`, which turns the live store's changes into the writer's edits);
`examples/blend_live_edit.rs` runs one live edit through the kit's session, as a host does, for
a run that has the program reopen the live write-back too.

**Our animated and constrained edits, reopened by the program.** The program reopened our writes of
its own saves of a constrained scene and an animated-visibility scene at 5.2.4 and 5.3.0. Values
written held; a written location under constraints held, with its placement following the
constraints; a cleared animated visibility bit was restored by its animation (the edit the writer
refuses). A private row compares our edits with the program's report after reopening; it runs where
the private outputs are. The inputs: the program's own builds at 5.2.4 and 5.3.0 of two test scenes,
one with an animated visibility bit (an action keys `Blinking`'s viewport bit, set at the first
frame; a driver sets `Ghost`'s render bit), one with an object under a track-to and a child-of
constraint (`Follower`, tracking `Target`, a child of `Anchor`). The writer disabled `Plain` in
viewports and refused `Blinking`'s animated bit (`blend_driven`); that bit was then cleared in the
file by a raw byte patch, in a test tool alone; the live tier (`examples/blend_live_edit.rs`)
disabled `Plain` in a second copy. The writer moved `Follower` (with its warning,
`blend_constrained`) and `Free`; the live tier moved `Free` in a second copy and refused
`Follower`'s placement. Each build was checked by its sha256 before it was edited; the edited files
and their record (`PATCHES.tsv`) are an archive of sha256
`13ab429357d1efbf5a8d2285b41d060418c65f54179047c5dcf7fa54fac0eeb1`, never committed. The program,
run as above, reopened each, reported every object, logged each F-curve, driver and constraint (its
axes, target and inverse matrix), and saved it again: an archive of sha256
`2457e4ed8eb5c3d43042c561c81d5a31aa10ae7c6987441ec43b019a29238bc9`; its reports of the two builds
before the edits are archives of sha256
`37adbdca628eb1bd3321ecb83b35d547282312714b11bd65c6957c138a1d1005` (the animated scene) and
`7c8bd448ce1ccd90ec932e828022736083ddd32b24608f9e003f73187ab38f75` (the constrained scene); all stay
private. `the_program_reopens_our_animated_and_constrained_edits` compares, per release and file:
each edit with the program's report after reopening; `Blinking`'s bit, clear in our input as our
reader reads it, with the program's report, which the row expects to hold the keyed value (set);
every other column of every object with the report before, by name, as printed, but the edited ones
and an edited object's world, whose `world_eval` is compared with a composition of its own values
instead (an object not evaluated, `Plain` once disabled, is expected to have the identity world);
`Follower`'s evaluated world with its constraint stack composed in the row in `f64`, in stack order,
within 1e-5 (the track-to aiming its -Z axis from its location at `Target`'s world position, its +Y
toward the world's +Z, the axes read from the log; then the child-of composing `Anchor`'s world and
its identity inverse on it), and its `world_eval`, which leaves constraints out, with its location's
translation; every other evaluated object's world with `world_eval`; and our reader's read of our
input and of the program's re-save with each edit (`Blinking`'s bit expected set in the re-save).
The writer leaves three cached fields as they are, which the program refreshes on load, as its
reopening of our writes shows: an object's `constinv` (its constraint stack's stored inverse)
after a location written under constraints, and its `base_flag` and its view layer `Base`'s
`flag` after a hide.

The builds at 5.2.4 and 5.3.0 are kept in this repo (`crates/blend/tests/builds/`): each scene
built by the program from the pack's own spec (`scene-spec.json`, written for these tests, of
our own numbers), saved, and reported by it, with `MANIFEST.tsv` listing each file's size and
sha256. `tests/builds.rs` reads them in every gate as the round trip reads its builds. They are
the pack's own test data (`NOTICE.md`); the saves of the public samples and of our other inputs
stay outside the repo.

## Running the saved-file and round-trip rows, by name

Both rows read files that stay on the build node, so the gate's everyday run
(`scripts/check.sh`) leaves them out (`#[ignore]`d), and each fails without its folder. On the node, after the runs above:

- `every_saved_state_indexes_as_it_was_set` (`crates/blend/tests/saved.rs`): `BLEND_SAVED_DIR`
  names the saved files' folder, one subfolder per release;
  `BLEND_SAVED_DIR=DIR cargo test --locked -p k2-mpc-blend --test saved -- --ignored --exact
  every_saved_state_indexes_as_it_was_set`.
- `the_program_reads_our_writes_and_keeps_them` (`crates/blend/tests/roundtrip.rs`):
  `BLEND_ROUNDTRIP_DIR` names the round trip's folder, one subfolder per release, each with
  `values.tsv` and `resaved/` (the releases 5.2.4 and 5.3.0); `BLEND_ROUNDTRIP_DIR=DIR
  BLEND_OWN_SAVES_DIR=DIR sh scripts/roundtrip.sh`, the gate's
  round-trip part. It refuses a folder or a run that checked nothing (a release's folder, report
  or saves missing or empty; no test run; a release not checked, or with no objects seen), and
  `scripts/check.sh` runs it when a change lands (`CHECK_TARGETS_LANDING=1`), so a landing
  can't skip it. Its self-test, `scripts/test-roundtrip.sh`, runs in every gate.
- `the_program_reopens_our_writes_of_its_own_saves` (`crates/blend/tests/roundtrip.rs`):
  `BLEND_OWN_SAVES_DIR` names the folder of the program's reopened patches of its own saves,
  one subfolder per release, each with `values.tsv` and `resaved/`, and `BLEND_ROUNDTRIP_DIR`
  the round trip's, whose reports are the ones before the patches; the same part runs it, and
  refuses its folder missing, a release's folder, report or saves missing or empty, no test
  run, or a release not checked.
- `the_program_reopens_our_animated_and_constrained_edits` (`crates/blend/tests/roundtrip.rs`):
  `BLEND_REOPENED_DIR` names the folder of the program's reopening of our animated and
  constrained edits, one subfolder per release, each with `values.tsv`, a `<file>.log` per file
  and `resaved/`; `BLEND_REOPENED_IN` our edited files and their record (`PATCHES.tsv`, one
  subfolder per release); `BLEND_REOPENED_ANIMATED_BEFORE` and
  `BLEND_REOPENED_CONSTRAINED_BEFORE` the program's reports of the two builds before the edits
  (one subfolder per release, each with `values.tsv`). The same part runs it, and refuses any of
  the four folders missing, the record empty, a release's folder, report, saves, logs, inputs or
  report before missing or empty, no test run, or a release not checked.

Nothing these runs read or print is committed, but the builds kept in `crates/blend/tests/builds/` (above).

## Private notes

By hash only. The driver script of the saves (kept outside every repo), sha256
`ecb92a089e3a324893e00e401ec4928e6eb4f9b19b9250f9a5cba08e5433d4a2`; the releases it ran on:
3.6.23 `0e9a18af4d0060b825e9617e24a775f759e0f9f67271c062f3d53a539030af00`, 4.5.9
`dcdc3eca6c9825bb35a8033b689c053f3cb5a9b0cd2a61b2eac2a49436b4ad3d`, 5.1.2
`aaccb355f50183979b698bcce7467103a76261b5fa59f4972295842662a285fb`. The round trip on our own scenes
records its driver's sha256, the program's version and archive sha256, and every output's sha256
in each run's record, beside its outputs.
The run whose outputs back this release (the per-file round trip and the builds, at 5.2.4 and
5.3.0): the driver script, sha256
`2ad3c3446ff7a37b5b0ab37a0d4f8738ddd8f6e31de60274665c930ec61bf5f5`; the build script,
`805c1ff747ec6b03dfeb6bddb73579739f8e5fcaf606832cb4d8c0bf5af4bcd5`; the run's own script,
`8a522117b5f3cdee545dd85cf25dffe76f6c302c2dc920fed3170b0a1014dc42`; the releases it ran on: 5.2.4
`2fd14c50987426897ba60237ee996edf2b4151f4b1a72a460ca20be58c1d2d86`, 5.3.0
`dacb8264197a3f7fda6001285a5a506fc97ee943881261c2c3bd63c882f22e05`.
The run that reopened the writer's patches of the program's own saves (output archive sha256
`ae18518aeb48916ba464d24e25b20dc79a8685f7efe9888d1082c83177056c79`): the run's own script,
`447f95bea2b368eb138fe6f0a80e139d328e28318b3558613ce440cd3c031334`; its unit script,
`99bea02dd129b1ed239618a37eebc247778967c50c545f88c813c6c714b0d631`; the driver script,
`2ad3c3446ff7a37b5b0ab37a0d4f8738ddd8f6e31de60274665c930ec61bf5f5`; the same releases.
The run that reopened our writes of its own saves of a constrained scene and an
animated-visibility scene (output archive sha256
`2457e4ed8eb5c3d43042c561c81d5a31aa10ae7c6987441ec43b019a29238bc9`): the run's own script,
`2b0c333747ab8e4b29db3629644ef3c79c662c15193fc9e788849b556a1683ae`; its unit script,
`99bea02dd129b1ed239618a37eebc247778967c50c545f88c813c6c714b0d631`; the driver script,
`b29360a1feb8bc88e1d833f40435612ae22f222bd7571af3b6069e50e8002d61`; the same releases.
