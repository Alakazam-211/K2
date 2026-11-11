# Changelog

## v0.1.1 (2026-11-10): pairs with kit v0.13.1

**Requires `k2-mpc-base` v0.13.1** (the tag at its public cut 6ef688e4), checked out beside this
repo. The crate version is `0.1.1`; `Cargo.lock` records k2-mpc-base 0.13.1 (only the kit's crate
version moves; no `mpce` entry). No kit API change since v0.13.0, and this pack's code is
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

The first slices of the pack: the vendored parser, the streaming reader, and the index.

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
