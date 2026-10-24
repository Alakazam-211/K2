//! The redactor moved to the shared `k2-redact` crate (compute secret-env
//! SE24b) so k2-node redacts exactly as the vault does. This module keeps
//! the old path for the daemon and the vault.

pub use k2_redact::*;
