# Provenance

How this crate was written, recorded truthfully so its licensing can be
checked.

## The encoder's design is our own

RFC 6386 specifies only decoding. Everything that is an encoder decision
was designed for this crate:
- the forward DCT and Walsh-Hadamard transforms, derived as the transposes
  of the RFC's inverse transforms;
- quantization with rounding offsets;
- mode decision by SAD plus a rate term from the bitstream's own
  probabilities;
- the motion search: diamond, then half- and quarter-pixel refinement;
- probability updates fitted to each frame's token counts;
- rate control: a per-frame quantizer model with a leaky buffer and frame
  skipping;
- the temporal-layer patterns;
- the picture scaler.

No encoder source code was consulted: not libvpx, and not any other VP8
encoder.

## What it shares with vp8-decoder

The bitstream must be exactly what a decoder expects, so the encoder reuses
vp8-decoder's code (a dependency, not a copy) for:
- the boolean coder;
- the probability, tree and quantizer tables;
- intra and inter prediction;
- the inverse transforms;
- the near-MV search;
- the decoder itself, which decodes each encoded frame to produce the
  reference frames.

vp8-decoder is a port of the RFC 6386 Section 20 reference decoder; see its
PROVENANCE.md. Because this crate links that code, its licence is
`(MIT OR Apache-2.0) AND BSD-3-Clause`, and NOTICE carries Section 20.26's
BSD-3-Clause text. libvpx source was not opened for either crate.

## Outside tools used in testing

ffmpeg with libvpx is used only as an outside test tool, never as a
dependency:
- to make reference streams;
- to check that libvpx's decoder (the one in Chrome) decodes this
  encoder's output byte for byte the same as vp8-decoder does.
