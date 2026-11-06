# kessel-video

Cameras, screen capture, pixels and video codecs in Rust, for apps that
carry live video over their own transport:

- **camera and screen capture** through each operating system's own APIs;
- **H.264 encoding and decoding through the OS's codec** (VideoToolbox on
  macOS and iOS). The crate contains no H.264 implementation and links no
  codec library;
- **VP8** encoding and decoding in pure Rust, through the published
  [`vp8-encoder`](https://crates.io/crates/vp8-encoder) and
  [`vp8-decoder`](https://crates.io/crates/vp8-decoder) crates;
- **codec-neutral traits** (`VideoBackend`, `FrameSource`, `VideoEncoder`,
  `VideoDecoder`) that a call engine drives without knowing which codec or
  OS is underneath. Sew's call engine uses them;
- a scripted **fake backend** that runs all of it without hardware.

Kessel is Alakazam Labs' family of open-source Rust crates for real-time
apps. kessel-video stands alone: it needs no other Kessel crate. It is the
video sibling of [`kessel-audio`](https://crates.io/crates/kessel-audio). API
docs are on [docs.rs](https://docs.rs/kessel-video), and the source is at
<https://github.com/Alakazam-211/K2> (`crates/kessel-video`). Licensed MIT OR
Apache-2.0.

**Version 0.1** is the first release. Until 1.0, a minor release (0.2, 0.3)
may change the API, as Cargo allows for 0.x versions; a patch release (0.1.x)
won't. Each change is listed in CHANGELOG.md.

## Platforms

| Platform | Cameras | Screens | Encode | Decode | Status |
|---|---|---|---|---|---|
| macOS (Apple Silicon) | AVFoundation | ScreenCaptureKit (12.3+) | H.264 (VideoToolbox), VP8 | H.264 (VideoToolbox), VP8 | The test suite passes, including VideoToolbox H.264 encoding and decoding. **Camera and screen capture are untested on hardware**: they need a person to grant permission, so their tests only run by hand and have not been run. |
| iOS (iPhone, iPad) | AVFoundation | Frame-in: the app feeds its broadcast upload extension's frames | H.264 (VideoToolbox), VP8 | H.264 (VideoToolbox), VP8 | **Type-checked and unit-tested only: never run on an iPhone or iPad.** The backend shares its VideoToolbox, pixel buffer and AVFoundation code with macOS, where that code's tests pass. The iOS-only code and its tests (which need the simulator or a phone) have not run, and camera capture, rotation and interruptions need a device and a person. |
| Linux (x86-64) | V4L2 | The ScreenCast portal with PipeWire | VP8 (no H.264: Linux has no OS codec) | VP8 | The test suite passes on machines with no camera and no desktop. **Camera capture is untested with a real camera, and screen capture is untested on a desktop** (`scripts/try-linux-screen.sh` in the repository is the manual check). |
| Windows (x86-64) | Media Foundation | Windows Graphics Capture | VP8 | VP8 | **Type-checked only.** |
| Android (API 24+) | Camera2 through the NDK | MediaProjection (API 26+), once the app hands one over | H.264 (MediaCodec), VP8 | VP8 | **Type-checked and unit-tested only: never run on a device or an emulator** (aarch64 and x86-64). The backend's logic (cameras, rotation, pause and resume, MediaProjection screens, errors, VP8) is tested on a host over a scripted platform; the NDK and JNI calls themselves have not run. |
| Any | the fake backend | the fake backend | VP8, the fake's structural H.264 | the same | Tested on Linux and macOS. |

"Type-checked" means `cargo check` and `cargo clippy` pass for that target.
"Unit-tested" means the logic's tests pass on a Mac or a Linux machine, not on
that platform. The test suite needs no camera, screen or GPU.

## What it does

```text
 camera / screen ─► FrameSource ─► VideoFrame ─► VideoEncoder ─► EncodedFrame ─► (your packetiser, encryption, RTP)
                                                                                         │
 (your depacketiser, decryption) ─► EncodedInput ─► VideoDecoder ─► VideoFrame ─► (your renderer)
```

The transport stays outside. kessel-video never sees RTP, keys or
ciphertext: it hands out plaintext encoded frames and takes in frames the
caller has already decrypted and verified.

| Piece | How |
|---|---|
| Backend | One `VideoBackend` per app: capabilities, cameras, permissions, screens and windows, encoders, decoders, conversion, machine-wide events. `system()` for the OS, `null()` for none, `fake::FakeBackend` for tests. |
| Threads and queues | Sources, encoders and decoders work by submit and poll. Each takes a `Wake` callback that runs after something is queued. Queues are small and bounded and never block the producer: capture 2 frames (oldest dropped), encoder output 8 (`submit` answers `Busy` instead), decoder output 2 (oldest dropped), events 32. No async runtime. |
| Frame-in | `frame_in()` makes a `FrameSource` that the host feeds: CPU frames or native buffers (a CVPixelBuffer through `pixel_buffer_frame`, an AHardwareBuffer through `hardware_buffer_frame`), each with its timestamp and the quarter turn that makes it upright. Same queue, rate, size box and events as any source, and it never pauses for the app's background. Phone screen sharing comes in this way on iOS. |
| Codecs | Codec-neutral: VP8 as raw RFC 6386 frames, H.264 as one Annex B access unit with 4-byte start codes and SPS and PPS before every IDR. Real time: no reordering, no B-frames, one output per input at most, `set_rates` never forces a keyframe, `capture_us` passes through. |
| VP8 | `vp8-encoder` and `vp8-decoder` (feature `vp8`, part of `system`) in every backend, the fake included: real time, L1T2 and L1T3 temporal layers, one encoder per simulcast layer. |
| Pixels | I420, NV12, BGRA and RGBA, with conversion and scaling on the CPU (`convert`), and OS-native buffers (`NativeFrame`) that pass from a source to an encoder of the same backend without a copy. |
| Capability query | Per codec, direction and implementation: hardware or software, the largest size and rate, how many may be open at once, temporal layers, H.264 profiles and level, pixel formats. |

### Screens by platform

- **macOS:** list displays and windows, the system picker (macOS 14+), and
  window exclusion.
- **Windows:** displays and windows, and the system picker (pass the app's
  HWND with `PickOptions::with_parent_window`). No window exclusion: Windows
  can only hide our windows from every capture.
- **Linux:** the portal's picker is the only way to choose, and there is no
  window exclusion.
- **iOS:** a broadcast upload extension captures the screen in its own
  process; the app feeds its frames into a `frame_in()` source. The
  capabilities say `screen.frame_in`: wait for the app's frames instead of
  opening a picker.
- **Android:** the app asks for a MediaProjection and hands it over
  (`AndroidContext::set_media_projection`); then there is one screen
  source, and `pick_screen` answers it without a picker.

## Using it

```toml
[dependencies]
kessel-video = "0.1"
```

This runs as it stands, with no hardware: the fake backend's camera, its
structural H.264 encoder and its decoder. With `system()` in place of the
fake, the same code uses the OS's camera and codecs.

```rust
use kessel_video::fake::{read_frame_code, FakeBackend, FakeConfig};
use kessel_video::{no_wake, CaptureConstraints, DecoderConfig, EncodeOptions, EncodedInput, EncoderConfig, VideoBackend, VideoCodec};
use std::time::Duration;

fn next<T>(mut f: impl FnMut() -> Option<T>) -> T {
    loop {
        if let Some(t) = f() {
            return t;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = FakeBackend::new(FakeConfig::default());
    let mut camera = backend.open_camera(None, CaptureConstraints::new(1280, 720, 30), no_wake())?;
    let mut encoder = backend.encoder(EncoderConfig::h264(640, 360, 30, 600_000), no_wake())?;
    let mut decoder = backend.decoder(DecoderConfig::new(VideoCodec::H264), no_wake())?;

    let frame = next(|| camera.try_recv());
    let code = read_frame_code(&frame).expect("a coded frame");
    encoder.submit(&frame, EncodeOptions::default())?;
    let encoded = next(|| encoder.poll())?;

    // Your transport would encrypt, packetise, send, receive and decrypt here.
    decoder.submit(EncodedInput::new(VideoCodec::H264, encoded.data, encoded.keyframe, encoded.capture_us))?;
    let decoded = next(|| decoder.poll())?;
    assert_eq!(read_frame_code(&decoded), Some(code));
    Ok(())
}
```

### Frames from the host

A `frame_in()` source takes frames the host already has: from an iPhone's
broadcast upload extension (through the app), from the host's own capture,
or in tests. Each push carries how far the frame must turn to stand upright.

```rust
use kessel_video::{frame_in, no_wake, now_us, FrameInOptions, FrameSource, PixelFormat, Rotation, VideoFrame};

let (sender, mut source) = frame_in(FrameInOptions::new(1280, 720, 15), no_wake())?;
// A frame lying on its side: 360 x 640, to turn a quarter clockwise.
let frame = VideoFrame::filled(PixelFormat::Nv12, 360, 640, [128, 128, 128], now_us())?;
sender.push(frame, Rotation::Deg90)?;
let upright = source.try_recv().expect("queued at once");
assert_eq!((upright.width, upright.height), (640, 360));
# Ok::<(), kessel_video::VideoError>(())
```

### Errors

`VideoError` is an enum with stable string forms (`no_permission`,
`device_busy`, `device_lost`, `not_found`, `unsupported`, `no_resources`,
`busy`, `os`, `shut_down`, `invalid`). Decoders report `DecodeError`:
`need_keyframe`, `busy`, `corrupt`, `session_lost`; all but `busy` mean
"ask the sender for a keyframe".

## The fake backend

`fake::FakeBackend` runs everything without hardware, from a `FakeConfig`
script: cameras and screens, hot-plug steps, permission answers (including a
prompt that answers after a delay), source and machine events, an encoder
instance limit, and encoder faults (`Busy`, a lost session).

Its frames carry a **frame code**: a 32-bit counter drawn as an 8 x 4 grid of
black and white blocks in the frame's top-left quarter, each block 1/16 of the
frame's width and height. `fake::read_frame_code` reads it back after
scaling and lossy coding.

Its codec is **structural H.264**: well formed but not decodable by a real
decoder. Each access unit is SPS + PPS (keyframes only) and one slice NAL
that holds the frame code, sized to the target bitrate. It follows every
bitstream rule above, so packetiser, encryption and RTP tests run on any
machine. Its decoder returns a grey frame with the code drawn on it, at the
size the slice names, up to 8192 x 8192 (larger is `corrupt`). It is a test
tool, so don't give it untrusted input.

## Platform notes

On Linux:
- Screen capture needs xdg-desktop-portal with a desktop backend and
  PipeWire at run time. libpipewire is the system's own, loaded with dlopen:
  nothing is linked or needed at build time, and a system without it reports
  no screen capture.
- Camera access comes from the user's groups or the session's device ACLs;
  there is no prompt.

On macOS:
- A bundled app needs `NSCameraUsageDescription` in its Info.plist (and
  `com.apple.security.device.camera` under the hardened runtime). Without it
  `camera_permission()` answers `Denied` instead of letting the OS end the
  app for asking.
- Listing displays and windows needs Screen Recording permission.
  `pick_screen` (macOS 14+) doesn't: the person's choice in the system picker
  is the permission. It runs the picker on the main thread itself, so it can
  be called from any thread, but it needs the app's main run loop.
- ScreenCaptureKit is loaded at run time, so the app still starts on systems
  without it.

On iOS:
- `system()` gives the Apple backend. The app needs `NSCameraUsageDescription`
  in its Info.plist; without it `camera_permission()` answers `Denied`
  instead of letting iOS end the app for asking. Ask with
  `request_camera_permission`; a denied camera makes `open_camera` answer
  `no_permission`.
- `open_camera(None, ...)` opens the front camera. `CaptureConstraints::facing`
  picks the front or back camera, and `cameras()` lists both with their
  `facing`. Modes are chosen with the asked box turned landscape (the
  sensor's way), and frames come out upright and unmirrored: held upright, a
  1280 x 720 box gives 720 x 1280 frames. Mirroring the self-view is the
  app's choice, in its own preview.
- iOS stops the camera while the app is in the background, during a phone
  call, when another app takes the camera, and under heavy thermal load. The
  source stays open and sends `SourceEvent::Interrupted` (with
  `Interruption::Background`, `InUseByAnother` or `Thermal`), then
  `Resumed` when the camera comes back; frames simply pause in between.
  `subscribe()` also gets `Background(true)` and `Background(false)`.
  Opening a camera or an H.264 encoder while the app is in the background
  answers `shut_down`. An H.264 encoder whose session iOS takes away ends
  with `shut_down` (open a new one in front); an H.264 decoder reports
  `session_lost` (call `reset`). VP8 needs no OS session and keeps working
  while the app runs.
- Screens come in through frame-in: sharing an iPhone's screen needs a
  broadcast upload extension, a separate process (with a 50 MB memory cap)
  that the person starts from the system's picker. The extension hands its
  frames to the app, which pushes them into a `frame_in()` source with each
  frame's turn and timestamp. The capabilities say `screen.supported` and
  `screen.frame_in`; `screen_permission()` is `Granted` (the person's start
  is the consent); `pick_screen` and `open_screen` answer `unsupported`,
  and `screen_sources()` is empty. A frame-in source keeps running while
  the app is in the background. How the frames cross from the extension to
  the app is the app's choice; kessel-video doesn't carry them. The
  `kessel-frame-ring` crate is one way: a shared-memory ring the extension
  writes through a small C surface and the app reads in Rust.
- `pixel_buffer_frame` wraps a CVPixelBuffer the app already has (NV12 or
  BGRA) as a native frame: upright, VideoToolbox encodes it without a copy.

On Android:
- Make the backend with `system(config)`, with `config.android` set to an
  `AndroidContext` from the host's JNI handles:
  `unsafe { AndroidContext::new(vm, context) }`, where `vm` is the
  `JavaVM*` and `context` a global reference to the Activity or Application
  (in a Tauri or winit app, `ndk_context::android_context()` has both).
  Keep a clone of the context.
- The app declares `<uses-permission android:name="android.permission.CAMERA" />`
  and `<uses-feature android:name="android.hardware.camera" android:required="false" />`
  in its manifest, and asks for CAMERA at run time itself (in Tauri, through
  its permission API): asking needs an Activity's result callback, which is
  Java. `camera_permission()` reads the state through JNI (`NotDetermined`
  without the handles); `request_camera_permission()` reports it without a
  prompt; a denied open answers `NoPermission`.
- Call `AndroidContext::set_background(true)` when the app pauses and
  `false` when it resumes. Cameras close in the background (Android takes
  them from background apps) and report `Interrupted(Background)`, then
  reopen and report `Resumed`. Call `set_device_orientation` when the device
  turns: frames are rotated upright for the sensor and the turn. Size boxes
  are orientation-free: 1280x720 also fits a portrait 720x1280 frame.
- A camera another app takes reports `Interrupted(InUseByAnother)`; it
  reopens on the next resume, or when the caller unpauses the source.
- Screen capture runs from a MediaProjection the app hands over with
  `unsafe { ctx.set_media_projection(projection, width, height, dpi) }`
  (a JNI global reference, and the display's size and density now). The
  app's part is Java or Kotlin: the consent (`createScreenCaptureIntent`),
  a foreground service of type `mediaProjection` running for the whole
  share (with `FOREGROUND_SERVICE_MEDIA_PROJECTION` on Android 14+), and a
  `MediaProjection.Callback` registered before the handover (Android 14+
  throws otherwise) whose `onStop` calls `ctx.media_projection_stopped()`.
  Then `screen_sources()` lists one display, `pick_screen` answers it, and
  `open_screen` makes a `VirtualDisplay` into an `ImageReader` sized to the
  asked box (turned to the screen's shape). Call `ctx.set_screen_size` when
  the display turns: the virtual display is resized and frames come at the
  new shape (`Started`). Frames are NV12. The screen keeps running while the
  app is in the background; when the projection stops, the source ends with
  `Ended(SourceClosed)`. Android allows one virtual display per projection.
  Needs API 26 (`ANativeWindow_toSurface`).
- `hardware_buffer_frame` wraps an AHardwareBuffer the app already has
  (RGBA or RGBX on API 26+, YUV 4:2:0 on API 29+) as a native frame for a
  `frame_in()` source or an encoder.
- H.264 encoding uses the device's MediaCodec AVC encoder with CPU (NV12 or
  I420) input; `ImplPreference::SoftwareOnly` picks Android's own software
  encoder. Keyframe requests and bitrate changes need API 26; on API 24 and
  25 a keyframe request restarts the codec and the bitrate stays as opened.
  There is no H.264 decoder yet; VP8 decodes in Rust.

## Testing

| Command | Runs |
|---|---|
| `cargo test` | The unit tests and the README's example. On a Mac, also the VideoToolbox H.264 tests. From the repository, also `tests/` (the VP8 end-to-end test and the licence and publish checks), which is not in the published package. |
| `cargo test -- --ignored` | Also the tests that need a real camera, a desktop session or a person to answer a permission prompt or a picker. Run by hand; see the platform table for what has been run. |

None of the default tests need a camera, screen or GPU. In the repository,
`scripts/check-targets.sh` type-checks Windows, iOS and Android, or any
target you name, from a machine that can't run them, and
`scripts/check-package.sh` builds the package as crates.io would and runs
its tests outside the repository. A library build never links, so
`scripts/check-apple-link.sh` and `scripts/check-android-link.sh` link
`examples/link_check.rs` as apps do (a static library with no
`-framework` flags of its own on macOS and iOS, a shared library with no
undefined symbols allowed on Android) and run it on macOS.

## Building

- Needs Rust 1.87 or later (zbus 5's minimum, for the Linux backend; the
  `windows` crate needs 1.82).
- No system libraries or headers at build time on any platform. The OS
  backends reach the OS through system frameworks (macOS), OS DLLs through
  the `windows` crate (Windows), raw ioctls, D-Bus in pure Rust and a
  run-time dlopen of the system's libpipewire (Linux), and the NDK's system
  libraries `libcamera2ndk` and `libmediandk`, declared in Rust and linked
  from the NDK's own stubs like any Android app (Android). No build script
  and no C is compiled.
- Apps that link Rust as a static library (Tauri on iOS, where Xcode links
  `libapp.a`) need no extra frameworks for kessel-video. rustc doesn't pass
  `#[link]` frameworks to that link, so the Apple backend also writes them
  into its object code as linker options, as Clang does for `@import`:
  AVFoundation, CoreFoundation, CoreMedia, CoreVideo, Foundation and
  VideoToolbox. An app built with `-ignore_auto_link`, or an older linker
  that skips these options, must list those six itself.
  ScreenCaptureKit and CoreGraphics (macOS screens) are opened at run time.
- Features: `system` (default) is the OS backend for the target plus `vp8`.
  `apple`, `windows`, `linux` and `android` pick backends one by one; `vp8` adds the
  Rust VP8 codecs. With `--no-default-features` you get the API, `null()`
  and the fake backend (with structural H.264 only), with no OS code and no
  `unsafe`.
- `deny.toml` lists the licence allowlist; `tests/licences.rs` checks the
  same rules without needing cargo-deny, and that no C codec library or its
  bindings are in the tree.

## Security

kessel-video's decoders take frames that arrive from the network, so treat a
crash, hang or memory blow-up on a crafted frame as a security bug. Please
report it privately, not in a public issue, through GitHub's private
vulnerability reporting: <https://github.com/Alakazam-211/K2/security>, then
"Report a vulnerability". The repository's SECURITY.md has the policy.

### Untrusted input

What parses data from the network, and how it is tested:

- **VP8 frames** (`VideoDecoder` for `VideoCodec::Vp8`, on every backend)
  go to `vp8-decoder`, which has no `unsafe` code and returns errors for
  malformed frames. It refuses frames larger than 4096x2304, so a crafted
  header can't make it allocate a huge frame.
- **The `h264` helpers** (`annexb_nals`, `avcc_nals`, `unescape_rbsp` and
  the rest) are safe Rust and check every length.
- **H.264 decoding** is not offered by any real backend yet, so no H.264
  from the network reaches a decoder in this crate.
- **Not fuzzed.** kessel-video has no coverage-guided fuzzer. A seeded
  random test sends 3,000 hostile frames (random bytes; valid VP8 frames
  with bits flipped, truncated, extended, spliced, with a rewritten header
  or claiming a random size) through the VP8 decoder, with a valid keyframe
  every 40 so deltas reach the inter-prediction paths, and checks that every
  answer is an error or a well-formed frame within the size limit. Another
  500 go through the decoder's own thread, which must still decode a valid
  keyframe afterwards. 5,000 hostile inputs go through the `h264` helpers.
  It runs the same inputs every time. vp8-decoder's own README says how it
  is tested (the WebM conformance vectors).

### Unsafe code

- **kessel-video's own `unsafe`** is only in the OS backends' FFI, in three
  modules: `apple` (Objective-C runtime, CoreFoundation, CoreMedia,
  CoreVideo, VideoToolbox and AVFoundation calls, and the run-time load of
  ScreenCaptureKit), `win` (Media Foundation, Direct3D 11 and Windows
  Graphics Capture through the `windows` crate) and `linux` (V4L2 ioctls,
  mmap, a file descriptor from the portal, and libpipewire through dlopen).
  The rest of the crate is `deny(unsafe_code)`: the API, the frame and pixel
  code, conversion, the H.264 helpers, the VP8 glue, the fake and null
  backends. The decode path for VP8 has no `unsafe` of its own.
- **vp8-decoder and vp8-encoder** are `forbid(unsafe_code)`.
- objc2, block2, the `windows` crate, libc, zbus and crossbeam-queue use
  `unsafe` internally, as FFI and lock-free queue crates do.

### System libraries only

Every C library is reached as the system's own copy, never vendored, built
or statically linked: Apple's frameworks and Windows' DLLs are linked
dynamically, and libpipewire is opened at run time with dlopen
(`libpipewire-0.3.so.0`). No dependency compiles C: there is no `cc`,
`cmake` or `pkg-config` build step in the tree (`windows-sys` and
`linux-raw-sys` are Rust declarations, not C).

## Licence

kessel-video's own code is MIT OR Apache-2.0, at your option. It contains no
third-party code, so it has no NOTICE file.

With the `vp8` feature (on by default), it uses `vp8-decoder` and
`vp8-encoder`, which are `(MIT OR Apache-2.0) AND BSD-3-Clause`: they port
code from RFC 6386's reference decoder. **If you distribute binaries** that
include them, BSD-3-Clause requires you to reproduce their notice, which is
in each crate's `NOTICE` file. Tools such as `cargo about` collect them. The
rest of the dependency tree is MIT, Apache-2.0, BSD, ISC, Zlib or
Unicode-3.0, checked by `deny.toml` and `tests/licences.rs` in the
repository.

### Patents

- **Our own code:** no patent licence is granted beyond Apache-2.0's for our
  own contributions.
- **H.264:** kessel-video ships no H.264 implementation. It hands frames to
  the operating system's own codec (VideoToolbox on macOS and iOS), so the OS
  vendor's licence for that codec applies. The fake backend's structural
  H.264 only wraps a frame code in H.264's NAL unit framing; it encodes no
  picture.
- **VP8:** VP8 comes from the vp8-encoder and vp8-decoder crates. Their
  READMEs cover VP8 patents (in short: Google's VP8 patent grant covers only
  Google's own code; you can join Google's free VP8 Patent Cross-License
  yourself).
