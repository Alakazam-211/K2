# Changelog

## 0.1.5

A camera that goes away now ends its source on macOS and iOS, the camera
list says which cameras are suspended, and opening the default camera skips
a suspended one. Additions only: nothing in
0.1.4's API changes or goes away.

### A lost camera ends its source (macOS and iOS)

- Before, a camera source noticed a lost device only by a look every
  250 ms at `isConnected` and `isRunning`. On macOS a Continuity Camera
  can be invalidated (its session posts a runtime error and stops) while
  those still read as running, and the source then stayed open with no
  frames and no event.
- Now each source also observes `AVCaptureDeviceWasDisconnectedNotification`
  for its device and `AVCaptureSessionRuntimeErrorNotification` for its
  session, on macOS as on iOS. The source ends with
  `SourceEvent::Ended(EndReason::DeviceLost)` (after the error as
  `SourceEvent::Error`) when:
  - its device disconnects;
  - on macOS, its session posts any runtime error;
  - on either OS, the runtime error says the device is gone
    (`AVErrorDeviceWasDisconnected`, `AVErrorDeviceNotConnected`);
  - as before, the device stops being connected, or the session stops
    without an interruption.
- A pause is not an end. On iOS a phone call, the app in the background,
  iPad multitasking or heat sends `Interrupted(...)` and later `Resumed`,
  as before, and never `DeviceLost`. iOS resetting its media services still
  sends the error and restarts the session once.
- `Ended` is sent once, and nothing is sent after the source is dropped:
  the observers are removed then, and one already running on another
  thread sends nothing.
- `AVErrorDeviceWasDisconnected` (-11808) now maps to `VideoError::DeviceLost`
  and `AVErrorDeviceInUseByAnotherApplication` (-11815) to
  `VideoError::DeviceBusy` (both were `Os`).

### Suspended cameras

- `CameraInfo` has a new field, `suspended: bool`: the OS lists the camera
  but says it can't deliver frames now. On macOS and iOS it is
  `AVCaptureDevice.isSuspended`, which is true for a MacBook's built-in
  camera while the lid is closed (and for a camera with a shut lens cover).
  `CameraInfo::new` sets it to `false`. Windows, Linux and Android don't
  report it: there it is always `false`.
- A change of it is a change of the list, so `BackendEvent::Cameras`
  reports it.
- `open_camera(None, ...)` skips a suspended default. Before, it always
  opened the system default, so a MacBook with its lid closed opened the
  built-in camera and got no frames. Now, when the default is suspended,
  the first camera in `cameras()`' order that isn't suspended opens. When
  every camera is suspended, the default opens as before, and its source
  reports `Interrupted(Suspended)`. With `CaptureConstraints::facing`
  (phones), the first camera facing that way that isn't suspended opens,
  else the first facing that way, as before. `is_default` still marks the
  system default. The fake backend follows the same rule.
- `Interruption` has a new variant, `Suspended`: an open camera source on
  macOS or iOS whose device becomes suspended sends
  `Interrupted(Suspended)`, then `Resumed` when it comes back.
- `CameraInfo::id` is unchanged and now documented per platform:
  `AVCaptureDevice.uniqueID` on macOS and iOS, which stays the same when a
  camera goes away and comes back.


Open encoders and decoders say what the OS really made, hardware H.264 is
listed only after a real probe on every platform, `convert` reads native
buffers in place, the encoder and decoder threads ask for a QoS class on
Apple, and the Windows backend gets H.264 through Media Foundation.
Additions only: nothing in 0.1.3's API changes or goes away.

### What an open codec runs

- `Implementation` has a new variant, `Unknown`: the OS doesn't say. Only
  an open session reports it; capability entries stay `Hardware` or
  `Software`. Treat it as not known to be hardware.
- `VideoEncoder::implementation()` and `VideoDecoder::implementation()`
  (provided methods) read `EncoderInfo::implementation` and
  `DecoderStats::implementation`, which now come from the session the OS
  created, never from what was asked for or listed:
  - after an encoder's `set_size` reopens its session, it is asked again;
  - a decoder reports `None` until its session opens on the first
    keyframe, and again after `reset`.
- VideoToolbox: the session's own "using hardware" property when it
  answers; else a binding ask (hardware required, which fails the create
  without it, or hardware disabled, which is never used); else
  VideoToolbox's encoder list, by the session's encoder ID. Nothing said is
  `Unknown`.
  - Before, on macOS, VideoToolbox's low-latency encoder (which every
    `PreferHardware` and `HardwareOnly` encoder gets) doesn't answer the
    property, and a missing answer counted as software. So a hardware
    session reported `Software`, `HardwareOnly` always failed, and
    `capabilities()` listed no hardware H.264 encoder, even on Apple
    silicon. `PreferHardware` now asks for hardware first and falls back to
    letting VideoToolbox choose, so a hardware session is known to be one.
  - `HardwareOnly` refuses a session it can't confirm as hardware.
  - An iOS before 17.4 can't require or report hardware for an encoder,
    nor an iOS before 17.0 for a decoder: unless the encoder list names the
    encoder, those sessions are `Unknown`.
- Android: from the codec's component name (`AMediaCodec_getName`, API 28,
  looked up at run time). Android's own software codecs (`c2.android.*`,
  `c2.google.*`, `OMX.google.*`), FFmpeg's, the emulator's, components
  marked `.sw`, and names that aren't a vendor's `OMX.*` or `c2.*` count as
  software. Before API 28 the name isn't there: `Unknown`, except for the
  software encoder asked for by name.
- VP8 (the Rust codecs) is `Software`, as before.

### Hardware H.264 is probed, never assumed

- macOS and iOS: `capabilities()` lists the hardware H.264 encoder only if
  a hardware-only 720p30 session opens and is confirmed as hardware. Before,
  iOS listed it without asking. The probe runs once; one run from an iOS
  app in the background isn't kept, and the next one in front decides.
- Android: the AVC encoders are probed once, on first use: the codec
  `video/avc` resolves to (by name), and Android's own software encoder.
  H.264 encode is listed as `Hardware` only when the first is a vendor
  component, as `Software` (Constrained Baseline, 720p30) when Android's
  own encoder exists, and not at all otherwise. Before, a `Hardware` entry
  was always listed. VP8 is listed as before.
- Android `HardwareOnly`: `Unsupported` when the probe found no hardware
  encoder; `NoResources` when it did but this open got a software or
  unnamed codec (the hardware one is taken).

### `convert` reads native buffers in place

- To I420 or NV12, `convert` (and every backend's `convert`) now reads the
  source where it lies: CPU planes, a CVPixelBuffer (NV12 `420v` and
  `420f`, BGRA) or an AHardwareBuffer (RGBA, RGBX, and YUV 4:2:0 with any
  row and pixel stride: planar, NV12 or NV21), locked for reading. Before,
  a native buffer was first copied out whole.
- Its planes are scaled straight into the output, with the same
  arithmetic as before: the same bytes for every source, size and format.
- The output's memory comes from a pool and goes back to it when the frame
  and all its clones drop, so a steady stream of frames allocates no pixel
  memory (only a small handle a frame). The pool keeps at most eight idle
  buffers and 64 MiB.
- A frame whose width and height don't match its native buffer is
  `Invalid`.
- From a 1280x720 `420v` CVPixelBuffer to I420, a frame takes 0.27 ms at
  640x360 (and 360x640 from portrait), 0.19 ms at 1280x720 and 0.51 ms at
  320x180 on an Apple M4 Mac mini; 0.39, 0.24 and 0.71 ms on an M1. From
  BGRA, 1.7 ms at 640x360 on the M4. Before, each conversion also copied
  the whole buffer out first and allocated every intermediate picture.
- Needs bytes 1.9 (`Bytes::from_owner`).

### Codec threads at QoS user-initiated on macOS and iOS

- Before, the threads behind every encoder and decoder (`ThreadedEncoder`,
  `ThreadedDecoder`: VP8 on every backend, VideoToolbox H.264 on Apple)
  set no QoS class. They ran at the default class, which the system may
  move to efficiency cores. On an Apple M4, a VP8 decoder thread at the
  background class took 3.5 ms a frame at 640x360 instead of 0.6 ms.
- Now each thread moves itself to `QOS_CLASS_USER_INITIATED` when it
  starts: real-time media work, but below the UI's user-interactive class.
- Other platforms are unchanged.

### H.264 encoding and decoding on Windows

- Before, the Windows backend had VP8 only: H.264 encoders and decoders
  answered `Unsupported` and `capabilities()` listed no H.264.
- Now H.264 runs through the Media Foundation transforms (MFTs) that come
  with Windows or its GPU driver. Hardware MFTs (`MFTEnumEx` with
  `MFT_ENUM_FLAG_HARDWARE`) come first; otherwise Microsoft's software H.264
  encoder and decoder, picked by CLSID, so no software MFT another app
  registered is ever used. The crate still contains no H.264 code.
- `capabilities()` lists each kind (hardware or software, encode or
  decode) only after a session of it opened, and for an encoder encoded a
  frame. The probe runs once per process. Hardware entries say
  `Implementation::Hardware`, Microsoft's say `Software`.
- Every session reports the MFT it actually runs: the encoder's
  `info().implementation` and the decoder's `stats().implementation`.
  `ImplPreference::HardwareOnly` never falls back to software; it answers
  `Unsupported` where there is no hardware MFT.
- The encoder: low-latency mode, no B-frames, constant bitrate at the
  target with `set_rates` taking effect in place (no keyframe), keyframes
  on request (`CODECAPI_AVEncVideoForceKeyFrame`) plus the configured
  interval, and Constrained Baseline or Constrained High (plain Baseline or
  High where an MFT lacks the constrained ones). Input is CPU NV12; other
  formats and sizes are converted first.
- Its output is the crate's H.264 form: one Annex B access unit per frame,
  4-byte start codes, no access unit delimiters, SPS and PPS first on every
  IDR (from the stream, or the MFT's sequence header).
- Hardware MFTs are asynchronous (they say when they want input and have
  output through their event queue); Microsoft's are synchronous. Both are
  driven one frame at a time.
- `capabilities()` and opening an H.264 encoder or decoder do their COM
  and Media Foundation work on a short-lived thread of their own, so the
  caller's thread never joins COM's multithreaded apartment (an app's UI
  thread must stay free to join a single-threaded one for its webview).
- The decoder takes Annex B access units and gives CPU frames, cropped to
  the picture, in the format the caller asks for. It runs in low-latency
  mode, so each frame comes out as it is decoded. Microsoft's decoder runs
  on the CPU here: it is given no Direct3D device.
- No Direct3D 11 native frames yet: Windows cameras, screens and the
  decoder all give CPU planes, so `convert` from a native frame doesn't
  arise on Windows.
- Tested on Windows 11 with no GPU (Microsoft's software MFTs): encoding
  and decoding at 640x360 and 1280x720, a bitrate change, keyframe
  requests, a size change, and the hardware and software flags. Hardware
  MFTs have not run.

### Hardening

- The Windows H.264 decoder reads each output only within the length its
  buffer's lock reports (`Lock2DSize` for 2D buffers, the current length
  otherwise), with every offset computed with checked arithmetic. A
  picture layout that doesn't fit the buffer is an error, never a read
  past it. A change of output type clears the cached layout even when the
  same call fails, and outputs from before a change are dropped instead
  of read with the new layout.
- A Windows crop that starts on an odd column or row starts on the even
  one before it, so each pixel keeps its own chroma.
- The Windows codecs' sample times can't overflow.
- Android: a locked AHardwareBuffer's span is checked before any slice is
  made over it (a zero height, a stride shorter than a row, overflowing
  sizes).
- `Planes::validate` computes each plane's end with checked arithmetic, so
  any offset or stride gives an error rather than a panic.
- Apple: a pixel buffer's base address is unlocked by a guard, so an error
  or a panic while it is locked can't leave it locked. CoreFoundation
  values the OS hands back by key are type-checked before they are read
  as booleans, numbers, dictionaries or arrays.
- `pixel_buffer_frame` and `hardware_buffer_frame` now say in their safety
  contract that the host must not write to the buffer while any clone of
  the frame is alive: frames are read in place.

### Windows N and KN editions

- Media Foundation's DLLs (`mfplat.dll`, `mf.dll`, `mfreadwrite.dll`) are
  now loaded at run time from System32 instead of imported. An N or KN
  edition without the Media Feature Pack has none of them; before, an app
  built with the Windows backend would fail to start there.
- There, `capabilities()` lists no H.264 and `camera` is false, H.264
  encoders and decoders and cameras answer `Unsupported`, and VP8 and
  screen capture work as before.

## 0.1.3

The Apple backend's `convert` makes native frames from CPU planes. Nothing
in 0.1.2's API changes.

### `convert` to `PixelFormat::Native` on macOS and iOS

- Before, the Apple backend's `convert(frame, w, h, PixelFormat::Native)`
  answered `Invalid` for every frame, so CPU-decoded video (VP8) had no way
  to become a CVPixelBuffer for an `AVSampleBufferDisplayLayer`.
- Now CPU planes (I420, NV12, BGRA or RGBA) become an NV12 CVPixelBuffer,
  which `apple_pixel_buffer` hands out. I420 and NV12 of the asked size are
  copied straight in; other formats and other sizes are converted and
  scaled on the CPU first.
- The buffers come from a `CVPixelBufferPool` per size and range, reused
  from frame to frame. A buffer goes back to its pool once every frame
  holding it is dropped. The backend keeps the four most recently used
  pools, and a new pool (a size change) lets the others release their idle
  buffers.
- Each buffer is IOSurface-backed (a display layer takes it without a
  copy) and Metal-compatible.
- Colour: `Bt709Full` frames get full-range buffers (`420f`), the others
  video range (`420v`), and every buffer carries the frame's matrix,
  primaries and transfer function as attachments.
- A native frame that already has the asked size passes through
  unchanged. At another size it is read back, scaled on the CPU and copied
  into a pooled buffer.
- Other formats, and the other backends, are unchanged: they still answer
  `Invalid` for `Native`.

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
