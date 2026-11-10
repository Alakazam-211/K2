//! Keeping a refreshing sign-in (`Sign::OauthRefresh`) fresh
//! (prd-vault-for-agents-v1 §6.2). `fetch`, `proxy` and `run` call
//! [`access_token`] instead of pasting the stored value.
//!
//! - **Fast path:** the token has at least 5 more minutes (or half its
//!   life, for a short-lived one), so no lock and no network.
//! - **Otherwise:** one refresh per secret at a time, with a double-check
//!   under the lock, so parallel uses make one POST.
//! - **`invalid_grant` / `invalid_client`:** the sign-in is `expired`, uses
//!   get `oauth_expired` (exit 3), and one reconnect card goes to the Thread.
//! - **Any other failure:** a token that still works is used; if none does,
//!   the use is refused. The failure is remembered for 30 s so an outage
//!   costs one POST, not one per use.
//!
//! **Nothing a token endpoint sends is repeated.** It might echo a token, so
//! errors, logs and audit rows carry only a word from a fixed list
//! ([`outcome_word`]), the secret's name and the endpoint's host.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use k2_vault::inject::{oauth_token_ok, Inject, Sign};
use k2_vault::vault::OauthCurrent;
use k2_vault::{Actor, LocalVault, ScopeKind, SecretBytes, SecretMeta, SecretStore, Selector, VaultError};
use parking_lot::Mutex;

use crate::mail::oauth::{refresh_lock_for, refresh_with, HttpClient, OauthError, Tokens};

/// Refresh when the token has less than this left (the mail engine uses 60 s).
pub const REFRESH_WINDOW_SECS: i64 = 300;
/// A failed refresh isn't tried again for this long.
const FAILURE_COOLDOWN: Duration = Duration::from_secs(30);
/// A 401 forces a refresh at most this often, so a URL that always answers
/// 401 can't turn every fetch into a token POST.
const FORCE_MIN_GAP: Duration = Duration::from_secs(60);
/// Used when the endpoint doesn't say how long the token lasts. A token that
/// dies sooner comes back as a 401, which refreshes it.
const DEFAULT_LIFETIME_SECS: i64 = 3600;
const MAX_LIFETIME_SECS: i64 = 366 * 24 * 3600;

/// The token endpoint and the reconnect card, separable for tests.
pub struct Deps<'a> {
    pub http: &'a dyn HttpClient,
    /// The workspace making this use: where a server-wide secret's reconnect
    /// card goes (a server-wide secret has no Thread of its own).
    pub asker_workspace: &'a str,
    /// Post a reconnect card: (workspace, scope word, secret).
    pub post_card: &'a (dyn Fn(&str, &str, &SecretMeta) -> Result<(), String> + Sync),
}

/// The real card poster.
pub fn post_live_card(workspace: &str, scope_word: &str, m: &SecretMeta) -> Result<(), String> {
    crate::overlay_routes::post_oauth_reconnect_card(workspace, scope_word, m)
}

/// Does this secret refresh itself?
pub fn is_oauth(inject: &Inject) -> bool {
    inject.sign.as_ref().is_some_and(Sign::is_oauth_refresh)
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn lock_key(secret_id: &str) -> String {
    format!("vault:{secret_id}")
}

fn failures() -> &'static Mutex<HashMap<String, (Instant, VaultError)>> {
    static M: OnceLock<Mutex<HashMap<String, (Instant, VaultError)>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn forced() -> &'static Mutex<HashMap<String, Instant>> {
    static M: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Every log line this module writes, so a test can read them.
#[cfg(test)]
pub(crate) fn test_log_lines() -> &'static Mutex<Vec<String>> {
    static L: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Vec::new()))
}

fn log(name: &str, what: &str) {
    let line = format!("[vault-oauth] {name}: {what}");
    #[cfg(test)]
    test_log_lines().lock().push(line.clone());
    k2_core::log_debug!("{line}");
}

fn expired_error(name: &str) -> VaultError {
    VaultError::forbidden(
        "oauth_expired",
        format!("{name}: expired: sign in again. A person saves a new sign-in on the reconnect card in this Thread, or runs k2 vault rotate {name} < sign-in.json"),
    )
}

/// The word logs, audit rows and error text use for a failed refresh. A
/// fixed list: `error` comes from the endpoint and might hold anything.
fn outcome_word(e: &OauthError) -> &'static str {
    const KNOWN: &[&str] = &[
        "invalid_grant",
        "invalid_client",
        "invalid_request",
        "invalid_scope",
        "unauthorized_client",
        "unsupported_grant_type",
        "access_denied",
        "temporarily_unavailable",
        "server_error",
    ];
    match e {
        OauthError::Provider { error, .. } => KNOWN.iter().find(|k| *k == error).copied().unwrap_or("provider_error"),
        OauthError::Http(_) => "transport",
        OauthError::Parse(_) => "bad_response",
        OauthError::Egress { code, .. } => code,
        OauthError::Config(_) => "config",
        OauthError::Vault(_) | OauthError::State => "internal",
    }
}

/// The endpoint refused this refresh token for good.
fn is_dead_sign_in(e: &OauthError) -> bool {
    matches!(e, OauthError::Provider { error, .. } if error == "invalid_grant" || error == "invalid_client")
}

fn fresh(cur: &OauthCurrent, now: i64) -> bool {
    let (Some(_), Some(exp)) = (&cur.access, cur.expires_at) else { return false };
    let window = match cur.issued_at {
        Some(i) if exp > i => REFRESH_WINDOW_SECS.min((exp - i) / 2).max(1),
        _ => REFRESH_WINDOW_SECS,
    };
    now < exp - window
}

/// The current token if it hasn't actually expired yet.
fn still_valid(cur: &OauthCurrent, now: i64) -> Option<SecretBytes> {
    match (&cur.access, cur.expires_at) {
        (Some(a), Some(exp)) if now < exp => Some(a.duplicate()),
        _ => None,
    }
}

enum Refreshed {
    New(SecretBytes),
    /// The refresh failed, but the current token still works.
    StillValid(SecretBytes),
    /// No usable token, for now.
    Failed(VaultError),
    /// Someone has to sign in again.
    Expired(VaultError),
}

fn remember_failure(secret_id: &str, e: &VaultError) {
    let mut f = failures().lock();
    f.retain(|_, (at, _)| at.elapsed() < FAILURE_COOLDOWN);
    f.insert(secret_id.to_string(), (Instant::now(), e.clone()));
}

fn recent_failure(secret_id: &str) -> Option<VaultError> {
    failures().lock().get(secret_id).filter(|(at, _)| at.elapsed() < FAILURE_COOLDOWN).map(|(_, e)| e.clone())
}

/// Post the reconnect card, unless one is already waiting. Card trouble
/// doesn't change the answer the caller gets; it is logged.
fn ask_to_sign_in_again(vault: &LocalVault, secret_id: &str, deps: &Deps<'_>) {
    let m = match vault.meta(&Selector::Id(secret_id.to_string()), &Actor::System("oauth-refresh".into())) {
        Ok(m) => m,
        Err(e) => return log(secret_id, &format!("no reconnect card: {}", e.code)),
    };
    let (scope_word, workspace) = match m.scope.kind {
        ScopeKind::Workspace => ("workspace", m.scope.id.clone()),
        ScopeKind::Server => ("server", deps.asker_workspace.to_string()),
        // A session's secret ends with the session; the agent asks again.
        _ => return log(&m.name, "no reconnect card: it is kept for one session"),
    };
    // A card proposes exact sites only, and a card that narrowed a person's
    // wildcard would change where the secret goes. `k2 vault rotate` still works.
    if m.hosts.iter().any(|h| h.contains('*')) {
        return log(&m.name, "no reconnect card: its sites have a wildcard");
    }
    match vault.requests(Some(&workspace)) {
        Ok(rows) if rows.iter().any(|r| r.status == "pending" && r.name == m.name && r.proposed_scope == scope_word) => return,
        Ok(_) => {}
        Err(e) => return log(&m.name, &format!("no reconnect card: {}", e.code)),
    }
    if let Err(why) = (deps.post_card)(&workspace, scope_word, &m) {
        // `why` comes from K2's own card path and holds names only.
        log(&m.name, &format!("reconnect card not posted: {why}"));
    }
}

fn store_new(vault: &LocalVault, cur: &OauthCurrent, t: Tokens, now: i64) -> Refreshed {
    let word = |w: &str| format!("error:{w}");
    let failed = |e: VaultError, outcome: String| {
        vault.oauth_audit(&cur.secret_id, &outcome);
        log(&cur.name, &format!("refresh {outcome}"));
        remember_failure(&cur.secret_id, &e);
        match still_valid(cur, now) {
            Some(a) => Refreshed::StillValid(a),
            None => Refreshed::Failed(e),
        }
    };
    let bad_response = || VaultError::new("oauth_refresh_failed", 502, format!("{}: the token endpoint's answer can't be used (bad_response); K2 tries again in a moment", cur.name));
    let refresh = match t.refresh_token.as_deref().filter(|r| !r.is_empty()) {
        Some(r) if oauth_token_ok(r.as_bytes()) => SecretBytes::from_slice(r.as_bytes()),
        Some(_) => return failed(bad_response(), word("bad_response")),
        // Google keeps the old one (the field is absent); Microsoft rotates it.
        None => match &cur.refresh {
            Some(r) => r.duplicate(),
            None => return failed(bad_response(), word("bad_response")),
        },
    };
    if !oauth_token_ok(t.access_token.as_bytes()) {
        return failed(bad_response(), word("bad_response"));
    }
    let access = SecretBytes::from_slice(t.access_token.as_bytes());
    let lifetime = if t.expires_in > 0 { t.expires_in.min(MAX_LIFETIME_SECS) } else { DEFAULT_LIFETIME_SECS };
    match vault.oauth_store_refresh(&cur.secret_id, cur.version, &access, &refresh, now + lifetime) {
        Ok(true) => {
            failures().lock().remove(&cur.secret_id);
            vault.oauth_audit(&cur.secret_id, "ok");
            log(&cur.name, "refresh ok");
            Refreshed::New(access)
        }
        // A person saved a new sign-in while this one refreshed: theirs wins.
        Ok(false) => Refreshed::Failed(VaultError::conflict("oauth_changed", format!("{}'s sign-in changed while K2 refreshed it; try again", cur.name))),
        Err(e) => failed(e, word("store")),
    }
}

/// Refresh now. The caller holds the secret's lock.
fn refresh_locked(vault: &LocalVault, cur: &OauthCurrent, deps: &Deps<'_>) -> Refreshed {
    let now = unix_now();
    let hold = |e: VaultError| match still_valid(cur, now) {
        Some(a) => Refreshed::StillValid(a),
        None => Refreshed::Failed(e),
    };
    if let Some(e) = recent_failure(&cur.secret_id) {
        return hold(e);
    }
    // A box move froze the vault: a refresh couldn't be stored, and a rotating
    // provider would have revoked the old token by then.
    if cur.frozen {
        return hold(VaultError::new("vault_moved_out", 423, format!("{} can't refresh here: this vault has moved to another server", cur.name)));
    }
    let Some(refresh) = cur.refresh.as_ref().and_then(|r| r.expose_str()) else {
        return Refreshed::Failed(VaultError::internal("the sign-in has no refresh token"));
    };
    let secret = cur.client_secret.as_ref().and_then(|s| s.expose_str());
    match refresh_with(&cur.token_url, &cur.client_id, secret, cur.client_auth, refresh, deps.http) {
        Ok(t) => store_new(vault, cur, t, now),
        Err(e) if is_dead_sign_in(&e) => {
            let word = outcome_word(&e);
            match vault.oauth_mark_expired(&cur.secret_id, cur.version, word) {
                Ok(true) => {
                    vault.oauth_audit(&cur.secret_id, "expired");
                    log(&cur.name, "refresh expired");
                    ask_to_sign_in_again(vault, &cur.secret_id, deps);
                    Refreshed::Expired(expired_error(&cur.name))
                }
                Ok(false) => Refreshed::Failed(VaultError::conflict("oauth_changed", format!("{}'s sign-in changed while K2 refreshed it; try again", cur.name))),
                Err(err) => Refreshed::Failed(err),
            }
        }
        Err(e) => {
            let word = outcome_word(&e);
            vault.oauth_audit(&cur.secret_id, &format!("error:{word}"));
            log(&cur.name, &format!("refresh error:{word}"));
            let err = match e {
                OauthError::Egress { code, status, hint } => VaultError::new(code, status, format!("{}'s token endpoint: {hint}", cur.name)),
                _ => VaultError::new("oauth_refresh_failed", 502, format!("{}: the token endpoint didn't refresh it ({word}); K2 tries again in a moment", cur.name)),
            };
            remember_failure(&cur.secret_id, &err);
            hold(err)
        }
    }
}

/// The access token to send for this secret, refreshing it if needed. Call
/// it after `resolve` has allowed the use.
pub fn access_token(vault: &LocalVault, secret_id: &str, deps: &Deps<'_>) -> Result<SecretBytes, VaultError> {
    let cur = vault.oauth_current(secret_id)?;
    if cur.expired {
        return Err(expired_error(&cur.name));
    }
    if fresh(&cur, unix_now()) {
        return cur.access.ok_or_else(|| VaultError::internal("no access token"));
    }
    let lock = refresh_lock_for(&lock_key(secret_id));
    let _guard = lock.lock();
    // A sibling may have refreshed while this call waited.
    let cur = vault.oauth_current(secret_id)?;
    if cur.expired {
        return Err(expired_error(&cur.name));
    }
    if fresh(&cur, unix_now()) {
        return cur.access.ok_or_else(|| VaultError::internal("no access token"));
    }
    match refresh_locked(vault, &cur, deps) {
        Refreshed::New(a) | Refreshed::StillValid(a) => Ok(a),
        Refreshed::Failed(e) | Refreshed::Expired(e) => Err(e),
    }
}

/// The upstream answered 401 to `rejected`. Returns a different token to
/// retry with, or `None` to hand the 401 back. Parallel uses that all got
/// the 401 share one refresh: whoever comes second finds a newer token.
pub fn refresh_after_401(vault: &LocalVault, secret_id: &str, rejected: &SecretBytes, deps: &Deps<'_>) -> Result<Option<SecretBytes>, VaultError> {
    let lock = refresh_lock_for(&lock_key(secret_id));
    let _guard = lock.lock();
    let cur = vault.oauth_current(secret_id)?;
    if cur.expired {
        return Err(expired_error(&cur.name));
    }
    if let Some(a) = cur.access.as_ref().filter(|a| a.expose() != rejected.expose()) {
        return Ok(Some(a.duplicate()));
    }
    {
        let mut f = forced().lock();
        f.retain(|_, at| at.elapsed() < FORCE_MIN_GAP);
        if f.contains_key(secret_id) {
            return Ok(None);
        }
        f.insert(secret_id.to_string(), Instant::now());
    }
    match refresh_locked(vault, &cur, deps) {
        Refreshed::New(a) => Ok(Some(a)),
        Refreshed::StillValid(_) | Refreshed::Failed(_) => Ok(None),
        Refreshed::Expired(e) => Err(e),
    }
}

#[cfg(test)]
#[path = "oauth_refresh_tests.rs"]
pub(crate) mod tests;
