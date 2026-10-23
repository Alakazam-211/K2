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

/// Supabase project that backs the k2.dev account (mirrors the renderer's
/// `k2-account.ts` — same project, same RPC). Kept in lockstep with the
/// client so the daemon hits the identical control-plane contract.
const SUPABASE_URL: &str = "https://ttgcalfrzzgkxnfepkiu.supabase.co";

/// Public `anon` key for the project. Safe to embed in a client (it only
/// authorizes the unauthenticated role; row access is RLS-scoped to the
/// signed-in caller via the bearer access token). Identical to the value
/// shipped in the renderer.
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6InR0Z2NhbGZyenpna3huZmVwa2l1Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODA1MDIyMzksImV4cCI6MjA5NjA3ODIzOX0.L28xgtYkPEj5eCNDGO5Zf5xxhdKQLxKD8c1CJRHNqI8";

// The account-session keychain item (constants, read/write/clear, the
// trusted-app ACL) lives in `k2_client::account_session` so the thin client
// writes it with the same code. Re-exported here so every
// `tunnel::lease::*` path keeps working.
pub use k2_client::account_session::{
    bundle_peer_binaries, clear_account_session, read_account_session, resolve_account_session,
    session_blob_json, write_account_session, AccountSession,
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

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaimRow {
    #[serde(default)]
    claimed: bool,
}

/// Exchange a refresh token for a fresh `(access_token, refresh_token)`.
/// Mirrors `refreshSession` in the renderer. The refresh token may rotate
/// — the caller MUST persist the returned one.
fn refresh_access_token(refresh_token: &str) -> Result<(String, String), String> {
    let client = http_client()?;
    let resp = client
        .post(format!(
            "{SUPABASE_URL}/auth/v1/token?grant_type=refresh_token"
        ))
        .header("apikey", SUPABASE_ANON_KEY)
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
        .map_err(|e| format!("refresh response parse failed: {e}"))?;
    match (body.access_token, body.refresh_token) {
        (Some(a), Some(r)) => Ok((a, r)),
        _ => Err("refresh response missing access/refresh token".to_string()),
    }
}

/// Re-POST the `claim_subdomain` RPC — the heartbeat. Mirrors
/// `claimSubdomain` in the renderer byte-for-byte on the wire (same RPC,
/// same params). Returns `Ok(true)` when THIS device holds the lease after
/// the call (the heartbeat landed), `Ok(false)` when another device holds
/// a fresh claim, `Err` on transport/HTTP failure.
fn claim_subdomain(
    access_token: &str,
    label: &str,
    device_id: &str,
    device_label: Option<&str>,
) -> Result<bool, String> {
    let client = http_client()?;
    let resp = client
        .post(format!("{SUPABASE_URL}/rest/v1/rpc/claim_subdomain"))
        .header("apikey", SUPABASE_ANON_KEY)
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
        .map_err(|e| format!("claim request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("claim rejected (HTTP {})", resp.status()));
    }
    let text = resp
        .text()
        .map_err(|e| format!("claim response read failed: {e}"))?;
    let rows: Vec<ClaimRow> = serde_json::from_str(&text)
        .map_err(|e| format!("claim response parse failed: {e}"))?;
    Ok(rows.first().map(|r| r.claimed).unwrap_or(false))
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client build failed: {e}"))
}

/// Perform one full renewal cycle for `target`: read the keychain session,
/// refresh its token to an access token (persisting any rotation), and
/// re-claim the subdomain. Returns `Ok(true)` when the lease was renewed
/// (this device holds it), `Ok(false)` when another device now holds it
/// (caller may choose to log/stop), `Err` on a transport/auth failure (a
/// transient error — the next tick retries).
///
/// This is the network-touching entry point; it is NEVER exercised by unit
/// tests (those cover the pure seams: [`cadence_is_safe`],
/// [`LeaseTarget::from_config`], [`resolve_account_session`],
/// [`decide_renewal_mode`]).
pub fn renew_once(target: &LeaseTarget) -> Result<bool, String> {
    let sess = read_account_session()
        .ok_or_else(|| "no account session in keychain (signed out?)".to_string())?;
    let (access, rotated) = refresh_access_token(&sess.refresh_token)?;
    // Persist the rotated refresh token back into the SAME single-blob
    // layout the renderer reads first — preserving the email field — so
    // neither side presents a spent token on its next refresh.
    if rotated != sess.refresh_token {
        write_account_session(&AccountSession {
            refresh_token: rotated,
            email: sess.email.clone(),
        });
    }
    claim_subdomain(
        &access,
        &target.label,
        &target.device_id,
        target.device_label.as_deref(),
    )
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
}
