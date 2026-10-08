//! Where k2-core keeps LLM keys people configured before the vault
//! (`projects.anthropic_api_key`, `api_keys.anthropic_api_key`; K2 vault
//! §10.4, migration 0148). The daemon installs a vault-backed holder at
//! boot: new keys go into the vault and only a `k2v://` reference is
//! stored in the main DB. Without the holder (no vault), the old
//! plaintext column is used.

use std::sync::OnceLock;

pub trait SecretRefs: Send + Sync {
    /// Store a workspace's LLM key; returns its reference.
    fn store_workspace_key(&self, project_id: &str, key: &str) -> Result<String, String>;
    /// Store an API key's BYO LLM credential; returns its reference.
    fn store_api_key_credential(
        &self,
        api_key_id: &str,
        key: &str,
        provider: Option<&str>,
        base_url: Option<&str>,
    ) -> Result<String, String>;
    /// Resolve a reference (for spawn staging). Never logged.
    fn resolve(&self, sref: &str) -> Option<String>;
    fn delete(&self, sref: &str);
}

static HOLDER: OnceLock<Box<dyn SecretRefs>> = OnceLock::new();

pub fn install(h: Box<dyn SecretRefs>) -> bool {
    HOLDER.set(h).is_ok()
}

pub fn holder() -> Option<&'static dyn SecretRefs> {
    HOLDER.get().map(|b| b.as_ref())
}

/// A stored value: the reference when present (resolved through the
/// holder), else the plaintext column (pre-vault or no vault).
pub fn read(plaintext: Option<String>, sref: Option<String>) -> Option<String> {
    if let Some(r) = sref.filter(|r| !r.trim().is_empty()) {
        return holder().and_then(|h| h.resolve(&r)).filter(|k| !k.trim().is_empty());
    }
    plaintext.filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string())
}
