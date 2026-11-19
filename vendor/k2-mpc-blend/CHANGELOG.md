# Changelog

## v0.3.1: pairs with kit v0.23.0

**Requires `k2-mpc-base` v0.23.0**. BLEND files behave as in v0.3.0.

- Pairs with kit v0.23.0. It changes no pack API but one: a blob pack's `merge3_known` is also asked
  for a revert (`SaveBase::Revert`, base the version taken away, ours head, theirs the version
  before it; the default calls `merge3_bytes`). Its other changes are the host's (undo, redo and
  revert on a blob doc, a batch of verbs, a blob verb that plans with no lease, holds that bind an
  agent's whole-file write, the store's cache at 64 MiB, grouped commits as an opt-in) and the
  native engine's (off).
- On a BLEND file, `undo`, `redo` and `revert` are refused with `MPC-E730` `revert_unsupported`
  (this pack has no merge); the next step is `restore`.
- `Cargo.lock` and `fuzz/Cargo.lock`: k2-mpc-base 0.23.0 and the crate version 0.3.1; `README.md`'s
  kit line names v0.23.0.

## v0.3.0: pairs with kit v0.22.0

**Requires `k2-mpc-base` v0.22.0** (its edit check, `Pack::check_edit`). The crate version is
`0.3.0`; both locks name k2-mpc-base 0.22.0 and its dependencies (the allocator call's `libc`), so
`--locked` builds beside v0.22.0, not v0.21.0.
The verbs' result goldens (`tests/results_golden/`) hold the fields that kit adds to a call's
result (`view`, `dry_run`, `changed.bytes`), and the scene turn's golden
(`tests/watch_turn_golden/`) the roster's `view_address` event an agent's placement makes
there; the pack's own data in them is unchanged.

### Behaviour change: a live change the live rules refuse is refused at the call

`BlendLive` opts in to the kit's edit check (`Pack::checks_edits`, `Pack::check_edit`): every
live change, whoever makes it (a person's or a host's edit, an agent's verb, undo, redo, revert,
restore), is judged by the live rules (`live::check_edit`, the write-back's own) before it
applies. A change they refuse is refused at the call: the kit's `pack_refused` (`MPC-E823`,
exit 2), its `details.code` the rule's registered code (`blend_driven`, `blend_rotation_mode`,
`blend_not_editable`, `blend_name`, `live_read_only`) and `details.reason` why, the object and,
for a driven change, the channel named (`(channel xform)`, or the animated bits). Nothing
applies, the version doesn't move, the file stays as it was, and every read of the doc works.
A rule with no code of its own (an element of the lists added, removed, moved or retyped; a
collection's field; an object field live v1 doesn't write; a value its field can't hold; a
name without its `nid`) gives the new entry `blend_live_change` (`BLEND-E025`, exit 2, its next
step `scene list`). `live::EditRefusal::pack_code()` and `pack_refusal()` (new) give the code
and the refusal as the kit takes it.

v0.2.0's known limit is closed: a person's direct edit of a driven channel (or any change the
write-back can't write) is refused before it applies, so the doc and the file never disagree.
A host may let people edit a live BLEND doc directly once it vendors that kit release and this
pack. An undo or a restore that would put such a change back (a rename into a name another
object took since; a restore past an app's save that moved an animated channel or added an
object) is refused the same way.

Rows (`tests/edit_hook.rs`): each rule refused at the call by a person's direct edit, nothing
moving and every read Ok (a constrained transform, a quaternion rotation, a linked object, a
list insert, a collection's field, another field, an animated visibility bit); an undo and two
restores refused the same way; a change the rules take (a placement and a driven object's
rename) committed and written back byte for byte as the pack's hook writes it, and its undo
too. Changed rows: `row_person_edit_of_driven_channel` (`tests/newer.rs`) expects the refusal
at the call (`MPC-E823`, `blend_driven`, the channel named) with nothing applied and the reads
Ok; `row_live_read_only` (the gap rows) expects a person's edit of a read-only doc refused at
the call with `live_read_only`; the view rows whose change the pack refuses at the call
(`row_view_hidden_by`, `row_move_keeps_placements`, `row_collection_move_carries_parent_after`)
open the scene through a test pack with the edit check off (`live_support::unchecked`), their
subject being the view's derive of that change. The planted bug, the kit never asked
(`checks_edits` false), is caught alone by the person's row.

### Tests and docs: the cached fields a write leaves as they are

The writer leaves three cached fields as they are, which the program refreshes on load, as its
reopening of our writes shows: an object's `constinv` (its constraint stack's stored inverse)
after a location written under constraints, and its `base_flag` and its view layer `Base`'s
`flag` after a hide (the writer's docs and `PROVENANCE.md` say so; no writer change). Rows
(`tests/cached_flags.rs`): a viewport hide written by the writer, on the generated living room
and on the program's own save of it at 5.3.0, leaves each view layer base's flags (and, in the
program's save, the object's `base_flag`) as stored, and reads back hidden through our reader
(`restrictflag`), the mesh read (`hidden` self) and the live view (`hidden_by` self).

### Tests and tooling: our animated and constrained edits, reopened by the program

The program reopened our writes of its own saves of a constrained scene and an
animated-visibility scene at 5.2.4 and 5.3.0. Values written held; a written location under
constraints held, with its placement following the constraints; a cleared animated visibility
bit was restored by its animation (the edit the writer refuses).

A private round-trip row, `the_program_reopens_our_animated_and_constrained_edits`
(`tests/roundtrip.rs`), compares our edits with the program's report after reopening; it runs
where the private outputs are (the gate's round-trip part, `PROVENANCE.md`). The edits: the
writer's and the live tier's, of the program's own builds of a scene with an animated
visibility bit and of one with a constrained object, at 5.2.4 and 5.3.0. Per release and file
it compares: each edit with the program's report; an animated viewport bit, cleared in our
input by a raw byte patch, with the report, expected to hold its keyed value; every other value
with the program's report of its build before; the constrained object's evaluated world with
its track-to and child-of stack composed in the row, and its `world_eval` with its location's
translation; and our reader's read of the program's re-save with each edit.
`scripts/roundtrip.sh` takes its four folders (`BLEND_REOPENED_DIR`, `BLEND_REOPENED_IN`,
`BLEND_REOPENED_ANIMATED_BEFORE`, `BLEND_REOPENED_CONSTRAINED_BEFORE`) and refuses each missing;
`scripts/test-roundtrip.sh` plants each absence. The test support reads one file's lines of a
report (`builds_support::seen_of`).

## v0.2.0: pairs with kit v0.21.0

### Breaking

What a host built against v0.1.0 must check (the sections below give each in full):

- **API changes** (what `cargo semver-checks` finds against v0.1.0, and what it can't):
  - `live::errors::file_version_message(format, written)` takes two parameters; `written` says
    whether the refusal is a write's (the releases written) or a read's (the releases read).
  - `verbs::run` returns `verbs::Ran { write, warnings }` in place of the kit's write result;
    `Verb::typed` takes the warnings; each edit verb's data holds `warnings` (an added field,
    the shapes' version unchanged).
  - `live::model::LiveObj` gains a public field, `vis_animated` (the derived `visanim`): a
    literal that names every field doesn't compile.
  - Under the test-only `testgen` feature: `testgen::ObjectSpec` gains `slot`, and the
    `Driven` enum gains variants (`Dangling`, `Constraints`) and `Driven::Driver` takes the
    channel it drives.
  - New: `write::WriteWarning` (`blend_constrained`), `Written::warnings`,
    `live::check_edit` (one pure check of a live edit), `live::errors::written_releases()` and
    `read_only_message()`, `write::written_to()`.
- **The releases:** files of format versions 3.06 to 5.03 are read AND written
  (`checked_releases()` and `written_releases()` both name 3.06 to 5.03): 5.02 and 5.03 open
  live and editable, where they were declined (`live_version`). `live_read_only` (BLEND-E024)
  stays registered for a release read before it is written; no release reaches it today.
- **Behaviour a `^0.1` host relies on:**
  - the reader's default raw cap is 4 GiB (it was 64 GiB): a larger file read with the defaults
    is refused, typed;
  - after the host's `note_opened`, an app's re-save of the noted version merges into the live
    doc, where it kept both;
  - the writer refuses a change to a channel an action, an NLA strip or a driver animates, and a
    flip of an animated visibility bit (`blend_driven`, the channel named), where it wrote them;
    a transform under constraints is written with a `blend_constrained` warning;
  - the live tier's rules are one check for its verbs and its write-back: a live edit (a
    person's or an agent's) of a driven or constrained object is refused per object, so a
    person's direct edit of such an object, written before, is refused at write-back; the
    write-back's driven refusal reads `blend_driven: ...`. File-level edits keep the
    per-channel rule with the warning;
  - the live store gains a derived `visanim` meta field (an object's animated visibility bits):
    a host's outliner sees one more meta column;
  - a link placeholder takes its type code's kind (a linked object is an `object`, linked) and
    `scene list` lists it.
- **Known limit:** a person's direct edit that the write-back refuses is taken by the session,
  and every read of the doc then fails until the edit is undone; a host must not let a person
  write a BLEND doc directly until the kit can refuse such an edit up front (it will call
  `live::check_edit`).

### The live rules, one check: `live::check_edit` (a behaviour change of the write-back)

`live::check_edit(view, edits)` (new, public) says whether a live edit is one the write-back
writes: `Ok`, or the first rule it breaks (`live::EditRefusal`: its `live::Rule`, the registered
code when the rule has one, the object and the channel, a message; `text()` is the code, then
the message). It is pure: it reads the doc's `View` before the edit and the kit's edit ops, and
nothing else, the input a kit hook that refuses a person's edit up front would have (the kit's
structured guards see the same), so it is ready for that hook, which the kit doesn't have yet.
The rules, in order:

- a doc read but not written back: any change, `live_read_only` (no release read is one);
- the lists: no collection or object added, removed, moved or retyped (any op on `colls` or
  `objs`); the collections: no field;
- each changed object, in the list's order: `edit` `0` (linked, or a field the writer can't
  find), any change, `blend_not_editable`; `edit` `a` (animation, a driver or a constraint
  drives the transform), any transform change, `blend_driven` (channel `xform`); `edit` `t` (it
  rotates by quaternion or axis and angle), a rotation change, `blend_rotation_mode`; an
  animated visibility bit (`visanim`), any visibility change, `blend_driven` (the bits named);
  a name the file can't hold (empty, a NUL, past the name field, another object's at that point
  of the list's order), `blend_name`; `nid` only with its name, set to the new name's file key;
  a value its field can't hold, or any other field, refused (no code).

The scene map isn't written back and isn't judged, nor a field of an element the list doesn't
hold; an edit of a root the live schema doesn't have is refused by `check_edit`.

The write-back (`splice::encode_from_paired`) judges by the same rules, so a host's check and
the write-back never disagree; its refusal's text starts with the rule's code when it has one
(`blend_driven: ...`; `live_read_only: ...` as it reads). The writer's own driven refusal,
reaching the kit through the write-back, reads `blend_driven: location[2] is driven (...)`;
the writer's other refusals read `the writer refused: ...` (each has a registered code as a file
verb's refusal: `blend_version`, `blend_not_found`, `blend_ambiguous`, `blend_not_an_object`,
`blend_not_editable`, `blend_rotation_mode`, `blend_name`, `blend_value`, `blend_write_check`,
and a reader's; the live rules refuse the changes that reach them first).

Behaviour change, live edits only: a live edit (a person's or an agent's) of an object whose
transform is driven or under constraints is refused per object, by one rule for the live verbs,
the live write-back and the coming kit hook: any transform change, `blend_driven`, a driven
object's undriven component included, and any visibility change on an object with an animated
bit. For a person's edit that is a change: a transform under constraints was written, with the
writer's `blend_constrained` warning. A rotation change on an object that rotates another way,
a change to an object nothing writes and a name the file can't hold are refused live with their
codes first. File-level edits keep the writer's per-channel rule: the file verbs and the writer
used directly write a driven object's undriven components (its driven one refused,
`blend_driven`, the channel named) and a transform under constraints, with the
`blend_constrained` warning naming its constraints' kinds.

Rows (`tests/check_edit.rs`): one per rule, each with a control that the same kind of edit on
an object the rule doesn't hold is taken; the first rule broken whatever the edits' order; and
the agreement row: on every generated file but the large one and on the live rows' scenes, each
candidate edit (each field of each object changed, each kind of list op, a collection's field,
two objects in one edit, renames in both orders) gets the same verdict from `check_edit` on the
decoded view as from the write-back of the doc with the edit applied, taken by both or refused
by both with the same code, each code reached; the file path's own rule beside the live one: the
file verb `object place` writes a constrained transform with its warning and a driven object's
x and y with z kept (z refused), where `check_edit` refuses each. Changed:
`row_person_edit_of_driven_channel` expects the write-back's text to start with
`blend_driven: `. The planted bug, a driven transform taken by the live rules, is caught alone
by its row.

### Behaviour change: files of 5.02 and 5.03 are written, and live-editable

The writer writes files of format versions 5.02 and 5.03 (`write::WRITE_TO` is 503,
`write::writes` the one rule, `write::written_to` the newest written): the file verbs `object
place`, `object visibility` and `object rename` write them, with the same refusals and warnings
as every release written (a change to a driven channel refused, `blend_driven`; a transform
under constraints written with `blend_constrained`). A file past 5.03 is refused,
`blend_version`, its message naming the releases written, 3.06 to 5.03, the releases read. The
live tier writes them back (`live::model::writes_back`): each live edit verb applies to a 5.02 or
5.03 doc and its write-back is written; untouched, the write-back is the file's own bytes. The
read-only state applies to none of the releases read: the entry `live_read_only` (`BLEND-E024`)
stays registered, kept for a release the live tier reads before the writer writes it, its text
naming no release (its message takes the releases written as `{written}`).

A known limit, at every release written: a person's edit made on the live doc directly, not
through a verb, that the live rules refuse (`live::check_edit`; a driven transform, which the
verbs refuse with `blend_driven`) is taken by the session, and its write-back is refused (a
`PackError` whose text starts with the rule's code: `blend_driven: @obj:5: its transform is
driven (...)`); the file stays as it was and the doc keeps the edit, so the two disagree until
the edit is undone, and every read of the doc (a whole read, a verb's), which renders the file,
fails the same way. The fix is a kit hook that refuses a person's edit before it applies by
calling `live::check_edit`, as the kit's guards refuse an agent's; until the kit has one, a host
must not let a person write a BLEND doc directly. Row:
`row_person_edit_of_driven_channel` (`tests/newer.rs`), at 4.03 and 5.03.

The evidence (`PROVENANCE.md`, "The program's own saves, patched"): the writer patched the
program's own 5.2.4 and 5.3.0 saves (its builds of the scene spec and its re-saves of the public
samples; places, a scale, each visibility bit, renames), the program reopened each, reported it
and saved it again: every patched value held as it reports it, nothing else changed against its
report of the same files before, but an animated location z it re-evaluated from its action
(the change the writer refuses), and our reader finds the values in its re-saves, at both
releases. The round trip's row `the_program_reopens_our_writes_of_its_own_saves` checks it, run
by the gate's round-trip part (`BLEND_OWN_SAVES_DIR`), whose self-test plants the folder's
absence. `examples/blend_live_edit.rs` runs one live edit through the kit's session as a host
does and writes the file back (`LIVE ok <format> <bytes>` or `LIVE FAIL <code>: <why>`).

Rows (`tests/newer.rs`): the writer writes 5.02 and 5.03 (a place, a render flip and a rename
read back; the driven refusal and the constrained warning) and refuses 5.04; the live tier opens
them writable (agent verbs and a person's edit written back and read by our reader, driven
placements refused, untouched byte for byte) and declines 5.04; the live edit tool on a
generated 5.03 file and on the kept 5.3.0 build of the three cameras; the kept builds
(`tests/builds.rs`) take a live change at 5.2.4 and 5.3.0. The read-only rows run in a gap the
test-only `mutants` feature opens (`mutant::in_gap`, the writer's range set back to 5.01).
Plants caught, each alone: the writer's range left at 5.01, a 5.04 written; and, in the gap, the
versions read stopping at the writer's, the names stopping at the writer's, a read-only
write-back refused without its code, a read-only doc's verbs applied.

### Behaviour change: a link placeholder takes its type code's kind

A link placeholder (a data-block linked from a library that a save holds as its bare `ID`
header, its type only in its stored name's first two bytes: the program's 5.x saves hold a
linked object so) takes the kind of that type code, as the file's own data-blocks of the code
have it: a linked object is kind `object` (`OBPlant`), a linked mesh `mesh`, and so on; its kind
was `id`. A code no other data-block of the file has leaves it `id`. What a host sees changes:

- the model (`IdBlock::kind`) and the structure index: the placeholder's node is of its kind
  (its record digest with it), still flagged `linked`, its struct `ID`; its key and content
  digest are as they were (`@li:plants.blend/id:Plant`);
- `scene list` lists a linked object's placeholder among the objects, `linked`, with its
  address, name and holders only (no data, parent, transform or flags: they are in its
  library): `@li:plants.blend/id:Plant object "Plant" data - parent - in [...] loc - rot - (-)
  scale - shown linked`;
- an object whose data is a placeholder has its data's kind: `data_type` in the index, the data
  in `scene list` and in the live scene's `data` (`mesh @li:plants.blend/id:Leaf`).

Unchanged: a placeholder is never drawn, never a live object or collection and never written
(the writer refuses its key, linked); an object whose data is a mesh placeholder is left out of
the mesh read as `linked_mesh`. A placeholder reads nothing of its kind (no fields, named
values, hidden flag, material slots or detail), so a collection's or a scene's placeholder is
never read as one. Rows (`tests/newer.rs`): placeholders of an object, a mesh, a collection and
a scene at 5.01 and 5.03, each of its kind and linked in the model, the index and `scene list`,
never drawn, live or written, and a text whose code the file has nowhere else `id`; the kept
builds (`tests/builds.rs`): the living rooms' Plant at 5.2.4 and 5.3.0 is an object, a link
placeholder, linked. The planted bug, the kind looked up by the block's code, is caught alone.

### A change to a driven channel is refused (a behaviour change)

The writer (`write::write`, the file verbs' `object place`, `examples/blend_patch.rs`) refuses
a change to a channel an action or a driver animates: `WriteError::Driven`, the live tier's
`blend_driven` (`BLEND-E002`, exit 2), its message naming the channel (`location[2]`) and what
drives it, its report the detail `channel`. A write the writer accepted before is refused now:
the program evaluates an object's animation when it opens a file and keeps the result, so a
location written as (-3, 2, 1) on an object whose z an action animates read back with x and y
kept and z re-evaluated (`PROVENANCE.md`). A channel nothing animates is written: the writer
compares each component a change gives with the one the file stores, bit for bit, so a
location writing x and y and keeping the animated z as stored is written; the object's other
properties, its visibility and its name too.

One reading, shared (`anim::drives`): the channels each object's actions (its own, the one held
aside while a strip is edited, each NLA strip's, strips held in strips) and drivers animate, by
the path and array index each F-curve names, read through the file's own schema: an action's
`curves` list and its channel bags (`strip_keyframe_data_array`, `channelbag_array`,
`fcurve_array`), a bag counting when its slot is the one the animation data or the strip names.
Animation data the reading can't follow (a pointer no block has, a body not kept, a field of
another shape, a count out of range, an action on the object itself) drives every channel. The
live tier marks `edit` `a` from the same reading: a constraint, or a channel of the transform
(location, rotations, scale, their deltas, the rotation mode) animated; an action animating
only other properties leaves the transform placeable live, where any action marked it before.
The live tier holds one `edit` per object, so it refuses a driven object's whole placement; a
constraint drives no channel in the writer (the stored values hold under it). The
`blend_driven` entry's text names both. Test builds: `testgen` actions hold F-curves
(`SceneBuilder::action_with`, by slot), drivers (`Driven::Driver`) and NLA strips
(`Driven::Nla`) of their own, and from 5.00 channel bags beside the list
(`ACTION_SLOTS_FROM`, `SceneBuilder::curves_in_list`). Rows: `driven_quat.blend`'s z refused
typed through the writer and the file verbs, its x and y, rotation, scale and the other objects
written; a driver's channel and an NLA strip's refused, their other components written;
animation the reading can't follow refusing every transform channel; three slots of one action
each driving its own object's channel; one rule (the live tier's `a` exactly where the reading
says the transform is driven; every component the live tier places, the writer writes; every
driven one refused); the program's saves at 5.2.4 and 5.3.0 read per channel through the
channel bags alone, the list alone, and neither. The plant writing a driven channel is caught,
alone. Rows asserting the earlier behaviour, changed: the `blend_driven` entry's text (its
golden `refused-driven`), and the generated files' actions (an action holding no F-curve marked
its objects driven; each now animates the channel the scene spec names).

### A flip of an animated visibility bit is refused (a behaviour change)

The writer (`write::write`, the file verbs' `object visibility`, `examples/blend_patch.rs`)
refuses a change that flips a visibility bit an action (the object's own, or an NLA strip's) or
a driver animates: `WriteError::Driven`, `blend_driven` (`BLEND-E002`, exit 2), the bit named as
its F-curve names it (`hide_viewport`, `hide_select`, `hide_render`; `anim::VISIBILITY_PATHS`)
in the message and as `details.channel`. A write the writer accepted before is refused now. A
change that keeps an animated bit as the file stores it is written with the other bits beside
it, as are the bits nothing animates. The shared reading (`anim::drives`) already read every
channel an F-curve names; the writer now refuses these too. Animation data the reading can't
follow drives every bit, as it drives every transform channel. A merge holds an agent's flip of
a bit the person's save animates, with the writer's reason, as it holds a driven transform.

The live tier refuses `object visibility` on an object with an animated bit (the next entry).
Rows (`tests/write_driven.rs`): flips of a bit an action, an NLA strip,
a driver and an action slot's channel bag animate refused typed, through the writer and the
file verb; a kept bit, the other bits, a transform beside an animated bit and an object nothing
animates written; the live tier placing these objects (`edit` `1`). Their golden
`file-refused-driven-visibility`. The plant writing an animated bit is caught, alone. Rows
asserting the earlier behaviour, changed: animation the reading can't follow (`Lost`) had its
visibility written; a flip of its bit is refused now, a change keeping every bit written; the
`blend_driven` entry's text (its goldens `refused-driven` and `file-refused-driven`) names the
bits and drops "its visibility is still written".

### Live: `object visibility` on an object with an animated visibility bit is refused (a behaviour change, live tier)

The live store gains a derived `visanim` meta field naming an object's animated visibility bits: a
host's outliner sees one more meta column (below).

The live `object visibility` is refused on an object with any visibility bit an action (its own,
or an NLA strip's) or a driver animates: `blend_driven` (`BLEND-E002`, exit 2), nothing applied,
`object show` next, the message and `details.channel` naming the bits as their F-curves name
them (`hide_viewport`, `hide_viewport,hide_render`). The live tier accepted such a call before,
and its write-back was then refused by the writer, so the live doc held a bit the file didn't.
The rule is per object: any animated bit refuses the whole call, a call that sets only bits
nothing animates (or keeps an animated bit as stored) too. Animation the reading can't follow
counts as animating all three bits, as it does for the writer. An object whose transform only
is animated, and one nothing animates, are written as before; the live `object place` and
`object rename` are unchanged. A mode per bit, so a bit nothing animates could be written live
beside an animated one, is a later change to the live store.

The live store holds the animated bits as an object field, `visanim` (comma-separated, in the
order `hide_viewport`, `hide_select`, `hide_render`; present only when a bit is animated),
read from the file by the one reading the writer takes (`anim::drives`,
`live::model::animated_bits`), `LiveObj::vis_animated` in the model; `object show` prints it;
the view index classes it `meta`. An outside save sets it as it sets `edit`; the write-back
refuses a doc whose `visanim` differs from the file's, as for `edit`; a merged doc whose
`visanim` names anything but the three properties is refused, `blend_merge_value`. The
`blend_driven` entry's text names the live rule and its next step names `visanim`.

Rows: `tests/live.rs` (`live_visibility_refused_on_animated_bits`): on a generated file, an
action's `hide_viewport`, a driver's `hide_render` and animation the reading can't follow (all
three named) each refuse the live call typed, with the entry's exit and next step, a flip of a
bit nothing animates on them too; the live doc and the file unchanged; an object nothing
animates and objects whose transform only is animated (a driver of a location, an action of a
location) flip theirs and write back; untouched, the file writes back byte for byte.
`tests/write_driven.rs` (`the_live_store_marks_exactly_the_bits_the_writer_refuses`): on every
generated file but the large one, the live store marks each object's `visanim` exactly where the
writer refuses a flip of that bit alone. The refusal's golden `refused-driven-visibility`; a
merged doc's `visanim` case. The plant dropping the live refusal is caught, alone: accepted
live, the row reports the write-back's refusal. Rows asserting the earlier behaviour, changed:
the `blend_driven` entry's text (its goldens `refused-driven`, `file-refused-driven`,
`file-refused-driven-visibility`); the view index's field classes (golden
`watch_turn_golden/scene_turn.json`, `visanim` added).

### A transform written under constraints comes with a warning

A transform (location, rotation or scale) written to an object with constraints is written, as
before (its stored values hold), and the write comes with a warning:
`write::WriteWarning::Constrained`, code `blend_constrained`, naming the object and each kind of
constraint it has,
once, in the list's order, by the file's own schema: the struct of the constraint's data
(`bTrackToConstraint`), or `unknown` where the reading can't name it (no data, a pointer no
block has, a list it can't follow). The program's constraints decide where it shows the object,
so the placement shown may not move. Its visibility and name come with no warning. The kit has
no warning a pack's call returns (its `ToolOutput::warnings` are its own `note: ` lines about a
call), so the warnings are the pack's own: `Written::warnings`; `verbs::Ran::warnings` from
`verbs::run`, which returns `Ran` (the kit's write and the warnings) in place of the kit's
`BlobWriteResult`; each edit verb's data, `Verb::typed(warnings)`, holding them as `warnings`,
each `{code, object, kinds, text}` (`verbs::warning_data`), `text` its `note: ` line for stderr
(`WriteWarning::note`), as the kit's warnings read. `warnings` is a new field: the shapes keep
version 1. A warning is no refusal: the exit is 0 and its code is no entry of the pack's
refusals. The live tier is unchanged (it refuses a constrained placement). The shared reading
gives the kinds (`anim::Drives::constraint_kinds`), read on their own, never making a channel
driven. Test builds: `SceneBuilder::constrained` (a schema that gives a constraint its data),
`Driven::Constraints`. Rows: a location, rotation, scale and all three written to an object with
a track-to, a child-of and a track-to again, each with one warning naming the two kinds, through
the writer and the file verb; its visibility, its rename and an object without constraints with
none; a constraint with no data, and a schema whose constraints have no data, `unknown`. Golden:
`file-object-place-constrained`; the file verbs' data goldens (`file-object-place`,
`file-object-visibility`, `file-object-rename`) gain `warnings: []`. The plant dropping the
warning is caught, alone.

### The reader's default raw cap: 4 GiB

`ReadLimits::default()` decodes at most 4 GiB after decompression
(`error::DEFAULT_MAX_RAW_BYTES`), not 64 GiB. The reader streams, so its memory stays bounded at
any cap; the cap bounds the decoding a small compressed file can cost a host that reads with the
defaults. It is the streaming index's default too (`pack::INDEX_MAX_RAW_BYTES` is that value,
and `pack::index_read_limits()` equals `ReadLimits::default()`); the whole-file paths keep the
live tier's 512 MiB. A host that reads larger files passes its own `ReadLimits`. Rows: a stream
of about 128 KiB that decodes to just past 4 GiB is refused by the default, typed, in a small
heap; the index's defaults are the reader's. The plant leaving the default at 64 GiB is caught.

### A still after a live edit, without a whole read

`live::live_k2scene(state, carrier, limits)` writes the K2SCENE1 of a live state as two parts: a new
header, and the carrier's mesh data section, shared (`Arc<[u8]>`, the same allocation on every call
over one carrier). No live change makes or changes geometry, so the meshes and their hashes are the
carrier's; the header goes through the mesh read's own steps (placements by the live view's rule and
the kit's `world_matrix`, the shared hidden-by rule, ids and keys from the store, each mesh by the
object's data in the carrier's read, the same bounds, checks and header text), and `header ++ data`
is byte for byte the whole read (`mesh_read_paired`) of the version a write-back of the state
produces. No file is decoded: the view's carrier read (once per carrier) keeps its data section,
each mesh's counts and hashes and what the read takes of each object from the file
(`meshread::read::CarrierRead`); the header writer is shared (`k2scene::write_header`). With it:
`carrier_id` (SHA-256 of the data section, the same exactly when the data is), the `changed`,
`added` and `deleted` keys against the carrier's read, and typed declines (`ReuseDeclined`, codes
`reuse_*`), each falling back to the whole read. `live_k2scene_of` takes a carrier view and pairing
a host holds. The live view keeps that carrier read while the carrier lasts: one data section per
live doc (at most the output limit, 256 MiB by default), not the whole-file decode. Test builds:
`reader::take_reads` counts the files the reader reads on a thread, and
`testgen::scenes::living_room_edited` writes the living room with a collection hidden, an object
moved, one left out or a mesh grown. Rows: parity after a place, a turn, a hide, a rename, a
parent's move and a camera's move through the kit's write-back, and after a collection hidden, an
object moved between collections and one deleted against generated files holding them; headers of
every pad; the carrier id; one shared data section; no file decoded during calls; the lists; twelve
declines; a call's heap peak within 16 bytes per header byte and 64 KiB. Plants caught, each alone:
the carrier's transform kept, the id from the data's length, the file decoded on every call, the
data copied, every key listed as changed. `docs/WIRE.md` has the section for hosts.

### An app's save of the noted version merges on the live tier

`BlendLive` checks a doc the kit merged field by field from an outside save
(`Pack::check_merged_view`, `live::merged::check_merged`), so a save from an app whose opened
version the host noted (`note_opened`) merges instead of keeping both: an app that reopened
what the kit wrote and saved it with another change keeps the kit's changes and its own. The
hook sees the merged doc only (no pinned version, save or carrier); the save taken in becomes
the carrier, so the merged doc differs from it only where the live doc changed since the
version the app opened. The check is of what the merged doc itself must hold: one name per
object of the file (`blend_merge_name_taken`), each file key its name's (`blend_merge_nid`), a
name that fits its field (`blend_merge_name`), values in the store's own shapes
(`blend_merge_value`), holders and parents the doc lists (`blend_merge_holder`,
`blend_merge_parent`). A doc that breaks one keeps both, as with no version noted. Rows: a
re-save of the written-back version with another object moved merges after `note_opened`
(taken in, the save's change and the session's own kept, no copy), keeps both without it, and
is nothing when it is the same bytes; a merge giving two objects one name keeps both (the save
the doc, the kit's version a copy on disk); each rule broken alone is refused by its code.
Plants caught, each alone: every merged doc refused, every merged doc taken.

### Files of format versions 5.02 and 5.03: read

(This release also writes them, live and through the file verbs: "files of 5.02 and 5.03 are
written, and live-editable", above. The read-only state below is what `live_read_only` does for a
release read before it is written; no release reaches it today.)

The reader names the flags and values of files saved at 5.02 and 5.03 (`scene::NAMES_TO` is
503), the mesh read draws them, and the live tier opens them (`scene::READ_FROM` to
`scene::READ_TO`, `scene::reads`); a file past 5.03 is declined, `live_version`, its message
naming the releases read (3.06 to 5.03). The writer writes up to 5.01 (`write::WRITE_TO`,
`write::writes`): a file verb that would write a 5.02 or 5.03 file is refused, `blend_version`,
its message naming the releases written (3.06 to 5.01). A live doc of a 5.02 or 5.03 file is
read-only: it opens and reads as its file holds it (the scene map's `version` says which,
`live::model::writes_back`), each live edit verb is refused with the new entry `live_read_only`
(`BLEND-E024`, exit 2), and a write-back of a change fails with a `PackError` whose text starts
`live_read_only: `, nothing written; untouched, the write-back is the file's own bytes. The kit
offers a pack no way to open a doc read-only, so a host's own edit to such a doc is taken by
the session, and its write-back (and a read, which renders the file) then fails, typed: a host
shows these docs read-only. Where the meanings at 5.02 and 5.03 come from is in
`PROVENANCE.md` ("Flags and values"): the program's own saves at 5.2.4 and 5.3.0, through the
round trip, which also checks the live tier and the mesh read in full on its builds of our
scene spec.

A link placeholder (an object linked from a library that a save holds as its bare `ID` header, its
type only in its stored name) is linked in the model and is neither drawn, a live object nor
written; `testgen::SceneBuilder::link_placeholder` writes one. Rows (`tests/newer.rs`): the mesh
read draws 5.02 and 5.03 files and declines 5.04; their values are named and 5.04's are not; the
live tier opens them, refuses each edit verb and a changed write-back by `live_read_only` and leaves
the file as it was; the writer refuses them, typed, and the file verb `scene meshes` reads them; a
link placeholder at 5.01 and 5.03 is linked, never drawn or written; the mesh read's world matrices
at 5.03 equal a composition made in the row alone, in `f64` (Euler rotations in three orders, a
quaternion, a two-level parent chain; `tests/meshread.rs`). Plants caught, each alone: the versions
read stopping at the writer's, a version past 5.03 read, the names stopping at the writer's
versions, a read-only write-back refused without its code, a read-only doc's verbs applied.

The program's own builds of the test scenes at 5.2.4 and 5.3.0 are kept in the repo
(`crates/blend/tests/builds/`: each release's nine saves and its report, the scene spec, and
`MANIFEST.tsv` with each file's size and sha256; `NOTICE.md`), and `tests/builds.rs` checks them
in every gate, with no environment: the manifest first (every file, nothing unlisted), then each
release's saves against its own report as the round trip's builds are (`tests/builds_support`,
shared with `tests/roundtrip.rs`): world matrices, the driver's own composition for an object
the program doesn't evaluate, triangles, cameras and fits, hidden, named values, the linked
object, an object in no scene never drawn, the live tier read-only, and no path in what a host
gets. Plant caught alone: a kept file changed on disk.

### Tests and tooling

- `scripts/check-labels.sh` reads every file under its root outside a git checkout (as
  `scripts/check-private.sh` does) and fails closed (exit 2) when it finds none to read; its
  self-test shows both.
- **The program's round trip, per file** (`tests/roundtrip.rs`, `scripts/roundtrip.sh`, run by
  `scripts/check.sh` when a change lands): the program that writes BLEND files, at 5.2.4 and
  5.3.0, opens each input in a separate process and reports every object; each input is checked
  on its own (the program's evaluated world matrices against our composition, its triangle
  counts against ours, each camera's vertical field of view against ours), and the writer's
  patches of four public samples the program itself saved (3.06, 4.03, 5.00, 5.01) are checked
  both ways: the program read the values written, and our reader finds them in the program's
  own saves. The part hands the test an absolute folder, and fails without the program's builds
  beside the releases' folder.
- **The sensor fit's meanings are pinned** by that run: a camera in each fit, the image wide and
  tall (pixels twice as tall as wide), gives the program's own field of view, equal to the mesh
  read's for all six within 1e-6 rad.
- **The test scenes** (`testgen::scenes`) hold what a saved scene holds: each master collection
  carries the embedded flag (1024, as the samples' do), and each scene its tool settings, render
  views, a view layer with its layer collections and bases; two camera-fit scenes join them. The
  generated living room's family stays out of the round trip's inputs (its library block comes
  before its local data, which the program then files as library data); the program's own
  builds of those scenes stand in for it.

## v0.1.0: pairs with kit v0.21.0

The BLEND pack: it reads, indexes and writes BLEND files, and agents write them. **Requires
`k2-mpc-base` v0.21.0** (results as data with their shape named, and a pack's registered
refusals, on the carrier and its pairing), checked out beside this repo (a host vendors both side
by side: `vendor/k2-mpc-base` beside `vendor/k2-mpc-blend`); `Cargo.lock` and `fuzz/Cargo.lock`
name that kit.

### Reading

- **`reader`** reads a file forward only, under `ReadLimits`: the classic header and the newer
  one (17 bytes, 32-byte block headers with 8-byte lengths; any value of it not seen in sample
  files is refused as unknown), plain, gzip or zstd storage, every block to the end block, and
  the file's own schema, through which every struct and field is looked up by name. zstd is read
  frame by frame: each frame's window checked against the reader's limit, which bounds the
  decoder too, dictionaries refused, skippable frames skipped, checksums checked, and each
  block's output bounded by the format before any of it is read. Large blocks are digested as
  they pass, never held; a 1.2 GiB file reads in under 2 MiB of heap. Pointers resolve to the
  one block with their address, else the one owned by the same data-block, else the one
  data-block, else they are ambiguous. Every refusal is typed, with fixed text.
- **`scene`**, the scene model: every data-block with its type code, struct, kind, name,
  library and key (`@ob:Cube`; `@li:lib.blend/ma:Steel` when linked). Scenes hold their
  collections and objects, a camera and a world; objects their data, parent and material
  slots; cameras, lights, materials and collections a few fields, named as the file names them.
  A content digest per data-block, with pointers replaced by what they name, digests another
  save of the same scene the same. For format versions 3.06 to 5.01 (`scene::NAMES_FROM` to
  `NAMES_TO`), visibility bits, object, light and camera types and rotation modes are named,
  learnt from files saved headless at three pinned releases (`PROVENANCE.md`); other versions
  keep them as numbers.
- **`pack`**: `BlendPack`, the kit's `BlobPack`. Each version is indexed into the kit's
  structure index (verbs `object list`, `object show`, `object diff`, `object blame`): scenes,
  then what each holds, then every other data-block but the program's own interface, with the
  file's fields and the model's references as attributes and an object's data as its weak ID.
  `visible` is false for an object or collection disabled in viewports. The index format is 2.

### Writing, merging and the file verbs

- **`write`** patches an object's fixed-size fields in place (location, Euler rotation, scale,
  the three visibility bits, the name inside its field), keeps every other byte, and reads the
  result again before returning it; a compressed file is written back whole (gzip; zstd in
  frames of at most 8 MiB) and decoded again. Refused, typed: format versions outside 3.06 to
  5.01, a key naming no object or several, a linked object, a field not in the schema at the
  expected type and size, an Euler rotation on a quaternion or axis-angle object, a name that is
  empty, holds a NUL, doesn't fit or is taken, a value that isn't finite.
- **`merge`**: the three-way merge of a person's app save with the agent's version, by object
  and field. The save is the file and the agent's field changes are replayed onto it; a field
  both changed is reported (`FieldConflict`), never decided by who saved last. A clash keeps
  both, whole-file: the kit keeps the save as the file and the agent's version as a conflict
  copy in its store. On the blob tier every app save that isn't one side's bytes again keeps
  both (`BlendPack`'s `can_merge` is `false`): the kit hands a pack what the file last held, not
  the version the app had open. `merge_known_base` is the merge for a base known to be the
  app's.
- **`verbs`**, the agent's file verbs: `scene list` (collections, objects, and each camera by
  its object's name), `scene meshes` (the mesh read's report), `object place`,
  `object visibility`, `object rename`, each edit run on the version the agent read and refused
  when the doc has moved on. Each answer comes as data (`verbs::Typed`, `data_spec`
  `blend.file.<verb>/1`). Agents may write (`AgentWrites::Allowed`); a host that wants verbs
  only refuses raw agent writes itself.
- **Limits**: what is decoded whole in memory (the merges, and the file verbs a host runs with
  them) defaults to the live tier's limits, 512 MiB after decompression
  (`BlendPack::default().limits`). The streaming index, which never holds the file, decodes at
  most 4 GiB by default (`BlendPack::index_limits`, `pack::index_read_limits`,
  `INDEX_MAX_RAW_BYTES`); a host sets either.

### The live tier (off by default)

`live::BlendLive`, the kit's live `Pack`, which a host opens a file with only when it chooses to:

- **The store**: each scene's master collection, each collection, and `c0` for objects no
  collection holds, then the objects, with minted keys in file order. An object's transform is
  one field (`xform`, each `f32` as its shortest round-trip text), its visibility three bits,
  its name its file key's, and `edit` says what may be written. Meshes, materials, modifiers and
  every other byte stay in the file the kit keeps (its carrier).
- **Declines**, typed: `live_version` (outside format versions 3.06 to 5.01), `live_ambiguous`,
  `live_value`, `live_too_large` (over `MAX_FILE_BYTES`, 32 MiB stored, or a reader limit), and
  the reader's own.
- **The write-back** patches the changed fields into the carrier with the writer, addressed by
  the carrier's pairing; anything else that differs is refused, never written by guess.
  Untouched, the carrier stays byte for byte. **Outside saves** pair collections and objects by
  file key, else by their data or parent, else mint new keys.
- **The view**: collections are containers, each naming its parent; objects are units, each
  placed in 3D on the kit's view slot (location, rotation in its own mode, scale, parent and
  parent inverse) in the doc's space, with metres per unit only when every scene agrees. Each
  unit carries its mesh's hash and what hides it; each scene marks the one drawn.
- **Verbs**: `scene list`, `collection show`, `object show`, `object place`,
  `object visibility`, `object rename --break-links`. Each returns the kit's result object as
  data (`mpc.result/1`, `data_spec` `blend.<verb>/1`); each refusal prints `refused: <code>`
  and a `next:` verb.

### The mesh read

`meshread::mesh_read` returns a version's scene as K2SCENE1 for a host's renderer, and a report.
It is pure: the same bytes give the same output. Meshes are read by the file's own schema from
any of three layouts and triangulated with their corner order kept; world matrices are composed
from stored transforms; perspective cameras are composed with a fixed arc tangent; each mesh
carries a SHA-256 content hash. Every count, the heap the read needs (512 MiB at most) and the
output's length are checked before anything is allocated. `docs/WIRE.md` is what a client codes
against.

### Refusals

Every refusal of the verbs, the reads and the live tier's open is registered
(`live::errors::ERRORS`, `BLEND-E001` to `BLEND-E023`), with its message, what to do next and,
where there is one, the call to run. A fault of the pack itself (the writer's read-back, the
mesh read's own check) says to report it to the pack's maintainers.

### Tests and tooling

- Files built from a schema of ours (`testgen`) in both header forms, both pointer sizes and byte
  orders, and all three storages; every refusal by type; crafted zstd streams of under 128 KiB
  that would decode to hundreds of MiB, refused in a heap under 8 times the raw cap.
- The merge matrix (88 pairs against an oracle), the live rows, golden view snapshots, each
  verb's result and refusal pinned, the scene's watched turn pinned, and planted bugs each caught
  by its own row (`tests/mutants.rs`, one alone with `BLEND_MUTANT_ONLY`).
- The public corpus: seven CC0 demo files from format versions 2.73 to 5.01, fetched by
  checksum, read, compared with an independent test-only reader, indexed, and drawn and opened
  live where their format version allows.
- Fuzz targets on the reader and the model, the index, the writer and the triangulator.
- `scripts/check.sh`: formatting, lints, the docs with default and with all features, the
  suite and the plants, licences and advisories at the locked versions, the licence files, no
  internal host names, labels or commit hashes in public text, every test target accounted for,
  and the program's headless round trip when a change lands.
- `PROVENANCE.md`: where every layout comes from, with the sample files' licences and checksums.
