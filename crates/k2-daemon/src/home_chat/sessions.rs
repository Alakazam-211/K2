//! A person's Sew session, held by the daemon (§14.1; HP-4, HP-24, HP-28).
//!
//! **One session per person**, sid `hc_<32 hex of SHA-256(external id)>`:
//! the follower's websocket and every Home chat call of that person use it.
//! It is minted by the daemon with no bearer at that moment (new: the
//! voice BFF mints only for a live request), so the rule is narrow:
//! - only `k2:owner` or `k2:connect:` (never an agent or an App user);
//! - only while the person has at least one live login session on this
//!   server ([`super::follower`]'s supervisor drops it, and revokes it by
//!   sid, within one tick of the last session ending, a disable, a removal
//!   or a lost chat grant);
//! - every mint is audited as `chat-pass`, outcome `home-chat`;
//! - never person proof (`chat_pass::mint`).
//!
//! **Renew (HP-28).** A session lasts an hour. About [`RENEW_BEFORE_SECS`]
//! before `expires_at` the daemon mints a fresh pass and calls `POST
//! /api/v1/auth/renew` on the live session; the old token stays good for
//! 60 s and the follower swaps the new one onto its websocket (op 11
//! REAUTH), so the socket never drops. A failed renew retries with backoff;
//! at expiry the daemon signs in again with `auth/k2`.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{deps, Deps};
use crate::voice::bff::{sew_refusal, Refusal, SewAnswer};

pub const AUTH_K2: &str = "/api/v1/auth/k2";
pub const AUTH_RENEW: &str = "/api/v1/auth/renew";
/// The held sessions' sid prefix (never a login session's `cu_`).
pub const SID_PREFIX: &str = "hc_";
/// Renew this long before the session's expiry (Sew's sessions last 1 h).
pub const RENEW_BEFORE_SECS: i64 = 300;
/// A session whose answer names no expiry is treated as this long.
const DEFAULT_SESSION_SECS: i64 = 3600;

/// A held Sew session.
#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    pub token: String,
    pub expires_at: i64,
    pub member_id: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("token", &"***").field("expires_at", &self.expires_at).field("member_id", &self.member_id).finish()
    }
}

impl Session {
    /// Is it time to renew (at `now`)?
    pub fn renew_due(&self, now: i64) -> bool {
        self.expires_at - now <= RENEW_BEFORE_SECS
    }

    pub fn expired(&self, now: i64) -> bool {
        self.expires_at <= now
    }
}

/// The held session's sid for `ext`.
pub fn sid_for(ext: &str) -> String {
    let hex: String = Sha256::digest(ext.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    format!("{SID_PREFIX}{}", &hex[..32])
}

/// Only box people get a held session.
pub fn may_hold(ext: &str) -> bool {
    k2_core::home_chat::is_box_person(ext)
}

pub fn unavailable(e: impl std::fmt::Display) -> Refusal {
    Refusal::new(503, "chat_unavailable", format!("K2 could not reach this server's chat: {e}")).retry(crate::voice::bff::RETRY_SECS)
}

fn session_of(a: &SewAnswer, member_fallback: &str, now: i64) -> Result<Session, Refusal> {
    let token = a
        .body
        .get("token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| unavailable("Sew answered without a session token"))?
        .to_string();
    let expires_at = a
        .body
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp())
        .unwrap_or(now + DEFAULT_SESSION_SECS);
    let member_id = match &a.body["member"]["id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => member_fallback.to_string(),
    };
    Ok(Session { token, expires_at, member_id })
}

fn mint(d: &Deps, ext: &str) -> Result<String, Refusal> {
    if !may_hold(ext) {
        return Err(Refusal::new(403, "person_login_required", "Home chat holds sessions for this server's people only"));
    }
    let sid = sid_for(ext);
    let pass = (d.mint)(ext, &sid).map_err(|e| {
        k2_core::log_debug!("[home-chat] pass for {ext}: {e}");
        Refusal::new(503, "chat_unavailable", "Chat / Voice Hosting is not enabled or not reachable on this server")
    })?;
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new("chat-pass", ext, "home-chat".to_string(), "daemon", "-", "api"));
    Ok(pass)
}

/// Sign in as `ext` (`POST /api/v1/auth/k2` with a fresh pass).
pub fn sign_in(ext: &str, now: i64) -> Result<Session, Refusal> {
    let d = deps();
    let pass = mint(&d, ext)?;
    let a = d.sew.post(None, AUTH_K2, &json!({ "pass": pass })).map_err(unavailable)?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    session_of(&a, "", now)
}

/// Renew `cur` (`POST /api/v1/auth/renew` with a fresh pass, on the live
/// session). The login continues: same member, new token and expiry.
pub fn renew(ext: &str, cur: &Session, now: i64) -> Result<Session, Refusal> {
    let d = deps();
    let pass = mint(&d, ext)?;
    let a = d.sew.post(Some(&cur.token), AUTH_RENEW, &json!({ "pass": pass })).map_err(unavailable)?;
    if !a.ok() {
        return Err(sew_refusal(&a));
    }
    session_of(&a, &cur.member_id, now)
}

/// End the held session in Sew (by its sid, through S27's outbox).
pub fn revoke(ext: &str) {
    if !k2_core::sew_revoke::writer_available() {
        return;
    }
    let op = k2_core::sew_revoke::SewSessionRevoke::K2Sid(sid_for(ext));
    if let Err(e) = k2_core::skin::with_skin_conn(|c| k2_core::sew_revoke::enqueue(c, &op)) {
        k2_core::log_debug!("[home-chat] revoke of {ext}'s held session not queued: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sids_are_stable_prefixed_and_per_person() {
        let a = sid_for("k2:owner");
        assert!(a.starts_with("hc_") && a.len() == 35, "{a}");
        assert_eq!(a, sid_for("k2:owner"));
        assert_ne!(a, sid_for("k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(may_hold("k2:owner") && may_hold("k2:connect:x"));
        assert!(!may_hold("k2:workspace:x") && !may_hold("k2:principal:x"));
    }

    #[test]
    fn renew_is_due_five_minutes_before_expiry() {
        let s = Session { token: "sews_a.b".into(), expires_at: 10_000, member_id: "5".into() };
        assert!(!s.renew_due(10_000 - 301));
        assert!(s.renew_due(10_000 - 300));
        assert!(!s.expired(9_999) && s.expired(10_000));
        assert!(!format!("{s:?}").contains("sews_"), "the token never prints");
    }
}
