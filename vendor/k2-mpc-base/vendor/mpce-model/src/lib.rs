//! Reference models for the MPC engine.
//!
//! Each primitive gets a deliberately simple implementation here (O(n²) is fine, no caches, no
//! run-length encoding). They are the oracle of record: the Quint models' ITF traces replay on
//! them, and the engine is compared against them on generated graphs.
//!
//! - [`tree`]: the Fugue tree (Algorithm 1) with FugueMax's right-sibling rule (Def. 6); it
//!   lives in `mpce-order` (the order layer) and is re-exported here unchanged;
//! - [`text`]: plain text: events, causal delivery, tombstones, the visible text; versions,
//!   restore, branches and purge;
//! - [`check`]: the automated Def. 4 checker and the strong list specification;
//! - [`tree_move`]: movable trees: [Move] Fig. 4 transcribed, FugueMax sibling order per
//!   parent, the causal skeleton, compaction, and the rescue projection;
//! - [`list`], [`mlist`], [`map`]: lists, movable lists and maps, composed from two layers
//!   that never see each other: the slot order (`mpce-order`) and the
//!   per-element registers (`mpce-reg`). These modules are the only place that uses both.
#![forbid(unsafe_code)]

pub mod check;
pub mod list;
pub mod map;
pub mod mlist;
pub mod text;
pub mod tree_move;

pub use mpce_order::collapsed;
pub use mpce_order::tree;
