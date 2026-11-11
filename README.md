# k2-mpc-psd

Reads PSD and PSB files: the file-type pack for layered images in the multiplayer kit.

- **MPC** (multiplayer cores): files that people and agents work on together, with presence,
  attribution, history and comments.
- **The kit** (`k2-mpc-base`): the file-type-agnostic part of MPC. It keeps a file, its versions and
  who made them, and serves a file's structure for comments, diffs and blame. It knows no format.
- **A pack** (this repo): what one file type adds to the kit. This one reads layered images.

The kit keeps a PSD file as one whole blob; this pack reads each version of it into a tree of
layers beside it, so people and agents can comment on, diff and blame by layer. The pack only ever
reads files. It never writes one, and it refuses agents' writes: agents propose a new file and a
comment instead. (The vendored parser can write PSD files; the pack doesn't use that.)

- **`crates/psd`** (`k2-mpc-psd`): the pack. `PsdPack` is the kit's `BlobPack` for these files.
  It reads each version into the kit's structure index: a tree of layers with their kinds, keys
  from the file's own layer IDs, fingerprints and keyed hashes, streamed, never holding pixels.
  Its half of a comment anchor's state is in `anchor`: whether a layer is still the anchored one,
  and which layers to suggest when it's gone. Under it, the records-only reader streams a file of
  any size and holds only its layer records.
- **`vendor/photocraft-psd`**: the PSD parser it reads with, vendored (`VENDORED.md`).
- **Test tooling**, in this repository's main branch only: a release tag and a vendored copy
  leave it out, and nothing here needs it. `fuzz/` holds cargo-fuzz targets for the reader and the
  index. `scripts/oracle/` is a test checker, never shipped: psd-tools, run as a separate Python
  process from a hashed lock in a git-ignored venv, prints each public corpus file's layer tree for
  `tests/pack_corpus.rs` to compare; no psd-tools code is in the pack, and it runs on the public
  corpus only.

## Using it

```rust
use std::fs::File;

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_base::session::BlobPack;
use k2_mpc_psd::PsdPack;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: layers FILE.psd")?;
    let mut file = File::open(&path)?;
    let size = file.metadata()?.len();
    // the bounds a host's kit config sets: most nodes, deepest nesting, stored bytes, text bytes
    let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let draft = PsdPack.index(&mut file, size, &limits)?;
    for node in &draft.nodes {
        let key = node.key.as_deref().unwrap_or("(the kit mints one)");
        println!("{key} {} {}", node.kind, node.name);
    }
    Ok(())
}
```

This is `crates/psd/examples/layers.rs`: `cargo run -p k2-mpc-psd --example layers -- FILE.psd`. A
host doesn't call `index` itself: it opens the file with `SessionManager::open_blob(Arc::new(PsdPack),
…)` and runs the index jobs the kit hands it.

## The kit

The pack takes the kit by path: `crates/psd/Cargo.toml` names `../../../k2-mpc-base/crates/base`,
so a checkout of the kit sits beside this repo's, at the kit release `CHANGELOG.md` names. A host
vendors both side by side: the kit at `vendor/k2-mpc-base`, this pack at `vendor/k2-mpc-psd`.
`Cargo.lock` is committed.

## Requirements and limits

- Rust 1.95 or newer (the vendored parser is edition 2024; this crate is edition 2021).
- Any file size reads: pixels pass through a fixed 64 KiB buffer and are hashed, never held. The
  reader holds at most 64 MiB in one layer record and 256 MiB in all of them, and reads at most
  32,768 layers; past those a file is refused as unreadable.
- What the kit indexes is the host's to bound (the kit's `Config`): by default files up to 2 GiB,
  100,000 layers, and 64 MiB of stored index. Past a bound a version stays a plain blob: its
  versions, presence and comments on the whole file still work.

Build and test: `scripts/check.sh`.

Status: the reader, the index and the anchor rules. The raster export comes next.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`) or the MIT licence
(`LICENSE-MIT`), at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work, as defined in the Apache-2.0 licence, is
dual-licensed as above, without any additional terms or conditions. Third-party code keeps its own
licence (see `NOTICE.md`).
