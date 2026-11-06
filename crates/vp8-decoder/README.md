# vp8-decoder

A VP8 video decoder in pure Rust (no dependencies, no C), following
RFC 6386. Safe code throughout, except for small NEON (aarch64) and SSE2
(x86_64) loop-filter kernels (see Security).

It decodes every bitstream version (0 to 3), with:
- keyframes and interframes;
- all intra modes and all inter modes, split motion vectors included;
- segmentation;
- the normal and simple loop filters;
- multiple token partitions;
- probability updates, saved or kept;
- the last, golden and alt-ref references, with sign bias and buffer copies;
- hidden frames.

It matches the WebM project's 59 VP8 test vectors bit for bit (see Testing).

## Using it

Input is one raw VP8 frame at a time, starting at the frame tag: no IVF,
WebM or RTP framing. That is what an RTP depacketiser (RFC 7741) produces.

```rust
use vp8_decoder::{DecodeError, Decoder};

// Your app: draw a frame, or ask the sender for a keyframe (PLI).
fn show(_i420: Vec<u8>, _width: u32, _height: u32) {}
fn request_keyframe() {}

fn receive(frames: &[Vec<u8>]) {
    let mut decoder = Decoder::new();
    for frame in frames {
        // One raw VP8 frame, as an RTP depacketiser hands it over.
        match decoder.decode(frame) {
            Ok(Some(picture)) => show(picture.to_i420(), picture.width, picture.height),
            Ok(None) => {} // a hidden frame: references updated, nothing to show
            Err(DecodeError::NeedKeyframe) => {} // dropped until a keyframe
            Err(_) => request_keyframe(), // corrupt
        }
    }
}
```

- **`Picture` borrows the decoder.** It points into the decoder's own frame
  buffer, so it lives until the next call to `decode`. To keep a frame,
  copy it with `Picture::to_i420()` or `write_i420()`. It is I420 at the
  frame's display size; chroma is `(width + 1) / 2` by `(height + 1) / 2`.
- **The size comes from the stream:** every keyframe carries it, and the
  decoder reallocates when it changes. `Decoder::dimensions()` returns it.
- **Errors** (`DecodeError`):
  - `NeedKeyframe`: an interframe before the first keyframe, or after an error;
  - `Truncated`: the frame is shorter than its headers say;
  - `Invalid`: the frame is malformed;
  - `Unsupported`: a bitstream version above 3.

  After any error the decoder waits for the next keyframe, and a real-time
  receiver should ask for one (PLI).
- **`frame_info()`** reads a frame's uncompressed header without decoding it:
  keyframe flag, version, whether it is shown, and a keyframe's size.
- **The scaling bits** in the keyframe header are reported in `Dimensions`
  and otherwise ignored, as RFC 6386 Section 9.1 allows. Pictures come out
  at the coded size.
- **`ivf::IvfReader`** reads IVF files (the container the test vectors use)
  frame by frame.

## Testing

The conformance test needs a checkout of the repository: `scripts/` and
`tests/` are not in the published crate.

```sh
crates/vp8-decoder/scripts/fetch-test-vectors.sh /tmp/vp8-vectors
VP8_TEST_VECTORS=/tmp/vp8-vectors cargo test -p vp8-decoder --release --features conformance
```

The script downloads the WebM project's vectors (`vp80-00-comprehensive-001`
to `018`, plus the intra, inter, segmentation, partitions, sharpness and
small-size sets) and their per-frame MD5 files. It checks every file
against `tests/vectors.sha256`. The vectors are test data and are not in
this repository.

The `conformance` feature only switches on that test, which decodes every
frame and compares the MD5 of each shown frame's I420 data with the
published one. It needs `VP8_TEST_VECTORS` and fails, never skips, without
it. A plain `cargo test -p vp8-decoder` runs the unit tests only.

## Security

The decoder is meant for untrusted input: malformed frames return errors
rather than panicking.

Its only `unsafe` code is in two modules, `neon.rs` (aarch64) and
`sse2.rs` (x86_64): the loop filter's SIMD kernels, written with
`std::arch` intrinsics. The rest of the crate is `deny(unsafe_code)`. Each
kernel works on a slice that ordinary, bounds-checked indexing has already
cut to the exact size it reads and writes, so a bad stream can't make it
touch memory outside the frame. Their output matches the portable code
byte for byte (tests compare them with the 0.1.0 scalar filter on random
pictures at every filter level and sharpness). Build with
`RUSTFLAGS="--cfg vp8_portable"` to leave them out; the decoder is then
entirely safe Rust, and about half as fast on aarch64.

Please report a crash
or hang on a malicious stream privately, through GitHub's "Report a
vulnerability" on the Security tab of
[Alakazam-211/K2](https://github.com/Alakazam-211/K2/security).

## Licence, provenance and patents

This crate is an independent port of the reference decoder in RFC 6386
Section 20. Its licence is `(MIT OR Apache-2.0) AND BSD-3-Clause`:
- the RFC code's BSD-3-Clause notice is in NOTICE;
- our own code is MIT OR Apache-2.0.

**If you distribute binaries** that include this crate, you must reproduce
the BSD-3-Clause notice in NOTICE, as that licence requires.

**Patents:** no patent licence is granted beyond Apache-2.0's for our own
contributions. Google's VP8 patent grant covers only Google's own code.
Users who want patent cover can join Google's free VP8 Patent
Cross-License themselves.

PROVENANCE.md records exactly which sources were used.
