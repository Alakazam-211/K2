//! The sign-in door's skin.db half (prd-app-hosted-sign-in-v1 and its
//! vs-live review, 0.46.9).
//!
//! An App's own page posts a password to `/.k2/auth/*` on the App's host;
//! K2 answers that path before the App sees it (`apps::auth_door` in
//! k2-daemon) and hands back a one-time code. The App's server swaps the
//! code, with its PKCE verifier, for a session pass. This module holds what
//! that needs from skin.db:
//!
//! - **Per-App door settings** ([`settings`]): the door off for one App
//!   (V23: the App owns `/.k2/auth` itself) and the extra page origins a
//!   bundled shell (Tauri) posts from (V11). A table of its own, made at
//!   open, no numbered step.
//! - **Token targets** ([`token_target`]): which person and App a
//!   `k2chg_` or `k2inv_` token belongs to, read without using it, so the
//!   door refuses another App's token and an owner's reset (V15) before
//!   anything is consumed.
//! - **The dry run** ([`dry_run`]): every rule a session mint has (grant,
//!   tie, test guest), rolled back, as `must_change::start` does.
//! - **The mint** ([`mint`]): the code's pass, minted only while the
//!   person's password is still the one the code was made with and
//!   must-change is off (V12), in one transaction.

use rusqlite::{params, Connection, OptionalExtension};

use super::app_login::{self, Device, DeviceSession};
use super::{
    immediate_tx, invites, mint_app_session_conn, must_change, normalize_app_target, now_secs, principal_by_username, sha256_hex, ties,
    with_conn, SkinAppSession, HOST_LOGIN_DENIED,
};

/// Most extra page origins one App may list.
pub const MAX_ORIGINS: usize = 10;

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_auth_door (
            app_id TEXT PRIMARY KEY,
            door_off INTEGER NOT NULL DEFAULT 0,
            origins TEXT NOT NULL DEFAULT '[]',
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );
         CREATE TABLE IF NOT EXISTS app_auth_door_box (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            door_on INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );",
    )
    .map_err(|e| format!("skin.db auth door: {e}"))
}

// ── the box switch ───────────────────────────────────────────────────

/// Is the door on for this box's Apps? Off until the owner turns it on
/// (vs-live V32: it lands dark and is tried live first).
pub fn box_on() -> Result<bool, String> {
    with_conn(|conn| {
        let on: Option<i64> = conn
            .query_row("SELECT door_on FROM app_auth_door_box WHERE id = 1", [], |r| r.get(0))
            .optional()
            .map_err(|e| format!("auth door switch: {e}"))?;
        Ok(on.is_some_and(|v| v != 0))
    })
}

/// Turn the door on or off for every App on this box.
pub fn set_box_on(on: bool, actor: &str) -> Result<bool, String> {
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_auth_door_box (id, door_on, updated_at, updated_by) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET door_on = excluded.door_on, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![on as i64, now_secs(), actor],
        )
        .map_err(|e| format!("auth door switch: {e}"))?;
        Ok(())
    })?;
    box_on()
}

// ── per-App settings ─────────────────────────────────────────────────

/// One App's door settings (nothing secret).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoorSettings {
    pub app: String,
    /// The door is off for this App: `/.k2/auth/*` goes to the App.
    pub off: bool,
    /// Page origins (beyond the request's own host) whose posts the door
    /// takes and answers with CORS: a bundled shell's `tauri://localhost`.
    pub origins: Vec<String>,
}

/// `app`'s door settings (defaults when it has no row).
pub fn settings(app: &str) -> Result<DoorSettings, String> {
    let app = app.trim().to_string();
    with_conn(|conn| {
        let row: Option<(i64, String)> = conn
            .query_row("SELECT door_off, origins FROM app_auth_door WHERE app_id = ?1", params![app], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| format!("auth door settings: {e}"))?;
        Ok(match row {
            Some((off, origins)) => DoorSettings { app: app.clone(), off: off != 0, origins: serde_json::from_str(&origins).unwrap_or_default() },
            None => DoorSettings { app: app.clone(), off: false, origins: Vec::new() },
        })
    })
}

/// Turn the door off (or back on) for `app`.
pub fn set_off(app: &str, off: bool, actor: &str) -> Result<DoorSettings, String> {
    let app = app.trim();
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_auth_door (app_id, door_off, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(app_id) DO UPDATE SET door_off = excluded.door_off, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![app, off as i64, now_secs(), actor],
        )
        .map_err(|e| format!("auth door off: {e}"))?;
        Ok(())
    })?;
    settings(app)
}

/// Check and normalise a list of page origins (the pairing rules:
/// `normalize_native_origin`). `Err` names the first bad one.
pub fn normalize_origins(raw: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for o in raw.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let n = super::app_pairing::normalize_native_origin(o).ok_or_else(|| {
            format!("{o:?} isn't a page origin K2 takes: a scheme and a host, no path (tauri://localhost, http://tauri.localhost, https://tauri.localhost)")
        })?;
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.len() > MAX_ORIGINS {
        return Err(format!("at most {MAX_ORIGINS} origins"));
    }
    Ok(out)
}

/// Replace `app`'s extra page origins.
pub fn set_origins(app: &str, origins: &[String], actor: &str) -> Result<DoorSettings, String> {
    let app = app.trim();
    let list = normalize_origins(origins)?;
    let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_auth_door (app_id, origins, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(app_id) DO UPDATE SET origins = excluded.origins, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![app, json, now_secs(), actor],
        )
        .map_err(|e| format!("auth door origins: {e}"))?;
        Ok(())
    })?;
    settings(app)
}

/// Every page origin the door takes for `app` beyond the request's own
/// host: its own list, plus the webview origins its phone pairing lists
/// (so an existing phone setup works with no second command).
pub fn allowed_origins(app: &str) -> Result<Vec<String>, String> {
    let mut out = settings(app)?.origins;
    if let Some(cfg) = super::app_pairing::config(app)? {
        for o in cfg.native_origins {
            if !out.contains(&o) {
                out.push(o);
            }
        }
    }
    Ok(out)
}

// ── set-password tokens ──────────────────────────────────────────────

/// The person and App a live set-password token belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenTarget {
    pub principal_id: String,
    pub username: String,
    /// The App the token was made for (its service id, or `host`).
    pub app: String,
    /// The owner's own reset link: never taken at an App's door (V15).
    pub owner_reset: bool,
}

/// Which person and App a live `k2chg_` (must-change) or `k2inv_` (invite,
/// App reset, owner reset) token belongs to, without using it. `None`:
/// unknown, used, expired, or another kind of token (the older reset
/// store's tokens have no App and never open the door).
pub fn token_target(token: &str) -> Result<Option<TokenTarget>, String> {
    let token = token.trim();
    let hash = sha256_hex(token);
    let now = now_secs();
    if token.starts_with(must_change::CHANGE_PREFIX) {
        return with_conn(|conn| {
            conn.query_row(
                "SELECT t.principal_id, p.username, t.app FROM password_change_tokens t JOIN principals p ON p.id = t.principal_id
                  WHERE t.token_hash = ?1 AND t.used_at IS NULL AND t.expires_at > ?2 AND p.password_must_change != 0",
                params![hash, now],
                |r| Ok(TokenTarget { principal_id: r.get(0)?, username: r.get(1)?, app: r.get(2)?, owner_reset: false }),
            )
            .optional()
            .map_err(|e| format!("change token target: {e}"))
        });
    }
    if token.starts_with(invites::INVITE_PREFIX) {
        return with_conn(|conn| {
            conn.query_row(
                "SELECT i.principal_id, p.username, i.app, i.purpose FROM app_user_invites i JOIN principals p ON p.id = i.principal_id
                  WHERE i.token_hash = ?1 AND i.used_at IS NULL AND i.expires_at IS NOT NULL AND i.expires_at > ?2",
                params![hash, now],
                |r| {
                    let purpose: String = r.get(3)?;
                    Ok(TokenTarget {
                        principal_id: r.get(0)?,
                        username: r.get(1)?,
                        app: r.get(2)?,
                        owner_reset: purpose == invites::PURPOSE_OWNER_RESET,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("invite target: {e}"))
        });
    }
    Ok(None)
}

// ── the dry run and the mint ─────────────────────────────────────────

/// Would a session mint for `username` on `app` be allowed now (grant,
/// tie, test guest)? Nothing is kept. `Err(HOST_LOGIN_DENIED)` when not.
/// A test guest passes only inside `test_guests::with_link_login` for its
/// own App (the caller sets it).
pub fn dry_run(username: &str, app: &str) -> Result<(), String> {
    let username = super::normalize_username(username)?;
    let app = normalize_app_target(Some(app));
    let app_ws = ties::app_workspace(&app);
    with_conn(|conn| {
        let mut tx = immediate_tx(conn)?;
        {
            let mut sp = tx.savepoint().map_err(|e| format!("auth door dry run: {e}"))?;
            sp.set_drop_behavior(rusqlite::DropBehavior::Rollback);
            mint_app_session_conn(&sp, &username, &app, app_ws.as_deref())?;
        }
        tx.rollback().map_err(|e| format!("auth door dry run: {e}"))?;
        Ok(())
    })
}

/// The person's stored password hash now (`None`: no person, or no password).
pub fn password_hash_of(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| {
        let h: Option<Option<String>> = conn
            .query_row("SELECT password_hash FROM principals WHERE id = ?1", params![principal_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("auth door hash: {e}"))?;
        Ok(h.flatten())
    })
}

/// What a code turned into.
#[derive(Debug, Clone)]
pub enum Minted {
    /// A plain session pass (no device, or the App's "Stay signed in" is off).
    Plain(SkinAppSession),
    /// A device sign-in: the pass plus a refresh token.
    Device(DeviceSession),
}

/// Mint the pass a one-time code stands for: `principal_id` on `app`, only
/// while the person's stored hash is still `expected_hash` (the one the
/// code was made with) and must-change is off. With `device` and the App's
/// "Stay signed in" on, a device sign-in. One IMMEDIATE transaction, so a
/// password write can't slip between the check and the mint.
/// `Err(HOST_LOGIN_DENIED)`: the password changed, must-change came on, the
/// grant went, or any other rule a login has refuses it.
pub fn mint(principal_id: &str, app: &str, expected_hash: &str, device: Option<&Device>) -> Result<Minted, String> {
    let app = normalize_app_target(Some(app));
    let app_ws = ties::app_workspace(&app);
    let r = with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let row: Option<(String, Option<String>, i64)> = tx
            .query_row(
                "SELECT username, password_hash, password_must_change FROM principals WHERE id = ?1",
                params![principal_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("auth door mint: {e}"))?;
        let Some((username, hash, must)) = row else { return Err(HOST_LOGIN_DENIED.to_string()) };
        if hash.as_deref() != Some(expected_hash) || must != 0 {
            return Err(HOST_LOGIN_DENIED.to_string());
        }
        if let Some(device) = device {
            let stay = app_login::settings_conn(&tx, &app)?.stay_signed_in_days;
            if stay > 0 {
                let session = mint_app_session_conn(&tx, &username, &app, app_ws.as_deref())?;
                let principal = principal_by_username(&tx, &username)?.ok_or_else(|| HOST_LOGIN_DENIED.to_string())?;
                let (chain, refresh, expires) = app_login::new_chain_conn(&tx, &principal.id, &app, device, &session.meta.id, stay)?;
                let pass_expires = app_login::cap_pass_expiry(&tx, &session.meta.id, expires)?;
                tx.commit().map_err(|e| format!("auth door mint commit: {e}"))?;
                return Ok(Minted::Device(DeviceSession {
                    session,
                    refresh,
                    refresh_expires_at: expires,
                    pass_expires_at: pass_expires,
                    device_login_id: chain,
                    device: device.clone(),
                }));
            }
        }
        let session = mint_app_session_conn(&tx, &username, &app, app_ws.as_deref())?;
        tx.commit().map_err(|e| format!("auth door mint commit: {e}"))?;
        Ok(Minted::Plain(session))
    })?;
    Ok(match r {
        Minted::Plain(s) => Minted::Plain(app_login::finish(s)),
        Minted::Device(d) => Minted::Device(DeviceSession { session: app_login::finish(d.session), ..d }),
    })
}

#[cfg(test)]
#[path = "skin_auth_door_tests.rs"]
mod tests;
