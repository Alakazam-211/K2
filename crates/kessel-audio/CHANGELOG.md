# Changelog

## 0.2.5

Additions only: code written against 0.2.4 builds and behaves unchanged
(the default mode is 0.2.4's set-up).

### Added

- **A scheduling-mode knob for the processing thread:**
  `EngineConfig::dsp_sched` (`DspSchedConfig`, set with
  `EngineConfig::with_dsp_sched`), read once as the thread starts. Its
  `DspSchedMode` (`#[non_exhaustive]`) is one of:
  - `Workgroup` (the default): 0.2.4's set-up. The QoS class, the
    time-constraint policy, and on iOS the audio unit's I/O workgroup while
    the unit runs.
  - `TimeConstraintOnly`: the QoS class and
    `thread_policy_set(THREAD_TIME_CONSTRAINT_POLICY)`, and no workgroup.
  - `None`: a plain thread, as before 0.2.4 (still woken by the capture
    callback).
  - `OwnWorkgroup`: the QoS class and the time-constraint policy, then an
    audio work interval of the thread's own (`AudioWorkIntervalCreate`,
    macOS 11 and iOS 14 or later, looked up at run time), joined. Each pass
    with capture audio is one interval: `os_workgroup_interval_start` with a
    deadline `DspSchedConfig::interval_deadline` (3 ms by default) after it
    starts, `os_workgroup_interval_update` for each further block of a
    backlog, `os_workgroup_interval_finish` after the last.

  `DspSchedConfig::time_constraint` (`DspTimeConstraint`: period,
  computation, constraint) sets the policy's figures; the default is 0.2.4's
  10 ms / 6 ms / 9 ms. `AudioEngine::start` refuses figures that can't work
  (`invalid_config`).
- **Switching modes without a rebuild:** `DspSchedMode` parses from its
  `as_str()` names (`"workgroup"`, `"time_constraint_only"`, `"none"`,
  `"own_workgroup"`). `DspSchedConfig::from_env()` reads
  `KESSEL_AUDIO_DSP_SCHED` and, in whole microseconds,
  `KESSEL_AUDIO_DSP_INTERVAL_DEADLINE_US`, `KESSEL_AUDIO_DSP_PERIOD_US`,
  `KESSEL_AUDIO_DSP_COMPUTATION_US` and `KESSEL_AUDIO_DSP_CONSTRAINT_US`;
  `DspSchedConfig::from_lookup` reads the same keys from an app's own
  settings. The engine never reads the environment by itself.
- **Fallbacks:** in `TimeConstraintOnly` and `OwnWorkgroup`, a set-up step
  the OS refuses (the time-constraint policy; creating, joining or running
  the work interval) falls back to `Workgroup`, and says why
  (`DspSchedFallback`, `#[non_exhaustive]`: `TimeConstraintRefused`,
  `OwnWorkgroupUnavailable`, `OwnWorkgroupJoinRefused`, `IntervalRefused`).
  Nothing on the processing thread panics.
- `Stats::dsp_sched` (`DspSchedStats`, `#[non_exhaustive]`):
  `mode_requested`, `mode_effective`, `fallback_reason`, `time_constraint`
  (the figures asked for), `interval_deadline` (`OwnWorkgroup`) and
  `intervals` (work intervals finished). A change of mode is published at
  once. Off macOS and iOS, `mode_effective` is `DspSchedMode::Unsupported`
  and nothing is asked of the OS.
- `DspScheduling::OwnWorkgroup`: the thread joined its own work interval.

## 0.2.4

### Changed

- **On macOS and iOS the processing thread (`kessel-audio-dsp`) runs
  real-time.** It was a default-QoS thread that slept 2 ms between passes,
  and on a phone the OS could move it to an efficiency core or a low clock:
  a capture stage's CPU time per block could climb several times over
  shortly after the start of a call. Now, as it starts, it asks for:
  1. the QoS class `QOS_CLASS_USER_INTERACTIVE`;
  2. then Mach's time-constraint policy (`THREAD_TIME_CONSTRAINT_POLICY`):
     a 10 ms period (one block), 6 ms of computation and a 9 ms constraint
     (the capture stage's percentile and hard budgets);
  3. on iOS with `Backend::OsVoiceProcessing`, once the audio unit runs,
     it joins the unit's I/O thread's OS workgroup
     (`kAudioOutputUnitProperty_OSWorkgroup`, `os_workgroup_join`; iOS 14
     or later), and leaves it when the unit closes. Only a thread that got
     the time-constraint policy joins.

  Whatever the OS refuses leaves the thread on the step before (an older
  iOS has no workgroups: the thread stays on the time-constraint policy).
  The workgroup calls are looked up at run time, so the crate still runs on
  systems older than macOS 11 and iOS 14.
- **On macOS and iOS the capture callback wakes the processing thread**,
  instead of it only polling: it processes each block as it arrives, in
  step with the audio I/O thread. It still wakes at least every
  `EngineConfig::processing_poll` (2 ms by default) for playback and the
  paths without capture.
- Linux, Android and Windows: no change; the thread keeps the OS's default
  scheduling and polls.

### Added

- `DspScheduling` (`#[non_exhaustive]`): how the OS schedules the processing
  thread: `Workgroup`, `TimeConstraint`, `Qos` or `None` (the default:
  other platforms, or everything refused); `as_str()` for logs
  (`"workgroup"`, `"time_constraint"`, `"qos"`, `"none"`).
- `Stats::dsp_scheduling` and `CaptureStageStats::dsp_scheduling` (the same
  value), so call logs show it next to the stage's timings. A change (the
  thread joining or leaving a workgroup) is published at once.

## 0.2.3

### Changed

- **Behaviour change: the capture stage's time budget is measured on the
  processing thread's CPU time by default**, not the wall clock. On a busy
  device (a phone with the camera and a video encoder running) the OS
  preempts the processing thread mid-call, and 0.2.2 counted that wait
  against the stage: a fast stage could be bypassed again and again for
  the OS's work. Now only the time the stage's own thread runs counts
  (`clock_gettime(CLOCK_THREAD_CPUTIME_ID)` on macOS, iOS, Linux and
  Android). Real starvation is still caught: a call that takes 20 ms or more
  on the wall clock counts as slow toward the 2-in-a-row rule whatever its
  CPU time. Windows keeps the wall clock (`GetThreadTimes` counts in 15.6 ms
  ticks, too coarse for the 6 ms budget). **To get 0.2.2's timing back:**
  `ProcessingConfig::with_stage_clock(StageClock::Wall)`. Otherwise
  additions only: code written against 0.2.2 builds unchanged.

### Added

- `StageClock` (`#[non_exhaustive]`): `ThreadCpu` and `Wall`.
  `StageClock::default()` is `ThreadCpu` where the platform supports it,
  else `Wall`; `StageClock::is_supported(self) -> bool`.
- `ProcessingConfig::stage_clock` / `with_stage_clock(StageClock)`: the
  clock that times the stage. Asking for `ThreadCpu` where it isn't
  supported times by the wall clock.
- `ProcessingConfig::stage_starvation_cutoff` /
  `with_stage_starvation_cutoff(Duration)`: with `ThreadCpu`, a call this
  long or longer on the wall clock counts as slow toward the 2-in-a-row
  rule (not the percentile rule) whatever its CPU time. Default
  `DEFAULT_STAGE_STARVATION_CUTOFF` (20 ms: two blocks, more than twice the
  9 ms hard budget); at least 9 ms (less is refused with
  `invalid_config`); `Duration::MAX` leaves CPU time alone to decide. The
  grace window excuses starved calls like any other slow call.
- New `CaptureStageStats` fields, so nothing is hidden: `clock` (the clock
  in use), `max_wall_us` (the longest call on the wall clock),
  `preempted_over_budget` (calls over 6 ms on the wall clock that weren't in
  CPU time) and `starved` (calls past the cutoff). With `ThreadCpu`,
  `max_block_us` and `p99_block_us` are CPU time.
- A dependency on `libc` (MIT OR Apache-2.0) on macOS, iOS, Linux and
  Android, for the thread CPU clock. Declarations only: no C is built.

## 0.2.2

### Changed

- **Behaviour change:** with a capture stage and the default config, slow
  calls in the first 1.5 s of capture after the stage starts (and after each
  capture stream start or restart) no longer strike. A stage that is too
  slow from its first call is bypassed about 1.5 s later than in 0.2.1. Set
  `stage_grace` to `Duration::ZERO` for 0.2.1's timing. Otherwise additions
  only: code written against 0.2.1 builds unchanged.
- The package no longer ships the crate's unit tests and their helpers
  (`src/tests/`, `src/testkit.rs`, `src/phone/tests.rs`, about 130 KB): they
  are `#[cfg(test)]` and were never built from the package.

### Added

- **A grace window for the capture stage's time budget.** OS work the stage
  doesn't control (the call starting, the audio session activating, a route
  change, the camera starting, the app changing scene) can make a few calls
  slow, and two slow calls in a row used to bypass the stage for 10 s. For
  a short window after such an event, slow calls don't count toward a
  strike:
  - `AudioEngine::transient_event(TransientEvent)`: the host reports the
    event, opening the window or extending the open one. Never blocks.
    `TransientEvent` (`#[non_exhaustive]`): `CallStart`,
    `AudioSessionActivated`, `RouteChange`, `CameraStart`,
    `AppSceneChange`, `Other`.
  - `ProcessingConfig::stage_grace` / `with_stage_grace(Duration)`: the
    window's length, in capture, rounded up to 10 ms blocks. Default
    `DEFAULT_STAGE_GRACE` (1.5 s), at most `MAX_STAGE_GRACE` (10 s; more is
    refused with `invalid_config`); `Duration::ZERO` turns it off.
  - The engine opens the window itself when the stage starts and whenever
    the capture stream starts or restarts (a device opening or reopening,
    the end of a capture gap). On the phone backend, `session_activated()`
    and `route_changed()` open it too.
  - Nothing is hidden: the slow calls are still timed and reported.
    `CaptureStageStats` has new fields: `grace` (a window is open now),
    `grace_blocks`, `grace_over_budget` (calls over 6 ms that the window
    excused) and `last_transient`. `max_block_us` and `p99_block_us` include
    the window's calls. `Event::CaptureStage` is also sent when a window
    that excused slow calls closes.
  - Faults are unchanged: a panic or a NaN in the window bypasses the stage
    for good, at once.
- `homepage` in the manifest: the crate's folder in the repository.
- An issue form for the crate, linked from the README (Issues, next to the
  private security line).

### Documented

- Known behaviour, unchanged: the phone backend's raw mode
  (`PhoneMode::Raw`) runs no noise suppression, so the step-down floor
  (`stage_fallback_noise_suppression`) never runs there. A noise-removing
  stage on raw capture that is bypassed leaves the signal unsuppressed
  until its retry. Now said in the README and on the field's docs.

## 0.2.1 (2026-11-15)

Additions only; nothing changes for code written against 0.2.0.

### Added

- **Raw phone capture** (iOS and Android), for "noise removal off" on a call:
  `PhoneMode::Raw(RawCapture)` (or `PhoneMode::raw()`) turns the OS's voice
  processing off, so the far end hears the full microphone signal,
  background noise and all, and keeps the call echo-free:
  - iOS opens the plain RemoteIO unit instead of VoiceProcessingIO;
  - Android opens the input with AAudio's unprocessed preset when
    `RawCapture::unprocessed_supported` says the device supports it (the
    app reads `AudioManager.PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED`), else
    the voice-recognition preset, and with no audio session, so no
    `NoiseSuppressor` or other `AudioEffect` can attach; playback is
    unchanged;
  - sonora runs AEC3 against the playback mix (with its high-pass filter, as
    on desktop) and AGC2, and no noise suppression: neither
    `ProcessingConfig::noise_suppression` nor the capture stage's step-down
    floor, whatever the config says. A capture stage still runs.
- `PhoneMode::Os` (the default) is 0.2.0's behaviour, unchanged.
- `EngineConfig::phone_mode` / `with_phone_mode(PhoneMode)`: the starting
  mode.
- `AudioEngine::set_phone_mode(PhoneMode)`: switch mid-call. Never blocks.
  With the device open, the control thread stops it and reopens it in the
  new mode (one restart); the participants, jitter buffers, encoder and
  capture stage carry on. sonora's chain for the new mode is built on the
  control thread and swapped in on the processing thread while the capture
  path is paused, so no audio thread allocates (tested with the counting
  allocator). The capture path treats the gap like a short suspend: a frame
  of silence ends the talkspurt and `ts48k` jumps by the gap in whole
  frames. If the new mode won't open, the backend goes straight back to the
  old one and raises `Event::Error`. With no device open (session inactive,
  or suspended) the mode applies when it opens. `AudioEngine::phone_mode()`
  says the mode in use.
- `Event::PhoneMode(PhoneModeChange { mode, gap })`, and
  `Stats::phone_mode_switches` and `Stats::last_phone_mode_gap`: the
  switches and how long the device was stopped for each.
- On iOS, `OsEffects` with all three `false` is now accepted in the OS's
  mode: the plain RemoteIO unit, and sonora runs everything the config asks
  for. (0.2.0 refused it with `InvalidConfig`.) Partial sets other than the
  unit's own (echo cancellation and noise suppression, with or without gain
  control) are still refused.

### Verified

- With fakes only (no phone in CI): which path each mode asks each platform
  for, sonora's configuration in each mode (read back from the sonora
  instances), the state machine's restarts, the go-back on a failed open,
  the events and stats, the switch on the engine's real threads with a
  clocked fake device, and the allocation-free swap. The iOS and Android OS
  calls are type-checked (`aarch64-apple-ios`, `aarch64-linux-android`), not
  run on a device: how a phone's raw microphone sounds and how long its
  restart takes have not been measured.
- cargo-semver-checks 0.51.0 against 0.2.0, both as the patch release this
  is and as a minor one, on the host (all features) and for
  `aarch64-apple-ios` (where the phone API exists): 202 checks pass, no
  semver update required.

### Unchanged

- Desktop backends: with no stage the pipeline is bit for bit 0.2.0's (the
  no-stage digest matches on the same machine); the sonora chain moved into
  its own struct so it can be swapped, with the same instances and order.

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
