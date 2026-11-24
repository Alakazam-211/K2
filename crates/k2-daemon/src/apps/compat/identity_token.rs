//! Verified identity on the App's own connection (prd-app-supabase-compat-v1
//! §9.3, S3; lane A).
//!
//! Server code that talks to Postgres on `DATABASE_URL` used to say who the
//! user is with `k2.set_principal(uuid)`, which nothing checks. With the
//! layer on, it asks K2 instead:
//!
//! 1. `GET /cli/skin/identity-token` on the App's gateway link, with the
//!    person's own session pass as the bearer: a claims token for that
//!    person, good for 60 seconds.
//! 2. In its transaction: `SELECT k2.set_claims($token)`. Postgres checks
//!    the token (HMAC-SHA-256 under the database's claims key, which only
//!    the key's owner, the migrator, can read; the time; the database) and
//!    writes the claims lock for that transaction. `auth.uid()` and the
//!    rest then answer for that person.
//!
//! **Why an HMAC, not Ed25519** (the PRD's first sketch): Postgres has no
//! Ed25519 check without an extension K2 doesn't install. The key is a
//! table only its owner can read (`REVOKE ALL`, kept out of K2's privilege
//! sync and refused as a grant relation), and `k2.set_claims` refuses to
//! work while any other role could read it.
//!
//! Token: `k2c1.<hex of {"db","iat","exp","claims"}>.<hex of the MAC of the first two parts>`.

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::cli_response::CliResponse;

/// The route (a person's pass, on the App's gateway link only).
pub const ROUTE: &str = "/cli/skin/identity-token";

/// How long a claims token is good for.
pub const TOKEN_TTL_SECS: i64 = 60;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// A claims token for database `db`, under the claims key `key_hex` (as
/// the db secret store keeps it), issued at `iat`.
pub fn mint(db: &str, key_hex: &str, claims: &Value, iat: i64) -> Result<String, String> {
    let key = unhex(key_hex).filter(|k| k.len() >= 32).ok_or("the claims key is not 32+ bytes of hex")?;
    let payload = json!({ "db": db, "iat": iat, "exp": iat + TOKEN_TTL_SECS, "claims": claims }).to_string();
    let head = format!("{}.{}", k2_root_proto::k2_auth::CLAIMS_TOKEN_PREFIX, hex(payload.as_bytes()));
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|e| e.to_string())?;
    mac.update(head.as_bytes());
    Ok(format!("{head}.{}", hex(&mac.finalize().into_bytes())))
}

fn refusal(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

/// `GET /cli/skin/identity-token`: `pass` is the bearer; `on_link` = the
/// call came in on an App's gateway link.
pub fn handle(pass: &k2_core::skin::SkinPass, on_link: bool) -> CliResponse {
    if !on_link {
        return refusal("403 Forbidden", "gateway_link_only", "the identity token is for the App's own server, on its gateway link");
    }
    let Some(principal) = pass.principal_id.as_deref().filter(|_| pass.session) else {
        return refusal("403 Forbidden", "person_pass_required", "send the person's own session pass (the one their sign-in gave the App)");
    };
    let app = {
        let h = k2_core::db::shared();
        let c = h.lock();
        k2_core::deploy::store::get_app_by_service(&c, &pass.app).ok().flatten()
    };
    let Some(app) = app else {
        return refusal("404 Not Found", "app_not_found", "this pass is not for an App on this server");
    };
    let Some(stored) = super::door_config::load(&app.app_id).ok().flatten() else {
        return refusal("409 Conflict", "compat_off", format!("the Supabase-compatible layer is off for {} ({cli} app compat on)", app.app_id, cli = k2_core::brand::CLI_NAME));
    };
    let user = match k2_core::skin::compat::app_user(&app.service_id, principal) {
        Ok(Some(u)) if u.enabled => u,
        Ok(_) => return refusal("403 Forbidden", "not_a_user", format!("this person is not an enabled user of {}", app.app_id)),
        Err(e) => return CliResponse::internal_error(e),
    };
    let key = {
        use crate::sql::secrets::SecretStore as _;
        match crate::sql::secrets::FileSecretStore::default().resolve(&stored.claims_key_ref) {
            Ok(Some(k)) => k,
            Ok(None) => return refusal("409 Conflict", "compat_key_missing", format!("{}'s claims key is missing ({cli} app compat on {} puts it back)", app.app_id, app.app_id, cli = k2_core::brand::CLI_NAME)),
            Err(e) => return CliResponse::internal_error(e),
        }
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let claims = super::claims::for_user(&app.app_id, &user, Some(&pass.id));
    match mint(&stored.config.database, &key, &claims, now) {
        Ok(token) => CliResponse::ok_json(
            json!({ "ok": true, "claimsToken": token, "expiresIn": TOKEN_TTL_SECS, "database": stored.config.database, "use": "BEGIN; SELECT k2.set_claims($1); …; COMMIT;" })
                .to_string(),
        ),
        Err(e) => CliResponse::internal_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_prefix_payload_mac_in_hex_and_the_mac_is_hmac_sha256() {
        let key = "ab".repeat(32);
        let t = mint("ws_x_main", &key, &json!({"sub": "u1", "role": "authenticated"}), 1000).unwrap();
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "k2c1");
        let payload: Value = serde_json::from_slice(&unhex(parts[1]).unwrap()).unwrap();
        assert_eq!(payload, json!({"db": "ws_x_main", "iat": 1000, "exp": 1060, "claims": {"sub": "u1", "role": "authenticated"}}));
        let mut mac = Hmac::<Sha256>::new_from_slice(&unhex(&key).unwrap()).unwrap();
        mac.update(format!("k2c1.{}", parts[1]).as_bytes());
        assert_eq!(parts[2], hex(&mac.finalize().into_bytes()));
        assert!(mint("db", "abcd", &json!({}), 1).is_err(), "a short key is refused");
    }
}
