//! Per-session STREAM tokens for the external `/v1/sandboxes` API (P3b).
//!
//! ## Why this exists
//!
//! `/cli/sessions/grid` + `/cli/sessions/bytes` gate on `token_ok` — the OWNER
//! token or a connect-user session. An external `/v1/sandboxes` caller holds
//! NEITHER: it authenticates the spawn with an API key, and we must NOT broaden
//! that API key to the raw PTY streams (it would then stream EVERY session, not
//! just the one it spawned). So `/v1/sandboxes` mints a **per-session stream
//! token** bound to the EXACT `session_id` it just spawned, and hands the caller
//! `…/grid?session=<id>&token=<streamTok>`. The token authorizes ONLY that one
//! session's grid/bytes — never spawn, never another session, never the API key
//! surface.
//!
//! ## Token model
//!
//! A high-entropy opaque secret (`k2st_` + 256-bit CSPRNG hex) mapped to a
//! `(session_id, expiry)` grant in an in-memory registry. Validation looks the
//! presented token up and checks (a) the grant's session == the `?session=` the
//! request is serving, and (b) the grant has not expired. The secret is 256-bit,
//! so the (non-constant-time) HashMap lookup is not a practical timing oracle —
//! the SAME shape `connect_users::validate_session` uses.
//!
//! ## Lifetime
//!
//! In-memory ONLY (no disk): a stream token is meaningless once the daemon
//! restarts (the live session is gone). The real boundary is the spawned
//! session's lifetime — the child-exit observer calls [`revoke_for_session`] on
//! ChildExit, and once the session is unregistered grid/bytes can't find it
//! anyway. The [`TTL_HOURS`] expiry is a generous backstop for a session that
//! outlives its observer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};

use k2_core::session::SessionId;

/// Backstop TTL. The live boundary is session teardown ([`revoke_for_session`]);
/// this only bounds a token whose session somehow outlives its observer.
const TTL_HOURS: i64 = 24;

/// A live grant: the session this token streams + when it stops validating.
#[derive(Debug, Clone)]
struct StreamGrant {
    session_id: String,
    expires_at: DateTime<Utc>,
}

/// In-memory map of `secret → grant`. No disk: ephemeral by design.
#[derive(Debug, Default)]
struct StreamTokenRegistry {
    grants: HashMap<String, StreamGrant>,
}

impl StreamTokenRegistry {
    fn mint(&mut self, session_id: &SessionId) -> String {
        // Opportunistic prune so a long-lived daemon doesn't accumulate dead
        // grants (every mint is a fresh API session, so this stays bounded).
        let now = Utc::now();
        self.grants.retain(|_, g| g.expires_at > now);

        let secret = format!("k2st_{}", new_secret());
        self.grants.insert(
            secret.clone(),
            StreamGrant {
                session_id: session_id.to_string(),
                expires_at: now + Duration::hours(TTL_HOURS),
            },
        );
        secret
    }

    fn authorizes_session(&self, token: &str, session_id: &SessionId) -> bool {
        let token = token.trim();
        if token.is_empty() {
            return false;
        }
        match self.grants.get(token) {
            Some(g) => g.session_id == session_id.to_string() && g.expires_at > Utc::now(),
            None => false,
        }
    }

    fn revoke_for_session(&mut self, session_id: &SessionId) {
        let sid = session_id.to_string();
        self.grants.retain(|_, g| g.session_id != sid);
    }
}

fn registry() -> &'static Mutex<StreamTokenRegistry> {
    static REGISTRY: OnceLock<Mutex<StreamTokenRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(StreamTokenRegistry::default()))
}

/// Mint a fresh stream token bound to `session_id`. The returned `k2st_…` secret
/// is the ONLY copy — it's never persisted and never logged.
pub fn mint(session_id: &SessionId) -> String {
    let mut g = registry().lock().unwrap_or_else(|e| e.into_inner());
    g.mint(session_id)
}

/// True iff `token` is a live stream token bound to EXACTLY `session_id`. A
/// token minted for session A presenting `?session=B` → `false` (scoping
/// enforced); an expired/unknown/empty token → `false`.
pub fn authorizes_session(token: &str, session_id: &SessionId) -> bool {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .authorizes_session(token, session_id)
}

/// True iff the RAW query carries a `token=` that is a live stream token scoped
/// to the `session=` UUID in the SAME query. Used by the grid/bytes dispatcher
/// gate so an external caller streams with the per-session token. Both values
/// are parsed straight from the query (a session UUID and the `k2st_…` token
/// contain no URL-escapes).
pub fn query_authorizes(query: &str) -> bool {
    let Some(token) = query_value(query, "token") else {
        return false;
    };
    let Some(session) = query_value(query, "session") else {
        return false;
    };
    match SessionId::parse(session) {
        Some(sid) => authorizes_session(token, &sid),
        None => false,
    }
}

/// Drop every grant for `session_id` (called on ChildExit). Cheap in-memory
/// no-op for the overwhelming majority of sessions that never minted one.
pub fn revoke_for_session(session_id: &SessionId) {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .revoke_for_session(session_id);
}

/// Extract a single `key=value` from a raw `&`-joined query string.
fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        if k == key {
            it.next()
        } else {
            None
        }
    })
}

/// 256-bit CSPRNG secret, hex-encoded (mirrors `session_token::new_secret`).
fn new_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("getrandom for stream token secret");
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_then_authorizes_only_its_own_session() {
        let mut reg = StreamTokenRegistry::default();
        let a = SessionId::new();
        let b = SessionId::new();
        let tok = reg.mint(&a);

        assert!(tok.starts_with("k2st_"), "token is prefixed for debuggability");
        assert!(
            reg.authorizes_session(&tok, &a),
            "a stream token must authorize the session it was minted for",
        );
        assert!(
            !reg.authorizes_session(&tok, &b),
            "a stream token minted for A must NOT authorize session B (scoping)",
        );
    }

    #[test]
    fn rejects_unknown_empty_and_garbage() {
        let reg = StreamTokenRegistry::default();
        let s = SessionId::new();
        assert!(!reg.authorizes_session("", &s), "empty token never authorizes");
        assert!(!reg.authorizes_session("   ", &s), "blank token never authorizes");
        assert!(
            !reg.authorizes_session("k2st_not-a-real-token", &s),
            "an unminted token never authorizes",
        );
    }

    #[test]
    fn revoke_for_session_invalidates_the_token() {
        let mut reg = StreamTokenRegistry::default();
        let s = SessionId::new();
        let tok = reg.mint(&s);
        assert!(reg.authorizes_session(&tok, &s), "valid before revoke");

        reg.revoke_for_session(&s);
        assert!(
            !reg.authorizes_session(&tok, &s),
            "a revoked (torn-down) session's stream token must stop authorizing",
        );
    }

    #[test]
    fn expired_grant_does_not_authorize() {
        let mut reg = StreamTokenRegistry::default();
        let s = SessionId::new();
        let tok = reg.mint(&s);
        // Force the grant's expiry into the past.
        reg.grants
            .get_mut(&tok)
            .expect("grant present")
            .expires_at = Utc::now() - Duration::seconds(1);
        assert!(
            !reg.authorizes_session(&tok, &s),
            "an expired stream token must not authorize",
        );
    }

    #[test]
    fn query_value_extracts_keys() {
        assert_eq!(query_value("session=abc&token=xyz", "token"), Some("xyz"));
        assert_eq!(query_value("session=abc&token=xyz", "session"), Some("abc"));
        assert_eq!(query_value("session=abc", "token"), None);
        // Only the first '=' splits — a token containing '=' survives.
        assert_eq!(query_value("token=a=b", "token"), Some("a=b"));
    }

    #[test]
    fn query_authorizes_round_trips_through_the_process_registry() {
        // Exercises the process-wide singleton path the dispatcher gate uses.
        let s = SessionId::new();
        let tok = mint(&s);
        let q = format!("session={s}&token={tok}");
        assert!(query_authorizes(&q), "minted token must authorize its own session in the query gate");

        // Wrong session in the query → refused even with a valid token.
        let other = SessionId::new();
        let q_wrong = format!("session={other}&token={tok}");
        assert!(!query_authorizes(&q_wrong), "token scoped to S must not authorize session O");

        // Missing token / missing session → refused.
        assert!(!query_authorizes(&format!("session={s}")));
        assert!(!query_authorizes(&format!("token={tok}")));

        revoke_for_session(&s);
        assert!(!query_authorizes(&q), "after revoke the query gate refuses");
    }

    #[test]
    fn an_api_key_string_is_never_accepted_on_the_stream_gate() {
        // The load-bearing /v1 invariant: an API key (`k2sk_…`) is NOT a stream
        // token, so it never authorizes grid/bytes — even naming the real
        // session. Streaming must use the per-session stream token only. (The
        // dispatcher also rejects it via token_ok, which an API key fails too.)
        let s = SessionId::new();
        let _real = mint(&s); // a real grant exists for this session…
        let api_key = "k2sk_pretend_api_key_value";
        assert!(
            !authorizes_session(api_key, &s),
            "an API key must NEVER authorize a session's stream",
        );
        assert!(
            !query_authorizes(&format!("session={s}&token={api_key}")),
            "an API key in the stream query is rejected by the gate",
        );
        revoke_for_session(&s);
    }
}
