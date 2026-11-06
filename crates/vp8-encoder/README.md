# vp8-encoder

A real-time VP8 video encoder in pure, safe Rust (`#![forbid(unsafe_code)]`),
for live camera and screen video over WebRTC. It writes standard RFC 6386
VP8, which every browser decodes.

- **One frame in, at most one frame out:** no look-ahead, no reordering.
- **Rate control to a target bitrate.** `set_rates` changes it at any
  time, without a keyframe. Frames are skipped (`Ok(None)`) when the
  stream runs far over budget.
- **Keyframes on request:** `request_keyframe()` or the `force_keyframe`
  flag, for PLI and FIR. Optionally, also on a fixed interval.
- **Temporal layers:** L1T2 and L1T3, in the usual WebRTC pattern. Each
  frame reports `temporal_id` and `layer_sync` (RFC 7741's Y bit).
- **Simulcast:** run one encoder per layer at its own size, and make the
  smaller pictures with `scale_i420`.
- **Loss tolerant:** each frame's probability updates apply to that frame
  only, so a lost or dropped frame never leaves a receiver with the wrong
  probabilities.

```rust
use vp8_encoder::{Encoder, EncoderConfig, Picture};

let mut enc = Encoder::new(EncoderConfig::new(1280, 720, 30, 2_000_000))?;
let picture = Picture::from_i420(&i420_bytes, 1280, 720).expect("a 1280x720 I420 buffer");
if let Some(frame) = enc.encode(&picture, false)? {
    send(&frame.data, frame.keyframe, frame.temporal_id, frame.layer_sync);
}
```

`frame.data` is one raw VP8 frame, starting at the frame tag: what an
RFC 7741 packetiser takes.

## How it is checked

- Every encoded frame is decoded by vp8-decoder, which matches the WebM
  project's VP8 test vectors bit for bit. The encoder's reference frames
  are that decoder's output, so encoder and receiver cannot drift apart.
- The tests check picture quality (PSNR), the bitrate, keyframe requests,
  rate changes and the temporal-layer patterns. They also check that
  receivers which drop the upper layers still decode.
- The encoded streams were decoded by libvpx (through ffmpeg, the decoder
  Chrome uses) and give byte-identical pictures.

## Licence

`(MIT OR Apache-2.0) AND BSD-3-Clause`. This crate builds on vp8-decoder,
which is a port of RFC 6386's reference decoder; the BSD-3-Clause notice
is in NOTICE. If you distribute binaries, reproduce it. The encoder's own
design is described in PROVENANCE.md.

**Patents:** no patent licence is granted beyond Apache-2.0's for our own
contributions. Google's VP8 patent grant covers only Google's own code.
Users who want patent cover can join Google's free VP8 Patent
Cross-License themselves.

Report security problems privately through GitHub's "Report a
vulnerability" on [Alakazam-211/K2](https://github.com/Alakazam-211/K2/security).
