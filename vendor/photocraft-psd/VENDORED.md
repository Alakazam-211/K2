# photocraft-psd, vendored

A copy of the `crates/psd` crate of PhotoCraft, version 0.9.0.

- **Source:** https://github.com/storytold/photocraft
- **Commit:** `fd28da56dae573df141edc25059e62788eef8304`
- **Licence:** MIT OR Apache-2.0. `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` here are upstream's;
  `crates/psd` has none of its own, so they come from the repository's root. `NOTICE` keeps the
  copyright and licence lines; the fonts, icons, word list and brand lines cover material that is
  not in this crate, so they are left out.

## Left out

- `crates/psd/README.md`, and `crates/psd/fuzz/` (its `libfuzzer-sys` is NCSA and compiles C++;
  the fuzz targets on the reader are this repository's own, on its main branch).
- No test data: `tests/` and `proptest-regressions/` are source and generated seeds only.
- The rest of the PhotoCraft repository.

## Local changes (each changed file says so on its first line)

| File | Change |
|---|---|
| `Cargo.toml` | Workspace inheritance replaced with the same values; `publish = false`; the description names only the format. |
| `src/lib.rs` | `pub mod records;` added; the module docs no longer name the product or the provenance claim. |
| `src/abr.rs`, `src/atn.rs` | The provenance claim removed from the module docs. |
| `src/atn.rs` | The module docs link to `write()` rather than `write`, so the link names the function (the plain link was ambiguous). |
| `src/layer.rs` | `LayerRecord::read` is `pub(crate)`, for `records.rs`. |
| `src/tagged.rs` | `is_sig` is `pub(crate)`, for `records.rs`. |
| `src/descriptor.rs` | From upstream commit `a7301f1db89b2f24b0fb6d1334b424da898faf0b`, newer than the one above, only its `UnFl` change: a descriptor's `UnitFloats` count reserves at most 4,096 entries up front (`vec_for_count`), as the other counts already do, so a large declared count can't abort on allocation. The rest of that commit is not taken. |
| `src/records.rs` | New file, ours: the records-only streaming walker. Its sink also receives every global tagged block other than the layers' (begin, bytes, end), for a reader that wants a few of them. |

`records.rs` copies one rule rather than sharing it, to keep upstream's function untouched: the
part of `read_blocks_with`'s padding detection that decides where the next block starts. The
differential tests (and, on the main branch, a fuzz target) hold the two together.

`rustfmt.toml` and `clippy.toml` here are upstream's, so this copy keeps upstream's formatting.

## Updating

Replace the tree from a new commit, keep this file and `src/records.rs`, reapply the changes above,
and rerun the licence gate at the new versions.
