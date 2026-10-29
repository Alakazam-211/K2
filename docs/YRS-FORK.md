# The Yrs fork (k2-yrs) and its copy in `vendor/yrs`

The kit's engine is **Yrs v0.28.0 plus the k2 patches**, from the private repo `k2-yrs` (owned
and reviewed by the engine's maintainers), vendored at a tag into `vendor/yrs`. The kit depends
on it by path; nothing else in the kit sees an engine type (ARCHITECTURE rule 1).

## What the fork changes (tag `k2-yrs-0.28.0-k2.2.4`)

| Patch | Why | Tests (in k2-yrs) |
|---|---|---|
| Decoders bound every pre-allocation sized by a decoded count by what the remaining input could hold (`k2.1`) | hardening: what decoding allocates follows the input's size | `yrs/tests/decode_prealloc.rs` (not vendored; `decode_bombs.rs` before k2.2.3) |
| `Doc::transact_mut_as(client)` (`k2.2`) | the kit makes each participant's edit on the doc itself, as the participant's client (ARCHITECTURE rule 6), instead of on a throwaway replica of the whole doc (O(history) per edit) | `yrs/tests/transact_mut_as.rs`, the undo manager's unit tests |
| `Update::has_subdoc_content()` (`k2.2.1`) | hardening, a decode-only check: the kit refuses client updates carrying content it never uses (subdocuments) before applying anything, and quarantines a stored doc holding such content (v0.9.5) | `yrs/tests/subdoc_content.rs` |
| Hardening (`k2.2.2`) | engine-side hardening for content the kit never uses; the kit keeps refusing that content at ingress | in k2-yrs |
| Warning-free (`k2.2.3`) | hardening; the engine builds warning-free under `RUSTFLAGS='-D warnings'` (default features and `weak`), with no change in behaviour | the whole suite |
| NOTICE wording (`k2.2.4`) | hardening entries in the fork's NOTICE; no other change | – |

`transact_mut_as` swaps the store's client ID for the transaction's life; new blocks and redone
items take that client's ID and next clocks. The doc's own ID is restored at commit, before
subdocs added in the transaction are given an ID (so they share the doc's ID, as in any
transaction), and on a panic inside commit before that point (caught, restored, resumed).

**Known limitation** (upstream behaviour, recorded at K2 Engine's request for the next tag's
doc comment): a panic in the caller's scope makes `Drop` commit the transaction while
unwinding; if that commit then panics too (an observer, say), Rust aborts the process. The
restore-path test covers a panic in commit only when not already unwinding. The kit registers
no observers on its docs.

**What the kit relies on:** the doc holds everything a client ever wrote, and no one else
writes as it (`session/one_writer_tests.rs`); the edit is made under the doc's exclusive
borrow (a session's lock). The differential test (`tests/edit_paths.rs`) and the runtime
canary (`Config::edit_check_every`) compare it with the throwaway-replica path byte for byte.

**The property test's scope** (k2-yrs, 40,000 random cases: node jobs `d7a9928a`, `b58db970`):
about 571k plain ops byte-identical with equal state vectors; formatting ops compared on blocks
and the delete set only, because upstream orders format items through a randomly seeded
HashMap. The kit uses plain text only.

## The copy in `vendor/yrs`

`tools/vendor-yrs.sh <k2-yrs clone> <tag>` writes it; `--check` rebuilds it and compares. It
holds the tag's `yrs/` crate, the repo's `LICENSE` (Yrs's MIT licence) and `NOTICE` (what the
fork changed), and `VENDORED`: the tag, its commit, the tree hash of its `yrs/`, the left-out
folder (`yrs/tests/`, the engine's own tests: they run in k2-yrs), the upstream tag and crate
(`yrs-0.28.0` on crates.io and its sha256), `K2.patch` (the fork's diff of `src/` from the
upstream tag), and sha256s of `src/` and of every file. Never edit files there by hand.

**Compute jobs** carry `vendor/yrs` without `src/` (the kit plus yrs is over a job's bundle):
`tools/test-bundle.sh` has the job download the public crate, check its sha256, apply
`K2.patch`, and check the result against `src-sha256` before anything builds.

## Rebasing on a new Yrs release

1. In k2-yrs: branch from the new upstream tag, cherry-pick the k2 patches (one commit each:
   `git log --oneline v0.28.0..k2-yrs-0.28.0-k2.2`), resolve, and run the whole suite plus the
   property test at 10^4 cases or more (`YRS_AS_CASES`) on a node. The engine's maintainers
   review, merge and tag `k2-yrs-<version>-k2.N`.
2. Check the release's stored format against the kit's: the kit stamps each doc with the engine
   and format (`ENGINE_NAME`, `ENGINE_FORMAT`); a new encoding needs a re-encode on upgrade.
3. In `tools/vendor-yrs.sh`, set `UPSTREAM_TAG`, `CRATE` and `CRATE_SHA256` (the crate's
   checksum from a `Cargo.lock` that resolves it from crates.io), and check the crate's `src/`
   equals the upstream tag's `yrs/src` (it did for 0.28.0).
4. `tools/vendor-yrs.sh <k2-yrs clone> <new tag>`, then `--check`; update the version pin in
   `crates/base/Cargo.toml`; run the kit's whole suite and the differential test on nodes
   (`tools/tests/full.sh`, `tests/edit_paths.rs` at `EDIT_PATHS_SEEDS=1000`), and the soak.
5. Carry `vendor/yrs/NOTICE` into `NOTICE.md` if the list of changes grew; say what changed in
   the CHANGELOG.
