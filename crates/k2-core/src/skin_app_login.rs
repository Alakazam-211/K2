//! Phone sign-in that stays signed in, and the per-App sign-in settings
//! (app-mobile-access-v1, 0.46.3; `research-app-mobile-surface-parity-v1`
//! §5 E and F, R8, R9).
//!
//! **Device sign-in (E).** A login that names a device
//! (`POST /cli/skin/login {…, device: {id, name}}`) gets today's session
//! pass plus a **refresh token** (`k2skr_…`). `POST /cli/skin/refresh
//! {refresh}` trades it for a new pass and a new refresh token:
//! - one-time use, rotating: each refresh token works once; presenting a
//!   spent one again ends the whole device sign-in (its chain) and its pass
//!   (R8 reuse detection);
//! - bound to the App and the device id it was minted for;
//! - lasts the App's "Stay signed in" length from its last use
//!   ([`DEFAULT_STAY_DAYS`] by default, at most [`MAX_STAY_DAYS`]); `0`
//!   turns device sign-in off for that App, and a device login is then
//!   exactly today's login (7-day pass, no refresh);
//! - each refresh mints the pass from the person's grant as it is now, so a
//!   removed or disabled grant ends the chain at its next refresh;
//! - ends with the person's sessions (password change/reset, grant removed
//!   or disabled, App removed, the pass revoked, logout) and is listed for
//!   the person (`guests.devices`) and the owner (Settings → Apps → People).
//!
//! A login without `device` is unchanged: a 7-day pass, no refresh, no
//! device row (Rosson's standing rule: the new path sits beside the old).
//!
//! **Settings (E, F).** One row per App (`host` included): `listed`
//! (discovery, `GET /cli/skin/app-info`, default off, R9) and
//! `stay_days`. Tables are additive in skin.db (created at open, no
//! numbered step), like `app_owner_grants`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{
    enqueue_session_revokes, immediate_tx, live_session_rows, mint_app_session_conn, normalize_app_target,
    normalize_username, now_secs, principal_by_username, sha256_hex, with_conn, SkinAppSession, BASE62,
    HOST_LOGIN_DENIED, KEY_BODY_LEN,
};

/// Refresh tokens start with this (never a pass: `k2skn_` / `k2sk_`).
pub const REFRESH_PREFIX: &str = "k2skr_";
/// "Stay signed in" when the owner never set it (Rosson 2026-10-20).
pub const DEFAULT_STAY_DAYS: i64 = 30;
/// The longest the owner may set.
pub const MAX_STAY_DAYS: i64 = 90;
/// A device id is at most this many characters (`[A-Za-z0-9._:-]`).
pub const MAX_DEVICE_ID_LEN: usize = 128;
/// A device name is cut to this many characters.
pub const MAX_DEVICE_NAME_LEN: usize = 80;
/// Live device sign-ins per (person, App); the least recently used one
/// ends when another signs in.
pub const MAX_LIVE_DEVICES: i64 = 20;

/// Why a device sign-in ended (stored, shown to the owner).
pub const REASON_REPLACED: &str = "replaced";
pub const REASON_LIMIT: &str = "device_limit";
pub const REASON_REUSED: &str = "refresh_reused";
pub const REASON_EXPIRED: &str = "expired";
pub const REASON_GRANT: &str = "grant";
pub const REASON_OFF: &str = "stay_signed_in_off";
pub const REASON_SELF: &str = "signed_out";
pub const REASON_OWNER: &str = "owner";
pub const REASON_SESSIONS_ENDED: &str = "sessions_ended";

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_login_settings (
            app_id TEXT PRIMARY KEY,
            listed INTEGER NOT NULL DEFAULT 0,
            stay_days INTEGER NOT NULL DEFAULT 30,
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );
         CREATE TABLE IF NOT EXISTS device_logins (
            id TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            app_id TEXT NOT NULL,
            device_id TEXT NOT NULL,
            device_name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            refresh_hash TEXT NOT NULL UNIQUE,
            refresh_gen INTEGER NOT NULL DEFAULT 1,
            session_token_id TEXT,
            revoked_at INTEGER,
            revoked_reason TEXT
         );
         CREATE INDEX IF NOT EXISTS device_logins_person ON device_logins(principal_id, app_id);
         CREATE INDEX IF NOT EXISTS device_logins_session ON device_logins(session_token_id);
         CREATE TABLE IF NOT EXISTS device_refresh_spent (
            hash TEXT PRIMARY KEY,
            device_login_id TEXT NOT NULL,
            spent_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db device logins: {e}"))
}

// ── per-App settings ─────────────────────────────────────────────────

/// One App's sign-in settings (the defaults when the owner never set any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLoginSettings {
    pub app: String,
    /// Discovery (`app.info`) answers for this App. Default off.
    pub listed: bool,
    /// Device sign-in length in days; `0` = off (today's 7-day login).
    pub stay_signed_in_days: i64,
}

pub(super) fn settings_conn(conn: &Connection, app: &str) -> Result<AppLoginSettings, String> {
    let row: Option<(i64, i64)> = conn
        .query_row(
            "SELECT listed, stay_days FROM app_login_settings WHERE app_id = ?1",
            params![app],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("app login settings: {e}"))?;
    let (listed, stay) = row.unwrap_or((0, DEFAULT_STAY_DAYS));
    Ok(AppLoginSettings { app: app.to_string(), listed: listed != 0, stay_signed_in_days: stay.clamp(0, MAX_STAY_DAYS) })
}

/// An App's sign-in settings (`host` or a published service id).
pub fn settings(app: &str) -> Result<AppLoginSettings, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| settings_conn(conn, &app))
}

/// Set an App's sign-in settings (owner). `None` keeps a field. The stay
/// length is `0` (off) or 1..=[`MAX_STAY_DAYS`]; anything else is `Err`.
pub fn set_settings(app: &str, listed: Option<bool>, stay_days: Option<i64>, actor: &str) -> Result<AppLoginSettings, String> {
    let app = normalize_app_target(Some(app));
    if let Some(d) = stay_days {
        if !(0..=MAX_STAY_DAYS).contains(&d) {
            return Err(format!("staySignedInDays must be 0 (off) or 1 to {MAX_STAY_DAYS}"));
        }
    }
    with_conn(|conn| {
        let cur = settings_conn(conn, &app)?;
        let listed = listed.unwrap_or(cur.listed);
        let stay = stay_days.unwrap_or(cur.stay_signed_in_days);
        conn.execute(
            "INSERT INTO app_login_settings (app_id, listed, stay_days, updated_at, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(app_id) DO UPDATE SET listed = ?2, stay_days = ?3, updated_at = ?4, updated_by = ?5",
            params![app, listed as i64, stay, now_secs(), actor],
        )
        .map_err(|e| format!("app login settings write: {e}"))?;
        settings_conn(conn, &app)
    })
}

// ── devices ──────────────────────────────────────────────────────────

/// The device a login names (`{id, name}`), checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// `device` from a login body: absent or `null` = `Ok(None)` (today's
/// login). An id is 1..=[`MAX_DEVICE_ID_LEN`] of `[A-Za-z0-9._:-]`; the
/// name loses control, format and bidi characters (Companion's F7 rules,
/// app pairing V12: a name is shown to the person and the owner), its
/// whitespace runs fold to one space, it is cut to
/// [`MAX_DEVICE_NAME_LEN`] (empty = "Device").
pub fn parse_device(v: Option<&serde_json::Value>) -> Result<Option<Device>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(None) };
    let Some(o) = v.as_object() else {
        return Err("device must be an object {id, name}".to_string());
    };
    let id = o.get("id").and_then(|x| x.as_str()).map(str::trim).unwrap_or("");
    if id.is_empty() || id.len() > MAX_DEVICE_ID_LEN {
        return Err(format!("device.id must be 1 to {MAX_DEVICE_ID_LEN} characters"));
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-')) {
        return Err("device.id may contain only letters, digits, '.', '_', ':' and '-'".to_string());
    }
    let raw_name = o.get("name").and_then(|x| x.as_str()).unwrap_or("");
    let name = crate::connect_device_pairing::clean_name(raw_name, MAX_DEVICE_NAME_LEN, "Device");
    Ok(Some(Device { id: id.to_string(), name }))
}

/// One device sign-in, for its person and the owner. Never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLogin {
    /// The device sign-in's id (what revoke takes).
    pub id: String,
    pub principal_id: String,
    pub username: String,
    pub app: String,
    /// The id the device sent at login.
    pub device_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: i64,
    /// When it ends unless it is used again (refresh slides it).
    pub expires_at: i64,
    /// The session pass it last minted (to mark "this device").
    #[serde(skip)]
    pub session_token_id: Option<String>,
    /// How it started: `pairing` (a QR from the person's web App, app
    /// pairing v1) or `password` (a device login).
    pub via: &'static str,
}

/// [`DeviceLogin::via`] for a phone paired by QR.
pub const VIA_PAIRING: &str = "pairing";
/// [`DeviceLogin::via`] for a device login with the password.
pub const VIA_PASSWORD: &str = "password";

/// A device login or refresh: the pass, and the next refresh token (both
/// shown once).
#[derive(Debug, Clone)]
pub struct DeviceSession {
    pub session: SkinAppSession,
    pub refresh: String,
    pub refresh_expires_at: i64,
    pub pass_expires_at: i64,
    pub device_login_id: String,
    pub device: Device,
}

fn generate_refresh_raw() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut body = String::with_capacity(REFRESH_PREFIX.len() + KEY_BODY_LEN);
    body.push_str(REFRESH_PREFIX);
    const REJECT_AT: u8 = 248;
    let mut produced = 0;
    let mut scratch = [0u8; 64];
    while produced < KEY_BODY_LEN {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b >= REJECT_AT {
                continue;
            }
            body.push(BASE62[(b % 62) as usize] as char);
            produced += 1;
            if produced == KEY_BODY_LEN {
                break;
            }
        }
    }
    body
}

/// True iff `raw` is shaped like a refresh token (prefix + body length).
pub fn is_refresh_token(raw: &str) -> bool {
    raw.len() == REFRESH_PREFIX.len() + KEY_BODY_LEN
        && raw.starts_with(REFRESH_PREFIX)
        && raw[REFRESH_PREFIX.len()..].bytes().all(|b| b.is_ascii_alphanumeric())
}

/// End one session pass (a device's) and queue its Sew op. Not
/// [`super::revoke_token`]: that would end the device's chain too.
fn end_session_conn(conn: &Connection, token_id: &str) -> Result<(), String> {
    let ended = live_session_rows(conn, "id = ?1", &[&token_id])?;
    conn.execute(
        "UPDATE tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
        params![now_secs(), token_id],
    )
    .map_err(|e| format!("device session revoke: {e}"))?;
    enqueue_session_revokes(conn, &ended)
}

/// End one device chain and its current pass.
fn end_chain_conn(conn: &Connection, id: &str, reason: &str) -> Result<bool, String> {
    let session: Option<Option<String>> = conn
        .query_row(
            "SELECT session_token_id FROM device_logins WHERE id = ?1 AND revoked_at IS NULL",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("device login lookup: {e}"))?;
    let Some(session) = session else { return Ok(false) };
    conn.execute(
        "UPDATE device_logins SET revoked_at = ?1, revoked_reason = ?2 WHERE id = ?3 AND revoked_at IS NULL",
        params![now_secs(), reason, id],
    )
    .map_err(|e| format!("device login revoke: {e}"))?;
    if let Some(t) = session {
        end_session_conn(conn, &t)?;
    }
    // Push devices tied to this sign-in go with it (CP6).
    let principal: Option<String> = conn
        .query_row("SELECT principal_id FROM device_logins WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("device login lookup: {e}"))?;
    if let Some(p) = principal {
        super::app_push::prune_principal_conn(conn, &p)?;
    }
    Ok(true)
}

/// Mark device chains matching `filter` (a fixed SQL fragment over
/// `device_logins` with `?3`.. bound from `args`) ended. Their passes are
/// the caller's (the session revoke that called this already ended them).
pub(super) fn end_chains_where(conn: &Connection, filter: &str, args: &[&dyn rusqlite::ToSql], reason: &str) -> Result<usize, String> {
    let now = now_secs();
    let mut all: Vec<&dyn rusqlite::ToSql> = vec![&now, &reason];
    all.extend_from_slice(args);
    // The people whose push devices may hang on these chains (CP6).
    let people: Vec<String> = {
        let sql = format!("SELECT DISTINCT principal_id FROM device_logins WHERE revoked_at IS NULL AND ({filter})");
        let mut stmt = conn.prepare(&sql).map_err(|e| format!("device logins end: {e}"))?;
        let rows = stmt.query_map(all.as_slice(), |r| r.get::<_, String>(0)).map_err(|e| format!("device logins end: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("device logins end: {e}"))?
    };
    let sql = format!(
        "UPDATE device_logins SET revoked_at = ?1, revoked_reason = ?2 WHERE revoked_at IS NULL AND ({filter})"
    );
    let n = conn.execute(&sql, all.as_slice()).map_err(|e| format!("device logins end: {e}"))?;
    for p in &people {
        super::app_push::prune_principal_conn(conn, p)?;
    }
    Ok(n)
}

const DEVICE_SELECT: &str = "SELECT d.id, d.principal_id, COALESCE(p.username, ''), d.app_id, d.device_id, d.device_name,
        d.created_at, d.last_used_at, d.expires_at, d.session_token_id,
        EXISTS (SELECT 1 FROM device_login_pairings x WHERE x.device_login_id = d.id)
   FROM device_logins d LEFT JOIN principals p ON p.id = d.principal_id";

fn map_device(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceLogin> {
    Ok(DeviceLogin {
        id: r.get(0)?,
        principal_id: r.get(1)?,
        username: r.get(2)?,
        app: r.get(3)?,
        device_id: r.get(4)?,
        name: r.get(5)?,
        created_at: r.get(6)?,
        last_used_at: r.get(7)?,
        expires_at: r.get(8)?,
        session_token_id: r.get(9)?,
        via: if r.get::<_, bool>(10)? { VIA_PAIRING } else { VIA_PASSWORD },
    })
}

fn live_devices_where(conn: &Connection, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<DeviceLogin>, String> {
    let now = now_secs();
    let mut all: Vec<&dyn rusqlite::ToSql> = vec![&now];
    all.extend_from_slice(args);
    let sql = format!(
        "{DEVICE_SELECT} WHERE d.revoked_at IS NULL AND d.expires_at > ?1 AND ({filter}) ORDER BY d.last_used_at DESC, d.id"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("device logins list: {e}"))?;
    let rows = stmt.query_map(all.as_slice(), map_device).map_err(|e| format!("device logins list: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("device logins list: {e}"))
}

/// A person's live device sign-ins on one App.
pub fn devices_for(principal_id: &str, app: &str) -> Result<Vec<DeviceLogin>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| live_devices_where(conn, "d.principal_id = ?2 AND d.app_id = ?3", &[&principal_id, &app]))
}

/// Every live device sign-in on one App (the owner's People view).
pub fn devices_for_app(app: &str) -> Result<Vec<DeviceLogin>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| live_devices_where(conn, "d.app_id = ?2", &[&app]))
}

/// A person ends one of their own device sign-ins (any other id, another
/// person's or another App's, is `Ok(false)`: the same not-found).
pub fn revoke_own(principal_id: &str, app: &str, id: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    let id = id.trim().to_string();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let mine: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM device_logins WHERE id = ?1 AND principal_id = ?2 AND app_id = ?3 AND revoked_at IS NULL",
                params![id, principal_id, app],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| format!("device login lookup: {e}"))?
            > 0;
        let r = if mine { end_chain_conn(&tx, &id, REASON_SELF)? } else { false };
        tx.commit().map_err(|e| format!("device revoke commit: {e}"))?;
        Ok(r)
    })
}

/// The owner ends one device sign-in (and its pass).
pub fn revoke_by_owner(id: &str) -> Result<bool, String> {
    let id = id.trim().to_string();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let r = end_chain_conn(&tx, &id, REASON_OWNER)?;
        tx.commit().map_err(|e| format!("device revoke commit: {e}"))?;
        Ok(r)
    })
}

/// The live device sign-in whose current pass is `token_id`, if any.
pub fn device_for_session(token_id: &str) -> Result<Option<DeviceLogin>, String> {
    with_conn(|conn| Ok(live_devices_where(conn, "d.session_token_id = ?2", &[&token_id])?.into_iter().next()))
}

pub(super) fn new_chain_conn(
    conn: &Connection,
    principal_id: &str,
    app: &str,
    device: &Device,
    session_id: &str,
    stay_days: i64,
) -> Result<(String, String, i64), String> {
    // The same device signing in again replaces its earlier sign-in.
    let old: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM device_logins WHERE principal_id = ?1 AND app_id = ?2 AND device_id = ?3 AND revoked_at IS NULL")
            .map_err(|e| format!("device logins: {e}"))?;
        let rows = stmt
            .query_map(params![principal_id, app, device.id], |r| r.get::<_, String>(0))
            .map_err(|e| format!("device logins: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("device logins: {e}"))?
    };
    for id in old {
        end_chain_conn(conn, &id, REASON_REPLACED)?;
    }
    // At most MAX_LIVE_DEVICES live per (person, App): the least recently
    // used ones end.
    let live = live_devices_where(conn, "d.principal_id = ?2 AND d.app_id = ?3", &[&principal_id, &app])?;
    if live.len() as i64 >= MAX_LIVE_DEVICES {
        let drop = live.len() as i64 - MAX_LIVE_DEVICES + 1;
        for d in live.iter().rev().take(drop as usize) {
            end_chain_conn(conn, &d.id, REASON_LIMIT)?;
        }
    }
    let now = now_secs();
    let id = uuid::Uuid::new_v4().to_string();
    let refresh = generate_refresh_raw();
    let expires_at = now.saturating_add(stay_days.saturating_mul(86_400));
    conn.execute(
        "INSERT INTO device_logins (id, principal_id, app_id, device_id, device_name, created_at, last_used_at, expires_at, refresh_hash, refresh_gen, session_token_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, 1, ?9)",
        params![id, principal_id, app, device.id, device.name, now, expires_at, sha256_hex(&refresh), session_id],
    )
    .map_err(|e| format!("device login insert: {e}"))?;
    Ok((id, refresh, expires_at))
}

/// A pass never outlives its device sign-in.
pub(super) fn cap_pass_expiry(conn: &Connection, token_id: &str, chain_expires: i64) -> Result<i64, String> {
    conn.execute(
        "UPDATE tokens SET expires_at = MIN(COALESCE(expires_at, ?1), ?1) WHERE id = ?2",
        params![chain_expires, token_id],
    )
    .map_err(|e| format!("device pass expiry: {e}"))?;
    conn.query_row("SELECT COALESCE(expires_at, ?1) FROM tokens WHERE id = ?2", params![chain_expires, token_id], |r| r.get(0))
        .map_err(|e| format!("device pass expiry: {e}"))
}

pub(super) fn finish(session: SkinAppSession) -> SkinAppSession {
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    SkinAppSession { meta: super::attach_token_handles(session.meta), ..session }
}

/// Mint a login session for `username` on `app` **with** a device sign-in
/// (the caller already checked the password). `Ok(None)` when the App has
/// device sign-in off (`stay_days = 0`): the caller then mints exactly as
/// today. A missing grant is [`HOST_LOGIN_DENIED`], as for a plain login.
pub fn login_device(username: &str, app: &str, device: &Device) -> Result<Option<DeviceSession>, String> {
    let username = normalize_username(username)?;
    let app = normalize_app_target(Some(app));
    // A21: the App's workspace (daemon DB) before the skin mutex.
    let app_ws = super::ties::app_workspace(&app);
    let r = with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let stay = settings_conn(&tx, &app)?.stay_signed_in_days;
        if stay == 0 {
            return Ok(None);
        }
        let session = mint_app_session_conn(&tx, &username, &app, app_ws.as_deref())?;
        let principal = principal_by_username(&tx, &username)?.ok_or_else(|| HOST_LOGIN_DENIED.to_string())?;
        let (chain, refresh, expires) = new_chain_conn(&tx, &principal.id, &app, device, &session.meta.id, stay)?;
        let pass_expires = cap_pass_expiry(&tx, &session.meta.id, expires)?;
        tx.commit().map_err(|e| format!("device login commit: {e}"))?;
        Ok(Some(DeviceSession {
            session,
            refresh,
            refresh_expires_at: expires,
            pass_expires_at: pass_expires,
            device_login_id: chain,
            device: device.clone(),
        }))
    })?;
    Ok(r.map(|d| DeviceSession { session: finish(d.session), ..d }))
}

/// What a refresh did.
#[derive(Debug)]
pub enum Refresh {
    Ok(Box<DeviceSession>),
    /// Unknown, ended, expired, another App's, or the person lost the App:
    /// one answer for all (no oracle).
    Invalid,
    /// A spent refresh token came back: its device sign-in has ended.
    Reused,
}

/// Trade a refresh token for a new pass and refresh token (one-time,
/// rotating, R8). `app` (when the caller names one) must be the App the
/// token was minted for.
pub fn refresh(raw: &str, app: Option<&str>) -> Result<Refresh, String> {
    let raw = raw.trim();
    if !is_refresh_token(raw) {
        return Ok(Refresh::Invalid);
    }
    let hash = sha256_hex(raw);
    let asked_app = app.map(str::trim).filter(|a| !a.is_empty()).map(|a| normalize_app_target(Some(a)));
    // A21: the chain's App and its workspace (daemon DB) before the skin
    // mutex is taken for the rotation. A chain's App never changes.
    let chain_app: Option<String> = with_conn(|conn| {
        conn.query_row("SELECT app_id FROM device_logins WHERE refresh_hash = ?1", params![hash], |r| r.get(0))
            .optional()
            .map_err(|e| format!("refresh lookup: {e}"))
    })?;
    let chain_ws = chain_app.as_deref().and_then(super::ties::app_workspace);
    let r = with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        struct Row {
            id: String,
            principal_id: String,
            app: String,
            device_id: String,
            device_name: String,
            expires_at: i64,
            revoked: bool,
            session: Option<String>,
        }
        let row: Option<Row> = tx
            .query_row(
                "SELECT id, principal_id, app_id, device_id, device_name, expires_at, revoked_at IS NOT NULL, session_token_id
                   FROM device_logins WHERE refresh_hash = ?1",
                params![hash],
                |r| {
                    Ok(Row {
                        id: r.get(0)?,
                        principal_id: r.get(1)?,
                        app: r.get(2)?,
                        device_id: r.get(3)?,
                        device_name: r.get(4)?,
                        expires_at: r.get(5)?,
                        revoked: r.get(6)?,
                        session: r.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("refresh lookup: {e}"))?;
        let Some(row) = row else {
            let spent: Option<String> = tx
                .query_row("SELECT device_login_id FROM device_refresh_spent WHERE hash = ?1", params![hash], |r| r.get(0))
                .optional()
                .map_err(|e| format!("refresh lookup: {e}"))?;
            return match spent {
                Some(chain) => {
                    end_chain_conn(&tx, &chain, REASON_REUSED)?;
                    tx.commit().map_err(|e| format!("refresh commit: {e}"))?;
                    Ok(Refresh::Reused)
                }
                None => Ok(Refresh::Invalid),
            };
        };
        if row.revoked {
            return Ok(Refresh::Invalid);
        }
        if asked_app.as_deref().is_some_and(|a| a != row.app) {
            return Ok(Refresh::Invalid);
        }
        let now = now_secs();
        let end = |reason: &str| -> Result<Refresh, String> {
            end_chain_conn(&tx, &row.id, reason)?;
            Ok(Refresh::Invalid)
        };
        if row.expires_at <= now {
            let r = end(REASON_EXPIRED)?;
            tx.commit().map_err(|e| format!("refresh commit: {e}"))?;
            return Ok(r);
        }
        let stay = settings_conn(&tx, &row.app)?.stay_signed_in_days;
        if stay == 0 {
            let r = end(REASON_OFF)?;
            tx.commit().map_err(|e| format!("refresh commit: {e}"))?;
            return Ok(r);
        }
        let username: Option<String> = tx
            .query_row("SELECT username FROM principals WHERE id = ?1", params![row.principal_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("refresh principal: {e}"))?;
        let minted = match username {
            Some(u) if chain_app.as_deref() == Some(row.app.as_str()) => {
                mint_app_session_conn(&tx, &u, &row.app, chain_ws.as_deref())
            }
            Some(_) => Err(HOST_LOGIN_DENIED.to_string()),
            None => Err(HOST_LOGIN_DENIED.to_string()),
        };
        let session = match minted {
            Ok(s) => s,
            Err(e) if e == HOST_LOGIN_DENIED => {
                let r = end(REASON_GRANT)?;
                tx.commit().map_err(|e| format!("refresh commit: {e}"))?;
                return Ok(r);
            }
            Err(e) => return Err(e),
        };
        if let Some(old) = row.session.as_deref() {
            end_session_conn(&tx, old)?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO device_refresh_spent (hash, device_login_id, spent_at) VALUES (?1, ?2, ?3)",
            params![hash, row.id, now],
        )
        .map_err(|e| format!("refresh spend: {e}"))?;
        // Spent hashes outlive any chain that could present them.
        tx.execute(
            "DELETE FROM device_refresh_spent WHERE spent_at < ?1",
            params![now - (MAX_STAY_DAYS + 1) * 86_400],
        )
        .map_err(|e| format!("refresh prune: {e}"))?;
        let next = generate_refresh_raw();
        let expires = now.saturating_add(stay.saturating_mul(86_400));
        tx.execute(
            "UPDATE device_logins SET refresh_hash = ?1, refresh_gen = refresh_gen + 1, session_token_id = ?2,
                    last_used_at = ?3, expires_at = ?4 WHERE id = ?5",
            params![sha256_hex(&next), session.meta.id, now, expires, row.id],
        )
        .map_err(|e| format!("refresh rotate: {e}"))?;
        let pass_expires = cap_pass_expiry(&tx, &session.meta.id, expires)?;
        tx.commit().map_err(|e| format!("refresh commit: {e}"))?;
        Ok(Refresh::Ok(Box::new(DeviceSession {
            session,
            refresh: next,
            refresh_expires_at: expires,
            pass_expires_at: pass_expires,
            device_login_id: row.id,
            device: Device { id: row.device_id, name: row.device_name },
        })))
    })?;
    Ok(match r {
        Refresh::Ok(d) => {
            let d = *d;
            Refresh::Ok(Box::new(DeviceSession { session: finish(d.session), ..d }))
        }
        other => other,
    })
}

#[cfg(test)]
#[path = "skin_app_login_tests.rs"]
mod tests;
