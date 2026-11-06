# Provenance

How this crate was written, recorded truthfully so its licensing can be
checked.

## Sources used

- **RFC 6386 prose and inline pseudo-code** (Sections 1 to 19): the format,
  the decoding process and the bitstream syntax (Annex A, Section 19).
- **RFC 6386 Section 20, the attached reference decoder source.** It was read
  and used as the specification for edge cases. The files read were
  `dixie.c`, `modemv.c`, `predict.c`, `tokens.c`, `idct_add.c`,
  `dixie_loopfilter.c`, `bool_decoder.h`, `modemv_data.h`,
  `dequant_data.h` and `vp8_prob_data.h`. In particular, it decided:
  - the order of the golden and alt-ref buffer copies;
  - the near-MV search and the split-MV chroma rounding;
  - the edge pixels for intra prediction (127 above, 129 left, the corner);
  - the token contexts and the skip handling;
  - the 16-bit intermediate storage in the inverse transforms.
- **Tables.** The coefficient, motion-vector, mode and quantizer tables were
  extracted from Section 20's headers by a script, not typed by hand. They
  were then checked value for value against the same tables in the
  `image-webp` crate (MIT OR Apache-2.0).

## Sources not used

- **libvpx source was not opened.** Some design reasoning drew on prior
  knowledge of how libvpx behaves (for example, that it treats pixels
  outside the reference frame as an unbounded extension of its edges). No
  libvpx code was consulted or copied, and its PATENTS file is not included.

## What this is, legally

The Rust code has its own structure. It is not a line-by-line translation.
The algorithms follow the Section 20 reference decoder closely, though, so
the crate is treated as a **port of RFC 6386 Section 20**. NOTICE carries
that code's BSD-3-Clause licence (Section 20.26) verbatim. The crate's
licence is `(MIT OR Apache-2.0) AND BSD-3-Clause`. Nothing here grants any
patent licence beyond what Apache-2.0 grants for our own contributions.

## Test data

The WebM project's VP8 test vectors (`vp80-00-comprehensive-*.ivf` and
their `.md5` files) are test data only. They are downloaded at test time by
`scripts/fetch-test-vectors.sh` (pinned by SHA-256) and are not part of
this repository or the published crate.

## Other implementations evaluated

**zvidlib-vp8** 0.5.0 (MIT, by Loren Segal) passes the same vectors; its
own tests were run on 2026-11-05. Its notices also describe it as derived
from the Section 20 reference decoder. It was not adopted, for four reasons:
- its decoder is private, and the public API outputs RGBA only, not I420;
- it pulls in its own framework crates, with hardware decoder loaders on
  by default and a Swift build script;
- it has about 100 `unsafe` blocks in its SIMD code;
- it needs edition 2024.

During that evaluation, the following parts of zvidlib-vp8 were read:
- `THIRD_PARTY_NOTICES.md`, `LICENSE`, `Cargo.toml` and `build.rs`;
- the first 80 lines of `src/lib.rs` (crate docs, module list, imports);
- the test-vector harness in `src/tests.rs` (about 30 lines, plus a search for test names);
- a count of `unsafe` uses.

None of its decoder code or tables was read, and nothing in this crate came
from it. This crate's decoder had been written before the evaluation. It
may be used later only as a black-box test oracle (a dev-dependency or CI
step), never as a normal dependency.
