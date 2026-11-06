# Changelog

## 0.1.0 (unreleased)

The first release: the codec-neutral API (`VideoBackend`, `FrameSource`,
`VideoEncoder`, `VideoDecoder`), the null and fake backends, the macOS
backend (AVFoundation cameras, ScreenCaptureKit screens, VideoToolbox H.264
encoding), the Windows backend (Media Foundation cameras, Windows Graphics
Capture screens), the Linux backend (V4L2 cameras, ScreenCast portal and
PipeWire screens), and VP8 through vp8-encoder and vp8-decoder in every
backend.

### Packaging

- Ready for crates.io: `repository` is set, `publish = false` is gone, and
  `include` ships only `src/`, the README and the licences.
- vp8-decoder and vp8-encoder come from crates.io (`0.1`, caret
  requirements), not from a path.
- The minimum Rust version is 1.82, the `windows` crate's.
- The README has a platforms table (tested, untested on hardware, or only
  type-checked), the security contact, untrusted input, `unsafe` code,
  system libraries only, the BSD-3-Clause notice duty for the VP8 crates,
  patents, and what 0.x means for API stability.
- A seeded hostile-input test sends random and mutated frames through the
  VP8 decoder (directly and through its thread) and the H.264 helpers.
- `h264::avcc_nals` checks a NAL's length without an addition that could
  overflow on a 32-bit target.
- `scripts/check-targets.sh` (was `check-windows.sh`) type-checks Windows,
  iOS and Android, or any target, on a toolchain that can't add targets.
