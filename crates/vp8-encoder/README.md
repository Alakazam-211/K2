# vp8-encoder

A real-time VP8 video encoder in pure, safe Rust (`#![forbid(unsafe_code)]`),
for live camera and screen video over WebRTC. It writes standard RFC 6386
VP8, which every browser decodes.

- **One frame in, at most one frame out:** no look-ahead, no reordering.
- **Rate control to a target bitrate.** `set_rates` changes it at any
  time, without a keyframe. Frames are skipped (`Ok(None)`) when the
  stream runs far over budget. Easy content comes in under a high
  target, by design: the quantizer has a floor (index 4, the best
  quality), so a mostly flat or static picture, like a test pattern,
  cannot spend all the bits. Camera content lands on the target.
- **Keyframes on request:** `request_keyframe()` or the `force_keyframe`
  flag, for PLI and FIR. Optionally, also on a fixed interval.
- **Temporal layers:** L1T2 and L1T3, in the usual WebRTC pattern. Each
  frame reports `temporal_id` and `layer_sync` (RFC 7741's Y bit).
- **Probabilities don't carry over:** each frame's probability updates
  apply to that frame only, so a lost or dropped frame never leaves a
  receiver with the wrong probabilities. A lost *reference* frame still
  breaks the frames that predict from it: the receiver needs a keyframe
  (PLI) then.

For simulcast, run one encoder per layer, each at its own size, and make
the smaller pictures with `scale_i420`.

```rust
use vp8_encoder::{EncodeError, Encoder, EncoderConfig, Picture};

// Your transport: an RFC 7741 packetiser, encryption, RTP.
fn send(_vp8_frame: &[u8], _keyframe: bool, _temporal_id: u8, _layer_sync: bool) {}

fn publish(i420_frames: &[Vec<u8>]) -> Result<(), EncodeError> {
    let mut enc = Encoder::new(EncoderConfig::new(1280, 720, 30, 2_000_000))?;
    for i420_bytes in i420_frames {
        let picture = Picture::from_i420(i420_bytes, 1280, 720).ok_or(EncodeError::InvalidPicture)?;
        let force_keyframe = false; // true after a PLI or FIR
        if let Some(frame) = enc.encode(&picture, force_keyframe)? {
            send(&frame.data, frame.keyframe, frame.temporal_id, frame.layer_sync);
        }
    }
    Ok(())
}
```

`frame.data` is one raw VP8 frame, starting at the frame tag: what an
RFC 7741 packetiser takes.

**Sizes and bad input.** Any size from 1x1 to 16383x16383 works, odd sizes
included (chroma is `(width + 1) / 2` by `(height + 1) / 2`). The encoder
doesn't panic on bad input; it returns an error:
- `Picture::from_i420` returns `None` if the buffer is shorter than an I420
  picture of that size;
- `encode` returns `EncodeError::SizeMismatch` for a picture that isn't the
  configured size, and `EncodeError::InvalidPicture` if its planes are too
  short for their strides;
- `Encoder::new` returns `EncodeError::InvalidConfig` for a zero or
  oversized dimension, or a zero frame rate or bitrate.

## Speed

Single-threaded, with no SIMD, on a 720p camera clip at 2 Mbit/s:
- 47 fps on one Ryzen 7 3800X core;
- 124 fps on an Apple M4.

That is real time at 720p30 with room to spare. 360p runs at 178 fps on
the same Ryzen core.

## How it is checked

- The encoder reconstructs each frame with vp8-decoder's own prediction,
  transforms and loop filter (vp8-decoder matches the WebM project's VP8
  test vectors bit for bit). Debug builds, which the tests use, also decode
  every frame and check that the decoder's reference frames equal the
  encoder's, bit for bit.
- The tests check picture quality (PSNR), keyframe requests,
  rate changes and the temporal-layer patterns. They also check that
  receivers which drop the upper layers still decode.
- Bitrate accuracy: on synthetic camera-like content at 640x360 and
  1280x720, from 300 kbit/s to 2.5 Mbit/s, with and without temporal
  layers, the stream lands within 12% of the target
  (`tests/bitrate.rs`). `scripts/rate-check.sh` runs the same ladder on a
  real camera clip.
- The encoded streams were decoded by libvpx (via ffmpeg) and by Chrome's
  WebCodecs, byte-identical to vp8-decoder's output
  (`scripts/libvpx-crosscheck.sh`, `scripts/chrome-check.sh`).

## Licence

`(MIT OR Apache-2.0) AND BSD-3-Clause`. This crate builds on
[vp8-decoder](https://crates.io/crates/vp8-decoder) (same team), which is a
port of RFC 6386's reference decoder. The encoder's own design is
described in PROVENANCE.md.

NOTICE, with the RFC code's BSD-3-Clause notice, ships in the crate.
**If you distribute binaries** that include this crate, you must reproduce
that notice.

**Patents:** no patent licence is granted beyond Apache-2.0's for our own
contributions. Google's VP8 patent grant covers only Google's own code.
Users who want patent cover can join Google's free VP8 Patent
Cross-License themselves.

Report security problems privately through GitHub's "Report a
vulnerability" on [Alakazam-211/K2](https://github.com/Alakazam-211/K2/security).
