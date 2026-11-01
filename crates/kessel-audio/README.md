kessel-audio is a real-time voice audio pipeline in pure Rust: devices, echo cancellation, noise suppression, gain control, Opus, jitter buffers with loss concealment, mixing and levels, behind a small frame interface for your own transport.

# kessel-audio

It is part of the Kessel family of open-source Rust crates from Alakazam
Labs. Licensed MIT OR Apache-2.0.

## What it does

```
 mic ─► resample to 48 kHz ─► AEC3 / NS / AGC2 ─► Opus encode ─► EncodedFrame ─► (your transport)
                                   ▲
                                   │ echo reference = exactly what is played
 speaker ◄─ resample ◄─ mix ◄─ per-participant jitter buffer + Opus decode ◄─ RecvFrame ◄─ (your transport)
```

| Piece | How |
|---|---|
| Devices | cpal: CoreAudio, WASAPI, ALSA (and PulseAudio with the `pulseaudio` feature). Device lists, hot-plug polling, switching, fallback to the default when a chosen device goes away. A **null device** runs the whole pipeline without hardware. |
| Resampling | rubato (FFT, synchronous), device rate ↔ 48 kHz in 10 ms blocks. Skipped when the device runs at 48 kHz. |
| Echo, noise, gain | sonora 0.2.0 (a pure-Rust port of WebRTC's audio processing): AEC3, noise suppression, AGC2, high-pass filter. The playback mix is the echo reference. |
| Opus | ropus (a pure-Rust port of the reference codec, bit-exact): 48 kHz mono, 20 ms frames; bitrate, in-band FEC, DTX and complexity from the caller, changeable mid-call. |
| Jitter buffer | One per participant: adaptive delay (40 to 200 ms by default), reordering, duplicates, FEC recovery from the next packet, packet-loss concealment, DTX gaps, stall recovery and delay trimming. |
| Mixing and levels | Sum with a soft limiter; levels and a speaking flag about every 50 ms for this device and each participant. |
| Statistics | Per participant: jitter, loss, concealment, jitter-buffer delay, and counters; the echo canceller's measured delay and ERLE. |

The transport (RTP, encryption, signalling) stays outside. kessel-audio only
ever handles plaintext Opus: the transport encrypts each `EncodedFrame` after
it leaves and must hand over only `RecvFrame`s that decrypted and verified.

## Using it

```rust
use kessel_audio::{AudioEngine, EncoderCfg, EngineConfig, Event, RecvFrame};

let engine = AudioEngine::start(EngineConfig::default())?;
engine.set_encoder(EncoderCfg { bitrate_bps: 24_000, ..EncoderCfg::default() });

// Send path: drain encoded frames (a bounded, lock-free queue).
let frames = engine.capture_frames();
while let Some(f) = frames.try_recv() {
    // encrypt f.payload, send with RTP timestamp f.ts48k
}

// Receive path.
engine.add_participant(7);
# let (payload, seq, ts48k) = (kessel_audio::bytes::Bytes::new(), 0u16, 0u32);
engine.push(RecvFrame { participant: 7, payload, seq, ts48k, arrival: std::time::Instant::now() });

// Levels, devices and errors.
for e in std::iter::from_fn(|| engine.events().try_recv()) {
    match e {
        Event::Levels(l) => { /* speaking indicators */ }
        Event::Error(err) => eprintln!("{}: {}", err.code.as_str(), err.detail),
        _ => {}
    }
}
# Ok::<(), kessel_audio::AudioError>(())
```

### Timestamps

`EncodedFrame::ts48k` is the capture time in 48 kHz units: +960 per 20 ms
frame, wrapping at 2^32 like an RTP timestamp. During DTX the frames with
nothing to send are not pushed, so the next frame's `ts48k` has advanced
across the gap. Muting stops frames; time keeps advancing.

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
replaces the sound card with a clock thread that pulls 10 ms of capture from
your `input` closure and hands 10 ms of the mix to your `output` closure. It
runs the full pipeline headless: tests, servers, bots.

## Measured quality

The test suite runs synthetic laptop echo paths (image-method rooms, a small
speaker driven into soft clipping, a chassis path, 60 ms system delay) and
asserts metric bars, never sample values. sonora's output is not
sample-identical to the C++ library (its high-pass filter differs), so
waveform comparisons would be meaningless.

## Building

- Linux needs the ALSA development headers (`alsa-lib` / `libasound2-dev`)
  for the default `cpal` feature. Build with `--no-default-features` for the
  null device only.
- cpal's `realtime` feature is deliberately not enabled (it pulls an
  MPL-2.0 crate); the device threads run at the priority the OS gives them.
- `deny.toml` lists the licence allowlist; `tests/licences.rs` checks the
  same rules without needing cargo-deny.

## Licence

MIT OR Apache-2.0, at your option.
