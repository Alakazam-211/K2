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
