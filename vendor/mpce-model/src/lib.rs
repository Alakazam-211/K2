//! Reference models for the MPC engine.
//!
//! Each primitive gets a deliberately simple implementation here (O(n²) is fine, no caches, no
//! run-length encoding). They are the oracle of record: the Quint models' ITF traces replay on
//! them, and the engine is compared against them on generated graphs.
//!
//! - [`tree`]: the Fugue tree (Algorithm 1) with FugueMax's right-sibling rule (Def. 6);
//! - [`text`]: plain text: events, causal delivery, tombstones, the visible text; versions,
//!   restore, branches and purge;
//! - [`check`]: the automated Def. 4 checker and the strong list specification.
#![forbid(unsafe_code)]

pub mod check;
pub mod text;
pub mod tree;
