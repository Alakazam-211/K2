# kessel-audio

A real-time voice pipeline in pure Rust (microphone to Opus frames and back)
for apps that bring their own network transport.

Kessel is Alakazam Labs' family of open-source Rust crates for real-time
apps. kessel-audio stands alone: it needs no other Kessel crate. API docs are
on [docs.rs](https://docs.rs/kessel-audio), and the source is at
<https://github.com/Alakazam-211/K2> (`crates/kessel-audio`). Licensed MIT OR
Apache-2.0.

**Version 0.1** is the first release. Until 1.0, a minor release (0.2, 0.3)
may change the API, as Cargo allows for 0.x versions; a patch release (0.1.x)
won't. Each change is listed in the changelog in the repository.

## Platforms

| Platform | Backend | Status |
|---|---|---|
| Linux (x86-64) | cpal: ALSA, or PulseAudio with the `pulseaudio` feature | Tested: the test suite passes. |
| macOS (Apple Silicon) | cpal: CoreAudio | Tested: the test suite passes. |
| Windows (x86-64) | cpal: WASAPI | Type-checked only. |
| iOS (arm64) | The OS's voice processing (VoiceProcessingIO) | Type-checked only. The state machine behind it is tested on a fake OS. |
| Android (arm64, API 28+) | The OS's voice processing (AAudio) | Type-checked only. The state machine behind it is tested on a fake OS. |
| Any | The null device (no hardware) | Tested on Linux and macOS. |

"Type-checked" means `cargo check` and `cargo clippy` pass for that target;
the tests have not run on it. The test suite needs no audio hardware: the
null device and fake devices stand in for it.

## What it does

```text
 mic ─► resample to 48 kHz ─► AEC3 / NS / AGC2 ─► Opus encode ─► EncodedFrame ─► (your transport)
                                   ▲
                                   │ echo reference = exactly what is played
 speaker ◄─ resample ◄─ mix ◄─ per-participant jitter buffer + Opus decode ◄─ RecvFrame ◄─ (your transport)
```

| Piece | How |
|---|---|
| Devices | cpal: CoreAudio, WASAPI, ALSA (and PulseAudio with the `pulseaudio` feature). Device lists, hot-plug polling, switching, fallback to the default when a chosen device goes away. A **null device** runs the whole pipeline without hardware. On iOS and Android, the **phone's own voice processing** (see [Phones](#phones)). |
| Resampling | rubato (FFT, synchronous), device rate ↔ 48 kHz in 10 ms blocks. Skipped when the device runs at 48 kHz. |
| Echo, noise, gain | sonora 0.2.0 (a pure-Rust port of WebRTC's audio processing): AEC3, noise suppression, AGC2, high-pass filter. The playback mix is the echo reference. |
| Opus | ropus (a pure-Rust port of the reference codec, bit-exact): 48 kHz mono, 20 ms frames; bitrate, in-band FEC, DTX and complexity from the caller, changeable mid-call. |
| Jitter buffer | One per participant: adaptive delay (40 to 200 ms by default), reordering, duplicates, FEC recovery from the next packet, packet-loss concealment, DTX gaps, stall recovery and delay trimming. |
| Mixing and levels | Sum with a soft limiter; levels and a speaking flag about every 50 ms for this device and each participant. |
| Statistics | Per participant: jitter, loss, concealment, jitter-buffer delay, and counters; the echo canceller's measured delay and ERLE. |

The transport (RTP, encryption, signalling) stays outside. kessel-audio only
ever handles plaintext Opus: the transport encrypts each `EncodedFrame` after
it leaves and must hand over only `RecvFrame`s that decrypted and verified.

### Glossary

- **AEC:** acoustic echo cancellation. Removes the far end's voice from your microphone after it comes out of your speaker. AEC3 is WebRTC's current design.
- **NS:** noise suppression.
- **AGC:** automatic gain control. AGC2 is WebRTC's current design.
- **ERLE:** echo return loss enhancement. How many dB of echo the canceller removed; higher is better.
- **DTX:** discontinuous transmission. The encoder sends almost nothing during silence.
- **FEC:** forward error correction. Each Opus packet carries a low-rate copy of the previous frame, so one lost packet can be recovered from the next.
- **PLC:** packet-loss concealment. The decoder makes up plausible audio for a lost frame.

## Using it

```toml
[dependencies]
kessel-audio = "0.1"
```

This runs as it stands, with no audio hardware: a test tone stands in for the
microphone, and each encoded frame goes straight back in as if it had come
from a remote participant. With a real transport, you'd encrypt and send each
frame instead, and push what you receive.

```rust
use kessel_audio::{AudioEngine, EncoderCfg, EngineConfig, Event, NullDevice, RecvFrame};
use std::time::{Duration, Instant};

fn main() -> Result<(), kessel_audio::AudioError> {
    // The null device: this closure is the microphone, 10 ms at a time.
    let mut phase = 0.0f32;
    let device = NullDevice {
        input: Some(Box::new(move |block: &mut [f32]| {
            for sample in block.iter_mut() {
                *sample = 0.2 * phase.sin();
                phase = (phase + 0.0576) % std::f32::consts::TAU; // 440 Hz at 48 kHz
            }
        })),
        ..NullDevice::default()
    };
    // DTX off: a frame every 20 ms, even if the tone is suppressed as noise.
    let encoder = EncoderCfg { dtx: false, ..EncoderCfg::default() };
    let engine = AudioEngine::start(EngineConfig { encoder, ..EngineConfig::null(device) })?;

    let frames = engine.capture_frames();
    let events = engine.events();
    engine.add_participant(7);

    let mut seq = 0u16;
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        // Send path: a bounded, lock-free queue of encoded frames.
        while let Some(f) = frames.try_recv() {
            // Receive path: only frames that decrypted and verified.
            engine.push(RecvFrame {
                participant: 7,
                payload: f.payload,
                seq,
                ts48k: f.ts48k,
                arrival: Instant::now(),
            });
            seq = seq.wrapping_add(1);
        }
        // Levels, devices and errors.
        while let Some(event) = events.try_recv() {
            if let Event::Error(e) = event {
                eprintln!("{}: {}", e.code.as_str(), e.detail);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(seq > 0, "no frames came out");
    println!("{seq} frames; levels: {:?}", engine.levels());
    Ok(())
}
```

For the system's default microphone and speaker, start with
`EngineConfig::default()` instead. The rest of the code is the same.

### Errors

`AudioEngine::start` only fails for a bad configuration or if its threads can't
start. Device problems arrive as `Event::Error`, each with a stable code, and
the engine keeps running with whatever devices it could open:

| Code | Meaning | What the engine does |
|---|---|---|
| `no_permission` | The person refused the microphone, policy forbids it, or the app can't ask (see [Building](#building)). | No input device opens. It opens by itself if access is granted later. |
| `device_lost` | A device in use went away. | Falls back to the default device. |
| `device_busy` | Another app holds the device. | Stays without it until the device list changes or you call `set_device`. |
| `no_device` | No such device, or none of that kind. | Stays without it until the device list changes or you call `set_device`. |

### Timestamps

`EncodedFrame::ts48k` is the capture time in 48 kHz units: +960 per 20 ms
frame, wrapping at 2^32 like an RTP timestamp. During DTX the frames with
nothing to send are not pushed, so the next frame's `ts48k` has advanced
across the gap. Muting stops frames; time keeps advancing.
A suspend (below) stops frames too, and the first frame after it has a
`ts48k` advanced by the suspended time, like the end of a DTX gap.

### Threads

`AudioEngine` is `Send + Sync` and needs no async runtime. The device
callbacks only move samples through lock-free rings (no allocation, locks or
blocking; tested with a counting allocator). One processing thread runs the
DSP and codec work and never waits on a lock. A control thread handles
devices and turns snapshots into events. A full queue drops and counts; the
audio side never blocks on the caller.

The Opus and echo-cancellation crates allocate small buffers per frame on the
processing thread. The device callbacks are insulated from that by the rings
(30 ms of playout by default).

### The null device

`Backend::Null(NullDevice { input, output, sample_rate, block_period })`
(`input` is a `CaptureSource`, `output` a `PlaybackSink`)
replaces the sound card with a clock thread that pulls 10 ms of capture from
your `input` closure and hands 10 ms of the mix to your `output` closure. It
runs the full pipeline headless: tests, servers, bots.

### Suspend and resume

`AudioEngine::suspend()` and `resume()` pause a call without ending it: for
hold (a cellular call), an interruption, or lost audio focus. They work with
every backend. While suspended:

- no frames are produced, and playback is silent;
- participants are still decoded, so their jitter buffers are current on
  resume;
- the phone backend also stops its device.

The far end must hear a pause, not loss. So if you were speaking, one frame
of encoded silence (marked `dtx`) ends the talkspurt. After `resume`, the
next frame's `ts48k` has jumped by the suspended time, in whole frames. With
the transport's sequence numbers running on as usual, a receiver reads the
gap as DTX; this crate's jitter buffer does (tested).

## Phones

`Backend::OsVoiceProcessing(OsVoiceConfig)` uses the OS's own echo
cancellation, noise suppression and gain control, which are tuned per device
model:

- **iOS:** the VoiceProcessingIO audio unit (the FaceTime path), through
  coreaudio-rs;
- **Android:** AAudio with the voice-communication input preset and usage,
  through the `ndk` crate. Needs Android 9 (API 28).

sonora runs only what `OsVoiceConfig::os_effects` says the OS doesn't do. On
iOS that is nothing: the unit always cancels echo and suppresses noise, and
its gain control can be turned off. On Android, set the flags from
`AcousticEchoCanceler.isAvailable()` and friends, and sonora fills the gaps.

**The app owns the OS's audio session.** kessel-audio never configures or
activates it. On iOS, set the category (`.playAndRecord`, mode `.voiceChat`)
yourself, and let CallKit activate it:

| CallKit / AVAudioSession | Call |
|---|---|
| The call starts | `AudioEngine::start_when_session_active(config)` (the device waits) |
| `provider(_:didActivate:)` | `engine.session_activated()` |
| `provider(_:didDeactivate:)` | `engine.session_deactivated()` |
| `CXSetHeldCallAction`, or an interruption begins / ends | `engine.suspend()` / `engine.resume()` |
| `routeChangeNotification` | `engine.route_changed()` |
| `CXSetMutedCallAction` | `engine.set_capture(!muted)` |

None of these block: the device opens and closes on the engine's control
thread a few milliseconds later (`Event::DevicesInUse`). The device is open
exactly while the session is active and the call isn't suspended. A route
change at a new rate (Bluetooth hands-free runs at 16 kHz) reopens it at that
rate without ending the call; the pipeline stays at 48 kHz.

On Android, start with `AudioEngine::start` (there is no session to wait
for), call `suspend` / `resume` on audio focus loss and gain, and
`route_changed` when the communication device changes. A stream the system
disconnects is reopened on its own. The app sets `MODE_IN_COMMUNICATION`,
holds audio focus and the `RECORD_AUDIO` permission.

The OS picks the route: the backend lists one input and one output, and
`set_device` takes only the defaults. If the person refused the microphone,
playback still runs, with no capture, and an `Event::Error` is raised. Ask
for the permission before the call.

- **iOS** reads the record permission first and raises `no_permission`.
- **Android:** the backend doesn't check `RECORD_AUDIO` itself. Without it,
  AAudio refuses to open or start the input stream, and the error code is
  whatever AAudio's refusal maps to: usually `backend`, or
  `unsupported_config`, `device_busy` or `device_lost`, depending on the
  device. The detail starts `AAudio input:` (or `AAudio input start:`).
  This has not been checked on a device. Check `RECORD_AUDIO` yourself
  before the call rather than relying on the code.

## Measured quality

The metric tests play real speech (12 LibriSpeech utterances) through
simulated laptop echo paths (rooms with reflections, a small speaker driven
into soft clipping, a path through the chassis, 60 ms system delay), through
the whole chain: Opus encode, jitter buffer, decode, mix, playback, echo path,
microphone, AEC/NS/AGC, Opus encode, and a remote decoder. They assert metric
bars, never sample values: sonora's output is not sample-identical to the C++
WebRTC library (its high-pass filter differs), so waveform comparisons would be
meaningless.

Figures from the test run (x86-64 Linux; Apple Silicon agrees within about
1 dB). "Baseline" is an earlier measurement of sonora 0.2.0 and of the C++
WebRTC library on the same echo model, without the codec in the path. Every
baseline cell reads "sonora / C++", as a mean over that measurement's runs;
"-" means the baseline didn't measure it.

| Test | Measured | Bar | Baseline (sonora / C++) |
|---|---|---|---|
| Far-end only, ERLE 5-30 s, 3 rooms | 47.5 / 45.5 / 46.9 dB (mean 46.7) | each >= 38, mean >= 42 | 50.4 / 48.2 dB |
| Echo canceller's delay estimate (true 60 ms) | 56 ms | 45-75 ms | 56 / 16 ms (1) |
| Double talk, near-end level vs near-end alone, 2 rooms | -5.3 / -4.1 dB | >= -7 dB | -4.0 / -4.1 dB |
| Double talk, ERLE in far-only gaps, 2 rooms | 41.5 / 40.1 dB | >= 32 dB | 42.1 / 41.7 dB |
| Full chain (AEC + NS + AGC2) in double talk: ERLE in far-only gaps | 45.6 dB | >= 35 dB | 46.6 / 44.1 dB |
| Full chain in double talk: near-end level vs near-end alone | -3.9 dB | >= -7 dB | - (2) |
| Echo path change, time back to 30 dB ERLE: moved (+3 dB) / delay +40 ms | 0.0 / 0.0 s (3) | <= 0.5 s | moved 0.0 / 0.0 s; delay 0.0 / 0.1 s |
| Noise suppression (moderate), pink noise at 5 dB SNR: noise cut in pauses | 15.5 dB; NS's own share 10.3 dB (4) | >= 12 dB; NS's share >= 8 dB | 11.9 / 11.9 dB |
| AGC2 output (dBFS) at -50 / -40 / -30 / -20 dBFS input | -27.1 / -23.5 / -23.6 / -20.9 | within 1.5 dB of C++ | -27.0 / -27.3, -23.8 / -23.1, -24.0 / -23.1, -20.7 / -20.6 (5) |
| CPU per 10 ms block, AEC3 + NS + AGC2 alone / whole duplex path | 1.8% / 3.2% of a core (x86-64, DSP crates at `opt-level = 3`; 3.0% / 5.5% on a slower machine) | AEC chain <= 5%, duplex <= 10% (p50) | AEC chain: 1.55% / 1.13% |
| 10% loss, 5% reordering, 30 ms jitter | loss 9.4%, concealed 3.9%, FEC 51 frames | concealed < 15% | - |
| Lost frames: FEC vs concealment (SNR vs clean) | 13.4 vs -0.1 dB | FEC >= PLC + 6 dB | - |

1. The C++ library reported 16 ms for the same 60 ms path in that
   measurement. The bar is set on the true delay, not on the C++ figure.
2. The baseline measured this level against the raw input, so its figure
   (-11.1 / -10.3 dB) includes AGC2's and NS's gain changes. This test
   compares with the near end alone through the same chain, which isolates
   the double-talk ducking. The two don't compare.
3. The echo canceller never lost 30 dB. The test measures ERLE over 1 s
   windows that start every 50 ms after the change, and reports when the
   first window reaches 30 dB. Here the first window, starting at the
   change, already had more than 30 dB, so the time is 0. The same test
   asserts at least 30 dB in the first 2 s and 40 dB once settled.
4. 15.5 dB is the noise level in speech pauses at the remote decoder's
   output, compared with the microphone input. It comes from the whole send
   path: the high-pass filter, noise suppression, and Opus at 32 kbit/s with
   DTX, which turns pauses into comfort noise. NS's own share (10.3 dB)
   compares with the same chain with noise suppression off. The baseline
   (11.9 dB) is the high-pass filter and NS without the codec.
5. sonora's figures here are before the headroom tuning below.
   `AGC2_HEADROOM_DB` is a public constant, so you can read the value the
   engine uses, but not set it: the engine always runs AGC2 with 4 dB.

### AGC2 tuning

sonora 0.2.0's AGC2 settles 0.7 to 1.6 dB quieter than the C++ library
with WebRTC's default 5 dB headroom. A sweep against the C++ library on
LibriSpeech (5.0, 4.0, 3.4, 3.0 dB) put 4 dB closest on every level
metric: within 0.6 dB of the C++ library everywhere, against up to 1.6 dB.
`AGC2_HEADROOM_DB` carries it, and a test checks it still lifts mid-level
speech by about 1 dB.

## Testing

| Command | Runs |
|---|---|
| `cargo test` | Everything that needs no test data. From the published package, that is the unit tests and the README's examples; `tests/` (the interface and licence checks) is in the repository only. |
| `cargo test --features speech-tests` | Also the metric tests above. Needs `tests/data`, which is in the repository but not in the published package, so it won't compile from the package. |
| `cargo test --release --features speech-tests` | Also the CPU bars. |
| `cargo test --features cpu-bars` | The CPU bars in a debug build. Turn this on only if your root manifest sets `opt-level = 3` for `sonora*`, `ropus` and `rubato` in `[profile.dev.package]`. Otherwise the DSP runs unoptimised and misses the bars by a long way. |

None of them need audio hardware. `scripts/check-targets.sh` (in the
repository) type-checks the iOS and Android targets, or any target you name,
from a machine that can't run them.

## Building

- Needs Rust 1.91 or later (sonora's minimum).
- Linux needs the ALSA development headers (`alsa-lib` / `libasound2-dev`)
  for the default `cpal` feature. Build with `--no-default-features` for the
  null device only.
- On macOS, a bundled app needs `NSMicrophoneUsageDescription` in its
  Info.plist, and `com.apple.security.device.audio-input` if it uses the
  hardened runtime. If the person refuses the microphone, or the app can't ask,
  the engine raises `no_permission` rather than capture silence.
- cpal's `realtime` feature is deliberately not enabled (it pulls an
  MPL-2.0 crate); the device threads run at the priority the OS gives them.
- The `os-voice` feature (on by default) is the phone backend. It does
  nothing on other targets. On Android it links AAudio's API 28 calls, so
  the app's `minSdk` must be 28 or later; turn the feature off for older
  devices.
- `deny.toml` lists the licence allowlist; `tests/licences.rs` checks the
  same rules without needing cargo-deny.

## Security

kessel-audio decodes Opus packets that arrive from the network, so treat a
crash, hang or memory blow-up on a crafted packet as a security bug. Please
report it privately, not in a public issue, through GitHub's private
vulnerability reporting: <https://github.com/Alakazam-211/K2/security>, then
"Report a vulnerability". The repository's SECURITY.md has the policy.

### Unsafe code and fuzzing

What is and isn't covered, plainly:

- **kessel-audio's own `unsafe`** is only in OS calls: macOS's microphone
  permission (through the Objective-C runtime), iOS's audio session reads,
  and Android's AAudio callbacks (a slice over the buffer AAudio hands in).
  On Linux and Windows the crate is built with `forbid(unsafe_code)`. The
  receive path (packet checks, jitter buffer, mixing) has no `unsafe` on any
  platform.
- **Not fuzzed.** kessel-audio has no coverage-guided fuzzer. A seeded
  random test sends 3,000 hostile packets (random bytes; valid packets with
  bits flipped, truncated, extended or with a random first byte) to the
  decoder, and 1,500 blocks of hostile frames (those payloads plus random,
  repeated and backwards sequence numbers and timestamps, and arrival times
  2 s either side of now) through the jitter buffer and mixer. It checks
  for no panic and for finite output within the limiter's bounds. It runs
  the same inputs every time.
- **ropus** (the Opus decoder) has a few `unsafe` blocks: unchecked indexing
  in hot loops and some slice casts. Its README says it is fuzzed and
  differential-tested against the C reference decoder.
- **sonora** uses `unsafe` in its SIMD and FFT code. It processes only local
  audio (the microphone and the playback mix), never network data.
- rubato, rtrb, crossbeam-queue and cpal use `unsafe` internally too, as
  resamplers, lock-free queues and audio-device crates do.

## Licence

kessel-audio's own code is MIT OR Apache-2.0, at your option. It contains no
third-party code, so it has no NOTICE file.

Its signal processing comes from two dependencies under BSD-3-Clause:

- **sonora** (a Rust port of WebRTC's audio processing), copyright the
  WebRTC Project Authors and others;
- **ropus** (a Rust port of the Xiph.Org Opus codec), copyright Xiph.Org,
  Skype Limited and others.

**If you distribute binaries** that include kessel-audio, BSD-3-Clause
requires you to reproduce those crates' copyright notices and licence
text. They are in each crate's `LICENSE` file (sonora's sub-crates, such as
sonora-aec3, carry the same licence as sonora). Tools such as
`cargo about` collect them. The rest of the dependency tree is MIT,
Apache-2.0, BSD, ISC, Zlib or Unicode-3.0, checked by `deny.toml` and
`tests/licences.rs` in the repository.

### Patents

- **Our own code:** no patent licence is granted beyond Apache-2.0's for our
  own contributions.
- **Opus:** Opus's own patent holders (Xiph.Org, Broadcom and Microsoft)
  have made royalty-free licence declarations for implementations of
  RFC 6716, on the terms they state. Four other companies filed IETF
  disclosures with possibly royalty-bearing terms; the Opus project
  discusses them on the same page. See <https://opus-codec.org/license/>.
- **WebRTC:** Google's additional patent grant for WebRTC covers Google's
  own WebRTC code only. It does not cover ports such as sonora.
