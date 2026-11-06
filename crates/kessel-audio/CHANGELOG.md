# Changelog

## Unreleased

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
