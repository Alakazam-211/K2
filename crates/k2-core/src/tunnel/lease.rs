//! Subdomain lease renewal — the daemon-owned keepalive for a K2 Connect
//! tunnel's `<sub>.k2.dev` claim.
//!
//! ## Why this lives in the daemon (K2SO #674)
//!
//! The control plane (Supabase) expires a subdomain *claim* after **3
//! minutes** without a heartbeat (see `freshClaim` in the renderer's
//! `k2-account.ts` and the `claim_subdomain` RPC). The claim is what makes
//! `<sub>.k2.dev` route to THIS machine; once it lapses the host silently
//! drops off the internet even though `frpc` is still dialed in.
//!
//! Historically the renderer drove the renewal on a 60 s `setInterval`,
//! gated on the Settings → K2 Connect panel being *mounted*. Closing
//! Settings — or running the daemon headless with no client attached —
//! stopped the heartbeat and the lease lapsed. This module relocates the
//! *scheduling* into the daemon so renewal survives a closed panel and a
//! headless daemon, tied to the tunnel's own lifecycle.
//!
//! ## What a "renewal" actually is
//!
//! Re-POSTing the SAME `claim_subdomain` RPC the renderer already calls. A
//! successful claim by the holding device acts as the heartbeat. The wire
//! semantics and the control-plane contract are UNCHANGED — only *who
//! schedules* the periodic call moved. To call the RPC the daemon needs a
//! Supabase **access token**, which it derives from the account **refresh
//! token** the renderer persisted to the OS keychain at sign-in. The
//! CURRENT layout (renderer double-prompt fix) is ONE item — a JSON blob
//! `{"refreshToken": …, "email": …}` under `dev.k2.connect.account` /
//! `session`; the pre-blob bare-string item (`session-refresh-token`) is
//! read as a fallback only (the renderer DELETES it on fresh sign-ins).
//!
//! Cadence: [`RENEW_INTERVAL`] (60 s) — matches the old renderer cadence
//! and is well inside the 3-minute server TTL ([`LEASE_TTL`]).
//!
//! ## When renewal does NOT run (K2 Cloud P1-C)
//!
//! The lease only powers the "which device holds this subdomain" holder
//! UI — the tunnel itself works without it, and K2 Cloud hosted rows are
//! single-holder by construction. So on daemons with NO account session
//! (headless/hosted/provisioned boxes, non-macOS, a Mac that never signed
//! in) the renew loop is skipped with ONE loud line instead of warning on
//! every tick. Hosted images can also hard-disable it with
//! `K2_TUNNEL_LEASE=off` ([`LEASE_ENV_VAR`]), which is honored BEFORE any
//! keychain probing.

use std::time::Duration;

use serde::Deserialize;


// The account-session keychain item (constants, read/write/clear, the
// trusted-app ACL) lives in `k2_client::account_session` so the thin client
// writes it with the same code. Re-exported here so every
// `tunnel::lease::*` path keeps working.
pub use k2_client::account_session::{
    bundle_peer_binaries, clear_account_session, read_account_session, resolve_account_session,
    session_blob_json, try_write_account_session, write_account_session, AccountSession,
};
#[cfg(target_os = "macos")]
pub use k2_client::account_session::{
    item_write_options, purge_legacy_session_items, read_keychain_item, ACCOUNT_EMAIL_KEY,
    ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_REFRESH_KEY, ACCOUNT_SESSION_BLOB_KEY,
    LEGACY_ACCOUNT_KEYCHAIN_SERVICE,
};

/// Env kill-switch: `K2_TUNNEL_LEASE=off` disables daemon-side lease
/// renewal entirely. Honored BEFORE any keychain probing so hosted images
/// that set it never touch (or prompt for) a keychain.
pub const LEASE_ENV_VAR: &str = "K2_TUNNEL_LEASE";

/// Server-side claim TTL: a claim with no heartbeat for this long expires
/// and the subdomain is free for another device. Mirrors the renderer's
/// `freshClaim` 3-minute window. The renewal cadence MUST stay well inside
/// this.
pub const LEASE_TTL: Duration = Duration::from_secs(3 * 60);

/// How often the daemon re-claims (heartbeats) the lease while the tunnel
/// is up. 60 s — matches the old renderer cadence and leaves a 2-minute
/// safety margin under [`LEASE_TTL`] so a single missed tick never drops
/// the lease.
pub const RENEW_INTERVAL: Duration = Duration::from_secs(60);

/// HTTP timeout for a single control-plane call. Kept short so a hung
/// network never wedges the renewal loop past a tick.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Pure cadence/lifecycle invariant check, factored out so it can be
/// unit-tested without any network. The renewal cadence is only safe when
/// it fires strictly faster than the lease expires — otherwise a single
/// scheduling jitter could let the lease lapse between heartbeats.
///
/// Returns `true` when `interval` leaves a margin under `ttl` (we require
/// at least 2x headroom: the loop can miss one tick and still renew before
/// the TTL elapses).
pub const fn cadence_is_safe(interval: Duration, ttl: Duration) -> bool {
    // interval * 2 <= ttl  (avoids overflow vs. comparing interval to ttl/2
    // when ttl is odd seconds).
    let i = interval.as_secs();
    let t = ttl.as_secs();
    i > 0 && i.saturating_mul(2) <= t
}


// ── Renewal availability (env kill-switch + session presence) ───────────

/// Whether — and why not — the daemon-side renew loop should run for this
/// tunnel start. Decided ONCE at spawn time so a daemon with no session
/// material skips cleanly instead of erroring on every 60 s tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewalMode {
    /// Session material present → run the renew loop.
    Enabled,
    /// [`LEASE_ENV_VAR`] is set to an off value → hard off; no keychain
    /// was (or will be) probed.
    DisabledByEnv,
    /// No account session anywhere: never signed in, signed out, or a
    /// headless/hosted/provisioned daemon with no keychain at all. Normal
    /// for K2 Cloud boxes — the tunnel works without the lease.
    NoSession,
}

/// `K2_TUNNEL_LEASE=off` (also accepts `0`/`false`, case-insensitive,
/// trimmed) → `true`. Anything else — unset, empty, `on`, `1` — leaves the
/// lease enabled.
pub fn env_disables_lease(val: Option<&str>) -> bool {
    matches!(
        val.map(str::trim),
        Some(v) if v.eq_ignore_ascii_case("off")
            || v == "0"
            || v.eq_ignore_ascii_case("false")
    )
}

/// Pure half of the availability decision (unit-testable): env value +
/// whether session material exists. Env wins unconditionally — callers
/// ([`renewal_mode`]) check it BEFORE probing any keychain.
pub fn decide_renewal_mode(env_val: Option<&str>, has_session: bool) -> RenewalMode {
    if env_disables_lease(env_val) {
        return RenewalMode::DisabledByEnv;
    }
    if !has_session {
        return RenewalMode::NoSession;
    }
    RenewalMode::Enabled
}

/// Process-facing availability decision: the env kill-switch first (NO
/// keychain probe when it says off — a probe could raise a keychain prompt
/// on macOS), then the session probe.
pub fn renewal_mode() -> RenewalMode {
    if env_disables_lease(std::env::var(LEASE_ENV_VAR).ok().as_deref()) {
        return RenewalMode::DisabledByEnv;
    }
    decide_renewal_mode(None, read_account_session().is_some())
}

/// Everything the daemon needs to renew a single subdomain's lease,
/// resolved once from the stored config. Carrying these together keeps the
/// renewal loop free of config re-reads on the hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseTarget {
    /// The subdomain label being held, e.g. `rosson`.
    pub label: String,
    /// Stable per-install device id — MUST match the id the renderer used
    /// for its one-shot claim so the renewal continues the SAME lease
    /// rather than looking like a different device taking over.
    pub device_id: String,
    /// Human-readable device label (cosmetic; shown in the holder UI).
    pub device_label: Option<String>,
}

impl LeaseTarget {
    /// Build a renewal target from the stored tunnel config, or `None`
    /// when the config can't drive a renewal (no subdomain label or no
    /// device id persisted by the client). A `None` here is the signal to
    /// skip renewal entirely (e.g. a token-only manual config that never
    /// went through the account/claim flow).
    pub fn from_config(cfg: &super::config::TunnelConfig) -> Option<Self> {
        let label = cfg.subdomain.trim();
        let device_id = cfg.device_id.as_deref().unwrap_or("").trim();
        if label.is_empty() || device_id.is_empty() {
            return None;
        }
        Some(Self {
            label: label.to_string(),
            device_id: device_id.to_string(),
            device_label: cfg
                .device_label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }
}


/// 0.40.0 rebrand — proactively copy a pre-rename K2 Connect session
/// (`com.k2so.connect.account`) forward to `dev.k2.connect.account` at
/// daemon boot, so a signed-in user stays signed in WITHOUT having to
/// open the K2 Connect settings page (which is what used to trigger the
/// lazy copy-on-read). Returns `true` if a migration happened. Idempotent
/// + best-effort: a no-op when there's no legacy item, or the new item is
/// already present. macOS-only.
///
/// 0.40.7 — this boot pass ALSO upgrades the trusted-application ACL on the
/// current-service item when it already exists. The renderer creates the
/// item via `keyring`, whose default ACL trusts only the app binary, so
/// every daemon `security` read prompts. Re-stamping the ACL once at boot
/// (with the same token value, see [`write_token_with_acl`]) converts an
/// every-read prompt into — at most — a single boot prompt that "Always
/// Allow" then satisfies; once stamped, subsequent boots are prompt-free
/// because the daemon is already on the ACL. The boot probe read below is
/// the read that may prompt the one time; everything after is silent.
/// 0.40.30 (K2 Cloud P1-C) — this boot pass ALSO migrates the LEGACY
/// two-item layout (`session-refresh-token` [+ `session-email`]) forward
/// into the current single-blob layout, mirroring the renderer's
/// `readAccountSession` fallback-then-migrate, and purges the superseded
/// two-item entries afterwards exactly like the renderer's
/// `saveAccountSession` does.
#[cfg(target_os = "macos")]
pub fn migrate_account_keychain() -> bool {
    // Env kill-switch FIRST: hosted images set K2_TUNNEL_LEASE=off to keep
    // the daemon away from any keychain (a probe could raise a prompt).
    if env_disables_lease(std::env::var(LEASE_ENV_VAR).ok().as_deref()) {
        return false;
    }
    // Current-layout blob already present → no migration needed, but
    // UPGRADE its ACL once so the renewal loop's reads stop prompting.
    // Re-writing the SAME value (re)installs our ACL; idempotent — once the
    // daemon is on the ACL, this re-write needs no prompt.
    if let Some(raw) = read_keychain_item(ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_SESSION_BLOB_KEY) {
        if let Some(sess) = resolve_account_session(Some(&raw), None) {
            crate::log_debug!(
                "[tunnel/lease] boot ACL upgrade on {ACCOUNT_KEYCHAIN_SERVICE}/{ACCOUNT_SESSION_BLOB_KEY} (re-stamp trusted-app list)"
            );
            write_account_session(&sess);
        }
        return false;
    }
    // Blob under the pre-rename service → copy forward. The legacy-SERVICE
    // copy is deliberately left in place (renderer parity: "an un-updated
    // daemon may still read it").
    if let Some(raw) =
        read_keychain_item(LEGACY_ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_SESSION_BLOB_KEY)
    {
        if let Some(sess) = resolve_account_session(Some(&raw), None) {
            crate::log_debug!(
                "[tunnel/lease] boot keychain migration {LEGACY_ACCOUNT_KEYCHAIN_SERVICE} → {ACCOUNT_KEYCHAIN_SERVICE} (session blob)"
            );
            write_account_session(&sess);
            return true;
        }
    }
    // LEGACY two-item layout (either service) → migrate forward to the
    // single blob, carrying the email item along when present, then drop
    // the superseded two-item entries so a later read can't resurrect a
    // soon-to-be-spent refresh token (mirrors renderer saveAccountSession).
    for service in [ACCOUNT_KEYCHAIN_SERVICE, LEGACY_ACCOUNT_KEYCHAIN_SERVICE] {
        if let Some(token) = read_keychain_item(service, ACCOUNT_REFRESH_KEY) {
            let email = read_keychain_item(service, ACCOUNT_EMAIL_KEY)
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty());
            crate::log_debug!(
                "[tunnel/lease] boot keychain migration {service}/{ACCOUNT_REFRESH_KEY} → {ACCOUNT_KEYCHAIN_SERVICE}/{ACCOUNT_SESSION_BLOB_KEY}"
            );
            write_account_session(&AccountSession {
                refresh_token: token,
                email,
            });
            purge_legacy_session_items();
            return true;
        }
    }
    false
}

#[cfg(not(target_os = "macos"))]
pub fn migrate_account_keychain() -> bool {
    false
}


// ── Control-plane calls (identical wire to the renderer) ────────────────
//
// 0.46.6 refresh-loop fix. Before this, EVERY 60 s tick exchanged the
// refresh token for a new access token (`grant_type=refresh_token`) even
// though the access token lives an hour: ~1,440 new refresh tokens a day per
// signed-in Mac, from a client that sent no User-Agent. Now the access token
// is kept in memory ([`TokenKeeper`], one per process) and refreshed only
// when it is within [`REFRESH_MARGIN`] of expiry; a failed refresh backs off
// (doubling from [`RENEW_INTERVAL`] to [`REFRESH_BACKOFF_MAX`]); the rotated
// refresh token is persisted BEFORE it is used, and if that write fails the
// keeper keeps presenting the rotated token from memory instead of the
// spent one still in the keychain (a spent token must never be sent again).

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ClaimRow {
    #[serde(default)]
    claimed: bool,
}

/// Refresh the access token when it has less than this left.
pub const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);

/// Longest wait between refresh attempts after repeated failures.
pub const REFRESH_BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

/// Access-token lifetime assumed when the server's answer has no
/// `expires_in` (GoTrue always sends it; this is the cautious floor).
const DEFAULT_ACCESS_TTL: Duration = Duration::from_secs(10 * 60);

/// How many superseded refresh tokens the keeper remembers, so a keychain
/// still holding one of them (its rotation write failed) is recognised as
/// stale rather than as a new sign-in.
const SUPERSEDED_KEEP: usize = 8;

/// A successful `grant_type=refresh_token` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub access: String,
    pub refresh: String,
    /// Seconds until the access token expires, as the server said.
    pub expires_in: Option<u64>,
}

/// The wait after `failures` consecutive failed refreshes: RENEW_INTERVAL,
/// then doubling, capped at [`REFRESH_BACKOFF_MAX`].
pub fn refresh_backoff(failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(16);
    RENEW_INTERVAL
        .checked_mul(1u32 << shift)
        .unwrap_or(REFRESH_BACKOFF_MAX)
        .min(REFRESH_BACKOFF_MAX)
}

#[derive(Debug, Clone)]
struct Held {
    access: String,
    expires_at: std::time::Instant,
    /// The newest refresh token (the one to present next time).
    refresh: String,
    /// False when writing `refresh` to the keychain failed; retried on
    /// every tick until it lands.
    persisted: bool,
}

/// The process's access token for the account server, and when (not) to
/// refresh it. Pure apart from the two closures, so the whole policy is
/// unit-tested with fakes and synthetic clocks.
#[derive(Debug, Default)]
pub struct TokenKeeper {
    held: Option<Held>,
    /// Refresh tokens this keeper already exchanged (newest last).
    superseded: Vec<String>,
    failures: u32,
    retry_at: Option<std::time::Instant>,
    /// The refresh token the counted failures were for; new material
    /// (a fresh sign-in) starts over without waiting.
    failed_for: Option<String>,
}

impl TokenKeeper {
    /// An access token for the session whose stored refresh token is
    /// `stored`, refreshing only when needed.
    ///
    /// * `refresh(token)` performs the network exchange;
    /// * `persist(rotated)` writes the rotated refresh token to the
    ///   keychain and says whether it landed.
    pub fn access_token(
        &mut self,
        stored: &str,
        now: std::time::Instant,
        refresh: impl FnOnce(&str) -> Result<Refreshed, String>,
        mut persist: impl FnMut(&str) -> bool,
    ) -> Result<String, String> {
        // Which refresh token is current?
        let current = match self.held.as_ref().map(|h| h.refresh.clone()) {
            Some(ours) if ours == stored => ours,
            Some(ours) if self.superseded.iter().any(|s| s == stored) => {
                // The keychain still holds a token we already spent: our
                // rotation write did not land. Never present the spent one;
                // retry the write.
                if let Some(h) = self.held.as_mut() {
                    if !h.persisted {
                        h.persisted = persist(&h.refresh);
                    }
                }
                ours
            }
            Some(_) => {
                // Someone else wrote new material (the desktop client signed
                // in again or refreshed): start over from it.
                self.held = None;
                self.superseded.clear();
                stored.to_string()
            }
            None => stored.to_string(),
        };
        if let Some(h) = &self.held {
            if now + REFRESH_MARGIN < h.expires_at {
                return Ok(h.access.clone());
            }
        }
        if self.failed_for.as_deref() != Some(current.as_str()) {
            self.failures = 0;
            self.retry_at = None;
            self.failed_for = None;
        }
        if let Some(at) = self.retry_at {
            if now < at {
                // Still inside the backoff: use the old access token while it
                // is valid at all, else wait.
                if let Some(h) = self.held.as_ref().filter(|h| now < h.expires_at) {
                    return Ok(h.access.clone());
                }
                return Err(format!(
                    "token refresh backing off for {}s after {} failure(s)",
                    at.saturating_duration_since(now).as_secs(),
                    self.failures
                ));
            }
        }
        match refresh(&current) {
            Ok(r) => {
                // Persist BEFORE use: once the server rotated, `current` is
                // spent.
                let persisted = r.refresh == current || persist(&r.refresh);
                if r.refresh != current {
                    self.superseded.push(current);
                    let extra = self.superseded.len().saturating_sub(SUPERSEDED_KEEP);
                    self.superseded.drain(..extra);
                }
                let ttl = r
                    .expires_in
                    .map(Duration::from_secs)
                    .unwrap_or(DEFAULT_ACCESS_TTL);
                self.held = Some(Held {
                    access: r.access.clone(),
                    expires_at: now + ttl,
                    refresh: r.refresh,
                    persisted,
                });
                self.failures = 0;
                self.retry_at = None;
                self.failed_for = None;
                Ok(r.access)
            }
            Err(e) => {
                self.failures = self.failures.saturating_add(1);
                self.retry_at = Some(now + refresh_backoff(self.failures));
                self.failed_for = Some(current);
                if let Some(h) = self.held.as_ref().filter(|h| now < h.expires_at) {
                    return Ok(h.access.clone());
                }
                Err(e)
            }
        }
    }

    /// The server refused the access token (401): refresh on the next
    /// tick instead of reusing it until its stated expiry.
    pub fn expire_access(&mut self, now: std::time::Instant) {
        if let Some(h) = self.held.as_mut() {
            h.expires_at = now;
        }
    }
}

fn keeper() -> std::sync::MutexGuard<'static, TokenKeeper> {
    static KEEPER: std::sync::OnceLock<std::sync::Mutex<TokenKeeper>> = std::sync::OnceLock::new();
    KEEPER
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Exchange a refresh token for a fresh access token at `base`. Mirrors
/// `refreshSession` in the renderer. The refresh token rotates; the
/// caller ([`TokenKeeper`]) persists the returned one.
fn refresh_access_token(base: &str, refresh_token: &str) -> Result<Refreshed, String> {
    let client = http_client()?;
    let resp = client
        .post(format!("{base}/auth/v1/token?grant_type=refresh_token"))
        .header("apikey", crate::connect_db::ANON_KEY)
        .header("Content-Type", "application/json")
        .body(serde_json::json!({ "refresh_token": refresh_token }).to_string())
        .send()
        .map_err(|e| format!("refresh request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("refresh rejected (HTTP {})", resp.status()));
    }
    // Parse via serde_json::from_str (reqwest's `json` feature isn't
    // enabled in this crate — only `blocking`+`rustls-tls`).
    let text = resp
        .text()
        .map_err(|e| format!("refresh response read failed: {e}"))?;
    let body: TokenResponse = serde_json::from_str(&text)
        .map_err(|_| "refresh response parse failed".to_string())?;
    match (body.access_token, body.refresh_token) {
        (Some(access), Some(refresh)) if !access.is_empty() && !refresh.is_empty() => {
            Ok(Refreshed { access, refresh, expires_in: body.expires_in })
        }
        _ => Err("refresh response missing access/refresh token".to_string()),
    }
}

/// A failed claim: the HTTP status when the server answered.
struct ClaimFailure {
    status: Option<u16>,
    msg: String,
}

/// Re-POST the `claim_subdomain` RPC — the heartbeat. Mirrors
/// `claimSubdomain` in the renderer byte-for-byte on the wire (same RPC,
/// same params). Returns `Ok(true)` when THIS device holds the lease after
/// the call (the heartbeat landed), `Ok(false)` when another device holds
/// a fresh claim, `Err` on transport/HTTP failure.
fn claim_subdomain(
    base: &str,
    access_token: &str,
    label: &str,
    device_id: &str,
    device_label: Option<&str>,
) -> Result<bool, ClaimFailure> {
    let fail = |status: Option<u16>, msg: String| ClaimFailure { status, msg };
    let client = http_client().map_err(|e| fail(None, e))?;
    let resp = client
        .post(format!("{base}/rest/v1/rpc/claim_subdomain"))
        .header("apikey", crate::connect_db::ANON_KEY)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .body(
            serde_json::json!({
                "p_label": label,
                "p_device_id": device_id,
                "p_device_label": device_label,
            })
            .to_string(),
        )
        .send()
        .map_err(|e| fail(None, format!("claim request failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(fail(Some(status.as_u16()), format!("claim rejected (HTTP {status})")));
    }
    let text = resp
        .text()
        .map_err(|e| fail(None, format!("claim response read failed: {e}")))?;
    let rows: Vec<ClaimRow> = serde_json::from_str(&text)
        .map_err(|_| fail(None, "claim response parse failed".to_string()))?;
    Ok(rows.first().map(|r| r.claimed).unwrap_or(false))
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(crate::connect_db::USER_AGENT)
        .build()
        .map_err(|e| format!("http client build failed: {e}"))
}

/// Perform one renewal cycle for `target`: read the keychain session, get
/// an access token from the process's [`TokenKeeper`] (a network refresh
/// only near expiry, persisting any rotation first), and re-claim the
/// subdomain at the account server ([`crate::connect_db::base_url`]).
/// Returns `Ok(true)` when the lease was renewed (this device holds it),
/// `Ok(false)` when another device now holds it, `Err` on a transport/auth
/// failure (the next tick retries; refreshes back off).
///
/// This is the network-touching entry point; unit tests cover its pure
/// seams ([`TokenKeeper`], [`refresh_backoff`], [`cadence_is_safe`],
/// [`LeaseTarget::from_config`], [`decide_renewal_mode`]).
pub fn renew_once(target: &LeaseTarget) -> Result<bool, String> {
    let sess = read_account_session()
        .ok_or_else(|| "no account session in keychain (signed out?)".to_string())?;
    let base = crate::connect_db::base_url();
    let access = keeper().access_token(
        &sess.refresh_token,
        std::time::Instant::now(),
        |rt| refresh_access_token(&base, rt),
        |rotated| {
            try_write_account_session(&AccountSession {
                refresh_token: rotated.to_string(),
                email: sess.email.clone(),
            })
        },
    )?;
    match claim_subdomain(
        &base,
        &access,
        &target.label,
        &target.device_id,
        target.device_label.as_deref(),
    ) {
        Ok(held) => Ok(held),
        Err(f) => {
            if f.status == Some(401) {
                keeper().expire_access(std::time::Instant::now());
            }
            Err(f.msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::config::TunnelConfig;

    #[test]
    fn cadence_is_safe_for_the_shipped_constants() {
        // The constants we ship MUST satisfy the 2x-headroom invariant: a
        // 60 s heartbeat under a 180 s TTL leaves room to miss a tick.
        assert!(
            cadence_is_safe(RENEW_INTERVAL, LEASE_TTL),
            "shipped cadence {RENEW_INTERVAL:?} must be safe under TTL {LEASE_TTL:?}"
        );
    }

    #[test]
    fn cadence_rejects_too_slow_or_zero() {
        // Exactly TTL — no headroom, must be rejected.
        assert!(!cadence_is_safe(Duration::from_secs(180), LEASE_TTL));
        // Slower than TTL — definitely unsafe.
        assert!(!cadence_is_safe(Duration::from_secs(200), LEASE_TTL));
        // Just over half the TTL — still unsafe (no 2x margin).
        assert!(!cadence_is_safe(Duration::from_secs(91), LEASE_TTL));
        // Exactly half — the boundary, accepted.
        assert!(cadence_is_safe(Duration::from_secs(90), LEASE_TTL));
        // A zero interval would busy-loop — never safe.
        assert!(!cadence_is_safe(Duration::from_secs(0), LEASE_TTL));
    }

    #[test]
    fn lease_target_requires_label_and_device_id() {
        // Token-only manual config (no subdomain, no device id) → no
        // renewal target.
        let cfg = TunnelConfig {
            token: "tok".to_string(),
            ..Default::default()
        };
        assert_eq!(LeaseTarget::from_config(&cfg), None);

        // Subdomain but no device id (never went through the claim flow)
        // → still no target.
        let cfg = TunnelConfig {
            token: "tok".to_string(),
            subdomain: "rosson".to_string(),
            ..Default::default()
        };
        assert_eq!(LeaseTarget::from_config(&cfg), None);
    }

    #[test]
    fn lease_target_built_when_label_and_device_present() {
        let cfg = TunnelConfig {
            token: "tok".to_string(),
            subdomain: "  rosson  ".to_string(),
            device_id: Some("dev-123".to_string()),
            device_label: Some("MacIntel".to_string()),
            ..Default::default()
        };
        let target = LeaseTarget::from_config(&cfg).expect("target should build");
        assert_eq!(target.label, "rosson", "label must be trimmed");
        assert_eq!(target.device_id, "dev-123");
        assert_eq!(target.device_label.as_deref(), Some("MacIntel"));
    }

    #[test]
    fn lease_target_drops_blank_device_label() {
        let cfg = TunnelConfig {
            subdomain: "rosson".to_string(),
            device_id: Some("dev-123".to_string()),
            device_label: Some("   ".to_string()),
            ..Default::default()
        };
        let target = LeaseTarget::from_config(&cfg).expect("target should build");
        assert_eq!(
            target.device_label, None,
            "a whitespace-only device label must normalize to None"
        );
    }

    // ── resolve_account_session (pure seam; no keychain touched) ────────

    #[test]
    fn resolves_current_renderer_blob() {
        // The EXACT shape the renderer persists (`saveAccountSession` in
        // K2ConnectSection.tsx): JSON.stringify({ refreshToken, email }).
        let blob = r#"{"refreshToken":"rt-blob-1","email":"rosson@k2.dev"}"#;
        let sess = resolve_account_session(Some(blob), None)
            .expect("current-layout blob must resolve");
        assert_eq!(sess.refresh_token, "rt-blob-1");
        assert_eq!(sess.email.as_deref(), Some("rosson@k2.dev"));
    }

    #[test]
    fn blob_wins_over_legacy_entry() {
        let blob = r#"{"refreshToken":"rt-from-blob","email":"a@b.c"}"#;
        let sess = resolve_account_session(Some(blob), Some("rt-from-legacy"))
            .expect("must resolve");
        assert_eq!(
            sess.refresh_token, "rt-from-blob",
            "the blob is the CURRENT layout and must win over the legacy string"
        );
    }

    #[test]
    fn corrupt_blob_falls_back_to_legacy() {
        let sess = resolve_account_session(Some("definitely not json"), Some("rt-legacy"))
            .expect("legacy fallback must resolve");
        assert_eq!(sess.refresh_token, "rt-legacy");
        assert_eq!(sess.email, None, "legacy entry carries no email");
    }

    #[test]
    fn tokenless_or_empty_token_blob_falls_back_to_legacy() {
        // Blob parses but has no usable refreshToken → legacy wins.
        for blob in [
            r#"{"email":"a@b.c"}"#,
            r#"{"refreshToken":"","email":"a@b.c"}"#,
            r#"{"refreshToken":"   ","email":"a@b.c"}"#,
            r#"{"refreshToken":null,"email":"a@b.c"}"#,
        ] {
            let sess = resolve_account_session(Some(blob), Some("rt-legacy"))
                .unwrap_or_else(|| panic!("legacy fallback must resolve for blob {blob}"));
            assert_eq!(sess.refresh_token, "rt-legacy", "blob was {blob}");
        }
    }

    #[test]
    fn empty_email_in_blob_normalizes_to_none() {
        let blob = r#"{"refreshToken":"rt-1","email":""}"#;
        let sess = resolve_account_session(Some(blob), None).expect("must resolve");
        assert_eq!(sess.email, None);
    }

    #[test]
    fn legacy_only_resolves_trimmed() {
        let sess =
            resolve_account_session(None, Some("  rt-legacy  ")).expect("must resolve");
        assert_eq!(sess.refresh_token, "rt-legacy");
        assert_eq!(sess.email, None);
    }

    #[test]
    fn no_material_resolves_to_none() {
        assert_eq!(resolve_account_session(None, None), None);
        assert_eq!(
            resolve_account_session(Some("not json"), Some("   ")),
            None,
            "corrupt blob + whitespace-only legacy must resolve to no session"
        );
    }

    #[test]
    fn blob_roundtrip_matches_renderer_shape() {
        // What the daemon writes back on rotation MUST parse both through
        // our own reader AND through the renderer's expectations:
        // `refreshToken` a non-empty string, `email` always a string.
        let sess = AccountSession {
            refresh_token: "rt-rotated".to_string(),
            email: Some("rosson@k2.dev".to_string()),
        };
        let json = session_blob_json(&sess);
        assert_eq!(
            resolve_account_session(Some(&json), None).expect("roundtrip"),
            sess
        );
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["refreshToken"], "rt-rotated");
        assert!(
            v["email"].is_string(),
            "renderer's AccountSession.email is a string — never null/missing"
        );

        // An email-less session (recovered from the legacy entry) still
        // writes email as an EMPTY STRING, matching `parsed.email ?? ''`.
        let json = session_blob_json(&AccountSession {
            refresh_token: "rt".to_string(),
            email: None,
        });
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(v["email"], "");
    }

    // ── env kill-switch + renewal-mode decision (pure; no env mutated) ──

    #[test]
    fn env_off_values_disable_the_lease() {
        for v in ["off", "OFF", "Off", " off ", "0", "false", "FALSE"] {
            assert!(env_disables_lease(Some(v)), "{v:?} must disable");
        }
    }

    #[test]
    fn env_other_values_leave_lease_enabled() {
        for v in [None, Some(""), Some("on"), Some("1"), Some("true"), Some("offf")] {
            assert!(!env_disables_lease(v), "{v:?} must NOT disable");
        }
    }

    #[test]
    fn renewal_mode_env_wins_over_session() {
        // Kill-switch beats an existing session — hosted images can force
        // the lease off even on a signed-in box.
        assert_eq!(
            decide_renewal_mode(Some("off"), true),
            RenewalMode::DisabledByEnv
        );
        assert_eq!(
            decide_renewal_mode(Some("off"), false),
            RenewalMode::DisabledByEnv
        );
    }

    #[test]
    fn renewal_mode_no_session_is_a_clean_skip() {
        assert_eq!(decide_renewal_mode(None, false), RenewalMode::NoSession);
    }

    #[test]
    fn renewal_mode_enabled_with_session_and_no_kill_switch() {
        assert_eq!(decide_renewal_mode(None, true), RenewalMode::Enabled);
        assert_eq!(decide_renewal_mode(Some("on"), true), RenewalMode::Enabled);
    }

    // ── TokenKeeper: the refresh-loop fix (0.46.6) ──────────────────────

    use std::cell::{Cell, RefCell};
    use std::time::Instant;

    /// A fake account server: each refresh hands out `rt-<n>` / `at-<n>`
    /// with a one-hour access token, and records what it was given.
    struct FakeServer {
        calls: RefCell<Vec<String>>,
        fail: Cell<bool>,
    }

    impl FakeServer {
        fn new() -> Self {
            Self { calls: RefCell::new(Vec::new()), fail: Cell::new(false) }
        }
        fn refresh(&self, rt: &str) -> Result<Refreshed, String> {
            self.calls.borrow_mut().push(rt.to_string());
            if self.fail.get() {
                return Err("refresh request failed: connection refused".to_string());
            }
            let n = self.calls.borrow().len();
            Ok(Refreshed {
                access: format!("at-{n}"),
                refresh: format!("rt-{n}"),
                expires_in: Some(3600),
            })
        }
        fn count(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    #[test]
    fn a_valid_access_token_is_reused_for_an_hour_of_ticks() {
        let server = FakeServer::new();
        let mut keeper = TokenKeeper::default();
        let mut stored = "rt-0".to_string();
        let t0 = Instant::now();
        // 60 ticks, one per RENEW_INTERVAL: the old code refreshed 60 times.
        for i in 0..60u32 {
            let now = t0 + RENEW_INTERVAL * i;
            let at = keeper
                .access_token(&stored.clone(), now, |rt| server.refresh(rt), |rt| {
                    stored = rt.to_string();
                    true
                })
                .expect("access token");
            assert!(at.starts_with("at-"), "{at}");
        }
        // One refresh at the start, one when the first token neared expiry
        // (55 min in), never one per tick.
        assert_eq!(server.count(), 2, "calls: {:?}", server.calls.borrow());
        assert_eq!(*server.calls.borrow(), vec!["rt-0".to_string(), "rt-1".to_string()]);
        assert_eq!(stored, "rt-2", "every rotation was persisted");
    }

    #[test]
    fn refresh_happens_only_inside_the_margin() {
        let server = FakeServer::new();
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let ok = |_: &str| true;
        keeper.access_token("rt-0", t0, |rt| server.refresh(rt), ok).unwrap();
        // Just outside the margin: no refresh.
        let before = t0 + Duration::from_secs(3600) - REFRESH_MARGIN - Duration::from_secs(1);
        keeper.access_token("rt-1", before, |rt| server.refresh(rt), ok).unwrap();
        assert_eq!(server.count(), 1);
        // Inside it: refresh with the newest token.
        let inside = t0 + Duration::from_secs(3600) - REFRESH_MARGIN + Duration::from_secs(1);
        let at = keeper.access_token("rt-1", inside, |rt| server.refresh(rt), ok).unwrap();
        assert_eq!(at, "at-2");
        assert_eq!(server.calls.borrow().last().map(String::as_str), Some("rt-1"));
    }

    #[test]
    fn failed_refreshes_back_off_and_do_not_retry_every_tick() {
        let server = FakeServer::new();
        server.fail.set(true);
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let ok = |_: &str| true;
        // Two hours of 60 s ticks against a dead server.
        let mut errors = 0;
        for i in 0..120u32 {
            let now = t0 + RENEW_INTERVAL * i;
            if keeper.access_token("rt-0", now, |rt| server.refresh(rt), ok).is_err() {
                errors += 1;
            }
        }
        assert_eq!(errors, 120, "no access token without a successful refresh");
        // Attempts at 0, 1, 3, 7, 15, 31, 61 and 91 minutes (doubling to the
        // 30-minute cap), not 120.
        assert_eq!(server.count(), 8, "attempts: {}", server.count());
        // Recovery: the next attempt after the backoff succeeds and resets it.
        server.fail.set(false);
        let later = t0 + Duration::from_secs(121 * 60);
        assert!(keeper.access_token("rt-0", later, |rt| server.refresh(rt), ok).is_ok());
        assert_eq!(keeper.failures, 0);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(refresh_backoff(1), RENEW_INTERVAL);
        assert_eq!(refresh_backoff(2), RENEW_INTERVAL * 2);
        assert_eq!(refresh_backoff(3), RENEW_INTERVAL * 4);
        assert_eq!(refresh_backoff(6), REFRESH_BACKOFF_MAX);
        assert_eq!(refresh_backoff(u32::MAX), REFRESH_BACKOFF_MAX);
    }

    #[test]
    fn a_failed_refresh_keeps_using_a_still_valid_token() {
        let server = FakeServer::new();
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let ok = |_: &str| true;
        keeper.access_token("rt-0", t0, |rt| server.refresh(rt), ok).unwrap();
        server.fail.set(true);
        let inside = t0 + Duration::from_secs(3600) - REFRESH_MARGIN + Duration::from_secs(1);
        let at = keeper.access_token("rt-1", inside, |rt| server.refresh(rt), ok).unwrap();
        assert_eq!(at, "at-1", "the old token is still valid for ~5 minutes");
        // And the very next tick does not hammer the server again.
        let next = inside + Duration::from_secs(10);
        keeper.access_token("rt-1", next, |rt| server.refresh(rt), ok).unwrap();
        assert_eq!(server.count(), 2);
    }

    #[test]
    fn a_failed_keychain_write_never_resends_the_spent_token() {
        let server = FakeServer::new();
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let writes = RefCell::new(0u32);
        let failing = |_: &str| {
            *writes.borrow_mut() += 1;
            false
        };
        keeper.access_token("rt-0", t0, |rt| server.refresh(rt), failing).unwrap();
        // The keychain still says rt-0 (spent). Near expiry the keeper must
        // present rt-1 from memory, and keep retrying the write.
        let inside = t0 + Duration::from_secs(3600) - REFRESH_MARGIN + Duration::from_secs(1);
        keeper.access_token("rt-0", inside, |rt| server.refresh(rt), failing).unwrap();
        assert_eq!(*server.calls.borrow(), vec!["rt-0".to_string(), "rt-1".to_string()]);
        // Two rotations later the keychain STILL holds rt-0: still recognised.
        let inside2 = inside + Duration::from_secs(3600);
        keeper.access_token("rt-0", inside2, |rt| server.refresh(rt), failing).unwrap();
        assert_eq!(server.calls.borrow().last().map(String::as_str), Some("rt-2"));
        assert!(!server.calls.borrow()[1..].iter().any(|t| t == "rt-0"), "spent token resent");
        assert!(*writes.borrow() >= 3, "the write is retried, got {}", writes.borrow());
    }

    #[test]
    fn new_material_in_the_keychain_starts_over() {
        let server = FakeServer::new();
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let ok = |_: &str| true;
        keeper.access_token("rt-0", t0, |rt| server.refresh(rt), ok).unwrap();
        // The desktop client signed in again: a token this keeper never saw.
        let at = keeper
            .access_token("fresh-sign-in", t0 + RENEW_INTERVAL, |rt| server.refresh(rt), ok)
            .unwrap();
        assert_eq!(at, "at-2");
        assert_eq!(server.calls.borrow().last().map(String::as_str), Some("fresh-sign-in"));
    }

    #[test]
    fn new_material_skips_the_backoff() {
        let server = FakeServer::new();
        server.fail.set(true);
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let ok = |_: &str| true;
        assert!(keeper.access_token("dead", t0, |rt| server.refresh(rt), ok).is_err());
        server.fail.set(false);
        // Backing off for "dead", but a new sign-in is tried at once.
        assert!(keeper.access_token("dead", t0 + Duration::from_secs(5), |rt| server.refresh(rt), ok).is_err());
        assert_eq!(server.count(), 1);
        assert!(keeper.access_token("new", t0 + Duration::from_secs(6), |rt| server.refresh(rt), ok).is_ok());
        assert_eq!(server.count(), 2);
    }

    #[test]
    fn a_401_on_the_claim_forces_one_refresh_next_tick() {
        let server = FakeServer::new();
        let mut keeper = TokenKeeper::default();
        let t0 = Instant::now();
        let ok = |_: &str| true;
        keeper.access_token("rt-0", t0, |rt| server.refresh(rt), ok).unwrap();
        keeper.expire_access(t0 + RENEW_INTERVAL);
        let at = keeper
            .access_token("rt-1", t0 + RENEW_INTERVAL * 2, |rt| server.refresh(rt), ok)
            .unwrap();
        assert_eq!(at, "at-2");
        assert_eq!(server.count(), 2);
    }
}
