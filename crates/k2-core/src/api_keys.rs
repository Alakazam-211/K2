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

/// W5 (0.40.30, migration 0071) — the LLM providers whose credentials the
/// api-key store can carry, i.e. the PROVIDER→ENV-VAR mapping table for
/// principal credential staging. A key row's `provider` column (NULL =
/// [`LlmProvider::Anthropic`], the pre-0071 behavior) decides WHICH env
/// var(s) the stored credential is staged under when a `/v1` spawn runs
/// under that principal — so a workspace whose configured agent is
/// codex/gemini/grok receives the credential its CLI actually reads.
///
/// FAIL-CLOSED: an unrecognized `provider` value parses to `None` and the
/// staging seam ([`ApiPrincipal::staged_env_pairs`]) stages NOTHING (with a
/// structured warning naming the provider — never the credential).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    /// `ANTHROPIC_API_KEY` (claude).
    Anthropic,
    /// `OPENAI_API_KEY` (codex); `base_url` rides as `OPENAI_BASE_URL`.
    OpenAi,
    /// `GEMINI_API_KEY` + `GOOGLE_API_KEY` (gemini reads either, staged
    /// under BOTH so every CLI generation finds it).
    Google,
    /// `XAI_API_KEY` (grok).
    Xai,
}

impl LlmProvider {
    /// Parse a stored/requested provider name. Trimmed, case-insensitive;
    /// accepts the canonical name plus the obvious product-name aliases
    /// (`gemini`→Google, `grok`→Xai). Unknown → `None` (fail-closed —
    /// callers stage nothing / reject the create).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            "google" | "gemini" => Some(Self::Google),
            "xai" | "grok" => Some(Self::Xai),
            _ => None,
        }
    }

    /// The canonical lowercase name persisted in `api_keys.provider` (the
    /// create route normalizes aliases through parse→canonical so the DB
    /// only ever carries these four values, or NULL).
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
            Self::Xai => "xai",
        }
    }

    /// The env var name(s) the credential is staged under for this provider.
    pub fn key_env_vars(&self) -> &'static [&'static str] {
        match self {
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
            Self::OpenAi => &["OPENAI_API_KEY"],
            Self::Google => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::Xai => &["XAI_API_KEY"],
        }
    }

    /// The env var a stored `base_url` rides under, if this provider has an
    /// endpoint-override convention (OpenAI-compatible endpoints only today).
    pub fn base_url_env_var(&self) -> Option<&'static str> {
        match self {
            Self::OpenAi => Some("OPENAI_BASE_URL"),
            _ => None,
        }
    }

    /// The accepted `provider` spellings, for create-route error messages.
    pub const ACCEPTED: &'static str = "anthropic, openai, google (alias: gemini), xai (alias: grok)";
}

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
    /// The associated BYO LLM credential, if one was set at create (column
    /// `anthropic_api_key` — the name is historical; since migration 0071
    /// the `provider` field decides which env var(s) it stages under, and
    /// NULL provider = anthropic, the original behavior). Staged into the
    /// spawned session's env via [`Self::staged_env_pairs`]. SECRET — never
    /// logged.
    pub anthropic_key: Option<String>,
    /// W5 (migration 0071) — the credential's provider, verbatim from the
    /// `provider` column. `None` (NULL) = anthropic (pre-0071 behavior);
    /// an unrecognized value stages NOTHING (fail-closed). NOT a secret.
    pub provider: Option<String>,
    /// W5 (migration 0071) — optional provider endpoint override, verbatim
    /// from the `base_url` column. Staged as `OPENAI_BASE_URL` when the
    /// provider is openai; ignored otherwise. NOT a secret.
    pub base_url: Option<String>,
    /// Authorization scope. `"owner"` today; per-tenant scopes are P4.
    pub scope: String,
    /// Sandbox v2 (PRD §G2 #4) — the per-key WORKSPACE GRANT, the raw
    /// `allowed_workspaces` TEXT column verbatim. Interpretation (see
    /// [`ApiPrincipal::authorizes_workspace`]): `None` (NULL) = NO grant
    /// (FAIL-CLOSED — authorizes zero workspaces), `Some("*")` = all
    /// workspaces, `Some(json_array)` = the explicit set of authorized slugs.
    /// NOT a secret (slugs are non-sensitive); safe to keep on the principal.
    pub allowed_workspaces: Option<String>,
}

impl ApiPrincipal {
    /// Sandbox v2 (PRD §G2 #4) — does THIS key's workspace grant authorize the
    /// workspace `slug`? The tenancy check the `/v1/w/<workspace>/...` door
    /// runs (owner-token principals bypass it entirely — owner = all).
    ///
    /// **FAIL-CLOSED at every ambiguity:**
    /// - `None` (NULL column — the value existing pre-migration rows carry) →
    ///   `false`. A key with no grant reaches no workspace.
    /// - `Some("*")` (after trim) → `true` (the wildcard own-use grant).
    /// - `Some(json_array_of_slugs)` → `true` iff `slug` is an EXACT member
    ///   (or the array itself contains `"*"`). Exact match only — no prefix /
    ///   case-folding / path games.
    /// - Any malformed / non-array / empty JSON → `false` (never fail-open on a
    ///   grant we can't parse).
    ///
    /// `slug` is expected already-validated (percent-decoded, no `/`, no `..`,
    /// non-empty) by the caller; this method makes no path assumptions and
    /// only ever does string equality, so it cannot be tricked into a wider
    /// grant by a hostile slug.
    pub fn authorizes_workspace(&self, slug: &str) -> bool {
        match self.allowed_workspaces.as_deref().map(str::trim) {
            None => false,
            Some("") => false,
            Some("*") => true,
            Some(raw) => match serde_json::from_str::<Vec<String>>(raw) {
                Ok(list) => list.iter().any(|w| {
                    let w = w.trim();
                    w == "*" || w == slug
                }),
                Err(_) => false,
            },
        }
    }

    /// W5 (0.40.30) — THE credential-staging seam: the `(ENV_VAR, value)`
    /// pairs this principal's stored LLM credential should be staged under
    /// in a `/v1`-spawned session's env, per the [`LlmProvider`] mapping
    /// table.
    ///
    /// - No / blank credential → empty (never an empty `VAR=` assignment).
    /// - `provider` NULL/blank → **anthropic** (the pre-0071 behavior:
    ///   exactly `ANTHROPIC_API_KEY`, byte-identical staging).
    /// - Recognized provider → that provider's `key_env_vars()`, plus the
    ///   `base_url` pass-through (`OPENAI_BASE_URL`) when the provider has
    ///   one and the row carries a non-blank `base_url`.
    /// - **Unrecognized provider → EMPTY (fail-closed)**, with a structured
    ///   warning naming the principal + provider — NEVER the credential.
    ///
    /// The credential VALUE is never logged here or by callers.
    pub fn staged_env_pairs(&self) -> Vec<(String, String)> {
        let key = match self.anthropic_key.as_deref().map(str::trim) {
            Some(k) if !k.is_empty() => k,
            _ => return Vec::new(),
        };
        let raw_provider = self
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or("anthropic");
        let Some(provider) = LlmProvider::parse(raw_provider) else {
            crate::log_debug!(
                "[api-keys] WARNING event=unknown_llm_provider principal={} provider={:?}: \
                 unrecognized provider on the key row — staging NO credential env \
                 (fail-closed; accepted: {})",
                self.id,
                raw_provider,
                LlmProvider::ACCEPTED,
            );
            return Vec::new();
        };
        let mut pairs: Vec<(String, String)> = provider
            .key_env_vars()
            .iter()
            .map(|var| (var.to_string(), key.to_string()))
            .collect();
        if let Some(base_var) = provider.base_url_env_var() {
            if let Some(url) = self.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                pairs.push((base_var.to_string(), url.to_string()));
            }
        }
        pairs
    }
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
    /// Whether an associated LLM credential is stored (NEVER the value).
    pub anthropic_key_set: bool,
    /// W5 (migration 0071) — the credential's provider (canonical lowercase;
    /// `None` = anthropic default). NOT a secret — surfaced for owner audit.
    pub provider: Option<String>,
    /// W5 (migration 0071) — the optional endpoint override. NOT a secret.
    pub base_url: Option<String>,
    /// Sandbox v2 (PRD §G2 #4) — the raw per-key workspace grant
    /// (`allowed_workspaces`): `None` = no grant (fail-closed), `Some("*")` =
    /// all, `Some(json_array)` = the explicit slug set. NOT a secret (slugs are
    /// non-sensitive), so unlike the anthropic key its VALUE is surfaced here
    /// so the owner can audit which workspaces a key can address.
    pub allowed_workspaces: Option<String>,
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
///
/// `allowed_workspaces` (sandbox v2, PRD §G2 #4) is the raw per-key WORKSPACE
/// GRANT stored verbatim in the `allowed_workspaces` TEXT column: pass `None`
/// (or a blank string) for NO grant (FAIL-CLOSED — the key reaches zero
/// workspaces), `"*"` for all, or a JSON array of slugs (e.g. `["ai","docs"]`)
/// for an explicit set. The caller (`misc_routes::handle_api_key_create`)
/// normalizes the owner's request into this string; a blank value stores NULL
/// so a key minted without a grant is fail-closed by default.
///
/// `provider` (W5, migration 0071) is the LLM credential's provider —
/// callers should pass a [`LlmProvider::canonical_name`] (the route
/// validates + normalizes); blank/absent stores NULL = anthropic (the
/// pre-0071 default). `base_url` is the optional endpoint override
/// (`OPENAI_BASE_URL` pass-through for openai); blank stores NULL.
pub fn create_api_key(
    label: &str,
    anthropic_key: Option<&str>,
    allowed_workspaces: Option<&str>,
    provider: Option<&str>,
    base_url: Option<&str>,
) -> Result<(String, String), String> {
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
    // A blank grant stores NULL → fail-closed (no workspace access). Only a
    // non-blank grant ("*" or a JSON slug array) is persisted.
    let workspaces_stored: Option<&str> = match allowed_workspaces {
        Some(w) if !w.trim().is_empty() => Some(w.trim()),
        _ => None,
    };
    // W5 (0071): blank provider/base_url store NULL. NULL provider =
    // anthropic at staging time (the zero-migration-risk default).
    let provider_stored: Option<&str> = match provider {
        Some(p) if !p.trim().is_empty() => Some(p.trim()),
        _ => None,
    };
    let base_url_stored: Option<&str> = match base_url {
        Some(u) if !u.trim().is_empty() => Some(u.trim()),
        _ => None,
    };

    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO api_keys (id, key_hash, label, anthropic_api_key, scope, created_at, revoked_at, allowed_workspaces, provider, base_url) \
         VALUES (?1, ?2, ?3, ?4, 'owner', ?5, NULL, ?6, ?7, ?8)",
        // Deliberately keep the raw key + LLM credential OUT of any error string.
        rusqlite::params![
            id,
            key_hash,
            label_stored,
            anthropic_stored,
            now_secs(),
            workspaces_stored,
            provider_stored,
            base_url_stored
        ],
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
                    (anthropic_api_key IS NOT NULL AND TRIM(anthropic_api_key) <> ''), \
                    allowed_workspaces, provider, base_url \
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
                // Non-secret (slugs) — surface the raw grant for owner audit.
                allowed_workspaces: row.get::<_, Option<String>>(6)?,
                // W5 (0071) — non-secret provider metadata for owner audit.
                provider: row.get::<_, Option<String>>(7)?,
                base_url: row.get::<_, Option<String>>(8)?,
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
        "SELECT id, anthropic_api_key, scope, allowed_workspaces, provider, base_url FROM api_keys \
         WHERE key_hash = ?1 AND revoked_at IS NULL",
        rusqlite::params![key_hash],
        |row| {
            let id: String = row.get(0)?;
            let anthropic: Option<String> = row.get(1)?;
            let scope: String = row.get(2)?;
            let allowed_workspaces: Option<String> = row.get(3)?;
            let provider: Option<String> = row.get(4)?;
            let base_url: Option<String> = row.get(5)?;
            Ok(ApiPrincipal {
                id,
                // Treat a blank stored value as absent (parity with B3a).
                anthropic_key: anthropic.filter(|k| !k.trim().is_empty()),
                // W5 (0071) — raw provider metadata verbatim; interpreted by
                // `staged_env_pairs` (NULL = anthropic; unknown = fail-closed).
                provider,
                base_url,
                scope,
                // Raw grant verbatim; interpreted by `authorizes_workspace`
                // (NULL = fail-closed no grant).
                allowed_workspaces,
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
        let (id, raw) =
            create_api_key("ci-roundtrip", Some("sk-ant-roundtrip-1"), Some("[\"ai\"]"), None, None).expect("create");
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
        // The per-key workspace grant flows through verbatim and authorizes
        // exactly the granted slug (and nothing else).
        assert_eq!(principal.allowed_workspaces.as_deref(), Some("[\"ai\"]"));
        assert!(principal.authorizes_workspace("ai"), "granted slug authorized");
        assert!(!principal.authorizes_workspace("other"), "non-granted slug denied");
    }

    /// A key minted with no anthropic key resolves to a principal with `None`.
    #[test]
    fn create_without_anthropic_key_resolves_none_cred() {
        let (_id, raw) = create_api_key("ci-no-cred", None, None, None, None).expect("create");
        let principal = resolve_api_key(&raw).expect("resolves");
        assert_eq!(principal.anthropic_key, None);
        // No grant → fail-closed: authorizes NO workspace.
        assert_eq!(principal.allowed_workspaces, None);
        assert!(!principal.authorizes_workspace("ai"), "ungranted key reaches no workspace");

        // A blank anthropic key is also stored as absent.
        let (_id2, raw2) = create_api_key("ci-blank-cred", Some("   "), None, None, None).expect("create blank");
        assert_eq!(resolve_api_key(&raw2).expect("resolves").anthropic_key, None);
    }

    /// Revocation is immediate: after revoke, the SAME raw key resolves to None.
    #[test]
    fn revoke_then_resolve_is_none() {
        let (id, raw) = create_api_key("ci-revoke", None, None, None, None).expect("create");
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
        let (id, raw) = create_api_key("ci-list", Some(secret_anthropic), Some("*"), None, None).expect("create");

        let metas = list_api_keys().expect("list");
        let mine = metas.iter().find(|m| m.id == id).expect("our key is listed");
        assert_eq!(mine.label.as_deref(), Some("ci-list"));
        assert_eq!(mine.scope, "owner");
        assert!(mine.key_set, "key_set reported");
        assert!(mine.anthropic_key_set, "anthropic_key_set reported true");
        assert!(mine.revoked_at.is_none(), "fresh key is not revoked");
        // The (non-secret) workspace grant is surfaced for owner audit.
        assert_eq!(mine.allowed_workspaces.as_deref(), Some("*"));

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

    /// Sandbox v2 (PRD §G2 #4) — the per-key workspace grant is interpreted
    /// FAIL-CLOSED at every branch: no grant / blank / malformed → deny; `"*"`
    /// (or a `"*"` array member) → allow-all; a JSON array → exact-membership.
    /// This is a PURE unit test (no DB) so it pins the authz semantics directly.
    #[test]
    fn authorizes_workspace_is_fail_closed() {
        fn p(grant: Option<&str>) -> ApiPrincipal {
            ApiPrincipal {
                id: "k".to_string(),
                anthropic_key: None,
                provider: None,
                base_url: None,
                scope: "owner".to_string(),
                allowed_workspaces: grant.map(str::to_string),
            }
        }
        // NULL / blank grant → NO workspace (the existing-row backfill case).
        assert!(!p(None).authorizes_workspace("ai"), "NULL grant denies");
        assert!(!p(Some("")).authorizes_workspace("ai"), "blank grant denies");
        assert!(!p(Some("   ")).authorizes_workspace("ai"), "whitespace grant denies");
        // Wildcard → any workspace.
        assert!(p(Some("*")).authorizes_workspace("ai"));
        assert!(p(Some("*")).authorizes_workspace("anything-else"));
        assert!(p(Some(" * ")).authorizes_workspace("ai"), "wildcard trims");
        // Explicit array → exact membership only.
        assert!(p(Some("[\"ai\",\"docs\"]")).authorizes_workspace("ai"));
        assert!(p(Some("[\"ai\",\"docs\"]")).authorizes_workspace("docs"));
        assert!(!p(Some("[\"ai\",\"docs\"]")).authorizes_workspace("secret"));
        // No prefix / substring games — exact match required.
        assert!(!p(Some("[\"ai\"]")).authorizes_workspace("ai-staging"));
        assert!(!p(Some("[\"ai\"]")).authorizes_workspace("a"));
        // A `"*"` member inside the array is still allow-all.
        assert!(p(Some("[\"*\"]")).authorizes_workspace("anything"));
        // Malformed / non-array JSON → fail-closed (never fail-open).
        assert!(!p(Some("not json")).authorizes_workspace("ai"));
        assert!(!p(Some("{\"ai\":true}")).authorizes_workspace("ai"));
        assert!(!p(Some("[]")).authorizes_workspace("ai"), "empty array grants nothing");
    }

    /// W5 — a pure principal builder for the staging-seam unit tests.
    fn principal(
        key: Option<&str>,
        provider: Option<&str>,
        base_url: Option<&str>,
    ) -> ApiPrincipal {
        ApiPrincipal {
            id: "w5-test-key".to_string(),
            anthropic_key: key.map(str::to_string),
            provider: provider.map(str::to_string),
            base_url: base_url.map(str::to_string),
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
        }
    }

    /// W5 — THE provider→env mapping contract, pinned pure (no DB):
    /// NULL/blank provider = anthropic (byte-identical pre-0071 staging);
    /// each known provider stages its own var(s); unknown stages NOTHING.
    #[test]
    fn staged_env_pairs_maps_provider_to_env_vars() {
        // NULL provider → anthropic (the zero-migration-risk default).
        assert_eq!(
            principal(Some("sk-test-1"), None, None).staged_env_pairs(),
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-test-1".to_string())],
        );
        // Blank provider → also anthropic.
        assert_eq!(
            principal(Some("sk-test-1"), Some("   "), None).staged_env_pairs(),
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-test-1".to_string())],
        );
        // Explicit anthropic (case/space-insensitive).
        assert_eq!(
            principal(Some("sk-test-1"), Some(" Anthropic "), None).staged_env_pairs(),
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-test-1".to_string())],
        );
        // openai → OPENAI_API_KEY only (no base URL on the row).
        assert_eq!(
            principal(Some("sk-oai"), Some("openai"), None).staged_env_pairs(),
            vec![("OPENAI_API_KEY".to_string(), "sk-oai".to_string())],
        );
        // google → BOTH GEMINI_API_KEY and GOOGLE_API_KEY.
        assert_eq!(
            principal(Some("gk"), Some("google"), None).staged_env_pairs(),
            vec![
                ("GEMINI_API_KEY".to_string(), "gk".to_string()),
                ("GOOGLE_API_KEY".to_string(), "gk".to_string()),
            ],
        );
        // gemini alias → same as google.
        assert_eq!(
            principal(Some("gk"), Some("gemini"), None).staged_env_pairs(),
            principal(Some("gk"), Some("google"), None).staged_env_pairs(),
        );
        // xai (+ grok alias) → XAI_API_KEY.
        assert_eq!(
            principal(Some("xk"), Some("xai"), None).staged_env_pairs(),
            vec![("XAI_API_KEY".to_string(), "xk".to_string())],
        );
        assert_eq!(
            principal(Some("xk"), Some("grok"), None).staged_env_pairs(),
            principal(Some("xk"), Some("xai"), None).staged_env_pairs(),
        );
    }

    /// W5 — FAIL-CLOSED arms: unknown provider stages NOTHING; a missing/
    /// blank credential stages nothing regardless of provider.
    #[test]
    fn staged_env_pairs_fails_closed() {
        // Unknown provider → NOTHING (never a guessed env var).
        assert!(
            principal(Some("sk-x"), Some("mystery-llm"), None).staged_env_pairs().is_empty(),
            "unknown provider must stage nothing",
        );
        // Unknown provider with a base_url → STILL nothing (no orphan URL var).
        assert!(
            principal(Some("sk-x"), Some("mystery-llm"), Some("https://x.test"))
                .staged_env_pairs()
                .is_empty(),
        );
        // No credential → nothing, whatever the provider says.
        assert!(principal(None, Some("openai"), None).staged_env_pairs().is_empty());
        assert!(principal(Some("   "), Some("openai"), None).staged_env_pairs().is_empty());
        assert!(principal(None, None, None).staged_env_pairs().is_empty());
    }

    /// W5 — OPENAI_BASE_URL pass-through: staged for openai when the row
    /// carries a non-blank base_url; ignored for providers with no override
    /// convention.
    #[test]
    fn staged_env_pairs_base_url_passthrough() {
        assert_eq!(
            principal(Some("sk-oai"), Some("openai"), Some("https://proxy.example/v1"))
                .staged_env_pairs(),
            vec![
                ("OPENAI_API_KEY".to_string(), "sk-oai".to_string()),
                ("OPENAI_BASE_URL".to_string(), "https://proxy.example/v1".to_string()),
            ],
        );
        // Blank base_url → no OPENAI_BASE_URL entry.
        assert_eq!(
            principal(Some("sk-oai"), Some("openai"), Some("   ")).staged_env_pairs(),
            vec![("OPENAI_API_KEY".to_string(), "sk-oai".to_string())],
        );
        // Non-openai provider ignores a stored base_url (no convention).
        assert_eq!(
            principal(Some("sk-a"), None, Some("https://ignored.example")).staged_env_pairs(),
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-a".to_string())],
        );
        assert_eq!(
            principal(Some("xk"), Some("xai"), Some("https://ignored.example"))
                .staged_env_pairs(),
            vec![("XAI_API_KEY".to_string(), "xk".to_string())],
        );
    }

    /// W5 — LlmProvider::parse canonicalization table.
    #[test]
    fn llm_provider_parse_and_canonical_names() {
        for (raw, want) in [
            ("anthropic", LlmProvider::Anthropic),
            ("ANTHROPIC", LlmProvider::Anthropic),
            ("openai", LlmProvider::OpenAi),
            (" OpenAI ", LlmProvider::OpenAi),
            ("google", LlmProvider::Google),
            ("gemini", LlmProvider::Google),
            ("xai", LlmProvider::Xai),
            ("grok", LlmProvider::Xai),
        ] {
            assert_eq!(LlmProvider::parse(raw), Some(want), "parse {raw:?}");
        }
        for bad in ["", "  ", "azure", "claude", "gpt", "llama"] {
            assert_eq!(LlmProvider::parse(bad), None, "must reject {bad:?}");
        }
        assert_eq!(LlmProvider::Anthropic.canonical_name(), "anthropic");
        assert_eq!(LlmProvider::OpenAi.canonical_name(), "openai");
        assert_eq!(LlmProvider::Google.canonical_name(), "google");
        assert_eq!(LlmProvider::Xai.canonical_name(), "xai");
    }

    /// W5 — provider + base_url round-trip create → resolve → list; the
    /// credential still never leaks from list.
    #[test]
    fn provider_metadata_round_trips() {
        let secret = "sk-oai-w5-secret-zz";
        let (id, raw) = create_api_key(
            "w5-openai-key",
            Some(secret),
            Some("*"),
            Some("openai"),
            Some("https://oai-proxy.example/v1"),
        )
        .expect("create");

        let p = resolve_api_key(&raw).expect("resolves");
        assert_eq!(p.provider.as_deref(), Some("openai"));
        assert_eq!(p.base_url.as_deref(), Some("https://oai-proxy.example/v1"));
        assert_eq!(
            p.staged_env_pairs(),
            vec![
                ("OPENAI_API_KEY".to_string(), secret.to_string()),
                ("OPENAI_BASE_URL".to_string(), "https://oai-proxy.example/v1".to_string()),
            ],
        );

        let metas = list_api_keys().expect("list");
        let mine = metas.iter().find(|m| m.id == id).expect("listed");
        assert_eq!(mine.provider.as_deref(), Some("openai"));
        assert_eq!(mine.base_url.as_deref(), Some("https://oai-proxy.example/v1"));
        let json = serde_json::to_string(&metas).expect("serialize");
        assert!(!json.contains(secret), "list must never contain the credential");

        // Blank provider/base_url store NULL (the anthropic-default row).
        let (id2, raw2) =
            create_api_key("w5-null-provider", Some("sk-a2"), None, Some("  "), Some(""))
                .expect("create 2");
        let p2 = resolve_api_key(&raw2).expect("resolves 2");
        assert_eq!(p2.provider, None);
        assert_eq!(p2.base_url, None);
        assert_eq!(
            p2.staged_env_pairs(),
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-a2".to_string())],
            "NULL provider stages exactly the pre-0071 Anthropic pair",
        );
        let metas = list_api_keys().expect("list 2");
        let mine2 = metas.iter().find(|m| m.id == id2).expect("listed 2");
        assert_eq!(mine2.provider, None);
        assert_eq!(mine2.base_url, None);
    }

    /// Two minted keys are distinct (CSPRNG) and resolve to distinct principals.
    #[test]
    fn minted_keys_are_unique() {
        let (id1, raw1) = create_api_key("u1", None, None, None, None).expect("create 1");
        let (id2, raw2) = create_api_key("u2", None, None, None, None).expect("create 2");
        assert_ne!(raw1, raw2, "two CSPRNG keys must differ");
        assert_ne!(id1, id2, "ids differ");
        assert_eq!(resolve_api_key(&raw1).unwrap().id, id1);
        assert_eq!(resolve_api_key(&raw2).unwrap().id, id2);
    }
}
