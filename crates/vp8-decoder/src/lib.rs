//! A VP8 video decoder in pure Rust (RFC 6386).
//!
//! It decodes every VP8 bitstream version (0 to 3): keyframes and
//! interframes, all intra and inter prediction modes including split motion
//! vectors, segmentation, the normal and simple loop filters, token
//! partitions, probability updates and persistence, and the last, golden and
//! alt-ref reference frames with their sign bias and buffer copies. Its
//! output matches the WebM project's VP8 test vectors bit for bit.
//!
//! Input is one raw VP8 frame at a time, starting at the frame tag (no IVF,
//! WebM or RTP framing). Output is a borrowed I420 [`Picture`].
//!
//! ```no_run
//! let frames: Vec<Vec<u8>> = Vec::new(); // raw VP8 frames, in order
//! let mut decoder = vp8_decoder::Decoder::new();
//! for frame in &frames {
//!     match decoder.decode(frame) {
//!         Ok(Some(picture)) => { let _i420 = picture.to_i420(); }
//!         Ok(None) => {} // a frame that only updates references
//!         Err(vp8_decoder::DecodeError::NeedKeyframe) => {} // wait for a keyframe
//!         Err(_) => {} // corrupt: ask the sender for a keyframe
//!     }
//! }
//! ```
//!
//! See PROVENANCE.md for how this crate was written.

// The only `unsafe` code is in the SIMD modules (NEON on aarch64, SSE2 on
// x86_64), which allow it for themselves; see the comments at their tops.
#![deny(unsafe_code)]

mod bool_decoder;
mod decoder;
mod frame;
pub mod ivf;
#[cfg(all(target_arch = "aarch64", not(vp8_portable)))]
#[allow(unsafe_code)]
mod neon;
#[cfg(all(target_arch = "aarch64", not(vp8_portable)))]
use neon as simd;
#[cfg(all(target_arch = "x86_64", not(vp8_portable)))]
#[allow(unsafe_code)]
mod sse2;
#[cfg(all(target_arch = "x86_64", not(vp8_portable)))]
use sse2 as simd;
#[cfg(test)]
mod reference;

// Shared with the vp8-encoder crate. Not a stable API: it may change in any
// release, and vp8-encoder pins this crate's exact version. Lockstep rule:
// any vp8-decoder release that touches these modules ships with a matching
// vp8-encoder release that bumps its `=` pin.
#[doc(hidden)]
pub mod bool_encoder;
#[doc(hidden)]
pub mod idct;
#[doc(hidden)]
pub mod loop_filter;
#[doc(hidden)]
pub mod modes;
#[doc(hidden)]
pub mod predict;
#[doc(hidden)]
pub mod tables;

pub use decoder::{frame_info, DecodeError, Decoder, Dimensions, FrameInfo, DEFAULT_MAX_PIXELS};
pub use frame::Picture;

#[cfg(vp8_stage_timers)]
#[doc(hidden)]
pub use decoder::StageTimes;

// The README's examples are compiled as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
