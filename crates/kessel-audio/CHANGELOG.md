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

### Unchanged

- Desktop backends and their defaults behave as before.
