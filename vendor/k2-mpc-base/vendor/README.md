# vendor/

Copies of other repos. `yrs` is **the kit's engine**: `crates/base` depends on it by path, so it
ships with the kit. `mpce-model`, with `mpce-order` and `mpce-reg`, is a reference model of the
native engine's primitives; no crate in this tree depends on it. `vendor/` is excluded from the
cargo workspace, so the kit's lints, formatting and tests don't run on it, and these copies keep
their upstream style.

## mpce-model, mpce-order, mpce-reg

`mpce-model` is a reference model of the native engine's primitives: text (a Fugue tree with
FugueMax's right-sibling rule; byte-offset edits, events, blame and anchors; versions, restore,
branches, purge and span IDs), movable trees, lists, movable lists and maps, with checkers for its
own rules. Std only, no unsafe code.

It composes two layers, each its own crate with no dependencies, vendored beside it so its path
dependencies (`../mpce-order`, `../mpce-reg`) resolve: `mpce-order` (the slot order: FugueMax
over opaque slot IDs) and `mpce-reg` (per-element registers). The two never depend on each other.

- From: the engine repo, prefixes `crates/mpce-model`, `crates/mpce-order` and
  `crates/mpce-reg`, all three at one commit: **`b732aff0b6ac4b94068e6d7d1bee29b12d7babb0`** (no
  tag; `mpce-model` 0.4.1, `mpce-order` 0.1.0, `mpce-reg` 0.1.0). Each directory here is byte for
  byte that crate's directory at the commit, copied by `git subtree`, never edited by hand.
- Licence: MIT OR Apache-2.0 (each manifest's `license`). The texts are the engine repo's root
  `LICENSE-MIT` and `LICENSE-APACHE` at that commit, byte for byte this repo's own root
  `LICENSE-MIT` and `LICENSE-APACHE`.
- `mpce-model`'s `CHANGELOG.md` is the upstream one.

## yrs

The kit's engine: Yrs v0.28.0 (MIT, https://github.com/y-crdt/y-crdt) plus the k2 patches,
from the private repo `k2-yrs`.

- Tag: **`k2-yrs-0.28.0-k2.6.5`** (commit `b37f49b`; v0.9.8): what it changes is listed in
  `docs/YRS-FORK.md` and `yrs/NOTICE`. Before it: `k2-yrs-0.28.0-k2.6.4` (commit `76c73df`),
  `k2-yrs-0.28.0-k2.6.3` (commit `8fa90bd`, v0.9.7) and `k2-yrs-0.28.0-k2.2` (commit `44922a9`,
  v0.9.3); earlier tags were never vendored (the kit used crates.io's 0.28.0).
- Copied by `tools/vendor-yrs.sh` (not by subtree): the tag's `yrs/` crate, its `LICENSE` and
  `NOTICE`, without the engine's own tests (`yrs/tests/`: they run in k2-yrs); `VENDORED` records the tag, commit and
  hashes, and `--check` verifies the copy. `docs/YRS-FORK.md` says how to rebase on a new Yrs
  release.
- Upstream's own compile warnings show in the kit's builds (a path dependency's lints aren't
  capped); they are not the kit's and fail nothing.
