# What the BLEND pack sends: the mesh read and the live view

The shapes a client codes against. Two outputs carry a scene:

- **The mesh read** (`meshread::mesh_read`): one version's geometry as K2SCENE1, made by the
  host once per version and fetched by a client. Geometry once.
- **The live view** (the kit's view index for this pack's live tier): objects and collections
  as an outliner placed in 3D, with every change as it happens. Changes live.

A client joins the two by address: an object's `key` in K2SCENE1 is its unit's address in the
live view, and its mesh's `hash` in K2SCENE1 is the unit's `mesh_hash` there.

## Addresses

| Element | Address | Where it comes from |
|---|---|---|
| an object | `@obj:N` | its live key `oN` |
| a camera | `@cam:N` | the live key `oN` of the camera's object: the same `N` as its `@obj:N` |
| a collection, or a scene's master collection | `@coll:N` | its live key `cN` |
| objects in no collection | `@coll:0` | the key `c0` |

`N` is a decimal number with no sign and no leading zero. A key never changes while the doc
lives: renames, moves and edits keep it, so a view that follows `@obj:15` keeps following it.
Keys are minted when a file is first imported, `o1`, `o2`, ... for the objects and `c1`, `c2`,
... for the collections, in the order the file holds them; an object added later takes the next
number, and a number is never used twice. The mesh read reads a version's bytes; it is given
the keys the live store holds for that version's objects (the carrier's pairing), and without
them takes the keys an import of those bytes would mint.

## The mesh read: K2SCENE1

```text
magic    8 bytes  "K2SCENE1"
hlen     u32 LE   the header's length
header   hlen     UTF-8 JSON (below)
pad      0-3      zero bytes up to a 4-byte boundary
data     per mesh, in header order: positions f32 LE x 3 x vertices,
         indices u32 LE x 3 x triangles
```

```json
{"version": 1, "up": "z",
 "meshes":  [{"vertices": 24, "triangles": 12, "normals": false,
              "hash": "9b1f...64 hex digits"}],
 "objects": [{"id": "Sofa", "key": "@obj:15", "mesh": 7, "base_color": [0.8, 0.8, 0.8, 1],
              "transform": [16 floats, column-major],
              "modifiers": ["Subsurf"], "modifiers_not_applied": true,
              "transform_not_evaluated": true, "hidden": "collection"}],
 "cameras": [{"id": "Camera", "key": "@cam:1", "position": [-2.6, -3.4, 1.35],
              "forward": [0.58, 0.8, -0.16], "up": [0.09, 0.13, 0.99], "yfov": 0.3996,
              "aspect": 1.7777778, "znear": 0.1, "active": true, "lens_mm": 50,
              "sensor_mm": [36, 24], "sensor_fit": "auto"}],
 "cameras_left_out": [{"name": "Top", "key": "@cam:7", "reason": "camera_not_perspective"}]}
```

Fields are written in the order shown; an optional field that doesn't apply is left out, never
`null`. Every float is finite and written as the shortest text that reads back to the same
`f32`. The same version's bytes always give the same output, byte for byte, whatever the file's
storage (plain, gzip or zstd), so objects keep their order and index across reads of one
version.

### Meshes

- One entry per mesh data-block that a written object uses, written once however many objects
  use it, in the order the file holds the meshes.
- `vertices`, `triangles`: counts. Only vertices a triangle uses are written, so the indices are
  renumbered from the file's; every index is below `vertices`.
- `normals`: always `false`: the renderer shades from the triangles.
- **`hash` (always present):** the mesh's content hash, 64 lowercase hex digits: SHA-256
  of `vertices` as u32 LE, then `triangles` as u32 LE, then the mesh's data exactly as written
  in the data section (its positions, then its indices). A client can check it against the
  bytes it received. Equal meshes give equal hashes, in one file or across files and versions,
  so a client caches geometry by it; a mesh whose geometry changes gets a new one. Objects that
  share a mesh share its hash.

### Objects

The mesh objects of the scene drawn (below), in the order the file holds them.

- `id`: the object's name as the live view shows it.
- **`key` (always present):** `@obj:N`, the object's address. Unique among the objects.
- `mesh`: the index of its mesh in `meshes`.
- `base_color`: `[0.8, 0.8, 0.8, 1]` for every object.
- `double_sided`: not written (both sides are drawn).
- `transform`: object to world, column-major, composed from the stored location, rotation and
  scale through the object's parents and their parent inverses; animation, drivers and
  constraints are not evaluated. A world matrix the file may cache is never read.
- `modifiers`: the names of its modifiers in order, as the file's own schema names them (a
  subdivision modifier is `Subsurf`): at most 256, each at most 128 bytes. `modifiers_not_applied:
  true` whenever there is one: the mesh is drawn without them.
- `transform_not_evaluated: true`: animation, a driver or a constraint drives the object's
  transform, and the stored values are drawn.
- `hidden`: `"self"` when the object's own viewport flag hides it, `"collection"` when a hidden
  collection hides it (the rule is [Hidden by](#hidden-by)); absent when shown. A hidden object
  stays in the output with its mesh and its index.

An object the read can't draw honestly is left out, never placed by guess, and named in the
read's report with a reason: an object or mesh linked from a library, a delta transform that
isn't identity, a parent that isn't an object, a world matrix that can't be composed, a mesh
with no triangle. A collection instance is not expanded.

### Cameras

The camera objects of the scene drawn, in the order the file holds them, perspective cameras
only (an orthographic or panoramic camera is left out and reported).

- `id`: the camera object's name (what a person calls the camera, not its data's name).
- `key`: `@cam:N`, `N` its object's live key. Unique among the cameras. A camera is not also an
  object here: `objects` holds mesh objects only.
- `position`: the world matrix applied to the origin.
- `forward`: the camera's local -Z through its world matrix, normalised.
- `up`: its local +Y through its world matrix, made orthogonal to `forward`, normalised.
- `yfov`: the vertical field of view in radians, in (0, pi), from the lens, the sensor, the
  sensor fit and the scene's render size and pixel aspect.
- `aspect`: the render width over its height, pixel aspect included.
- `znear`: the camera's near clip distance.
- `active: true` for the scene's own camera, `false` for the others; at most one is `true`.
- `lens_mm`, `sensor_mm` (`[width, height]`): as the file stores them.
- `sensor_fit`: `"auto"`, `"horizontal"` or `"vertical"`. A camera whose fit the pack doesn't
  know has no honest `yfov`, so it is left out and reported, like an orthographic one.

### Cameras left out

`cameras_left_out` lists the camera objects of the scene drawn that `cameras` doesn't hold, so
none is missing silently; the field is left out when there are none. Each has `name` (the camera
object's name), `key` (`@cam:N`, when it has a live key) and `reason`, a fixed word:

| `reason` | Why |
|---|---|
| `camera_not_perspective` | an orthographic or panoramic camera |
| `camera_sensor_fit_unknown` | a sensor fit the pack doesn't know, so no honest `yfov` |
| `camera_no_data`, `camera_no_lens`, `camera_no_clip` | the camera's data, lens or clip distance missing |
| `camera_no_aspect` | the scene's render size gives no aspect |
| `camera_degenerate`, `camera_bad_fov` | no direction, or a field of view outside (0, pi) |
| `linked_object`, `transform_unplaced`, `transform_not_finite`, `delta_transform`, `parent_not_object`, `parent_unlisted` | as for objects |

A reader that predates the field ignores it.

### The scene drawn

The file's current scene: the one the file names as current, or, when that is missing or names
no scene, the first scene the file holds. Objects and cameras are the ones that scene holds
through its master collection and the collections below it.

### Refusals

A read is refused only when the file can't be read at all or a limit is crossed; a problem
with one object or mesh leaves that object out instead. Each refusal has a code, its next step
and its numbers, and quotes nothing from the file:

| Code | When |
|---|---|
| `too_large` | a count, the header, the output, or the read's memory (512 MiB at most, by default) over its limit |
| `not_blend`, `unknown_form`, `unreadable` | not a file the pack reads |
| `live_version` | a version the pack doesn't read |
| `live_ambiguous` | a pointer the reader can't resolve on a listed element |
| `pairing_mismatch` | the keys given for the version don't fit its objects (their count differs) |
| `invalid` | a value the writer would refuse to write (a fault of the pack, never of the file) |

### A live state's K2SCENE1, without a whole read

While a doc is live, a host makes the K2SCENE1 of its live state without reading the file again
(`live::live_k2scene(state, carrier, limits)`, called with the state and carrier the kit hands
the pack's view hook). No live change makes or changes geometry, so every mesh, and its hash, is
the carrier's (the version the kit last read from the file or wrote to it); only the header
changes. The output comes in **two parts**:

- `header`: `magic`, `hlen`, the header and its pad, exactly as above;
- `data`: the carrier's data section, shared: every call over one carrier returns the same
  allocation, never a copy. Mesh offsets are relative to it, so a header of another length
  changes only the pad.

`header ++ data` is byte for byte the whole read (`meshread::mesh_read_paired`) of a version that
holds the live state under its keys: the version a write-back of the state produces, for a
state the write-back writes. A host that needs one buffer joins the two; one that can send the
parts separately skips a copy of the data.

**`carrier_id`**: SHA-256 of `data`, 64 lowercase hex digits. Equal across calls exactly when the
data section is byte-identical: through live edits, through a write-back or an outside save
that changes no mesh. A host keeps what it built from the data (its buffers) by it.

**Three lists, as hints** (`@obj:N`, `@cam:N`), comparing the output's objects and cameras with
the carrier's own read under the same keys: `changed` (in both, written differently: a move, a
turn, a parent's move, a hide, a rename), `added` (in this output only, as when an object is
brought into the scene drawn), `deleted` (in the carrier's read only: deleted, or out of the
scene drawn). Objects come first, then cameras. The header is what counts; the lists say what
to draw again.

**Declined**, each with a fixed code, and every one with the same fallback: the whole read of
the version a write-back of the live state produces.

| Code | When |
|---|---|
| `reuse_no_carrier_read` | the carrier's own read refused (a limit), or the carrier doesn't read live |
| `reuse_pairing` | the carrier has no pairing, or one that doesn't fit its objects and collections |
| `reuse_mesh_not_held` | a drawn object's mesh is one the carrier's read doesn't hold |
| `reuse_no_facts` | a drawn object the carrier holds no facts for: added since the carrier, or its data changed |
| `reuse_drawn_scene` | the scene drawn's master collection was deleted |
| `reuse_collections` | a collection added, deleted or moved, or its kind or parent changed |
| `reuse_meshes_changed` | a mesh of the carrier's read that no drawn object uses: the data section would differ |
| `reuse_unsupported` | the objects' order, an object's type, parent, rotation mode or quaternion changed, its name removed, a collection holding it not listed, or a parent or the scene's camera deleted |
| `too_large`, `invalid` | a limit crossed, or a writer check (as for the whole read) |

The carrier is decoded once per carrier (by the view, or the first call); no call decodes a
file. The carrier's read is kept while the carrier is: its data section (at most the output
limit, 256 MiB by default), not the whole-file decode.

## The live view

The kit's keyed watch carries the view index (the kit's wire reference: the keyed watch, view
deltas, derived values). Containers are collections and each scene's master collection, in
file order, then `@coll:0` for the objects in no collection; each container's `parent` is the
collection or scene holding it first. Units are objects, in their first collection, with
`kind` (`mesh`, `camera`, `light`, `empty`, ...), `name` and `hidden` (the object's own viewport
flag), and their placement on the kit's 3D slot.

### Derived values per unit

Texts on each unit's `derived`, and in a change frame's `derived.units.<address>.<name>` (`null`
when the unit has none any more):

| Name | Value | Absent when |
|---|---|---|
| `transform` | `loc x,y,z rot x,y,z scale x,y,z` | never, for an object |
| `data` | the object's data: `mesh @me:Sofa`, `camera @ca:Camera` | the object has none (an empty) |
| `mesh_hash` | the `hash` its mesh has in the mesh read: the same 64 hex digits, so a client finds the geometry it cached | the object has no mesh, or its mesh is one the mesh read leaves out |
| `hidden_by` | `self`, or `collection:<address>` naming the outermost hidden collection that hides it (`collection:@coll:3`) | the object is shown in the current scene, or isn't in it |

`mesh_hash` changes only when the mesh does; moving, hiding or renaming an object leaves it as
it was. Objects that share a mesh carry the same value. `hidden_by` changes on the object when
its own flag changes, and on every object below a collection whose flag changes. A collection
moved live sits under its new parent, after that parent's other children, for this walk.

Without the version the live store was made from (or with keys that don't fit it), the view
knows no collection tree, scene or meshes: `hidden_by` is `self` or absent, and `mesh_hash` and
`current_scene` are absent.

### Derived values per container

| Name | Value | Absent when |
|---|---|---|
| `current_scene` | `true` | on every container but the master collection of the scene the mesh read draws |

The outliner shows every scene as before; this marks which one the 3D view draws.

### Hidden by

Whether an object is hidden in the scene drawn, and by what. The same rule gives K2SCENE1's
`hidden` (`"collection"` for any `collection:<address>`) and the live view's `hidden_by`.

1. The object's own viewport flag is set: `self` (whatever its collections are).
2. Otherwise, if some path from the scene's master collection down through collections to a
   collection holding the object passes no hidden collection: shown (absent). A master
   collection is never hidden.
3. Otherwise, if no path reaches the object: absent (it isn't in the scene drawn).
4. Otherwise: `collection:<address>`, where the collection is the outermost hidden one (the one
   nearest the master collection) on the first path a depth-first walk finds from the master
   collection, visiting each collection's children in the order the file lists them, a
   collection holding the object ending the path, and a collection visited once.

The rule is pinned by shared test vectors, `crates/blend/tests/data/hidden-by.json`: each case
gives a scene's collections (each with its `hidden` flag and its children, in the file's order),
its objects (each with its own flag and the collections holding it), optionally `moves` (a
collection moved live, `{"collection", "to"}`, applied in order: taken out from under every
parent and put under `to` after its other children) and every object's expected value. The
pack's tests read the same file; a client's implementation can too.
