# vp8-decoder

A VP8 video decoder in pure, safe Rust (`#![forbid(unsafe_code)]`, no
dependencies), following RFC 6386.

It decodes every bitstream version (0 to 3), with:
- keyframes and interframes;
- all intra modes and all inter modes, split motion vectors included;
- segmentation;
- the normal and simple loop filters;
- multiple token partitions;
- probability updates, saved or kept;
- the last, golden and alt-ref references, with sign bias and buffer copies;
- hidden frames.

It matches the WebM project's VP8 test vectors bit for bit (see Testing).

Input is one raw VP8 frame at a time, starting at the frame tag: no IVF,
WebM or RTP framing. That is what an RTP depacketiser (RFC 7741) produces.
Output is a borrowed I420 picture. A small IVF reader is included.

```rust
let mut decoder = vp8_decoder::Decoder::new();
let frame: &[u8] = next_frame(); // one raw VP8 frame
match decoder.decode(frame) {
    Ok(Some(picture)) => { /* picture.y, .u, .v with their strides */ }
    Ok(None) => { /* a hidden frame: references updated, nothing to show */ }
    Err(vp8_decoder::DecodeError::NeedKeyframe) => { /* drop until a keyframe */ }
    Err(_) => { /* corrupt: ask the sender for a keyframe */ }
}
```

After any error the decoder waits for the next keyframe. A real-time
receiver should send a keyframe request (PLI) then.

The scaling bits in the keyframe header are reported in `FrameInfo` and
otherwise ignored, as RFC 6386 Section 9.1 allows: pictures come out at
the coded size.

## Testing

```sh
scripts/fetch-test-vectors.sh /tmp/vp8-vectors
VP8_TEST_VECTORS=/tmp/vp8-vectors cargo test -p vp8-decoder --release --features conformance
```

The script downloads the WebM project's vectors. These are
`vp80-00-comprehensive-001` to `018`, plus the intra, inter,
segmentation, partitions, sharpness and small-size sets. It also downloads
their per-frame MD5 files, and checks every file against
`tests/vectors.sha256`. The vectors are test data and are not in this
repository.

The test decodes every frame and compares the MD5 of each shown frame's
I420 data with the published one.

## Licence and provenance

This crate is a port of the reference decoder in RFC 6386 Section 20.
Its licence is `(MIT OR Apache-2.0) AND BSD-3-Clause`:
- the RFC code's BSD-3-Clause notice is in NOTICE;
- our own code is MIT OR Apache-2.0.

PROVENANCE.md records exactly which sources were used.
