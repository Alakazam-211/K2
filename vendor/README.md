# vendor/

Copies of other repos, brought in by `git subtree --squash` at a tag. **Test-only:** nothing
here is shipped, and no crate a host depends on (`crates/base`) depends on anything here.
`vendor/` is excluded from the cargo workspace, so the kit's lints, formatting and tests
don't run on it; format the kit with `cargo fmt -p k2-mpc-base -p k2-mpc-conformance`
(`cargo fmt --all` would also reformat these copies, which keep their upstream style).

## mpce-model

The reference model for the engine's primitives (text today: a Fugue tree with FugueMax's
right-sibling rule; byte-offset `edit_as`, `diff_events` / `deliver_all`, blame and anchors;
versions, restore, branches and purge; Def. 4 and strong-list-spec checkers). Std only, no dependencies. It is
the oracle the conformance suite (`crates/conformance`) runs every rule on, with an empty
deviation list. The only crate that depends on it is `k2-mpc-conformance`.

- From: the engine repo (`k2-mpc-engine`), prefix `crates/mpce-model`.
- Tag: **`mpce-model-v0.3.0`** (commit `d8a7974`); subtree split `a9b2d21`.
  Earlier: v0.2.1 (`4528429`, split `3fa204a`), v0.2.0 (`15d19da`, split `d5865c4`), v0.1.1
  (`80789b6`, split `7a2bc23`). v0.2.2 (`40ba770`, tests only) was skipped.
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
