use super::keychain;
use super::types::{CompanionState, Session};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

/// Hash a password using argon2id.
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("Password hashing failed: {}", e))
}

/// Verify a password against an argon2 hash.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Load the companion password hash, preferring the macOS Keychain over the
/// legacy on-disk copy. Performs opportunistic one-shot migration: if the
/// hash lives only on disk, copy it to Keychain and clear it from
/// settings.json so future verifications read from the Keychain only.
///
/// Returns None if no password is configured.
pub fn load_password_hash() -> Option<String> {
    if let Some(hash) = keychain::read_password_hash() {
        return Some(hash);
    }

    // Fallback + migration for pre-0.32.12 installs.
    let snap = super::settings_bridge::read_settings();
    let legacy = snap.password_hash.clone();
    if legacy.is_empty() {
        return None;
    }

    // Try to migrate: Keychain first, then clear from disk.
    let migrated = keychain::write_password_hash(&legacy).is_ok();
    if migrated {
        super::settings_bridge::clear_password_hash_after_migration();
        crate::log_debug!("[companion] Migrated password hash from settings.json to Keychain");
    } else {
        crate::log_debug!(
            "[companion] Keychain unavailable — continuing to read from settings.json"
        );
    }
    Some(legacy)
}

/// Returns true iff a companion password has been configured (Keychain or
/// legacy on-disk hash). Used by `start_companion` + UI state.
pub fn has_password() -> bool {
    if keychain::read_password_hash().is_some() {
        return true;
    }
    let snap = super::settings_bridge::read_settings();
    snap.password_set || !snap.password_hash.is_empty()
}

/// Create a new authenticated session with 24hr expiry.
pub fn create_session(remote_addr: &str) -> Session {
    let now = chrono::Utc::now();
    Session {
        token: uuid::Uuid::new_v4().to_string(),
        created_at: now,
        expires_at: now + chrono::Duration::hours(24),
        last_active: now,
        remote_addr: remote_addr.to_string(),
        request_count: 0,
        window_start: std::time::Instant::now(),
    }
}

/// Validate a Bearer token against active sessions.
/// Returns the session token if valid, error message if not.
///
/// Uses constant-time comparison (subtle::ConstantTimeEq) against every stored
/// token rather than HashMap::get(), which would reveal bucket-collision timing
/// and leak via byte-wise String equality on the final compare. O(n) over active
/// sessions, which is bounded to a handful in practice.
pub fn validate_bearer(token: &str, state: &CompanionState) -> Result<String, &'static str> {
    use subtle::ConstantTimeEq;
    let token_bytes = token.as_bytes();

    let mut sessions = state.sessions.lock();

    // Find the matching session via constant-time scan.
    let mut matched_key: Option<String> = None;
    for key in sessions.keys() {
        if key.as_bytes().ct_eq(token_bytes).into() {
            matched_key = Some(key.clone());
            break;
        }
    }
    let matched_key = matched_key.ok_or("Invalid session token")?;

    let session = sessions
        .get_mut(&matched_key)
        .ok_or("Invalid session token")?;

    if session.is_expired() {
        drop(sessions);
        state.sessions.lock().remove(&matched_key);
        return Err("Session expired");
    }

    if !session.check_rate_limit() {
        return Err("Rate limit exceeded (60 requests/minute)");
    }

    session.last_active = chrono::Utc::now();
    Ok(matched_key)
}

/// Parse Basic Auth header: "Basic base64(username:password)"
pub fn parse_basic_auth(header: &str) -> Option<(String, String)> {
    let encoded = header.strip_prefix("Basic ")?;
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Parse Bearer token header: "Bearer <token>"
pub fn parse_bearer(header: &str) -> Option<String> {
    header.strip_prefix("Bearer ").map(|s| s.to_string())
}

/// Extract the public-grid credential. JS `WebSocket` cannot set
/// `Authorization`, so query `token=` is the real client path.
/// Bearer-on-upgrade is defense-in-depth for non-JS clients.
pub fn extract_grid_token(
    query_token: Option<&str>,
    authorization_header: Option<&str>,
) -> Option<String> {
    if let Some(t) = query_token.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(t.to_string());
    }
    authorization_header.and_then(parse_bearer).and_then(|t| {
        let t = t.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    })
}

/// Public-tunnel auth for `/companion/sessions/grid`.
///
/// Accepts **only** a live companion session token from `/companion/auth`.
/// Daemon hook/owner, stream (`k2st_…`), and Connect tokens are rejected
/// even if they would authorize `/cli/sessions/grid`.
pub fn authorize_grid_token(token: &str, state: &CompanionState) -> Result<String, &'static str> {
    use subtle::ConstantTimeEq;

    if token.is_empty() {
        return Err("Invalid session token");
    }
    if !state.hook_token.is_empty() && token.as_bytes().ct_eq(state.hook_token.as_bytes()).into() {
        return Err("Invalid session token");
    }
    // Stream tokens are never minted into CompanionState.sessions; reject
    // the prefix so a leaked `k2st_` cannot be confused with a companion
    // UUID even if someone stuffed it into the session map.
    if token.starts_with("k2st_") {
        return Err("Invalid session token");
    }
    validate_bearer(token, state)
}

/// Companion session still exists and is unexpired — used by the grid
/// WS re-auth tick. Does **not** increment the HTTP rate-limit counter.
pub fn session_alive(token: &str) -> bool {
    let guard = super::STATE.lock();
    match guard.as_ref() {
        Some(state) => session_alive_in(token, state),
        None => false,
    }
}

/// Same as [`session_alive`] against an explicit state (tests).
pub fn session_alive_in(token: &str, state: &CompanionState) -> bool {
    use subtle::ConstantTimeEq;
    if token.is_empty() {
        return false;
    }
    let sessions = state.sessions.lock();
    for (key, session) in sessions.iter() {
        if key.as_bytes().ct_eq(token.as_bytes()).into() {
            return !session.is_expired();
        }
    }
    false
}

#[cfg(test)]
mod grid_auth_tests {
    use super::*;
    use crate::companion::types::CompanionState;

    fn state_with_session(token: &str, hook_token: &str) -> CompanionState {
        let state = CompanionState::new(0, hook_token.to_string());
        state.sessions.lock().insert(
            token.to_string(),
            Session {
                token: token.to_string(),
                created_at: chrono::Utc::now(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
                last_active: chrono::Utc::now(),
                remote_addr: "test".into(),
                request_count: 0,
                window_start: std::time::Instant::now(),
            },
        );
        state
    }

    #[test]
    fn companion_session_token_is_accepted() {
        let state = state_with_session("comp-session-aaa", "owner-hook-secret");
        assert_eq!(
            authorize_grid_token("comp-session-aaa", &state).unwrap(),
            "comp-session-aaa"
        );
    }

    #[test]
    fn hook_owner_token_is_rejected() {
        let state = state_with_session("comp-session-aaa", "owner-hook-secret");
        assert!(authorize_grid_token("owner-hook-secret", &state).is_err());
    }

    #[test]
    fn stream_token_prefix_is_rejected() {
        let state = state_with_session("comp-session-aaa", "owner-hook-secret");
        assert!(authorize_grid_token("k2st_deadbeef", &state).is_err());
    }

    #[test]
    fn unknown_connect_looking_token_is_rejected() {
        let state = state_with_session("comp-session-aaa", "owner-hook-secret");
        assert!(authorize_grid_token("connect-user-session-xyz", &state).is_err());
    }

    #[test]
    fn missing_and_empty_tokens_rejected() {
        let state = state_with_session("comp-session-aaa", "owner-hook-secret");
        assert!(authorize_grid_token("", &state).is_err());
        assert!(extract_grid_token(None, None).is_none());
        assert!(extract_grid_token(Some(""), Some("Bearer ")).is_none());
    }

    #[test]
    fn query_token_wins_over_bearer() {
        assert_eq!(
            extract_grid_token(Some("from-query"), Some("Bearer from-header")).as_deref(),
            Some("from-query")
        );
        assert_eq!(
            extract_grid_token(None, Some("Bearer from-header")).as_deref(),
            Some("from-header")
        );
    }

    #[test]
    fn session_alive_ignores_rate_limit_and_expiry() {
        let state = state_with_session("comp-session-aaa", "owner-hook-secret");
        assert!(session_alive_in("comp-session-aaa", &state));
        assert!(!session_alive_in("missing", &state));
        assert!(!session_alive_in("", &state));

        // Force expiry — alive must go false without touching rate limit.
        state
            .sessions
            .lock()
            .get_mut("comp-session-aaa")
            .unwrap()
            .expires_at = chrono::Utc::now() - chrono::Duration::hours(1);
        assert!(!session_alive_in("comp-session-aaa", &state));
    }
}
