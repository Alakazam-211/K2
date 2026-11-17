# k2-mpc-timeline

The timeline pack for the multiplayer kit: a video or audio timeline that people and agents work
on at once. OTIO, FCPXML and PRPROJ files are read in; OTIO is also written back.

- **MPC** (multiplayer cores): files that people and agents work on together, with presence,
  attribution, history and comments.
- **The kit** (`k2-mpc-base`): the file-type-agnostic part of MPC. It keeps a file, its versions and
  who made them, and serves a file's structure for comments, diffs and blame. It knows no format.
- **A pack** (this repo): what one file type adds to the kit. This one handles timelines.
- **A host**: the program that runs the kit and its packs for people and agents. **K2** is one:
  it vendors the kit and its packs into its own repository.

## What it does now

The read side: exact time (`time`), the neutral timeline model (`model`), and readers that
read OTIO, FCPXML and PRPROJ files into it (`otio`, `fcpxml`, `prproj`). Each reader is also
the kit's blob pack for its files (`pack`): the kit keeps each version of the file, and the
pack indexes it into a tree of sequences, tracks, clips and markers for comments, diffs and
blame. `fuzz/` holds cargo-fuzz targets for the readers: test tooling, never shipped.

OTIO timelines also go live (`live`), off by default: one timeline as a live doc on the kit's
structured mode, its tracks as lanes and its items as bars, agents' verbs on it (move, trim,
rename, enable, delete and add clips; markers; track names), and a write-back that splices only
what changed into the file, every other byte kept. FCPXML and PRPROJ files are read and indexed
only: nothing writes them.

## The kit

The pack takes the kit by path: `crates/timeline/Cargo.toml` names
`../../../k2-mpc-base/crates/base`, so a checkout of the kit sits beside this repo's, at the kit
release `CHANGELOG.md` names. A host vendors both side by side: the kit at `vendor/k2-mpc-base`,
this pack at `vendor/k2-mpc-timeline`. `Cargo.lock` is committed. A public reader gets the kit
from K2's repository, https://github.com/Alakazam-211/K2, where it is vendored at
`vendor/k2-mpc-base`: check that folder out beside this repo as `k2-mpc-base`. Kit v0.22.0, the
release this pack pairs with, is vendored there from the K2 release that vendors kit v0.22.0.

Build and test: `scripts/check.sh`. One of its steps, `scripts/check-private.sh`, **fails without
private patterns** (exit 2, fail closed): it looks for the names of a host's own build machines,
which are never in this repo, so the patterns live outside it. Give them one per line (extended
regular expressions) in `MPC_PRIVATE_PATTERNS`, in a file named by `MPC_PRIVATE_PATTERNS_FILE`, or
in `~/.config/k2-mpc/private-patterns.txt`; a pattern that matches nothing (say
`MPC_PRIVATE_PATTERNS='a^'`) runs the rest.

The public corpus tests need the files first, so the gate leaves them out (`#[ignore]`d; each
fails without `TIMELINE_PUBLIC_CORPUS`):
`scripts/public-corpus.sh [DIR]` fetches them (`scripts/fetch-corpus.sh`) and runs the tests, each
by name. `python3 -I scripts/corpus-expected.py DIR` reproduces
`crates/timeline/tests/corpus/expected.tsv`, an independent count used as a test checker.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions. Third-party code keeps its own
licence (see `NOTICE.md`).
