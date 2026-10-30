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

/// The run record's `job` field (`tools/evidence/`): the job id K2 Compute's nodes set in
/// `K2_COMPUTE_JOB`, as a JSON string, or `null` off a node (then `collect.py` fills it in).
/// Only `[A-Za-z0-9_-]` is taken, so the record stays valid JSON.
pub fn runrec_job() -> String {
    match std::env::var("K2_COMPUTE_JOB") {
        Ok(j)
            if !j.is_empty()
                && j.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
        {
            format!("\"{j}\"")
        }
        _ => "null".to_owned(),
    }
}
