//! Reads PSD and PSB files: the file-type pack for layered images in the multiplayer kit.
//!
//! - [`PsdPack`]: the kit's [`k2_mpc_base::session::BlobPack`] for these files. The kit keeps
//!   each version as one blob; the pack reads a version into the kit's structure index, a tree
//!   of layers ([`pack`] says what a node holds), and refuses agent writes: agents propose a
//!   new file and a comment, they never write one. Its half of an anchor's state (is a layer
//!   still the anchored one, which layers to suggest when it's gone) is in [`anchor`].
//! - [`walk_records`]: the records-only reader it reads with, streaming a file of any size and
//!   holding only its layer records. It lives in the vendored parser (`vendor/photocraft-psd`,
//!   whose `VENDORED.md` says what changed there).
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod anchor;
pub mod pack;

pub use pack::{index_psd, PsdPack};
pub use photocraft_psd::records::{
    walk_records, LayerSource, WalkError, WalkLimits, WalkOptions, WalkSink, Walked, WalkedChannel,
    WalkedLayer,
};

#[cfg(feature = "differential")]
pub mod differential;
