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
//! token** the renderer persisted to the OS keychain at sign-in
//! (`dev.k2.connect.account` / `session-refresh-token`).
//!
//! Cadence: [`RENEW_INTERVAL`] (60 s) — matches the old renderer cadence
//! and is well inside the 3-minute server TTL ([`LEASE_TTL`]).

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

/// Keychain coordinates for the k2.dev ACCOUNT session. MUST match the
/// renderer's `ACCOUNT_KEYCHAIN_SERVICE` / `SESSION_ACCOUNT_KEY` in
/// `K2ConnectSection.tsx` so the daemon reads the very token the client
/// stored at sign-in.
const ACCOUNT_KEYCHAIN_SERVICE: &str = "dev.k2.connect.account";
/// Pre-0.40 service name — read-only fallback, migrated on first read.
const LEGACY_ACCOUNT_KEYCHAIN_SERVICE: &str = "com.k2so.connect.account";
/// Human-facing item LABEL (kSecAttrLabel). The service above is the
/// stable lookup KEY; the label is the only string macOS shows in the
/// "K2 wants to use your confidential information stored in '<label>'"
/// keychain dialog. macOS owns that dialog's body text entirely (no
/// per-prompt reason string like osascript's `with prompt`), so a clear
/// label is the only lever we have to explain what's being unlocked.
const ACCOUNT_KEYCHAIN_LABEL: &str = "K2 Connect sign-in";
const ACCOUNT_REFRESH_KEY: &str = "session-refresh-token";

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

// ── Keychain (account refresh token) ────────────────────────────────────
//
// The renderer persists the refresh token via the `keyring` crate
// (apple-native on macOS), which writes a standard generic-password
// item. The daemon reads it back with the `security` CLI — the same
// mechanism `companion::keychain` already relies on for cross-process
// access to a keyring-written secret.
//
// ## Why this item prompts, and the ACL fix (0.40.7)
//
// On macOS the `keyring` crate (and the `security` CLI) operate on the
// LEGACY file-based **login keychain** via `SecKeychainAddGenericPassword`
// / `find_generic_password`. Access to an item in that keychain is gated by
// the item's **ACL** — a per-item list of *trusted applications* — NOT by
// the modern `keychain-access-groups` entitlement (that entitlement only
// governs the iOS-style data-protection keychain, which neither keyring nor
// the `security` CLI use on macOS). So the entitlement added in 0.40.6
// makes "Always Allow" survive relaunches for the *creating app's* own
// signature, but it does nothing for a DIFFERENT binary touching the item.
//
// When the renderer (`keyring`) creates the item, its default ACL trusts
// only the creating app (`K2.app/Contents/MacOS/k2`). Every OTHER process
// that touches it then provokes the login-keychain password prompt:
//
//   * the daemon READ shells `/usr/bin/security find-generic-password`, so
//     the keychain sees `/usr/bin/security` as the requester — not on the
//     ACL → prompt, and "Always Allow" can't persist a grant for a tool
//     that re-launches as a fresh process each time;
//   * even reading via the Security framework from the daemon would make
//     `k2-daemon` the requester — still a different app than the writer
//     (`k2`), still not on the ACL → still a prompt — and the `keyring`
//     crate exposes NO API to add trusted apps to an item's ACL.
//
// The only lever on the login keychain is the `security` CLI's `-T <path>`
// flag (set the item's trusted-application list at write time). So we keep
// the `security` CLI and, whenever the daemon (re)writes the item, we stamp
// it with an explicit ACL that trusts every binary that legitimately reads
// it. See [`acl_trusted_apps`] for the exact `-T` set and the per-entry
// rationale tied to which process performs each access.
//
// `-U` updates the VALUE of an existing item but does NOT reset its ACL, so
// to (re)apply the ACL we DELETE + re-ADD (a plain add — no `-U` — installs
// a fresh ACL). The token value is preserved across the delete/re-add. This
// also self-heals a legacy item the renderer created without our ACL: the
// first daemon write after updating to 0.40.7 upgrades it in place.

/// The `-T` trusted-application set stamped onto the account keychain item
/// so every binary that reads it does so WITHOUT a login-keychain prompt.
///
/// Returns absolute paths to pass as `-T <path>`. Each entry is justified by
/// the process that actually performs a keychain access against this item:
///
/// 1. `/usr/bin/security` — the daemon's READ path
///    ([`read_keychain_token`]) shells `security find-generic-password`, so
///    the keychain sees `/usr/bin/security` as the requesting application on
///    every daemon read. This is the load-bearing entry.
/// 2. the running daemon executable (`std::env::current_exe()`, i.e.
///    `K2.app/Contents/MacOS/k2-daemon`) — covers any future direct
///    Security-framework read from the daemon and the daemon's own
///    rotation/migration re-writes, so they never re-prompt.
/// 3. the app/renderer binary (`…/Contents/MacOS/k2`, the daemon exe's
///    sibling) — the renderer ORIGINALLY created the item (via `keyring`)
///    and reads it back through `k2_secret_get`. Once the daemon re-writes
///    the item with an explicit ACL, that ACL REPLACES the keyring default
///    that trusted `k2`; without re-adding `k2` here the daemon's own ACL
///    rewrite would lock the renderer OUT and re-introduce a prompt on the
///    app side. We derive its path as the daemon exe's sibling (both live in
///    `K2.app/Contents/MacOS/`) so it stays correct across versions and
///    install locations without hard-coding `/Applications`.
///
/// The CLI (`@alakazamlabs/k2`, an npm-installed bash script) does NOT read
/// this item — it has no account-session code path — so it is intentionally
/// omitted; adding a `-T` for a binary that never touches the item would be
/// dead ACL surface.
#[cfg(target_os = "macos")]
fn acl_trusted_apps() -> Vec<String> {
    let mut apps = vec!["/usr/bin/security".to_string()];
    if let Ok(exe) = std::env::current_exe() {
        // Resolve symlinks so the ACL records the real bundle path (launchd
        // may exec the daemon through a symlink).
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        // (2) the daemon itself.
        apps.push(exe.to_string_lossy().to_string());
        // (3) the app binary `k2`, the daemon exe's sibling in
        // Contents/MacOS/. Only add it when it actually exists on disk
        // (e.g. inside the .app bundle) — in a bare dev/CI build the daemon
        // has no `k2` sibling and a phantom path would just be inert.
        if let Some(dir) = exe.parent() {
            let app_bin = dir.join("k2");
            if app_bin != exe && app_bin.exists() {
                apps.push(app_bin.to_string_lossy().to_string());
            }
        }
    }
    apps
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
#[cfg(target_os = "macos")]
pub fn migrate_account_keychain() -> bool {
    // New item already present → no migration needed, but UPGRADE its ACL
    // once so the renewal loop's reads stop prompting.
    if let Some(existing) = read_keychain_token(ACCOUNT_KEYCHAIN_SERVICE) {
        crate::log_debug!(
            "[tunnel/lease] boot ACL upgrade on {ACCOUNT_KEYCHAIN_SERVICE} (re-stamp trusted-app list)"
        );
        // Re-write the SAME value to (re)install our ACL. Idempotent: once
        // the daemon is on the ACL, this re-write needs no prompt.
        write_account_refresh_token(&existing);
        return false;
    }
    match read_keychain_token(LEGACY_ACCOUNT_KEYCHAIN_SERVICE) {
        Some(legacy) => {
            crate::log_debug!(
                "[tunnel/lease] boot keychain migration {LEGACY_ACCOUNT_KEYCHAIN_SERVICE} → {ACCOUNT_KEYCHAIN_SERVICE}"
            );
            // write_account_refresh_token stamps the ACL on the NEW item, so
            // the migrated session is prompt-free from its first daemon read.
            write_account_refresh_token(&legacy);
            true
        }
        None => false,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn migrate_account_keychain() -> bool {
    false
}

/// Read the account refresh token the renderer stored at sign-in. Returns
/// `None` when absent (never signed in / signed out) so the caller can
/// skip renewal cleanly rather than erroring.
#[cfg(target_os = "macos")]
pub fn read_account_refresh_token() -> Option<String> {
    if let Some(token) = read_keychain_token(ACCOUNT_KEYCHAIN_SERVICE) {
        return Some(token);
    }
    // 0.40.0 rebrand: pre-rename sign-ins stored the item under
    // `com.k2so.connect.account`. Copy-on-read so an existing K2 Connect
    // session survives the update without re-authenticating.
    let legacy = read_keychain_token(LEGACY_ACCOUNT_KEYCHAIN_SERVICE)?;
    crate::log_debug!(
        "[tunnel/lease] migrating account refresh token {LEGACY_ACCOUNT_KEYCHAIN_SERVICE} → {ACCOUNT_KEYCHAIN_SERVICE}"
    );
    write_account_refresh_token(&legacy);
    Some(legacy)
}

#[cfg(target_os = "macos")]
fn read_keychain_token(service: &str) -> Option<String> {
    let output = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            service,
            "-a",
            ACCOUNT_REFRESH_KEY,
            "-w",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Persist a rotated refresh token back to the keychain. Supabase rotates
/// the refresh token on every refresh; if we don't write the new one back
/// the NEXT daemon refresh (and the renderer's next mount) would present a
/// spent token. Best-effort — a keychain write failure is logged and
/// swallowed (the in-memory token is still good for this run).
///
/// Every write goes through [`write_token_with_acl`], which stamps the
/// trusted-application ACL ([`acl_trusted_apps`]) so the daemon's `security`
/// reads — and the renderer's reads — never provoke a login-keychain
/// prompt. A write also UPGRADES the ACL on a legacy item the renderer
/// created without one (delete + re-add installs the fresh ACL).
#[cfg(target_os = "macos")]
pub fn write_account_refresh_token(token: &str) {
    write_token_with_acl(ACCOUNT_KEYCHAIN_SERVICE, ACCOUNT_REFRESH_KEY, token);
}

/// Write `token` under `(service, account)` with our trusted-application
/// ACL applied, idempotently and migration-safely.
///
/// `security`'s `-U` updates an existing item's VALUE but does NOT reset its
/// ACL, so we cannot rely on `-U` to upgrade a legacy item that the renderer
/// created with a default (creator-only) ACL. Instead we DELETE any existing
/// item first, then a plain ADD (no `-U`) installs a fresh item carrying the
/// `-T` ACL. Deleting then adding the SAME value preserves the token across
/// the operation; the only observable change is the (now correct) ACL.
///
/// Best-effort throughout: a delete of a missing item is fine (we ignore its
/// status), and an add failure is logged + swallowed (the in-memory token is
/// still good for this run).
#[cfg(target_os = "macos")]
fn write_token_with_acl(service: &str, account: &str, token: &str) {
    // Drop any existing item so the subsequent ADD installs our ACL rather
    // than leaving a legacy creator-only ACL in place. Ignore the status:
    // "not found" is the expected first-write case.
    let _ = std::process::Command::new("security")
        .args(["delete-generic-password", "-s", service, "-a", account])
        .output();

    let mut cmd = std::process::Command::new("security");
    cmd.args([
        "add-generic-password",
        // NOTE: no `-U` — we deleted above, and a plain add is what installs
        // a fresh ACL. `-U` would update-in-place and skip the ACL.
        "-s",
        service,
        "-a",
        account,
        // Friendly label shown in the macOS keychain access dialog instead
        // of the bare service id.
        "-l",
        ACCOUNT_KEYCHAIN_LABEL,
    ]);
    // One `-T <path>` per trusted binary (see acl_trusted_apps for the
    // rationale tied to which process performs each access).
    for app in acl_trusted_apps() {
        cmd.arg("-T").arg(app);
    }
    // `-w <token>` LAST so the secret is the final arg (never shell-parsed;
    // args are passed directly to exec, not through a shell).
    cmd.arg("-w").arg(token);

    match cmd.output() {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            crate::log_debug!(
                "[tunnel/lease] WARN: failed to persist refresh token with ACL (service={service} rc={:?})",
                out.status.code()
            );
        }
        Err(e) => {
            crate::log_debug!(
                "[tunnel/lease] WARN: keychain write spawn failed (service={service}): {e}"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read_account_refresh_token() -> Option<String> {
    // Non-macOS keychain bridge for the account session is not yet wired
    // (the renderer uses the keyring crate's Secret Service / Credential
    // Manager backends, which have no stable CLI we read here). Renewal is
    // a no-op on those platforms for now; the renderer still drives the
    // one-shot claims. Tracked as a follow-up.
    None
}

#[cfg(not(target_os = "macos"))]
pub fn write_account_refresh_token(_token: &str) {}

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

/// Perform one full renewal cycle for `target`: read the keychain refresh
/// token, refresh it to an access token (persisting any rotation), and
/// re-claim the subdomain. Returns `Ok(true)` when the lease was renewed
/// (this device holds it), `Ok(false)` when another device now holds it
/// (caller may choose to log/stop), `Err` on a transport/auth failure (a
/// transient error — the next tick retries).
///
/// This is the network-touching entry point; it is NEVER exercised by unit
/// tests (those cover [`cadence_is_safe`] / [`LeaseTarget::from_config`]).
pub fn renew_once(target: &LeaseTarget) -> Result<bool, String> {
    let refresh = read_account_refresh_token()
        .ok_or_else(|| "no account refresh token in keychain (signed out?)".to_string())?;
    let (access, rotated) = refresh_access_token(&refresh)?;
    // Persist the rotated refresh token so the next cycle (and the
    // renderer's next mount) doesn't present a spent token.
    if rotated != refresh {
        write_account_refresh_token(&rotated);
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
}
