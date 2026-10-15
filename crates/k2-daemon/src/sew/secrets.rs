//! Sew's master keys in the K2 vault (plan-sew-s27-s29 §1: "the vault
//! replaces `~/.k2/sew-secrets.json`").
//!
//! Each key is one `system` secret in scope `engine:sew`, named by an
//! opaque ref `sewsec_<kind>_<hex>` that the `sew_server` row records
//! (`master`, `master-next` during a rotation; the retiring key stays under
//! its old ref until `drop-prev`). No route returns these values except the
//! person-only recovery-key reveal. There is no file fallback: a box whose
//! vault isn't open can't enable Sew.
//!
//! The key text is Sew's recovery form: 32 random bytes as 43 base64url
//! characters without padding (S29 G9). The vault is on the box, so it is
//! not escrow (G15): the recovery key is the off-box copy.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;

pub trait SewSecrets: Send + Sync {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String>;
    fn resolve(&self, sref: &str) -> Result<Option<String>, String>;
    fn delete(&self, sref: &str) -> Result<(), String>;
}

/// The live store: the open vault, looked up on every call.
pub struct VaultSecrets;

fn refstore() -> Result<crate::vault::refstore::VaultRefStore, String> {
    #[cfg(test)]
    let v = crate::vault::ensure_test_vault();
    #[cfg(not(test))]
    let v = crate::vault::get().map_err(|e| format!("the K2 vault isn't open ({}): {}", e.code, e.hint))?;
    Ok(crate::vault::refstore::VaultRefStore::new(v, "sew", "sew"))
}

impl SewSecrets for VaultSecrets {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        refstore()?.store_new("sewsec", kind, secret)
    }
    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        refstore()?.resolve(sref)
    }
    fn delete(&self, sref: &str) -> Result<(), String> {
        refstore()?.delete(sref)
    }
}

/// In-memory store for the machine tests.
#[derive(Default)]
pub struct MemSecrets {
    pub map: Mutex<HashMap<String, String>>,
    pub fail_store: Mutex<bool>,
    n: Mutex<u32>,
}

impl SewSecrets for MemSecrets {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String> {
        if *self.fail_store.lock().unwrap() {
            return Err("vault: vault_not_open".into());
        }
        let mut n = self.n.lock().unwrap();
        *n += 1;
        let r = format!("sewsec_{kind}_{:012x}", *n);
        self.map.lock().unwrap().insert(r.clone(), secret.to_string());
        Ok(r)
    }
    fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        if *self.fail_store.lock().unwrap() {
            return Err("vault: vault_not_open".into());
        }
        Ok(self.map.lock().unwrap().get(sref).cloned())
    }
    fn delete(&self, sref: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(sref);
        Ok(())
    }
}

/// A fresh master key: 32 bytes from the OS, base64url without padding.
pub fn generate_master_key() -> Result<String, String> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|e| format!("random: {e}"))?;
    let s = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    b.iter_mut().for_each(|x| *x = 0);
    Ok(s)
}

/// The 32 key bytes of a master key text (43 base64url, optional `\n`).
pub fn decode_master_key(text: &str) -> Result<[u8; 32], String> {
    if !k2_root_proto::sew::master_key_text_ok(text) {
        return Err("a Sew master key is 43 base64url characters".into());
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('\n'))
        .map_err(|_| "a Sew master key is 43 base64url characters".to_string())?;
    raw.try_into().map_err(|_| "a Sew master key is 32 bytes".to_string())
}

/// The KEK id Sew reports for this key (`status.keys.kek_id`, Sew S4 §5.3).
pub fn kek_id(text: &str) -> Result<String, String> {
    let key = decode_master_key(text)?;
    let derived = blake3::derive_key(k2_root_proto::sew::KEK_ID_CONTEXT, &key);
    Ok(k2_root_proto::sew::kek_id_from_derived(&derived))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_43_base64url_and_unique() {
        let a = generate_master_key().unwrap();
        let b = generate_master_key().unwrap();
        assert_ne!(a, b);
        for k in [&a, &b] {
            assert!(k2_root_proto::sew::master_key_text_ok(k), "{k}");
            assert_eq!(decode_master_key(k).unwrap().len(), 32);
        }
        assert!(decode_master_key("short").is_err());
    }

    /// The KEK id is Sew's formula over the 32 raw bytes, 16 lowercase hex
    /// with the top bit cleared. Fixed vector: an all-zero key.
    #[test]
    fn kek_id_matches_the_s4_formula() {
        let zero = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 32]);
        let id = kek_id(&zero).unwrap();
        let d = blake3::derive_key("sew 2026-09-27 kek id v1", &[0u8; 32]);
        let want = u64::from_be_bytes(d[..8].try_into().unwrap()) & 0x7fff_ffff_ffff_ffff;
        assert_eq!(id, format!("{want:016x}"));
        assert_eq!(id.len(), 16);
        assert!(u64::from_str_radix(&id, 16).unwrap() < 1 << 63);
        assert_eq!(kek_id(&format!("{zero}\n")).unwrap(), id);
    }

    #[test]
    fn the_vault_store_round_trips_under_engine_sew() {
        let s = VaultSecrets;
        let key = generate_master_key().unwrap();
        let r = s.store("master", &key).unwrap();
        assert!(r.starts_with("sewsec_master_"), "{r}");
        assert_eq!(s.resolve(&r).unwrap().as_deref(), Some(key.as_str()));
        s.delete(&r).unwrap();
        assert_eq!(s.resolve(&r).unwrap(), None);
    }
}
