# Changelog

## v0.1.8: pairs with kit v0.19.0

**Requires `k2-mpc-base` v0.19.0**, checked out beside this repo. The crate version is `0.1.8`;
`Cargo.lock` records k2-mpc-base 0.19.0. This pack's own code's behaviour is unchanged from
v0.1.7. As for v0.1.7, the tag is a cut commit that leaves out the test tooling (`fuzz/`,
`scripts/oracle/`).

- Pairs with kit v0.19.0. The kit release adds chunked tier-0 versions, off by default: a host
  that turns them on stores a large PSD or PSB file's versions as chunks (this pack gives no cut
  hints). A blob doc's node claims end as before, and the pack declares no view schema, so PSD
  and PSB files behave as in v0.1.7.
- Public text: `scripts/check-labels.sh` also blocks parity-row, finding and run labels, branch
  names and engine codes. The v0.1.7 entry's long line is wrapped.
- `crates/psd/Cargo.toml`'s kit comment names v0.19.0; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.19.0 and the crate version 0.1.8.

## v0.1.7 (2026-11-12): pairs with kit v0.18.0

**Requires `k2-mpc-base` v0.18.0**, checked out beside this repo. The crate version is `0.1.7`;
`Cargo.lock` records k2-mpc-base 0.18.0 (only the kit's crate version moves). This pack's own
code's behaviour is unchanged from v0.1.6. As for v0.1.6, the tag is a cut commit that leaves out
the test tooling (`fuzz/`, `scripts/oracle/`).

- Pairs with kit v0.18.0. Its changes for a pack: build the view index's structs with
  `..X::default()` (and the schema's parts with `..X::NONE`); a keyed doc's `MPC-E822` is
  `SessionError::KeyedAtLimit`; an optional per-pack keyed limit (`Pack::keyed_entries_default`);
  verb state, derived values per unit and placements in 3D, all additive with defaults. This tier-0
  pack has no live doc, so PSD and PSB files behave as in v0.1.6.
- Public text in plain words: internal labels in docs, comments and test names are now plain
  descriptions, and `scripts/check-labels.sh` (run by `scripts/check.sh`) fails on one. The
  CHANGELOG's lines are wrapped to at most 100 characters, words unchanged.
- `crates/psd/Cargo.toml`'s kit comment names v0.18.0; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.18.0 and the crate version 0.1.7.

## v0.1.6 (2026-11-12): pairs with kit v0.17.0

**Requires `k2-mpc-base` v0.17.0**, checked out beside this repo. The crate version is `0.1.6`;
`Cargo.lock` records k2-mpc-base 0.17.0 (only the kit's crate version moves). This pack's own
code is unchanged from v0.1.5. As for v0.1.5, the tag is a cut commit that leaves out the test
tooling (`fuzz/`, `scripts/oracle/`).

- Pairs with kit v0.17.0. Its changes: text → tier 0 (`demote_text_file`, the host's call for a text
  doc whose type now routes to tier 0), the carrier's pairing (opt-in by `Pack::wants_pairing`, new
  error `MPC-E521`), the live grid (wire 3, additive), and an outside save on unwritten changes
  keeping both on a keyed doc that isn't a table. This tier-0 pack keeps no carrier and has no live
  doc, so PSD and PSB files behave as in v0.1.5. No new error id reaches it.
- `crates/psd/Cargo.toml`'s kit comment names v0.17.0; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.17.0 and the crate version 0.1.6.

## v0.1.5 (2026-11-12): pairs with kit v0.16.0

**Requires `k2-mpc-base` v0.16.0**, checked out beside this repo. The crate version is `0.1.5`;
`Cargo.lock` records k2-mpc-base 0.16.0 (only the kit's crate version moves). As for v0.1.4, the
tag is a cut commit that leaves out the test tooling (`fuzz/`, `scripts/oracle/`).

- Pairs with kit v0.16.0. Its changes are additive: the view schema, addresses and view index,
  the keyed watch (new frames behind the feature `keyed_watch`; wire still 3) with view deltas,
  roster addresses, and the structure index's row format 2. The pack hooks it adds
  (`Pack::view_schema`, `view_derive`, `address_of` / `key_of`, `watch_extras`) are provided
  methods with defaults, and this pack declares none of them. Its index reader hands no view
  items, so the kit writes its index rows in row format 1, byte for byte as before. No new
  error id.
- `crates/psd/Cargo.toml`'s kit comment names v0.16.0; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.16.0 and the crate version 0.1.5.
- **The vendored reader's report of lost layers follows its own rule for where the layers are.**
  `PsdFile::unreadable_layer_block` now looks only at the global block `from_bytes` reads the
  layers from: `Lr16` in a 16-bit file, `Lr32` in a 32-bit one, else the first `Layr`, `Lr16` or
  `Lr32`. Before, it looked at the first block with any of those keys. Two kinds of file were
  reported wrongly. A flat 32-bit file with a malformed `Lr16` block, which a 32-bit file never
  reads layers from, was reported as having lost its layers. A 16-bit file whose `Lr16` block
  was malformed, with a readable `Layr` block before it, was reported as having lost nothing.
  The pack's index was right both times: it reads the first file as flat and refuses the second
  as unreadable. Only the report was wrong, and nothing the pack ships calls it; this
  repository's index-against-full-reader comparison does, and the `index_draft` fuzz target
  found the difference. A marked local change (`vendor/photocraft-psd/VENDORED.md` has its row;
  `src/file.rs` says so on its first line).
- The test-only comparison of a records walk with the full reader now also checks that both say
  the same about lost layers, so the `records_diff` fuzz target holds the two together too.
- Hand-built regression rows: `a_block_the_depth_never_reads_loses_no_layers` and
  `a_readable_block_before_the_depths_own_hides_no_loss` (`tests/pack.rs`), and
  `a_layer_keyed_block_the_depth_never_reads_loses_nothing` (`tests/records.rs`).

## v0.1.4 (2026-11-12): pairs with kit v0.15.1

**Requires `k2-mpc-base` v0.15.1**, checked out beside this repo. The crate version is `0.1.4`;
`Cargo.lock` records k2-mpc-base 0.15.1 (only the kit's crate version moves). This pack's own
code is unchanged from v0.1.3. As for v0.1.3, the tag is a cut commit that leaves out the test
tooling (`fuzz/`, `scripts/oracle/`).

- Pairs with kit v0.15.1. Its changes are speed on keyed docs that aren't tables and the pack
  contract's carrier row for a pack that keeps a carrier; no kit API, wire or error id changed.
  This tier-0 pack keeps no carrier, so PSD and PSB files behave as in v0.1.3.
- `crates/psd/Cargo.toml`'s kit comment names v0.15.1; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.15.1 and the crate version 0.1.4.

## v0.1.3 (2026-11-11): pairs with kit v0.15.0

**Requires `k2-mpc-base` v0.15.0**, checked out beside this repo. The crate version is `0.1.3`;
`Cargo.lock` records k2-mpc-base 0.15.0 (only the kit's crate version moves). This pack's own
code is unchanged from v0.1.2. As for v0.1.2, the tag is a cut commit that leaves out the test
tooling (`fuzz/`, `scripts/oracle/`).

- Pairs with kit v0.15.0. Its additions are additive or provided methods with defaults (turns,
  tier 0's `BlobWriteOptions` among them, the pack verb on a change, `Pack::container_of`, the
  carrier hook, host cache keys); this pack overrides none of them, so PSD and PSB files behave
  as in v0.1.2. The kit's changes for keyed live docs don't reach this tier-0 pack.
- `crates/psd/Cargo.toml`'s kit comment names v0.15.0; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.15.0 and the crate version 0.1.3.

## v0.1.2 (2026-11-11): pairs with kit v0.14.0

**Requires `k2-mpc-base` v0.14.0**, checked out beside this repo. The crate version is `0.1.2`;
`Cargo.lock` records k2-mpc-base 0.14.0 (only the kit's crate version moves). This pack's own
code is unchanged from v0.1.1. As for v0.1.1, the tag is a cut commit that leaves out the test
tooling (`fuzz/`, `scripts/oracle/`).

- **The vendored reader, from upstream PhotoCraft commit `a7301f1d`:** a descriptor's
  `UnitFloats` (`UnFl`) count reserves at most 4,096 entries up front (`vec_for_count`), as
  the descriptor's other counts already do. Only that change of the commit is taken
  (`vendor/photocraft-psd/VENDORED.md` has its row; `src/descriptor.rs` says so on its first
  line). This copy already bounded that count by the bytes left in the record, so files read
  as before.
- Pairs with kit v0.14.0. Its additions are provided methods with defaults
  (`Pack::check_merged`, `Host::conflict_copy_name`) and person-writer reuse, off by default;
  this pack overrides none of them. The kit's tier-0 change (an app's save over an agent's
  write keeps both, with a conflict copy) applies to PSD and PSB files with no change here.
- `scripts/private-allow.txt`'s header says what `scripts/check-private.sh` skips (only the
  allowlist itself). v0.1.1's pairing line names the kit tag alone.
- `crates/psd/Cargo.toml`'s kit comment names v0.14.0; `Cargo.lock` and `fuzz/Cargo.lock`:
  k2-mpc-base 0.14.0 and the crate version 0.1.2.

## v0.1.1 (2026-11-10): pairs with kit v0.13.1

**Requires `k2-mpc-base` v0.13.1**, checked out beside this
repo. The crate version is `0.1.1`; `Cargo.lock` records k2-mpc-base 0.13.1 (only the kit's crate
version moves). No kit API change since v0.13.0, and this pack's code is
unchanged from v0.1.0. As for v0.1.0, the tag is a cut commit that leaves out the fuzz crate.

- `scripts/check-private.sh` reads its patterns from outside the tree (`MPC_PRIVATE_PATTERNS`,
  `MPC_PRIVATE_PATTERNS_FILE`, or `~/.config/k2-mpc/private-patterns.txt`) and fails closed
  without them, so the tree names no host.
- The README is written for readers outside the project: what the kit, a pack and MPC are, how
  the kit is found, an example (`examples/layers.rs`), the Rust version and the size limits.
- The text that ships reads the same with or without the test tooling (`fuzz/`,
  `scripts/oracle/`), which a release tag and a vendored copy leave out.

## v0.1.0 (2026-11-10): pairs with kit v0.13.0

The pack's first release. **Requires `k2-mpc-base` v0.13.0**, checked out beside this repo. The
crate version is `0.1.0`; `Cargo.lock` records k2-mpc-base 0.13.0.

**The tag's tree leaves out the fuzz crate:** the tag is a cut commit on the release commit that
removes it and the script lines that read it. The repository's main branch keeps it.

The first parts of the pack: the vendored parser, the streaming reader, and the index.

- `PsdPack`: the kit's `BlobPack` for PSD and PSB files (the kit, `k2-mpc-base`, by path at a
  pinned commit). Its index reads a version into the kit's structure index as the walker passes:
  every layer record but a group's closing marker is a node, parent first, siblings top to bottom;
  kinds `group`, `artboard`, `raster`, `text`, `smart`, `adjustment`, `fill`, `shape`, `other`;
  keys from the file's own layer IDs (`@lyid:N`), left to the kit to mint where an ID is missing
  or repeated; the attributes and flags of the kit's generic node (opacity, fill opacity, blend,
  clipping, locks, the adjustment or fill block, a smart object's placed ID and whether its file is
  embedded or linked; masks, styles, cut text or names, unreadable blocks); plain text of text
  layers; raw record and content digests through the kit's `Digest`, the content hashed piece by
  piece, never held. Agents can't write the file (`AgentWrites::Refused`).
- The walker passes every global tagged block but the layers' to the sink, so the index reads the
  linked-file items it needs without holding the rest.
- Tests: hand-built and generated files against the full reader's own layer tree; the kit's job,
  minted keys and agent refusal in a session; a generated multi-GB PSB indexed in a small fixed
  heap; the public corpus against the full reader and against psd-tools (a separate process, test
  only). On the main branch, a fuzz target on the index entry point.
- Anchors (the kit's anchors and comment threads): the pack's `judge_anchor` and `anchor_candidate`
  (`k2_mpc_psd::anchor`; `PsdPack` is also the kit's `AnchorRules`). Kinds come in families, group
  and artboard, fill and shape: a change within one keeps the layer, any other kind change (raster
  to smart, text to raster) is `KindChanged`. Within a family the kit's identity rule: in doubt only
  when renamed, moved past the old diagonal and repainted at once. `placed_id` is a weak ID: it
  never vouches for a layer, and only ever suggests a candidate of the same family. A hint is never
  judged off. Spec rows on hand-built layers, on every native node of the public corpus, and
  through a session's comment threads.
- The psd-tools cross-check (test tooling, on the main branch) installs from a hashed lock (no
  extras, `--require-hashes`) into a git-ignored venv, and its pins are checked against the public
  advisory data.

- `vendor/photocraft-psd`: the PSD parser, vendored from PhotoCraft at a fixed commit. `VENDORED.md`
  lists what was left out and every local change; each changed file carries a line saying so.
- The records-only reader (`walk_records`, added to the vendored crate as `records.rs`): reads the
  layer records of a PSD or PSB from any `Read`, passing channel bytes to a sink as they go and
  holding none of them; a multi-GB file fits in a fixed buffer plus its records.
- Differential tests against the vendored full reader, on generated files, hand-built files and the
  public corpus (fetched by reference); memory measured on a generated multi-GB PSB.
- On the main branch, cargo-fuzz targets on the reader.
