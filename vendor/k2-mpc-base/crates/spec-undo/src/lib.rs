//! The undo spec's oracle and its trace test.
//!
//! - [`model`]: `spec_undo::model`, the **oracle of record** for the undo spec
//!   (`docs/SPEC-UNDO.md`, U1–U22, O1–O5) over plain strings and hunks. It calls only the
//!   kit's diff pipeline (D1–D2); everything else is written from the spec.
//! - The trace-equivalence test (`tests/traces.rs`) replays seeded traces through the oracle
//!   and through the kit's real sessions on Yrs, and compares them after every step.
//!
//! Never published; nothing depends on it.

pub mod bounded;
pub mod itf;
pub mod model;
