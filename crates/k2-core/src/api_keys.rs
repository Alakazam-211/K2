//! P3a (sandbox / K2-as-a-server) — the first-class, owner-minted, revocable
//! API-key auth tier for the external `/v1/*` surface.
//!
//! This is the EXTERNAL security boundary. An API key is a high-entropy random
//! credential the owner mints (`k2 api-key create`) and hands to an external
//! caller; the caller presents it (`Authorization: Bearer k2sk_…` or
//! `?token=k2sk_…`) to authenticate to the public sandbox API. A valid,
//! non-revoked key resolves to an [`ApiPrincipal`] — the host-side identity the
//! P3b policy-resolver turns into a locked-down spawn (never trusting caller
//! input). An API key can ONLY authorize `/v1/*`; it can NEVER mint/list/revoke
//! keys (that is owner-token-only at the route layer).
//!
//! ## Storage (table `api_keys`, migration 0058)
//! - `key_hash` = hex `SHA-256(raw key)`. We hash with SHA-256, **not argon2**:
//!   argon2 is deliberately slow to blunt brute force of LOW-entropy human
//!   passwords. An API key is 256+ bits of CSPRNG output — there is no
//!   dictionary or keyspace to grind, so a fast cryptographic hash is the
//!   CORRECT choice. This mirrors how [`crate::connect_users`] stores SHA-256
//!   digests of its (high-entropy) session tokens while reserving argon2 for
//!   passwords. The presented key is hashed and the digest looked up; the raw
//!   key never touches the DB and is unrecoverable from a stolen table.
//! - `anthropic_api_key` (nullable) = the BYO LLM cred associated with THIS key,
//!   staged (B3a-style) into the ephemeral session a `/v1/*` call spawns under
//!   this principal. PLAINTEXT at rest like the B3a per-workspace column (the
//!   box DB is root-only); at-rest encryption is a follow-up. NEVER logged.
//! - `scope` = the principal's authorization scope. Only `'owner'` exists today
//!   (own-use); the column lets per-tenant scopes drop in for P4 without a
//!   migration.
//! - `revoked_at` (nullable) = immediate, durable revocation. Once set,
//!   [`resolve_api_key`] never returns the row again.
//!
//! ## Secret hygiene
//! The raw key is returned to the owner exactly ONCE (from [`create_api_key`]).
//! The raw key and the stored anthropic key are NEVER logged, echoed, or
//! returned by [`list_api_keys`] (which exposes only booleans). The validator
//! and the list helper deliberately keep these values out of every error
//! string.

use sha2::{Digest, Sha256};

/// The prefix every K2 sandbox API key carries. Lets a human (and the auth
/// path) recognize a key at a glance and distinguishes it from the owner token
/// / a connect-user session token.
pub const API_KEY_PREFIX: &str = "k2sk_";

/// Number of base62 characters of randomness after the prefix. 43 base62 chars
/// ≈ 43·log2(62) ≈ 256 bits of entropy (> 32 bytes), matching the strength of
/// the connect-user session token (32 random bytes).
const API_KEY_BODY_LEN: usize = 43;

/// The base62 alphabet (digits, upper, lower) used for the key body.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A resolved API-key principal — the host-side identity a presented key maps
/// to. This is the input the P3b policy-resolver consumes (never any
/// caller-supplied hint).
///
/// `anthropic_key` is the BYO LLM credential to stage into the session this
/// principal spawns (may be absent → no credential staged, logged host-side).
/// `scope` is the authorization scope (`"owner"` today). NEITHER the raw API
/// key nor `anthropic_key` is ever logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiPrincipal {
    /// The `api_keys.id` (a UUID) — a stable, non-secret handle for the
    /// principal (safe to log / surface as the `/v1/ping` identity).
    pub id: String,
    /// The associated Anthropic API key, if one was set at create. Staged into
    /// the spawned session's env (B3a-style). SECRET — never logged.
    pub anthropic_key: Option<String>,
    /// Authorization scope. `"owner"` today; per-tenant scopes are P4.
    pub scope: String,
}

/// Redacted metadata about a stored API key — safe to return over the wire.
/// Carries NEITHER the raw key (unrecoverable — only its hash is stored) NOR
/// the anthropic key (only a boolean saying whether one is set).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ApiKeyMeta {
    pub id: String,
    pub label: Option<String>,
    pub scope: String,
    pub created_at: i64,
    /// `Some(ts)` once revoked; the key no longer authorizes.
    pub revoked_at: Option<i64>,
    /// Whether a (hashed) key is on file. Always true for a real row — present
    /// so the shape is explicit and future-proof.
    pub key_set: bool,
    /// Whether an associated Anthropic key is stored (NEVER the value).
    pub anthropic_key_set: bool,
}

/// Hex `SHA-256` of `s`. Used for both the stored `key_hash` and the lookup of
/// a presented key. (Same digest construction as `connect_users::token_digest`.)
fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let out = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Generate a fresh raw API key: `k2sk_` + [`API_KEY_BODY_LEN`] base62 chars of
/// CSPRNG output. Uses the OS CSPRNG (`OsRng`, the same source
/// [`crate::connect_users`] uses for session tokens) with rejection sampling so
/// the base62 mapping is unbiased (each char uniform over the 62-symbol
/// alphabet — no modulo skew).
fn generate_raw_key() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut body = String::with_capacity(API_KEY_PREFIX.len() + API_KEY_BODY_LEN);
    body.push_str(API_KEY_PREFIX);
    // Reject bytes >= 248 (= 62*4) so the remaining range [0,248) maps onto
    // 62 symbols with no bias.
    const REJECT_AT: u8 = 248;
    let mut produced = 0;
    let mut scratch = [0u8; 64];
    while produced < API_KEY_BODY_LEN {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b >= REJECT_AT {
                continue;
            }
            body.push(BASE62[(b % 62) as usize] as char);
            produced += 1;
            if produced == API_KEY_BODY_LEN {
                break;
            }
        }
    }
    body
}

/// Current unix epoch seconds (creation/revocation stamps).
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Mint a new API key. Generates a CSPRNG `k2sk_…` raw key, stores only its
/// SHA-256 digest plus the optional BYO `anthropic_key`, and returns
/// `(id, raw_key)`. **The raw key is returned exactly ONCE here and is never
/// recoverable afterward** — the caller must surface it to the owner now.
///
/// `label` is a human tag (may be empty). A blank `anthropic_key` is stored as
/// NULL (no credential). NEVER logs the raw key or the anthropic key.
pub fn create_api_key(label: &str, anthropic_key: Option<&str>) -> Result<(String, String), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    let key_hash = sha256_hex(&raw);
    let label_stored: Option<&str> = {
        let t = label.trim();
        if t.is_empty() { None } else { Some(t) }
    };
    let anthropic_stored: Option<&str> = match anthropic_key {
        Some(k) if !k.trim().is_empty() => Some(k.trim()),
        _ => None,
    };

    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO api_keys (id, key_hash, label, anthropic_api_key, scope, created_at, revoked_at) \
         VALUES (?1, ?2, ?3, ?4, 'owner', ?5, NULL)",
        // Deliberately keep the raw key + anthropic key OUT of any error string.
        rusqlite::params![id, key_hash, label_stored, anthropic_stored, now_secs()],
    )
    .map_err(|e| format!("DB insert failed: {e}"))?;
    Ok((id, raw))
}

/// Revoke the key with `id` (sets `revoked_at` if not already set). Returns
/// `Ok(true)` if a non-revoked row was just revoked, `Ok(false)` if no such
/// row existed (unknown id or already revoked) — idempotent. Revocation is
/// immediate: [`resolve_api_key`] rejects a revoked row on the very next call.
pub fn revoke_api_key(id: &str) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = conn
        .execute(
            "UPDATE api_keys SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
            rusqlite::params![now_secs(), id],
        )
        .map_err(|e| format!("DB update failed: {e}"))?;
    Ok(rows > 0)
}

/// List all API keys as redacted metadata (newest first). NEVER includes the
/// raw key (only its hash is stored, and not even that is returned) or the
/// anthropic key (only `anthropic_key_set`).
pub fn list_api_keys() -> Result<Vec<ApiKeyMeta>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, label, scope, created_at, revoked_at, \
                    (anthropic_api_key IS NOT NULL AND TRIM(anthropic_api_key) <> '') \
             FROM api_keys ORDER BY created_at DESC, id DESC",
        )
        .map_err(|e| format!("DB prepare failed: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ApiKeyMeta {
                id: row.get(0)?,
                label: row.get::<_, Option<String>>(1)?,
                scope: row.get(2)?,
                created_at: row.get(3)?,
                revoked_at: row.get::<_, Option<i64>>(4)?,
                // Every persisted row has a key hash on file.
                key_set: true,
                anthropic_key_set: row.get::<_, i64>(5)? != 0,
            })
        })
        .map_err(|e| format!("DB query failed: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("DB row failed: {e}"))?);
    }
    Ok(out)
}

/// Resolve a PRESENTED raw key to an [`ApiPrincipal`], or `None` if it doesn't
/// match a non-revoked stored key. SHA-256-hashes the presented value and looks
/// up the digest (a non-revoked row only).
///
/// **Why a direct hash-equality lookup is safe** (vs the constant-time digest
/// scan `connect_users` uses): an attacker cannot mount a timing side-channel
/// against the index probe to recover a stored hash, because reaching a given
/// `key_hash` requires finding a SHA-256 PREIMAGE of it — SHA-256 is preimage-
/// resistant, so they cannot craft inputs whose digest progressively matches a
/// target. The secret is the raw key, and it is gone the instant it is hashed.
/// An empty/blank presented value never resolves. NEVER logs the key.
pub fn resolve_api_key(presented_raw: &str) -> Option<ApiPrincipal> {
    let presented = presented_raw.trim();
    if presented.is_empty() {
        return None;
    }
    let key_hash = sha256_hex(presented);
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, anthropic_api_key, scope FROM api_keys \
         WHERE key_hash = ?1 AND revoked_at IS NULL",
        rusqlite::params![key_hash],
        |row| {
            let id: String = row.get(0)?;
            let anthropic: Option<String> = row.get(1)?;
            let scope: String = row.get(2)?;
            Ok(ApiPrincipal {
                id,
                // Treat a blank stored value as absent (parity with B3a).
                anthropic_key: anthropic.filter(|k| !k.trim().is_empty()),
                scope,
            })
        },
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A freshly minted key round-trips: it resolves to a principal whose id
    /// matches the create return, and (a) the raw key has the right shape,
    /// (b) the anthropic key flows through to the principal.
    #[test]
    fn create_then_resolve_round_trips() {
        let (id, raw) = create_api_key("ci-roundtrip", Some("sk-ant-roundtrip-1")).expect("create");
        assert!(raw.starts_with(API_KEY_PREFIX), "raw key must carry the k2sk_ prefix");
        assert_eq!(
            raw.len(),
            API_KEY_PREFIX.len() + API_KEY_BODY_LEN,
            "raw key body must be the fixed base62 length",
        );

        let principal = resolve_api_key(&raw).expect("valid key resolves");
        assert_eq!(principal.id, id, "resolved principal id matches create");
        assert_eq!(principal.scope, "owner", "default scope is owner");
        assert_eq!(
            principal.anthropic_key.as_deref(),
            Some("sk-ant-roundtrip-1"),
            "the BYO anthropic key flows through to the principal",
        );
    }

    /// A key minted with no anthropic key resolves to a principal with `None`.
    #[test]
    fn create_without_anthropic_key_resolves_none_cred() {
        let (_id, raw) = create_api_key("ci-no-cred", None).expect("create");
        let principal = resolve_api_key(&raw).expect("resolves");
        assert_eq!(principal.anthropic_key, None);

        // A blank anthropic key is also stored as absent.
        let (_id2, raw2) = create_api_key("ci-blank-cred", Some("   ")).expect("create blank");
        assert_eq!(resolve_api_key(&raw2).expect("resolves").anthropic_key, None);
    }

    /// Revocation is immediate: after revoke, the SAME raw key resolves to None.
    #[test]
    fn revoke_then_resolve_is_none() {
        let (id, raw) = create_api_key("ci-revoke", None).expect("create");
        assert!(resolve_api_key(&raw).is_some(), "valid before revoke");

        assert!(revoke_api_key(&id).expect("revoke"), "first revoke flips the row");
        assert_eq!(resolve_api_key(&raw), None, "revoked key must not resolve");

        // Idempotent: revoking again is a no-op (already revoked).
        assert!(!revoke_api_key(&id).expect("revoke again"), "second revoke is a no-op");
        // Revoking an unknown id is also a clean no-op.
        assert!(!revoke_api_key("no-such-id").expect("revoke unknown"), "unknown id no-op");
    }

    /// A garbage / unknown / empty presented key never resolves.
    #[test]
    fn resolve_bad_key_is_none() {
        assert_eq!(resolve_api_key("k2sk_not-a-real-key"), None);
        assert_eq!(resolve_api_key("totally-bogus"), None);
        assert_eq!(resolve_api_key(""), None);
        assert_eq!(resolve_api_key("   "), None);
    }

    /// `list_api_keys` returns metadata and NEVER leaks the raw key or the
    /// stored anthropic key — only booleans + the redacted fields.
    #[test]
    fn list_never_contains_raw_or_anthropic_key() {
        let secret_anthropic = "sk-ant-list-secret-zzz";
        let (id, raw) = create_api_key("ci-list", Some(secret_anthropic)).expect("create");

        let metas = list_api_keys().expect("list");
        let mine = metas.iter().find(|m| m.id == id).expect("our key is listed");
        assert_eq!(mine.label.as_deref(), Some("ci-list"));
        assert_eq!(mine.scope, "owner");
        assert!(mine.key_set, "key_set reported");
        assert!(mine.anthropic_key_set, "anthropic_key_set reported true");
        assert!(mine.revoked_at.is_none(), "fresh key is not revoked");

        // Serialize the whole list and assert NEITHER secret appears anywhere.
        let json = serde_json::to_string(&metas).expect("serialize metas");
        assert!(
            !json.contains(secret_anthropic),
            "list output must never contain the anthropic key",
        );
        // The raw key body (post-prefix) must never appear.
        let raw_body = raw.strip_prefix(API_KEY_PREFIX).unwrap();
        assert!(
            !json.contains(raw_body),
            "list output must never contain the raw API key",
        );
    }

    /// Two minted keys are distinct (CSPRNG) and resolve to distinct principals.
    #[test]
    fn minted_keys_are_unique() {
        let (id1, raw1) = create_api_key("u1", None).expect("create 1");
        let (id2, raw2) = create_api_key("u2", None).expect("create 2");
        assert_ne!(raw1, raw2, "two CSPRNG keys must differ");
        assert_ne!(id1, id2, "ids differ");
        assert_eq!(resolve_api_key(&raw1).unwrap().id, id1);
        assert_eq!(resolve_api_key(&raw2).unwrap().id, id2);
    }
}
