//! # kessel-video
//!
//! Cameras, screen capture, pixels and video codecs for apps that carry live
//! video over their own transport. No RTP, no keys, no ciphertext and no
//! signalling: the app does those, and hands kessel-video only plaintext
//! frames it has already decrypted and verified.
//!
//! - **One [`VideoBackend`] per app** ([`system`], [`null`] or
//!   [`fake::FakeBackend`]): capabilities, cameras, permissions, screens and
//!   windows, encoders, decoders, conversion, and machine-wide events.
//! - **Sources, encoders and decoders work by submit and poll**, with a
//!   [`Wake`] callback and small bounded queues that never block the
//!   producer. No async runtime: kessel-video runs its own threads.
//! - **Codec-neutral:** VP8 as raw RFC 6386 frames, H.264 as one Annex B
//!   access unit (4-byte start codes, SPS and PPS before every IDR). H.264
//!   comes only from the operating system's codecs (VideoToolbox on macOS);
//!   no codec library is linked or shipped.
//!
//! ## Example
//!
//! ```
//! use kessel_video::{fake, no_wake, EncodeOptions, EncoderConfig, VideoBackend};
//!
//! let backend = fake::FakeBackend::new(fake::FakeConfig::default());
//! let mut camera = backend.open_camera(None, Default::default(), no_wake())?;
//! let mut encoder = backend.encoder(EncoderConfig::h264(640, 360, 30, 600_000), no_wake())?;
//! let frame = loop {
//!     if let Some(f) = camera.try_recv() {
//!         break f;
//!     }
//!     std::thread::sleep(std::time::Duration::from_millis(5));
//! };
//! encoder.submit(&frame, EncodeOptions::default())?;
//! let encoded = loop {
//!     if let Some(r) = encoder.poll() {
//!         break r?;
//!     }
//!     std::thread::sleep(std::time::Duration::from_millis(1));
//! };
//! assert!(encoded.keyframe && encoded.data.starts_with(&[0, 0, 0, 1]));
//! # Ok::<(), kessel_video::VideoError>(())
//! ```

// FFI to the OS frameworks is the only `unsafe`, in the platform modules.
#![deny(unsafe_code)]

mod api;
mod convert;
mod error;
pub mod fake;
mod frame;
pub mod h264;
pub mod kit;
mod null;
mod platform;
mod rust_codecs;
mod types;
#[cfg(feature = "vp8")]
mod vp8;

#[cfg(all(feature = "apple", target_os = "macos"))]
#[allow(unsafe_code)]
mod apple;

#[cfg(all(feature = "windows", windows))]
#[allow(unsafe_code)]
mod win;

#[cfg(all(feature = "linux", target_os = "linux"))]
#[allow(unsafe_code)]
mod linux;

// Runs the README's examples as doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub use api::{no_wake, Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake, EVENT_QUEUE};
/// The `bytes` crate, re-exported so callers use the same version.
pub use bytes;
pub use bytes::Bytes;
pub use convert::{convert, fit_within, psnr_y};
pub use error::{DecodeError, VideoError};
pub use frame::{tight_layout, ColorSpace, FrameBuffer, NativeBuffer, NativeFrame, PixelFormat, Planes, VideoFrame};
pub use kit::now_us;
pub use null::null;
pub use platform::{AndroidContext, SystemConfig};
#[cfg(any(feature = "apple", feature = "windows", feature = "linux", feature = "android"))]
pub use platform::system;
pub use types::*;
