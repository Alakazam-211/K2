# Changelog

## 0.1.1

### vp8-decoder 0.1.1

- Pins vp8-decoder `=0.1.1`, whose faster loop filter and inter
  prediction this encoder shares for its own reconstruction. The output is
  the same byte for byte, so encoded streams don't change.
- `examples/decode_speed` (in the repository): vp8-decoder's speed on
  camera-like streams made by this encoder at several sizes and bitrates,
  with a checksum of the decoded frames. The camera-like content moves
  from `tests/bitrate.rs` to `tests/common/camera.rs` so both use it.

### Documentation

- Easy content comes in under the target bitrate, by design. Rate control
  never goes below quantizer index 4 (its best quality), so a mostly flat
  or static picture, such as a test pattern, cannot spend a high target:
  kessel-video's fake camera pattern at 640x360 makes about 700 kbit/s at
  a 1.2 Mbit/s target, with every frame at q 4. Camera content lands on
  the target. The `bitrate_bps` docs and the README say so.

### Tests

- `tests/bitrate.rs`: bitrate accuracy on synthetic camera-like content
  (a panning scene with detail at every scale, a moving object and sensor
  noise) at 640x360 and 1280x720, at 300 kbit/s, 600 kbit/s, 1.2 Mbit/s and
  2.5 Mbit/s, with and without temporal layers: within 12% of the target
  either way. On kessel-video's fake pattern it checks the stream never
  overshoots, and undershoots only with the quantizer at its floor.
- `scripts/rate-check.sh`: the same ladder on a real camera clip,
  downloaded for the run. `examples/encode_y4m` gains `--loop` (plays a
  short clip forward and back) and prints the quantizer range and the
  bitrate after the first second.

## 0.1.0

- First release.
