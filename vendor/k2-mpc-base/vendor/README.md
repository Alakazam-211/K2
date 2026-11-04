# vendor/

Copies of other repos at a tag. `mpce-model`, and `mpce-sync` with `mpce-core` and
`mpce-codec`, are **test-only** (only `k2-mpc-conformance` depends on them; the kit's own build
never sees them: `cargo tree -p k2-mpc-base -e normal` has no `mpce-*` crate). `yrs` is **the
kit's engine** (v0.9.3): `crates/base` depends on it by path, so it ships with the kit.
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

## mpce-sync, mpce-core, mpce-codec

The native engine's sync layer (`mpce-sync`: the stability-frontier tracker and the send log,
E-SEC-16, -17, -18, E-CLI-14), with the two crates it depends on: `mpce-core` (its refusal
type, `Refusal::FalseReceipt` = `MPCE-E185`) and `mpce-codec` (mpce-core's decoder). The
conformance suite runs `sec.false_receipt` and `sec.rebase_required` on it
(`crates/conformance/src/sync.rs`); its tests turn on mpce-sync's `planted` feature (a
dev-dependency) for the planted tracker mutants. Nothing else depends on them.

- From: the engine repo (`k2-mpc-engine`), prefixes `crates/mpce-sync`, `crates/mpce-core`,
  `crates/mpce-codec`; path dependencies among themselves (`../mpce-core`, `../mpce-codec`).
- Tag: **`mpce-g2a-m3.1`** (hardening), commit
  `2fa1f2a5a53a1fad4b586077f2c56ead08c5e3af`, for all three crates (one commit, so the set is
  consistent). Subtree splits: mpce-sync `fe10ddd` (`fe10ddd19d1f4b7f23929a2a552d350308a3852a`),
  mpce-core `6ea9cb3` (`6ea9cb344d423436b98d6693322cac804d085c33`), mpce-codec `5b74a92`
  (`5b74a9275484db8ad108fde6bb377a64209318ce`). The copies are byte-for-byte the tag's
  (checked against `git archive` of the tag). Against `mpce-g2a-m3`: mpce-core only. Hardening,
  with one new refusal code (listed in the kit's error table); no manifest changed, so the
  lockfile didn't. `mpce-model` keeps its own tag (above).
- Earlier: `mpce-g2a-m3` (commit `6101d66456b5d9de2897d08dab33675377bb934f`; mpce-core split
  `8f20e02`, mpce-codec split `5b74a92`: hardening and performance), on main at kit `3e5c7cb6`.
  `mpce-g2a-m2.4.7` (commit `26765a8c94fdd86fd23b65feedad4aae82fead26`; mpce-core split
  `92cf3e9`, mpce-codec split `af206a1`: hardening), on main at kit `55957c66`.
  `mpce-g2a-m2.4.6` (commit `2dd1cede4c14175f98aad7885db34beab0bf1eab`; mpce-core split
  `1c7651f`: test-only additions; mpce-codec split `bdb3379`), on main at kit `270598c5`.
  `mpce-g2a-m2.4.5` (commit `c184284953b7844458929a3cea4e83d22983bb93`; mpce-core split
  `a91c187`: hardening; the `planted-shortcuts` feature), on main at kit `085db90f`.
  `mpce-g2a-m2.4.4` (commit `40f7825665c273151fc801a4987ad6fc960f6afe`; mpce-core split
  `0c2070e`: hardening; the `verify-shortcuts` feature),
  vendored at kit `82900e12` and landing with m2.4.5. `mpce-g2a-m2.4.3` (commit
  `e68e1c22a4d1ca91204bb22166c7b61864010f7c`; mpce-core split `4ba497c`: hardening), on main at kit `8b9b1418`.
  `mpce-g2a-m2.4.2` (commit `be31995dd91b29955da7b65627c47e2bd7ffe84f`; mpce-core split
  `1df8617`: hardening). `mpce-g2a-m2.4.1` (commit `51577ad31b85e19e0f0ad7bacddec83cc10114b3`; mpce-core
  split `3d6dcba`), on main at kit `5ddc4aae`. m2.4.1 is `mpce-g2a-m2.4` (`45ad5b6`; mpce-core
  split `19cbb6f`) with comments changed only: with Rust comments stripped (token-aware), the
  three crates' code is identical to m2.4's. Against `mpce-g2a-m2.3`: mpce-core's sources
  only: hardening, with two new refusal codes (listed in the kit's error table); mpce-sync and mpce-codec are the
  same splits; no manifest changed, so the lockfile didn't.
- Earlier: `mpce-g2a-m2.3` (hardening; commit `c958fc46c424f337decc44150c79a20ec5af1f2b`;
  splits mpce-sync `fe10ddd`, mpce-core `fdee2d8`, mpce-codec `bdb3379`), on main at kit
  `14733897`.
- **Licence: MIT OR Apache-2.0** (the engine repo's, since this tag; each manifest says
  `license = "MIT OR Apache-2.0"`). The licence texts are at the engine repo's root, not in
  the crate directories, so a subtree of a crate doesn't carry them: they are copied
  byte-for-byte from the tag's root into **`vendor/mpce-licences/`** (`LICENSE-MIT`,
  `LICENSE-APACHE`), outside the three crate directories so those stay byte-identical to the
  tag. They cover the three crates (and are the engine repo's licence; `mpce-model`, vendored
  earlier at `mpce-model-v0.4.1`, has no `license` field yet).
- mpce-sync is 0.1.1 at this tag (first tagged `mpce-sync-v0.1.1`): the per-client re-base
  epoch (`Participant::ep`, `Tracker::on_rebase`, the fence in `Tracker::route_op`) and
  standalone manifests.
- Earlier: `mpce-g2a-m2.2` (hardening; commit
  `76cdaa70154059bd58cac84f886b12f88ffc6925`; splits mpce-sync `756464f`, mpce-core `5fde064`,
  mpce-codec `9d1349e`; it added `MPCE-E196`), on main at kit
  `540eef7`.
- Earlier: `mpce-sync-v0.1.1` (commit `2631272`; splits mpce-sync `756464f`, mpce-core
  `c0c8d0f`, mpce-codec `9d1349e`) and the hardening tag `mpce-g2a-m2.1` (commit
  `3dd5bbb86377a30ef130be186577e0a05dd8d595`; mpce-core split `4dbb4fa`), vendored on this
  branch first and replaced by `mpce-g2a-m2.2` before landing. `mpce-sync-v0.1.0` (commit `a156fef49e924b0a4d05dbfe35a2214ca16662d9`; splits
  mpce-sync `201b32a`, mpce-core `6576e69`, mpce-codec `089f4f6`), with the kit's stand-in
  `vendor/Cargo.toml` for the engine root's inherited fields, removed at 0.1.1.
- mpce-sync is 0.x and **unstable until the engine's GS freeze**: bump it only to a tag K2
  Engine names, and re-run the conformance part. At a bump, copy the engine root's
  `LICENSE-MIT` and `LICENSE-APACHE` from the tag into `vendor/mpce-licences/` again (`git
  archive <tag> LICENSE-MIT LICENSE-APACHE`) and compare.
- **Standalone manifests** (since 0.1.1): no workspace inheritance, so no stand-in root.
  `count-alloc` is not vendored: it is only a dev-dependency (mpce-codec's and mpce-core's,
  with mpce-core's `../mpce-model` and `../mpce-sync`), and cargo never resolves a
  dependency's dev-dependencies (the lockfile has no `count-alloc`); the kit doesn't run
  these crates' own tests.
- External dependencies (mpce-core's): `hmac` 0.12 and `sha2` 0.10 (RustCrypto, MIT OR
  Apache-2.0), with `digest`, `block-buffer`, `crypto-common`, `generic-array`, `typenum`,
  `subtle`, `cpufeatures`, `libc`, `version_check`: all on the allow-list
  (`scripts/check-licences.sh`), at the versions of the engine's lock at the tag.

Bump to a new tag (do not edit files here by hand):

```sh
# in a clone of the engine repo (<tag>: the engine tag K2 Engine names, e.g. mpce-g2a-m3.1)
for c in mpce-sync mpce-core mpce-codec; do
  git subtree split --prefix crates/$c <tag>                  # prints <split sha> per crate
done
# in this repo, per crate (git subtree mis-reads a repository path with spaces: give it a
# symlink to the clone without them)
git subtree pull --prefix vendor/<crate> <engine clone> <split sha> --squash \
  -m "vendor: <crate> at tag <tag> (<commit>)"
```

Then update the tag and shas above (and check the manifests are still standalone, and that
no new normal dependency needs vendoring), and run the `conformance` and `wire-guard` parts
and `scripts/check-licences.sh`.

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
