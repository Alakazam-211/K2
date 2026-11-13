# A unit's placement in 3D (design, r1)

A proposal, not built. How a pack declares, and the kit carries, where a unit sits in a 3D
scene, so a host can draw a live 3D doc (a scene's objects, a building model's elements) from
the view index with no pack code, as it draws a deck from the 2D geometry slot today.

r1 replaces r0 (`view-3d-slot-r0.md`) after a design review. The direction is unchanged: the
file's own form, parent-relative, a pack hook, an additive wire. What r1 adds is the meaning of
every form and chain as formulas, one set of test vectors for a Rust and a JS helper, what a
broken chain means, a placement on every unit that comes into view, a change held only in the
carrier, and the cases a scene file and a building model need from the first version. Section 9
lists the changes from r0.

**Marks.** HYPOTHESIS: a claim about how a format holds placements that hasn't been checked
against a file; each names its referee, the check that settles it. Everything else is this
design's own rule or was read where it says.

**The problem.** The view schema's geometry slot (`ViewSchema::geometry`) is a 2D box in page
space: integer `x,y,w,h`, a rotation about the box centre, two flips. A 3D placement is a
location, a rotation and a scale in three axes, as decimals, often relative to another element.
A pack that has one today carries it as an ordinary field of the keyed state (a scene pack's
`xform`) and at best writes its numbers as the unit's text or a derived value. A watcher can't
place anything without the pack's own parser, and a change frame can't say "this moved" in a
form a renderer reads.

---

## 1. What the kit carries

### 1.1 A placement, typed

```rust
/// Where a unit (or a container) sits: relative to another element, or to the doc's space,
/// as a list of steps.
pub struct Placement {
    /// The address of the element this placement is relative to (a parent object, the storey
    /// an element stands in, the wall an opening is cut into): a unit or a container of the
    /// same doc. `None`: **the doc's space**. A unit's container plays no part in its
    /// placement unless `relative_to` names it.
    pub relative_to: Option<String>,
    /// At least one; at most `PLACEMENT_STEPS_MAX` (4) from the kit (the client sets no limit).
    pub steps: Vec<Step>,
    /// `Some(reason)` when the file can't say exactly where the unit is (section 1.6).
    pub approx: Option<&'static str>,
}

pub enum Step {
    Trs { loc: [f64; 3], rot: Rotation, scale: [f64; 3] },
    Axes { origin: [f64; 3], z: Option<[f64; 3]>, x: Option<[f64; 3]> },
    /// A 4x4 affine matrix, column-major.
    Matrix([f64; 16]),
}

pub enum Rotation {
    Euler { order: EulerOrder, angles: [f64; 3] },
    Quat([f64; 4]),
    AxisAngle { axis: [f64; 3], angle: f64 },
}
```

**Why the file's own form and not a matrix.** Each pack hands the kit the numbers its file
stores, in the shape it stores them. Nothing is computed on the way:

- **Exact.** The numbers in the index are the stored numbers. Two builds of the index agree to
  the bit on every machine, so the kit compares placements for equality (a change frame says
  "moved" only when a number moved), and nothing in the kit's comparisons depends on a
  platform's trigonometry.
- **No loss.** An Euler rotation past a half turn, a negative scale, a non-uniform scale under
  a rotation: a matrix loses which of several decompositions the file had, and the form a verb
  writes (`object place --rot`) is the form a watcher shows.

A host that draws needs a matrix: the kit and the client each give the same two helpers
(section 1.4), defined by the formulas below and tested on one shared set of vectors.

### 1.2 What each form means

All matrices act on column vectors (`p' = M p`), right-handed, angles in radians, positive
angles counter-clockwise looking down the axis towards the origin.

- **Basic rotations.** With `c = cos a`, `s = sin a`:
  `Rx(a) = [[1,0,0],[0,c,-s],[0,s,c]]`, `Ry(a) = [[c,0,s],[0,1,0],[-s,0,c]]`,
  `Rz(a) = [[c,-s,0],[s,c,0],[0,0,1]]`.
- **Euler, order word `ABC`** (one of `XYZ`, `XZY`, `YXZ`, `YZX`, `ZXY`, `ZYX`), `angles` =
  the angles about X, Y and Z, in that slot order whatever the word: the point is turned about
  `A` first, then `B`, then `C`, **each about the parent frame's fixed axes**:
  `R = R_C(angle_C) · R_B(angle_B) · R_A(angle_A)`. So `XYZ` is `Rz · Ry · Rx`. HYPOTHESIS:
  this is what a scene file's rotation mode words mean; referee: the cached world matrix
  (section 6).
- **Quaternion** `(w, x, y, z)`: the helper normalises it first (divides by its length). A
  quaternion of length 0, or one that isn't finite, is **no placement**. Then the usual
  rotation matrix of a unit quaternion: `[[1-2(y²+z²), 2(xy-wz), 2(xz+wy)], [2(xy+wz),
  1-2(x²+z²), 2(yz-wx)], [2(xz-wy), 2(yz+wx), 1-2(x²+y²)]]`. `q` and `-q` give the same matrix.
- **Axis and angle**: the axis is normalised first; an axis of length 0 is **no placement**
  (also with an angle of 0). Then Rodrigues' formula: `R = I + sin(t)·K + (1-cos(t))·K²`, `K`
  the cross-product matrix of the unit axis.
- **`Trs`**: the point is scaled, then rotated, then translated: `M = T(loc) · R · S(scale)`.
  A scale of 0 on an axis is allowed (the matrix is singular; nothing is inverted).
- **`Axes`** (a building model's origin and two directions), as the IFC spec builds its axes
  (read: the spec's `IfcBuildAxes` and `IfcFirstProjAxis`, IFC 4.3 documentation source; the
  latter says "with arg defaulted the result is the projection of (1.0,0.0,0.0) onto this
  plane except that if z-axis = (1.0,0.0,0.0) then (0.0,1.0,0.0) is used as initial value of
  arg", and "a violation occurs if arg is in the same direction as the input z-axis"):
  1. `Z` = `z` normalised; `None` is `(0, 0, 1)`. A given `z` of length 0: no placement.
  2. `V` = `x` as given; `None` is `(1, 0, 0)`, except `(0, 1, 0)` when `Z` is exactly
     `(1, 0, 0)`. A given `x` of length 0: no placement.
  3. `X` = `V − (V·Z) Z`, normalised. When its length before normalising is at most
     `1e-12 · |V|` (`x` along `Z`): no placement.
  4. `Y = Z × X`. `M` has columns `X`, `Y`, `Z` and the origin.
- **`Matrix`**: the 16 numbers column-major (`m[12..15]` the translation). Its last row must
  be exactly `(0, 0, 0, 1)`; otherwise no placement (the slot is affine only).
- **Steps**: `P = M(step 1) · M(step 2) · … · M(step n)`: the first step is outermost, nearest
  the parent.
- **The chain**: `W(u) = W(relative_to(u)) · P(u)`, and `W(u) = P(u)` when `relative_to` is
  `None`. `W` is in the doc's space (section 1.3).
- **Unknown words.** A step kind, a rotation kind, an Euler order, an `up` or a `handed` word a
  reader doesn't know makes the placement **unknown**: no placement, never a skipped step. So a
  new step kind is additive.

### 1.3 The doc's space

One value per doc, on the snapshot's head beside `page`:

```rust
pub struct Space {
    /// The length unit's word (`m`, `mm`, `ft`, `scene-unit`) and how many metres one is, when
    /// the file says; `None` when it has no real-world scale, or several (a scene file whose
    /// scenes have different unit scales).
    pub length_unit: String,
    pub metres_per_unit: Option<f64>,
    /// Which axis is up in the doc's space: `Z` or `Y`.
    pub up: Axis,
    /// `Right` for both formats here; carried so a left-handed format can say so.
    pub handed: Handed,
}
```

The doc's space is where `relative_to: None` puts a placement. A renderer turns it into its own
(up axis, units) once, at the root. Rotations are always radians; no format here stores
degrees. When one does, `Space` gains an angle unit word (additive), rather than a conversion
the kit would have to compute.

### 1.4 Two helpers, one set of vectors

- **Rust** (in the kit, for hosts and renderers written in Rust): `Placement::matrix() ->
  Option<[f64; 16]>` and `ViewIndex::world_matrix(address) -> Option<[f64; 16]>`,
  column-major. Its `sin` and `cos` are a pure-Rust implementation with a fixed algorithm
  (never the platform's), so the same placement gives the same bits on every host.
- **JS** (in the client): `placementMatrix(p)` and `worldMatrix(view, address)`, each
  `Float64Array(16) | null`. `Math.sin` differs between engines, so JS results match the
  vectors to a stated tolerance (1e-12 per entry, relative to the largest entry, plus 1e-12
  absolute), not to the bit.
- **The vectors**: `client/test/vectors/placement.json`, written by a kit test and read by both
  helpers' tests: every form, every Euler order, each degenerate case above (expecting none),
  `x` parallel to `z`, the `(1,0,0)` special case, a two-step and a four-step placement,
  chains of each length to 5, and each broken-chain case of section 3.2. The Rust helper
  matches them bit for bit on Linux and macOS.
- **`F32`**: for a slot of precision `F32`, both helpers round every number to `f32` before
  composing (the stored value), and the kit's test builds fail when a pack hands over a number
  in an `F32` slot that doesn't survive `as f32 as f64`.
- **Never compared.** A matrix from either helper is for drawing. The kit compares placements
  by their numbers (section 1.5), never by matrices.

### 1.5 Precision and equality

- **`f64` throughout.** A file that stores `f32` loses nothing: every `f32` is an `f64`.
- **Printed short.** For `F32` the kit writes each number as the shortest decimal that reads
  back as the same `f32` (`0.1`, not `0.10000000149011612`); a reader rounds back to `f32`
  (`Math.fround`) to get it exactly. For `F64`, the shortest decimal of the `f64`.
- **Finite only.** A placement with a NaN or an infinity in it is no placement (`None`).
- **Equality is exact**, number by number, bit for bit (as the kit's JSON floats compare): `-0`
  and `0` differ, so a change from one to the other is a move.

### 1.6 Approximate placements

`approx: Some(reason)`, inside the placement, so it travels with every `placement_after`:

- used only where **the file can't say** exactly where the unit is. The one reason word so far
  is `driven` (the app recomputes the transform from animation or constraints; the stored
  numbers are its base);
- never for something the pack hasn't read yet: a pack that can't give an exact placement it
  could read gives none (section 4);
- a reader that meets an `approx` word it doesn't know treats the placement as approximate.

### 1.7 Limits

- At most **4 steps** a placement from the kit (`PLACEMENT_STEPS_MAX`); a pack that gives more
  has its placement dropped, and a test build fails naming the element. The limit is the
  kit's only: the client and the helpers take any number, so raising it changes no wire. It is
  fixed only after the building model's anonymous chains are measured (section 4).
- A placement's JSON is at most about 450 bytes (four matrix steps); it counts in a units
  page's budget as every field does.
- The chain walk for the render hint visits each element once per change, at most.

---

## 2. How a pack declares it

```rust
pub struct SpaceSlot {
    /// The element fields a unit's placement is read from (a scene object's `xform`,
    /// `rotmode`, `quat`, `parent`; a building element's `place`).
    pub unit_fields: &'static [&'static str],
    /// The same for containers (`&[]`: containers have no placement).
    pub container_fields: &'static [&'static str],
    pub space: SpaceSrc,
    pub precision: Precision,
}

pub enum SpaceSrc {
    Fixed { length_unit: &'static str, metres_per_unit: Option<f64>, up: Axis, handed: Handed },
    /// The pack reads it (`Pack::view_space`) from the map roots named here; a change to any
    /// of them asks again.
    Pack { roots: &'static [&'static str] },
}

// in ViewSchema, defaulting to None through ViewSchema::NONE:
pub space: Option<SpaceSlot>,
```

Two hooks on `Pack`, both defaulting to `None`:

```rust
/// Element `key` of list root `root` (a unit or a container), placed, from its own fields in
/// `source` (the keyed state, and the carrier when the pack keeps one). Pure and bounded.
fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement>;

/// The doc's space, for `SpaceSrc::Pack`.
fn view_space(&self, source: ViewSource<'_>) -> Option<Space>;
```

**The own-fields rule: a placement reads only its own element's fields** (and the carrier).

- **Its reason** is that it lets the kit **name the `place` deltas from a change's ops**: a
  change that writes one of `unit_fields` on an element is the only change that can move that
  element's placement in the store, so the kit asks the hook for that element and no other,
  and says `place` exactly when the numbers differ. A change to any other field asks nothing.
  This is the same way view deltas are worked out from ops today; it is not an optimisation
  that a diff of before and after could replace without losing which change moved what.
- The kept view index (kept through changes, asking the pack for the touched elements only)
  uses the same rule.
- Relative placements are what make it hold: moving a parent changes the parent's fields only.
  Anything a placement reads that isn't the element's own (an anonymous frame) comes from the
  carrier, which changes only with a new carrier (section 3.3).
- A test build checks it: after every change and every new carrier, the placements the kit
  kept equal a whole build's, or it fails naming the element.

**Why a hook and not parts the kit parses.** A scene object's rotation is Euler in an order its
own field names, or a quaternion in another field, or an axis and angle in the carrier; a
building model's numbers are the file's own real tokens, with optional directions left out. A
grammar wide enough for both would be a parser in two languages that every later format
stretches. The pack already parses its fields (its verbs do); it hands the kit typed numbers
once, and the kit and the client never parse a placement.

---

## 3. How it rides

All additive: nothing below appears for a pack that declares no slot, so every 2D pack's
snapshot, pages and frames stay byte for byte as they are.

### 3.1 On the wire

| Where | What | When |
|---|---|---|
| `schema.space` (snapshot head) | `{unit_fields, container_fields, precision}` | the slot is declared |
| `view.space` (snapshot head, beside `page`) | the doc's `Space` | the slot is declared |
| each `Container` in the head | `placement` (or `null`) | the slot has container fields |
| each `Unit` on a units page | `placement` (or `null`) | the slot is declared |
| `ops_change` unit delta | op word `place` and `placement_after` | its placement changed |
| `ops_change` unit delta, `add` or `restore` | `placement_after`, always (the placement, or `null`) | the slot is declared |
| `ops_change` container delta | `placement_after` on `place`, `add` and `restore`, as for units | the slot has container fields |
| `ops_change` `view_space` | the new `Space` | it changed |
| `ops_change` `containers` (the render hint) | also the containers of every element placed through a moved one | a placement changed |

A unit that comes into view (`add`, `restore`) carries its placement, so a client that builds
it from the keyed state (with no pack code) has one at once.

**The form** of a placement:

```json
{"relative_to": "@obj:3",
 "steps": [{"trs": {"loc": [1, 0, 2.5], "rot": {"euler": "XYZ", "angles": [0, 0, 1.5707964]},
                    "scale": [1, 1, 1]}}]}
{"relative_to": "@storey:12", "approx": null,
 "steps": [{"axes": {"origin": [4000, 0, 0], "z": null, "x": [0, 1, 0]}}]}
{"relative_to": null, "steps": [{"matrix": [1,0,0,0, 0,1,0,0, 0,0,1,0, 5,0,0,1]}]}
{"relative_to": "@obj:7", "approx": "driven", "steps": [...]}
```

A quaternion is `{"quat": [w, x, y, z]}`, an axis and angle `{"axis": [x, y, z], "angle": a}`.
`approx` is absent or `null` for an exact placement.

- **`null` has one meaning**: not placed by this slot; don't draw it from here. Never "at its
  parent's origin" and never "at the origin".
- **`placement_after` is the whole placement**, never a diff.
- **`relative_to` may name a unit on a later page, or a container.** A client resolves chains
  after the snapshot's last page.

### 3.2 A broken chain

A unit's chain is broken when it passes through:

- an element with no placement (`null`), or an unknown one (section 1.2);
- an address the view doesn't show (a deleted element, an element in no shown container, an
  address nobody has);
- a cycle;
- a degenerate form (section 1.2).

Then the unit has **no world matrix**, and neither has anything placed through it. Both
helpers return none (`None`, `null`); **a missing link is never identity**, which would draw
the thing at a wrong place, worse than not drawing it. The kit's test builds fail on a cycle,
naming its elements (the pack's bug); a host still gets none, never a loop.

### 3.3 A change held only in the carrier

An outside save can move what the carrier alone holds (a building model's anonymous frame; a
scene file's parent inverse set by the app) and change no field of the store: no ops, so no
change frame. The kit's kept index is keyed by the carrier's id and is built whole for a new
carrier, so the kit sees the new placements; a watcher must too:

- when a new carrier is taken in and any unit's or container's placement differs from the kept
  index's before it, every keyed watch on the doc is told to take a fresh snapshot (its
  `needs_snapshot`, as when a frame would pass the cap). No new frame kind, so older clients
  behave as they already do;
- a carrier that changes no placement sends nothing;
- an additive patch frame (only the placements that moved) can replace the snapshot later, if
  a measurement says snapshots after outside saves cost too much.

### 3.4 Render hashes

A container's render hash stays the pack's. The pack states **which frame a container's
picture is drawn in**. If in the doc's space (a building's storey drawn where it stands), its
hash folds the placement of every ancestor of every element the picture draws, so a building's
move changes the hash of every storey whose elements it moves. The render hint (section 3.1)
says where to look; the hash says whether to redraw.

### 3.5 The client

`ViewUnit.placement` and `ViewContainer.placement` (absent from an older kit), kept by
`KeyedView` from each frame's `placement_after` (on `place`, `add` and `restore`);
`KeyedWatch.space`; `placementMatrix` and `worldMatrix` (section 1.4). No pack code, no field
parsing, no step limit. A v0.17.0 client ignores every key above, and a delta's unknown op word
`place` changes nothing in its view. The wire's version doesn't move. Nothing is stored: the
view index is memory only.

---

## 4. What each format needs

### A scene file (BLEND)

From the first version, with no approximate placement where the file has an exact one:

- **Units** are objects; containers are collections, with no placement
  (`container_fields: &[]`).
- **The placement** of an object with no parent: one `Trs` step from `xform` (location, Euler
  angles, scale), the rotation from `rotmode` (an Euler order word), or `quat` (`w,x,y,z`), or
  an axis and angle the hook reads from the carrier (the store has no field for it).
  `relative_to: None`.
- **A parented object**: `relative_to` is the parent object's address, and the steps are a
  `Matrix` step holding the parent inverse matrix the file keeps for the object, then the
  `Trs` step. HYPOTHESIS: the file holds one such 4x4 `f32` matrix per object (a fixed-size
  field the hook reads from the carrier), and the world matrix is `parent world · parent
  inverse · own transform`; referee: the cached world matrix (section 6).
- **A parent that isn't an object** (a bone, vertices of a mesh): no placement; the world
  matrix depends on a pose or a mesh the slot doesn't hold. HYPOTHESIS: the file marks the
  parent's kind in a field of the object; the hook reads it from the carrier. Referee: the
  public corpus's objects of each kind, against the cached world matrix.
- **Delta transforms**: HYPOTHESIS: the file holds a second location, rotation and scale
  applied on top of the first. When any is not identity, no placement until the corpus is
  counted and a decision taken; when all are identity, they're left out. Referee: the cached
  world matrix, which a placement without them must match only where they are identity.
- **Driven transforms** (the store's `edit` = `a`): the placement with `approx: "driven"`.
- **Precision** `F32`. **Space** from the scene's unit scale (`SpaceSrc::Pack`); with several
  scenes of different scales, `metres_per_unit: None`. Up `Z`, right-handed.
- **Placement fields**: `xform`, `rotmode`, `quat`, `parent`.
- At most 2 steps.
- **Not said by the slot**: an object in several collections draws in each (the hint names its
  first collection only); a collection instance draws objects many times. Both stay with the
  pack's scene read.

### A building model (IFC)

- **Units** are elements; containers are spatial elements (site, building, storey, space), with
  placements of their own (`container_fields: &["place"]`).
- **Every placed element is shown.** The pack keeps a **view container field** for each element:
  the nearest spatial container through containment, else through the element it hangs from
  (an opening's wall) or the whole it is part of (a stair's flight, a curtain wall's plate),
  and names it as the schema's `unit_container_field`. A verb that moves a whole to another
  container updates its parts' field in the same change, and decode checks it. Without it, an
  opening and a part would have no unit, and a door placed through its opening would have a
  broken chain.
- **The placement** is one `Axes` step from the element's `place` field: origin, the Z axis and
  the X reference direction, each optional direction `None` when the file leaves it out.
  `relative_to` is the address of the product whose placement it is relative to (a storey, a
  wall for its opening), or `None` for a placement with no parent: **the doc's space**, not the
  storey's frame, even though the element stands in the storey. HYPOTHESIS: some exporters
  write every placement this way; referee: the IfcOpenShell referee (section 6).
- **A placement no product owns** (an anonymous frame), **or one owned by a class the store
  doesn't hold** (a port): the hook reads its numbers from the carrier and puts it in front as
  another `Axes` step; `relative_to` is the first owned ancestor's address, or `None`.
- **A placement owned by two products**: `relative_to` names the owner with the lowest key, and
  both owners' `place` is read-only (`edit = 0`); the pack's plan says how a move treats it.
- **A parent with no placement the slot can give** (a grid or a linear placement): the parent
  has `null`, so the chain of every child is broken (section 3.2).
- **Short coordinates**: a 2D placement or a point with fewer than three numbers is padded with
  zeros; that is reading, not computing.
- **Precision** `F64` (the file's reals). The store keeps each number's own token for the
  round trip; the placement carries the parsed `f64`.
- **Space** from the project's length unit (`SpaceSrc::Pack`): `mm` and `0.001` for most
  models. A unit defined by conversion from a prefixed one takes one IEEE multiply, which is
  deterministic. Up `Z`, right-handed.
- **The mesh's frame**: the pack's mesh read gives each unit's mesh in the frame its placement
  ends in, with the representation's own transforms applied. HYPOTHESIS: the geometric
  context's world coordinate system is identity in nearly every file; referee: count it over
  the corpus. If not, `Space` takes an optional root placement (additive).
- **Anonymous chains** are measured over the corpus (their length distribution) before the
  step limit of 4 is fixed.

## 5. What stays the pack's job

- Reading its own fields into a `Placement`: rotation modes, number tokens, empty directions.
- Anonymous frames and parent inverses, from its carrier.
- Meshes, representations, materials, lights, cameras: never in the kit. The view index names
  units and places them; it doesn't describe their shape.
- Bounds: a box in 3D needs the mesh, so it comes with the pack's mesh read, not from this slot
  (a later slot can carry one, additively).
- Render hashes, and the frame its pictures are drawn in (section 3.4).
- Edits: the slot is read-only. Moving something is the pack's verb, which writes the pack's
  fields; the kit sees the field change and asks for the new placement.

## 6. Proof, when built

**Referees that aren't us** (test-only checkers on build nodes, never shipped):

- **BLEND**: the world matrix the app caches in each saved object. Over the public corpus, the
  composed chain of every object nothing drives, whose parent is an object or none and whose
  deltas are identity, equals the cached matrix within `f32` rounding. This settles the Euler
  convention, the parent inverse's place and the scale order with no app run and no source
  read. HYPOTHESIS: the cached matrix is current in files the app saved.
- **IFC**: the IfcOpenShell referee on nodes (already in the IFC plan): each element's world
  placement by GlobalId equals the helper's `worldMatrix`, within `1e-9` of the model's size.

**Rows:**

- each form, each Euler order and each degenerate case against the shared vectors, in Rust and
  in the client; the Rust helper bit for bit on Linux and macOS;
- a placement of each form round-trips the wire, numbers bit for bit at each precision;
- an `F32` slot refuses a number that isn't an `f32` (test build);
- a one-field change on one unit carries `place` and its new placement, and nothing for its
  children; a change to a field outside `unit_fields` asks the hook for nothing (counted);
- an added unit and a restored unit arrive with their placements;
- a parent's move hints its children's containers, and changes the render hash of every
  container holding something placed through it;
- a door through an opening through a wall resolves to a world matrix;
- a chain through an element with no placement, a deleted element, an element out of view, a
  cycle and an unknown step kind each gives none, not identity;
- an absolutely placed element in a storey whose placement isn't identity lands in the doc's
  space (plant: `None` read as the container's frame);
- an outside save that changes only a carrier-held frame makes the watch take a snapshot, and
  the watcher's placement after it is the new one;
- the kept placements equal a whole build's after a random run of changes and of new carriers;
- a 2D pack's snapshot and frames are byte for byte as before; a v0.17.0 client reads a 3D
  snapshot and frames.

**Plants:** `place` dropped when only a rotation changed; no chain walk for the hint; an `f32`
printed as its `f64` digits; every unit asked again on every change (caught by the count);
`None` read as the container's frame; a missing link taken as identity; `placement_after` left
off an `add`.

## 7. Questions r0 left open, answered

1. **Containers' placements for a scene file: no.** A collection's instance offset is not a
   placement of the collection; it belongs with instancing, later.
2. **One slot or two: one slot, two field lists.** The hook takes the root, so units and
   containers can differ in form without a second slot.
3. **The approximate mark: separate from the 2D slot's `approx_box`, inside the placement, with
   a reason word** (section 1.6), so it travels with `placement_after`.
4. **Native ids across tiers: out of scope.** Nothing here blocks it: `relative_to` is an
   address and each tier uses its own. A host doesn't keep a placement's addresses across a
   change of tier.

## 8. What would change the wire later, and what wouldn't

Settled here because a later fix would change meaning, not shape: `None` is the doc's space;
the formulas; unknown words make a placement unknown; no step limit in the client;
`placement_after` on `add` and `restore`; one meaning of `null`; `approx` inside the placement;
`relative_to` resolved after the last page.

Additive later: an angle unit; a root frame on `Space`; instances; a 3D box; a reason word for
a missing placement; new step kinds; a patch frame for carrier-only changes.

## 9. Changes from r0

- `relative_to: None` is the doc's space (r0 called it the container's frame in one place and
  the chain's root in another).
- Every form's meaning as a formula; the Euler convention; normalising quaternions and axes;
  how `Axes` are built, with IFC's default X rule from its spec text; the degenerate cases.
- A Rust helper beside the JS one, one shared set of vectors, two outside referees.
- What a broken chain means; a view container field for building elements so every placed
  element is shown.
- `placement_after` on every unit or container that comes into view; a carrier-only change
  makes watches take a snapshot.
- A scene file's parent inverse from the first version, with no approximate mode; parents that
  aren't objects, delta transforms and driven transforms.
- `approx` with a reason word, inside the placement.
- The own-fields rule's real reason: naming `place` deltas from ops.
- Render hashes: the pack states the frame its pictures are drawn in.
- The "small" argument for the file's own form is dropped; exactness and no loss carry it.
