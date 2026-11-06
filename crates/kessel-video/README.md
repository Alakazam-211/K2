# kessel-video

Cameras, screen capture, pixels and video codecs in Rust, for apps that
carry live video over their own transport.

It stands alone: it needs no other Kessel crate. It is the video sibling of
[`kessel-audio`](https://crates.io/crates/kessel-audio). Licensed MIT OR
Apache-2.0.

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
| Codecs | Codec-neutral: VP8 as raw RFC 6386 frames, H.264 as one Annex B access unit with 4-byte start codes and SPS and PPS before every IDR. Real time: no reordering, no B-frames, one output per input at most, `set_rates` never forces a keyframe, `capture_us` passes through. H.264 comes only from the operating system's codecs. No codec library is linked or shipped. |
| Pixels | I420, NV12, BGRA and RGBA, with conversion and scaling on the CPU (`convert`), and OS-native buffers (`NativeFrame`) that pass from a source to an encoder of the same backend without a copy. |
| Capability query | Per codec, direction and implementation: hardware or software, the largest size and rate, how many may be open at once, temporal layers, H.264 profiles and level, pixel formats. |

### Platforms

| | Cameras | Screens | Encode | Decode |
|---|---|---|---|---|
| macOS | AVFoundation | ScreenCaptureKit (12.3+): list, system picker (14+), window exclusion | VP8 (Rust); H.264 (VideoToolbox) | VP8 (Rust) |
| Windows | Media Foundation (NV12 through the source reader) | Windows Graphics Capture: displays and windows, the system picker (pass the app's HWND with `PickOptions::with_parent_window`); no window exclusion (Windows can only hide our windows from every capture) | VP8 (Rust) | VP8 (Rust) |
| Linux | V4L2 through raw ioctls (NV12, I420, YUYV modes; MJPEG not yet) | the ScreenCast portal (its picker is the only way to choose) with a PipeWire stream; no window exclusion | VP8 (Rust); no H.264 (no OS codec) | VP8 (Rust) |

VP8 comes from the pure-Rust `vp8-encoder` and `vp8-decoder` crates (feature
`vp8`, part of `system`) in every backend, the fake included: real time, L1T2
and L1T3 temporal layers, one encoder per simulcast layer. Every platform has the API, `null()` and the fake backend today.

On Linux:
- Screen capture needs xdg-desktop-portal with a desktop backend and
  PipeWire at run time. libpipewire is the system's own, loaded with dlopen:
  nothing is linked or needed at build time, and a system without it reports
  no screen capture. `scripts/try-linux-screen.sh` tries it by hand on a
  desktop.
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
machine. Its decoder returns a grey frame with the code drawn on it.

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

## Errors

`VideoError` is an enum with stable string forms (`no_permission`,
`device_busy`, `device_lost`, `not_found`, `unsupported`, `no_resources`,
`busy`, `os`, `shut_down`, `invalid`). Decoders report `DecodeError`:
`need_keyframe`, `busy`, `corrupt`, `session_lost`; all but `busy` mean
"ask the sender for a keyframe".

## Testing

`cargo test` runs everything, with no camera, screen or GPU needed. On a
Mac the H.264 tests use VideoToolbox.

`deny.toml` lists the licence allowlist; `tests/licences.rs` checks the same
rules without needing cargo-deny, and also checks that no C codec library or
its bindings are in the tree.

## Licence

MIT OR Apache-2.0, at your option.
