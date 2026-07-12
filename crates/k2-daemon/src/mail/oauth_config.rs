//! BYO OAuth client config (S1 — bring-your-own OAuth client for Email
//! Link). Storage + resolution for the daemon owner's OWN per-provider
//! OAuth client, which overrides the baked default at link/refresh time.
//!
//! **Split by sensitivity.** A client *id* is PUBLIC (it rides on every
//! auth/token request and ships inside installed apps), so it lives in
//! plain daemon-wide config — `app_settings` (`~/.k2/settings.json`), the
//! same place `mail_agent_send` / `mail_address_cap` / `mail_default_domain`
//! already live. The Gmail client *secret* is token-grade (Google Desktop
//! clients MUST send it at the token endpoint) and is handled EXACTLY like
//! the app-password path: the daemon **vault** (`FileSecretStore`, atomic
//! 0600 write), NEVER a DB column, NEVER logged, NEVER returned by any
//! endpoint. Microsoft is a TRUE public client — it has no secret at all.
//!
//! **Resolution seam.** [`oauth::client_id`] / [`oauth::client_secret`]
//! consult [`stored_client_id`] / [`stored_client_secret_default`] when a
//! caller passes no explicit override, so the effective order at use time
//! is: explicit call override > stored BYO override > baked default. The
//! production link/refresh callers keep passing `None`, so they now pick up
//! the owner's stored client automatically.

use crate::mail::oauth::OauthProvider;
use crate::mail::secrets::{FileSecretStore, SecretStore};

/// The daemon-vault key for the owner's BYO **Gmail** client secret.
/// Gmail Desktop clients require a token-endpoint secret; Microsoft is a
/// public client and has NONE (so there is no Microsoft key). Handled
/// exactly like the app-password path (`store_exact` / `delete`) — a plain
/// map key, never a scheme ref, never a DB column, never logged.
pub const GMAIL_CLIENT_SECRET_VAULT_KEY: &str = "oauth-client-secret-gmail";

/// The owner's stored BYO client id for `provider` (from `app_settings`),
/// or `None` when unset/blank → the baked default applies. Non-secret.
pub fn stored_client_id(provider: OauthProvider) -> Option<String> {
    let s = k2_core::app_settings::load();
    let raw = match provider {
        OauthProvider::Gmail => s.mail_oauth_gmail_client_id,
        OauthProvider::Microsoft => s.mail_oauth_microsoft_client_id,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// The owner's stored BYO client secret for `provider`, read from the
/// injected vault. **Gmail only** — Microsoft is a public client, so this
/// always returns `None` for it (no key is ever stored). A missing / blank
/// entry, or a vault read error, falls back to `None` (→ the baked default
/// applies) rather than crash the link/refresh hot path. The returned value
/// is a token-grade secret: callers MUST NEVER log it.
pub fn stored_client_secret(secrets: &dyn SecretStore, provider: OauthProvider) -> Option<String> {
    if provider != OauthProvider::Gmail {
        return None;
    }
    match secrets.resolve(GMAIL_CLIENT_SECRET_VAULT_KEY) {
        Ok(Some(s)) if !s.trim().is_empty() => Some(s),
        _ => None,
    }
}

/// Production convenience over [`stored_client_secret`]: read the Gmail BYO
/// secret from the real daemon vault (`FileSecretStore::default()`).
pub fn stored_client_secret_default(provider: OauthProvider) -> Option<String> {
    stored_client_secret(&FileSecretStore::default(), provider)
}

/// Is a BYO Gmail client secret currently set in the vault? Reports only
/// the boolean — it NEVER returns the value (the GET endpoint's #1 rule).
pub fn client_secret_is_set(secrets: &dyn SecretStore, provider: OauthProvider) -> bool {
    stored_client_secret(secrets, provider).is_some()
}

/// The `app_settings` camelCase key backing a provider's stored client id
/// (the key the set/clear routes feed to `app_settings::update`).
pub fn client_id_settings_key(provider: OauthProvider) -> &'static str {
    match provider {
        OauthProvider::Gmail => "mailOauthGmailClientId",
        OauthProvider::Microsoft => "mailOauthMicrosoftClientId",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    /// In-memory vault fake — mirrors the O1 engine's `MemStore`.
    #[derive(Default)]
    struct MemStore {
        map: StdMutex<HashMap<String, String>>,
    }
    impl SecretStore for MemStore {
        fn store(&self, _kind: &str, _secret: &str) -> Result<String, String> {
            Err("unused".into())
        }
        fn resolve(&self, k: &str) -> Result<Option<String>, String> {
            Ok(self.map.lock().unwrap().get(k).cloned())
        }
        fn delete(&self, k: &str) -> Result<(), String> {
            self.map.lock().unwrap().remove(k);
            Ok(())
        }
        fn store_exact(&self, k: &str, v: &str) -> Result<(), String> {
            self.map.lock().unwrap().insert(k.to_string(), v.to_string());
            Ok(())
        }
    }

    #[test]
    fn stored_client_secret_is_gmail_vault_only() {
        let store = MemStore::default();
        // Nothing vaulted → None for both providers.
        assert_eq!(stored_client_secret(&store, OauthProvider::Gmail), None);
        assert_eq!(stored_client_secret(&store, OauthProvider::Microsoft), None);
        assert!(!client_secret_is_set(&store, OauthProvider::Gmail));

        // Vault a Gmail secret → resolves for Gmail, still None for Microsoft
        // (a public client never has a secret).
        store
            .store_exact(GMAIL_CLIENT_SECRET_VAULT_KEY, "goog-secret-xyz")
            .unwrap();
        assert_eq!(
            stored_client_secret(&store, OauthProvider::Gmail).as_deref(),
            Some("goog-secret-xyz")
        );
        assert_eq!(stored_client_secret(&store, OauthProvider::Microsoft), None);
        assert!(client_secret_is_set(&store, OauthProvider::Gmail));

        // A blank vault entry is treated as unset (falls back to default).
        store.store_exact(GMAIL_CLIENT_SECRET_VAULT_KEY, "   ").unwrap();
        assert_eq!(stored_client_secret(&store, OauthProvider::Gmail), None);
    }

    #[test]
    fn settings_key_is_camel_case_per_provider() {
        assert_eq!(
            client_id_settings_key(OauthProvider::Gmail),
            "mailOauthGmailClientId"
        );
        assert_eq!(
            client_id_settings_key(OauthProvider::Microsoft),
            "mailOauthMicrosoftClientId"
        );
    }
}
