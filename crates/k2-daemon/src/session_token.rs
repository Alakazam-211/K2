//! Scoped per-session hook tokens (#58 Phase 1 — default ON, dual-accept).
//!
//! ## Why this exists
//!
//! Historically the "hook token" injected into every spawned PTY WAS the
//! daemon OWNER token (`main.rs` writes one `generate_token()` into
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
//! ## Phase 1 scope (this module + spawn paths)
//!
//! Gated on `K2_HOOK_SCOPED` (**default ON**; opt out with `0`/`false`/`off`).
//! When ON, every agent spawn path mints a scoped passport into
//! `K2_HOOK_TOKEN` + binds a per-cell UDS. **The owner token never enters
//! an agent child env** (the load-bearing security fix of PR-A). Owner
//! still works on TCP for the host/app surface (dual-accept) — Phase 2
//! (owner REJECTION on agent verbs) ships separately.
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

/// Wall-clock lifetime used only when stamping `TokenRecord::expires_at`
/// for on-disk schema compatibility. **Not enforced on validate.**
///
/// Long-running agent cells (heartbeats, multi-day/month work) keep the
/// same env passport for the life of the PTY. There is no sliding re-mint
/// (the child env cannot be rewritten after spawn). A 24h wall TTL was
/// stranding agents after a day with "auth token expired" / failed
/// peer messaging while the session was still live.
///
/// The real security boundary is **epoch revocation** at cell teardown
/// (`revoke_session` from the child-exit observer / `v2_spawn`) plus the
/// global kill switch — instant and restart-surviving. Abandoned records
/// for sessions that never tore down cleanly are cleaned when the session
/// map / registry is pruned, not by stranding live agents.
///
/// `expires_at` is still written far in the future so older code that
/// inspects the field does not treat live passports as already stale.
const TOKEN_RECORD_HORIZON_YEARS: i64 = 100;

// ─────────────────────────────────────────────────────────────────────
// Feature flag
// ─────────────────────────────────────────────────────────────────────

/// True unless `K2_HOOK_SCOPED` is explicitly set to a negative value
/// (`0`/`false`/`no`/`off`, case-insensitive). **Default ON** (Phase 1 /
/// PR-A — agent sessions get a real passport at spawn; dual-accept keeps
/// owner TCP for host/app rollback).
///
/// This is the single gate for minting, the per-cell UDS bind, and the
/// scoped arm of `/hook/complete`. Explicit opt-out is the fleet-safety
/// rollback (`Environment=K2_HOOK_SCOPED=0`).
pub fn scoped_hooks_enabled() -> bool {
    match std::env::var("K2_HOOK_SCOPED") {
        Ok(v) => {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off" | ""
            )
        }
        // Unset → ON (Phase 1 default). Cloud provision scripts also set
        // `K2_HOOK_SCOPED=1` explicitly; that remains a no-op affirmative.
        Err(_) => true,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Principal + claims
// ─────────────────────────────────────────────────────────────────────

/// B3a — how the in-cell agent (Claude Code) obtains its model credential.
///
/// Resolved HOST-SIDE at the spawn door and STAMPED into the token claim;
/// the cell NEVER chooses its own mode (a body-supplied mode would let one
/// tenant pick another tenant's billing/credential path). Read from the
/// VALIDATED token, never from a request body. `#[serde(default)]` → `ApiKey`
/// so a record persisted before this field existed loads as the primary
/// (tenant/server) path, not as the deferred subscription stub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CredMode {
    /// B3a (PRIMARY — tenant/server). A PER-WORKSPACE configured API key is
    /// staged into the cell's guest env (`<provider.key_env_var()>=<key>`);
    /// the in-cell Claude Code picks it up and skips interactive auth.
    #[default]
    ApiKey,
    /// B3b (DEFERRED — own-use). Reuse the on-device subscription creds by
    /// bind-mounting them into the cell. NOT built in B3a — this arm is a
    /// no-op stub here: it stages NO key (the env builder treats it the same
    /// as "no key configured").
    Subscription,
}

/// B3a — the model provider whose credential is embedded. Carries the env
/// var the Claude-Code-compatible CLI reads its key from. Anthropic-only
/// today; adding a provider later is ONE enum arm + its key env var, no
/// architecture change. Resolved host-side + bound in the token claim; read
/// from the VALIDATED token, never the request body.
///
/// SCOPE NOTE (W5, 0.40.30): this enum covers ONLY the B3a PER-WORKSPACE
/// key staged into scoped-hook microVM cells (`cell_env_pairs`; the 0056
/// `projects.anthropic_api_key` store, which carries no provider metadata).
/// The `/v1` PRINCIPAL credential path is provider-aware separately via
/// [`k2_core::api_keys::LlmProvider`] +
/// [`k2_core::api_keys::ApiPrincipal::staged_env_pairs`] — extend THAT
/// mapping for principal keys, and this enum only when the per-workspace
/// store learns providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Anthropic,
    // TODO(provider): OpenAI→OPENAI_API_KEY, Gemini→GEMINI_API_KEY (see
    // SCOPE NOTE — mirror k2_core::api_keys::LlmProvider when built).
}

impl Provider {
    /// The environment variable name the in-cell CLI reads the embedded key
    /// from. (Anthropic → `ANTHROPIC_API_KEY`, which Claude Code honors to
    /// skip login and call Anthropic directly.)
    pub fn key_env_var(&self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            // TODO(provider): OpenAI → "OPENAI_API_KEY", Gemini → "GEMINI_API_KEY"
        }
    }
}

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
    /// B3a — the credential mode the cell was spawned under, surfaced from
    /// the VALIDATED token (never the request body).
    pub cred_mode: CredMode,
    /// B3a — the model provider bound at mint.
    pub provider: Provider,
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
    /// Far-future stamp only (`TOKEN_RECORD_HORIZON_YEARS`). **Not** used
    /// to cut off live agents — see [`SessionTokenRegistry::validate_hook`].
    expires_at: DateTime<Utc>,
    /// The session's epoch captured at mint. Validation rejects the record
    /// when it != the CURRENT `session_epochs[sid]` (mirror of
    /// `SessionRecord.token_epoch` vs `User.token_epoch`).
    session_epoch: u64,
    /// The global hook epoch captured at mint. Validation rejects the
    /// record when it != the CURRENT global epoch (daemon-wide kill switch).
    global_epoch: u64,
    /// B3a — credential mode stamped host-side at mint. `#[serde(default)]`
    /// → `ApiKey` so records written before this field existed load as the
    /// primary path. NOTE: the raw API KEY is NEVER persisted here — only the
    /// MODE. The key lives solely in the (transient) cell guest env.
    #[serde(default)]
    cred_mode: CredMode,
    /// B3a — model provider stamped host-side at mint (default `Anthropic`).
    #[serde(default)]
    provider: Provider,
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
        cred_mode: CredMode,
        provider: Provider,
    ) -> String {
        self.mint_sid(&session_id.to_string(), pane_id, principal, cred_mode, provider)
    }

    /// [`Self::mint`] for a selector that isn't a `SessionId` (a `k2 job`
    /// run's `job-<run id>`). The selector must hold no `.`.
    fn mint_sid(
        &mut self,
        sid: &str,
        pane_id: &str,
        principal: HookPrincipal,
        cred_mode: CredMode,
        provider: Provider,
    ) -> String {
        debug_assert!(!sid.contains('.'), "a token selector holds no dot");
        let sid = sid.to_string();
        let secret = new_secret();
        let now = Utc::now();
        // Far-future stamp only — validate does not cut off live agents.
        let expires_at = now + Duration::days(365 * TOKEN_RECORD_HORIZON_YEARS);
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
            cred_mode,
            provider,
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
    ///      `session_epochs[sid]` (not revoked), AND
    ///   5. `ct_eq(SHA-256(secret), record.token_digest)` (constant-time —
    ///      no byte-by-byte timing leak).
    ///
    /// Wall-clock `expires_at` is **not** checked — passports live for the
    /// life of the cell (months-long agents must keep messaging). Revocation
    /// is epoch-based at teardown / kill-switch only.
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
            cred_mode: rec.cred_mode,
            provider: rec.provider,
        })
    }

    /// Revoke the token for one session by bumping its per-session epoch
    /// (the durable, restart-surviving signal) AND dropping the live
    /// record (belt-and-suspenders, exactly like
    /// `revoke_user_sessions`). The next call 403s within one request, no
    /// daemon restart.
    pub fn revoke(&mut self, session_id: &SessionId) {
        self.revoke_sid(&session_id.to_string())
    }

    /// [`Self::revoke`] by selector string (`k2 job` runs).
    fn revoke_sid(&mut self, sid: &str) {
        let sid = sid.to_string();
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

    /// The SCOPED half of the `/hook/complete` dual-accept decision (and the
    /// per-cell UDS server's auth): a presented bearer authorizes a hook for
    /// `req_pane` iff it validates against this registry AND is bound to
    /// EXACTLY that pane. A token scoped to cell A presenting `paneId` = cell
    /// B → `false` (PRD §5 smoke #4). The OWNER-token arm is independent
    /// (`ct_eq_token`) and handled by the caller — this is the scoped half
    /// only, so it is provably DISJOINT from the owner credential.
    ///
    /// NOTE (0.40.31): the LIVE `/hook/complete` gate in
    /// `routes::dispatcher` inlines this decision via `require_hook` +
    /// pane equality (it also path-gates on `is_agent_verb`); this method
    /// exists to pin the scope-enforcement semantics in unit tests, so it
    /// is compiled for test builds only.
    #[cfg(test)]
    pub fn scoped_hook_authorizes_pane(&self, bearer: &str, req_pane: &str) -> bool {
        if req_pane.is_empty() {
            return false;
        }
        match self.validate_hook(bearer) {
            Some(v) => v.pane_id == req_pane,
            None => false,
        }
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
///
/// Permission cards (prd-agent-settings-cards-v1 §2.4, vs-live C1): the
/// `/cli/permission/` family is admitted only while the global "Permission
/// cards" switch is on. Off, a passport on it gets exactly what it gets on
/// any non-agent path today (`require_hook` → `None` → the cell's codeless
/// 403). On, its handlers answer an agent whose own switch is off with that
/// same body, and refuse a passport on the person verbs.
pub fn is_agent_verb(path: &str) -> bool {
    is_agent_verb_when(path, k2_core::permission::gate::server_on())
}

/// [`is_agent_verb`] with the permission-cards switch given (pure; tests).
fn is_agent_verb_when(path: &str, permission_cards_on: bool) -> bool {
    if k2_core::permission::gate::agent_verb_gated(path) {
        return permission_cards_on;
    }
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
        // Sew (S28 §17 item 8, S32 §11.1): agents never get chat passes,
        // the key routes or the install owner. Exact paths, so S30's agent
        // verbs under `/cli/sew/chat/` stay possible.
        "/cli/sew/pass",
        "/cli/sew/owner-handoff",
        "/cli/sew/keys",
        // `/cli/terminal/*` mutations (write/spawn/kill/…) stay denied;
        // C2 admits exact `/cli/terminal/read` via ALLOW_EXACT below so
        // agents can peer-gated `k2 read` of connected workspaces.
        "/cli/terminal/write",
        "/cli/terminal/create",
        "/cli/terminal/kill",
        "/cli/terminal/resize",
        "/cli/terminal/kill-foreground",
        "/cli/terminal/scroll",
        "/cli/terminal/log",
        "/cli/terminal/lifecycle-write",
        "/cli/terminal/set-focus",
        "/cli/terminal/pin-size",
        "/cli/terminal/send-message",
        "/cli/terminal/compose-history",
        "/cli/terminal/classify",
        // Wave 0: mail owner/admin surfaces stay off the scoped allowlist
        // (config, approvals, external link, access, doctor, plus M6
        // leftovers uninstall/domain-remove). M5 hostmail manage paths
        // (server enable/disable, domain list/show/add/check) ride
        // ALLOW_PREFIXES `/cli/mail/` and are extra-gated by
        // `mail_manage_allowed_for_path`. Do not prefix-open
        // `/cli/mail/server/` or `/cli/mail/domain/` — exact-DENY the
        // blast leftovers. Writer `/cli/mail-manage` is not an agent verb.
        "/cli/mail/server/uninstall",
        // Calendars S1: replaces the Stalwart binary (owner/admin only, CAL6).
        "/cli/mail/server/upgrade",
        "/cli/mail/domain/remove",
        // Calendars S2: host-wide DAV policy (owner/admin only, CAL22).
        "/cli/mail/dav",
        // Calendars S6: Apple profile mints a mailbox credential (owner/admin).
        "/cli/mail/profile",
        "/cli/mail/external/",
        "/cli/mail/link/",
        "/cli/mail/oauth-config",
        "/cli/mail-manage",
        // Workspace data sidecar owner surfaces (enable/disable/doctor).
        "/cli/db/server/",
        "/cli/db/doctor",
        "/cli/db/bind",
        // Host-wide Tickets list (every workspace, incl. removed ones) is
        // a human surface. No `/cli/feedback/` is on the allowlist; this
        // is the belt.
        "/cli/feedback/list-all",
        // DNS K1: zone lifecycle is owner/dashboard-only. Agent DNS verbs
        // (access/zones list/records CRUD/verify) ride ALLOW_PREFIXES
        // `/cli/dns/` below; these exact owner surfaces stay denied.
        "/cli/dns/zones/create",
        "/cli/dns/zones/delete",
        // Heartbeat schedule is agent-owned per workspace, but OS tick
        // install + fleet-wide list stay owner/dashboard-only.
        "/cli/heartbeat/install-launchd",
        "/cli/heartbeat/uninstall-launchd",
        "/cli/heartbeat/apply-wake-scheduler",
        "/cli/heartbeat/list-all",
        "/cli/heartbeat/fires-list-all",
        "/cli/heartbeat/set-show-sessions",
        "/cli/heartbeat/active-projects",
        // Rosson R1 (prd-app-heartbeats-surface-v1): hard delete trashes
        // the WAKEUP.md folder and is owner-only. Agents archive instead.
        "/cli/heartbeat/remove",
        // K2 vault: people only (KV14 DENY, VS2).
        "/cli/vault/confirm/",
        "/cli/vault/confirm-password/",
        "/cli/vault/reveal",
        "/cli/vault/recovery/",
        "/cli/vault/kek/",
        "/cli/vault/rekey",
        "/cli/vault/rollback",
        "/cli/vault/restore",
        "/cli/vault/backup",
        "/cli/vault/legacy-export",
        // prd-vault-gaps-v1 item 4: systemd credentials (and their remove)
        // are a person's, with a confirm (Q8: not a card in v1).
        "/cli/vault/systemd-cred",
        // Secret handoff (prd-agent-secret-handoff-v1 §13): a person
        // accepts or declines; an `accept-from` POST is refused in the handler.
        "/cli/vault/incoming/decide",
        // Sew on K2 (S29 K12): revealing the master recovery key is a
        // person's step (vault confirm). Purge and drop-prev ride routes an
        // agent may reach; their handlers refuse every passport.
        "/cli/sew/server/recovery-key",
        // Thread export / import (T4) are people only (the `/cli/thread`
        // allow prefix would otherwise admit them). Covers `imports`.
        "/cli/thread/export",
        "/cli/thread/import",
        // app-room-stream (0.46.3): the multi-room Thread socket is for App
        // passes and people (contract-app-room-stream-0463 §1: a passport
        // is 403). A WebSocket, never a cell-socket GET.
        "/cli/thread/stream",
        // Email Notifications (0.46.5): senders, the system sender, relays
        // and their settings are the owner's and Admins' (the handlers
        // refuse every passport too). `k2 app email` is under `/cli/app/`.
        "/cli/notifications/",
        // Folder trust (SS10): the owner decides; agents only ask.
        "/cli/folder-trust/decide",
        "/cli/folder-trust/reset",
        "/cli/folder-trust/forget",
    ];
    if DENY_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return false;
    }

    // Allowlist — agent verbs only.
    const ALLOW_EXACT: &[&str] = &[
        "/hook/complete",
        // prd-daemon-activity-and-thread-working-v1 DA12 (A4): the full-
        // payload hook ingest. The cell server pins the pane to the token.
        "/hook/event",
        "/cli/workspace/msg",
        // Changes tab v1 (`k2 changes`): the cell server scopes every one
        // of these to the passport's own workspace (git_routes::AGENT_ROUTES).
        "/cli/git/commit",
        "/cli/git/diff-file",
        "/cli/git/discard",
        // C5: review notes (list + resolve only) and `k2 changes open`.
        "/cli/git/init",
        "/cli/git/notes",
        "/cli/git/notes/resolve",
        "/cli/git/open-changes",
        "/cli/git/snapshot",
        "/cli/git/stage",
        "/cli/git/stage-all",
        "/cli/git/unstage",
        // First run without git (0.46.5, first-run review item 13): an
        // agent reads the state and runs recheck (only git steps that
        // already wait). `/cli/git-tools/install` is NOT here: it opens
        // Apple's installer on the owner's screen (owner token only).
        "/cli/git-tools",
        "/cli/git-tools/recheck",
        // C2: workspace-addressed live terminal peek (`k2 read <ws>`).
        // Peer gate runs in the handler when principal is scoped.
        "/cli/terminal/read",
        // #58 Phase-1 close: awareness publish is the agent's peer-signal
        // egress (status/reservation/presence). +1 allowlist delta this
        // release. `subscribe` (a WS) is NOT here — read-only fan-out stays
        // owner/connect-user over TCP.
        "/cli/awareness/publish",
        // F2 (sandbox API): the in-cell agent's RESPONSE egress. `/cli/respond`
        // appends a line to this session's response log (drained by the external
        // `GET /v1/sandboxes/<id>/messages`). Pure params-driven write; carries
        // no escalation — it is NOT under any DENY_PREFIX (users/fs/clone/tunnel/
        // daemon/sessions/terminal). The cell server pins it to THIS session id
        // (the body never names the session), so a cell can only write its own log.
        "/cli/respond",
        // Host-session completion lifecycle: `k2 done` → mark_complete only
        // (arms Grace). Does NOT append a product message to the respond drain.
        // Exact match; same scoped-hook auth as `/cli/respond`.
        "/cli/session/complete",
        // Folder trust (prd-security-sweep-0464-v1 SS10): agents ask
        // (`k2 workspace trust request`, which only presents the card) and
        // read the status; people decide (`decide`/`reset` are denied below).
        "/cli/folder-trust/status",
        "/cli/folder-trust/request",
        // 0.40.34 browser-open: the staged k2-open shim's egress from a
        // SCOPED session (K2_HOOK_SOCK + K2_HOOK_TOKEN) — xdg-open/$BROWSER
        // inside the cell forwards the URL so the CONNECTED app opens it.
        // Carries no escalation: the handler validates http/https-only and
        // broadcasts an app-level `open_url` event; it never touches the
        // filesystem and never shells out. Not under any DENY_PREFIX
        // (`/cli/browser/` is new with this verb).
        "/cli/browser/open-url",
        // Design mode (prd-browser-design-mode-v1 D-A13): `k2 browser note`
        // from an agent files page notes into ITS OWN workspace's Thread
        // (authorize_write refuses another workspace). No escalation: a
        // Thread post plus the same wake + inject a person's post gets.
        "/cli/browser/notes",
        "/cli/browser/notes/redeliver",
        // C1 (0.40.45): workspace connections list/add/remove. Mutate is
        // further gated by agents_can_create_connections_for_path (owner
        // always bypasses; agents need the effective toggle ON). List is
        // free for any authenticated agent principal.
        "/cli/connections",
        // Agent hire (0.45.6): `k2 agent hire <dir>` in a cell sends the
        // passport (the handler refuses a DB-access raise, HT14); `k2 agent
        // team` reads its own workspace's links; `k2 msg who` checks the
        // sender of a delivery to or from its own workspace. Team hire,
        // link, unlink and retire are NOT here (people only).
        "/cli/agents/hire",
        "/cli/agent-teams/links",
        "/cli/msg/who",
        // 0.46.1 `k2 agent identity`: an agent reads and sets its OWN
        // color and picture. The handler pins the workspace to the passport
        // and refuses a name. `/cli/agent/handle` is NOT here (people and
        // App passes with agent:handle only).
        "/cli/agent/identity",
        // 0.45.7: `k2 publish status` / `subdomain create|target|rm` from
        // an agent — the daemon makes the control-plane call with the
        // token it holds. `connect_proxy` admits a passport for its OWN
        // workspace's nested labels only (no primary, no reserved, no
        // other workspace's label, rate-limited). Exact path only.
        "/cli/connect/proxy",
        // k2 sidecar v1 (SC36): new / list / stop. The handler classifies
        // the validated session (canonical harness, sidecar, shell tab,
        // api cell) and applies the agent gate. `/cli/agent-access/set`
        // (the switch itself) is deliberately NOT here.
        // LLM login wallet: passports may READ (list/status/usage); the
        // handler refuses every mutation from a passport (agent tabs and
        // K2 shell tabs alike) with a teaching `owner_only`.
        "/cli/llm/accounts/list",
        "/cli/llm/accounts/status",
        "/cli/llm/accounts/usage",
        // The token one chat runs on and its chip number (read-only; its
        // refresh POST is people-only, refused in the handler).
        "/cli/llm/accounts/for",
        // Usage v2 (prd-usage-v2 §1): an agent reads its OWN workspace's
        // tokens and turns, and the limits; the handler forces the
        // workspace. `/cli/usage/limits/refresh` is NOT here (people only).
        "/cli/usage/tokens",
        "/cli/usage/turns",
        "/cli/usage/limits",
        // K2 vault (prd-k2-vault-v1 KV14): agents use and manage their own
        // workspace's secrets; the handler applies §8.2/§11.2. Confirm,
        // reveal, recovery and key routes are absent on purpose (DENY).
        "/cli/vault/status",
        "/cli/vault/list",
        "/cli/vault/show",
        "/cli/vault/presets",
        // Item 3: an ssh key's public half, and its agent settings (the
        // handler applies the loosening rule).
        "/cli/vault/public-key",
        "/cli/vault/agent-policy",
        "/cli/vault/set",
        "/cli/vault/rotate",
        "/cli/vault/policy",
        "/cli/vault/grant",
        "/cli/vault/revoke",
        "/cli/vault/disable",
        "/cli/vault/enable",
        "/cli/vault/destroy",
        "/cli/vault/get",
        "/cli/vault/audit",
        "/cli/vault/requests",
        "/cli/vault/doctor",
        "/cli/vault/fetch",
        "/cli/vault/env-file",
        "/cli/vault/import",
        "/cli/vault/service",
        // Manual placement records (prd-vault-gaps-v1 item 4): bookkeeping
        // for the agent's own workspace's secrets; the handler checks it.
        "/cli/vault/placements",
        "/cli/vault/placement/add",
        "/cli/vault/placement/done",
        "/cli/vault/placement/remove",
        // Migrations (VS2): an IT agent with "Allow managing server-wide
        // secrets" may run the carry; the handler checks the switch.
        "/cli/vault/recipient",
        "/cli/vault/carry/export",
        "/cli/vault/carry/import",
        // Secret handoff (prd-agent-secret-handoff-v1 §13): an agent sends
        // by name and reads its own workspace's incoming / sent and
        // allow-list. `incoming/decide` is DENY; an `accept-from` POST is
        // refused in the handler (people only, H19).
        "/cli/vault/send",
        "/cli/vault/incoming",
        "/cli/vault/sent",
        "/cli/vault/accept-from",
        "/cli/vault/ask",
        "/cli/vault/asks",
        "/cli/vault/unsend",
        // `k2 job` (prd-vault-gaps-v1 item 5, Q14): an agent manages its
        // own workspace's jobs; the handler pins the workspace, refuses a
        // job run's own passport and keeps `pause --all` for people.
        "/cli/jobs/list",
        "/cli/jobs/show",
        "/cli/jobs/logs",
        "/cli/jobs/add",
        "/cli/jobs/edit",
        "/cli/jobs/run",
        "/cli/jobs/enable",
        "/cli/jobs/disable",
        "/cli/jobs/kill",
        "/cli/jobs/remove",
        "/cli/jobs/pause",
        "/cli/jobs/resume",
        "/cli/llm/accounts/login/status",
        "/cli/llm/accounts/add",
        "/cli/llm/accounts/login",
        "/cli/llm/accounts/login/input",
        "/cli/llm/accounts/login/cancel",
        "/cli/llm/accounts/switch",
        "/cli/llm/accounts/next",
        "/cli/llm/accounts/rename",
        "/cli/llm/accounts/remove",
        "/cli/llm/accounts/refresh",
        "/cli/llm/accounts/pins",
        "/cli/llm/accounts/pin",
        "/cli/llm/accounts/unpin",
        "/cli/llm/accounts/add-key",
        "/cli/llm/accounts/clear-default",
        "/cli/llm/accounts/for/refresh",
        // App users' wallets (0.46.3): an agent of an App's own workspace
        // with switch 2 reads and sets that App's fallback only (the
        // handler checks; every other /cli/app/wallet/* answers owner_only).
        "/cli/app/wallet/settings",
        // K2 compute nodes (CN18): the agent verbs. The handler checks the
        // workspace switch (`compute_off`) and the node grant
        // (`not_granted`) and scopes everything to the passport's
        // workspace. Enroll/confirm/remove/grant/pause/policy are NOT here.
        "/cli/compute/nodes",
        "/cli/compute/nodes/info",
        "/cli/compute/run",
        "/cli/compute/jobs",
        "/cli/compute/jobs/get",
        "/cli/compute/jobs/logs",
        "/cli/compute/jobs/receipt",
        "/cli/compute/jobs/artifact",
        "/cli/compute/jobs/cancel",
        "/cli/compute/jobs/retry",
        "/cli/compute/grants",
        "/cli/compute/local/status",
        // Node sharing (0.46.3): a node's queue, redacted per §7.
        "/cli/compute/queue",
        // Node admin (prd-k2-node-admin-v1 §13.1): propose / list own /
        // withdraw own, policy show/set (widening becomes a held request),
        // health, update/rollback. The handler needs the Agent switch
        // "Allow this agent to operate compute nodes". approve, deny,
        // alerts and approval-key are NOT here: owner/admin humans only.
        "/cli/compute/admin/propose",
        "/cli/compute/admin/requests",
        "/cli/compute/admin/withdraw",
        "/cli/compute/nodes/health",
        "/cli/compute/nodes/policy",
        "/cli/compute/nodes/policy/set",
        "/cli/compute/nodes/update",
        "/cli/compute/nodes/rollback",
        "/cli/compute/nodes/wall",
        // App runtime stage 1 (prd-k2-app-runtime-v1 §5.11.1): the handler
        // scopes a passport to its own workspace's Apps and folders and
        // needs "Allow this agent to manage Apps" for every change.
        // `/cli/app/box` is the read-only doctor. `/cli/app/box/prepare`
        // is here ONLY for `--dry-run` (a read: what prepare would change);
        // the handler refuses every passport without `dryRun` (owner_only,
        // `box_prep::prepare_route`). `/cli/app/engine/install` and
        // `/cli/app/box/podman/uninstall` are NOT here (the owner's).
        // Sew on K2 (S29 K12, K19): the server on this box. Reads for any
        // passport; every mutation needs "Manage Sew" (`chat_manage`) on
        // the passport's workspace (the handler checks). Exact paths.
        "/cli/sew/status",
        "/cli/sew/preflight",
        "/cli/sew/backups",
        "/cli/sew/job",
        "/cli/sew/server/enable",
        "/cli/sew/server/disable",
        "/cli/sew/server/uninstall",
        "/cli/sew/server/upgrade",
        "/cli/sew/server/rollback",
        "/cli/sew/server/backup",
        "/cli/sew/server/restore",
        "/cli/sew/server/doctor",
        "/cli/sew/server/rotate-master",
        // Voice (K13) and standalone users (K14): Manage Sew in the handler.
        "/cli/sew/server/config",
        "/cli/sew/media/preflight",
        "/cli/sew/media/install",
        "/cli/sew/media/url",
        "/cli/sew/media/renew",
        "/cli/sew/media/probe",
        "/cli/sew/media/uninstall",
        "/cli/sew/meet/bind",
        "/cli/sew/meet/unbind",
        "/cli/sew/users",
        "/cli/sew/users/owner",
        "/cli/sew/users/invite",
        "/cli/sew/users/reset",
        "/cli/sew/users/revoke",
        "/cli/sew/users/reactivate",
        "/cli/sew/users/role",
        "/cli/app/box",
        "/cli/app/box/prepare",
        "/cli/app/build",
        // 0.46.3: data in, and the label from its own legacy service.
        "/cli/app/data/put",
        // 0.46.4: read back, list, remove and rename what the App keeps.
        "/cli/app/data/ls",
        "/cli/app/data/get",
        "/cli/app/data/rm",
        "/cli/app/data/mv",
        "/cli/app/label/take",
        "/cli/app/label/return",
        // 0.46.4: extra subdomains of its own workspace on its App.
        "/cli/app/domains",
        "/cli/app/domain/add",
        "/cli/app/domain/rm",
        // 0.46.4: the move's steps; one owner request for the owner's.
        "/cli/app/move-plan",
        "/cli/app/move-plan/request",
        // 0.46.4: test guests of its own Apps. The list needs switch 1; add
        // and rm need "Let this agent manage its Apps' users" (add without
        // it is one held request with a card for the owner; rm also with
        // switch 1, it narrows), Rosson 2026-10-26.
        "/cli/app/test-guest/add",
        "/cli/app/test-guest/rm",
        // Its own test guest's password: with the users switch, or once
        // after the owner approved its held request (the handler).
        "/cli/app/test-guest/password",
        "/cli/app/test-guests",
        "/cli/app/config",
        "/cli/app/deploy",
        "/cli/app/deployment",
        "/cli/app/errors",
        // `on` refuses passports (owner_only, with the command for their
        // human); `off` is a tightening under the manage switch.
        "/cli/app/exception",
        "/cli/app/exec",
        "/cli/app/history",
        "/cli/app/inspect",
        "/cli/app/install",
        "/cli/app/link",
        "/cli/app/link/rotate",
        "/cli/app/list",
        "/cli/app/logs",
        // 0.46.3 box usage (the view: no folder paths for passports), its
        // rescan, and settings (listed so a passport gets the teaching
        // owner_only from the handler, like box/prepare).
        "/cli/app/resources",
        // 0.46.4: clean (its own Apps' only; a real run needs switch 1)
        // and the trends.
        "/cli/app/resources/clean",
        "/cli/app/resources/rescan",
        "/cli/app/resources/settings",
        "/cli/app/resources/trends",
        "/cli/app/restart",
        "/cli/app/rollback",
        "/cli/app/shell",
        "/cli/app/start",
        "/cli/app/stop",
        "/cli/app/uninstall",
        "/cli/app/validate",
        // 0.46.0 grace: warnings, clean mode (on narrows; off needs switch
        // 2, else held), App keys (a mint applies at once with switch 2,
        // else held for the owner; revoke narrows).
        "/cli/app/warnings",
        "/cli/app/clean-mode",
        "/cli/app/key/mint",
        "/cli/app/keys",
        "/cli/app/key/revoke",
        // Egress (S15): read, add (applies at once with switch 2, else
        // held; `broad` mode is always the person's), narrow.
        "/cli/app/egress",
        "/cli/app/egress/add",
        "/cli/app/egress/log",
        "/cli/app/egress/mode",
        "/cli/app/egress/rm",
        // Box services (0.46.0): read, request (applies at once with switch
        // 2, else held for the owner), revoke (narrowing, at once).
        "/cli/app/box-service",
        "/cli/app/box-service/request",
        "/cli/app/box-service/revoke",
        // K-M3 (0.46.3): the seal key's status; rotate needs switch 2
        // (the handler refuses an agent without it).
        "/cli/app/seal",
        "/cli/app/seal/rotate",
        // Held requests: list own, withdraw own through deny.
        // `/cli/app/approve` is NOT here: the owner's, in person, with a
        // vault confirm.
        "/cli/app/approvals",
        "/cli/app/deny",
        // 0.46.5 App handover (`k2 app handover`): A offers its own App
        // (POST /cli/app/handover, switch 1), B accepts or declines, A
        // cancels; plan, status, brief. Approving the card is the owner's
        // (`/cli/app/approve` stays off this list).
        "/cli/app/handover",
        "/cli/app/handover/plan",
        "/cli/app/handover/status",
        "/cli/app/handover/brief",
        "/cli/app/handover/accept",
        "/cli/app/handover/decline",
        "/cli/app/handover/cancel",
        "/cli/app/handover/rekey",
        "/cli/app/handover/finish",
        // Clone proposals (prd-clone-carries-threads-and-vault-v1 CL21):
        // an agent proposes a clone of its own workspace, lists and
        // withdraws its own. `/cli/clone/*` stays DENY (agents never clone)
        // and approve/deny/finish are people only (not here).
        "/cli/clone-proposal/new",
        "/cli/clone-proposal/list",
        "/cli/clone-proposal/withdraw",
        // The 0.46.1 door (S19, AR10): read the audit and quotas, lower a
        // quota (a raise applies with switch 2, else held), suspend
        // (narrows), list and revoke remote passes. Resume (0.46.4): only a
        // suspension the agent made itself, with switch 2 (the handler);
        // the owner's suspension stays the owner's.
        // `/cli/app/remote-pass/rotate` is NOT here: the owner's.
        "/cli/app/audit",
        "/cli/app/quota",
        "/cli/app/quota/set",
        "/cli/app/suspend",
        "/cli/app/resume",
        "/cli/app/remote-pass",
        "/cli/app/remote-pass/revoke",
        // RS-A15 (0.46.1): an agent reads its own App's grants, revokes
        // one (narrows), and sets that App's guests' rooms within the grant
        // (switch 1). 0.46.4 (audit-app-agent-access-0464): with switch 2 an
        // agent gives its own App the guests-rooms grant in its own room at
        // once; otherwise, and for copy-host always (it copies the box's
        // people), the call is one held request with a card for the owner.
        // The handlers check the switches and the scope.
        "/cli/app/grants",
        "/cli/app/grant",
        "/cli/app/grant/copy-host",
        "/cli/app/grant/revoke",
        "/cli/app/guest/rooms",
        // prd-app-caps-0464-v1 AC4 (0.46.4): its own App's ceiling, and its
        // guests under the grant (reads).
        "/cli/app/guest/ceiling",
        "/cli/app/guest/list",
        // Room-scope R9/R16 (0.46.1): read who holds a grant on the agent's
        // own room, and its own Apps' self and usage. `/cli/app/room-names`
        // is NOT here: hiding guests' names is the owner's.
        "/cli/app/members",
        "/cli/app/self",
        "/cli/app/usage",
        // prd-custom-app-push-v1 (audit 0.46.4 §4.5): the App's own agent's
        // push controls, so a sealed cell matches TCP. The handler admits
        // switch 1 (status, log, turning push off) and switch 2 (send,
        // test, replies and previews). Keys, check and devices stay the
        // owner's and Admins' (not here).
        "/cli/app/push",
        "/cli/app/push/set",
        "/cli/app/push/send",
        "/cli/app/push/test",
        "/cli/app/push/log",
        "/cli/sidecar/new",
        "/cli/sidecar/list",
        "/cli/sidecar/stop",
        // Root helper status (read-only): agents see whether the helper is
        // there and what to ask their human for. update/rollback are NOT
        // here (owner/admin steps).
        "/cli/root-helper/status",
        // `k2 workspace swap-canonical`: the handler classifies the
        // passport like the sidecar routes. An agent may swap only its
        // own workspace's pinned chat, stamped as itself (not the owner);
        // a shell tab passport is a human and may swap any workspace.
        "/cli/workspace/swap-canonical",
        // Skin roster GET + manage mutations (`k2 skin` / `k2 skin-token`).
        // Exact paths only — never prefix `/cli/skin/` (front-door / hydra
        // stay owner even when the Agent-tab toggle is ON). POST mutate is
        // further gated by `agents_can_manage_skin_for_path` in the
        // dispatcher. `/cli/agents-manage-skin` is owner-only (not here).
        "/cli/skin/users",
        "/cli/skin/users/remove",
        "/cli/skin/users/password",
        "/cli/skin/users/unlock",
        "/cli/skin/users/email",
        // 0.46.4: an agent with "Let this agent manage its Apps' users"
        // names its own Apps' users (the door and `agent_scoped`).
        "/cli/skin/users/full-name",
        "/cli/skin/users/rooms",
        "/cli/skin/roles",
        "/cli/skin/roles/update",
        "/cli/skin/roles/remove",
        "/cli/skin/roles/assign",
        "/cli/skin/roles/unassign",
        "/cli/skin/roles/room",
        "/cli/skin/roles/paths",
        // app-mobile-access-v1 (0.46.3): the suggested role templates (read,
        // and make a role from one: gated like the role writes above), and
        // listing an App for discovery (the handler admits only the App's
        // own agent with switch 2).
        "/cli/skin/roles/suggest",
        "/cli/skin/roles/from-template",
        "/cli/skin/access/listed",
        // agent-app-users (0.46.4): an agent invites people to its own
        // workspace's Apps (the manage-Apps gate + the handler's checks), and
        // an App's own agent sets its invite settings (switch 2, handler).
        // `/cli/skin/users/reset-link` is the owner's alone: never here.
        "/cli/skin/users/invite",
        "/cli/skin/access/invites",
        // `/cli/skin/grants`, `/cli/skin/grants/delete`, `/cli/skin/grants/enabled`,
        // `/cli/skin/grants/host`, and `/cli/skin/templates*` stay off this list
        // (owner-only). Do not add `/cli/people`.
        "/cli/skin-tokens",
        "/cli/skin-tokens/revoke",
        "/cli/skin-tokens/rooms",
        // PR1 federation dual-auth under passports: agents may list peers,
        // pull a paired peer's roster, and send. pair/confirm/outbox/pubkey
        // stay owner-or-admin only (not on this allowlist). send forces
        // from_workspace from HookPrincipal (Wave 0 PR-C).
        "/cli/federation/peers",
        "/cli/federation/peer-roster",
        "/cli/federation/peer-rosters",
        "/cli/federation/send",
        // Sidecar identity: `k2 whoami` (NOT /cli/auth/whoami — that
        // `role` is connect-user).
        "/cli/whoami",
        "/cli/chatter",
        "/cli/chatterlog",
        // Custom domains / certs (prd-custom-domains A9 + name-add fold).
        // Exact paths only — never prefix `/cli/domains/` or `/cli/certs/`
        // (apex attach/remove + PEM upload stay owner/admin). Hostname
        // add/remove is dns_manage (handler-gated), same as cert issue.
        "/cli/domains",
        "/cli/domains/names",
        "/cli/domains/names/remove",
        "/cli/certs",
        "/cli/certs/issue",
        "/cli/certs/renew",
    ];
    const ALLOW_PREFIXES: &[&str] = &[
        "/cli/inbox/",
        // Garden v3 data sources (contract-sources-v1 §4, GV52): every
        // `/cli/sources/*` path is served on the cell socket for the bound
        // session; handlers refuse person steps with 403 person_only.
        "/cli/sources/",
        // Garden data (contract-garden-data-v1 §4.7): every
        // `/cli/zen/data/*` path is served on the cell socket for the bound
        // session; grants apply per call, person routes answer 403
        // person_only. `/cli/zen/call` (the widget door) is NOT here.
        "/cli/zen/data/",
        // Wave 0: agent mail verbs (messages/read/send/draft/inboxes/…).
        // Owner surfaces are DENY_PREFIXES above. Identity is forced from
        // HookPrincipal via stamp_principal + resolve_caller_workspace —
        // client project= is never self-identity for these paths.
        "/cli/mail/",
        "/cli/db/",
        "/cli/store/",
        // DNS K1: agent DNS verbs (access/zones list/records/verify).
        // Zone create/delete are DENY_PREFIXES above. Identity is forced
        // from HookPrincipal; handlers also gate on dns_manage_allowed_for_path.
        "/cli/dns/",
        // Named heartbeat schedule CRUD + fire/status (agents own their
        // workspace schedules). OS tick install + fleet list are DENY above.
        // Identity forced from HookPrincipal → only the caller's workspace.
        "/cli/heartbeat/",
        // Published services (prd-k2-publish-hosted-services-v1). NOT
        // under `/cli/tunnel/` which is owner-only (DENY_PREFIXES).
        "/cli/publish/",
        // Overlay threads (prd-overlay-threads-v1 S1). `/cli/chatterlog`
        // is exact on ALLOW_EXACT so it does not ride a `/cli/chatter`
        // prefix (chatterlog starts with chatter).
        "/cli/thread",
        "/cli/chatter/",
        "/cli/chatterlog/",
        // K2 projects (`k2 project`, 0.45.6): reads, the project chat and
        // the mutations. `project_group_routes::dispatch_post_as`
        // classifies the passport and refuses an agent's mutations when
        // its workspace's "Manage K2 projects" switch is off. The switch
        // itself (`/cli/agent-access/set`) is NOT an agent verb.
        "/cli/project-group/",
        // Sandbox P1 (Finding-1 follow-on): `/cli/review-checklist/` was
        // DROPPED from the scoped allowlist. Its handlers take the raw `body`
        // (not the params map) and so are NOT reached by the principal-pin in
        // `cell_server::stamp_principal` — a sealed cell could drive them at
        // another workspace. With the prefix removed these verbs fall back to
        // TCP+owner (they're renderer/TCP-driven, never over the cell UDS).
        // Re-admit in P2 with a body-restamp.
        // `memory` stays GATED: present as a recognized verb namespace but
        // not auto-allowed for writes (memory.write is owner/owner-gated).
        // Reads can be added here once the route exists.
    ];
    // `/cli/heartbeat-log` is a sibling path (no trailing slash under /heartbeat/).
    ALLOW_EXACT.contains(&path)
        || path == "/cli/heartbeat-log"
        || ALLOW_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// The `require_hook` guard: a presented bearer authorizes `path` iff the
/// path is an allowed agent verb AND the token validates against the
/// process registry. Returns the resolved [`ValidatedHook`] (so the caller
/// can match `paneId` + attribute the principal) or `None` (→ 403).
///
/// The per-cell UDS server ([`crate::cell_server`]) uses the returned
/// `principal` to STAMP the sender identity (`from` / `project_id` /
/// awareness `from.*`) server-side: the request body is NEVER trusted for
/// WHO is sending. The body's recipient/routing args (`workspace`/`target`)
/// stay client-supplied — they are WHO you address, not WHO you are.
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

/// The scoped arm shared by `/hook/complete` and `/hook/event` (TCP): a
/// per-session token authorizes a hook ONLY for its own pane. Same token,
/// another pane → `false` (→ 403). Inert when scoped hooks are opted out.
pub fn scoped_hook_authorizes(presented: &str, path: &str, pane: &str) -> bool {
    if !scoped_hooks_enabled() || presented.is_empty() || pane.is_empty() {
        return false;
    }
    match require_hook(presented, path) {
        Some(v) => v.pane_id == pane,
        None => false,
    }
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
/// PAT. Phase 1 wires this into the spawn env (via [`cell_env_pairs`]).
// COMPAT-58: remove in Phase 3 (owner-token deprecation) once the scoped
// token is the ONLY hook credential.
pub fn mint_session_token(
    session_id: &SessionId,
    pane_id: &str,
    principal: HookPrincipal,
    cred_mode: CredMode,
    provider: Provider,
) -> String {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    let token = g.mint(session_id, pane_id, principal, cred_mode, provider);
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

/// The workspace (`projects.id`) a live passport's session belongs to, by
/// the daemon session id (the vault's `session:<id>` consumer). `None` when
/// no live passport names it.
pub fn workspace_for_session(session_id: &str) -> Option<String> {
    let g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.records.get(session_id.trim()).map(|r| r.principal.workspace_uuid.clone())
}

/// Revoke one session's token via the process registry + persist. Phase 1
/// calls this on cell teardown/kill (the child-exit observer) so a dead
/// cell's token 403s immediately, no daemon restart.
// COMPAT-58: remove in Phase 3 (owner-token deprecation).
pub fn revoke_session(session_id: &SessionId) {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.revoke(session_id);
    save_to_disk(&g);
}

/// Selector prefix of a `k2 job` run's passport (`job-<run id>`).
pub const JOB_SID_PREFIX: &str = "job-";

/// Mint the passport of one `k2 job` run (prd-vault-gaps-v1 item 5): an
/// agent-level principal of the job's workspace, selector `job-<run id>`,
/// `agent_address` `job:<name>`. It dies at run end
/// ([`revoke_job_passport`]) and at the next boot
/// ([`revoke_job_passports_at_boot`]). Minted whatever `K2_HOOK_SCOPED`
/// says: the alternative for an unattended run is the owner token.
pub fn mint_job_passport(run_sid: &str, workspace_id: &str, job_name: &str) -> String {
    assert!(run_sid.starts_with(JOB_SID_PREFIX) && !run_sid.contains('.'), "job passport selector");
    let principal = HookPrincipal { workspace_uuid: workspace_id.to_string(), agent_address: format!("job:{job_name}") };
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    let token = g.mint_sid(run_sid, run_sid, principal, CredMode::Subscription, Provider::Anthropic);
    save_to_disk(&g);
    token
}

/// A run ended: its passport stops working at once. A run id is never
/// reused, so the record just goes (no epoch is kept: a job every minute
/// would otherwise grow `hook-sessions.json` forever).
pub fn revoke_job_passport(run_sid: &str) {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    if g.drop_job_sid(run_sid) {
        save_to_disk(&g);
    }
}

/// Boot: no job run survives a daemon restart, so no job passport does.
pub fn revoke_job_passports_at_boot() -> usize {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    let n = g.revoke_job_selectors();
    if n > 0 {
        save_to_disk(&g);
    }
    n
}

impl SessionTokenRegistry {
    /// Revoke every `job-…` record (see [`revoke_job_passports_at_boot`]).
    fn revoke_job_selectors(&mut self) -> usize {
        let sids: Vec<String> = self.records.keys().filter(|s| s.starts_with(JOB_SID_PREFIX)).cloned().collect();
        for s in &sids {
            self.drop_job_sid(s);
        }
        sids.len()
    }

    /// Remove a job run's record (and any epoch). `false` = nothing there.
    fn drop_job_sid(&mut self, sid: &str) -> bool {
        if !sid.starts_with(JOB_SID_PREFIX) {
            return false;
        }
        let had = self.records.remove(sid).is_some();
        self.session_epochs.remove(sid);
        had
    }
}

/// Daemon-wide kill switch via the process registry + persist.
pub fn revoke_all() {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.revoke_all();
    save_to_disk(&g);
}

// ─────────────────────────────────────────────────────────────────────
// Phase 1 — spawn-time activation (opt-in, flag-gated)
// ─────────────────────────────────────────────────────────────────────

/// Build the per-cell env pairs injected into a spawned PTY for a given
/// (already-minted) scoped token + socket path. **PURE** — no flag read,
/// no disk, no minting — so the env SHAPE is unit-testable in isolation.
///
/// The scoped token replaces the owner token behind the SAME
/// `K2_HOOK_TOKEN` key (the load-bearing security change: the value in the
/// cell is now per-cell, not the daemon owner credential), and the per-cell
/// socket path rides `K2_HOOK_SOCK` so `notify.sh` / the `k2` CLI can prefer
/// the UDS. Both the canonical `K2_*` and the legacy `K2SO_*` aliases are
/// emitted (0.40 rebrand dual-emit; `COMPAT-58` for the eventual cleanup).
///
/// **B3a credential staging:** when `cred_mode == ApiKey` AND a non-blank
/// per-workspace `api_key` is supplied, the provider's key env var
/// (`provider.key_env_var()`, e.g. `ANTHROPIC_API_KEY`) is appended so the
/// in-cell Claude Code authenticates without interactive login. The key is
/// PER-WORKSPACE (right key → right cell) and is NEVER logged. Absent key,
/// blank key, or `Subscription` mode (deferred stub) → NOTHING is staged, so
/// no empty `ANTHROPIC_API_KEY=` ever appears (default-OFF parity preserved).
pub fn scoped_cell_env_for_token(
    sock_path: &str,
    scoped_token: &str,
    pane_id: &str,
    hook_port: u16,
    cred_mode: CredMode,
    provider: Provider,
    api_key: Option<&str>,
) -> Vec<(String, String)> {
    let mut pairs = vec![
        // Scoped per-cell token (NOT the owner token) behind the same key
        // the CLI + notify.sh already read.
        ("K2_HOOK_TOKEN".to_string(), scoped_token.to_string()),
        ("K2SO_HOOK_TOKEN".to_string(), scoped_token.to_string()),
        // Per-cell UDS path — the preferred (Bearer) hook channel.
        ("K2_HOOK_SOCK".to_string(), sock_path.to_string()),
        ("K2SO_HOOK_SOCK".to_string(), sock_path.to_string()),
        // Pane/tab identity the hook script echoes back as `paneId` — the
        // dual-accept arm + the per-cell server match it against the token.
        ("K2_PANE_ID".to_string(), pane_id.to_string()),
        ("K2SO_PANE_ID".to_string(), pane_id.to_string()),
        ("K2_TAB_ID".to_string(), pane_id.to_string()),
        ("K2SO_TAB_ID".to_string(), pane_id.to_string()),
    ];
    // BLOCKER 3 — the in-guest `k2` CLI's connection gate (`cli/k2` ~:130)
    // refuses to dispatch ANY verb unless BOTH `K2_PORT` and a hook token are
    // present in the env. The scoped UDS verbs actually transport over
    // `K2_HOOK_SOCK` (curl `--unix-socket`), NOT TCP — so `K2_PORT` here is a
    // pure PRESENCE-gate, never dialed from inside the guest (which has no
    // host TCP reach). We stage the daemon's real loopback port so the gate
    // passes; the verb then rides the cell socket. Mirrors the dual-name
    // (`K2_PORT` + `K2SO_PORT`) pattern the normal-session terminal env uses
    // (`alacritty_backend.rs`). Omitted when the hook server hasn't bound yet
    // (port 0), exactly like the normal path's `if hook_port > 0` guard — the
    // socket-bound hook verbs still work, only the TCP fallback is unavailable
    // (and the guest can't reach TCP anyway).
    if hook_port > 0 {
        pairs.push(("K2_PORT".to_string(), hook_port.to_string()));
        pairs.push(("K2SO_PORT".to_string(), hook_port.to_string()));
    }
    // B3a — stage the PER-WORKSPACE model credential so the in-cell Claude
    // Code skips interactive auth. Only in ApiKey mode (Subscription is a
    // deferred no-op stub) and only when a non-blank key was resolved
    // host-side — never stage an empty `<KEY_ENV>=`. The key is NEVER logged.
    if matches!(cred_mode, CredMode::ApiKey) {
        if let Some(k) = api_key {
            let k = k.trim();
            if !k.is_empty() {
                pairs.push((provider.key_env_var().to_string(), k.to_string()));
            }
        }
    }
    pairs
}

/// Phase 1 spawn entry point: when scoped hooks are ON (the default), mint a
/// per-cell scoped token (process registry + disk) and return the env pairs
/// to inject into the child PTY. **Returns `None` when the flag is
/// explicitly OFF** so a fleet opt-out injects no scoped env (and still must
/// not inject the owner — see [`strip_owner_hook_tokens`]).
///
/// The minted token is bound to `(session_id, pane_id, principal)`; the
/// socket path is the deterministic [`crate::cell_uds::cell_socket_path`].
///
/// **B3a:** `cred_mode` + `provider` are resolved HOST-SIDE at the spawn door
/// (`v2_spawn`) and stamped into the token claim here; the optional
/// `api_key` (a non-blank PER-WORKSPACE key, microVM cells only) is staged
/// into the returned env. The key is forwarded ONLY into the (transient) cell
/// env — it is NEVER persisted in the token record, and NEVER logged.
pub fn cell_env_pairs(
    session_id: &SessionId,
    pane_id: &str,
    principal: HookPrincipal,
    cred_mode: CredMode,
    provider: Provider,
    api_key: Option<&str>,
) -> Option<Vec<(String, String)>> {
    if !scoped_hooks_enabled() {
        return None;
    }
    let token = mint_session_token(session_id, pane_id, principal, cred_mode, provider);
    let sock = crate::cell_uds::cell_socket_path(session_id);
    // The daemon's actual loopback hook port — same source the normal-session
    // terminal env reads (`alacritty_backend.rs`). 0 before the hook server
    // binds; `scoped_cell_env_for_token` then omits `K2_PORT` (see there).
    let hook_port = k2_core::hook_config::get_port();
    Some(scoped_cell_env_for_token(
        &sock.to_string_lossy(),
        &token,
        pane_id,
        hook_port,
        cred_mode,
        provider,
        api_key,
    ))
}

/// The pane/tab identity pairs (`K2_PANE_ID`, `K2SO_PANE_ID`, `K2_TAB_ID`,
/// `K2SO_TAB_ID`), all = the v2 session id.
pub fn pane_id_env_pairs(pane_id: &str) -> Vec<(String, String)> {
    ["K2_PANE_ID", "K2SO_PANE_ID", "K2_TAB_ID", "K2SO_TAB_ID"]
        .iter()
        .map(|k| (k.to_string(), pane_id.to_string()))
        .collect()
}

/// Env keys that must never carry the daemon owner token into an agent
/// child. Dual-emitted (K2 + K2SO) for the 0.40 rebrand.
const HOOK_TOKEN_ENV_KEYS: &[&str] = &["K2_HOOK_TOKEN", "K2SO_HOOK_TOKEN"];

/// Remove any `K2_HOOK_TOKEN` / `K2SO_HOOK_TOKEN` entries whose value equals
/// the daemon owner token. **Always safe to call** on agent/PTY spawn env —
/// the load-bearing PR-A invariant: agent children never receive the owner
/// credential in env (even when scoped mint is opted out).
///
/// Does not touch non-owner values (a pre-minted scoped passport is kept).
pub fn strip_owner_hook_tokens(
    env: &mut std::collections::HashMap<String, String>,
    owner_token: &str,
) {
    if owner_token.is_empty() {
        return;
    }
    for key in HOOK_TOKEN_ENV_KEYS {
        if env.get(*key).map(|v| v == owner_token).unwrap_or(false) {
            env.remove(*key);
        }
    }
}

/// Merge scoped passport pairs into a spawn env, **replacing** any prior
/// hook-token values. Call after [`cell_env_pairs`] returns `Some`. The
/// scoped token rides the same keys the CLI / notify.sh already read —
/// that is the security change (value is per-cell, not owner).
pub fn apply_scoped_passport(
    env: &mut std::collections::HashMap<String, String>,
    pairs: Vec<(String, String)>,
) {
    for (k, v) in pairs {
        env.insert(k, v);
    }
}

/// Full PR-A spawn-env preparation for an agent cell:
/// 1. Mint + apply scoped passport when the flag is ON (default).
/// 2. Always strip any residual owner-token values from hook keys.
///
/// Returns `true` when a scoped passport was minted (caller should bind
/// the per-cell UDS after the PTY is open).
pub fn prepare_agent_spawn_env(
    env: &mut std::collections::HashMap<String, String>,
    session_id: &SessionId,
    pane_id: &str,
    principal: HookPrincipal,
    cred_mode: CredMode,
    provider: Provider,
    api_key: Option<&str>,
    owner_token: &str,
) -> bool {
    // A8 (prd-daemon-activity-and-thread-working-v1): the pane id is how
    // `notify.sh` names its session, so it rides every v2 spawn whatever
    // `K2_HOOK_SCOPED` says. Only the token and socket are flag-gated.
    for (k, v) in pane_id_env_pairs(pane_id) {
        env.insert(k, v);
    }
    let minted = if let Some(pairs) =
        cell_env_pairs(session_id, pane_id, principal, cred_mode, provider, api_key)
    {
        apply_scoped_passport(env, pairs);
        true
    } else {
        false
    };
    // Defense in depth: even if a caller pre-seeded the owner into env
    // (legacy alacritty path, preset leak, inherited process env), strip it.
    strip_owner_hook_tokens(env, owner_token);
    minted
}

/// Bind the per-cell UDS and start the cell server when scoped hooks are ON.
/// Call AFTER the PTY is open (the socket path was already injected into the
/// child env at mint time). Bind failure is non-fatal — the cell degrades to
/// the TCP dual-accept / disk-owner CLI fallback (non-stranding).
///
/// `per_session_uid` is `Some` only for microVM cells (socket chown + peer-cred
/// belt); bare-PTY agent sessions pass `None`.
#[cfg(unix)]
pub fn activate_cell_uds(session_id: SessionId, per_session_uid: Option<u32>) {
    if !scoped_hooks_enabled() {
        return;
    }
    match crate::cell_uds::bind_cell_socket(&session_id) {
        Ok(listener) => {
            if let Some(cell_uid) = per_session_uid {
                if let Err(e) =
                    crate::cell_uds::set_cell_socket_owner(&session_id, cell_uid)
                {
                    log_debug!(
                        "[hook-scoped] WARN chown cell sock to uid {cell_uid} failed for session={session_id}: {e}; socket stays daemon-only"
                    );
                }
            }
            crate::cell_server::serve_cell(session_id, listener, per_session_uid);
            log_debug!(
                "[hook-scoped] bound + serving per-cell UDS for session={session_id}"
            );
        }
        Err(e) => {
            log_debug!(
                "[hook-scoped] WARN bind cell sock failed for session={session_id}: {e}; degrading to TCP dual-accept"
            );
        }
    }
}

/// [`activate_cell_uds`] for a spawned bare-PTY session: a k2-ptyd session's
/// listener is held by ptyd so it survives a daemon restart (PRD
/// prd-k2-ptyd-v1 P44; see `cell_uds::bind_cell_socket_for`).
#[cfg(unix)]
pub fn activate_cell_uds_for(session: &k2_core::terminal::DaemonPtySession) {
    if !scoped_hooks_enabled() {
        return;
    }
    let session_id = session.session_id;
    match crate::cell_uds::bind_cell_socket_for(session) {
        Ok(listener) => {
            crate::cell_server::serve_cell(session_id, listener, None);
            log_debug!("[hook-scoped] bound + serving per-cell UDS for session={session_id}");
        }
        Err(e) => {
            log_debug!(
                "[hook-scoped] WARN bind cell sock failed for session={session_id}: {e}; degrading to TCP dual-accept"
            );
        }
    }
}

#[cfg(not(unix))]
pub fn activate_cell_uds_for(_session: &k2_core::terminal::DaemonPtySession) {}

#[cfg(not(unix))]
pub fn activate_cell_uds(_session_id: SessionId, _per_session_uid: Option<u32>) {
    // UDS is unix-only; scoped tokens still mint into env for TCP dual-accept
    // of /hook/complete when the flag is ON.
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
// Tests — pure registry logic on a *local* SessionTokenRegistry::new()
// never touches disk. Tests that call the process-wide mint/validate/
// prepare helpers (mint_session_token → save_to_disk → real ~/.k2) MUST
// wrap in `crate::test_support::with_temp_home` so dirs::home_dir()
// redirects store_path() to a throwaway dir (fail-loud per
// feedback_test_discipline; prevents live hook-sessions.json pollution).
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;

    fn principal() -> HookPrincipal {
        HookPrincipal {
            workspace_uuid: "ws-uuid-1".to_string(),
            agent_address: "agent-a".to_string(),
        }
    }

    /// `k2 job` (item 5): a run's passport validates while the run lives,
    /// then is gone without leaving an epoch behind; boot drops every
    /// job record and leaves session records alone.
    #[test]
    fn job_passports_live_for_the_run_only() {
        let mut reg = SessionTokenRegistry::new();
        let jp = HookPrincipal { workspace_uuid: "ws-1".into(), agent_address: "job:collect".into() };
        let tok = reg.mint_sid("job-r1", "job-r1", jp.clone(), CredMode::Subscription, Provider::Anthropic);
        let v = reg.validate_hook(&tok).expect("valid during the run");
        assert_eq!(v.session_id, "job-r1");
        assert_eq!(v.principal, jp);
        let sid = SessionId::new();
        let stok = reg.mint(&sid, "pane", principal(), CredMode::ApiKey, Provider::Anthropic);
        let tok2 = reg.mint_sid("job-r2", "job-r2", jp, CredMode::Subscription, Provider::Anthropic);
        assert!(reg.drop_job_sid("job-r1"));
        assert!(reg.validate_hook(&tok).is_none(), "gone at run end");
        assert!(!reg.session_epochs.contains_key("job-r1"), "no epoch kept for a job run");
        assert!(!reg.drop_job_sid(&sid.to_string()), "only job selectors drop this way");
        assert_eq!(reg.revoke_job_selectors(), 1);
        assert!(reg.validate_hook(&tok2).is_none(), "boot revokes job passports");
        assert!(reg.validate_hook(&stok).is_some(), "session passports stay");
    }

    // ── env flag ─────────────────────────────────────────────────────

    #[test]
    fn scoped_hooks_flag_defaults_on() {
        // Phase 1 / PR-A: default ON so new agent sessions get a passport.
        // Opt-out is explicit (`0`/`false`/`off`). The guards hold the ONE
        // shared env lock and restore whatever was set (LIFO) so parallel
        // tests that pin the flag keep working.
        let _env = crate::test_support::lock_home();
        let _scoped = k2_core::test_env::EnvVar::remove("K2_HOOK_SCOPED");
        assert!(scoped_hooks_enabled(), "must default ON when unset");
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
        assert!(!scoped_hooks_enabled(), "explicit 0 must opt out");
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "false");
        assert!(!scoped_hooks_enabled(), "explicit false must opt out");
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "off");
        assert!(!scoped_hooks_enabled(), "explicit off must opt out");
        let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "1");
        assert!(scoped_hooks_enabled(), "explicit 1 must stay ON");
    }

    // ── mint + validate round trip ──────────────────────────────────

    #[test]
    fn mint_then_validate_accepts_correct_secret() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);

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
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
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
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
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
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
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
        let ta = reg.mint(&a, "pane-a", principal(), CredMode::ApiKey, Provider::Anthropic);
        let tb = reg.mint(&b, "pane-b", principal(), CredMode::ApiKey, Provider::Anthropic);
        assert!(reg.validate_hook(&ta).is_some());
        assert!(reg.validate_hook(&tb).is_some());

        reg.revoke_all();
        assert!(reg.validate_hook(&ta).is_none(), "global kill switch (a)");
        assert!(reg.validate_hook(&tb).is_none(), "global kill switch (b)");
    }

    // ── wall-clock stamp is not a kill switch ───────────────────────

    #[test]
    fn validate_ignores_past_expires_at_stamp() {
        // Live agent passports must not die on wall clock. Epoch revoke is
        // the kill switch; expires_at is informational only.
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
        reg.records
            .get_mut(&sid.to_string())
            .expect("record present")
            .expires_at = Utc::now() - Duration::days(30);
        assert!(
            reg.validate_hook(&token).is_some(),
            "past expires_at must NOT 403 a still-live passport",
        );
    }

    #[test]
    fn validate_accepts_months_old_passport() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
        let rec = reg
            .records
            .get_mut(&sid.to_string())
            .expect("record present");
        rec.created_at = Utc::now() - Duration::days(90);
        // leave expires_at far future (as mint would); still valid
        assert!(
            reg.validate_hook(&token).is_some(),
            "90-day-old passport must still validate for long-running agents",
        );
    }

    // ── capability allowlist / denylist (require_hook §3.3) ──────────

    /// Permission cards (vs-live C1): the family is an agent verb only while
    /// the global switch is on; off it is refused like any non-agent path.
    /// Pure twin only: the process-wide switch is never flipped here.
    #[test]
    fn permission_family_is_an_agent_verb_only_while_the_server_switch_is_on() {
        for p in [
            "/cli/permission/request",
            "/cli/permission/enabled",
            "/cli/permission/status",
            "/cli/permission/approve",
        ] {
            assert!(!is_agent_verb_when(p, false), "{p} admitted while off");
            assert!(is_agent_verb_when(p, true), "{p} not admitted while on");
        }
        // Never the bare word, and nothing else moves with the switch.
        for on in [false, true] {
            assert!(!is_agent_verb_when("/cli/permission", on));
            assert!(!is_agent_verb_when("/cli/agent-access/set", on));
            assert!(!is_agent_verb_when("/cli/settings/update", on));
            assert!(is_agent_verb_when("/cli/thread/answer", on));
        }
    }

    /// 0.46.5 (first-run review item 13): an agent reads `k2 git-tools`
    /// and may recheck; installing (Apple's dialog) stays the owner's.
    #[test]
    fn git_tools_status_and_recheck_are_agent_verbs_install_is_not() {
        assert!(is_agent_verb("/cli/git-tools"));
        assert!(is_agent_verb("/cli/git-tools/recheck"));
        assert!(!is_agent_verb("/cli/git-tools/install"));
        assert!(!is_agent_verb("/cli/git-tools/installx"));
    }

    #[test]
    fn is_agent_verb_allows_agent_routes() {
        assert!(is_agent_verb("/hook/complete"));
        assert!(is_agent_verb("/cli/workspace/msg"));
        assert!(is_agent_verb("/cli/thread"));
        assert!(is_agent_verb("/cli/thread/post"));
        assert!(is_agent_verb("/cli/thread/ask"));
        assert!(is_agent_verb("/cli/thread/secret"));
        assert!(is_agent_verb("/cli/thread/answer"));
        assert!(is_agent_verb("/cli/thread/void"));
        assert!(is_agent_verb("/cli/chatter"));
        assert!(is_agent_verb("/cli/chatterlog"));
        assert_eq!(crate::overlay_ws::OVERLAY_WS_PATH, "/cli/overlay/events");
        assert!(is_agent_verb("/cli/inbox/respond"));
        assert!(is_agent_verb("/cli/awareness/publish"));
        // Wave 0: agent mail verbs (owner mail surfaces stay denied).
        assert!(is_agent_verb("/cli/mail/messages"));
        assert!(is_agent_verb("/cli/mail/inboxes"));
        assert!(is_agent_verb("/cli/mail/send"));
        assert!(is_agent_verb("/cli/mail/draft"));
        // DNS K1: agent DNS prefixes.
        assert!(is_agent_verb("/cli/dns/access"));
        assert!(is_agent_verb("/cli/dns/zones"));
        assert!(is_agent_verb("/cli/dns/records"));
        assert!(is_agent_verb("/cli/dns/records/add"));
        assert!(is_agent_verb("/cli/dns/records/remove"));
        assert!(is_agent_verb("/cli/dns/verify"));
        // Custom domains: list + hostname add/remove + cert issue/renew
        // are agent verbs; apex attach/remove + PEM upload stay owner-only.
        assert!(is_agent_verb("/cli/domains"));
        assert!(is_agent_verb("/cli/certs"));
        assert!(is_agent_verb("/cli/certs/issue"));
        assert!(is_agent_verb("/cli/certs/renew"));
        assert!(
            !is_agent_verb("/cli/domains/remove"),
            "apex attach/remove is owner/admin — not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/domains/refresh"),
            "re-bind may auto-add a zone to the k2.dev account — owner/admin"
        );
        assert!(is_agent_verb("/cli/domains/names"));
        assert!(is_agent_verb("/cli/domains/names/remove"));
        assert!(
            !is_agent_verb("/cli/certs/upload"),
            "PEM upload is owner-only"
        );
        // Tickets badge orphans: the host-wide Tickets list is not an agent
        // verb (and no other /cli/feedback/ route is either).
        assert!(!is_agent_verb("/cli/feedback/list-all"));
        assert!(!is_agent_verb("/cli/feedback/list"));
        assert!(!is_agent_verb("/cli/feedback/waiting-count"));
        // C1 (0.40.45): connections list/add/remove (mutate toggle-gated).
        assert!(is_agent_verb("/cli/connections"));
        assert!(
            !is_agent_verb("/cli/users"),
            "GET /cli/users is owner/admin — not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/connections/users"),
            "do not invent /cli/connections/users — users=1 rides /cli/connections"
        );
        // Heartbeat schedule family (agent-owned workspace schedules).
        assert!(is_agent_verb("/cli/heartbeat/list"));
        assert!(is_agent_verb("/cli/heartbeat/add"));
        assert!(is_agent_verb("/cli/heartbeat/edit"));
        assert!(is_agent_verb("/cli/heartbeat/fire"));
        assert!(is_agent_verb("/cli/heartbeat/status"));
        assert!(is_agent_verb("/cli/heartbeat-log"));
        // OS install + fleet-wide list stay owner-only (teaching owner_only).
        assert!(!is_agent_verb("/cli/heartbeat/install-launchd"));
        assert!(!is_agent_verb("/cli/heartbeat/uninstall-launchd"));
        assert!(!is_agent_verb("/cli/heartbeat/apply-wake-scheduler"));
        assert!(!is_agent_verb("/cli/heartbeat/list-all"));
        assert!(!is_agent_verb("/cli/heartbeat/fires-list-all"));
        assert!(!is_agent_verb("/cli/heartbeat/set-show-sessions"));
        assert!(!is_agent_verb("/cli/heartbeat/active-projects"));
        // PR1: federation agent send surface under passports.
        assert!(is_agent_verb("/cli/federation/peers"));
        assert!(is_agent_verb("/cli/federation/peer-roster"));
        assert!(is_agent_verb("/cli/federation/peer-rosters"));
        assert!(is_agent_verb("/cli/federation/send"));
        assert!(
            is_agent_verb("/cli/whoami"),
            "scoped tokens must reach GET /cli/whoami"
        );
        assert!(
            is_agent_verb("/cli/publish/list"),
            "published services are agent verbs (not under /cli/tunnel/)"
        );
        assert!(is_agent_verb("/cli/publish/run"));
        assert!(
            !is_agent_verb("/cli/tunnel/publish/run"),
            "must not hang publish under owner-only /cli/tunnel/"
        );
        assert!(
            is_agent_verb("/cli/connect/proxy"),
            "agents publish their own nested labels end to end (0.45.7)"
        );
        assert!(!is_agent_verb("/cli/connect/other"), "exact path only");
        assert!(
            !is_agent_verb("/cli/auth/whoami"),
            "/cli/auth/whoami is connect-user identity — not an agent verb"
        );
        // Skin roster GET + manage mutations for workspace-agent passports.
        // Dispatcher still gates POST mutate on the Agent-tab column.
        // front-door / hydra / prefix / the toggle writer stay off.
        assert!(is_agent_verb("/cli/skin/users"));
        assert!(is_agent_verb("/cli/skin/users/remove"));
        assert!(is_agent_verb("/cli/skin/users/password"));
        assert!(is_agent_verb("/cli/skin/users/unlock"));
        assert!(
            !is_agent_verb("/cli/users/unlock"),
            "box login unlock is owner-tier, not an agent verb"
        );
        assert!(is_agent_verb("/cli/skin/users/email"));
        assert!(
            is_agent_verb("/cli/skin/users/full-name"),
            "0.46.4: an agent with \"Let this agent manage its Apps' users\" names its own Apps' users"
        );
        assert!(is_agent_verb("/cli/app/grant"));
        assert!(is_agent_verb("/cli/app/grant/copy-host"));
        assert!(is_agent_verb("/cli/app/resume"), "its own suspension, with switch 2 (the handler)");
        for p in ["/cli/app/push", "/cli/app/push/set", "/cli/app/push/send", "/cli/app/push/test", "/cli/app/push/log"] {
            assert!(is_agent_verb(p), "{p}: the cell socket matches TCP");
        }
        for p in ["/cli/app/push/credentials", "/cli/app/push/credentials/remove", "/cli/app/push/check", "/cli/app/push/devices", "/cli/app/push/devices/remove"] {
            assert!(!is_agent_verb(p), "{p}: the owner's and Admins'");
        }
        assert!(!is_agent_verb("/cli/app/remote-pass/rotate"));
        assert!(
            !is_agent_verb("/cli/skin/users/agent"),
            "an agent must never mark or unmark a principal as an agent"
        );
        assert!(is_agent_verb("/cli/skin/users/rooms"));
        assert!(is_agent_verb("/cli/skin/roles"));
        assert!(is_agent_verb("/cli/skin/roles/update"));
        assert!(is_agent_verb("/cli/skin/roles/remove"));
        assert!(is_agent_verb("/cli/skin/roles/assign"));
        assert!(is_agent_verb("/cli/skin/roles/unassign"));
        assert!(is_agent_verb("/cli/skin/roles/room"));
        assert!(
            !is_agent_verb("/cli/skin/grants"),
            "grants are owner-only, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/grants/delete"),
            "grant revoke is owner-only, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/grants/enabled"),
            "grant enabled is owner-only, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/grants/host"),
            "host login grant is owner-only, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/grants/copy-host"),
            "copying box logins onto an App is owner-only, not an agent verb (S24-11)"
        );
        assert!(
            !is_agent_verb("/cli/skin/templates"),
            "access templates are owner-only, not an agent verb"
        );
        assert!(!is_agent_verb("/cli/skin/templates/update"));
        assert!(!is_agent_verb("/cli/skin/templates/delete"));
        assert!(!is_agent_verb("/cli/skin/templates/lines"));
        assert!(!is_agent_verb("/cli/skin/templates/lines/update"));
        assert!(!is_agent_verb("/cli/skin/templates/lines/delete"));
        assert!(
            !is_agent_verb("/cli/skin/templates/apply"),
            "template apply is owner-only, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/people"),
            "do not add /cli/people as an agent verb"
        );
        assert!(is_agent_verb("/cli/skin-tokens"));
        assert!(is_agent_verb("/cli/skin-tokens/revoke"));
        assert!(is_agent_verb("/cli/skin-tokens/rooms"));
        assert!(
            !is_agent_verb("/cli/skin/front-door"),
            "must not allow-prefix /cli/skin/ (front-door is owner-only)"
        );
        assert!(
            !is_agent_verb("/cli/skin/hydra"),
            "hydra sidecar is owner-only"
        );
        assert!(
            !is_agent_verb("/cli/skin/"),
            "must not allow-prefix all of /cli/skin/"
        );
        assert!(
            !is_agent_verb("/cli/agents-manage-skin"),
            "toggle writer is owner-only, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/password/forgot"),
            "forgot is owner|platform, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/password/reset"),
            "reset is public, not an agent verb"
        );
        assert!(
            !is_agent_verb("/cli/skin/password/change"),
            "change is session k2skn_, not an agent verb"
        );
    }

    /// R1 (agent-app-users S1, T1.3): the admin logins (`/cli/users`, the
    /// bare path too, and every arm under it) are never an agent verb,
    /// with permission cards on or off.
    #[test]
    fn users_routes_are_never_agent_verbs() {
        const ARMS: &[&str] = &[
            "/cli/users",
            "/cli/users/add",
            "/cli/users/remove",
            "/cli/users/set-password",
            "/cli/users/unlock",
            "/cli/users/set-disabled",
            "/cli/users/set-role",
            "/cli/users/set-can-clone",
            "/cli/users/policy",
            "/cli/users/audit",
        ];
        for on in [false, true] {
            for p in ARMS {
                assert!(!is_agent_verb_when(p, on), "{p} must never be an agent verb (permission cards on={on})");
            }
        }
    }

    /// agent-app-users T4.4 / A22: the owner reset link is never an agent
    /// verb; inviting is (the handler scopes it to the agent's own Apps).
    #[test]
    fn reset_link_is_owner_only_invite_is_an_agent_verb() {
        for on in [false, true] {
            assert!(!is_agent_verb_when("/cli/skin/users/reset-link", on), "reset-link (permission cards on={on})");
            assert!(is_agent_verb_when("/cli/skin/users/invite", on), "invite (permission cards on={on})");
        }
    }

    #[test]
    fn is_agent_verb_denies_federation_owner_surfaces() {
        // pair/confirm stay owner-or-admin; outbox/pubkey stay owner-gated.
        // inbound is envelope-auth (not a scoped verb either).
        for p in [
            "/cli/federation/pair/request",
            "/cli/federation/pair/confirm",
            "/cli/federation/outbox",
            "/cli/federation/pubkey",
            "/cli/federation/inbound",
            "/cli/federation/roster",
        ] {
            assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
        }
    }

    #[test]
    fn is_agent_verb_denies_mail_owner_surfaces() {
        // M5 hostmail manage: DENY shrink so scoped hooks can pass
        // require_hook; extra gate ORs mail_manage_allowed_for_path.
        // C5b (C21) also opens access/doctor/approvals/config-set/import.
        for p in [
            "/cli/mail/server/enable",
            "/cli/mail/server/disable",
            "/cli/mail/domain/list",
            "/cli/mail/domain/show",
            "/cli/mail/domain/add",
            "/cli/mail/domain/check",
            "/cli/mail/access/grant",
            "/cli/mail/access/revoke",
            "/cli/mail/access/set-primary",
            "/cli/mail/access/set-level",
            "/cli/mail/access/set-manage",
            "/cli/mail/doctor",
            "/cli/mail/approvals/list",
            "/cli/mail/approvals/approve",
            "/cli/mail/approvals/deny",
            "/cli/mail/config/set",
            "/cli/mail/import",
            "/cli/mail/quota",
            "/cli/mail/catchall",
            "/cli/mail/alias",
            "/cli/mail/alias/remove",
            "/cli/mail/forward",
            "/cli/mail/forward/unset",
            "/cli/mail/ooo",
            "/cli/mail/ooo/unset",
            "/cli/mail/footer",
            "/cli/mail/footer/unset",
            "/cli/mail/cert/renew",
            "/cli/mail/server/rotate-admin",
            "/cli/mail/address/password",
            "/cli/mail/list",
            "/cli/mail/list/members",
            "/cli/mail/list/delete",
            "/cli/mail/spam/train",
            "/cli/mail/spam/allow",
            "/cli/mail/spam/block",
            "/cli/mail/spam/quarantine",
            "/cli/mail/spam/quarantine/release",
            "/cli/mail/spam/quarantine/discard",
            "/cli/mail/queue",
            "/cli/mail/queue/retry",
            "/cli/mail/queue/drop",
            "/cli/mail/acl",
            "/cli/mail/acl/revoke",
            "/cli/mail/autoconfig",
            "/cli/mail/app-password",
            "/cli/mail/app-password/revoke",
            "/cli/mail/dkim",
            "/cli/mail/dkim/rotate",
            "/cli/mail/dkim/retire",
            "/cli/mail/dmarc",
            "/cli/mail/dmarc/report-to",
            "/cli/mail/ptr",
            "/cli/mail/ptr/set",
        ] {
            assert!(is_agent_verb(p), "M5 must be an agent verb: {p}");
        }
        // Leftover M6 stay denied (uninstall / remove / OAuth / link / writer).
        for p in [
            "/cli/mail/server/uninstall",
            "/cli/mail/server/upgrade",
            "/cli/mail/domain/remove",
            "/cli/mail/dav",
            "/cli/mail/profile",
            "/cli/mail/external/add",
            "/cli/mail/link/oauth/start",
            "/cli/mail/oauth-config",
            "/cli/mail-manage",
        ] {
            assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
        }
        // 0.45.0: the IT-agent credential verbs ride the M5 extra gate
        // (mail-manage), with the self-elevation check in the handlers.
        for p in ["/cli/mail/credentials/keep", "/cli/mail/address/person"] {
            assert!(is_agent_verb(p), "M5 must be an agent verb: {p}");
        }
        // Calendars S4: groups + calendar sharing ride the same M5 gate
        // (self-widening refused in the handlers, mail::calendar_share).
        for p in [
            "/cli/mail/group",
            "/cli/mail/group/members",
            "/cli/mail/group/delete",
            "/cli/mail/calendar/manage",
            "/cli/mail/calendar/share",
            "/cli/mail/calendar/unshare",
            "/cli/mail/calendar/shares",
            "/cli/mail/calendar/sharing",
        ] {
            assert!(is_agent_verb(p), "M5 must be an agent verb: {p}");
        }
        assert!(is_agent_verb("/cli/db/create"));
        assert!(is_agent_verb("/cli/db/list"));
        assert!(is_agent_verb("/cli/db/grant"));
        assert!(is_agent_verb("/cli/store/put"));
        assert!(!is_agent_verb("/cli/db/server/enable"));
        assert!(!is_agent_verb("/cli/db/doctor"));
        assert!(!is_agent_verb("/cli/db/bind"));
    }

    /// E3 — prove scoped agents cannot self-grant: every access management
    /// verb is DENY_PREFIXES (`/cli/mail/access/`) before ALLOW_PREFIXES
    /// `/cli/mail/`. Mirrors the DNS zones/create denial style.
    #[test]
    fn is_agent_verb_denies_mail_access_mutations() {
        // Agent catalog + read/send stay admitted…
        assert!(
            is_agent_verb("/cli/mail/inboxes"),
            "agent view of the catalog remains an agent verb"
        );
        assert!(is_agent_verb("/cli/mail/messages"));
        assert!(is_agent_verb("/cli/mail/send"));
        // C5b: access management is an extra-gated mail_manage verb,
        // not a DENY. Unknown nested paths ride `/cli/mail/` and the
        // extra gate still fail-closes leftover M6.
        for p in [
            "/cli/mail/access/grant",
            "/cli/mail/access/revoke",
            "/cli/mail/access/set-primary",
            "/cli/mail/access/set-level",
            "/cli/mail/access/set-manage",
        ] {
            assert!(
                is_agent_verb(p),
                "C5b access is an extra-gated agent verb: {p}"
            );
        }
    }

    #[test]
    fn is_agent_verb_allows_dns_prefixes_and_denies_zone_lifecycle() {
        assert!(is_agent_verb("/cli/dns/access"));
        assert!(is_agent_verb("/cli/dns/zones"));
        assert!(is_agent_verb("/cli/dns/records/add"));
        // Zone create/delete are owner-only (DENY before ALLOW_PREFIXES).
        assert!(
            !is_agent_verb("/cli/dns/zones/create"),
            "scoped token must NOT create zones"
        );
        assert!(
            !is_agent_verb("/cli/dns/zones/delete"),
            "scoped token must NOT delete zones"
        );
        // A9: no prefix `/cli/domains/` or `/cli/certs/`. Apex remove
        // stays owner; hostname add/remove is dns_manage (exact paths).
        assert!(is_agent_verb("/cli/domains"));
        assert!(!is_agent_verb("/cli/domains/remove"));
        assert!(is_agent_verb("/cli/domains/names"));
        assert!(is_agent_verb("/cli/domains/names/remove"));
        assert!(is_agent_verb("/cli/certs"));
        assert!(is_agent_verb("/cli/certs/issue"));
        assert!(is_agent_verb("/cli/certs/renew"));
        assert!(!is_agent_verb("/cli/certs/upload"));
    }

    #[test]
    fn is_agent_verb_allows_respond_but_not_the_deny_surface() {
        // F2 (sandbox API): the in-cell agent's response egress is allowlisted.
        assert!(is_agent_verb("/cli/respond"), "/cli/respond is the agent response verb");
        // Host-session completion lifecycle (`k2 done`).
        assert!(
            is_agent_verb("/cli/session/complete"),
            "/cli/session/complete is the agent done verb"
        );
        // And the +1 must NOT have widened the escalation surface — the deny
        // prefixes still default-deny (belt-and-braces against an allowlist edit
        // accidentally crossing a DENY_PREFIX).
        for p in [
            "/cli/users/set-role",
            "/cli/fs/write-file",
            "/cli/clone/bundle",
            "/cli/tunnel/start",
            "/cli/daemon/restart",
            "/cli/sessions/v2/spawn",
            "/cli/terminal/create",
        ] {
            assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
        }
        // Near-miss: a `respond`-prefixed sub-path is NOT auto-allowed (exact match
        // only — `/cli/respond` is in ALLOW_EXACT, not ALLOW_PREFIXES).
        assert!(!is_agent_verb("/cli/respond/anything"), "respond is exact-match only");
        assert!(
            !is_agent_verb("/cli/session/complete/extra"),
            "session/complete is exact-match only"
        );
        // Do not open a broad /cli/session/ prefix.
        assert!(!is_agent_verb("/cli/session/anything-else"));
    }

    #[test]
    fn is_agent_verb_allows_design_notes_exact_only() {
        assert!(is_agent_verb("/cli/browser/notes"));
        assert!(is_agent_verb("/cli/browser/notes/redeliver"));
        assert!(!is_agent_verb("/cli/browser/notes/other"));
        assert!(!is_agent_verb("/cli/browser/"));
    }

    #[test]
    fn is_agent_verb_allows_browser_open_url_exact_only() {
        // 0.40.34: the shim's scoped-session egress verb is allowlisted…
        assert!(
            is_agent_verb("/cli/browser/open-url"),
            "shim must be able to forward URLs over the scoped channel"
        );
        // …EXACT match only — no `/cli/browser/` prefix widening.
        assert!(!is_agent_verb("/cli/browser/open-url/extra"));
        assert!(!is_agent_verb("/cli/browser/anything-else"));
    }

    #[test]
    fn is_agent_verb_admits_own_identity_not_handle() {
        // 0.46.1 `k2 agent identity`: color + picture on the agent's own
        // workspace; the handle stays a person's (or an App's) change.
        assert!(is_agent_verb("/cli/agent/identity"));
        assert!(!is_agent_verb("/cli/agent/handle"));
        assert!(!is_agent_verb("/cli/agent/identity/x"));
    }

    #[test]
    fn is_agent_verb_denies_review_checklist_after_p1_drop() {
        // Sandbox P1 (Finding-1 follow-on): review-checklist verbs were
        // DROPPED from the scoped allowlist — they take the raw body (not the
        // principal-pinned params) so they fall back to TCP+owner, never the
        // scoped cell UDS. Re-admitted in P2 with a body-restamp.
        assert!(!is_agent_verb("/cli/review-checklist/toggle"));
        assert!(!is_agent_verb("/cli/review-checklist/write"));
        assert!(!is_agent_verb("/cli/review-checklist/init"));
    }

    #[test]
    fn is_agent_verb_allows_awareness_publish_only() {
        // #58 Phase-1 close: publish is the +1 allowlist delta; subscribe
        // (read-only WS) and resolve are NOT widened onto the scoped token.
        assert!(is_agent_verb("/cli/awareness/publish"));
        // The swap handler scopes an agent passport to its own workspace.
        assert!(is_agent_verb("/cli/workspace/swap-canonical"));
        // C2: workspace-addressed terminal read is peer-gated agent verb.
        assert!(
            is_agent_verb("/cli/terminal/read"),
            "C2 admits k2 read <ws> for scoped principals (peer-gated)"
        );
        // The deliberate this-release DENIALS (PRD §B SCOPE HONESTY): these
        // stay owner/connect-user over TCP (write still denied).
        for p in [
            "/cli/awareness/subscribe",
            "/cli/workspace/resolve",
            "/cli/sessions/list-for-workspace",
            "/cli/terminal/write",
        ] {
            assert!(!is_agent_verb(p), "scoped token must NOT reach {p}");
        }
    }

    // ── mint stamps far-future expires_at (informational only) ─────────

    #[test]
    fn minted_expires_at_is_far_horizon_not_24h() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let _ = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
        let rec = reg.records.get(&sid.to_string()).expect("record present");
        let span = rec.expires_at - rec.created_at;
        assert!(
            span >= Duration::days(365 * 50),
            "expires_at stamp must be a multi-decade horizon (not 24h), got {span}",
        );
    }

    #[test]
    fn token_thirteen_hours_old_still_validates() {
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
        let rec = reg
            .records
            .get_mut(&sid.to_string())
            .expect("record present");
        rec.created_at = Utc::now() - Duration::hours(13);
        assert!(
            reg.validate_hook(&token).is_some(),
            "a 13h-old token must still validate",
        );
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
            "/cli/terminal/send-message",
            "/cli/terminal/compose-history",
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
        let scoped = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);

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

    // ── Phase 1: spawn-time activation (flag opt-out + env shape) ──

    #[test]
    fn cell_env_pairs_returns_none_when_flag_off() {
        // Explicit fleet opt-out: K2_HOOK_SCOPED=0 → mint NOTHING.
        // (Only reads the env var; no disk, no registry mutation.)
        // Still wrap in with_temp_home so a future edit that mints cannot
        // silently write real ~/.k2/hook-sessions.json.
        with_temp_home(|| {
            let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
            let sid = SessionId::new();
            assert!(
                cell_env_pairs(
                    &sid,
                    &sid.to_string(),
                    principal(),
                    CredMode::ApiKey,
                    Provider::Anthropic,
                    None
                )
                .is_none(),
                "flag OFF (explicit opt-out) MUST inject no scoped env",
            );
        });
    }

    /// T-S1i (A8): with `K2_HOOK_SCOPED=0` a v2 spawn still carries the
    /// pane id, so `notify.sh` can name its session; no token, no socket.
    #[test]
    fn t_s1i_pane_id_rides_the_spawn_with_scoped_hooks_off() {
        with_temp_home(|| {
            let scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
            let sid = SessionId::new();
            let pane = sid.to_string();
            let mut env = std::collections::HashMap::new();
            let minted = prepare_agent_spawn_env(
                &mut env,
                &sid,
                &pane,
                principal(),
                CredMode::ApiKey,
                Provider::Anthropic,
                None,
                "owner-secret",
            );
            drop(scoped);
            assert!(!minted);
            for k in ["K2_PANE_ID", "K2SO_PANE_ID", "K2_TAB_ID", "K2SO_TAB_ID"] {
                assert_eq!(env.get(k), Some(&pane), "{k} missing with scoped hooks off");
            }
            assert!(!env.contains_key("K2_HOOK_TOKEN"));
            assert!(!env.contains_key("K2_HOOK_SOCK"));
        });
    }

    #[test]
    fn prepare_agent_spawn_env_never_leaves_owner_token() {
        // PR-A load-bearing invariant: after prepare_agent_spawn_env, the
        // child env must not carry the owner token behind K2_HOOK_TOKEN
        // (whether scoped mint ran or the flag was opted out).
        //
        // MUST run under with_temp_home: flag-ON path calls mint_session_token
        // → save_to_disk(store_path()) which otherwise pollutes the
        // developer's real ~/.k2/hook-sessions.json with test principals
        // (ws-uuid-1 / agent-a).
        with_temp_home(|| {
            let owner = "owner-secret-deadbeef-aaaaaaaa";
            let sid = SessionId::new();
            let pane = sid.to_string();

            // Path A — flag ON (default mint): scoped passport replaces owner.
            let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "1");
            let mut env = std::collections::HashMap::new();
            env.insert("K2_HOOK_TOKEN".to_string(), owner.to_string());
            env.insert("K2SO_HOOK_TOKEN".to_string(), owner.to_string());
            let minted = prepare_agent_spawn_env(
                &mut env,
                &sid,
                &pane,
                principal(),
                CredMode::ApiKey,
                Provider::Anthropic,
                None,
                owner,
            );
            assert!(minted, "flag ON must mint a scoped passport");
            let tok = env.get("K2_HOOK_TOKEN").expect("scoped K2_HOOK_TOKEN present");
            assert_ne!(tok, owner, "scoped path must not leave the owner token");
            assert_ne!(
                env.get("K2SO_HOOK_TOKEN").map(String::as_str),
                Some(owner),
                "legacy alias must not leave the owner token",
            );
            // Minted value must validate as a real principal (process registry
            // under this temp HOME).
            let v = validate_hook(tok).expect("minted passport must validate_hook");
            assert_eq!(v.session_id, sid.to_string());
            assert_eq!(v.pane_id, pane);
            assert_eq!(v.principal, principal());

            // Path B — flag OFF: still strips owner even without a mint.
            let _scoped = k2_core::test_env::EnvVar::set("K2_HOOK_SCOPED", "0");
            let mut env_off = std::collections::HashMap::new();
            env_off.insert("K2_HOOK_TOKEN".to_string(), owner.to_string());
            env_off.insert("K2SO_HOOK_TOKEN".to_string(), owner.to_string());
            let minted_off = prepare_agent_spawn_env(
                &mut env_off,
                &sid,
                &pane,
                principal(),
                CredMode::ApiKey,
                Provider::Anthropic,
                None,
                owner,
            );
            assert!(!minted_off, "flag OFF must not mint");
            assert!(
                env_off.get("K2_HOOK_TOKEN").is_none(),
                "flag OFF must still strip owner from K2_HOOK_TOKEN",
            );
            assert!(
                env_off.get("K2SO_HOOK_TOKEN").is_none(),
                "flag OFF must still strip owner from K2SO_HOOK_TOKEN",
            );
        });
    }

    #[test]
    fn strip_owner_hook_tokens_preserves_non_owner_values() {
        let owner = "owner-secret";
        let mut env = std::collections::HashMap::new();
        env.insert("K2_HOOK_TOKEN".to_string(), "sid.scopedsecret".to_string());
        env.insert("K2SO_HOOK_TOKEN".to_string(), owner.to_string());
        strip_owner_hook_tokens(&mut env, owner);
        assert_eq!(
            env.get("K2_HOOK_TOKEN").map(String::as_str),
            Some("sid.scopedsecret"),
            "non-owner scoped value must be preserved",
        );
        assert!(
            env.get("K2SO_HOOK_TOKEN").is_none(),
            "owner value on the legacy key must be stripped",
        );
    }

    #[test]
    fn scoped_cell_env_for_token_carries_token_behind_hook_token_and_sock() {
        // Phase-1 env SHAPE (pure builder — no flag, no disk): the scoped
        // token rides the SAME K2_HOOK_TOKEN key (now per-cell, not owner)
        // and the per-cell socket rides K2_HOOK_SOCK, with K2SO_* aliases.
        let sid = SessionId::new();
        let pane = sid.to_string();
        let pairs = scoped_cell_env_for_token(
            "/run/cells/x.sock",
            "sid.secretverifier",
            &pane,
            54321,
            CredMode::ApiKey,
            Provider::Anthropic,
            None,
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();

        // The scoped token REPLACES the owner token behind the same key.
        assert_eq!(map.get("K2_HOOK_TOKEN").map(String::as_str), Some("sid.secretverifier"));
        assert_eq!(map.get("K2SO_HOOK_TOKEN").map(String::as_str), Some("sid.secretverifier"));
        // The per-cell UDS path is exposed so the CLI / notify.sh prefer it.
        assert_eq!(map.get("K2_HOOK_SOCK").map(String::as_str), Some("/run/cells/x.sock"));
        assert_eq!(map.get("K2SO_HOOK_SOCK").map(String::as_str), Some("/run/cells/x.sock"));
        // Pane/tab identity the hook script echoes back as paneId.
        assert_eq!(map.get("K2_PANE_ID").map(String::as_str), Some(pane.as_str()));
        assert_eq!(map.get("K2SO_TAB_ID").map(String::as_str), Some(pane.as_str()));
        // BLOCKER 3 — the daemon's loopback port rides BOTH names so the
        // in-guest `k2` CLI connection gate (which requires K2_PORT present)
        // dispatches; the verb still transports over K2_HOOK_SOCK.
        assert_eq!(map.get("K2_PORT").map(String::as_str), Some("54321"));
        assert_eq!(map.get("K2SO_PORT").map(String::as_str), Some("54321"));
    }

    #[test]
    fn scoped_cell_env_omits_port_when_hook_server_unbound() {
        // Port 0 = the hook server hasn't bound yet. Mirror the normal-session
        // env's `if hook_port > 0` guard: omit K2_PORT entirely rather than
        // stage a bogus `:0`. The socket-bound hook verbs still work; only the
        // (guest-unreachable) TCP fallback is absent.
        let sid = SessionId::new();
        let pane = sid.to_string();
        let pairs = scoped_cell_env_for_token(
            "/run/cells/x.sock",
            "sid.secret",
            &pane,
            0,
            CredMode::ApiKey,
            Provider::Anthropic,
            None,
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert!(map.get("K2_PORT").is_none(), "K2_PORT must be omitted at port 0");
        assert!(map.get("K2SO_PORT").is_none(), "K2SO_PORT must be omitted at port 0");
        // The token/sock/pane env is still fully present.
        assert_eq!(map.get("K2_HOOK_SOCK").map(String::as_str), Some("/run/cells/x.sock"));
    }

    // ── B3a: per-workspace API-key env staging + cred_mode claim ─────

    /// ApiKey mode + a non-blank key → `ANTHROPIC_API_KEY=<key>` is staged.
    #[test]
    fn scoped_cell_env_stages_api_key_in_apikey_mode() {
        let sid = SessionId::new();
        let pane = sid.to_string();
        let pairs = scoped_cell_env_for_token(
            "/run/cells/x.sock",
            "sid.secret",
            &pane,
            0,
            CredMode::ApiKey,
            Provider::Anthropic,
            Some("sk-ant-workspace-key"),
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert_eq!(
            map.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-ant-workspace-key"),
            "the per-workspace key must be staged behind the provider's env var",
        );
    }

    /// No key resolved (None) → NO `ANTHROPIC_API_KEY` at all (default-OFF
    /// parity: a workspace without a configured key stages nothing).
    #[test]
    fn scoped_cell_env_omits_api_key_when_none() {
        let sid = SessionId::new();
        let pane = sid.to_string();
        let pairs = scoped_cell_env_for_token(
            "/run/cells/x.sock",
            "sid.secret",
            &pane,
            0,
            CredMode::ApiKey,
            Provider::Anthropic,
            None,
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert!(
            map.get("ANTHROPIC_API_KEY").is_none(),
            "no key configured → ANTHROPIC_API_KEY must be absent (no empty assignment)",
        );
    }

    /// A blank/whitespace key → NOTHING staged (never an empty
    /// `ANTHROPIC_API_KEY=`).
    #[test]
    fn scoped_cell_env_omits_blank_api_key() {
        let sid = SessionId::new();
        let pane = sid.to_string();
        let pairs = scoped_cell_env_for_token(
            "/run/cells/x.sock",
            "sid.secret",
            &pane,
            0,
            CredMode::ApiKey,
            Provider::Anthropic,
            Some("   "),
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert!(
            map.get("ANTHROPIC_API_KEY").is_none(),
            "a blank key must never produce an empty ANTHROPIC_API_KEY=",
        );
    }

    /// Subscription mode is a DEFERRED no-op stub here: even WITH a key it
    /// stages nothing (B3b will bind-mount the on-device creds instead).
    #[test]
    fn scoped_cell_env_omits_api_key_in_subscription_mode() {
        let sid = SessionId::new();
        let pane = sid.to_string();
        let pairs = scoped_cell_env_for_token(
            "/run/cells/x.sock",
            "sid.secret",
            &pane,
            0,
            CredMode::Subscription,
            Provider::Anthropic,
            Some("sk-ant-should-be-ignored"),
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert!(
            map.get("ANTHROPIC_API_KEY").is_none(),
            "Subscription mode is a no-op stub in B3a — no key staged",
        );
    }

    /// cred_mode + provider are stamped at mint and surfaced by validate_hook
    /// (read from the VALIDATED token, never a body). Round-trips for both
    /// modes; the key itself is NEVER part of the token record.
    #[test]
    fn cred_mode_and_provider_round_trip_through_validate() {
        let mut reg = SessionTokenRegistry::new();

        let sid_a = SessionId::new();
        let tok_a = reg.mint(&sid_a, "pane-a", principal(), CredMode::ApiKey, Provider::Anthropic);
        let v_a = reg.validate_hook(&tok_a).expect("apikey token validates");
        assert_eq!(v_a.cred_mode, CredMode::ApiKey);
        assert_eq!(v_a.provider, Provider::Anthropic);

        let sid_b = SessionId::new();
        let tok_b = reg.mint(&sid_b, "pane-b", principal(), CredMode::Subscription, Provider::Anthropic);
        let v_b = reg.validate_hook(&tok_b).expect("subscription token validates");
        assert_eq!(v_b.cred_mode, CredMode::Subscription);
        assert_eq!(v_b.provider, Provider::Anthropic);
    }

    /// CredMode defaults to ApiKey (serde default) so a pre-B3a persisted
    /// record loads on the primary path, and Provider maps to its env var.
    #[test]
    fn cred_mode_default_is_apikey_and_provider_env_var() {
        assert_eq!(CredMode::default(), CredMode::ApiKey);
        assert_eq!(Provider::default(), Provider::Anthropic);
        assert_eq!(Provider::Anthropic.key_env_var(), "ANTHROPIC_API_KEY");
    }

    // ── Phase 1: dual-accept + pane scoping (flag-ON semantics) ──────

    #[test]
    fn dual_accept_owner_token_independent_of_scoped_token() {
        // The OWNER arm of /hook/complete is `ct_eq_token` and is NEVER
        // touched by the scoped machinery — so the owner token keeps working
        // over TCP whether or not the flag is on (Phase 1 is dual-accept;
        // Phase 2 — owner REJECTION — is deliberately NOT implemented here).
        let owner = "owner-fixed-hex-token";
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let scoped = reg.mint(&sid, &sid.to_string(), principal(), CredMode::ApiKey, Provider::Anthropic);

        // Owner token always passes its own constant-time compare.
        assert!(crate::routes::http::ct_eq_token(owner, owner));
        // …and the scoped token is provably NOT the owner token.
        assert_ne!(scoped, owner);
        assert!(!crate::routes::http::ct_eq_token(&scoped, owner));
    }

    #[test]
    fn scoped_hook_authorizes_only_its_own_pane() {
        // PRD §5 smoke #3/#4: a scoped token authorizes ONLY the exact pane
        // it was minted for; the same token with a different paneId → false.
        let mut reg = SessionTokenRegistry::new();
        let sid = SessionId::new();
        let token = reg.mint(&sid, "pane-A", principal(), CredMode::ApiKey, Provider::Anthropic);

        assert!(
            reg.scoped_hook_authorizes_pane(&token, "pane-A"),
            "correct pane must authorize",
        );
        assert!(
            !reg.scoped_hook_authorizes_pane(&token, "pane-B"),
            "a different pane MUST be refused (scoping enforced)",
        );
        assert!(
            !reg.scoped_hook_authorizes_pane(&token, ""),
            "an empty paneId never authorizes",
        );
        // After revoke the token authorizes NO pane.
        reg.revoke(&sid);
        assert!(
            !reg.scoped_hook_authorizes_pane(&token, "pane-A"),
            "a revoked token authorizes no pane",
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
        let _scoped = reg.mint(&sid, "pane-1", principal(), CredMode::ApiKey, Provider::Anthropic);
        // require_hook consults the PROCESS registry, not `reg`, so we
        // assert the capability gate via is_agent_verb (the first guard).
        assert!(!is_agent_verb("/cli/users/set-role"));
        assert!(require_hook("anything.here", "/cli/users/set-role").is_none());
    }
}
