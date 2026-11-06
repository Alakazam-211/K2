//! A real-time VP8 video encoder in pure, safe Rust.
//!
//! It turns I420 pictures into raw VP8 frames (RFC 6386) for live video:
//! camera and screen sharing over WebRTC. No look-ahead and no reordering:
//! one frame in, at most one frame out.
//!
//! - Rate control to a target bitrate, changeable at any time with
//!   [`Encoder::set_rates`]; frames are skipped when the stream runs far
//!   over budget.
//! - Keyframes on request ([`Encoder::request_keyframe`] or the
//!   `force_keyframe` flag), for PLI and FIR.
//! - Optional temporal layers (L1T2, L1T3) in the usual WebRTC pattern.
//! - Simulcast: run one encoder per layer, each at its own size;
//!   [`scale_i420`] makes the smaller pictures.
//!
//! Every frame's probability updates apply to that frame only, so a lost or
//! dropped frame never leaves a receiver with the wrong probabilities.
//!
//! ```no_run
//! use vp8_encoder::{Encoder, EncoderConfig, Picture};
//! let mut enc = Encoder::new(EncoderConfig::new(1280, 720, 30, 2_000_000)).unwrap();
//! let i420 = vec![128u8; 1280 * 720 * 3 / 2];
//! let picture = Picture::from_i420(&i420, 1280, 720).unwrap();
//! if let Some(frame) = enc.encode(&picture, false).unwrap() {
//!     // frame.data is one raw VP8 frame, ready for an RTP packetiser.
//!     let _ = (frame.keyframe, frame.temporal_id, frame.layer_sync);
//! }
//! ```
//!
//! See PROVENANCE.md for how this crate was written.

#![forbid(unsafe_code)]

mod encoder;
mod rate;
mod scale;
mod tokens;
mod transform;

use std::fmt;

pub use scale::scale_i420;
pub use vp8_decoder::Picture;

/// Temporal layering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TemporalLayers {
    /// One layer: every frame predicts from the previous one.
    #[default]
    L1T1,
    /// Two layers: 0, 1, 0, 1... Layer 1 frames are never referenced, so a
    /// receiver can drop them for half the frame rate.
    L1T2,
    /// Three layers: 0, 2, 1, 2... Drop layer 2 for half the frame rate, and
    /// layers 1 and 2 for a quarter.
    L1T3,
}

impl TemporalLayers {
    /// How many layers.
    pub fn count(self) -> u8 {
        match self {
            TemporalLayers::L1T1 => 1,
            TemporalLayers::L1T2 => 2,
            TemporalLayers::L1T3 => 3,
        }
    }
}

/// Encoder settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncoderConfig {
    /// Coded width in pixels (1 to 16383).
    pub width: u32,
    /// Coded height in pixels (1 to 16383).
    pub height: u32,
    /// The frame rate the input arrives at (for rate control).
    pub fps: u32,
    /// Target bitrate in bits per second.
    pub bitrate_bps: u32,
    /// Ceiling the encoder skips frames to stay under, in bits per second.
    /// Raised to `bitrate_bps` if lower.
    pub max_bitrate_bps: u32,
    /// A keyframe every this many frames; `None`, only on request.
    pub keyframe_interval: Option<u32>,
    /// Temporal layering.
    pub temporal_layers: TemporalLayers,
}

impl EncoderConfig {
    /// Settings for `width` x `height` at `fps` and `bitrate_bps`, with a
    /// 1.5x ceiling, keyframes on request only and no temporal layers.
    pub fn new(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Self {
        EncoderConfig {
            width,
            height,
            fps,
            bitrate_bps,
            max_bitrate_bps: bitrate_bps.saturating_add(bitrate_bps / 2),
            keyframe_interval: None,
            temporal_layers: TemporalLayers::L1T1,
        }
    }

    /// The same settings with temporal layering.
    pub fn with_temporal_layers(mut self, layers: TemporalLayers) -> Self {
        self.temporal_layers = layers;
        self
    }

    /// The same settings with a keyframe interval.
    pub fn with_keyframe_interval(mut self, frames: Option<u32>) -> Self {
        self.keyframe_interval = frames;
        self
    }
}

/// One encoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncodedFrame {
    /// One raw VP8 frame, starting at the frame tag.
    pub data: Vec<u8>,
    /// A keyframe.
    pub keyframe: bool,
    /// The temporal layer, 0 for the base.
    pub temporal_id: u8,
    /// RFC 7741's Y bit: the frame depends only on the base layer (or is a
    /// keyframe).
    pub layer_sync: bool,
    /// The quantizer index used (0 to 127), for statistics.
    pub q_index: u8,
}

/// Why a frame could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodeError {
    /// The configuration is unusable.
    InvalidConfig(&'static str),
    /// The picture's size differs from the configured size.
    SizeMismatch {
        /// The configured size.
        expected: (u32, u32),
        /// The picture's size.
        got: (u32, u32),
    },
    /// The picture's planes are shorter than its size and strides need.
    InvalidPicture,
    /// A bug: the encoder's own output did not decode.
    Internal(String),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::InvalidConfig(why) => write!(f, "invalid encoder config: {why}"),
            EncodeError::SizeMismatch { expected, got } => {
                write!(
                    f,
                    "picture is {}x{}, the encoder is {}x{}",
                    got.0, got.1, expected.0, expected.1
                )
            }
            EncodeError::InvalidPicture => f.write_str("picture planes are too short for its size"),
            EncodeError::Internal(why) => write!(f, "internal encoder error: {why}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// A real-time VP8 encoder for one stream (one simulcast layer).
pub struct Encoder {
    core: encoder::Core,
}

impl Encoder {
    /// An encoder; its first frame is a keyframe.
    pub fn new(config: EncoderConfig) -> Result<Self, EncodeError> {
        Ok(Encoder {
            core: encoder::Core::new(config)?,
        })
    }

    /// Encodes one picture, which must be the configured size. Returns
    /// `None` when rate control skips the frame. `force_keyframe` (or an
    /// earlier [`request_keyframe`](Self::request_keyframe)) makes this
    /// frame a keyframe.
    pub fn encode(
        &mut self,
        picture: &Picture<'_>,
        force_keyframe: bool,
    ) -> Result<Option<EncodedFrame>, EncodeError> {
        self.core.encode(picture, force_keyframe)
    }

    /// The next encoded frame is a keyframe. Repeated calls coalesce.
    pub fn request_keyframe(&mut self) {
        self.core.request_keyframe();
    }

    /// Changes the target bitrate and frame rate, without a keyframe.
    pub fn set_rates(&mut self, bitrate_bps: u32, fps: u32) {
        self.core.set_rates(bitrate_bps, fps);
    }

    /// The current settings.
    pub fn config(&self) -> &EncoderConfig {
        &self.core.cfg
    }
}

// The README's examples are compiled as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
