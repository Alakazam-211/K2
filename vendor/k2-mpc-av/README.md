# k2-mpc-av

Reads a **set** of AV system drawings the way a technician reads them, over the
format-neutral drawing model of `k2-mpc-cad`:

- **devices across sheets**: one physical device, named by an identifier in a block's
  attribute, wherever it appears;
- **cables**: each cable with its ends, the devices it runs between and its label: a line or
  polyline on a cable layer, or a straight run drawn as a block (scaled and turned along it),
  labelled by a nearby text or by a leader whose arrow touches it;
- **boundaries and pulls**: a cable crossing a dotted boundary (a rack, a room) has to be
  pulled through it, which makes the wire pull list;
- **flags**: off-sheet connectors, an outgoing flag paired with its incoming one by identifier,
  or two flags of one kind (no direction) paired by their value;
- **title blocks**: each sheet's number, name and latest revision (with its date), and the
  sheet index. A title block shared by external reference is read from the referenced
  drawing when the set includes it (the reference's path resolved within the set), the
  sheet's own number and name from a block of its own.

A drawing the sheets reference that has no sheet title of its own (a backdrop, a title block
drawing) is a **reference drawing**, listed apart from the sheet index.

It never writes a drawing. Per-cable progress (pulled, terminated) is not file content: it is
an annotation on the cable's handle, kept by the multiplayer kit.

## Configuration

Every convention (which attribute tags hold device identifiers, which layers hold boundaries
and cables, which blocks are flags or the title block) comes from a configuration file the
caller passes, one `key = value` per line (`#` starts a comment; lists are comma-separated):

```
device.tags = DEVICE, DEVICE_ID
device.letters = 2-4
device.digits = 2-3
boundary.layers = ROOMS
boundary.ltypes = DASHED, HIDDEN
cable.layers = AV-CABLE
cable.blocks = CONDUIT
cable.label_tags = LABEL, TAG
cable.snap = 1.0
cable.label_within = 2.0
cable.label_letters = 1-3
cable.label_digits = 3
flag.out_blocks = FLAG-OUT
flag.in_blocks = FLAG-IN
flag.blocks = OFFPAGE
flag.id_tag = FLAG
flag.sheet_tag = SHEET
flag.label_tag = TO
flag.kind_tag = KIND
title.block = TITLE
title.sheet_block = SHEET, SHEET2
title.sheet_tag = SHEET_NO, NO
title.name_tag = SHEET_NAME, NAME
title.rev_prefix = REV
title.date_prefix = DATE
title.number_from = blocks, layout, file-stem
```

Where a key takes a list, the first that applies wins: a device is a reference's first tag
holding an identifier (an empty value is no finding); a sheet's number is the first configured
tag with a value on its sheet blocks (blocks that disagree are a finding); a run drawn as a
block is labelled by the first of its label tags with a label's shape. Revision and date rows
read their number after the prefix, a separator (`_`, `-`, a space) between allowed
(`REVISION_2` with `title.rev_prefix = REVISION`). With `flag.label_tag`, flags are read as
labelled ends, each naming where it leads (a device, a sheet), never paired. `title.number_from`
says where a sheet's number may come from, in order: its sheet blocks, its paper layout's name,
its file's name (each only when shaped as a sheet number). An unknown key, or a value of the
wrong shape, is refused, naming the accepted keys. A
project's own configuration stays with the project, never in this repo.

## Output

`av.set/1` typed data: the sheet index (each sheet's number, name, latest revision and its
date, and the drawing its title block was read from), the reference drawings with the sheets
that reference them, the devices with their sheets, the cables (a cable drawn as a block with
its block and attributes), the wire pull list, the flag pairs (`directed`: an outgoing and an
incoming flag, `out` and `in`; or two flags of one kind, `a` and `b`), and the findings
(malformed identifiers, a cable end at no device, a cable without a label, unpaired or doubly
used flags, sheets without a title block, a sheet whose sheet blocks disagree on its number).

## Reading a set by hand

`cargo run --release -p k2-mpc-av --example av-read -- CONFIG DIR` reads every DXF drawing
under `DIR` as one set and prints its reading in counts and finding codes only, never a value
(`set::summary`); `--full` prints every row, values included, for the machine the set is on.

## Building and testing

`k2-mpc-cad` and the kit (`k2-mpc-base`) are path dependencies: check them out beside this
repo. `scripts/check.sh` runs the repo's checks, as in `k2-mpc-cad`.

## Licence

MIT OR Apache-2.0, at your option (`LICENSE-MIT`, `LICENSE-APACHE`). See `NOTICE.md` and
`PROVENANCE.md`.
