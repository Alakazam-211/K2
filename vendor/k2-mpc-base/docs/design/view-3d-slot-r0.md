# A unit's placement in 3D (design, r0)

A proposal, not built. How a pack declares, and the kit carries, where a unit sits in a 3D
scene, so a host can draw a live 3D doc (a scene's objects, a building model's elements) from
the view index with no pack code, as it draws a deck from the 2D geometry slot today.

**The problem.** The view schema's geometry slot (`ViewSchema::geometry`) is a 2D box in page
space: integer `x,y,w,h`, a rotation about the box centre, two flips. A 3D placement is a
location, a rotation and a scale in three axes, as decimals, often relative to another unit.
A pack that has one today carries it as an ordinary field of the keyed state (a scene pack's
`xform`), and at best writes its numbers as the unit's text. A watcher then can't place
anything without the pack's own parser, and a change frame can't say "this moved" in a form a
renderer reads.

---

## 1. What the kit carries

### 1.1 A placement, typed

```rust
/// Where a unit (or a container) sits: relative to another element, or to its container's
/// frame, as a short list of steps applied in order (the first outermost).
pub struct Placement {
    /// The address of the element this placement is relative to (a parent object, the
    /// storey an element stands in); `None`: the container's own frame.
    pub relative_to: Option<String>,
    /// One step for almost every unit; at most `PLACEMENT_STEPS_MAX` (4).
    pub steps: Vec<Step>,
}

pub enum Step {
    /// Translate, rotate, scale, in that order of application to a point: scale first,
    /// then rotate, then translate (the usual object transform).
    Trs { loc: [f64; 3], rot: Rotation, scale: [f64; 3] },
    /// An origin and two axis directions, as a building model writes them: the local Z axis
    /// and a reference direction for local X; `None` for the default (0,0,1) and (1,0,0).
    /// Not normalised by the pack or the kit: the renderer normalises and orthogonalises.
    Axes { origin: [f64; 3], z: Option<[f64; 3]>, x: Option<[f64; 3]> },
    /// A 4x4 affine matrix, column-major (the order scene formats and GPU APIs use), for a
    /// file that stores one.
    Matrix([f64; 16]),
}

pub enum Rotation {
    /// Angles in radians about X, Y and Z, applied in the order the word gives (`XYZ`,
    /// `XZY`, `YXZ`, `YZX`, `ZXY`, `ZYX`).
    Euler { order: EulerOrder, angles: [f64; 3] },
    /// `w, x, y, z`, as stored (not normalised).
    Quat([f64; 4]),
    /// A unit-less axis and an angle in radians.
    AxisAngle { axis: [f64; 3], angle: f64 },
}
```

**Why the file's own form and not a matrix.** Each pack hands the kit the numbers its file
stores, in the shape it stores them (location, Euler angles in the object's own order and
scale; or an origin and two directions). Nothing is computed on the way:

- **Exact.** The numbers in the index are the stored numbers. Two builds of the index agree to
  the bit on every machine, so the kit can compare placements for equality (a change frame
  says "moved" only when a number moved) and nothing in the kit depends on a platform's
  trigonometry.
- **Small.** A location, three angles and a scale are nine numbers; a matrix is sixteen.
- **No loss.** An Euler rotation of more than a half turn, a negative scale, a non-uniform
  scale under a rotation: a matrix loses which of several decompositions the file had, and a
  watcher showing "rot 0, 0, 3.4" would show something else.

A host that draws needs a matrix: the client library gives one (`placementMatrix`,
`worldMatrix`, section 3), in `f64`, documented as close to the file's own composition (to
rounding), never compared for equality.

### 1.2 The doc's space

One value per doc, on the snapshot's head beside `page`:

```rust
pub struct Space {
    /// The length unit's word (`m`, `mm`, `ft`, `scene-unit`) and how many metres one is,
    /// when the file says (`None`: the file has no real-world scale, a scene in plain units).
    pub length_unit: String,
    pub metres_per_unit: Option<f64>,
    /// Which axis is up in the container's frame: `Z` or `Y`.
    pub up: Axis,
    /// Right-handed for both formats here; carried so a left-handed format can say so.
    pub handed: Handed,
}
```

Rotations are always radians. A pack whose file stores degrees converts once, when it reads the
file into its store, or (if the store keeps degrees for an exact round trip) in its placement
hook (section 2), which is then the one place that multiplies.

### 1.3 Parent-relative, never world

A placement is carried **as the file holds it, relative to its parent**. The world placement is
the product along the `relative_to` chain, worked out by the host.

- **One move is one delta.** Moving a parent changes its own placement only; a hundred children
  that follow it change nothing in the store, and their frames carry nothing. Carrying world
  placements would turn one agent's nudge into a delta per descendant.
- **The store holds it so.** Both formats here store relative placements; world placements
  would be the kit's arithmetic, which section 1.1 keeps out.
- **The render hint covers the children.** When a placement changes, the kit adds to the
  frame's `containers` hint the container of every element whose chain passes through it, so a
  renderer redraws a child that sits in another container (an object in another collection) as
  well. The kit walks the chain from the index's `relative_to` links, which it holds; the walk
  is bounded by the index's size and stops at a cycle (a cycle is the pack's bug, refused
  loudly in test builds, and the elements on it get no placement).

### 1.4 Precision

- **`f64` throughout.** A file that stores `f32` loses nothing: every `f32` is an `f64`.
- **Printed short.** The pack declares the file's precision (`F32` or `F64`). For `F32` the
  kit writes each number as the shortest decimal that reads back as the same `f32` (`0.1`, not
  `0.10000000149011612`); a reader that wants the stored `f32` exactly rounds back to `f32`
  (`Math.fround`), and gets it. For `F64`, the shortest decimal of the `f64`, as the kit's JSON
  writes every float today.
- **Finite only.** A placement with a NaN or an infinity in it is no placement (`None`), as a
  2D geometry that doesn't parse is no box. Both packs here already decline such a file.
- **Equality is exact.** The kit compares placements by their numbers, bit for bit (as its
  JSON floats compare), so `-0` and `0` differ: a change from one to the other is a move. This
  can't jitter, since nothing is computed.

### 1.5 Limits

- At most **4 steps** a placement (`PLACEMENT_STEPS_MAX`); a pack that gives more has its
  placement dropped and a test build fails, naming the element.
- A placement's JSON is at most about 450 bytes (four matrix steps); it counts in a units
  page's byte budget as every field does, so a page of 3D units holds fewer units, never a
  larger page. The snapshot head's containers grow by one placement each.
- The chain walk for the render hint visits each element once per change, at most.

---

## 2. How a pack declares it

```rust
pub struct SpaceSlot {
    /// The element fields a placement is read from (`xform`, `rotmode`, `quat`; `place`).
    /// A change to any of them asks the pack for the element's placement again; every other
    /// field leaves it as it was.
    pub unit_fields: &'static [&'static str],
    /// The same for containers (`&[]`: containers have no placement).
    pub container_fields: &'static [&'static str],
    /// The doc's space: fixed, or read from a map root (a building model's project units).
    pub space: SpaceSrc,
    pub precision: Precision,
}

pub enum SpaceSrc {
    Fixed { length_unit: &'static str, metres_per_unit: Option<f64>, up: Axis, handed: Handed },
    /// The pack reads it (its hook below), from the doc's map roots named here; a change to
    /// any of them asks again.
    Pack { roots: &'static [&'static str] },
}

// in ViewSchema, defaulting to None through ViewSchema::NONE:
pub space: Option<SpaceSlot>,
```

And two hooks on `Pack`, both defaulting to `None`:

```rust
/// Element `key` of list root `root` (a unit or a container), placed: read from its own
/// fields in `source` (the keyed state, and the carrier when the pack keeps one). Pure and
/// bounded; called once per element on a first build, then once per element whose
/// placement fields a change touched.
fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement>;

/// The doc's space, for `SpaceSrc::Pack`.
fn view_space(&self, source: ViewSource<'_>) -> Option<Space>;
```

**The rule that makes it cheap: a placement reads only its own element's fields** (and the
carrier). That is what lets the kit ask again for touched elements only, as the incremental
view index does for every other field. A test build checks it: after a change, the placements
the kit kept equal those of a full build, and a difference is a loud failure naming the
element. A pack whose placement needs another element's fields (a placement relative to an
anonymous frame the store doesn't hold) reads that frame from the carrier, where it doesn't
change without a new carrier (which rebuilds the index anyway).

**Why a hook and not parts the kit parses.** The 2D slot is parsed by the kit (and the client)
from one field with declared parts. That doesn't fit 3D: a scene object's rotation is Euler in
an order its own field names, or a quaternion in another field, or an axis and angle; a
building model's numbers are the file's own real tokens (`1.E-3`), with an optional direction
left empty. A grammar wide enough for both would be a parser in two languages that every
future format stretches. The pack already parses its fields (its verbs do); it hands the kit
typed numbers once, and the kit and the client never parse a placement again.

---

## 3. How it rides

All additive: nothing below appears for a pack that declares no slot, so every 2D pack's
snapshot, pages and frames stay byte for byte as they are.

| Where | What | Only when |
|---|---|---|
| `schema.space` (snapshot head) | `{unit_fields, container_fields, precision}` | the slot is declared |
| `view.space` (snapshot head, beside `page`) | the doc's `Space` | the slot is declared |
| each `Container` in the head | `placement` (or `null`) | the slot has container fields |
| each `Unit` on a units page | `placement` (or `null`) | the slot is declared |
| `ops_change` unit delta | op word `place`; `placement_after` (the new placement, or `null` when it has none now) | its placement changed |
| `ops_change` container delta | the same | its placement changed |
| `ops_change` `view_space` | the new `Space` | it changed |
| `ops_change` `containers` (the render hint) | also the containers of every element placed through a moved one | a placement changed |

**The wire's form** of a placement:

```json
{"relative_to": "@obj:3",
 "steps": [{"trs": {"loc": [1, 0, 2.5], "rot": {"euler": "XYZ", "angles": [0, 0, 1.5707964]},
                    "scale": [1, 1, 1]}}]}
{"relative_to": "@storey:12",
 "steps": [{"axes": {"origin": [4000, 0, 0], "z": null, "x": [0, 1, 0]}}]}
{"relative_to": null, "steps": [{"matrix": [1,0,0,0, 0,1,0,0, 0,0,1,0, 5,0,0,1]}]}
```

A quaternion is `{"quat": [w, x, y, z]}`, an axis and angle `{"axis": [x, y, z], "angle": a}`.

**`placement_after` is the whole placement**, never a diff of it: nine numbers are smaller than
any patch format, and a watcher that missed nothing applies it as it is.

**Render hashes are unchanged.** A container's render hash stays the pack's: a pack whose
renderer draws from placements folds them into its digest (from the live store, as it folds
every other field); the kit's own fallback (the containers whose hash changed) is unchanged.
The render hint above is how a renderer learns to look; the hash is how it learns to redraw.

**The client** (`client/src`): `ViewUnit.placement` and `ViewContainer.placement` (absent from
an older kit), kept by `KeyedView` from each frame's `placement_after`; `KeyedWatch.space`; pure
helpers `placementMatrix(p)` (the steps' product, column-major `f64`) and
`worldMatrix(view, address)` (the product along `relative_to`, stopping at a cycle). No pack
code, no field parsing.

**Older peers.** A v0.17.0 client ignores every key above (it reads the fields it knows), and
a delta's unknown op word `place` changes nothing in its view. A newer client against an older
kit sees no `space` and no `placement`. The wire's version doesn't move. Nothing is stored: the
view index is memory only.

---

## 4. What each format needs

### A scene file (BLEND)

- **Units** are objects; containers are collections, with no placement of their own
  (`container_fields: &[]`).
- **The placement** is one `Trs` step from the object's `xform` (location, Euler angles,
  scale), with the rotation from `rotmode`: an Euler order word, or `quat` (`w,x,y,z`) when it
  rotates by quaternion, or axis and angle. `relative_to` is the parent object's address (the
  `parent` field), `None` without one. **The parent inverse matrix** the file keeps for a
  parented object (the parent's transform at parenting time) is a second step: a `Matrix`
  step the pack reads from the carrier, before the `Trs` (the file's composition is
  parent, then parent inverse, then the object's own). Until the pack reads it, a parented
  object's placement is approximate, and the pack says so the way the 2D slot says it
  (`approx_box`, reused for 3D: "the placement is approximate").
- **Precision** `F32`. **Space** fixed: `scene-unit`, `metres_per_unit` from the scene's unit
  scale when the pack reads it (the scene map root, so `SpaceSrc::Pack`), up `Z`, right-handed.
- **Placement fields**: `xform`, `rotmode`, `quat`, `parent`.
- **What it gets**: the transform as data, not as the unit's text; the unit's text is free
  again (or a per-unit derived value names the object's data, `mesh @me:Cube`); an agent's
  `object place` arrives as one delta with `place` and nine numbers.

### A building model (IFC)

- **Units** are elements; containers are spatial elements (site, building, storey, space),
  **with placements of their own** (`container_fields: &["place"]`): an element is placed
  relative to its storey, a storey to its building, and so on up.
- **The placement** is one `Axes` step from the element's `place` field (`<rel>|x,y,z|
  ax,ay,az|rx,ry,rz`): origin, the Z axis, the X reference direction, each optional direction
  `None` when the file leaves it out. `relative_to` is the address of the product whose
  placement it is relative to (a storey, a wall for its opening); `-` (none) is `None`.
- **A placement no product owns** (`~`: an intermediate frame) has no element to name. The pack
  reads that frame's numbers from the carrier and puts them in front as another `Axes` step,
  `relative_to` the first owned ancestor's address. Chains longer than the step limit are rare
  in exported models; one that is longer gets no placement and is reported by the pack's
  measurement, before the limit moves.
- **Precision** `F64` (the file's reals). The store keeps each number's own token for the
  round trip; the placement carries the parsed `f64`.
- **Space** from the project's length unit (`SpaceSrc::Pack`, reading the root where the pack
  keeps the project's units): `mm` and `0.001` for most models. Up `Z`, right-handed.
- **What it gets**: elements placed in storeys with no pack code; a wall moved with its
  openings is one delta (the openings are relative to it, so their placements don't change,
  and the render hint names their containers).

---

## 5. What stays the pack's job

- **Reading its own fields** into a `Placement`: its rotation modes, its number tokens, its
  empty directions, any unit conversion it keeps out of the store.
- **Anonymous frames** in a chain, from its carrier.
- **Meshes, representations, materials, lights, cameras:** never in the kit. A renderer gets
  them from the pack's own host-only reads of the carrier; the view index names units and
  places them, it doesn't describe their shape.
- **Bounds.** A box around a unit in 3D needs its mesh; it comes with the pack's geometry read,
  not from this slot. A later slot can carry a box (min and max corners in the unit's own
  frame) beside the placement, by the same rules.
- **Render hashes**, as today.
- **Edits.** The slot is read-only. Moving something is the pack's verb (`object place`,
  `element move`), which writes the pack's fields; the kit sees the field change and asks for
  the new placement.

## 6. What the kit does

- Declares the types, the slot, the two hooks, their defaults.
- Builds placements with the rest of the index (first build: once per element), keeps them
  through changes (once per touched element), and drops the doc's space and every placement
  with the index (a reload, a purge, a new carrier).
- Compares before and after for the frame's `place` deltas; walks the chains for the render
  hint.
- Prints numbers at the declared precision.
- Checks in test builds that the kept placements equal a full build's, and that no chain has
  a cycle.
- The client's types, its keeping of `placement_after`, and the two matrix helpers.

## 7. Proof, when built

- **Rows:** a placement of each form round-trips the wire (Rust to JSON to the client and
  back, numbers bit for bit at each precision); a one-field change on one unit carries `place`
  and its new placement and nothing for its children; a parent's move hints its children's
  containers; a change to a non-placement field asks the hook for nothing (counted); the kept
  placements equal a full build's after a random run of changes; a 2D pack's snapshot and
  frames are byte for byte as before; an older client reads a 3D snapshot and frames.
- **Plants:** the kit drops `place` when only a rotation changed; it walks no chain for the
  hint; it prints an `f32` as its `f64` digits; it asks the hook again for every unit on every
  change (caught by the count).

## 8. Open questions

1. **Containers' placements for a scene file.** A collection has no transform in the file;
   should the slot still let a pack give one (an instance offset)? Proposed: no, until a format
   needs it.
2. **One slot or two.** Units and containers share one slot here, each with its own field
   list. A format whose containers are frames of their own (a building's storeys) needs both;
   none needs different forms for the two.
3. **The approximate flag.** Reusing `approx_box` for "the placement is approximate" keeps the
   unit's shape; a separate `approx_placement` is clearer. Proposed: a separate flag, emitted
   only when the slot is declared.
4. **Native IDs across tiers.** A scene's objects are `@obj:N` live and `@ob:Cube` on the blob
   tier. Placements on the blob tier (from a version's structure index) would need the
   structure index to carry them; out of scope here, the live tier first.
