//! Sew chat passes (Sew S28, K2 work item K6): the ES256 keyring that
//! signs short-lived chat passes, the public `GET /v1/chat/jwks`, the
//! rotation tick, and the owner key routes under `/cli/sew/keys*`.
//!
//! - [`keyring`]: keys as K2 vault entries (`system`, `engine:chat-pass`),
//!   lifecycle `next` → `current` → `retiring`, pure [`keyring::plan`].
//! - [`maint`]: the daemon-owned background tick (headless-safe). It only
//!   maintains an existing keyring; it never creates one.
//! - [`routes`]: the JWKS handler and the owner key routes.
//! - [`sign_chat_pass`] / [`verify_with_jwks`]: the signer K7's
//!   `POST /cli/sew/pass` uses, and the local verifier tests use (the same
//!   checks Sew S8 makes, S28 §6 / §18.1).
//!
//! ## What a K2 signature proves (KM13 §19a)
//!
//! **A chat pass, or anything else this keyring signs, is never proof that
//! a person acted.** It says "the K2 daemon on this box vouched for this
//! identity at this time", nothing more. On shared-user boxes every agent
//! runs as the daemon's OS user and can read the owner token and the vault
//! file, and on most boxes agents have passwordless sudo, so whoever holds
//! the box can make this key sign. Sew must not treat a pass (or a
//! `Sew-Actor` header, or `k2:owner`) as a person's consent. Steps that
//! need a person (KM13 widget approvals, an emergency key wipe here) need
//! a separate person-proof: the vault's confirm (`vault/human.rs`, Touch
//! ID, the vault password, a Connect password) and, per KM13 v1.5, the
//! person's passkey on `id.k2.dev`. KM13's approval attestations get their
//! own key and their own JWKS URL ([`APPROVAL_JWKS_PATH_RESERVED`]), never
//! this one.

pub mod keyring;
pub mod maint;
pub mod routes;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

pub use keyring::KeyError;

/// `iss` of every chat pass. Never `k2-host-sessions`.
pub const CHAT_PASS_ISSUER: &str = "k2-chat-pass";
/// JOSE `typ` of every chat pass.
pub const CHAT_PASS_TYP: &str = "k2-chat-pass+jwt";
/// `exp - iat` of a pass K2 mints.
pub const PASS_TTL_SECS: i64 = 60;
/// The longest pass a verifier accepts (C6).
pub const MAX_PASS_LIFETIME_SECS: i64 = 120;
/// Clock skew a verifier allows on `exp` and `iat`.
pub const VERIFY_SKEW_SECS: i64 = 30;
/// Public JWKS of the chat-pass keys.
pub const JWKS_PATH: &str = "/v1/chat/jwks";
/// Reserved for KM13's widget-approval key (own keyring, own URL). Not
/// served yet: it falls to the dark `/v1/` arm like any unknown path.
pub const APPROVAL_JWKS_PATH_RESERVED: &str = "/v1/chat/jwks/approval";
/// JWKS caching. Five minutes is half the publish lead (a `next` key is in
/// the JWKS ≥ 10 min before it signs), so a cached copy always holds the
/// key a pass names except after an emergency rotation, when Sew refetches
/// on the unknown `kid`.
pub const JWKS_CACHE_CONTROL: &str = "public, max-age=300";

/// The seven claims of a chat pass (S28 §6), exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatPassClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub sid: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

fn is_install_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 19 && (b'1'..=b'9').contains(&b[0]) && b.iter().all(u8::is_ascii_digit)
}

fn is_sid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// 32 lowercase hex characters (128 random bits).
pub fn new_jti() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| format!("os rng unavailable for a jti: {e}"))?;
    Ok(crate::es256::bytes_to_hex(&b))
}

impl ChatPassClaims {
    /// A 60-second pass for `sub` / `sid`, audience `sew:<install_id>`.
    pub fn new(install_id: &str, sub: &str, sid: &str, now: i64) -> Result<ChatPassClaims, String> {
        let c = ChatPassClaims {
            iss: CHAT_PASS_ISSUER.into(),
            aud: format!("sew:{install_id}"),
            sub: sub.into(),
            sid: sid.into(),
            iat: now,
            exp: now + PASS_TTL_SECS,
            jti: new_jti()?,
        };
        c.validate()?;
        Ok(c)
    }

    /// The shape rules K2 checks before signing.
    pub fn validate(&self) -> Result<(), String> {
        if self.iss != CHAT_PASS_ISSUER {
            return Err(format!("iss must be {CHAT_PASS_ISSUER}"));
        }
        match self.aud.strip_prefix("sew:") {
            Some(id) if is_install_id(id) => {}
            _ => return Err("aud must be sew:<install_id> (decimal, no leading zero)".into()),
        }
        if self.sub.is_empty() || self.sub.len() > 256 {
            return Err("sub must be 1-256 characters".into());
        }
        if !is_sid(&self.sid) {
            return Err("sid must match [A-Za-z0-9_.-]{1,128}".into());
        }
        let life = self.exp - self.iat;
        if life <= 0 || life > MAX_PASS_LIFETIME_SECS {
            return Err(format!("exp - iat must be 1..={MAX_PASS_LIFETIME_SECS}"));
        }
        if self.jti.len() != 32 || !self.jti.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err("jti must be 32 lowercase hex characters".into());
        }
        Ok(())
    }
}

/// Sign one chat pass with the daemon keyring's `current` key. The
/// keyring is created on first use. For K7's `POST /cli/sew/pass`.
pub fn sign_chat_pass(claims: &ChatPassClaims) -> Result<String, KeyError> {
    keyring::global()?.sign(claims, keyring::now())
}

/// The daemon keyring's public JWKS (`{"keys":[…]}`).
pub fn public_jwks() -> Result<serde_json::Value, KeyError> {
    keyring::global()?.public_jwks(keyring::now())
}

/// The URL Sew fetches the JWKS from on this box (S29 passes it to
/// `sew install`).
pub fn jwks_url(daemon_port: u16) -> String {
    format!("http://127.0.0.1:{daemon_port}{JWKS_PATH}")
}

/// Verify a pass the way Sew S8 does (S28 §6 table), minus the `jti`
/// replay cache and provisioning: header exactly `{alg: ES256, typ, kid}`,
/// `kid` in `jwks`, signature, `iss`, `aud == expected_aud`, exactly the
/// seven claims, `exp - iat ≤ 120`, and `now` within 30 s skew.
pub fn verify_with_jwks(token: &str, jwks: &serde_json::Value, expected_aud: &str, now: i64) -> Result<ChatPassClaims, String> {
    let head_b64 = token.split('.').next().ok_or("not a JWT")?;
    let head_raw = B64URL.decode(head_b64).map_err(|_| "header isn't base64url")?;
    let head: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&head_raw).map_err(|_| "header isn't a JSON object")?;
    let mut names: Vec<&str> = head.keys().map(String::as_str).collect();
    names.sort_unstable();
    if names != ["alg", "kid", "typ"] {
        return Err(format!("header fields must be exactly alg, kid, typ (got {names:?})"));
    }
    if head.get("alg").and_then(|v| v.as_str()) != Some("ES256") {
        return Err("alg is not ES256".into());
    }
    if head.get("typ").and_then(|v| v.as_str()) != Some(CHAT_PASS_TYP) {
        return Err(format!("typ is not {CHAT_PASS_TYP}"));
    }
    let kid = head.get("kid").and_then(|v| v.as_str()).ok_or("no kid")?;
    let jwk = jwks
        .get("keys")
        .and_then(|k| k.as_array())
        .ok_or("jwks has no keys array")?
        .iter()
        .find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid))
        .ok_or_else(|| format!("kid {kid} is not in the JWKS"))?;
    let (x, y) = match (jwk.get("x").and_then(|v| v.as_str()), jwk.get("y").and_then(|v| v.as_str())) {
        (Some(x), Some(y)) => (x, y),
        _ => return Err("JWK has no x/y".into()),
    };
    let key = DecodingKey::from_ec_components(x, y).map_err(|e| format!("JWK doesn't load: {e}"))?;
    let mut v = Validation::new(Algorithm::ES256);
    v.set_issuer(&[CHAT_PASS_ISSUER]);
    v.set_audience(&[expected_aud]);
    v.leeway = VERIFY_SKEW_SECS as u64;
    v.validate_exp = false; // checked below against the caller's `now`
    v.set_required_spec_claims(&["iss", "aud", "sub", "exp", "iat"]);
    let data = jsonwebtoken::decode::<serde_json::Map<String, serde_json::Value>>(token, &key, &v)
        .map_err(|e| format!("rejected: {:?}", e.kind()))?;
    let mut claim_names: Vec<&str> = data.claims.keys().map(String::as_str).collect();
    claim_names.sort_unstable();
    if claim_names != ["aud", "exp", "iat", "iss", "jti", "sid", "sub"] {
        return Err(format!("claims must be exactly the seven (got {claim_names:?})"));
    }
    let c: ChatPassClaims =
        serde_json::from_value(serde_json::Value::Object(data.claims)).map_err(|e| format!("claim types: {e}"))?;
    if c.aud != expected_aud {
        return Err("aud mismatch".into());
    }
    let life = c.exp - c.iat;
    if life <= 0 || life > MAX_PASS_LIFETIME_SECS {
        return Err(format!("exp - iat is {life}"));
    }
    if now > c.exp + VERIFY_SKEW_SECS {
        return Err("expired".into());
    }
    if c.iat > now + VERIFY_SKEW_SECS {
        return Err("issued in the future".into());
    }
    Ok(c)
}

#[cfg(test)]
mod tests;
