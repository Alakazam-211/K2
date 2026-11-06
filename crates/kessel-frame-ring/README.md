# kessel-frame-ring (placeholder name)

A single-producer, single-consumer ring of NV12 video frames in a
shared-memory file, for handing frames from one process to another on the
same machine. Made for an iPhone's screen share: the broadcast upload
extension writes each screen frame in, and the app reads it out and encodes
it. Pure Rust on `std` and `libc` (declarations only: no C is built, no
build script), Unix only. Not published; the name is a placeholder.

## How it works

- **The app (consumer) creates the ring** in a file both processes can open
  (the App Group container), with a fixed `Layout`: the largest frame (either
  way round), 3 slots, at most 15 fps. 1280 x 720 is about 4 MB. The file is
  built under a temporary name and renamed into place, so the extension never
  sees a half-made ring. Dropping the consumer marks it gone and removes the
  file.
- **The extension (producer) maps it read-write** and writes each frame into
  the slot after the newest, scaled down to fit the box (a box filter, so
  text stays readable). It never blocks and allocates nothing per frame.
- **Each frame carries its own** width, height, rotation (the clockwise turn
  to upright), colour (BT.601 or BT.709, limited or full range) and
  timestamp. A turn or a new size is just the next frame; only the capacity
  is fixed.
- **Torn frames are never read.** Each slot has a sequence word (a seqlock):
  odd while being written, even when whole. The newest frame's slot is
  published only after the write; a reader checks the sequence before and
  after its copy.
- **Control words** in the header: the producer's state (waiting, running,
  ended with a reason), the consumer's wishes (wants frames, please stop,
  gone, the highest rate) and a heartbeat the consumer bumps on every read.
  The producer holds an advisory lock (`flock`) while attached, so the app
  can tell an extension that was killed (`ProducerState::Gone`) from a quiet
  one.

## The producer's C surface (for Swift)

The extension links the crate as a static library and calls these through a
bridging header. They are Rust functions with the C calling convention.

```c
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct KfrProducer KfrProducer;

// Attach to the ring the app made at `path`. Null on failure, with *error:
// -1 invalid, -2 no ring (the app isn't in a call), -3 another producer,
// -4 not a matching ring, -5 an OS error.
KfrProducer *kfr_producer_attach(const char *path, int32_t *error);
// Before the first push.
int32_t kfr_producer_start(KfrProducer *p);
// One frame: planes 0 and 1 of a locked 420v/420f CVPixelBuffer.
// rotation: 0, 90, 180, 270 (clockwise to upright). color: 0 BT.601 limited,
// 1 BT.709 limited (420v), 2 BT.709 full (420f). timestamp_ns: on
// kfr_monotonic_ns's clock, or 0 for now. Returns 0 written, 1 dropped (the
// app doesn't want frames now), 2 dropped (too soon for the rate), or < 0.
int32_t kfr_producer_push_nv12(KfrProducer *p,
                               const uint8_t *y, size_t y_len, size_t y_stride,
                               const uint8_t *uv, size_t uv_len, size_t uv_stride,
                               uint32_t width, uint32_t height,
                               uint32_t rotation, uint32_t color,
                               uint64_t timestamp_ns);
// False: skip locking and copying pixel buffers.
bool kfr_producer_wants_frames(const KfrProducer *p);
// True: the call ended in the app; finish the broadcast.
bool kfr_producer_stop_requested(const KfrProducer *p);
// reason: 1 the person stopped (broadcastFinished), 2 closed, 3 failed.
// Frees p.
void kfr_producer_finish(KfrProducer *p, uint32_t reason);
// The clock timestamps use: mach absolute time in ns (CMClock host time).
uint64_t kfr_monotonic_ns(void);
```

In `processSampleBuffer(_:with: .video)`: if `kfr_producer_wants_frames` is
false, return. Otherwise lock the pixel buffer read-only, pass plane 0 and 1
(`CVPixelBufferGetBaseAddressOfPlane`, `GetBytesPerRowOfPlane`, and
`bytesPerRow x GetHeightOfPlane` as the length), the rotation from
`RPVideoSampleOrientationKey`, the colour from the pixel format, and the
presentation time on the host clock in ns; unlock. Poll
`kfr_producer_stop_requested` (on each frame and on a timer) and finish the
broadcast when it is true. Call `kfr_producer_finish(p, 1)` in
`broadcastFinished`.

## The consumer (the app, in Rust)

```rust,ignore
let mut ring = Consumer::create(container.join("screen.ring"), Layout::hd720())?;
let mut buf = Vec::new();
// Every ~10 ms while sharing:
match ring.producer()? {
    ProducerState::Ended(reason) => { /* end the share */ }
    ProducerState::Gone => { /* the extension died: end the share */ }
    _ => {}
}
if let Some(info) = ring.try_read(&mut buf) {
    // buf is tight NV12: Y at 0 (stride info.y_stride()), UV at
    // info.uv_offset() (stride info.uv_stride()).
}
```

`tests/frame_in_vp8.rs` has the whole glue into a kessel-video frame-in
source (`pump`), with the timestamp mapped to the source's clock.

## Tests

`cargo test -p kessel-frame-ring`: the producer in a second process (the
test binary runs itself again), a producer killed mid-write, a racing
producer that never hands over a torn frame, wraparound, files that don't
match, bad frames, the control words, scaling, and the C surface; then the
whole path through kessel-video's frame-in and VP8.

Licensed MIT OR Apache-2.0.
