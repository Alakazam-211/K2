//! Remote Session Layer 0 + Stage 2 grants.
//!
//! Fail-closed hard wall independent of grants:
//! - `remote_sessions_enabled` (app_settings, default OFF) is the Layer 0 gate
//! - When OFF, drive attempts are denied with `REMOTE_SESSIONS_DISABLED`
//! - Every denial is persisted in `remote_session_events` for owner visibility
//!
//! Stage 2 mints time-bounded shell grants:
//! - Owner/admin POSTs grant → mint `k2rs_<random>` once; store SHA-256 hex
//! - Drive path: Layer 0 first, then `validate_grant_token`
//! - Revoke sets `revoked_at`; list never returns token or credential_hash

use rusqlite::params;
use sha2::{Digest, Sha256};

/// Wire / DB code for "master switch is OFF".
pub const CODE_REMOTE_SESSIONS_DISABLED: &str = "REMOTE_SESSIONS_DISABLED";
/// Wire / DB code for "switch is ON but no grant covers this principal".
pub const CODE_NO_GRANT: &str = "NO_GRANT";
/// Wire / DB code for "grant credential found but past expires_at".
pub const CODE_GRANT_EXPIRED: &str = "GRANT_EXPIRED";
/// Wire / DB code for "grant credential found but revoked_at is set".
pub const CODE_GRANT_REVOKED: &str = "GRANT_REVOKED";

/// Event kind written for access denials.
pub const KIND_DENIAL: &str = "denial";
/// Event kind when a grant is minted.
pub const KIND_GRANT_CREATED: &str = "grant-created";
/// Event kind when a grant is revoked.
pub const KIND_REVOKE: &str = "revoke";
/// Event kind when a remote shell PTY is spawned under a grant.
pub const KIND_SPAWN: &str = "spawn";

/// Principal kind for token-bearing shell grants (Stage 2 v1).
pub const PRINCIPAL_KIND_GRANT_TOKEN: &str = "grant_token";
/// Only scope implemented in Stage 2.
pub const SCOPE_SHELL: &str = "shell";

/// Prefix every remote-session grant token carries.
pub const GRANT_TOKEN_PREFIX: &str = "k2rs_";
/// Prefix every grant row id carries.
pub const GRANT_ID_PREFIX: &str = "rs_";

/// Default TTL when create body omits `ttlSeconds` (30 minutes).
pub const DEFAULT_TTL_SECS: i64 = 1800;
/// Minimum TTL clamp.
pub const MIN_TTL_SECS: i64 = 60;
/// Maximum TTL clamp (24 hours).
pub const MAX_TTL_SECS: i64 = 86400;

/// Number of base62 characters of randomness after `k2rs_`.
/// 43 base62 chars ≈ 256 bits (same strength as api_keys / session tokens).
const GRANT_TOKEN_BODY_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// One `remote_session_events` row (camelCase wire shape for status).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionEvent {
    pub id: String,
    pub grant_id: Option<String>,
    pub principal_label: String,
    pub kind: String,
    pub code: Option<String>,
    pub payload: Option<String>,
    pub created_at: i64,
}

/// Owner-facing grant view — never includes token or credential_hash.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GrantPublic {
    pub id: String,
    pub scope: String,
    pub expires_at: i64,
    pub principal_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub issued_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
}

/// Failure modes for [`validate_grant_token`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    /// Token missing, wrong prefix, empty, or hash not in DB.
    NotFound,
    /// Row found but `expires_at <= now`.
    Expired,
    /// Row found but `revoked_at` is set.
    Revoked,
    /// Token does not look like a grant credential (wrong prefix / format).
    BadFormat,
}

impl GrantError {
    /// Stable wire code for denial responses.
    pub fn code(&self) -> &'static str {
        match self {
            GrantError::NotFound | GrantError::BadFormat => CODE_NO_GRANT,
            GrantError::Expired => CODE_GRANT_EXPIRED,
            GrantError::Revoked => CODE_GRANT_REVOKED,
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            GrantError::NotFound | GrantError::BadFormat => {
                "Remote Sessions are ON but no grant covers this principal. Ask the owner to mint a grant."
            }
            GrantError::Expired => {
                "This remote-session grant has expired. Ask the owner to mint a new grant."
            }
            GrantError::Revoked => {
                "This remote-session grant has been revoked. Ask the owner to mint a new grant."
            }
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Hex SHA-256 of `s` (same construction as `api_keys::sha256_hex`).
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

/// CSPRNG `k2rs_` + base62 body (unbiased rejection sampling, mirrors api_keys).
fn generate_raw_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut body = String::with_capacity(GRANT_TOKEN_PREFIX.len() + GRANT_TOKEN_BODY_LEN);
    body.push_str(GRANT_TOKEN_PREFIX);
    const REJECT_AT: u8 = 248; // 62 * 4
    let mut produced = 0;
    let mut scratch = [0u8; 64];
    while produced < GRANT_TOKEN_BODY_LEN {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b >= REJECT_AT {
                continue;
            }
            body.push(BASE62[(b % 62) as usize] as char);
            produced += 1;
            if produced == GRANT_TOKEN_BODY_LEN {
                break;
            }
        }
    }
    body
}

/// Clamp TTL into the allowed window.
pub fn clamp_ttl_secs(ttl: i64) -> i64 {
    ttl.clamp(MIN_TTL_SECS, MAX_TTL_SECS)
}

/// True when `token` looks like a remote-session grant credential.
pub fn is_grant_token(token: &str) -> bool {
    token.starts_with(GRANT_TOKEN_PREFIX) && token.len() > GRANT_TOKEN_PREFIX.len()
}

/// Layer 0 gate: true only when the owner opted this host into remote sessions.
pub fn is_enabled() -> bool {
    crate::app_settings::load().remote_sessions_enabled
}

/// Persist the Layer 0 master switch via app_settings deep-merge.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    crate::app_settings::update(serde_json::json!({
        "remoteSessionsEnabled": enabled
    }))
    .map(|_| ())
    .map_err(|e| format!("remote_sessions set_enabled: {e}"))
}

/// Insert an audit event (denial, grant-use, etc.).
pub fn record_event(
    grant_id: Option<&str>,
    principal_label: &str,
    kind: &str,
    code: Option<&str>,
    payload: Option<&str>,
) -> Result<RemoteSessionEvent, String> {
    let label = principal_label.trim();
    if label.is_empty() {
        return Err("principal_label must not be empty".to_string());
    }
    let kind = kind.trim();
    if kind.is_empty() {
        return Err("kind must not be empty".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO remote_session_events \
         (id, grant_id, principal_label, kind, code, payload, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            grant_id,
            label,
            kind,
            code,
            payload,
            created_at,
        ],
    )
    .map_err(|e| format!("remote_session_events insert failed: {e}"))?;
    Ok(RemoteSessionEvent {
        id,
        grant_id: grant_id.map(|s| s.to_string()),
        principal_label: label.to_string(),
        kind: kind.to_string(),
        code: code.map(|s| s.to_string()),
        payload: payload.map(|s| s.to_string()),
        created_at,
    })
}

/// Convenience: record a denial with kind=`denial`.
pub fn record_denial(
    principal_label: &str,
    code: &str,
    payload: Option<&str>,
) -> Result<RemoteSessionEvent, String> {
    record_event(None, principal_label, KIND_DENIAL, Some(code), payload)
}

/// Recent denial events (newest first), capped at `limit` (clamped 1..=200).
pub fn list_recent_denials(limit: usize) -> Result<Vec<RemoteSessionEvent>, String> {
    let limit = limit.clamp(1, 200) as i64;
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, grant_id, principal_label, kind, code, payload, created_at \
             FROM remote_session_events \
             WHERE kind = ?1 \
             ORDER BY created_at DESC \
             LIMIT ?2",
        )
        .map_err(|e| format!("remote_session_events prepare: {e}"))?;
    let rows = stmt
        .query_map(params![KIND_DENIAL, limit], |row| {
            Ok(RemoteSessionEvent {
                id: row.get(0)?,
                grant_id: row.get(1)?,
                principal_label: row.get(2)?,
                kind: row.get(3)?,
                code: row.get(4)?,
                payload: row.get(5)?,
                created_at: row.get(6)?,
            })
        })
        .map_err(|e| format!("remote_session_events query: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("remote_session_events row: {e}"))?);
    }
    Ok(out)
}

/// Active (non-revoked, non-expired) grant count.
pub fn active_grant_count() -> Result<usize, String> {
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_session_grants \
             WHERE revoked_at IS NULL AND expires_at > ?1",
            params![now],
            |row| row.get(0),
        )
        .map_err(|e| format!("remote_session_grants count: {e}"))?;
    Ok(n as usize)
}

/// Mint a shell grant. Returns `(public view, plaintext token)` — the token
/// is returned exactly ONCE and is never recoverable from the DB.
///
/// Minting is allowed while Layer 0 is OFF (admin surface); using the token
/// is blocked until enable.
///
/// `scope` must be `"shell"` (runbook rejected by the route layer).
/// `ttl_secs` is clamped to `MIN_TTL_SECS..=MAX_TTL_SECS`.
/// `label` is optional owner-facing audit text (not auth).
/// `issued_by` is optional actor identity for audit.
pub fn create_grant(
    scope: &str,
    ttl_secs: i64,
    label: Option<&str>,
    issued_by: Option<&str>,
) -> Result<(GrantPublic, String), String> {
    let scope = scope.trim();
    if scope != SCOPE_SHELL {
        return Err(format!(
            "unsupported scope {scope:?}; only '{SCOPE_SHELL}' is implemented"
        ));
    }
    let ttl = clamp_ttl_secs(ttl_secs);
    let issued_at = now_secs();
    let expires_at = issued_at.saturating_add(ttl);
    let id = format!("{GRANT_ID_PREFIX}{}", uuid::Uuid::new_v4());
    let raw = generate_raw_token();
    let credential_hash = sha256_hex(&raw);
    let label_stored: Option<&str> = label
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let issued_by_stored: Option<&str> = issued_by
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // principal_ref is NOT NULL; for grant_token the credential_hash is auth.
    // Store the grant id as a stable non-secret reference.
    let principal_ref = id.as_str();

    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO remote_session_grants \
         (id, principal_kind, principal_ref, credential_hash, scope, \
          issued_at, expires_at, issued_by, revoked_at, label) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
        params![
            id,
            PRINCIPAL_KIND_GRANT_TOKEN,
            principal_ref,
            credential_hash,
            SCOPE_SHELL,
            issued_at,
            expires_at,
            issued_by_stored,
            label_stored,
        ],
    )
    .map_err(|e| format!("remote_session_grants insert failed: {e}"))?;
    drop(conn);

    // Audit: never include the raw token or hash in the payload.
    let payload = serde_json::json!({
        "scope": SCOPE_SHELL,
        "ttlSeconds": ttl,
        "expiresAt": expires_at,
    })
    .to_string();
    let principal_label = issued_by_stored.unwrap_or("owner");
    if let Err(e) = record_event(
        Some(&id),
        principal_label,
        KIND_GRANT_CREATED,
        None,
        Some(&payload),
    ) {
        crate::log_debug!("[remote_session] grant-created event failed: {e}");
    }

    let public = GrantPublic {
        id,
        scope: SCOPE_SHELL.to_string(),
        expires_at,
        principal_kind: PRINCIPAL_KIND_GRANT_TOKEN.to_string(),
        label: label_stored.map(|s| s.to_string()),
        issued_at,
        issued_by: issued_by_stored.map(|s| s.to_string()),
        revoked_at: None,
    };
    Ok((public, raw))
}

/// List grants (active + recently revoked / all non-deleted). Never returns
/// credential_hash or token. Newest issued first.
pub fn list_grants() -> Result<Vec<GrantPublic>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, scope, expires_at, principal_kind, label, \
                    issued_at, issued_by, revoked_at \
             FROM remote_session_grants \
             ORDER BY issued_at DESC \
             LIMIT 200",
        )
        .map_err(|e| format!("remote_session_grants list prepare: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(GrantPublic {
                id: row.get(0)?,
                scope: row.get(1)?,
                expires_at: row.get(2)?,
                principal_kind: row.get(3)?,
                label: row.get(4)?,
                issued_at: row.get(5)?,
                issued_by: row.get(6)?,
                revoked_at: row.get(7)?,
            })
        })
        .map_err(|e| format!("remote_session_grants list query: {e}"))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("remote_session_grants list row: {e}"))?);
    }
    Ok(out)
}

/// Revoke a grant by id. Idempotent: already-revoked returns Ok with the row.
/// Missing id → Err.
pub fn revoke_grant(id: &str) -> Result<GrantPublic, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("grant id must not be empty".to_string());
    }
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();

    // Fetch first so we can return the public view (and distinguish missing).
    let existing: Option<(String, String, i64, String, Option<String>, i64, Option<String>, Option<i64>)> =
        conn
            .query_row(
                "SELECT id, scope, expires_at, principal_kind, label, \
                        issued_at, issued_by, revoked_at \
                 FROM remote_session_grants WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("remote_session_grants revoke select: {e}"))?;

    let Some((
        gid,
        scope,
        expires_at,
        principal_kind,
        label,
        issued_at,
        issued_by,
        revoked_at,
    )) = existing
    else {
        return Err(format!("grant not found: {id}"));
    };

    let revoked_at = if revoked_at.is_some() {
        revoked_at
    } else {
        conn.execute(
            "UPDATE remote_session_grants SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
            params![now, id],
        )
        .map_err(|e| format!("remote_session_grants revoke update: {e}"))?;
        Some(now)
    };
    drop(conn);

    if let Err(e) = record_event(
        Some(&gid),
        issued_by.as_deref().unwrap_or("owner"),
        KIND_REVOKE,
        None,
        None,
    ) {
        crate::log_debug!("[remote_session] revoke event failed: {e}");
    }

    Ok(GrantPublic {
        id: gid,
        scope,
        expires_at,
        principal_kind,
        label,
        issued_at,
        issued_by,
        revoked_at,
    })
}

/// Validate a presented grant credential (hash-at-rest lookup).
///
/// Check order: format → not found → revoked → expired → ok.
pub fn validate_grant_token(token: &str) -> Result<GrantPublic, GrantError> {
    let token = token.trim();
    if token.is_empty() || !token.starts_with(GRANT_TOKEN_PREFIX) {
        return Err(GrantError::BadFormat);
    }
    if token.len() <= GRANT_TOKEN_PREFIX.len() {
        return Err(GrantError::BadFormat);
    }
    let hash = sha256_hex(token);
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let row: Option<(String, String, i64, String, Option<String>, i64, Option<String>, Option<i64>)> =
        conn
            .query_row(
                "SELECT id, scope, expires_at, principal_kind, label, \
                        issued_at, issued_by, revoked_at \
                 FROM remote_session_grants WHERE credential_hash = ?1",
                params![hash],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| GrantError::NotFound)?;

    let Some((
        id,
        scope,
        expires_at,
        principal_kind,
        label,
        issued_at,
        issued_by,
        revoked_at,
    )) = row
    else {
        return Err(GrantError::NotFound);
    };

    if revoked_at.is_some() {
        return Err(GrantError::Revoked);
    }
    if expires_at <= now {
        return Err(GrantError::Expired);
    }

    Ok(GrantPublic {
        id,
        scope,
        expires_at,
        principal_kind,
        label,
        issued_at,
        issued_by,
        revoked_at: None,
    })
}

/// Test/helper: force a grant's `expires_at` into the past so expiry paths
/// can be exercised without sleeping. Returns Err if the id is missing.
pub fn force_expire_grant(id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("grant id must not be empty".to_string());
    }
    let past = now_secs().saturating_sub(10);
    let db = crate::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE remote_session_grants SET expires_at = ?1 WHERE id = ?2",
            params![past, id],
        )
        .map_err(|e| format!("force_expire_grant: {e}"))?;
    if n == 0 {
        return Err(format!("grant not found: {id}"));
    }
    Ok(())
}

/// rusqlite Optional query helper (avoids requiring Optional trait import
/// at every call site via a local extension — we re-export the pattern).
trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shared in-memory DB (db::shared under cfg(test)). Keys/ids are
    // unique per mint so tests coexist without isolation helpers.

    #[test]
    fn clamp_ttl_bounds() {
        assert_eq!(clamp_ttl_secs(1), MIN_TTL_SECS);
        assert_eq!(clamp_ttl_secs(DEFAULT_TTL_SECS), DEFAULT_TTL_SECS);
        assert_eq!(clamp_ttl_secs(MAX_TTL_SECS + 100), MAX_TTL_SECS);
        assert_eq!(clamp_ttl_secs(MIN_TTL_SECS), MIN_TTL_SECS);
    }

    #[test]
    fn is_grant_token_prefix() {
        assert!(is_grant_token("k2rs_abc"));
        assert!(!is_grant_token("k2rs_"));
        assert!(!is_grant_token("k2sk_abc"));
        assert!(!is_grant_token(""));
        assert!(!is_grant_token("owner-token"));
    }

    #[test]
    fn create_validate_revoke_lifecycle() {
        let (g, raw) = create_grant(SCOPE_SHELL, 1800, Some("lab"), Some("owner"))
            .expect("mint");
        assert!(g.id.starts_with(GRANT_ID_PREFIX));
        assert!(raw.starts_with(GRANT_TOKEN_PREFIX));
        assert_eq!(raw.len(), GRANT_TOKEN_PREFIX.len() + GRANT_TOKEN_BODY_LEN);
        assert_eq!(g.scope, SCOPE_SHELL);
        assert_eq!(g.principal_kind, PRINCIPAL_KIND_GRANT_TOKEN);
        assert_eq!(g.label.as_deref(), Some("lab"));
        assert!(g.revoked_at.is_none());
        assert!(g.expires_at > g.issued_at);

        // Hash-at-rest: raw never stored.
        let db = crate::db::shared();
        let conn = db.lock();
        let stored: String = conn
            .query_row(
                "SELECT credential_hash FROM remote_session_grants WHERE id = ?1",
                params![g.id],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(stored, sha256_hex(&raw));
        assert!(!stored.contains(&raw[GRANT_TOKEN_PREFIX.len()..]));

        let ok = validate_grant_token(&raw).expect("valid");
        assert_eq!(ok.id, g.id);

        let list = list_grants().unwrap();
        let mine = list.iter().find(|x| x.id == g.id).expect("listed");
        let listed = serde_json::to_string(mine).unwrap();
        assert!(!listed.contains(&raw));
        assert!(!listed.contains("credential"));
        assert!(!listed.contains(&stored));

        let rev = revoke_grant(&g.id).unwrap();
        assert!(rev.revoked_at.is_some());
        assert_eq!(validate_grant_token(&raw), Err(GrantError::Revoked));

        // Idempotent revoke.
        let rev2 = revoke_grant(&g.id).unwrap();
        assert!(rev2.revoked_at.is_some());
    }

    #[test]
    fn validate_expired() {
        let (g, raw) = create_grant(SCOPE_SHELL, 60, None, None).unwrap();
        force_expire_grant(&g.id).unwrap();
        assert_eq!(validate_grant_token(&raw), Err(GrantError::Expired));
        // Expired does not count as active.
        let n = active_grant_count().unwrap();
        // Other concurrent unit tests may hold active grants; only assert
        // THIS grant is not among them via re-validate.
        let _ = n;
    }

    #[test]
    fn validate_bad_format_and_not_found() {
        assert_eq!(validate_grant_token(""), Err(GrantError::BadFormat));
        assert_eq!(validate_grant_token("nope"), Err(GrantError::BadFormat));
        assert_eq!(
            validate_grant_token("k2rs_doesnotexist00000000000000000000000000"),
            Err(GrantError::NotFound)
        );
    }

    #[test]
    fn create_rejects_non_shell_scope() {
        let err = create_grant("runbook", 1800, None, None).unwrap_err();
        assert!(err.contains("runbook") || err.contains("scope"), "err={err}");
    }

    #[test]
    fn grant_error_codes() {
        assert_eq!(GrantError::Expired.code(), CODE_GRANT_EXPIRED);
        assert_eq!(GrantError::Revoked.code(), CODE_GRANT_REVOKED);
        assert_eq!(GrantError::NotFound.code(), CODE_NO_GRANT);
        assert_eq!(GrantError::BadFormat.code(), CODE_NO_GRANT);
    }
}
