//! `VaultRefStore` (KV11): the old mail and DB secret stores' ref API
//! (`store` → opaque ref, `resolve`, `delete`, `store_exact`) over the
//! vault's `system` namespace. Refs stay what they were (`mailsec_*`,
//! `account-<id>`, `ext-inbox-<id>`, `dbsec_*`), so no main-DB row
//! changes. Each ref is one `system` secret in scope `engine:<engine>`;
//! no route ever returns these values.

use std::sync::Arc;

use k2_vault::{
    Actor, Delivery, Kind, LocalVault, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore,
    Selector, Use,
};

pub struct VaultRefStore {
    vault: Arc<LocalVault>,
    engine: &'static str,
    subsystem: &'static str,
}

impl VaultRefStore {
    pub fn new(vault: Arc<LocalVault>, engine: &'static str, subsystem: &'static str) -> Self {
        VaultRefStore { vault, engine, subsystem }
    }

    fn actor(&self) -> Actor {
        Actor::System(self.subsystem.to_string())
    }

    fn sel(&self, name: &str) -> Selector {
        Selector::system(Scope::engine(self.engine), name)
    }

    pub fn put_exact(&self, key: &str, secret: &str) -> Result<(), String> {
        if secret.is_empty() {
            return Err("secret value must not be empty".into());
        }
        let mut r = PutRequest::new(Namespace::System, Scope::engine(self.engine), key, Kind::Password);
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::Env);
        r.origin = format!("system:{}", self.subsystem);
        r.mode = PutMode::Upsert;
        r.idempotent = true;
        self.vault
            .put(r, SecretBytes::from_slice(secret.as_bytes()), &self.actor())
            .map(|_| ())
            .map_err(|e| format!("vault: {}", e.code))
    }

    /// New random ref `<prefix>_<kind>_<hex12>`.
    pub fn store_new(&self, prefix: &str, kind: &str, secret: &str) -> Result<String, String> {
        let hex = k2_vault::crypto::hex(&k2_vault::crypto::random_bytes::<6>());
        let r = format!("{prefix}_{kind}_{hex}");
        self.put_exact(&r, secret)?;
        Ok(r)
    }

    pub fn resolve(&self, sref: &str) -> Result<Option<String>, String> {
        match self.vault.resolve(&self.sel(sref), Use::System, &Requester::System(self.subsystem.into())) {
            Ok(r) => String::from_utf8(r.value.expose().to_vec()).map(Some).map_err(|_| "stored secret isn't text".to_string()),
            Err(e) if e.status == 404 => Ok(None),
            Err(e) if e.code == "secret_not_active" => Ok(None),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }

    pub fn delete(&self, sref: &str) -> Result<(), String> {
        match self.vault.destroy(&self.sel(sref), &self.actor()) {
            Ok(()) => Ok(()),
            Err(e) if e.status == 404 => Ok(()),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }
}
