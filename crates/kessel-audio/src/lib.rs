//! # kessel-audio
//!
//! A real-time voice audio pipeline in pure Rust, for apps that carry live
//! calls over their own transport. It owns everything between the
//! microphone and speaker and the encoded frames on the wire:
//!
//! - **devices:** capture and playback through cpal (CoreAudio, WASAPI,
//!   ALSA), device lists, hot-plug, switching, and a null device for
//!   headless use; on iOS and Android, the OS's own voice processing
//!   (`Backend::OsVoiceProcessing`);
//! - **resampling** between the device rate and 48 kHz (rubato);
//! - **echo cancellation, noise suppression and gain control** (sonora, a
//!   port of WebRTC's audio processing), with the playback mix as the echo
//!   reference;
//! - **Opus** encode and decode (ropus), 48 kHz mono, 20 ms frames, with
//!   the caller's bitrate, FEC, DTX and complexity;
//! - **a jitter buffer per participant** with loss concealment and FEC
//!   recovery;
//! - **mixing**, **levels** (about every 50 ms) and **statistics**.
//!
//! The transport (RTP, encryption, signalling) stays outside: kessel-audio
//! produces plaintext [`EncodedFrame`]s and consumes [`RecvFrame`]s that the
//! transport has already decrypted and verified.
//!
//! ## Example
//!
//! ```no_run
//! use kessel_audio::{AudioEngine, EngineConfig, RecvFrame};
//!
//! let engine = AudioEngine::start(EngineConfig::default())?;
//! let frames = engine.capture_frames();
//! engine.add_participant(7);
//! loop {
//!     // Send path: encrypt and send each encoded frame.
//!     while let Some(f) = frames.try_recv() {
//!         let _ = (f.payload, f.ts48k);
//!     }
//!     // Receive path: hand over each decrypted, verified packet.
//!     # let packet: Option<RecvFrame> = None;
//!     if let Some(p) = packet {
//!         engine.push(p);
//!     }
//!     # break;
//! }
//! # Ok::<(), kessel_audio::AudioError>(())
//! ```
//!
//! ## Threads
//!
//! [`AudioEngine`] is a `Send + Sync` handle; no async runtime is needed.
//! Device callbacks only move samples through lock-free rings; one
//! processing thread does the signal processing and codec work; a control
//! thread handles devices and events. Every queue between the caller and
//! the engine is bounded and lock-free, and the audio side never blocks on
//! one: a full queue drops (and counts) instead.

// macOS's microphone permission calls (`mic::macos`) and the phone
// backends' OS calls (`phone::ios`, `phone::android`) are the only `unsafe`
// outside tests.
#![cfg_attr(
    not(any(
        test,
        all(feature = "cpal", target_os = "macos"),
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        test,
        all(feature = "cpal", target_os = "macos"),
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ),
    deny(unsafe_code)
)]

mod apm;
mod codec;
mod config;
mod device;
mod engine;
mod error;
mod events;
mod frame;
mod jitter;
mod level;
#[cfg(feature = "cpal")]
mod mic;
mod null;
#[cfg(any(
    test,
    all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
))]
mod phone;
mod pipeline;
mod resample;
mod stats;
#[cfg(feature = "cpal")]
mod system;

// Runs the README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(test)]
mod testkit;
#[cfg(test)]
mod tests;

pub use apm::{NoiseSuppressionStrength, ProcessingConfig, AGC2_HEADROOM_DB};
/// The `bytes` crate, re-exported so callers build payloads with the same
/// version.
pub use bytes;
pub use config::{Backend, EngineConfig};
pub use device::{CaptureSource, NullDevice, PlaybackSink};
pub use engine::AudioEngine;
pub use error::{AudioError, ErrorCode};
pub use events::{
    DeviceChoice, DeviceInfo, DeviceList, DeviceSelection, DevicesInUse, Event, EventRx, Level,
    Levels,
};
pub use frame::{
    EncodedFrame, EncoderCfg, FrameRx, RecvFrame, FEC_LOSS_HINT_PCT, FRAME_MS, FRAME_SAMPLES,
    SAMPLE_RATE,
};
pub use jitter::JitterConfig;
#[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
pub use phone::{OsEffects, OsVoiceConfig};
pub use stats::{CaptureStats, ParticipantStats, Stats};
