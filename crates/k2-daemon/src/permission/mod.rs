//! Permission cards, daemon side (`.k2/prds/prd-agent-settings-cards-v1.md`
//! rev 3.1). Shared types live in `k2_core::permission`.
//!
//! **P0:** every lane's file is pre-declared here as an empty stub, so lanes
//! own disjoint files and nobody edits this `mod.rs` again (PRD §12.2,
//! §12.3). Nothing here is wired: no route, hook or sweep calls into it, so
//! with the switches off K2 is unchanged (the off-goldens).
//!
//! | File | Lane |
//! |---|---|
//! | `routes`, `store`, `answer`, `limits`, `batch`, `inject`, `expiry` | A1 · core |
//! | `view` (`project()`, person arms) | A3 · projection |
//! | `routing`, `notes`, `undo`, `waiting`, `audit` | A2 · reach |
//! | `walls` | C · teach |
//! | `actions/{switches,connections,gardens}` | E1 · switches + Gardens |
//! | `actions/{held,compute}` | E2 · held requests |
//!
//! App guests never see action or permission cards (K2's rule,
//! 2026-10-21): every guest path drops them before any projection, so lane
//! F has no daemon module here (its contract rows, phone contract and golden
//! samples live elsewhere).
//!
//! Lane B's work (the gate, route tables, switches, void) lives in
//! `k2_core::permission::gate` and the existing route files, not here.
//! There are no info cards (rev 3.1): a machine-only refusal is a plain-text
//! hint in the refusal wrapper (`walls`, from the catalog's `machine_hints`).

// P0 stubs: lanes remove this as they fill their files.
#![allow(dead_code)]

pub(crate) mod actions;

// A1 · core
pub(crate) mod answer;
pub(crate) mod batch;
pub(crate) mod expiry;
pub(crate) mod inject;
pub(crate) mod limits;
/// A1 (S1a contract): the card's words, its stored view (`view_for`, which
/// A3's `project()` reuses per viewer), floors and approvers, and
/// `refresh_card` (every state change rewrites the same doc id and seq).
pub(crate) mod present;
pub(crate) mod routes;
pub(crate) mod store;
/// A1: test-only catalog actions (no E-lane action is needed to test the
/// core).
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod core_tests;

// A3 · projection
pub(crate) mod view;

// A2 · reach
pub(crate) mod audit;
pub(crate) mod notes;
pub(crate) mod routing;
pub(crate) mod undo;
pub(crate) mod waiting;

// C · teach
pub(crate) mod walls;
