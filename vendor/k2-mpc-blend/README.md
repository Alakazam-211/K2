# k2-mpc-blend

The BLEND pack for the multiplayer kit: 3D scene files that people and agents work on at once.
BLEND files are read in; those of format versions 3.06 to 5.03 also open live and are written
back, and files of other format versions, older ones among them, are read only. What is written
is an object's fixed-size fields: its location, rotation and scale, its visibility bits, and its
name inside the name field. Every other byte of the file stays as it was.

- **MPC** (multiplayer cores): files that people and agents work on together, with presence,
  attribution, history and comments.
- **The kit** (`k2-mpc-base`): the file-type-agnostic part of MPC. It keeps a file, its versions and
  who made them, and serves a file's structure for comments, diffs and blame. It knows no format.
- **A pack** (this repo): what one file type adds to the kit. This one handles BLEND files.
- **A host**: the program that runs the kit and its packs for people and agents. **K2** is one:
  it vendors the kit and its packs into its own repository.

## What it does

The kit keeps each version of a BLEND file as one blob; this pack reads each version into a tree
of its scenes, collections, objects, materials, cameras, lights and every other data-block beside
it, so people and agents can comment on, diff and blame by data-block. Agents change objects'
fixed-size fields through verbs. On the live tier, a person's app save merges with the agents'
changes by object and field; on the blob tier, the kit keeps both copies of such a save (the
save as the file and the agent's version beside it).

- **`reader`**: the file, forward only and under limits: both header forms, plain, gzip or zstd
  storage, every block, and the file's own schema, through which every struct and field is
  looked up by name at read time. Large blocks are digested as they pass, never held. Anything
  it doesn't know or can't trust is refused with a typed error.
- **`scene`**: the scene model. Each data-block's key is its type code and name (`@ob:Cube`),
  scoped by its library when linked (`@li:lib.blend/ma:Steel`); a key is only as stable as the
  name, and a rename is a new key. Objects carry their data, parent, transform and material
  slots; cameras, lights and materials a few of their fields, named as the file names them;
  everything else is carried as a content digest with pointers replaced by what they name, so
  another save of the same scene digests the same.
- **`write`**: an object's location, rotation, scale, visibility bits and name, patched in place
  where the file's schema puts them, every other byte kept, the result read again before it is
  returned; anything it can't prove safe is refused, typed, a change to a channel an action or
  a driver animates among them, a visibility bit it flips too (the program recomputes it when
  it opens the file; `anim`, the live tier's reading too). The program animates `hide_viewport`
  and `hide_render`, not `hide_select`: its own tools refuse to key `hide_select` (not
  animatable), so a refused `hide_select` flip comes only from a file another tool made with an
  F-curve or a driver on it; the writer refuses whichever bit such a curve names. A transform
  written to an object with constraints is written, with a warning naming their kinds
  (`blend_constrained`).
- **`merge`**: the three-way merge of a person's app save with the agent's version, by object
  and field: the person's save is the file and the agent's changes are replayed onto it. A
  field both changed is reported, never decided by who saved last; until the kit keeps held
  conflicts durably, such a save is a whole-file conflict (the kit keeps the save as the file
  and the agent's version beside it). The merge needs the version the app had open as its
  base; the kit doesn't say it yet, so in a session every such save keeps both (`pack`).
- **`verbs`**: `scene list` (collections, objects, and each camera by its object's name with
  its lens and the scene it is active in), `scene meshes` (the mesh read's report),
  `object place`, `object visibility`, `object rename`, each edit run on the version the agent
  read. Each answer comes as data too (`verbs::Typed`, `blend.file.<verb>/1`), and each refusal
  has a registered id (`VerbError::report`).
- **`pack`**: `BlendPack`, the kit's `BlobPack` for these files: the index and the anchor rules,
  with no state of its own. The index reads each version as a stream and never holds it, under a
  decoded-bytes cap (`index_limits`, 4 GiB after decompression by default, which a host sets);
  what is decoded whole (merges, file verbs) is under `limits`, 512 MiB by default. It doesn't
  merge in a session yet (`can_merge` is `false`): the kit hands it what the file last held, not
  what the app had open, and once the kit has written the file since the app's last save the two
  differ, so a merge could undo a kit write unseen. The kit keeps both instead.
  `merge_known_base` is the merge for when the kit says.
- **`live`**: `BlendLive`, the kit's live `Pack` (off by default): the scene's collections and
  objects as an outliner on the kit's structured mode, with each object's transform, visibility
  and name, and each object placed in 3D on the kit's view slot (its location, rotation and
  scale in the file's own form, its parent and the parent inverse); agents' verbs on it
  (`scene list`, `collection show`, `object show`, `object place`,
  `object visibility`, `object rename --break-links`); and a write-back that patches the changed
  fields into the file the kit keeps (its carrier), paired by the kit's pairing table, by the
  live rules (`live::check_edit`). The pack opts in to the kit's edit check
  (`Pack::check_edit`): every live change, whoever makes it (a person's edit, an agent's verb,
  undo, restore), is asked by the same rules before it applies, and one the write-back can't
  write is refused at the call (the kit's `pack_refused`, `MPC-E823`, with the rule's code), so
  a host may let people edit a live scene directly. An app's
  save after the kit wrote the file keeps both, unless the host noted the version the app
  opened (the kit's `note_opened`): then the kit merges the save field by field and the pack
  checks the merged doc (`live::merged`), keeping both for one the format forbids (two objects
  of one name).
  A driven transform refuses `object place`, and an object with any animated visibility bit
  refuses `object visibility` (`blend_driven`), so the live doc never holds what the writer
  wouldn't write. Everything else in the file never enters the live store. Each verb's call returns the kit's
  result object as data (`mpc.result/1`, `data_spec` `blend.<verb>/1`), and every refusal of the
  verbs and of the live tier's open is registered (`live::errors`, ids `BLEND-Ennn`).
- **`fuzz/`**: cargo-fuzz targets on the reader and the model, the index, and the writer: test
  tooling, never shipped.

`PROVENANCE.md` says where every layout comes from: the file's own schema, an openly licensed
description of the container, the public release notes, and openly licensed sample files.

## The kit

The pack takes the kit by path: `crates/blend/Cargo.toml` names
`../../../k2-mpc-base/crates/base`, so a checkout of the kit sits beside this repo's, at the kit
release `CHANGELOG.md` names. A host vendors both side by side: the kit at `vendor/k2-mpc-base`,
this pack at `vendor/k2-mpc-blend`. `Cargo.lock` is committed. A public reader gets the kit
from K2's repository, https://github.com/Alakazam-211/K2, where it is vendored at
`vendor/k2-mpc-base`: check that folder out beside this repo as `k2-mpc-base`. Kit v0.23.0, the
release this pack pairs with, is vendored there from the K2 release that vendors kit v0.23.0
on.

Build and test: `scripts/check.sh`. One of its steps, `scripts/check-private.sh`, **fails without
private patterns** (exit 2, fail closed): it looks for the names of a host's own build machines,
which are never in this repo, so the patterns live outside it. Give them one per line (extended
regular expressions) in `MPC_PRIVATE_PATTERNS`, in a file named by `MPC_PRIVATE_PATTERNS_FILE`, or
in `~/.config/k2-mpc/private-patterns.txt`; a pattern that matches nothing (say
`MPC_PRIVATE_PATTERNS='a^'`) runs the rest. The rows that need files the repo doesn't hold are
`#[ignore]`d there, and each fails without its folder: `scripts/public-corpus.sh [DIR]` fetches
the public corpus and the unit probes (`scripts/fetch-corpus.sh`) and runs their rows by name;
the saved-file and round-trip rows run on a build node (`PROVENANCE.md`); `scripts/measure.sh`
runs the measurements in a release build. The program's headless round trip is the gate's
round-trip part (`scripts/roundtrip.sh`, with `BLEND_ROUNDTRIP_DIR`): `scripts/check.sh` runs it
when a change lands (`CHECK_TARGETS_LANDING=1`), and it refuses a run that checked nothing.
`python3 -I scripts/corpus-expected.py DIR` (Python 3.14 or later, for zstd) reproduces
`crates/blend/tests/corpus/expected.tsv`: an independent reader used as a test checker only.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions. Third-party code keeps its own
licence (see `NOTICE.md`).
