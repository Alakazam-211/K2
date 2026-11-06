# Changelog

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
  x86_64 SSE2, including the byte transposes for vertical edges. Both are
  always present on those targets, so there is no run-time detection.
- **Inter prediction.** The six-tap and bilinear filters work in 16-bit
  lanes (the positive and negative taps summed apart, which is exact), the
  block width is a constant in each specialised copy, and the edge buffer
  is only set up for blocks near the frame's edge.

### Unsafe code

- The NEON and SSE2 modules are the crate's only `unsafe` code; the crate
  is now `deny(unsafe_code)` instead of `forbid(unsafe_code)`. Every
  kernel gets slices already cut to size with checked indexing. The
  README's Security section has the details.
- `RUSTFLAGS="--cfg vp8_portable"` leaves the SIMD modules out: the
  decoder is then entirely safe Rust.

### Tests

- The 0.1.0 scalar code is kept in `src/reference/` for test builds. The
  loop filter (portable and SIMD, every kind and direction, every level,
  sharpness and frame type) and inter prediction (every block size,
  filter, sub-pixel phase and edge case) are checked against it byte for
  byte, on random pictures that include the extreme values.
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
