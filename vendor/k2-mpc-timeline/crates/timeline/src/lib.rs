//! Reads timelines: the timeline pack for the multiplayer kit.
//!
//! - [`time`]: exact time. Rationals of a second, frame rates, timecode, and the parser a verb
//!   uses for a time a person or agent typed (off the frame grid is refused unless asked).
//! - [`model`]: the neutral timeline model every translator fills: timelines, tracks, items,
//!   markers and media, at absolute times, with keys in the kit's grammar, fingerprints of
//!   what the model understands, and digests of what it carries opaque.
//! - [`otio`], [`fcpxml`], [`prproj`]: read OTIO JSON, FCPXML and PRPROJ files into the
//!   model. Each reader refuses what it doesn't know with a typed [`ReadError`], under
//!   [`ReadLimits`].
//! - [`pack`]: each reader as the kit's `BlobPack` ([`OtioPack`], [`FcpxmlPack`],
//!   [`PrprojPack`]), indexing a version of a file into the kit's structure index: sequences,
//!   tracks, items and markers with their keys, fingerprints and carried digests.
//! - [`live`]: an OTIO timeline as a live doc on the kit's structured mode
//!   ([`live::TimelineLive`]), its tracks as lanes and its items as bars, the agent's verbs on
//!   it, and a write-back that splices the changes into the file. Off by default: a host opens a
//!   file live only when it chooses to.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod build;
pub mod error;
pub mod fcpxml;
mod json;
pub mod live;
pub mod model;
pub mod mutant;
pub mod otio;
pub mod pack;
pub mod prproj;
pub mod time;
mod xml;

#[cfg(feature = "fuzzing")]
pub mod fuzzing;

pub use error::{ReadError, ReadLimits};
pub use model::{Document, Format, Item, ItemKind, Key, Marker, Media, Timeline, Track};
pub use pack::{index_document, FcpxmlPack, OtioPack, PrprojPack};
pub use time::{parse_time, Rate, Snap, Time, TimeError, TimeInput, Timecode};
