# Changelog

## 0.1.2

Fixes linking the Apple backend into an app through a static library, and
gives screen thumbnails their size. Nothing in 0.1.1's API changes.

### Apple frameworks in a static library's link

- An iOS app that linked kessel-video inside its Rust static library (Tauri
  on iOS: Xcode links `libapp.a`) failed with VideoToolbox's session calls
  undefined. The `#[link]` attributes were there, but rustc passes them only
  to links it runs itself; for a static library it just prints them as
  `native-static-libs`, and Xcode never reads that list.
- The Apple backend now also writes its frameworks into its object code as
  linker options (`LC_LINKER_OPTION`, what Clang writes for `@import`), so
  the app's linker adds them on its own: AVFoundation, CoreFoundation,
  CoreMedia, CoreVideo, Foundation and VideoToolbox. Apps need no extra
  framework settings for kessel-video. ScreenCaptureKit and CoreGraphics
  are still opened at run time.
- A linker drops an auto-linked framework that it takes no symbol from, and
  AVFoundation's classes are looked up at run time. So the camera code now
  reads `AVMediaTypeVideo` by name instead of through `dlsym`, which keeps
  AVFoundation in the app. Before, a static-library app with no AVFoundation
  of its own would link and then fail when the camera code looked for its
  classes. Foundation, whose classes are also looked up at run time, comes
  in with AVFoundation and is now linked by name as well.
- `examples/link_check.rs` with `scripts/check-apple-link.sh` links the
  crate as such an app does (a static library, `cc`, no `-framework` flags)
  and runs it on macOS, and for iOS links it with an iPhoneOS SDK or checks
  the archive's undefined symbols against its linker options.
  `scripts/check-android-link.sh` also links it as a shared library with no
  undefined symbols allowed. These are repository checks, not in the
  package.

### Screen thumbnails

- `ScreenSourceInfo` has `thumbnail_width` and `thumbnail_height` beside
  `thumbnail`, `with_thumbnail(planes, width, height)` (RGBA, at most 320
  px wide, checked against the planes) and `thumbnail_size()`. A thumbnail
  with padded rows no longer has its size guessed from the stride.
- No backend makes thumbnails yet; the fields are 0 and `thumbnail` is
  `None` from `ScreenSourceInfo::new`.

## 0.1.1

Adds the iOS and Android backends, VideoToolbox H.264 decoding and frame-in
sources. Nothing in 0.1.0's API changes.

### iOS backend

- `system()` on iOS gives the Apple backend instead of `Unsupported`. It
  shares `src/apple/` with macOS: AVFoundation cameras, CoreVideo pixel
  buffers, VideoToolbox, and the FFI and Objective-C helpers.
  ScreenCaptureKit stays macOS-only (`screen.rs`); the iOS-only parts are in
  `ios.rs`.
- The camera session leaves the app's audio session alone on iOS
  (`automaticallyConfiguresApplicationAudioSession = NO`) and never adds an
  audio input, so it works inside a CallKit call.
- Cameras on iOS: the front camera by default, `CaptureConstraints::facing`
  for front or back, modes chosen with the box turned landscape, frames
  turned upright (the rotation coordinator on iOS 17+, the device's
  orientation before) and unmirrored. Interruptions (the app in the
  background, a call, another app, thermal pressure) are `Interrupted` and
  `Resumed` source events, not an end; a media-services reset restarts the
  session. `Background(bool)` backend events follow the app.
- Opening a camera or an H.264 encoder while the app is in the background
  answers `ShutDown`, and so does an H.264 encoder whose session iOS takes
  away. iOS offers no software-only H.264, so `SoftwareOnly` is
  `Unsupported` there.
- Screens come in through frame-in (below), from the app's broadcast upload
  extension: `pick_screen` and `open_screen` answer `Unsupported`.
- The objc2 and block2 dependencies apply to iOS as well as macOS.

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
- Screens come from a MediaProjection the app hands over
  (`AndroidContext::set_media_projection`, `set_screen_size`,
  `media_projection_stopped`; API 26+), as NV12 from a `VirtualDisplay` and
  `ImageReader`, resized when the display turns, and `Ended(SourceClosed)`
  when the projection stops. Asking for the projection and its foreground
  service stay the app's Java or Kotlin.
- The backend's logic runs over a scripted platform in the host tests; only
  the NDK and JNI calls need a device. Type-checked for
  `aarch64-linux-android` and `x86_64-linux-android`; not yet run on a
  device or an emulator.

### Everywhere

- Frame-in sources: `frame_in()` makes a `FrameSource` the host pushes
  frames into (CPU planes, or a native buffer through `pixel_buffer_frame`
  on Apple and `hardware_buffer_frame` on Android), with timestamps and a
  `Rotation` that turns them upright; it never pauses for the app's
  background. `ScreenCapability::frame_in` (new) says a platform's screens
  come in this way (iOS).
- H.264 decoding through VideoToolbox, on macOS and iOS: in-band SPS and PPS
  set up or replace the session, outputs are NV12 pixel buffers (`Native`,
  or converted to the asked format), and a session iOS takes away is
  `DecodeError::SessionLost`. The capabilities list it.
- `apple_pixel_buffer` (macOS and iOS): the `CVPixelBufferRef` inside an
  Apple native frame (a VideoToolbox decoder's `Native` output), borrowed
  while the buffer lives, for an `AVSampleBufferDisplayLayer`.
- macOS also honours `CaptureConstraints::facing` when no camera id is
  given (the first camera facing that way, else the default).
- `kit::SourceShared::set_box_follows_frame` turns the asked box to the
  frame's orientation before fitting (phone cameras use it).

### Packaging

- The package ships this changelog.
- The README's platform table says iOS and Android are type-checked and
  unit-tested only, never run on a device, and the README says where an
  iPhone broadcast extension's frames can come from.

## 0.1.0

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
