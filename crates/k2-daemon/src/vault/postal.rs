//! Postal's secrets (handle keys, the owner's decide token) as `system`
//! secrets in scope `engine:postal`: no route ever returns them, and an
//! agent can't resolve them (prd-postal-in-k2-v1 §5.6, §7).

use super::refstore::VaultRefStore;

struct Holder;

fn store() -> Result<VaultRefStore, String> {
    super::get().map(|v| VaultRefStore::new(v, "postal", "postal")).map_err(|e| format!("postal_vault_closed: vault {}", e.code))
}

impl k2_core::postal::keys::Secrets for Holder {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        store()?.resolve(name)
    }
    fn put(&self, name: &str, value: &str) -> Result<(), String> {
        store()?.put_exact(name, value)
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        store()?.delete(name)
    }
}

/// Install the holder (once, at daemon boot; it uses whichever vault is open).
pub fn install() {
    k2_core::postal::keys::install(std::sync::Arc::new(Holder));
}
