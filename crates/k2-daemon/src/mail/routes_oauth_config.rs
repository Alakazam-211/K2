//! `/cli/mail/oauth-config` (S1 — bring-your-own OAuth client) — the OWNER
//! sets/clears their OWN per-provider OAuth client, overriding the baked
//! default at link/refresh time.
//!
//! GATING: all three routes are owner/Primary surface. `set`/`clear` (POST)
//! are owner-or-admin-gated in the dispatcher (`is_owner_level_mutation`'s
//! `/cli/mail/oauth-config/` prefix); the GET has its own owner-or-admin
//! dispatcher clause (the `approvals/list` precedent). An agent token never
//! reads or writes the OAuth client config.
//!
//! **THE SECRET NEVER LEAVES THE DAEMON.** `set` vaults the Gmail client
//! secret via `FileSecretStore::store_exact` (atomic 0600, exactly the
//! app-password path); it is NEVER a DB column, NEVER logged, and NEVER
//! returned by ANY endpoint. GET reports only `secretSet: bool` (whether one
//! is vaulted) plus the NON-secret client id for display — never the secret
//! value (the #1 security rule). Microsoft is a public client: a
//! `clientSecret` for it is REJECTED.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::mail::oauth::OauthProvider;
use crate::mail::oauth_config;
use crate::mail::secrets::{FileSecretStore, SecretStore};

// ── Shared {code, hint} error contract (mirrors routes_link_oauth) ──────

fn error_response(status: &'static str, code: &str, hint: &str) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": { "code": code, "hint": hint },
        })
        .to_string(),
    }
}

fn ok_json(v: serde_json::Value) -> CliResponse {
    CliResponse::ok_json(v.to_string())
}

/// A one-key `app_settings::update` partial that sets a provider's stored
/// client id (the camelCase settings key → value). Built explicitly because
/// the `json!` macro can't take a dynamic path-call as an object key.
fn client_id_partial(provider: OauthProvider, value: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        oauth_config::client_id_settings_key(provider).to_string(),
        serde_json::Value::String(value.to_string()),
    );
    serde_json::Value::Object(map)
}

// ── POST /cli/mail/oauth-config/set ─────────────────────────────────────

/// Set body. **NO `Debug` derive** — `client_secret` is token-grade; a
/// derived `Debug` could leak it into a log line. It is only ever moved into
/// the vault, never formatted.
#[derive(serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SetBody {
    provider: String,
    client_id: String,
    client_secret: Option<String>,
}

/// POST `/cli/mail/oauth-config/set` `{provider, clientId, clientSecret?}` —
/// store the owner's own client id (config) and, for Gmail, vault the client
/// secret. Reverts nothing; overwrites in place. Microsoft is a public
/// client → a `clientSecret` is rejected.
pub fn handle_oauth_config_set(body: &[u8]) -> CliResponse {
    let b: SetBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    let provider = match OauthProvider::from_str(b.provider.trim()) {
        Some(p) => p,
        None => {
            return error_response(
                "400 Bad Request",
                "usage",
                "missing/unknown 'provider' — 'gmail' or 'microsoft'",
            )
        }
    };
    let client_id = b.client_id.trim();
    if client_id.is_empty() {
        return error_response(
            "400 Bad Request",
            "usage",
            "missing 'clientId' — your OAuth client id for this provider",
        );
    }
    let secret = b.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if provider == OauthProvider::Microsoft && secret.is_some() {
        return error_response(
            "400 Bad Request",
            "usage",
            "Microsoft is a PUBLIC client — it has no client secret; omit 'clientSecret'",
        );
    }

    // 1) Store the (non-secret) client id in daemon config.
    if let Err(e) = k2_core::app_settings::update(client_id_partial(provider, client_id)) {
        return error_response(
            "500 Internal Server Error",
            "engine",
            &format!("could not persist client id: {e}"),
        );
    }

    // 2) Gmail only: vault the client secret (atomic 0600), exactly like the
    //    app-password path. Never logged, never a column. If none is supplied
    //    we leave any existing vaulted secret untouched (id + secret rotate
    //    independently).
    if provider == OauthProvider::Gmail {
        if let Some(secret) = secret {
            let store = FileSecretStore::default();
            if let Err(e) =
                store.store_exact(oauth_config::GMAIL_CLIENT_SECRET_VAULT_KEY, secret)
            {
                return error_response(
                    "500 Internal Server Error",
                    "engine",
                    &format!("vault write failed: {e}"),
                );
            }
        }
    }

    let store = FileSecretStore::default();
    ok_json(serde_json::json!({
        "ok": true,
        "provider": provider.as_str(),
        "source": "custom",
        // Whether a BYO secret is now vaulted — never the value.
        "secretSet": oauth_config::client_secret_is_set(&store, provider),
    }))
}

// ── POST /cli/mail/oauth-config/clear ───────────────────────────────────

#[derive(serde::Deserialize, Default, Debug)]
#[serde(default, rename_all = "camelCase")]
struct ClearBody {
    provider: String,
}

/// POST `/cli/mail/oauth-config/clear` `{provider}` — delete the stored
/// client id and (Gmail) WIPE the vaulted client secret → reverts to the
/// baked default.
pub fn handle_oauth_config_clear(body: &[u8]) -> CliResponse {
    let b: ClearBody = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => {
            return error_response("400 Bad Request", "usage", &format!("invalid JSON body: {e}"))
        }
    };
    let provider = match OauthProvider::from_str(b.provider.trim()) {
        Some(p) => p,
        None => {
            return error_response(
                "400 Bad Request",
                "usage",
                "missing/unknown 'provider' — 'gmail' or 'microsoft'",
            )
        }
    };

    // Clear the (non-secret) client id back to empty → the baked default
    // resolves again.
    if let Err(e) = k2_core::app_settings::update(client_id_partial(provider, "")) {
        return error_response(
            "500 Internal Server Error",
            "engine",
            &format!("could not clear client id: {e}"),
        );
    }

    // Gmail only: wipe the vaulted secret (a no-op if none was set).
    if provider == OauthProvider::Gmail {
        let store = FileSecretStore::default();
        if let Err(e) = store.delete(oauth_config::GMAIL_CLIENT_SECRET_VAULT_KEY) {
            return error_response(
                "500 Internal Server Error",
                "engine",
                &format!("vault wipe failed: {e}"),
            );
        }
    }

    ok_json(serde_json::json!({
        "ok": true,
        "provider": provider.as_str(),
        "source": "default",
        "secretSet": false,
    }))
}

// ── GET /cli/mail/oauth-config ──────────────────────────────────────────

/// Build one provider's status: `{source, clientId, secretSet}`. `clientId`
/// is the NON-secret id shown for display (the owner's custom id when set,
/// else the baked default). `secretSet` reports only whether a BYO secret is
/// vaulted — **the secret value is NEVER included** (the #1 rule).
fn provider_status(secrets: &dyn SecretStore, provider: OauthProvider) -> serde_json::Value {
    let stored = oauth_config::stored_client_id(provider);
    let source = if stored.is_some() { "custom" } else { "default" };
    let client_id = stored
        .unwrap_or_else(|| provider.config().client_id_placeholder.to_string());
    serde_json::json!({
        "source": source,
        "clientId": client_id,
        "secretSet": oauth_config::client_secret_is_set(secrets, provider),
    })
}

/// GET `/cli/mail/oauth-config` → per-provider
/// `{source: 'default'|'custom', clientId, secretSet}`. NEVER the secret.
pub fn handle_oauth_config_get(_params: &HashMap<String, String>) -> CliResponse {
    let store = FileSecretStore::default();
    ok_json(serde_json::json!({
        "ok": true,
        "providers": {
            "gmail": provider_status(&store, OauthProvider::Gmail),
            "microsoft": provider_status(&store, OauthProvider::Microsoft),
        },
    }))
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests — real app_settings + a real (temp-HOME) vault, so the
// end-to-end storage/resolution path is exercised HERMETICALLY (no network,
// no touching the developer's real ~/.k2). The #1 assertion throughout: the
// secret VALUE never appears in any response body.
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;

    fn body_json(resp: &CliResponse) -> serde_json::Value {
        serde_json::from_str(&resp.body).expect("valid JSON body")
    }

    const GMAIL_SECRET: &str = "super-secret-google-value-DO-NOT-LEAK";

    #[test]
    fn set_gmail_stores_id_and_vaults_secret_get_hides_secret() {
        with_temp_home(|| {
            let resp = handle_oauth_config_set(
                serde_json::json!({
                    "provider": "gmail",
                    "clientId": "my-gmail-id.apps.googleusercontent.com",
                    "clientSecret": GMAIL_SECRET,
                })
                .to_string()
                .as_bytes(),
            );
            assert_eq!(resp.status, "200 OK", "{}", resp.body);
            assert_eq!(body_json(&resp)["secretSet"], true);
            // The set response never echoes the secret.
            assert!(!resp.body.contains(GMAIL_SECRET), "set leaked the secret: {}", resp.body);

            // The resolver reads the stored id + vaulted secret back.
            assert_eq!(
                oauth_config::stored_client_id(OauthProvider::Gmail).as_deref(),
                Some("my-gmail-id.apps.googleusercontent.com")
            );
            assert_eq!(
                oauth_config::stored_client_secret_default(OauthProvider::Gmail).as_deref(),
                Some(GMAIL_SECRET)
            );

            // GET reports custom + secretSet, WITHOUT the secret value.
            let get = handle_oauth_config_get(&HashMap::new());
            let g = body_json(&get);
            assert_eq!(g["providers"]["gmail"]["source"], "custom");
            assert_eq!(
                g["providers"]["gmail"]["clientId"],
                "my-gmail-id.apps.googleusercontent.com"
            );
            assert_eq!(g["providers"]["gmail"]["secretSet"], true);
            assert!(
                !get.body.contains(GMAIL_SECRET),
                "GET leaked the secret value: {}",
                get.body
            );
        });
    }

    #[test]
    fn clear_reverts_to_default_and_wipes_secret() {
        with_temp_home(|| {
            handle_oauth_config_set(
                serde_json::json!({
                    "provider": "gmail",
                    "clientId": "id-x.apps.googleusercontent.com",
                    "clientSecret": GMAIL_SECRET,
                })
                .to_string()
                .as_bytes(),
            );
            let resp = handle_oauth_config_clear(
                serde_json::json!({ "provider": "gmail" }).to_string().as_bytes(),
            );
            assert_eq!(resp.status, "200 OK", "{}", resp.body);
            assert_eq!(body_json(&resp)["source"], "default");
            assert_eq!(body_json(&resp)["secretSet"], false);

            // Resolver falls back to the baked default; the vault is wiped.
            assert_eq!(oauth_config::stored_client_id(OauthProvider::Gmail), None);
            assert_eq!(
                oauth_config::stored_client_secret_default(OauthProvider::Gmail),
                None
            );

            let get = handle_oauth_config_get(&HashMap::new());
            let g = body_json(&get);
            assert_eq!(g["providers"]["gmail"]["source"], "default");
            assert_eq!(g["providers"]["gmail"]["secretSet"], false);
            // Default display id is the baked placeholder.
            assert_eq!(
                g["providers"]["gmail"]["clientId"],
                OauthProvider::Gmail.config().client_id_placeholder
            );
        });
    }

    #[test]
    fn microsoft_sets_id_only_and_rejects_a_secret() {
        with_temp_home(|| {
            // A clientSecret for Microsoft is rejected (public client).
            let bad = handle_oauth_config_set(
                serde_json::json!({
                    "provider": "microsoft",
                    "clientId": "ms-id",
                    "clientSecret": "should-be-rejected",
                })
                .to_string()
                .as_bytes(),
            );
            assert_eq!(bad.status, "400 Bad Request", "{}", bad.body);
            assert_eq!(body_json(&bad)["error"]["code"], "usage");
            // Nothing was stored on the rejected call.
            assert_eq!(oauth_config::stored_client_id(OauthProvider::Microsoft), None);

            // Id-only set works.
            let ok = handle_oauth_config_set(
                serde_json::json!({ "provider": "microsoft", "clientId": "ms-id" })
                    .to_string()
                    .as_bytes(),
            );
            assert_eq!(ok.status, "200 OK", "{}", ok.body);
            assert_eq!(
                oauth_config::stored_client_id(OauthProvider::Microsoft).as_deref(),
                Some("ms-id")
            );

            let get = handle_oauth_config_get(&HashMap::new());
            let g = body_json(&get);
            assert_eq!(g["providers"]["microsoft"]["source"], "custom");
            assert_eq!(g["providers"]["microsoft"]["clientId"], "ms-id");
            // Microsoft never has a secret.
            assert_eq!(g["providers"]["microsoft"]["secretSet"], false);
        });
    }

    #[test]
    fn set_validates_provider_and_client_id() {
        // Bad JSON, unknown provider, and a blank client id are all 400s.
        assert_eq!(handle_oauth_config_set(b"not json").status, "400 Bad Request");
        assert_eq!(
            handle_oauth_config_set(
                serde_json::json!({ "provider": "yahoo", "clientId": "x" })
                    .to_string()
                    .as_bytes()
            )
            .status,
            "400 Bad Request"
        );
        assert_eq!(
            handle_oauth_config_set(
                serde_json::json!({ "provider": "gmail", "clientId": "   " })
                    .to_string()
                    .as_bytes()
            )
            .status,
            "400 Bad Request"
        );
    }

    /// The secret VALUE must never appear in the serialization of ANY
    /// response struct/body this module produces — set, clear, or get.
    #[test]
    fn no_endpoint_ever_serializes_the_secret_value() {
        with_temp_home(|| {
            let set = handle_oauth_config_set(
                serde_json::json!({
                    "provider": "gmail",
                    "clientId": "id.apps.googleusercontent.com",
                    "clientSecret": GMAIL_SECRET,
                })
                .to_string()
                .as_bytes(),
            );
            let get = handle_oauth_config_get(&HashMap::new());
            let clear = handle_oauth_config_clear(
                serde_json::json!({ "provider": "gmail" }).to_string().as_bytes(),
            );
            for resp in [&set, &get, &clear] {
                assert!(
                    !resp.body.contains(GMAIL_SECRET),
                    "a response body leaked the secret: {}",
                    resp.body
                );
            }
        });
    }
}
