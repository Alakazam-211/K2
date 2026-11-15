# Changelog

## 0.2.0 (2026-11-15)

### Breaking

- `ProcessingConfig` has a new field, `capture_stage`, and is now
  `#[non_exhaustive]`: build it from `ProcessingConfig::default()` or
  `ProcessingConfig::off()` and set fields, or use the new `with_` methods,
  instead of a struct literal. This is the last field addition that breaks.
- `ProcessingConfig` is no longer `Clone` or `PartialEq`, and no longer
  `Sync`, `UnwindSafe` or `RefUnwindSafe`: it holds the stage, a
  `Box<dyn CaptureStage>`, and `CaptureStage` is only `Send + 'static`. It
  is still `Send`. Its `Debug` output shows the stage's latency.
  (`EngineConfig`'s auto traits are unchanged: its null device's closures
  already made it `Send` only.)
- `EngineConfig` is now `#[non_exhaustive]` too: build it from
  `EngineConfig::default()` or `EngineConfig::null(device)` and chain the new
  `with_backend`, `with_processing`, `with_capture_stage`, `with_encoder`,
  `with_jitter`, `with_initial_ts48k`, `with_playout_buffer_ms`,
  `with_capture_queue_frames`, `with_recv_queue_frames`,
  `with_max_participants`, `with_levels_interval_ms`,
  `with_speaking_threshold_dbov`, `with_device_poll_interval` and
  `with_processing_poll`.
- `Stats` has a new field, `capture_stage`, and `Event` a new variant,
  `CaptureStage`; both were already `#[non_exhaustive]`, so these are not
  breaking. cargo-semver-checks 0.51.0 against 0.1.0 (run as if this were a
  minor release) finds exactly the breaks above: `struct_marked_non_exhaustive`
  (`ProcessingConfig`, `EngineConfig`), `derive_trait_impl_removed`
  (`ProcessingConfig`: `Clone`, `PartialEq`) and `auto_trait_impl_removed`
  (`ProcessingConfig`: `Sync`, `UnwindSafe`, `RefUnwindSafe`).

### Added

- **The capture stage**: `ProcessingConfig::capture_stage` /
  `with_capture_stage(Box<dyn CaptureStage>)`, the caller's own processing
  (a noise-removal model, for example) after the high-pass filter, echo
  cancellation and noise suppression and before AGC2 and its limiter.
  Levels, VAD and DTX read the signal after the stage and AGC2.
  `CaptureStage` is `process(&mut [f32; 480])` (10 ms of 48 kHz mono, in
  place), a fixed `latency_samples()` (at most `MAX_STAGE_LATENCY_SAMPLES`,
  one second) and `reset()`.
- The stage's budget, enforced on the processing thread: a strike is 2
  consecutive calls of 9 ms or more, or a 99th percentile over 6 ms across
  the last 100 calls (one late call alone, usually the thread being
  preempted, is not). On a strike the stage is bypassed: the echo
  canceller's output goes on, delayed by the stage's latency (a
  preallocated delay line), so the timing never shifts. It is retried after
  10 s of capture (reset, then warmed for its latency). 3 strikes within
  60 s of capture make the bypass permanent; otherwise the count decays. A
  panic (in `process` or `reset`) or a NaN or infinite sample is a fault:
  bypassed for good, no retry. A panic is only caught with
  `panic = "unwind"` (the default); with `panic = "abort"` it ends the
  process.
- `Stats::capture_stage: Option<CaptureStageStats>`: `active`, the sticky
  `bypassed: Option<StageBypassReason>` (`Budget` or `Fault`), `strikes`,
  `permanent`, `latency_samples`, block counts and the longest and 99th
  percentile call times. `Event::CaptureStage` is sent when `active`,
  `bypassed`, `strikes` or `permanent` change.
- `reset()` is called once on the processing thread before the first
  `process()` (when the engine builds its processing core, before audio
  flows; a stage may allocate there), when the capture stream reopens (a
  device change), after a capture gap of more than a second (a suspend, for
  example) and before a retry; never twice with no `process()` between.
- The step-down floor, `ProcessingConfig::stage_fallback_noise_suppression`
  (default `Some(Moderate)`): with a stage and `noise_suppression: None`,
  sonora's noise suppression stands in while the stage is bypassed (budget
  or fault) and stops when a retry brings the stage back. The suppressor
  runs on every block in its own sonora instance (so it is converged when
  needed), delayed to the stage's latency; each switch crossfades over one
  10 ms block. `CaptureStageStats::fallback_noise_suppression` says when it
  stands in. Neither the switching nor the suppressor allocates per block
  (counted). Its own delay (334 samples) comes out of the stage's latency,
  so the timing holds for stages with at least 7 ms of latency. Measured
  cost (60 s of duplex, release build, x86-64): a pass-through stage with
  noise suppression off and the floor on costs 50–54 µs per 10 ms block
  more than 0.1.0's full chain (0.51–0.54% of a core), against 2–7 µs
  (0.02–0.07%) for the hook alone with the same processing. Turn the floor
  off with `with_stage_fallback_noise_suppression(None)`.
- `ProcessingConfig::with_noise_suppression`, `with_echo_cancellation`,
  `with_gain_control`, `with_high_pass` and
  `with_stage_fallback_noise_suppression`.

### Timestamps

- `EncodedFrame::ts48k` does not include the stage's latency: the stamps are
  exactly what they are without a stage (and through a bypass). The audio in
  a frame is `latency_samples` older than its stamp;
  `CaptureStageStats::latency_samples` says how much.

### Unchanged

- With no stage set, the pipeline is bit for bit what 0.1.0 runs: one sonora
  instance with the same configuration. With a stage, echo cancellation and
  gain control run in two sonora instances with the stage between them.
- The stage path allocates nothing after construction (tested with a
  counting allocator, including bypasses, retries, resets and faults).

## 0.1.0

### Added

- `AudioEngine::suspend()` and `resume()`, for every backend: a call on hold
  (or interrupted, or without audio focus) produces no frames and plays
  silence, and still decodes its participants. The far end hears a pause, not
  loss: a frame of encoded silence (marked `dtx`) ends the talkspurt, and the
  first frame after `resume` carries a `ts48k` advanced by the suspended time
  in whole frames, like the end of a DTX gap. The frame interface is
  unchanged.
- On iOS and Android, `Backend::OsVoiceProcessing(OsVoiceConfig)`: the OS's own
  echo cancellation, noise suppression and gain control. iOS uses the
  VoiceProcessingIO audio unit (coreaudio-rs); Android uses AAudio with the
  voice-communication input preset and usage (`ndk`, API 28 or later).
  `OsEffects` says what the OS covers, and sonora runs only the rest. The
  backend never configures or activates the OS's audio session.
- On iOS and Android: `AudioEngine::start_when_session_active()` (the device
  waits for the session), `session_activated()`, `session_deactivated()` and
  `route_changed()`, which reopens the device at a new rate (Bluetooth
  hands-free runs at 16 kHz) without ending the call.
- The `os-voice` feature, on by default; it does nothing on other targets.
- docs.rs builds for iOS and Android too, so the phone API is documented.

### Packaging

- Ready for crates.io: `repository` is set, `publish = false` is gone, and
  `include` ships only `src/`, the README and the licences.
- The README has a platforms table (tested or only type-checked), a
  security contact and the licence notes for sonora and ropus.
- sonora and ropus are caret requirements (`0.2.0`, `0.12.18`), not exact
  pins, so patch and security releases reach library users.
- The README also covers patents, `unsafe` code and fuzzing, Android's
  missing-permission error, API stability for 0.x, and a consistent
  baseline column in the quality table.
- A seeded hostile-input test sends random and mutated packets, and
  nonsense sequence numbers, timestamps and arrival times, through the
  decoder, jitter buffer and mixer.
- `scripts/check-targets.sh` type-checks the iOS and Android targets (or any
  other) on a toolchain that can't add targets.

### Unchanged

- Desktop backends and their defaults behave as before.
