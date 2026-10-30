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
//! characters without padding (S29 G9).
//!
//! The recovery key (Rosson 2026-10-30: "move that to the vault"): K2
//! keeps a copy of the current master key in the vault under the fixed
//! name [`RECOVERY_NAME`] (`system`, scope `engine:sew`, delivered to
//! nobody), so the owner no longer saves it by hand: the vault's own
//! recovery key and backups cover it. It is the one system secret a person
//! may reveal (`k2_vault::policy::PERSON_REVEALABLE_SYSTEM`): the Owner or
//! an Admin, with a confirm, in `k2 hostchat recovery-key` or Settings →
//! Vault. Agents never see it, read it or grant it.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use k2_vault::{Actor, HumanProof};

/// The recovery key's vault name (scope `engine:sew`, `system`).
pub const RECOVERY_NAME: &str = "sew-recovery-key";
/// Its label in Settings → Vault and `k2 vault list --system`.
pub const RECOVERY_LABEL: &str = "Sew recovery key (chat/voice server)";

pub trait SewSecrets: Send + Sync {
    fn store(&self, kind: &str, secret: &str) -> Result<String, String>;
    fn resolve(&self, sref: &str) -> Result<Option<String>, String>;
    fn delete(&self, sref: &str) -> Result<(), String>;
    /// Write the recovery key copy ([`RECOVERY_NAME`]); the same value
    /// again adds no version.
    fn put_recovery(&self, key: &str) -> Result<(), String>;
    /// The recovery key copy, for the daemon only (sync checks).
    fn recovery(&self) -> Result<Option<String>, String>;
    /// The recovery key for a person: the vault's own reveal (Owner/Admin
    /// with a confirm; audited as a vault reveal). `Ok(None)`: no copy.
    fn reveal_recovery(&self, actor: &Actor, proof: &HumanProof) -> Result<Option<String>, (String, String)>;
    fn delete_recovery(&self) -> Result<(), String>;
}

/// The live store: the open vault, looked up on every call.
pub struct VaultSecrets;

fn vault() -> Result<std::sync::Arc<k2_vault::LocalVault>, String> {
    #[cfg(test)]
    let v = crate::vault::ensure_test_vault();
    #[cfg(not(test))]
    let v = crate::vault::get().map_err(|e| format!("the K2 vault isn't open ({}): {}", e.code, e.hint))?;
    Ok(v)
}

fn refstore() -> Result<crate::vault::refstore::VaultRefStore, String> {
    Ok(crate::vault::refstore::VaultRefStore::new(vault()?, "sew", "sew"))
}

fn recovery_sel() -> k2_vault::Selector {
    k2_vault::Selector::system(k2_vault::Scope::engine("sew"), RECOVERY_NAME)
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
    fn put_recovery(&self, key: &str) -> Result<(), String> {
        use k2_vault::{Delivery, Kind, Namespace, PutMode, PutRequest, Scope, SecretBytes, SecretStore};
        if key.is_empty() {
            return Err("the recovery key is empty".into());
        }
        let mut r = PutRequest::new(Namespace::System, Scope::engine("sew"), RECOVERY_NAME, Kind::Password);
        // Delivered to nobody, ever: a person reveals it, nothing else.
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::None);
        r.origin = "system:sew".into();
        r.description = RECOVERY_LABEL.into();
        r.mode = PutMode::Upsert;
        r.idempotent = true;
        vault()?
            .put(r, SecretBytes::from_slice(key.as_bytes()), &Actor::System("sew".into()))
            .map(|_| ())
            .map_err(|e| format!("vault: {}", e.code))
    }
    fn recovery(&self) -> Result<Option<String>, String> {
        refstore()?.resolve(RECOVERY_NAME)
    }
    fn reveal_recovery(&self, actor: &Actor, proof: &HumanProof) -> Result<Option<String>, (String, String)> {
        use k2_vault::SecretStore;
        let v = vault().map_err(|e| ("vault_closed".to_string(), e))?;
        match v.reveal(&recovery_sel(), actor, proof) {
            Ok(b) => String::from_utf8(b.expose().to_vec()).map(Some).map_err(|_| ("vault_error".into(), "stored key isn't text".into())),
            Err(e) if e.status == 404 || e.code == "secret_destroyed" => Ok(None),
            Err(e) => Err((e.code.to_string(), e.hint.to_string())),
        }
    }
    fn delete_recovery(&self) -> Result<(), String> {
        refstore()?.delete(RECOVERY_NAME)
    }
}

/// In-memory store for the machine tests.
#[derive(Default)]
pub struct MemSecrets {
    pub map: Mutex<HashMap<String, String>>,
    pub fail_store: Mutex<bool>,
    n: Mutex<u32>,
    /// The recovery key copy, and every value written to it (versions).
    pub recovery: Mutex<Option<String>>,
    pub recovery_writes: Mutex<Vec<String>>,
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
    fn put_recovery(&self, key: &str) -> Result<(), String> {
        if *self.fail_store.lock().unwrap() {
            return Err("vault: vault_not_open".into());
        }
        let mut r = self.recovery.lock().unwrap();
        if r.as_deref() != Some(key) {
            *r = Some(key.to_string());
            self.recovery_writes.lock().unwrap().push(key.to_string());
        }
        Ok(())
    }
    fn recovery(&self) -> Result<Option<String>, String> {
        if *self.fail_store.lock().unwrap() {
            return Err("vault: vault_not_open".into());
        }
        Ok(self.recovery.lock().unwrap().clone())
    }
    /// The vault's own rule ([`k2_vault::policy::can_reveal_secret`]).
    fn reveal_recovery(&self, actor: &Actor, proof: &HumanProof) -> Result<Option<String>, (String, String)> {
        let Some(k) = self.recovery.lock().unwrap().clone() else { return Ok(None) };
        k2_vault::policy::can_reveal_secret(actor, k2_vault::Namespace::System, &k2_vault::Scope::engine("sew"), RECOVERY_NAME, proof)
            .map_err(|e| (e.code.to_string(), e.hint.to_string()))?;
        Ok(Some(k))
    }
    fn delete_recovery(&self) -> Result<(), String> {
        *self.recovery.lock().unwrap() = None;
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

    /// Rosson 2026-10-30: Sew's recovery key in the vault, owner-only. A
    /// `system` secret named `sew-recovery-key` (scope `engine:sew`,
    /// labelled, delivered to nobody); the Owner (or an Admin) reveals it
    /// with a confirm; an agent never sees it, reads it, grants it or
    /// destroys it, even with "manage server-wide secrets" on.
    #[test]
    fn the_recovery_key_is_an_owner_only_system_secret_in_the_vault() {
        use k2_vault::{AgentActor, Consumer, Delivery, GrantRequest, ListFilter, Namespace, Requester, Role, Scope, SecretStore, Use};
        let s = VaultSecrets;
        let v = crate::vault::ensure_test_vault();
        let key = generate_master_key().unwrap();
        s.put_recovery(&key).unwrap();
        let sel = recovery_sel();
        let m = v.meta(&sel, &Actor::Owner).expect("owner sees it");
        assert_eq!((m.namespace, m.scope.clone(), m.name.as_str()), (Namespace::System, Scope::engine("sew"), RECOVERY_NAME));
        assert_eq!((m.default_delivery, m.delivery_ceiling), (Delivery::None, Delivery::None));
        assert_eq!(m.description, RECOVERY_LABEL);
        assert_eq!(s.recovery().unwrap().as_deref(), Some(key.as_str()));
        // The same value again adds no version.
        let v1 = m.latest_version;
        s.put_recovery(&key).unwrap();
        assert_eq!(v.meta(&sel, &Actor::Owner).unwrap().latest_version, v1);

        // Owner and Admin reveal with a confirm; nobody without one.
        let ok = HumanProof::Confirmed { method: "vault-password".into() };
        assert_eq!(s.reveal_recovery(&Actor::Owner, &ok).unwrap().as_deref(), Some(key.as_str()));
        let admin = Actor::Login { username: "ana".into(), role: Role::Admin };
        assert_eq!(s.reveal_recovery(&admin, &ok).unwrap().as_deref(), Some(key.as_str()));
        assert_eq!(s.reveal_recovery(&Actor::Owner, &HumanProof::None).unwrap_err().0, "vault_confirm_required");
        let member = Actor::Login { username: "m".into(), role: Role::Member };
        assert_eq!(s.reveal_recovery(&member, &ok).unwrap_err().0, "vault_admin_required");

        // Agents: invisible, unreadable, ungrantable, unchangeable.
        for manage in [false, true] {
            let a = Actor::Agent(AgentActor { workspace_id: "w-sew".into(), session_id: Some("s1".into()), shell: false, vault_manage: manage });
            assert!(s.reveal_recovery(&a, &ok).is_err(), "reveal (manage={manage})");
            assert!(v.meta(&sel, &a).is_err(), "meta (manage={manage})");
            let all = v.list(&ListFilter { include_system: true, ..Default::default() }, &a).unwrap();
            assert!(all.iter().all(|m| m.name != RECOVERY_NAME), "listed (manage={manage})");
            let g = GrantRequest {
                selector: sel.clone(),
                consumer: Consumer::workspace("w-sew"),
                delivery: Delivery::Fetch,
                env_name: None,
                file_name: None,
                hosts_narrowed: None,
                expires_at: None,
                version: None,
            };
            assert!(v.grant(g, &a).is_err(), "grant (manage={manage})");
            assert!(v.destroy(&sel, &a).is_err(), "destroy (manage={manage})");
            let who = Requester::Session { workspace_id: "w-sew".into(), session_id: Some("s1".into()) };
            for u in [Use::Api, Use::Env, Use::File] {
                assert!(v.resolve(&sel, u, &who).is_err(), "resolve {u:?} (manage={manage})");
            }
        }
        // Settings → Vault offers Reveal on this row only.
        assert_eq!(crate::vault_routes::meta_json(&m)["personReveal"], true);
        let master = s.store("master", &key).unwrap();
        let mm = v.meta(&k2_vault::Selector::system(Scope::engine("sew"), master.clone()), &Actor::Owner).unwrap();
        assert!(crate::vault_routes::meta_json(&mm).get("personReveal").is_none(), "the master key itself stays never shown");
        s.delete(&master).unwrap();

        // Purge removes it; a later enable writes it again.
        s.delete_recovery().unwrap();
        assert_eq!(s.recovery().unwrap(), None);
        assert_eq!(s.reveal_recovery(&Actor::Owner, &ok).unwrap(), None);
        let key2 = generate_master_key().unwrap();
        s.put_recovery(&key2).unwrap();
        assert_eq!(s.recovery().unwrap().as_deref(), Some(key2.as_str()));
        s.delete_recovery().unwrap();
    }
}
