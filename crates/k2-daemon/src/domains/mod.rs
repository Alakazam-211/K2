//! Host-wide custom-domain inventory + control-plane bind (Slice A).
//!
//! Routes live in [`routes`] via the thin [`crate::domain_routes`] shim.

pub mod bind;
pub mod routes;

pub use bind::{classify_bind_response, BindOutcome};
