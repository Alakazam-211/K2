# The Yrs fork (k2-yrs) and its copy in `vendor/yrs`

The kit's engine is **Yrs v0.28.0 plus the k2 patches**, from the private repo `k2-yrs` (owned
and reviewed by the engine's maintainers), vendored at a tag into `vendor/yrs`. The kit depends
on it by path; nothing else in the kit sees an engine type (ARCHITECTURE rule 1).

## What the fork changes (tag `k2-yrs-0.28.0-k2.6.5`)

| Patch | Why | Tests (in k2-yrs) |
|---|---|---|
| Hardening (`k2.1`, `k2.2.2`, `k2.3`, `k2.4`–`k2.6`) | engine-side hardening; upstream behaviour the kit never relies on (the kit uses plain root texts only) | in k2-yrs |
| `Doc::transact_mut_as(client)` (`k2.2`) | the kit makes each participant's edit on the doc itself, as the participant's client (ARCHITECTURE rule 6), instead of on a throwaway replica of the whole doc (O(history) per edit) | `yrs/tests/transact_mut_as.rs`, the undo manager's unit tests |
| A position index for text (`k2.3`): up to 32 markers per text (recent positions, by ID, least recently used replaced), dropped when an item is integrated or deleted; plain texts only (a formatted text walks); a shadow check, one marker lookup in N (default 1,000), also walks and compares, and a mismatch uses the walk, drops the markers and calls the audit hook (`Doc::set_position_audit_hook`, `PositionMismatch`: IDs and indices only), with `Doc::set_position_shadow_rate` and `Doc::position_index_stats` | finding a text position walked every item from the text's start, deleted ones included: the per-edit cost grew with the history (v0.9.3: 4.4 ms an edit at 78k changes in the soak) | `src/position_index.rs`'s unit tests, `yrs/tests/position_index.rs` (the property test against the walk, and two planted bugs the test and the shadow must catch) |
| A transaction's work is proportional to the clients it touches (`k2.5`) | upstream built whole state vectors per transaction: the cost grew with every client the doc had seen | `yrs/tests/touched_clients.rs` |
| A decode-only API for refusing subdocument content (`k2.2.1`) | the kit refuses client updates carrying content it never uses (subdocuments) before applying anything, and quarantines a stored doc holding such content (v0.9.5) | in k2-yrs |
| Warning-free (`k2.6.1`) | the engine builds warning-free under `RUSTFLAGS='-D warnings'` (default features, `weak` and the test-only features), with no change in behaviour | the whole suite |
| Wording (`k2.6.2`, `k2.6.3`) | hardening entries in the fork's NOTICE and its comments; no code change | – |
| Deleted-block runs (`k2.6.4`): a walk that steps through two or more deleted blocks in a row records the run's ends (unit IDs, per text), and later walks go from one end to the other; a live block linked in between drops the text's runs; the position index's shadow check steps through what the runs cross (with a step budget) instead of the full walk; `Doc::set_deleted_runs` (test-only) keeps the old stepping | a text lookup or edit still stepped through every deleted block between a marker and its target, so the cost grew with the text's history (the soak's young docs: a replica or a doc just loaded, with no markers yet). The same places are found, so updates are byte-identical | `yrs/tests/deleted_runs.rs`, `src/position_index.rs`'s unit tests; bench `yrs/benches/text_soak.rs` |
| `ReadTxn::client_state(&ClientID)` (`k2.6.5`): one client's entry of the state vector (0 for a client the doc has never seen), one lookup | the kit reads one client's clock at least twice per edit (`clock_of`); building the whole state vector for it cost O(clients), and a doc's clients grow with its joins | `yrs/tests/client_state.rs` |

`transact_mut_as` swaps the store's client ID for the transaction's life; new blocks and redone
items take that client's ID and next clocks. The doc's own ID is restored at commit, before
subdocs added in the transaction are given an ID (so they share the doc's ID, as in any
transaction), and on a panic inside commit before that point (caught, restored, resumed).

**Known limitation** (upstream behaviour, recorded at K2 Engine's request for the next tag's
doc comment): a panic in the caller's scope makes `Drop` commit the transaction while
unwinding; if that commit then panics too (an observer, say), Rust aborts the process. The
restore-path test covers a panic in commit only when not already unwinding. The kit registers
no observers on its docs.

**The position index in the kit (v0.9.7):** every engine doc the kit makes points the audit
hook at a small queue (the hook runs inside a transaction, with the doc locked, and only
pushes); the session reports each entry as `AuditEvent::PositionIndexMismatch` (the root,
the index and the marker's index), and `Config::position_check_every` sets the shadow rate.
`tests/position_index.rs` plants both of the fork's test bugs (the `planted-position-bugs`
feature, test-only) and checks they are reported and never served. The fork's property test
(job `210b3c63`: 10,000 cases, 545,125 steps, 336,795 lookups, 106,997 of them from a marker,
each shadow-checked: 0 mismatches) compares updates, contents and block layouts with the walk
after every step.

**Upstream hazard, `TextRef::remove_range`:** with a length past the text's end it deletes
what it can, then panics, and the transaction commits as it unwinds (`remove_range(5, 50)` on
`abc-def` leaves `abc-d`). The kit never asks for that: its hunks come from a diff of the
text it holds, and a debug assertion at the two production call sites
(`engine/yrs.rs`) checks the range.

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
(`yrs-0.28.0` on crates.io and its sha256), `FORK.patch` (the fork's diff of `src/` from the
upstream tag), and sha256s of `src/` and of every file. Never edit files there by hand.

**Compute jobs** carry `vendor/yrs` without `src/` (the kit plus yrs is over a job's bundle):
`tools/test-bundle.sh` has the job download the public crate, check its sha256, apply
`FORK.patch`, and check the result against `src-sha256` before anything builds.

## Rebasing on a new Yrs release

1. In k2-yrs: branch from the new upstream tag, cherry-pick the k2 patches (one commit each:
   `git log --oneline v0.28.0..k2-yrs-0.28.0-k2.6.3`), resolve, and run the whole suite plus the
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
