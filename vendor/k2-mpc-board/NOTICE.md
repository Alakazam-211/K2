# Third-party notices

This crate is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`; see the README).
The code it builds on keeps its own licence:

- `miniz_oxide` (MIT OR Zlib OR Apache-2.0), with `adler2` (0BSD OR MIT OR Apache-2.0);
- `sha2` (MIT OR Apache-2.0), with its RustCrypto dependencies (MIT OR Apache-2.0).

All dependencies are checked transitively by `scripts/check-licences.sh`.

Parts of the schema tables come from Board: see `THIRD-PARTY-NOTICES.md`.
