# Changelog

## v0.2.0: pairs with kit v0.23.0 and k2-mpc-cad v0.4.1

It builds with **`k2-mpc-cad` v0.4.1** (the drawing model and the DXF reader) and **`k2-mpc-base`
v0.23.0**, the kit `k2-mpc-cad` v0.4.1 pairs with, both checked out beside this repo (a host
vendors the three side by side); `Cargo.lock` names them. The library's code is unchanged from
v0.1.0: still read-only, with no edit verbs. A minor release because the CAD reader it builds on
moves from v0.1.0 to v0.4.1; the known limits of `k2-mpc-cad` v0.4.1 apply.

- **Tests:** the nested title sheet takes the CAD generator's external reference
  (`Spec::xref`) and sets its content, where it built a `testgen::Block` whole; that literal
  didn't compile against the generator's newer fields. The drawing is the same.
- `Cargo.lock`: k2-mpc-base 0.23.0, k2-mpc-cad-model and k2-mpc-dxf 0.4.1, and their dependencies
  as cargo resolves them; the crate version 0.2.0.

## v0.1.0: pairs with kit v0.21.0

The first release of the AV reading layer. It builds with **`k2-mpc-cad` v0.1.0** (the drawing
model and the DXF reader) and **`k2-mpc-base` v0.21.0**, the kit `k2-mpc-cad` v0.1.0 pairs
with, both checked out beside this repo (a host vendors the three side by side:
`vendor/k2-mpc-base`, `vendor/k2-mpc-cad` and `vendor/k2-mpc-av`); `Cargo.lock` names them.

**This release is read-only.** It reads a set of AV system drawings (devices across sheets,
cables, boundaries and the wire pull list, off-sheet flags, title blocks and the sheet index)
and never writes a drawing. It has no edit verbs.

- **What a host gets:** `k2_mpc_av::read_files` (a configuration file's text and a set of DXF
  files, each a name and its bytes) and `k2_mpc_av::read_set` (a parsed `Config` and sheets
  already read into the drawing model), both giving `av.set/1` typed data;
  `k2_mpc_av::set::summary`, the same reading in counts and finding codes only, never a value.
  A refusal is a typed code (`AV-E001 bad_config`, `AV-E002 sheet_unreadable`) or the drawing
  reader's own.
- **Known limits:** `read_files` reads DXF sheets; a DWG sheet reaches `read_set` only as a
  drawing model a host has read itself (`k2_mpc_dwg::drawing::DwgDrawing`, through the model's
  `DrawSource::doc`). Every convention (tags, layers, linetypes, blocks)
  comes from the caller's configuration; nothing about a project is built in. Text boxes come
  from the drawing model's estimates. The known limits of `k2-mpc-cad` v0.1.0 apply.
- **Tests** run on drawing sets the CAD repo's generator draws when they run, with neutral
  names. No referee program and no sample drawing is used, and none is committed.

### Reading a set of AV system drawings
- Devices across sheets: a device is every block reference whose attribute (by the
  configured tags) holds its identifier; one device appears on every sheet that names it.
  An identifier that doesn't follow the configured shape is a finding.
- Cables: each line or polyline on the configured layers, with its end points, the devices
  at its ends and its label (the nearest text, of the configured label shape when one is
  given). An end at no device, or a cable without a label, is a finding. A leader whose arrow
  touches a cable labels it, wherever its text sits (a multileader's own text, a leader's by
  the handle it points to), from any of a multileader's lines; a text a leader holds labels no
  other cable by nearness. Labels are matched by their plain text: a multiline text's or a
  multileader's inline formatting codes dropped.
- Cables drawn as blocks: a reference of a configured cable block (a straight run scaled and
  turned along its route) is a cable, its ends its block's long axis as placed, its attributes
  (size, fill) given with it.
- Boundaries and pulls: closed polylines on the configured layers or linetypes are
  boundaries, named by the text inside them; a cable crossing one is a pull, listed with the
  devices at its ends and its label.
- Flags: off-sheet connectors pair an outgoing flag with an incoming one by identifier,
  across the set; or, for flags of one kind with no direction (`flag.blocks`), the two with the
  same value, whatever its shape. An unpaired or doubly used flag is a finding.
- Title blocks and the sheet index: each sheet's number, name and latest revision. A title
  block may be an external reference to a drawing of the set (resolved by its path within the
  set): its revision rows are read there, dated when the dates' prefix is configured, and the
  sheet's number and name from a block of the sheet's own.
- Reference drawings: a drawing the sheets reference that has no sheet title of its own (a
  backdrop, a title block drawing) is listed as a reference drawing with the sheets that
  reference it, not as a sheet missing its title.
- Block references go by their block's real name: a reference drawn through the anonymous
  block of a dynamic block is matched (as a device, a flag, a title block) by the dynamic
  block's own name.
- A set's reading in counts and finding codes only (`set::summary`), and `av-read`, an example
  that reads a folder of drawings as a set and prints it.
- Lists where a set varies: device tags (a reference is a device by its first tag holding an
  identifier; an empty value is no finding), sheet blocks and their number and name tags (the
  first configured tag with a value; blocks that disagree are a finding), a run's own label
  tags (`cable.label_tags`). Revision and date rows allow a separator after their prefix.
- A title block placed on the sheet, else in the drawing of the set a placed reference names,
  else a copy inside a reference's block (as a converter writes one); never a reference
  inside another block taken for the sheet's own.
- A flag whose identifier tag is empty is a flag without an identifier, never paired.
- Flags as labelled ends (`flag.label_tag`, `flag.kind_tag`): each names where it leads, none
  pairs, and their labels are counted against the set's devices and sheets.
- A sheet's number from its sheet blocks, its paper layout's name or its file's name, in the
  configured order (`title.number_from`), only where shaped as a sheet number.
- More counts in the summary: each open cable end's distance to the nearest device symbol and
  whether it meets another cable; the title block's revision and date tags and how many are
  filled; where each sheet's number came from, and whether its layout's or file's name holds
  one.
- A cable's end meets a device at its insertion point, else at its symbol's edge (the
  block's drawn box as placed), within the snap distance; else, within it of another cable's
  end or body, it is a junction (the run goes on), not an open end. An end at a flag (its
  insertion point or its symbol, within the snap distance; after a device) ends at that flag,
  whose label is the cable's destination.
- The summary's diagnostics, counts only: where titles came from, each device tag's values by
  why they are or aren't identifiers, each flag tag's values (empty, distinct, used twice,
  once, more), and how cables' ends met devices.
- Every convention comes from a configuration file the caller passes; an unknown setting is
  refused, naming the accepted ones.
