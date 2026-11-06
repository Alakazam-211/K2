# Changelog

## 0.1.0 (unreleased)

The first release: the codec-neutral API (`VideoBackend`, `FrameSource`,
`VideoEncoder`, `VideoDecoder`), the null and fake backends, the macOS
backend (AVFoundation cameras, ScreenCaptureKit screens, VideoToolbox H.264
encoding), the Windows backend (Media Foundation cameras, Windows Graphics
Capture screens), the Linux backend (V4L2 cameras, ScreenCast portal and
PipeWire screens), the Android backend (Camera2 cameras, MediaCodec H.264
encoding), and VP8 through vp8-encoder and vp8-decoder in every backend.

### Android backend

- `system()` on Android: Camera2 cameras through the NDK (`libcamera2ndk`
  with `AImageReader`), front and back selection (the front camera is the
  default), the sensor modes at 30 fps with the auto-exposure range set to
  match, frames rotated upright for the sensor orientation and the device's
  turn, and size boxes that fit portrait and landscape frames alike.
- `AndroidContext` holds the host's JNI handles (`AndroidContext::new`,
  unsafe: the `JavaVM*` and a `Context` global reference) for reading the
  CAMERA permission, and the app's lifecycle: `set_background` closes
  cameras on pause (`Interrupted(Background)`) and reopens them on resume
  (`Resumed`); `set_device_orientation` turns frames upright.
- A denied CAMERA permission answers `NoPermission`; asking for it stays the
  app's job. A camera another app takes reports
  `Interrupted(InUseByAnother)` and reopens on resume or unpause.
- H.264 encoding through `AMediaCodec` (NV12 or I420 input, SPS and PPS
  before every IDR, Constrained Baseline flags set in the SPS). VP8 encodes
  and decodes through the Rust crates. No H.264 decoder yet.
- Screen capture answers `Unsupported`: MediaProjection needs a Java
  foreground service.
- The backend's logic runs over a scripted platform in the host tests; only
  the NDK calls need a device. Type-checked for `aarch64-linux-android` and
  `x86_64-linux-android`; not yet run on a device or an emulator.

### Packaging

- Ready for crates.io: `repository` is set, `publish = false` is gone, and
  `include` ships only `src/`, the README and the licences.
- vp8-decoder and vp8-encoder come from crates.io (`0.1`, caret
  requirements), not from a path.
- The minimum Rust version is 1.87: zbus 5 and its zvariant crates need it
  on Linux (the `windows` crate needs 1.82).
- The README has a platforms table (tested, untested on hardware, or only
  type-checked), the security contact, untrusted input, `unsafe` code,
  system libraries only, the BSD-3-Clause notice duty for the VP8 crates,
  patents, and what 0.x means for API stability.
- A seeded hostile-input test sends random and mutated frames through the
  VP8 decoder (directly and through its thread) and the H.264 helpers.
- The fake backend's structural decoder refuses frames over 8192 x 8192
  (`Corrupt`), and its docs say it is not for untrusted input.
- `deny.toml` ignores three unmaintained notices for zbus 5's dependencies
  (RUSTSEC-2026-0370, -0371, -0372); they clear with zbus 6.
- `h264::avcc_nals` checks a NAL's length without an addition that could
  overflow on a 32-bit target.
- `scripts/check-targets.sh` (was `check-windows.sh`) type-checks Windows,
  iOS and Android, or any target, on a toolchain that can't add targets.
