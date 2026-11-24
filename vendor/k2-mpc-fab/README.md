# k2-mpc-fab

The fabrication-output pack for the multiplayer kit: it reads the files a PCB design exports for
fabrication, photoplot files and drill files (and reads common drill exports), so they open as
documents with versions, comments and a navigable structure index. It reads them only: it never
writes one, and agents can't write one through it.

- **MPC** (multiplayer cores): files that people and agents work on together, with presence,
  attribution, history and comments.
- **The kit** (`k2-mpc-base`): the file-type-agnostic part of MPC. It keeps a file, its versions and
  who made them, and serves a file's structure for comments, diffs and blame. It knows no format.
- **A pack** (this repo): what one file type adds to the kit.
- **A host**: the program that runs the kit and its packs for people and agents. **K2** is one:
  it vendors the kit and its packs into its own repository.
- **Tier 0**: the kit's way of keeping a file as whole versions (bytes), with presence,
  comments, diffs and blame on the structure a pack reads from it; the pack never edits the file.

## What it does now

- `photoplot`: a strict reader for the current revision (2026.05) of the photoplot format's
  specification. Unit and format statements, standard and macro apertures (macros evaluated for
  each aperture's values and checked), draws, arcs, flashes, regions, step and repeat, block
  apertures, and file, aperture and object attributes. The model keeps every coordinate as an
  integer in the file's own units and every decimal exactly. A file that breaks a rule is refused
  with a typed code, the section it enforces and the byte offset; deprecated constructs read only
  as the specification's deprecated section describes them, each with a typed warning; a command
  the specification doesn't define is warned and skipped, as the specification requires of a
  reader, and the file is reported as not conforming. Nothing is guessed: a missing format or
  unit statement is refused.
- `drill`: a strict reader for the XNC subset (revision 2021.11): header, unit, tools, drill hits,
  rout paths with lines and arcs, and the attributes XNC carries in standard comments. A line
  outside the subset is refused with its kind, never read in another dialect's meaning.
- `drill::common`: a separate reading that reads common drill exports, the drill files CAD tools
  commonly write that XNC doesn't cover. It accepts only the constructs `docs/common-drill.md`
  documents, each learned from permissively licensed sample files; every file it reads is
  reported not conforming, with the constructs it relied on; anything ambiguous is refused, above
  all a coordinate without a decimal point when the header doesn't fix the number format.
  `CommonDrillPack` reads a drill file strictly first and uses it only when the strict reader
  refuses the file.
- `pack`: each reader as the kit's tier-0 blob pack with a structure index: families of
  apertures, macros, objects grouped by aperture and by attribute value, regions, step and
  repeats, tools and their hits and rout paths, and file attributes, with counts and point
  bounds. Apertures and tools are keyed by their definitions (an aperture's template,
  parameters and function; a tool's diameter and function), never by the numbers an exporter
  reissues, so a fresh export that renumbers them keeps every key.
- Resource limits (file size, objects, expanded objects, points, apertures, macros, expression
  depth, macro work, block nesting, attribute states) are declined with typed codes.
- `fuzz/` holds cargo-fuzz targets for both readers: test tooling, never shipped.

Specification sections are cited by number only; this repo holds no text from either
specification.

## The kit

The pack takes the kit by path: `crates/fab/Cargo.toml` names `../../../k2-mpc-base/crates/base`,
so a checkout of the kit sits beside this repo's, at the kit release `CHANGELOG.md` names. A host
vendors both side by side: the kit at `vendor/k2-mpc-base`, this pack at `vendor/k2-mpc-fab`.
`Cargo.lock` is committed. A public reader gets the kit from K2's repository,
https://github.com/Alakazam-211/K2, where it is vendored at `vendor/k2-mpc-base`: check that
folder out beside this repo as `k2-mpc-base`. Kit v0.24.0, the release this pack pairs with, is
vendored there from the K2 release that vendors kit v0.24.0.

Build and test: `scripts/check.sh`. One of its steps, `scripts/check-private.sh`, **fails without
private patterns** (exit 2, fail closed): it looks for the names of a host's own build machines,
which are never in this repo, so the patterns live outside it. Give them one per line (extended
regular expressions) in `MPC_PRIVATE_PATTERNS`, in a file named by `MPC_PRIVATE_PATTERNS_FILE`, or
in `~/.config/k2-mpc/private-patterns.txt`; a pattern that matches nothing (say
`MPC_PRIVATE_PATTERNS='a^'`) runs the rest.

The public corpus test reads files this repo doesn't hold, so `scripts/check.sh` leaves it out
(it is `#[ignore]`d, and fails when run without `FAB_PUBLIC_CORPUS`). `scripts/public-corpus.sh
[DIR]` fetches the files by reference (`scripts/fetch-corpus.sh`; never committed, each listed
with its licence and checksum in `crates/fab/tests/corpus/manifest.tsv`) and runs that test by
name.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions. Third-party material keeps its
own licence (see `NOTICE.md`).
