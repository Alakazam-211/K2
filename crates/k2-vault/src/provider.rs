//! Upstream secret sources (§5.2). v1 ships the trait and `local` only;
//! outside vaults (`op://`, `bao://`, `aws-sm://`) come by demand and are
//! resolved through a local row so policy, grants and audit stay here.

use crate::error::{Result, VaultError};
use crate::secret::SecretBytes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderHealth {
    Ok,
    Unavailable(String),
}

pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn resolve(&self, external_ref: &str) -> Result<SecretBytes>;
    fn health(&self) -> ProviderHealth;
}

/// The built-in provider: values live in vault.db, so there is nothing to
/// resolve externally.
pub struct Local;

impl Provider for Local {
    fn id(&self) -> &str {
        "local"
    }

    fn resolve(&self, _external_ref: &str) -> Result<SecretBytes> {
        Err(VaultError::bad_request("provider_local", "local secrets resolve from vault.db"))
    }

    fn health(&self) -> ProviderHealth {
        ProviderHealth::Ok
    }
}

/// Is a provider id one K2 knows how to talk to?
pub fn known(id: &str) -> bool {
    id == "local"
}
