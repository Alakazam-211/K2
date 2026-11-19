//! The pptx pack for the K2 multiplayer kit (`k2-mpc-base`): reads and writes pptx packages,
//! at tier 0.5 and, off by default, live.
//!
//! At tier 0.5 a `.pptx` is a tier 0 blob doc, and this pack adds
//! - [`deck`]: opening a deck (slides in presentation order by `sldId`, masters, layouts,
//!   notes), with typed refusals;
//! - [`slide`]: a shape tree split into shapes by their own ids, for splicing;
//! - [`index`]: the kit's structure index of slides and shapes;
//! - [`merge`]: the three-way merge of an app's save by shape, holding the agent's side of a
//!   clash;
//! - [`verbs`]: the agent's verbs, as registry data and as edits that splice only what they
//!   change;
//! - [`blob_verbs`]: the verbs on the kit's tier-0 path, each a plan of the byte ranges it
//!   replaces;
//! - [`PptxPack`]: the kit's [`k2_mpc_base::session::BlobPack`] for decks.
//!
//! Live, [`PptxLive`] ([`live`]) is the kit's [`k2_mpc_base::session::Pack`] for a deck held as
//! a live keyed doc: slides and shapes by their own ids, agent verbs on the store, the file
//! written back by splicing. For the renderer and the live view: [`view`] (the view index),
//! [`geom`] (transform boxes in page space) and [`render`] (the render hash).
//!
//! The package layer under them (a bounded package reader and byte-exact writer, the
//! three-way package merge) is the shared one for docx, pptx and xlsx, `k2-mpc-ooxml`'s
//! `k2-mpc-opc`.
//!
//! Everything here reads untrusted bytes. Nothing panics on them: a file this pack can't hold
//! is a typed [`Decline`], and the file stays at tier 0.
#![forbid(unsafe_code)]

pub mod blob_verbs;
pub mod deck;
pub mod geom;
pub mod index;
pub mod live;
pub mod merge;
pub mod mutant;
pub mod pack;
pub mod render;
pub mod slide;
#[cfg(feature = "testgen")]
pub mod testgen;
pub mod verbs;
pub mod view;
pub mod xmlx;

pub use k2_mpc_opc::{Decline, Limits};

pub use deck::Pptx;
pub use live::PptxLive;
pub use pack::{PptxPack, Since, HOLD_CLASHES};
