# Changelog

## 0.1.1

Hardening of the producer's side. No change to the API or the file format.

- The producer treats the whole ring file as untrusted, as the consumer
  already did: it keeps its own state rather than relying on values read
  back from the file, holds the rate to the range a layout allows, and
  refuses a file larger than any ring before mapping it.
- The producer checks the file's length before each use of the mapping.
  When the file has shrunk, `Producer::push` returns `RingError::Mismatch`
  (-4 from `kfr_producer_push_nv12`: finish the broadcast),
  `wants_frames` and `stop_requested` return false, and ending the
  producer leaves the file alone.
- More hostile-input tests on the producer's side, and a `producer`
  cargo-fuzz target in the repository.

## 0.1.0

The first release.

- `Consumer` (the app's side): creates the ring file under a fixed
  `Layout`, reads the newest frame (`try_read`, `read_latest`) as tight
  NV12 with its `FrameInfo` (size, rotation, colour, timestamp), reports
  the producer's state (`Waiting`, `Running`, `Ended(reason)`, `Gone`),
  and sets whether frames are wanted, the highest rate, and a stop request.
- `Producer` (the extension's side): attaches to the ring, scales each
  frame down to the box with a box filter, writes it into a slot under a
  seqlock and publishes it; never blocks; drops frames the consumer
  doesn't want or that come too soon.
- The producer's `extern "C"` surface (`kfr_*`) for an app extension that
  links the crate as a static library.
- Hardening: the consumer treats the whole file as untrusted and checks
  every frame against its own layout (`RingStats::rejected` counts what it
  skips); the C surface checks every size and stride and never unwinds
  into the caller; the ring file is created exclusively. See the README's
  trust boundary.
- `Color`, `EndReason` and `ProducerState` are `#[non_exhaustive]`, like
  `Layout`, `FrameInfo`, `RingStats`, `Pushed` and `RingError`.
