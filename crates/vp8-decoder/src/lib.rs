//! A VP8 video decoder in pure, safe Rust (RFC 6386).
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

#![forbid(unsafe_code)]

mod bool_decoder;
#[cfg(test)]
mod bool_encoder;
mod decoder;
mod frame;
mod idct;
pub mod ivf;
mod loop_filter;
mod predict;
mod tables;

pub use decoder::{frame_info, DecodeError, Decoder, Dimensions, FrameInfo};
pub use frame::Picture;
