# kessel-frame-ring

A single-producer, single-consumer ring of NV12 video frames in a
shared-memory file, for handing frames from one process to another on the
same machine. It was made for screen sharing from an iPhone: the broadcast
upload extension writes each screen frame in, and the app reads it out and
encodes it. Pure Rust on `std` and `libc` (declarations only: no C is built,
no build script). Unix only: macOS, iOS, Linux and Android. On Windows the
crate builds (its types only) but the ring itself is unsupported.

Kessel is Alakazam Labs' family of open-source Rust crates for real-time
apps. kessel-frame-ring stands alone: it needs no other Kessel crate. Its
frames suit [`kessel-video`](https://crates.io/crates/kessel-video)'s
frame-in sources, but any encoder will do. API docs are on
[docs.rs](https://docs.rs/kessel-frame-ring), and the source is at
<https://github.com/Alakazam-211/K2> (`crates/kessel-frame-ring`). Licensed
MIT OR Apache-2.0.

**Version 0.1** is the first release. Until 1.0, a minor release (0.2, 0.3)
may change the API or the file format, as Cargo allows for 0.x versions; a
patch release (0.1.x) won't. The app and its extension must use the same
format version (`VERSION`): a producer refuses a ring of another version.
Each change is listed in CHANGELOG.md.

## Platforms

| Platform | Status |
|---|---|
| Linux (x86-64) | The test suite passes, including two real processes and a producer killed mid-write. |
| macOS (Apple Silicon) | The test suite passes. |
| iOS | **Type-checked, and the static library is checked** (members and symbols). **Never run on an iPhone**: a broadcast upload extension needs a device and a person to start it. |
| Android | **Type-checked only.** |
| Windows | **Unsupported.** Type-checked so a cross-platform app still builds: only the types are there, no ring. |

## How it works

- **The app (consumer) creates the ring** in a file both processes can open
  (the App Group container), with a fixed `Layout`: the largest frame
  (either way round), 3 slots, at most 15 fps. 1280 x 720 is about 4 MB.
  The file is built under a fresh temporary name (created exclusively,
  never through a symbolic link) and renamed into place, so the extension
  never sees a half-made ring. Dropping the consumer marks it gone and
  removes the file.
- **The extension (producer) maps it read-write** and writes each frame
  into the slot after the newest, scaled down to fit the box (a box filter,
  so text stays readable). It never blocks and allocates nothing per frame.
  iOS caps a broadcast upload extension at about 50 MB of memory; the ring
  is mapped from the file rather than allocated, and a 720p ring is about
  4 MB of it.
- **Each frame carries its own** width, height, rotation (the clockwise
  turn to upright), colour (BT.601 or BT.709, limited or full range) and
  timestamp. A turn or a new size is just the next frame; only the capacity
  is fixed.
- **Torn frames are never read.** Each slot has a sequence word (a
  seqlock): odd while being written, even when whole. The newest frame's
  slot is published only after the write; a reader checks the sequence
  before and after its copy.
- **Control words** in the header: the producer's state (waiting, running,
  ended with a reason), the consumer's wishes (wants frames, please stop,
  gone, the highest rate) and a heartbeat the consumer bumps on every read.
  A consumer that hasn't looked for 2 seconds (the app suspended) counts as
  away: the producer drops frames without copying them until it reads
  again (or calls `Consumer::touch`).
  The producer holds an advisory lock (`flock`) while attached, so the app
  can tell an extension that was killed (`ProducerState::Gone`) from a
  quiet one.

## The extension's side: the C surface (for Swift)

The extension links the crate as a static library (`crate-type` includes
`staticlib`) and calls these through a bridging header. They are Rust
functions with the C calling convention; no C is compiled.

```c
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Not thread-safe: make every call on one producer from one serial queue,
// and kfr_producer_finish last.
typedef struct KfrProducer KfrProducer;

// Attach to the ring the app made at `path`. Null on failure, with *error:
// -1 invalid, -2 no ring (the app isn't in a call), -3 another producer,
// -4 not a matching ring, -5 an OS error, -6 an internal error.
KfrProducer *kfr_producer_attach(const char *path, int32_t *error);
// Before the first push. 0, or -1 (p is null) or -6 (an internal error).
int32_t kfr_producer_start(KfrProducer *p);
// One frame: planes 0 and 1 of a locked 420v/420f CVPixelBuffer.
// rotation: 0, 90, 180, 270 (clockwise to upright). color: 0 BT.601 limited,
// 1 BT.709 limited (420v), 2 BT.709 full (420f). timestamp_ns: on
// kfr_monotonic_ns's clock, or 0 for now. Returns 0 written, 1 dropped (the
// app doesn't want frames now), 2 dropped (too soon for the rate), -1 for
// any argument that doesn't describe a frame, -4 when the ring's file has
// shrunk under the extension (finish the broadcast), or another code below 0.
int32_t kfr_producer_push_nv12(KfrProducer *p,
                               const uint8_t *y, size_t y_len, size_t y_stride,
                               const uint8_t *uv, size_t uv_len, size_t uv_stride,
                               uint32_t width, uint32_t height,
                               uint32_t rotation, uint32_t color,
                               uint64_t timestamp_ns);
// False: skip locking and copying pixel buffers. False too once the ring's
// file has shrunk.
bool kfr_producer_wants_frames(const KfrProducer *p);
// True: the call ended in the app; finish the broadcast. (A shrunk file
// gives false here; the next push returns -4.)
bool kfr_producer_stop_requested(const KfrProducer *p);
// reason: 1 the person stopped (broadcastFinished), 2 closed, 3 failed.
// Frees p. Call it last, once.
void kfr_producer_finish(KfrProducer *p, uint32_t reason);
// The clock timestamps use: mach absolute time in ns (CMClock host time).
uint64_t kfr_monotonic_ns(void);
```

In `processSampleBuffer(_:with: .video)`: if `kfr_producer_wants_frames` is
false, return. Otherwise lock the pixel buffer read-only, pass plane 0 and
1 (`CVPixelBufferGetBaseAddressOfPlane`, `GetBytesPerRowOfPlane`, and
`bytesPerRow x GetHeightOfPlane` as the length), the rotation from
`RPVideoSampleOrientationKey`, the colour from the pixel format, and the
presentation time on the host clock in ns; unlock. Poll
`kfr_producer_stop_requested` (on each frame and on a timer) and finish the
broadcast when it is true. Call `kfr_producer_finish(p, 1)` in
`broadcastFinished`.

### Safety rules for the C surface

- **One queue.** A producer is not thread-safe. Make every call on one
  producer from one serial dispatch queue (or otherwise one at a time).
  ReplayKit may call `processSampleBuffer` and `broadcastFinished` on
  different queues, so hop to your queue in both. A push that races
  `kfr_producer_finish` uses freed memory.
- **Finish last, once.** After `kfr_producer_finish` returns, the pointer
  is gone; don't pass it again. Null is ignored everywhere.
- **Truthful lengths.** `y` and `uv` must be readable for `y_len` and
  `uv_len` bytes for the whole call (keep the pixel buffer locked). Sizes,
  strides, rotation and colour are checked, and anything that doesn't
  describe a frame inside those lengths returns -1; the lengths themselves
  can't be checked.
- **No panics cross the boundary.** Each function catches a panic and
  returns its error value instead: `kfr_producer_attach` null with error
  -6, `kfr_producer_start` and `kfr_producer_push_nv12` -6, the two `bool`
  functions false, `kfr_monotonic_ns` 0, and `kfr_producer_finish` nothing.
  An extension built with `panic = "abort"` aborts instead, as Rust
  requires.

## The app's side (Rust)

The ring's path comes from the app: on iOS, the App Group container
(`FileManager.containerURL(forSecurityApplicationGroupIdentifier:)` in
Swift), handed to the Rust side, and the same path to the extension. A
`Consumer` is `Send` but not `Sync`: keep it on one thread, or behind a
lock.

```rust,ignore
let mut ring = Consumer::create(container.join("screen.ring"), Layout::hd720())?;
let mut buf = Vec::new();
// Every ~10 ms while sharing:
match ring.producer() {
    Ok(ProducerState::Ended(reason)) => { /* end the share */ }
    Ok(ProducerState::Gone) => { /* the extension died: end the share */ }
    Err(_) => { /* the file shrank or the OS failed: end the share */ }
    Ok(_) => {}
}
if let Some(info) = ring.try_read(&mut buf) {
    // buf is tight NV12: Y at 0 (stride info.y_stride()), UV at
    // info.uv_offset() (stride info.uv_stride()). info.width and
    // info.height fit the layout's box and info.rotation is a quarter turn.
    // The timestamp is the extension's: map it with saturating maths.
}
```

The repository's `tests/frame_in_vp8.rs` has the whole glue into a
kessel-video frame-in source, with the timestamp mapped to the source's
clock.

## Trust boundary

The extension is a separate process with the ring's file open read-write,
so the app treats everything in the file as untrusted: a bug, a kill
mid-write or a compromised extension can leave any bytes there. What the
consumer guarantees, whatever the file holds:

- **No panic, no out-of-range read, no overflow**, except the shrink race
  below (a fault, not a wrong read). The slot index, size,
  rotation, colour and length of a frame are checked against the
  consumer's own layout (never the file's copy) before a byte is copied,
  and all size maths is checked. A frame handed out fits the box (either
  way round), has a rotation of 0, 90, 180 or 270, a known colour, and
  exactly `info.len()` bytes. A slot that fails is skipped and counted
  (`RingStats::rejected`).
- **A torn frame is never handed out** (the seqlock), and a stray frame
  number can't stop later frames: any number other than the last one read
  is a new frame.
- **The layout is the consumer's own.** `Consumer::layout()` never reads
  the file.

What it can't guarantee:

- **Pixels, timestamps and frame numbers are whatever the producer wrote.**
  They are only numbers and bytes: use saturating or checked maths on the
  timestamp, and treat the pixels as you would any untrusted image.
- **A producer can shrink the file.** Touching a mapped page past the end
  of a file kills the process with SIGBUS, and Apple platforms have no file
  seals to forbid shrinking. The consumer checks the file's length before
  each use of the mapping and then reports nothing to read
  (`Consumer::producer` returns `RingError::Mismatch`: end the share), but
  a shrink that lands between that check and the copy still faults. Keep
  the file in a container only your own app and extension can open.
- **A producer can deny service:** stop writing, write nonstop, or claim to
  be running. The app decides when a share has gone on too long.

The other way round, the app also has the file open read-write, so the
extension treats everything in it as untrusted too. What the producer
guarantees, whatever the file holds before it attaches or while it runs:

- **No panic, no overflow, no write outside its slot**, except the shrink
  race below. The layout is read once, at attach, and checked against
  itself and the file's length; a file larger than any ring is refused
  before it is mapped. After that the producer writes by its own copy of
  the layout and never reads the file's again.
- **The producer keeps its own counts.** Each slot's sequence word and the
  newest slot are tracked by the producer, so nothing it reads back from
  the file is added to: a sequence word it didn't write is overwritten
  with its own, and a newest slot or frame number out of range is ignored.
  The counters it bumps in the file (written, dropped, attaches) wrap.
- **The app's words are hints.** Wants frames, stop and the heartbeat only
  decide whether a frame is dropped, and the rate is held to 0 to 240 fps.
- **A frame it says it wrote is whole in the file**, with its sequence
  word even and the header's newest-frame word naming it, unless the app
  writes over it afterwards.

What it can't guarantee: **the app can shrink the file** too. The producer
checks the file's length before each use of the mapping, then refuses
pushes (`RingError::Mismatch`; -4 from the C surface: finish the
broadcast) and answers false to `wants_frames` and `stop_requested`; its
end and drop leave the file alone. A shrink between that check and the
write still faults, as on the consumer's side.

On iOS, the system ends a suspended process that holds a file lock in a
shared container (exception code `0xdead10cc`). The extension holds its
lock while attached and the app holds one for a moment in
`Consumer::producer`; broadcast extensions aren't normally suspended
mid-broadcast, but this hasn't been tried on a device. Call
`kfr_producer_finish` as the broadcast ends, and don't call
`Consumer::producer` as the app goes to the background.

## Tests and fuzzing

`cargo test -p kessel-frame-ring` runs the producer in a second process
(the test binary runs itself again), a producer killed mid-write, a racing
producer that never hands over a torn frame, wraparound, files that don't
match, bad frames, the control words, scaling and the C surface. Its
hostile-input tests run 20,000 seeded rounds of corrupt, torn and hostile
rings (random and edge-value header and slot words, slots that claim
anything, random frame bytes, shrunk files) and check that the consumer
never panics and hands out only frames that fit. The producer gets the
same: 20,000 seeded rounds of a ring scribbled on between pushes
(sequence words at the top of their range, a newest slot past the layout,
wild rates, a rewritten layout, shrunk files, producers attaching again)
and a second thread scribbling during pushes, checking that every frame
the producer says it wrote is whole in the file. Producers attach to
corrupt files, and the C surface gets bad sizes, strides and lengths.
`KFR_HOSTILE_ITERS` and `KFR_HOSTILE_SEED` change the rounds and the seed.

In the repository, `fuzz/` has three cargo-fuzz targets (`consumer`: a
ring the producer's side scribbles on; `producer`: a ring the consumer's
side scribbles on between pushes; `attach`: a producer opening any file),
`tests/frame_in_vp8.rs` runs the whole path through kessel-video's frame-in
and VP8, `scripts/check-staticlib.sh` checks that the static library holds
only this crate, libc's declarations and Rust's standard library and
exports exactly the C surface above, `scripts/check-targets.sh`
type-checks iOS, Android and Windows (types only) from any machine,
`scripts/check-fuzz.sh` builds and runs the fuzz targets,
`scripts/check-semver.sh` checks the API against a published version, and
`scripts/check-package.sh` builds
the package as crates.io would and runs its tests outside the repository.

## Security

Both sides read memory another process writes, so treat a crash, hang or
out-of-range access on a crafted ring file as a security bug, on either
side, and the same for the C surface on arguments within its safety rules. Please report
it privately, not in a public issue, through GitHub's private
vulnerability reporting: <https://github.com/Alakazam-211/K2/security>, then
"Report a vulnerability". The repository's SECURITY.md has the policy.

## Licence

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`)
or the MIT licence (`LICENSE-MIT`), at your option.
