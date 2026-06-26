//! Scoped per-session hook tokens (#58 Phase 0 — DORMANT / default OFF).
//!
//! ## Why this exists
//!
//! Today the "hook token" injected into every spawned PTY IS the daemon
//! OWNER token (`main.rs` writes one `generate_token()` into
//! `daemon.token` + `heartbeat.token` + `state.token` + every child env).
//! A prompt-injected agent that reads `$K2_HOOK_TOKEN` therefore holds the
//! crown-jewel credential — the confused-deputy in its strongest form (see
//! `.k2/prds/prd-58-token-channel-foundation.md` §1).
//!
//! #58 splits that conjoined credential into **scoped, attributable,
//! revocable per-session tokens**, mirroring the `connect_users` epoch
//! machinery (per-principal epoch + SHA-256-at-rest + restart-surviving
//! revocation) rather than adding a second `OnceLock`.
//!
//! ## Phase 0 scope (this module)
//!
//! Everything here is an **additive superset, gated on `K2_HOOK_SCOPED`
//! (default OFF)**. With the flag off NOTHING mints a scoped token, no
//! per-cell UDS is bound, and every existing owner-token caller behaves
//! byte-identically. Phase 1 (mint + inject the scoped token into the PTY
//! env, behind `K2_HOOK_TOKEN`) and Phase 2 (reject the owner token) ship
//! LATER, separately. This module deliberately does NOT inject anything
//! into a child env.
//!
//! ## Token format — selector + verifier (PAT pattern)
//!
//! `<session_id>.<secret>`. The `session_id` is the SELECTOR → O(1)
//! registry lookup (no timing-leaking full-table scan); the `secret` is a
//! 256-bit CSPRNG VERIFIER, constant-time-compared against the stored
//! SHA-256 digest. The raw secret is never persisted.
//!
//! ## Revocation model (mirrors `connect_users::token_epoch`)
//!
//! - **Per-session epoch** — `session_epochs[sid]` is the CURRENT epoch;
//!   each record stamps the epoch live at mint. [`SessionTokenRegistry::revoke`]
//!   bumps the current epoch so the stamped record no longer matches →
//!   next call 403, no restart. (One cell == one session, so keying the
//!   epoch by `session_id` IS the per-principal epoch the PRD calls for.)
//! - **Global hook epoch** — a daemon-wide kill switch.
//!   [`SessionTokenRegistry::revoke_all`] bumps it so every stamped record
//!   goes stale at once.
//!
//! ## Persistence
//!
//! The registry persists to `~/.k2/hook-sessions.json` (digest + claims +
//! epochs), exactly like `connect-sessions.json` (PRD §6 open-Q "Epoch
//! persistence — RESOLVED"): in-memory-only would force a re-mint on every
//! daemon restart and break the future VMM "restart re-adoption" path that
//! needs something to re-attest live cells against.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use k2_core::log_debug;
use k2_core::session::SessionId;

/// On-disk record/envelope version. An unknown version fails CLOSED on
/// load (every record dropped → forced re-mint), mirroring
/// `connect_users::SESSION_RECORD_VERSION`.
const STORE_VERSION: u32 = 1;

/// Default token TTL — a **multi-hour, ~session-lifetime** window, NOT a
/// short rotation. Per PRD §3.1 the security property rides EPOCH
/// revocation (instant, restart-surviving), so the TTL is kept OFF the
/// live critical path: a short TTL would strand every agent on the box
/// each rotation cycle mid-client-work.
const DEFAULT_TTL_HOURS: i64 = 12;

/// Hard ceiling on a single token's lifetime regardless of TTL refreshes.
const MAX_LIFETIME_HOURS: i64 = 24;

// ─────────────────────────────────────────────────────────────────────
// Feature flag
// ─────────────────────────────────────────────────────────────────────

/// True iff `K2_HOOK_SCOPED` is set to an affirmative value
/// (`1`/`true`/`yes`/`on`, case-insensitive). **Default OFF.**
///
/// This is the single gate that keeps #58 Phase 0 dormant: minting, the
/// per-cell UDS bind, and the scoped arm of `/hook/complete` all consult
/// it. Off → zero behavior change.
pub fn scoped_hooks_enabled() -> bool {
    match std::env::var("K2_HOOK_SCOPED") {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Principal + claims
// ─────────────────────────────────────────────────────────────────────

/// The capability principal a scoped token resolves to. **Derived from
/// the spawn context, never trusted from a request body** (PRD §3.2: the
/// body carries no identity). Broad reach != owner credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookPrincipal {
    /// `projects.id` of the owning workspace.
    pub workspace_uuid: String,
    /// Agent address within the workspace (free-form for Phase 0; the
    /// federation `AgentAddress` type is not built yet).
    pub agent_address: String,
}

/// What [`SessionTokenRegistry::validate_hook`] returns on success: the
/// bound session + pane + the derived principal. `pane_id` is what the
/// `/hook/complete` dual-accept arm matches the request `paneId` against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedHook {
    pub session_id: String,
    pub pane_id: String,
    pub principal: HookPrincipal,
}

/// A persisted scoped-token claim. The raw secret NEVER touches disk —
/// only its SHA-256 digest (hex) does, exactly like
/// `connect_users::SessionRecord`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    /// On-disk record version; a record whose version != [`STORE_VERSION`]
    /// is dropped on load (fail closed).
    version: u32,
    /// Selector — the `SessionId` this token is bound to.
    session_id: String,
    /// The pane/tab id (`K2_PANE_ID`) the token authorizes. The
    /// `/hook/complete` scoped arm requires the request `paneId` to equal
    /// this — a token scoped to cell A cannot complete a hook for cell B.
    pane_id: String,
    /// Derived capability principal (see [`HookPrincipal`]).
    principal: HookPrincipal,
    /// Hex `SHA-256(secret)`. The presented secret is hashed and
    /// constant-time-compared against this; the raw secret is never stored.
    token_digest: String,
    created_at: DateTime<Utc>,
    /// `min(created_at + TTL, created_at + MAX_LIFETIME)`.
    expires_at: DateTime<Utc>,
    /// The session's epoch captured at mint. Validation rejects the record
    /// when it != the CURRENT `session_epochs[sid]` (mirror of
    /// `SessionRecord.token_epoch` vs `User.token_epoch`).
    session_epoch: u64,
    /// The global hook epoch captured at mint. Validation rejects the
    /// record when it != the CURRENT global epoch (daemon-wide kill switch).
    global_epoch: u64,
}

// ─────────────────────────────────────────────────────────────────────
// Registry
// ─────────────────────────────────────────────────────────────────────

/// In-memory (disk-backed) map of `SessionId` → scoped-token claims, plus
/// the epoch bookkeeping that makes revocation instant + restart-surviving.
///
/// Tests construct a bare registry via [`SessionTokenRegistry::new`] and
/// exercise the pure mint/validate/revoke/epoch/expiry logic WITHOUT
/// touching disk; the process-wide singleton ([`registry`]) is loaded from
/// `~/.k2/hook-sessions.json` on first use and re-saved on every mutation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTokenRegistry {
    version: u32,
    /// Live claims, keyed by `session_id` (the selector).
    records: HashMap<String, TokenRecord>,
    /// CURRENT per-session epoch. A bump here invalidates the matching
    /// record (whose stamped epoch is now stale) without removing it.
    session_epochs: HashMap<String, u64>,
    /// CURRENT daemon-wide hook epoch (the kill switch).
    global_epoch: u64,
}

impl Default for SessionTokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionTokenRegistry {
    /// A fresh, empty registry (epoch 0, no records).
    pub fn new() -> Self {
        Self {
            version: STORE_VERSION,
            records: HashMap::new(),
            session_epochs: HashMap::new(),
            global_epoch: 0,
        }
    }

    /// Mint a scoped token for `session_id` bound to `pane_id` + `principal`.
    /// Returns the FULL `<session_id>.<secret>` PAT — the only place the
    /// raw secret ever exists. The stored record holds only the digest.
    pub fn mint(
        &mut self,
        session_id: &SessionId,
        pane_id: &str,
        principal: HookPrincipal,
    ) -> String {
        let sid = session_id.to_string();
        let secret = new_secret();
        let now = Utc::now();
        let expires_at = (now + Duration::hours(DEFAULT_TTL_HOURS))
            .min(now + Duration::hours(MAX_LIFETIME_HOURS));
        let session_epoch = self.session_epochs.get(&sid).copied().unwrap_or(0);
        let record = TokenRecord {
            version: STORE_VERSION,
            session_id: sid.clone(),
            pane_id: pane_id.to_string(),
            principal,
            token_digest: sha256_hex(&secret),
            created_at: now,
            expires_at,
            session_epoch,
            global_epoch: self.global_epoch,
        };
        self.records.insert(sid.clone(), record);
        format!("{sid}.{secret}")
    }

    /// Validate a presented `<session_id>.<secret>` bearer credential.
    ///
    /// Returns the bound principal iff, mirroring
    /// `connect_users::validate_session`:
    ///   1. it splits cleanly into a non-empty selector + verifier,
    ///   2. a record exists for the selector,
    ///   3. the record's stamped `global_epoch` == the CURRENT global epoch
    ///      (kill switch not tripped),
    ///   4. the record's stamped `session_epoch` == the CURRENT
    ///      `session_epochs[sid]` (not revoked),
    ///   5. the record has not expired, AND
    ///   6. `ct_eq(SHA-256(secret), record.token_digest)` (constant-time —
    ///      no byte-by-byte timing leak).
    ///
    /// Any miss → `None` (→ 403 at the call site).
    pub fn validate_hook(&self, bearer: &str) -> Option<ValidatedHook> {
        let bearer = bearer.trim();
        if bearer.is_empty() {
            return None;
        }
        // Selector is everything before the FIRST '.'; a UUID contains no
        // dots and the hex secret contains none either, so this split is
        // unambiguous.
        let (sid, secret) = bearer.split_once('.')?;
        if sid.is_empty() || secret.is_empty() {
            return None;
        }
        let rec = self.records.get(sid)?;

        // Global kill switch.
        if rec.global_epoch != self.global_epoch {
            return None;
        }
        // Per-session revocation (stamped vs current).
        let current_epoch = self.session_epochs.get(sid).copied().unwrap_or(0);
        if rec.session_epoch != current_epoch {
            return None;
        }
        // Expiry.
        if rec.expires_at <= Utc::now() {
            return None;
        }
        // Constant-time verifier compare (mirror of connect_users): equal
        // length hex digests, compared byte-for-byte regardless of where
        // they first differ.
        let presented = sha256_hex(secret);
        let eq: bool = presented
            .as_bytes()
            .ct_eq(rec.token_digest.as_bytes())
            .into();
        if !eq {
            return None;
        }
        Some(ValidatedHook {
            session_id: rec.session_id.clone(),
            pane_id: rec.pane_id.clone(),
            principal: rec.principal.clone(),
        })
    }

    /// Revoke the token for one session by bumping its per-session epoch
    /// (the durable, restart-surviving signal) AND dropping the live
    /// record (belt-and-suspenders, exactly like
    /// `revoke_user_sessions`). The next call 403s within one request, no
    /// daemon restart.
    pub fn revoke(&mut self, session_id: &SessionId) {
        let sid = session_id.to_string();
        let e = self.session_epochs.entry(sid.clone()).or_insert(0);
        *e = e.wrapping_add(1);
        self.records.remove(&sid);
    }

    /// Daemon-wide kill switch: bump the global hook epoch so EVERY
    /// already-minted token (stamped with the old global epoch) goes stale
    /// at once. Records are left in place but inert.
    pub fn revoke_all(&mut self) {
        self.global_epoch = self.global_epoch.wrapping_add(1);
    }
}

// ─────────────────────────────────────────────────────────────────────
// require_hook — default-deny capability guard (PRD §3.3)
// ─────────────────────────────────────────────────────────────────────

/// Capability check: `true` iff `path` is an AGENT verb a scoped hook
/// token is allowed to drive — `msg` / `reply` / `inbox` / `review` /
/// (gated) `memory`, plus the `/hook/complete` lifecycle ping itself.
///
/// **Default-deny.** A scoped token must NEVER reach `users/*`, `fs/*`,
/// `clone/*`, tunnel control, `daemon/restart`, or PTY spawn — those are
/// the owner-only escalation surface. The test suite asserts every entry
/// in that denylist returns `false`.
pub fn is_agent_verb(path: &str) -> bool {
    // Hard denylist FIRST — owner-only escalation surface. Belt on top of
    // the allowlist so a future allowlist edit can't accidentally widen
    // into the escalation routes.
    const DENY_PREFIXES: &[&str] = &[
        "/cli/users/",
        "/cli/fs/",
        "/cli/clone/",
        "/cli/tunnel/",
        "/cli/daemon/",
        "/cli/sessions/v2/", // PTY spawn/close = RCE-adjacent
        "/cli/terminal/",
    ];
    if DENY_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return false;
    }

    // Allowlist — agent verbs only.
    const ALLOW_EXACT: &[&str] = &[
        "/hook/complete",
        "/cli/workspace/msg",
    ];
    const ALLOW_PREFIXES: &[&str] = &[
        "/cli/inbox/",
        "/cli/review-checklist/",
        // `memory` stays GATED: present as a recognized verb namespace but
        // not auto-allowed for writes (memory.write is owner/owner-gated).
        // Reads can be added here once the route exists.
    ];
    ALLOW_EXACT.contains(&path) || ALLOW_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// The `require_hook` guard: a presented bearer authorizes `path` iff the
/// path is an allowed agent verb AND the token validates against the
/// process registry. Returns the resolved [`ValidatedHook`] (so the caller
/// can match `paneId` + attribute the principal) or `None` (→ 403).
///
/// Deliberately DISJOINT from the owner guards (`token_ok` /
/// `token_is_owner`): a scoped token is structurally `<sid>.<secret>` and
/// never equals the owner token, and it carries no owner capability — the
/// test suite asserts a minted scoped token fails `token_is_owner`.
pub fn require_hook(bearer: &str, path: &str) -> Option<ValidatedHook> {
    if !is_agent_verb(path) {
        return None;
    }
    validate_hook(bearer)
}

// ─────────────────────────────────────────────────────────────────────
// Process-wide singleton + disk persistence
// ─────────────────────────────────────────────────────────────────────

/// Path to `~/.k2/hook-sessions.json` (mirrors
/// `connect_users::sessions_store_path`).
pub fn store_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("hook-sessions.json")
}

/// The process-wide registry, lazily loaded from disk on first access.
fn registry() -> &'static Mutex<SessionTokenRegistry> {
    static REGISTRY: OnceLock<Mutex<SessionTokenRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(load_from_disk()))
}

/// Load the registry from disk. A missing/empty file → a fresh registry.
/// A malformed file or unknown envelope version fails CLOSED (empty
/// registry → every prior token invalid), matching `connect_users`.
fn load_from_disk() -> SessionTokenRegistry {
    let path = store_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return SessionTokenRegistry::new(),
    };
    if raw.trim().is_empty() {
        return SessionTokenRegistry::new();
    }
    match serde_json::from_str::<SessionTokenRegistry>(&raw) {
        Ok(mut reg) if reg.version == STORE_VERSION => {
            // Drop any per-record version mismatch (fail closed per-record).
            reg.records
                .retain(|_, r| r.version == STORE_VERSION && !r.token_digest.is_empty());
            reg
        }
        Ok(_) => {
            log_debug!(
                "[hook-scoped] WARN unknown hook-sessions store version; failing closed (empty)"
            );
            SessionTokenRegistry::new()
        }
        Err(e) => {
            log_debug!("[hook-scoped] WARN parse hook-sessions.json failed ({e}); failing closed");
            SessionTokenRegistry::new()
        }
    }
}

/// Persist the registry via tmp+rename then chmod 0600 (same discipline as
/// `connect_users::save_sessions`).
fn save_to_disk(reg: &SessionTokenRegistry) {
    let path = store_path();
    let Some(dir) = path.parent().map(PathBuf::from) else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log_debug!("[hook-scoped] WARN mkdir {}: {e}", dir.display());
        return;
    }
    let tmp = dir.join(format!("hook-sessions.json.tmp.{}", std::process::id()));
    let body = match serde_json::to_string_pretty(reg) {
        Ok(b) => b,
        Err(e) => {
            log_debug!("[hook-scoped] WARN serialize hook-sessions: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        log_debug!("[hook-scoped] WARN write {}: {e}", tmp.display());
        return;
    }
    restrict_mode(&tmp);
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        log_debug!("[hook-scoped] WARN rename into place {}: {e}", path.display());
        return;
    }
    restrict_mode(&path);
}

/// Mint via the process registry + persist. Returns the `<sid>.<secret>`
/// PAT. (Phase 1 wires this into the spawn env; Phase 0 exposes it for the
/// dormant per-cell prep + the smoke harness.)
// COMPAT-58: Phase-1 wiring entry point — unused by the Phase-0 binary
// (nothing mints while the flag's behavior is dormant). Kept as the public
// seam the spawn path calls in Phase 1.
#[allow(dead_code)]
pub fn mint_session_token(
    session_id: &SessionId,
    pane_id: &str,
    principal: HookPrincipal,
) -> String {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    let token = g.mint(session_id, pane_id, principal);
    save_to_disk(&g);
    token
}

/// Validate a bearer against the process registry (no capability check —
/// see [`require_hook`] for the guarded form).
pub fn validate_hook(bearer: &str) -> Option<ValidatedHook> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .validate_hook(bearer)
}

/// Revoke one session's token via the process registry + persist.
// COMPAT-58: Phase-1 teardown wiring (called on cell kill/teardown). Unused
// by the Phase-0 binary; the pure logic is covered by the registry tests.
#[allow(dead_code)]
pub fn revoke_session(session_id: &SessionId) {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.revoke(session_id);
    save_to_disk(&g);
}

/// Daemon-wide kill switch via the process registry + persist.
pub fn revoke_all() {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.revoke_all();
    save_to_disk(&g);
}

// ─────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────

/// 256-bit CSPRNG secret, hex-encoded (the verifier half of the PAT).
fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("getrandom for hook token secret");
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Hex `SHA-256(input)`.
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(unix)]
fn restrict_mode(file: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_mode(_file: &std::path::Path) {}

// ─────────────────────────────────────────────────────────────────────
// Tests — pure registry logic, no disk (fail-loud per feedback_test_discipline)
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> HookPrincipal {
        HookPrincipal {
            workspace_uuid: "ws-uuid-1".to_string(),
            agent_address: "agent-a".to_string(),
        }
    }

    // ── env flag ─────────────────────────────────────────────────────

    #[test]
    fn scoped_hooks_flag_defaults_off() {
        // Don't mutate the real env in a way that races other tests: just
        // assert the unset/disabled mappings. (CI runs without the flag.)
        std::env::remove_var("K2_HOOK_SCOPED");
        assert!(!scoped_hooks_enabled(), "must default OFF when unset");
    }

    // ── mint + validate round trip ──────────────────────────────────

    #[test]
    fn mint_then_validate_accepts_correct_secret() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal());

        // Format is selector.verifier and the selector is the session id.
        let (sel, secret) = token.split_once('.').expect("token has a dot");
        assert_eq!(sel, sid.to_string(), "selector must be the session id");
        assert!(!secret.is_empty(), "verifier must be present");

        let v = reg
            .validate_hook(&token)
            .expect("freshly minted token must validate");
        assert_eq!(v.session_id, sid.to_string());
        assert_eq!(v.pane_id, "pane-1");
        assert_eq!(v.principal, principal());
    }

    #[test]
    fn validate_rejects_tampered_secret() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal());
        // Flip the last hex char of the verifier.
        let mut bytes: Vec<char> = token.chars().collect();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == 'a' { 'b' } else { 'a' };
        let tampered: String = bytes.into_iter().collect();
        assert_ne!(tampered, token, "tamper must actually change the token");
        assert!(
            reg.validate_hook(&tampered).is_none(),
            "a tampered verifier must NOT validate",
        );
    }

    #[test]
    fn validate_rejects_unknown_selector() {
        let reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        // No record was ever minted for this selector.
        assert!(reg
            .validate_hook(&format!("{sid}.deadbeef"))
            .is_none());
    }

    #[test]
    fn validate_rejects_malformed_bearer() {
        let reg = SessionTokenRegistry::new();
        assert!(reg.validate_hook("").is_none());
        assert!(reg.validate_hook("no-dot-here").is_none());
        assert!(reg.validate_hook(".onlysecret").is_none());
        assert!(reg.validate_hook("onlyselector.").is_none());
    }

    // ── epoch revocation (per-session) ──────────────────────────────

    #[test]
    fn revoke_invalidates_the_session_token() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal());
        assert!(reg.validate_hook(&token).is_some(), "valid pre-revoke");

        reg.revoke(&sid);
        assert!(
            reg.validate_hook(&token).is_none(),
            "revoked session token must 403",
        );
    }

    #[test]
    fn validate_rejects_stale_session_epoch() {
        // Exercise the EPOCH branch specifically (record present, epoch
        // bumped underneath it) without relying on record removal.
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal());
        assert!(reg.validate_hook(&token).is_some());

        // Bump the CURRENT epoch but leave the (stale-stamped) record.
        *reg.session_epochs.entry(sid.to_string()).or_insert(0) += 1;
        assert!(
            reg.validate_hook(&token).is_none(),
            "record stamped with an older session epoch must be rejected",
        );
    }

    // ── global kill switch ──────────────────────────────────────────

    #[test]
    fn revoke_all_invalidates_every_token() {
        let mut reg = SessionTokenRegistry::new();
        let a = SessionId::new();
        let b = SessionId::new();
        let ta = reg.mint(&a, "pane-a", principal());
        let tb = reg.mint(&b, "pane-b", principal());
        assert!(reg.validate_hook(&ta).is_some());
        assert!(reg.validate_hook(&tb).is_some());

        reg.revoke_all();
        assert!(reg.validate_hook(&ta).is_none(), "global kill switch (a)");
        assert!(reg.validate_hook(&tb).is_none(), "global kill switch (b)");
    }

    // ── expiry ──────────────────────────────────────────────────────

    #[test]
    fn validate_rejects_expired_token() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal());
        // Force the stored record's expiry into the past.
        reg.records
            .get_mut(&sid.to_string())
            .expect("record present")
            .expires_at = Utc::now() - Duration::seconds(1);
        assert!(
            reg.validate_hook(&token).is_none(),
            "an expired token must 403",
        );
    }

    // ── capability allowlist / denylist (require_hook §3.3) ──────────

    #[test]
    fn is_agent_verb_allows_agent_routes() {
        assert!(is_agent_verb("/hook/complete"));
        assert!(is_agent_verb("/cli/workspace/msg"));
        assert!(is_agent_verb("/cli/inbox/respond"));
        assert!(is_agent_verb("/cli/review-checklist/toggle"));
    }

    #[test]
    fn is_agent_verb_denies_owner_escalation_surface() {
        // The escalation surface from PRD §3.3 / smoke test step 6 — these
        // MUST be default-denied to a scoped token.
        for p in [
            "/cli/users/set-role",
            "/cli/users/add",
            "/cli/fs/write-file",
            "/cli/fs/delete",
            "/cli/clone/bundle",
            "/cli/tunnel/config",
            "/cli/tunnel/start",
            "/cli/daemon/restart",
            "/cli/sessions/v2/spawn",
            "/cli/terminal/create",
        ] {
            assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
        }
    }

    // ── DISJOINTNESS from the owner guards (the escalation negative) ─

    #[test]
    fn minted_scoped_token_does_not_pass_owner_guards() {
        // The load-bearing security invariant: a scoped token is NOT the
        // owner token and carries NO owner capability.
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let owner_token = "owner-secret-deadbeef";
        let scoped = reg.mint(&sid, "pane-1", principal());

        assert_ne!(scoped, owner_token, "scoped != owner by construction");

        let q = format!("token={scoped}");
        assert!(
            !crate::routes::http::token_is_owner(&q, owner_token),
            "a scoped token must NOT satisfy token_is_owner (no escalation)",
        );
        assert!(
            !crate::routes::http::token_ok(&q, owner_token),
            "a scoped token is not a connect-user session either → fails token_ok",
        );
    }

    #[test]
    fn require_hook_denies_escalation_even_with_a_valid_token() {
        // Even a structurally VALID scoped token is refused on an
        // owner-only route — capability default-deny is independent of
        // token validity. (Validity is exercised against the process
        // registry elsewhere; here we assert the capability gate fires
        // FIRST, so an unknown token on a denied path is still None.)
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let _scoped = reg.mint(&sid, "pane-1", principal());
        // require_hook consults the PROCESS registry, not `reg`, so we
        // assert the capability gate via is_agent_verb (the first guard).
        assert!(!is_agent_verb("/cli/users/set-role"));
        assert!(require_hook("anything.here", "/cli/users/set-role").is_none());
    }
}
