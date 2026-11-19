//! Postal secrets on this box (prd-postal-in-k2-v1 §5.6): each handle's
//! identity key and the owner's decide token, in the vault's `system`
//! namespace, never in a table, a file an agent reads, or a log.
//!
//! - `postal.id.<24 hex of SHA-256(address)>`: the handle's Ed25519 seed
//!   (64 hex; vault names are `[A-Za-z0-9_.-]`, so the address is hashed).
//!   The same key signs mail, pairs and opens held mail sealed to it.
//! - `postal.decide.<label>`: the `k2c_pd_` token the owner approved for
//!   this box's label (the plane's postal-decide device grant). Only the
//!   daemon reads it, and only to confirm or revoke a pair after a
//!   person's answer. An agent's credentials can't confirm (the plane
//!   answers `owner_required`), and this token never reaches an agent.
//!
//! The daemon installs the vault holder at boot ([`install`]); tests
//! install [`MemorySecrets`]. With none installed every call is an error,
//! never a silent default.

use std::sync::{Arc, RwLock};

use postal_sdk::identity::Identity;

/// Where Postal keeps its secrets.
pub trait Secrets: Send + Sync {
    /// `Ok(None)`: nothing stored under `name`.
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn put(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

static HOLDER: RwLock<Option<Arc<dyn Secrets>>> = RwLock::new(None);

/// Install the secret holder (the daemon's vault; a test's memory).
pub fn install(s: Arc<dyn Secrets>) {
    *HOLDER.write().unwrap_or_else(|p| p.into_inner()) = Some(s);
}

fn holder() -> Result<Arc<dyn Secrets>, String> {
    HOLDER
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .ok_or_else(|| "postal_vault_closed: this server's vault isn't open, so Postal keys can't be read".to_string())
}

/// The vault name of a handle's identity seed.
pub fn identity_name(address: &str) -> String {
    use sha2::{Digest, Sha256};
    let h: String = Sha256::digest(address.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("postal.id.{h}")
}

/// The vault name of the owner's decide token for a label.
pub fn decide_name(label: &str) -> String {
    format!("postal.decide.{label}")
}

fn seed_of(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The handle's identity, if one was made.
pub fn identity(address: &str) -> Result<Option<Identity>, String> {
    match holder()?.get(&identity_name(address))? {
        None => Ok(None),
        Some(v) => seed_of(&v).map(|s| Some(Identity::from_seed(s))).ok_or_else(|| format!("postal_key_damaged: the stored key of {address} isn't a key")),
    }
}

/// The handle's identity, made (and stored, read back) when there is none.
pub fn identity_or_create(address: &str) -> Result<Identity, String> {
    if let Some(i) = identity(address)? {
        return Ok(i);
    }
    let mut seed = zeroize::Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *seed).map_err(|e| format!("postal key: the OS random source failed: {e}"))?;
    let hex: zeroize::Zeroizing<String> = zeroize::Zeroizing::new(seed.iter().map(|b| format!("{b:02x}")).collect());
    holder()?.put(&identity_name(address), &hex)?;
    identity(address)?.ok_or_else(|| "postal key: stored, but it doesn't read back".to_string())
}

pub fn decide_token(label: &str) -> Result<Option<String>, String> {
    holder()?.get(&decide_name(label))
}

pub fn set_decide_token(label: &str, token: &str) -> Result<(), String> {
    if !token.starts_with("k2c_pd_") {
        return Err("postal decide token: not a k2c_pd_ token".into());
    }
    holder()?.put(&decide_name(label), token)
}

pub fn clear_decide_token(label: &str) -> Result<(), String> {
    holder()?.delete(&decide_name(label))
}

/// In-memory secrets for tests.
#[derive(Default)]
pub struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl Secrets for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.get(name).cloned())
    }
    fn put(&self, name: &str, value: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.insert(name.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_key_is_made_once_and_reads_back_the_same() {
        install(Arc::new(MemorySecrets::default()));
        let a = format!("k{}::acme.postal.bot", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        assert!(identity(&a).unwrap().is_none());
        let one = identity_or_create(&a).unwrap();
        let two = identity_or_create(&a).unwrap();
        assert_eq!(one.fingerprint(), two.fingerprint());
        assert!(set_decide_token("acme", "pst_x").is_err(), "only a decide token is kept");
        set_decide_token("acme", "k2c_pd_0123").unwrap();
        assert_eq!(decide_token("acme").unwrap().as_deref(), Some("k2c_pd_0123"));
    }
}
