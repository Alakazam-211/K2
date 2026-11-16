# Changelog

## 0.1.2

Where the crate lives and where to report issues, plus two fixes in the
loop filter's hidden shared module. Decoded pictures don't change.

### Links, metadata and issues

- The crate's homepage is now its folder in the K2 repository,
  [`crates/vp8-decoder`](https://github.com/Alakazam-211/K2/tree/main/crates/vp8-decoder).
- The README's new Issues section links a short issue form for the vp8
  crates (label `vp8`; `.github/ISSUE_TEMPLATE/vp8.yml` in the
  repository) and the vp8 issues so far, and asks for the crate version,
  the platform and CPU, and a sample stream or a minimal repro.
- The `documentation` link points to docs.rs.
- The README states the MSRV, Rust 1.80 (the `rust-version` it already
  had), and points security reports to the repository's SECURITY.md:
  privately, never in a public issue.
- The `include` list is anchored to the crate's root and names exactly
  what the .crate holds: the source, README, CHANGELOG, the licences,
  NOTICE and PROVENANCE.md. The packaged files don't change.

### Loop filter

- **Overlapping rows take the portable code.** With a stride under a
  segment's width (under 16 pixels across a horizontal luma edge, under
  eight for chroma), the rows overlap. The NEON and SSE2 kernels write
  back every tap and the portable code only the taps the filter may
  change, so there the two could give different bytes. These strides now
  take the portable code on aarch64 and x86_64, as vertical luma edges
  under eight pixels already did, so the SIMD and portable paths give the
  same bytes at every stride, as the docs say. Only the hidden
  `loop_filter::filter_mb` can be given such strides; the decoder never
  does.
- **Edge limits can't overflow.** `filter_mb` computes its edge limits,
  `(level + 2) * 2 + interior` and `level * 2 + interior`, in 64-bit
  arithmetic and then clamps them to a byte, so any `level` gives the
  exact value, clamped. Before, a very large or very negative level (from
  715,827,882 up at sharpness 0) overflowed 32-bit arithmetic: a panic in
  debug builds, a wrapped limit in release builds. The decoder only uses
  levels 0 to 63, whose limits are unchanged.

### Tests

- Strides under a segment's width, for every edge kind, both directions,
  luma and chroma, on their own and through `filter_mb`: the platform
  path matches the portable one byte for byte.
- Edge limits at both ends of `i32` and on either side of the old
  overflow, at two sharpnesses and both frame types, with `filter_mb`
  run at each level on both paths.

## 0.1.1

Faster decoding, the same output byte for byte. Nothing in the public API
changes.

### Speed

- One core decodes camera-like 640x360 video at 600 kbit/s at about
  1,720 fps on an Apple M4 (was about 960) and 820 fps on
  an x86_64 server CPU (was about 310). See the README's Speed section.
- **Loop filter.** It now works in 8-bit lanes with saturating arithmetic,
  which is exactly the RFC's clamping, and filters U and V together (eight
  segments each in one 16-lane pass). On aarch64 it uses NEON and on
  x86_64 SSE2, including the byte transposes for vertical edges. They are
  chosen at compile time from the target's features (no run-time
  detection): a target without them gets the portable code.
- **Inter prediction.** The six-tap and bilinear filters work in 16-bit
  lanes (the positive and negative taps summed apart, which is exact), the
  block width is a constant in each specialised copy, and the edge buffer
  is only set up for blocks near the frame's edge.

### Unsafe code

- The NEON and SSE2 modules are the crate's only `unsafe` code; the crate
  is now `deny(unsafe_code)` instead of `forbid(unsafe_code)`. Every
  kernel asserts the length of the slices it uses, and its callers cut
  them with checked arithmetic, so any stride or position, even one that
  overflows, panics rather than reaching outside the planes. The README's
  Security section has the details.
- The loop filter's sizes and positions are computed with checked
  arithmetic on the portable path too.
- `RUSTFLAGS="--cfg vp8_portable"` leaves the SIMD modules out: the
  decoder is then entirely safe Rust.

### Tests

- The 0.1.0 scalar code is kept in `src/reference/` for test builds. The
  loop filter (portable and SIMD, every kind and direction, every level,
  sharpness and frame type) and inter prediction (every block size,
  filter, sub-pixel phase and edge case) are checked against it byte for
  byte, on random pictures that include the extreme values.
- The loop filter is also checked at edge strides and sizes: frames of
  one macroblock up to several, tight and odd strides, strides below a
  block, and buffers exactly as long as the planes. Strides and positions
  whose arithmetic overflows must panic on every path. The SIMD paths run
  under Miri for these tests where Miri supports the intrinsics.
- The WebM conformance vectors still match bit for bit.

### Profiling

- `RUSTFLAGS="--cfg vp8_stage_timers"` adds per-stage timers (modes,
  tokens, intra, inter prediction, inter residual, loop filter), read with
  the hidden `Decoder::stage_times()`. Not a stable API, and off unless
  that flag is set.
- vp8-encoder's `decode_speed` example (in the repository) measures this
  decoder on camera-like streams that the encoder makes.

## 0.1.0

- First release.
