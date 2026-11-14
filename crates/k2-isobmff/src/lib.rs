//! K2's own ISO BMFF / QuickTime reader and CMAF fMP4 writer: the container
//! layer of the video-derivatives worker, which parses untrusted media.
//!
//! - [`Mp4Reader`] reads ISO BMFF (MP4, CMAF, fragmented) and QuickTime MOV
//!   over any `Read + Seek` input, wherever `moov` sits: the movie and track
//!   headers, edit lists, the sample entries and their decoder
//!   configurations (`avcC`, `hvcC`, `av1C`, `dOps`, `esds`, PCM, ProRes
//!   identification, `tmcd`), and every sample's offset, size, decode and
//!   composition time and sync flag, from the `stbl` tables and from movie
//!   fragments. Every count and size is bounded by [`Limits`]; malformed
//!   input gives a typed [`Error`], never a panic or an allocation sized by
//!   a declared count.
//! - [`Fmp4Writer`] writes CMAF-style init and media segments for AV1, H.264
//!   and Opus, byte-identical for the same input.
//! - [`avcc_to_annexb`] turns length-prefixed H.264 samples into Annex B.
//!
//! Std only, no dependencies. Written from ISO/IEC 14496-12, -14 and -15,
//! the QuickTime File Format, the AV1 ISOBMFF binding and the Opus ISOBMFF
//! mapping.

#![forbid(unsafe_code)]

mod annexb;
mod bytes;
mod config;
mod entry;
mod error;
mod frag;
mod reader;
mod timecode;
mod track;
mod writer;

pub use annexb::avcc_to_annexb;
pub use config::{AacConfig, Av1Config, Av1SequenceHeader, AvcConfig, HevcConfig, OpusConfig};
pub use entry::{AudioInfo, Codec, Colour, Pcm, ProRes, SampleEntry, VideoInfo};
pub use error::{Error, FourCC, Limit, Limits, Result};
pub use frag::{SegmentIndex, SidxReference};
pub use reader::{FileType, Movie, Mp4Reader};
pub use timecode::{Timecode, TimecodeEntry};
pub use track::{Edit, EditList, Sample, Samples, Track, TrackExtends, TrackKind};
pub use writer::{FragmentSample, Fmp4Writer, InitConfig, SegmentOptions, TrackCodec, TrackFragment, TrackSetup};
