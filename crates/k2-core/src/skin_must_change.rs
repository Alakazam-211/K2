//! Must-change-password at next sign-in for App users (0.46.6, Rosson
//! 2026-10-29; prd-password-reset-card-v1 §7).
//!
//! skin.db step 7 adds `principals.password_must_change` and the
//! `password_change_tokens` table.
//!
//! **Set** when someone else gives the person a password: K2 generated it
//! (`generate: true`), the Owner or an Admin typed it, an agent set it for
//! anyone but its own test guest, or an App key set it. **Not set** when
//! the person chooses their own (password change, an invite, reset or
//! owner-reset link, this module's change token) or an agent sets one for
//! its own test guest. **Cleared** by any password the person sets.
//!
//! While it is on, a right password at login gets no pass and no session:
//! [`start`] hands back a one-time change token (`k2chg_…`, 15 minutes,
//! single use, SHA-256 stored) that only [`consume`] takes, and only to
//! set a new password that differs from the temporary one. Any password
//! write voids the person's unused change tokens.

use rusqlite::{params, Connection, OptionalExtension};

use super::{
    clear_lockouts_for_principal_conn, hash_password, immediate_tx, invites, mint_app_session_conn, now_secs,
    password_too_long, principal_by_username, revoke_sessions_for_principal, sha256_hex, ties, verify_hash, with_conn,
    PASSWORD_TOO_LONG,
};

/// Change-token prefix: never a pass (`k2skn_`), a key (`k2sk_`), a link
/// (`k2inv_`) or a refresh token (`k2skr_`).
pub const CHANGE_PREFIX: &str = "k2chg_";
/// How long a change token lives.
pub const CHANGE_TTL_SECS: i64 = 15 * 60;

/// skin.db step 7: the flag column and the change-token table.
pub(super) fn step7_must_change(conn: &Connection) -> Result<(), String> {
    if !super::column_exists(conn, "principals", "password_must_change")? {
        conn.execute_batch("ALTER TABLE principals ADD COLUMN password_must_change INTEGER NOT NULL DEFAULT 0;")
            .map_err(|e| format!("skin.db step 7 (flag): {e}"))?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS password_change_tokens (
            id           TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            app          TEXT NOT NULL,
            token_hash   TEXT NOT NULL UNIQUE,
            created_at   INTEGER NOT NULL,
            expires_at   INTEGER NOT NULL,
            used_at      INTEGER
         );
         CREATE INDEX IF NOT EXISTS password_change_tokens_principal ON password_change_tokens(principal_id);",
    )
    .map_err(|e| format!("skin.db step 7 (tokens): {e}"))?;
    Ok(())
}

/// Turn the flag on or off for `principal_id` inside the caller's
/// transaction (the password write's), and void the person's unused
/// change tokens: they went out for the old password.
pub(crate) fn set_flag_conn(conn: &Connection, principal_id: &str, on: bool) -> Result<(), String> {
    conn.execute("UPDATE principals SET password_must_change = ?2 WHERE id = ?1", params![principal_id, on as i64])
        .map_err(|e| format!("must-change flag: {e}"))?;
    conn.execute("DELETE FROM password_change_tokens WHERE principal_id = ?1 AND used_at IS NULL", params![principal_id])
        .map_err(|e| format!("change token void: {e}"))?;
    Ok(())
}

/// Is the flag on for `principal_id`?
pub fn is_flagged(principal_id: &str) -> Result<bool, String> {
    with_conn(|conn| {
        conn.query_row("SELECT password_must_change FROM principals WHERE id = ?1", params![principal_id], |r| r.get::<_, i64>(0))
            .optional()
            .map(|v| v.unwrap_or(0) != 0)
            .map_err(|e| format!("must-change flag: {e}"))
    })
}

/// Every principal id whose flag is on (the owner's App users list).
pub fn flagged() -> Result<std::collections::HashSet<String>, String> {
    with_conn(|conn| {
        let mut st = conn
            .prepare("SELECT id FROM principals WHERE password_must_change != 0")
            .map_err(|e| format!("must-change list: {e}"))?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("must-change list: {e}"))?;
        rows.collect::<rusqlite::Result<_>>().map_err(|e| format!("must-change list: {e}"))
    })
}

fn generate_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    format!("{CHANGE_PREFIX}{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// A change token, given out once with the login answer.
#[derive(Debug, Clone)]
pub struct ChangeStart {
    pub token: String,
    pub expires_at: i64,
}

/// A right password for a flagged person on `app`: check the login would
/// be allowed (the same rules as a session mint: grant, test guest, tie;
/// the mint is rolled back), then mint a change token. Older unused change
/// tokens of the person go. `Err(HOST_LOGIN_DENIED)` when the login would
/// be refused anyway (the caller answers the generic 401).
pub fn start(username: &str, app: &str) -> Result<ChangeStart, String> {
    let username = super::normalize_username(username)?;
    let app = super::normalize_app_target(Some(app));
    let app_ws = ties::app_workspace(&app);
    let token = generate_token();
    let now = now_secs();
    let expires_at = now + CHANGE_TTL_SECS;
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(super::HOST_LOGIN_DENIED.to_string());
        };
        let mut tx = immediate_tx(conn)?;
        // Dry run: every rule a session mint has, nothing kept.
        {
            let mut sp = tx.savepoint().map_err(|e| format!("must-change dry run: {e}"))?;
            sp.set_drop_behavior(rusqlite::DropBehavior::Rollback);
            mint_app_session_conn(&sp, &username, &app, app_ws.as_deref())?;
            // `sp` drops here: rolled back.
        }
        tx.execute("DELETE FROM password_change_tokens WHERE principal_id = ?1 AND used_at IS NULL", params![p.id])
            .map_err(|e| format!("change token void: {e}"))?;
        tx.execute(
            "INSERT INTO password_change_tokens (id, principal_id, app, token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![uuid::Uuid::new_v4().to_string(), p.id, app, sha256_hex(&token), now, expires_at],
        )
        .map_err(|e| format!("change token insert: {e}"))?;
        tx.commit().map_err(|e| format!("change token commit: {e}"))?;
        Ok(ChangeStart { token: token.clone(), expires_at })
    })
}

/// What a change-token use did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeConsume {
    Ok,
    /// Unknown, used, voided, expired, or the person's flag is off: one answer.
    BadToken,
    /// The new password is the temporary one. The token stays usable.
    SamePassword,
}

/// Use a change token: set the person's new password (it must differ
/// from the temporary one), clear the flag, end every session and void
/// their unused links and change tokens, in one IMMEDIATE transaction
/// behind a compare-and-set on `used_at`. Never mints a session: the
/// person signs in again with the new password.
pub fn consume(token: &str, password: &str) -> Result<ChangeConsume, String> {
    let token = token.trim();
    let password = password.trim();
    if password.is_empty() {
        return Err("password must not be empty".to_string());
    }
    if password_too_long(password) {
        return Err(PASSWORD_TOO_LONG.to_string());
    }
    if !token.starts_with(CHANGE_PREFIX) {
        return Ok(ChangeConsume::BadToken);
    }
    let token_hash = sha256_hex(token);
    let now = now_secs();
    // The token first (a 256-bit token needs no timing cover); the hash
    // only for a live one. The compare-and-set below decides.
    let current: Option<Option<String>> = with_conn(|conn| {
        conn.query_row(
            "SELECT p.password_hash FROM password_change_tokens t JOIN principals p ON p.id = t.principal_id
              WHERE t.token_hash = ?1 AND t.used_at IS NULL AND t.expires_at > ?2 AND p.password_must_change != 0",
            params![token_hash, now],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("change token lookup: {e}"))
    })?;
    let Some(current) = current else { return Ok(ChangeConsume::BadToken) };
    if current.as_deref().is_some_and(|h| verify_hash(password, h)) {
        return Ok(ChangeConsume::SamePassword);
    }
    // argon2 before the skin mutex.
    let pw_hash = hash_password(password)?;
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let row: Option<(String, String, Option<String>)> = tx
            .query_row(
                "SELECT t.id, t.principal_id, p.password_hash FROM password_change_tokens t JOIN principals p ON p.id = t.principal_id
                  WHERE t.token_hash = ?1 AND p.password_must_change != 0",
                params![token_hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("change token lookup: {e}"))?;
        let Some((id, principal_id, hash_now)) = row else { return Ok(ChangeConsume::BadToken) };
        // The temporary password changed between the check and the lock
        // (the owner set another): this token went out for the old one.
        if hash_now != current {
            return Ok(ChangeConsume::BadToken);
        }
        let claimed = tx
            .execute(
                "UPDATE password_change_tokens SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![id, now],
            )
            .map_err(|e| format!("change token use: {e}"))?;
        if claimed != 1 {
            return Ok(ChangeConsume::BadToken);
        }
        tx.execute("UPDATE principals SET password_hash = ?1 WHERE id = ?2", params![pw_hash, principal_id])
            .map_err(|e| format!("change password: {e}"))?;
        set_flag_conn(&tx, &principal_id, false)?;
        revoke_sessions_for_principal(&tx, &principal_id)?;
        invites::void_unused_conn(&tx, &principal_id, invites::Void::KeepOwnerReset, None)?;
        clear_lockouts_for_principal_conn(&tx, &principal_id)?;
        tx.commit().map_err(|e| format!("change commit: {e}"))?;
        Ok(ChangeConsume::Ok)
    })
}

#[cfg(test)]
#[path = "skin_must_change_tests.rs"]
mod tests;
