//! The one vault sealer behind both LLM login wallets
//! ([`k2_core::sealed_slots::SecretSealer`]): a server's saved logins
//! (scope `server`, `llm-login.<tool>.<id>`, origin
//! `system:llm-accounts`) and App users' wallets (scope
//! `person:<principal>`, `wallet-<item>`, origin `system:person-wallet`).
//!
//! Namespace `system`, delivery and ceiling `none`: nothing is ever
//! delivered from these items and no person or agent can reveal them;
//! only the daemon resolves them with `Use::System`. An Owner or Admin
//! sees the names in the vault list.
//!
//! Every call keeps "the vault is closed" (`Err`, e.g. `vault_not_open`,
//! `keychain_error`) apart from "no such item" (`Ok(None)`).

use k2_core::sealed_slots::{SealAddr, SealKind, SealScope, SealedState, SecretSealer};
use k2_vault::{Actor, Delivery, Kind, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStatus, SecretStore, Selector, Use};

/// The real sealer: calls the open vault ([`super::get`]) each time, so
/// a vault that opens late (the boot retry) is used as soon as it opens.
pub struct VaultSealer;

fn subsystem(addr: &SealAddr) -> &'static str {
    match addr.scope {
        SealScope::Server => "llm-accounts",
        SealScope::Person(_) => "person-wallet",
    }
}

fn scope(addr: &SealAddr) -> Scope {
    match &addr.scope {
        SealScope::Server => Scope::server(),
        SealScope::Person(p) => Scope::person(p.clone()),
    }
}

fn sel(addr: &SealAddr) -> Selector {
    Selector::system(scope(addr), addr.name.clone())
}

fn actor(addr: &SealAddr) -> Actor {
    Actor::System(subsystem(addr).into())
}

fn description(addr: &SealAddr) -> &'static str {
    match addr.scope {
        SealScope::Server => "A saved LLM login of this server (Settings → Language Models); daemon-only, never revealed.",
        SealScope::Person(_) => "A person's wallet item (App user); daemon-only.",
    }
}

impl SecretSealer for VaultSealer {
    fn put_sealed(&self, addr: &SealAddr, kind: SealKind, bytes: &[u8]) -> Result<(), String> {
        let vault = super::get().map_err(|e| format!("vault: {}", e.code))?;
        let k = match kind {
            SealKind::LoginSlot => Kind::LoginSlot,
            SealKind::ApiKey => Kind::ApiKey,
        };
        let mut r = PutRequest::new(Namespace::System, scope(addr), addr.name.clone(), k);
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::None);
        r.origin = format!("system:{}", subsystem(addr));
        r.description = description(addr).into();
        r.mode = PutMode::Upsert;
        r.idempotent = true;
        let out = vault
            .put(r, SecretBytes::from_slice(bytes), &actor(addr))
            .map_err(|e| format!("vault: {}", e.code))?;
        // A new version of a disabled item stays disabled; "seal" means
        // the value is the usable one, so turn it back on.
        if out.meta.status == SecretStatus::Disabled {
            vault.enable(&sel(addr), &actor(addr)).map_err(|e| format!("vault: {}", e.code))?;
        }
        Ok(())
    }

    fn get_sealed(&self, addr: &SealAddr) -> Result<Option<Vec<u8>>, String> {
        let vault = super::get().map_err(|e| format!("vault: {}", e.code))?;
        match vault.resolve(&sel(addr), Use::System, &Requester::System(subsystem(addr).into())) {
            Ok(r) => Ok(Some(r.value.expose().to_vec())),
            Err(e) if e.status == 404 || e.code == "secret_not_active" => Ok(None),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }

    fn disable_sealed(&self, addr: &SealAddr) -> Result<(), String> {
        let vault = super::get().map_err(|e| format!("vault: {}", e.code))?;
        match vault.disable(&sel(addr), &actor(addr)) {
            Ok(()) => Ok(()),
            Err(e) if e.status == 404 => Ok(()),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }

    fn destroy_sealed(&self, addr: &SealAddr) -> Result<(), String> {
        let vault = super::get().map_err(|e| format!("vault: {}", e.code))?;
        match vault.destroy(&sel(addr), &actor(addr)) {
            Ok(()) => Ok(()),
            Err(e) if e.status == 404 => Ok(()),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }

    fn sealed_state(&self, addr: &SealAddr) -> Result<Option<SealedState>, String> {
        let vault = super::get().map_err(|e| format!("vault: {}", e.code))?;
        match vault.meta(&sel(addr), &actor(addr)) {
            Ok(m) => Ok(match m.status {
                SecretStatus::Active => Some(SealedState::Active),
                SecretStatus::Disabled => Some(SealedState::Disabled),
                _ => None,
            }),
            Err(e) if e.status == 404 => Ok(None),
            Err(e) => Err(format!("vault: {}", e.code)),
        }
    }

    fn recovery_confirmed(&self) -> Option<bool> {
        let vault = super::get().ok()?;
        Some(vault.recovery_state().0 == "confirmed")
    }
}
