//! Signed URLs (PRD §10 "Signed URLs"): HS256 tokens, Supabase's shape.
//!
//! ```text
//! download  {"url": "<bucket>/<path>", "scope": "download", "iat", "exp"}
//! upload    {"url": "<bucket>/<path>", "scope": "upload", "owner", "upsert", "iat", "exp"}
//! ```
//!
//! - **The key** is per App: 32 random bytes the vault keeps as the system
//!   secret `engine:app-storage` / `url-key.<app id>`, made on first use.
//!   The daemon holds it in memory for at most [`KEY_TTL`].
//! - **No session in it.** A link works after the person who made it
//!   signed out, and there is no per-person cap (today's download tokens
//!   die with the session and stop at 256 per person). What it opens is
//!   the one path, until `exp`.
//! - **Rotation** ([`rotate`]) makes a new key: every link signed before
//!   stops at once (the CLI says so first). The cache is dropped in the
//!   same call.
//! - A token for another path, another scope, a forged or altered one, or
//!   an expired one is refused, each with Supabase's own error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use super::storage_err::{SResult, StorageError};

pub const ENGINE: &str = "app-storage";
/// How long the daemon keeps a key in memory between vault reads.
pub const KEY_TTL: Duration = Duration::from_secs(60);
/// The longest a signed URL may live (ten years: Supabase sets no cap; this
/// one only keeps the arithmetic sane).
pub const MAX_EXPIRES_IN: i64 = 10 * 365 * 86_400;

pub const SCOPE_DOWNLOAD: &str = "download";
pub const SCOPE_UPLOAD: &str = "upload";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlClaims {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upsert: Option<bool>,
    pub iat: i64,
    pub exp: i64,
}

pub fn key_name(app_id: &str) -> String {
    format!("url-key.{app_id}")
}

fn vault() -> SResult<Arc<k2_vault::LocalVault>> {
    #[cfg(test)]
    {
        Ok(crate::vault::ensure_test_vault())
    }
    #[cfg(not(test))]
    {
        crate::vault::get().map_err(|e| StorageError::unavailable(format!("the vault isn't open ({}): {}", e.code, e.hint)))
    }
}

fn cache() -> &'static Mutex<HashMap<String, (Vec<u8>, Instant)>> {
    static C: OnceLock<Mutex<HashMap<String, (Vec<u8>, Instant)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Serialises first use (two racing mints would leave two versions).
static KEY_LOCK: Mutex<()> = Mutex::new(());

/// The App's signing key (blocking: the vault).
pub fn key(app_id: &str) -> SResult<Vec<u8>> {
    if let Some((k, at)) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(app_id) {
        if at.elapsed() < KEY_TTL {
            return Ok(k.clone());
        }
    }
    let v = vault()?;
    let _g = KEY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let name = key_name(app_id);
    v.engine_ensure(ENGINE, &name, k2_vault::engine::Gen::Random32)
        .map_err(|e| StorageError::unavailable(format!("signing key: {}", e.hint)))?;
    let k = v.engine_value(ENGINE, &name).map_err(|e| StorageError::unavailable(format!("signing key: {}", e.hint)))?.expose().to_vec();
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(app_id.to_string(), (k.clone(), Instant::now()));
    Ok(k)
}

/// A new key for `app_id`: every outstanding link stops working.
pub fn rotate(app_id: &str) -> SResult<()> {
    let v = vault()?;
    let _g = KEY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    v.engine_rotate(ENGINE, &key_name(app_id), k2_vault::engine::Gen::Random32, |_| Ok(()))
        .map_err(|e| StorageError::unavailable(format!("rotate: {}", e.hint)))?;
    cache().lock().unwrap_or_else(|e| e.into_inner()).remove(app_id);
    Ok(())
}

/// Sign `claims` with `key`.
pub fn sign_with(key: &[u8], claims: &UrlClaims) -> SResult<String> {
    encode(&Header::new(Algorithm::HS256), claims, &EncodingKey::from_secret(key)).map_err(|e| StorageError::internal(format!("sign: {e}")))
}

/// Check `token` for `url` and `scope` against `key`, at `now`.
pub fn verify_with(key: &[u8], token: &str, url: &str, scope: &str, now: i64) -> SResult<UrlClaims> {
    let mut v = Validation::new(Algorithm::HS256);
    // The expiry is checked below against `now`, with no leeway.
    v.validate_exp = false;
    v.required_spec_claims = Default::default();
    let data = decode::<UrlClaims>(token, &DecodingKey::from_secret(key), &v).map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::InvalidSignature => StorageError::invalid_signature("Invalid signature"),
        _ => StorageError::invalid_jwt(format!("Invalid JWT: {e}")),
    })?;
    let c = data.claims;
    if c.exp <= now {
        return Err(StorageError::expired_signature());
    }
    // A token from before scopes were written is a download token.
    let got = c.scope.as_deref().unwrap_or(SCOPE_DOWNLOAD);
    if got != scope {
        return Err(StorageError::invalid_signature(format!("Token is not scoped for {scope}")));
    }
    if c.url != url {
        return Err(StorageError::invalid_signature("Invalid signature"));
    }
    Ok(c)
}

/// A download token for `bucket/name` that lives `expires_in` seconds.
pub fn sign_download(app_id: &str, url: &str, expires_in: i64, now: i64) -> SResult<String> {
    let exp_in = check_expires(expires_in)?;
    let claims = UrlClaims { url: url.to_string(), scope: Some(SCOPE_DOWNLOAD.into()), owner: None, upsert: None, iat: now, exp: now + exp_in };
    sign_with(&key(app_id)?, &claims)
}

/// An upload token: who the object will belong to and whether it may
/// replace one.
pub fn sign_upload(app_id: &str, url: &str, owner: Option<&str>, upsert: bool, expires_in: i64, now: i64) -> SResult<String> {
    let exp_in = check_expires(expires_in)?;
    let claims = UrlClaims {
        url: url.to_string(),
        scope: Some(SCOPE_UPLOAD.into()),
        owner: owner.map(str::to_string),
        upsert: Some(upsert),
        iat: now,
        exp: now + exp_in,
    };
    sign_with(&key(app_id)?, &claims)
}

pub fn verify(app_id: &str, token: &str, url: &str, scope: &str, now: i64) -> SResult<UrlClaims> {
    verify_with(&key(app_id)?, token, url, scope, now)
}

fn check_expires(s: i64) -> SResult<i64> {
    if !(1..=MAX_EXPIRES_IN).contains(&s) {
        return Err(StorageError::invalid_request(format!("expiresIn must be 1 to {MAX_EXPIRES_IN} seconds")));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn claims(url: &str, scope: &str, exp: i64) -> UrlClaims {
        UrlClaims { url: url.into(), scope: Some(scope.into()), owner: None, upsert: None, iat: NOW, exp }
    }

    #[test]
    fn a_good_token_opens_its_one_path() {
        let k = b"k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1";
        let t = sign_with(k, &claims("photos/org/s/p.jpg", SCOPE_DOWNLOAD, NOW + 60)).expect("sign");
        let c = verify_with(k, &t, "photos/org/s/p.jpg", SCOPE_DOWNLOAD, NOW).expect("verify");
        assert_eq!(c.exp, NOW + 60);
        assert_eq!(verify_with(k, &t, "photos/org/s/other.jpg", SCOPE_DOWNLOAD, NOW).expect_err("other path").code, "InvalidSignature");
        assert_eq!(verify_with(k, &t, "photos/org/s/p.jpg", SCOPE_UPLOAD, NOW).expect_err("other scope").code, "InvalidSignature");
    }

    #[test]
    fn expiry_is_exact() {
        let k = b"k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1";
        let t = sign_with(k, &claims("b/x", SCOPE_DOWNLOAD, NOW + 10)).expect("sign");
        verify_with(k, &t, "b/x", SCOPE_DOWNLOAD, NOW + 9).expect("one second left");
        let e = verify_with(k, &t, "b/x", SCOPE_DOWNLOAD, NOW + 10).expect_err("expired");
        assert_eq!(e.code, "ExpiredToken");
        assert_eq!(e.message, "The provided token has expired.");
    }

    #[test]
    fn tampering_and_another_key_are_refused() {
        let k = b"k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1";
        let t = sign_with(k, &claims("b/x", SCOPE_DOWNLOAD, NOW + 60)).expect("sign");
        let e = verify_with(b"k2k2k2k2k2k2k2k2k2k2k2k2k2k2k2k2", &t, "b/x", SCOPE_DOWNLOAD, NOW).expect_err("rotated key");
        assert_eq!(e.code, "InvalidSignature");
        // Swap the payload for one naming another path, keep the signature.
        let parts: Vec<&str> = t.split('.').collect();
        let forged_claims = sign_with(b"attacker-attacker-attacker-attac", &claims("b/secret", SCOPE_DOWNLOAD, NOW + 60)).expect("sign");
        let forged_payload = forged_claims.split('.').nth(1).expect("payload");
        let forged = format!("{}.{}.{}", parts[0], forged_payload, parts[2]);
        assert_eq!(verify_with(k, &forged, "b/secret", SCOPE_DOWNLOAD, NOW).expect_err("forged").code, "InvalidSignature");
        assert_eq!(verify_with(k, "not-a-token", "b/x", SCOPE_DOWNLOAD, NOW).expect_err("garbage").code, "InvalidJWT");
        // alg=none is never taken.
        let none = format!("eyJhbGciOiJub25lIn0.{}.", parts[1]);
        assert!(verify_with(k, &none, "b/x", SCOPE_DOWNLOAD, NOW).is_err(), "alg none");
    }

    #[test]
    fn vault_key_is_per_app_and_rotation_ends_old_links() {
        let app = format!("sign-test-{}", uuid::Uuid::new_v4().simple());
        let other = format!("sign-test-{}", uuid::Uuid::new_v4().simple());
        let t = sign_download(&app, "b/x", 3600, NOW).expect("sign");
        verify(&app, &t, "b/x", SCOPE_DOWNLOAD, NOW).expect("verify");
        assert!(verify(&other, &t, "b/x", SCOPE_DOWNLOAD, NOW).is_err(), "another App's key");
        rotate(&app).expect("rotate");
        let e = verify(&app, &t, "b/x", SCOPE_DOWNLOAD, NOW).expect_err("old link after rotation");
        assert_eq!(e.code, "InvalidSignature");
        let t2 = sign_download(&app, "b/x", 3600, NOW).expect("sign again");
        verify(&app, &t2, "b/x", SCOPE_DOWNLOAD, NOW).expect("new link");
    }

    #[test]
    fn upload_tokens_carry_owner_and_upsert() {
        let app = format!("sign-test-{}", uuid::Uuid::new_v4().simple());
        let t = sign_upload(&app, "b/x", Some("6f1c4a52-0b7d-4d0e-9a2c-3b8f1e7d9a10"), true, 7200, NOW).expect("sign");
        let c = verify(&app, &t, "b/x", SCOPE_UPLOAD, NOW).expect("verify");
        assert_eq!(c.owner.as_deref(), Some("6f1c4a52-0b7d-4d0e-9a2c-3b8f1e7d9a10"));
        assert_eq!(c.upsert, Some(true));
        assert!(verify(&app, &t, "b/x", SCOPE_DOWNLOAD, NOW).is_err(), "an upload token is not a download token");
        assert_eq!(sign_download(&app, "b/x", 0, NOW).expect_err("zero").code, "InvalidRequest");
        assert!(sign_download(&app, "b/x", MAX_EXPIRES_IN + 1, NOW).is_err());
    }
}
