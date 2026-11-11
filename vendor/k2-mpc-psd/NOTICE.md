# Notices

This repo's own code is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the
README).

`vendor/photocraft-psd/` is a copy of the PSD crate of PhotoCraft, MIT OR Apache-2.0, with the small
changes listed in its `VENDORED.md`. Its own licence texts and NOTICE are beside it:

- `vendor/photocraft-psd/LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`.

Other Rust dependencies keep their own licences: all permissive, checked transitively by
`scripts/check-licences.sh` (`Cargo.lock` here; on the main branch, the fuzz crate's separately,
with libFuzzer's NCSA allowed there alone, because it is test tooling and never shipped).

This project is not affiliated with, endorsed by or sponsored by Adobe. PSD and PSB are file
formats; "Photoshop" is a trademark of its owner, named in comments and strings only to describe
the file format's history.
