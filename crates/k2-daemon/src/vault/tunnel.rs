//! The K2 Connect bearer (`tunnel.json` `token`) as one `system` secret
//! `tunnel.token` in scope `engine:frpc` (KV24). It is both the relay
//! token and the k2.dev / connect.k2.dev API bearer; every caller of
//! `tunnel::config::load()` gets it resolved in memory. Splitting it is
//! control-plane work, later.

use std::sync::Arc;

use k2_vault::{LocalVault, VaultError};

use super::refstore::VaultRefStore;

pub const NAME: &str = "tunnel.token";

struct Holder(VaultRefStore);

impl k2_core::tunnel::config::TokenVault for Holder {
    fn get(&self) -> Option<String> {
        self.0.resolve(NAME).ok().flatten()
    }
    fn set(&self, token: &str) -> Result<(), String> {
        self.0.put_exact(NAME, token)
    }
    fn clear(&self) -> Result<(), String> {
        self.0.delete(NAME)
    }
}

/// Move the file token into the vault (verified), install the holder,
/// and rewrite `tunnel.json` without the token.
pub fn adopt(vault: &Arc<LocalVault>) -> Result<bool, VaultError> {
    let store = VaultRefStore::new(vault.clone(), "frpc", "tunnel");
    let file_token = k2_core::tunnel::config::file_token().map_err(|e| VaultError::internal(format!("tunnel.json: {e}")))?;
    let moved = !file_token.trim().is_empty();
    if moved {
        store.put_exact(NAME, file_token.trim()).map_err(VaultError::internal)?;
        if store.resolve(NAME).map_err(VaultError::internal)?.as_deref() != Some(file_token.trim()) {
            return Err(VaultError::internal("tunnel token read-back differs; tunnel.json kept"));
        }
    }
    let installed = k2_core::tunnel::config::install_token_vault(Box::new(Holder(VaultRefStore::new(vault.clone(), "frpc", "tunnel"))));
    if moved && installed {
        k2_core::tunnel::config::update(|_| {}).map_err(|e| VaultError::internal(format!("tunnel.json rewrite: {e}")))?;
    }
    Ok(moved)
}
