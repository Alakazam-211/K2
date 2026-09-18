//! Host-wide custom-domain inventory + on-box ACME certs.
//!
//! Routes live in [`routes`] via the thin [`crate::domain_routes`] shim.

pub mod acme;
pub mod bind;
pub mod routes;
pub mod status;
pub mod store;

pub use bind::{classify_bind_response, BindOutcome};
