use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use super::keychain;
use super::types::{CompanionState, Session};

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
        crate::log_debug!("[companion] Keychain unavailable — continuing to read from settings.json");
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
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Parse Bearer token header: "Bearer <token>"
pub fn parse_bearer(header: &str) -> Option<String> {
    header.strip_prefix("Bearer ").map(|s| s.to_string())
}
