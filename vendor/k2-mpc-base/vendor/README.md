# vendor/

Copies of other repos at a tag. `mpce-model` is **test-only** (only `k2-mpc-conformance`
depends on it). `yrs` is **the kit's engine** (v0.9.3): `crates/base` depends on it by path,
so it ships with the kit.
`vendor/` is excluded from the cargo workspace, so the kit's lints, formatting and tests
don't run on it; format the kit with `cargo fmt -p k2-mpc-base -p k2-mpc-conformance`
(`cargo fmt --all` would also reformat these copies, which keep their upstream style).

## mpce-model

The reference model for the engine's primitives (text today: a Fugue tree with FugueMax's
right-sibling rule; byte-offset `edit_as`, `diff_events` / `deliver_all`, blame and anchors;
versions, restore, branches, purge and span IDs; Def. 4 and strong-list-spec checkers). Std only, no dependencies. It is
the oracle the conformance suite (`crates/conformance`) runs every rule on, with an empty
deviation list. The only crate that depends on it is `k2-mpc-conformance`.

- From: the engine repo (`k2-mpc-engine`), prefix `crates/mpce-model`.
- Tag: **`mpce-model-v0.4.1`** (commit `da10b8e`); subtree split `5c897f4`.
  Earlier: v0.3.0 (`d8a7974`, split `a9b2d21`), v0.2.1 (`4528429`, split `3fa204a`),
  v0.2.0 (`15d19da`, split `d5865c4`), v0.1.1 (`80789b6`, split `7a2bc23`). v0.2.2
  (`40ba770`, tests only) and v0.4.0 (the tree model, not used by the kit yet) were
  skipped.
- Its `CHANGELOG.md` is the upstream one.

Bump to a new tag (do not edit files here by hand):

```sh
# in a clone of the engine repo
git subtree split --prefix crates/mpce-model mpce-model-vX.Y.Z   # prints <split sha>
# in this repo
git subtree pull --prefix vendor/mpce-model <engine clone> <split sha> --squash \
  -m "vendor: mpce-model at tag mpce-model-vX.Y.Z (<commit>)"
```

Then update the tag and shas above, and run `cargo test -p k2-mpc-conformance`.

## yrs

The kit's engine: Yrs v0.28.0 (MIT, https://github.com/y-crdt/y-crdt) plus the k2 patches,
from the private repo `k2-yrs`.

- Tag: **`k2-yrs-0.28.0-k2.6.3`** (commit `8fa90bd`; v0.9.7): what it changes is listed in
  `docs/YRS-FORK.md` and `yrs/NOTICE`. Before it: `k2-yrs-0.28.0-k2.2` (commit `44922a9`,
  v0.9.3); earlier tags were never vendored (the kit used crates.io's 0.28.0).
- Copied by `tools/vendor-yrs.sh` (not by subtree): the tag's `yrs/` crate, its `LICENSE` and
  `NOTICE`, without the engine's own tests (`yrs/tests/`: they run in k2-yrs); `VENDORED` records the tag, commit and
  hashes, and `--check` verifies the copy. `docs/YRS-FORK.md` says how to rebase on a new Yrs
  release.
- Upstream's own compile warnings show in the kit's builds (a path dependency's lints aren't
  capped); they are not the kit's and fail nothing.
