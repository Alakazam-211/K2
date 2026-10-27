//! App user profile (prd-app-user-profile-v1 P1/P2/P5, 0.46.5; vs-live
//! amendments U16–U18, U24 override the PRD).
//!
//! A person's own view of their account inside one App: their names and
//! picture, and the browsers and devices signed in to that App. skin.db is
//! one file per box and one `principals` row per person, so the name and
//! the picture are the person's (every App sees the same ones), while
//! settings and the session list are per App.
//!
//! Tables (additive, created at open; `SKIN_SCHEMA_VERSION` stays 6, so a
//! 0.46.4 that opens the file ignores them: no profile routes there, so
//! nothing opens):
//! - `app_profile_settings`: per App, may people edit their names and
//!   picture (both on by default: they change nothing about access).
//! - `principal_profiles`: display name, whether the full name was set by
//!   the person (U24: K2's own views show the username beside it), and the
//!   picture (K2's own JPEG re-encode, ≤ 64 KiB, with its SHA-256).
//! - `session_meta`: how a login session started and a coarse client
//!   label (never an IP), plus when it was last used (stamped from memory,
//!   at most once per 10 minutes per session, flushed every 5 minutes: no
//!   skin.db write per request). Rows go with their session.
//!
//! The box switch `K2_APP_PROFILE=off` turns the person's writes off
//! (reads and the session list stay); the old paths never change.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{app_login, immediate_tx, normalize_app_target, now_secs, with_conn, HOST_APP_ID};

/// The daemon environment switch; `off` (or `0`/`false`/`no`) turns the
/// person's profile writes off box-wide.
pub const SWITCH_ENV: &str = "K2_APP_PROFILE";
/// The actor prefix of a person's own edit (`guest:<principalId>`).
pub const SELF_ACTOR_PREFIX: &str = "guest:";
/// Longest full name (the owner's writer cap).
pub const MAX_FULL_NAME_CHARS: usize = 64;
/// Longest display name.
pub const MAX_DISPLAY_NAME_CHARS: usize = 32;
/// Longest client label (`clientLabel`, a device name, a parsed browser).
pub const MAX_CLIENT_LABEL_CHARS: usize = 40;
/// Profile writes (names and picture together) per person per window.
pub const WRITES_PER_WINDOW: usize = 20;
pub const WRITE_WINDOW_SECS: i64 = 600;
/// A session's last-used time is stamped at most this often.
pub const LAST_SEEN_EVERY_SECS: i64 = 600;
/// How long a `session_meta` row outlives its ended session (so the last
/// sign-in survives signing out).
pub const META_KEEP_SECS: i64 = 30 * 86_400;

/// How a session started (`session_meta.via`).
pub const VIA_PASSWORD: &str = "password";
pub const VIA_DEVICE: &str = "device";
pub const VIA_REFRESH: &str = "refresh";

/// Are the person's profile writes on (the box switch)?
pub fn writes_enabled() -> bool {
    !matches!(
        std::env::var(SWITCH_ENV).ok().map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_profile_settings (
            app_id TEXT PRIMARY KEY,
            edit_names INTEGER NOT NULL DEFAULT 1,
            edit_image INTEGER NOT NULL DEFAULT 1,
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );
         CREATE TABLE IF NOT EXISTS principal_profiles (
            principal_id TEXT PRIMARY KEY REFERENCES principals(id) ON DELETE CASCADE,
            display_name TEXT,
            name_self INTEGER NOT NULL DEFAULT 0,
            image BLOB,
            image_type TEXT CHECK (image_type IS NULL OR image_type = 'image/jpeg'),
            image_hash TEXT,
            image_set_at INTEGER,
            updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS session_meta (
            token_id TEXT PRIMARY KEY,
            via TEXT NOT NULL CHECK (via IN ('password','device','refresh','pairing','link','invite')),
            client_label TEXT,
            last_seen_at INTEGER
         );",
    )
    .map_err(|e| format!("skin.db profiles: {e}"))
}

// ── per-App settings ─────────────────────────────────────────────────

/// One App's profile settings (the defaults when nobody set any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSettings {
    pub app: String,
    /// The person may change their full name and display name.
    pub edit_names: bool,
    /// The person may set and clear their picture.
    pub edit_image: bool,
}

/// A change: `None` keeps a field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileSettingsPatch {
    pub edit_names: Option<bool>,
    pub edit_image: Option<bool>,
}

/// Why a settings write was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// An agent may only turn a setting off (vs-live U16): this field
    /// would go on.
    NotLower(&'static str),
    Store(String),
}

fn settings_conn(conn: &Connection, app: &str) -> Result<ProfileSettings, String> {
    let row: Option<(i64, i64)> = conn
        .query_row("SELECT edit_names, edit_image FROM app_profile_settings WHERE app_id = ?1", params![app], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(|e| format!("profile settings: {e}"))?;
    let (names, image) = row.unwrap_or((1, 1));
    Ok(ProfileSettings { app: app.to_string(), edit_names: names != 0, edit_image: image != 0 })
}

/// An App's profile settings (`host` or a published service id).
pub fn settings(app: &str) -> Result<ProfileSettings, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| settings_conn(conn, &app))
}

/// Set an App's profile settings. `lower_only` (an agent, U16): every
/// field given must be `false` or already `false`; anything that would
/// turn a setting on is refused and nothing is written. There is no
/// delete: a reset to the defaults would turn things on.
pub fn set_settings(app: &str, patch: &ProfileSettingsPatch, actor: &str, lower_only: bool) -> Result<ProfileSettings, SettingsError> {
    let app = normalize_app_target(Some(app));
    let now = now_secs();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let cur = settings_conn(&tx, &app)?;
        if lower_only {
            for (field, want, have) in [("editNames", patch.edit_names, cur.edit_names), ("editImage", patch.edit_image, cur.edit_image)] {
                if want == Some(true) && !have {
                    return Ok(Err(SettingsError::NotLower(field)));
                }
            }
        }
        let names = patch.edit_names.unwrap_or(cur.edit_names);
        let image = patch.edit_image.unwrap_or(cur.edit_image);
        tx.execute(
            "INSERT INTO app_profile_settings (app_id, edit_names, edit_image, updated_at, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(app_id) DO UPDATE SET edit_names = excluded.edit_names, edit_image = excluded.edit_image,
                updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![app, names as i64, image as i64, now, actor],
        )
        .map_err(|e| format!("profile settings write: {e}"))?;
        let out = settings_conn(&tx, &app)?;
        tx.commit().map_err(|e| format!("profile settings commit: {e}"))?;
        Ok(Ok(out))
    })
    .map_err(SettingsError::Store)?
}

// ── the person's profile row ─────────────────────────────────────────

/// One person's profile row (empty when they never set anything).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileRow {
    pub display_name: Option<String>,
    /// The full name was last set by the person themselves (U24).
    pub name_self: bool,
    pub image: Option<Vec<u8>>,
    pub image_hash: Option<String>,
    pub image_set_at: Option<i64>,
}

impl ProfileRow {
    /// The picture as a data URL (always JPEG), or `None`.
    pub fn image_data_url(&self) -> Option<String> {
        use base64::Engine as _;
        self.image
            .as_ref()
            .map(|b| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)))
    }
}

pub(crate) fn row_conn(conn: &Connection, principal_id: &str) -> Result<ProfileRow, String> {
    conn.query_row(
        "SELECT display_name, name_self, image, image_hash, image_set_at FROM principal_profiles WHERE principal_id = ?1",
        params![principal_id],
        |r| {
            Ok(ProfileRow {
                display_name: r.get(0)?,
                name_self: r.get::<_, i64>(1)? != 0,
                image: r.get(2)?,
                image_hash: r.get(3)?,
                image_set_at: r.get(4)?,
            })
        },
    )
    .optional()
    .map(Option::unwrap_or_default)
    .map_err(|e| format!("profile read: {e}"))
}

/// One person's profile row.
pub fn row(principal_id: &str) -> Result<ProfileRow, String> {
    with_conn(|conn| row_conn(conn, principal_id))
}

/// `(principal id → (name set by the person, image hash))` for every
/// person with a profile row (the owner's People list; no picture bytes).
pub fn marks() -> Result<HashMap<String, (bool, Option<String>)>, String> {
    with_conn(|conn| {
        let mut st = conn
            .prepare("SELECT principal_id, name_self, image_hash FROM principal_profiles")
            .map_err(|e| format!("profile marks: {e}"))?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)? != 0, r.get::<_, Option<String>>(2)?))))
            .map_err(|e| format!("profile marks: {e}"))?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>().map_err(|e| format!("profile marks: {e}"))
    })
}

fn ensure_row(conn: &Connection, principal_id: &str, now: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO principal_profiles (principal_id, updated_at) VALUES (?1, ?2) ON CONFLICT(principal_id) DO NOTHING",
        params![principal_id, now],
    )
    .map(|_| ())
    .map_err(|e| format!("profile row: {e}"))
}

/// Mark who last set `principal_id`'s full name, on the caller's
/// transaction (`set_principal_full_name_as` calls it: the person's own
/// edit sets it, the owner's or an agent's clears it).
pub(crate) fn mark_name_conn(conn: &Connection, principal_id: &str, by_self: bool) -> Result<(), String> {
    let now = now_secs();
    if by_self {
        ensure_row(conn, principal_id, now)?;
    }
    conn.execute(
        "UPDATE principal_profiles SET name_self = ?2, updated_at = ?3 WHERE principal_id = ?1",
        params![principal_id, by_self as i64, now],
    )
    .map(|_| ())
    .map_err(|e| format!("profile name mark: {e}"))
}

/// Set or clear (`None`) the person's display name (already cleaned).
pub fn set_display_name(principal_id: &str, display_name: Option<&str>) -> Result<(), String> {
    let now = now_secs();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        ensure_row(&tx, principal_id, now)?;
        tx.execute(
            "UPDATE principal_profiles SET display_name = ?2, updated_at = ?3 WHERE principal_id = ?1",
            params![principal_id, display_name, now],
        )
        .map_err(|e| format!("profile display name: {e}"))?;
        tx.commit().map_err(|e| format!("profile commit: {e}"))
    })
}

/// SHA-256 hex of a stored picture (what `imageHash` carries).
pub fn image_hash(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Store the person's picture: `jpeg` is K2's own re-encode
/// ([`crate::workspace::agent_look::normalize_profile_image`]). Returns
/// its hash.
pub fn set_image(principal_id: &str, jpeg: &[u8]) -> Result<String, String> {
    if jpeg.len() > crate::workspace::agent_look::PROFILE_MAX_BYTES {
        return Err("profile picture over 64 KiB".to_string());
    }
    let hash = image_hash(jpeg);
    let now = now_secs();
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        ensure_row(&tx, principal_id, now)?;
        tx.execute(
            "UPDATE principal_profiles SET image = ?2, image_type = 'image/jpeg', image_hash = ?3, image_set_at = ?4, updated_at = ?4
             WHERE principal_id = ?1",
            params![principal_id, jpeg, hash, now],
        )
        .map_err(|e| format!("profile image: {e}"))?;
        tx.commit().map_err(|e| format!("profile commit: {e}"))
    })?;
    Ok(hash)
}

/// Remove the person's picture. `true` when there was one.
pub fn clear_image(principal_id: &str) -> Result<bool, String> {
    let now = now_secs();
    with_conn(|conn| {
        conn.execute(
            "UPDATE principal_profiles SET image = NULL, image_type = NULL, image_hash = NULL, image_set_at = NULL, updated_at = ?2
             WHERE principal_id = ?1 AND image IS NOT NULL",
            params![principal_id, now],
        )
        .map(|n| n > 0)
        .map_err(|e| format!("profile image clear: {e}"))
    })
}

// ── names ────────────────────────────────────────────────────────────

fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}'
    )
}

/// Clean a name the person typed (§3.3, U24): ESC sequences and every
/// control character go, bidi overrides and zero-width characters go,
/// whitespace runs become one space, trimmed, cut to `max` characters.
/// `None` for an empty result.
pub fn clean_name(raw: &str, max: usize) -> Option<String> {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    let mut space = false;
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Drop an ESC and a CSI/OSC body after it.
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            continue;
        }
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if c.is_control() || invisible(c) {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    let out: String = out.chars().take(max).collect();
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

// ── profile write budget ─────────────────────────────────────────────

fn write_budget() -> &'static Mutex<HashMap<String, Vec<i64>>> {
    static B: OnceLock<Mutex<HashMap<String, Vec<i64>>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Take one profile write for `principal_id` (20 per 10 minutes, names and
/// picture together). `Err(retry_after_secs)` when spent. Memory only.
pub fn take_write(principal_id: &str) -> Result<(), i64> {
    let now = now_secs();
    let mut b = write_budget().lock();
    if b.len() > 4096 {
        b.retain(|_, v| v.iter().any(|t| now - t < WRITE_WINDOW_SECS));
    }
    let v = b.entry(principal_id.to_string()).or_default();
    v.retain(|t| now - t < WRITE_WINDOW_SECS);
    if v.len() >= WRITES_PER_WINDOW {
        let oldest = v.first().copied().unwrap_or(now);
        return Err((oldest + WRITE_WINDOW_SECS - now).max(1));
    }
    v.push(now);
    Ok(())
}

// ── sessions ─────────────────────────────────────────────────────────

/// Clean a client label (`clientLabel` from an App's server, a device
/// name): like a name, at most [`MAX_CLIENT_LABEL_CHARS`].
pub fn clean_label(raw: &str) -> Option<String> {
    clean_name(raw, MAX_CLIENT_LABEL_CHARS)
}

/// A coarse label from a `User-Agent` ("Safari on macOS", "Android app"),
/// or `None` when it says nothing useful. Never the raw string.
pub fn label_from_user_agent(ua: &str) -> Option<String> {
    let u = ua.trim();
    if u.is_empty() {
        return None;
    }
    let has = |s: &str| u.contains(s);
    let os = if has("iPhone") || has("iPad") || has("iOS") {
        Some("iOS")
    } else if has("Android") {
        Some("Android")
    } else if has("Mac OS X") || has("Macintosh") {
        Some("macOS")
    } else if has("Windows") {
        Some("Windows")
    } else if has("CrOS") {
        Some("ChromeOS")
    } else if has("Linux") {
        Some("Linux")
    } else {
        None
    };
    let browser = if has("Edg/") {
        Some("Edge")
    } else if has("OPR/") {
        Some("Opera")
    } else if has("Firefox/") {
        Some("Firefox")
    } else if has("Chrome/") || has("CriOS/") {
        Some("Chrome")
    } else if has("Safari/") && has("Version/") {
        Some("Safari")
    } else {
        None
    };
    let label = match (browser, os) {
        (Some(b), Some(o)) => format!("{b} on {o}"),
        (Some(b), None) => b.to_string(),
        (None, _) if has("okhttp") => "Android app".to_string(),
        (None, _) if has("CFNetwork") || has("Darwin/") => "iOS app".to_string(),
        (None, _) if u.starts_with("curl/") => "curl".to_string(),
        (None, Some(o)) => format!("App on {o}"),
        (None, None) => return None,
    };
    clean_label(&label)
}

/// Record how a new login session started (`via`, one of the `VIA_*`) and
/// its client label. A store failure is logged, never fatal to a login.
pub fn record_session(token_id: &str, via: &str, client_label: Option<&str>) {
    let label = client_label.and_then(clean_label);
    let r = with_conn(|conn| {
        conn.execute(
            "INSERT INTO session_meta (token_id, via, client_label, last_seen_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(token_id) DO UPDATE SET via = excluded.via, client_label = excluded.client_label",
            params![token_id, via, label, now_secs()],
        )
        .map(|_| ())
        .map_err(|e| format!("session meta: {e}"))
    });
    if let Err(e) = r {
        crate::log_debug!("[skin] {e}");
    }
}

fn last_seen() -> &'static Mutex<HashMap<String, i64>> {
    static M: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A login session resolved: note when (memory only; at most once per
/// [`LAST_SEEN_EVERY_SECS`] per session). Never touches skin.db.
pub fn touch(token_id: &str) {
    let now = now_secs();
    let mut m = last_seen().lock();
    match m.get_mut(token_id) {
        Some(t) if now - *t < LAST_SEEN_EVERY_SECS => {}
        Some(t) => *t = now,
        None => {
            m.insert(token_id.to_string(), now);
        }
    }
}

fn seen_in_memory(token_id: &str) -> Option<i64> {
    last_seen().lock().get(token_id).copied()
}

/// Write the remembered last-used times to `session_meta` and drop the
/// rows of sessions that ended (revoked, expired or gone). The daemon
/// runs it every 5 minutes. Returns `(stamped, pruned)`.
pub fn flush_last_seen() -> Result<(usize, usize), String> {
    let seen: Vec<(String, i64)> = std::mem::take(&mut *last_seen().lock()).into_iter().collect();
    let now = now_secs();
    let r = with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let mut stamped = 0;
        for (id, t) in &seen {
            stamped += tx
                .execute(
                    "UPDATE session_meta SET last_seen_at = MAX(COALESCE(last_seen_at, 0), ?2) WHERE token_id = ?1",
                    params![id, t],
                )
                .map_err(|e| format!("session meta stamp: {e}"))?;
        }
        // A row stays [`META_KEEP_SECS`] after its session ended, so
        // `lastSignInAt` survives a sign-out; then it goes.
        let pruned = tx
            .execute(
                "DELETE FROM session_meta WHERE NOT EXISTS (
                    SELECT 1 FROM tokens t WHERE t.id = session_meta.token_id
                      AND (t.revoked_at IS NULL OR t.revoked_at > ?1)
                      AND (t.expires_at IS NULL OR t.expires_at > ?1))",
                params![now - META_KEEP_SECS],
            )
            .map_err(|e| format!("session meta prune: {e}"))?;
        tx.commit().map_err(|e| format!("session meta commit: {e}"))?;
        Ok((stamped, pruned))
    });
    if r.is_err() {
        // Keep what we had for the next flush.
        let mut m = last_seen().lock();
        for (id, t) in seen {
            let e = m.entry(id).or_insert(t);
            *e = (*e).max(t);
        }
    }
    r
}

/// One signed-in browser or device of a person in one App (§3.7, U18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// `browser` (a login session) or `device` (a device sign-in).
    pub kind: &'static str,
    /// The tokens row id (browser) or the device sign-in id (device):
    /// never sent out as-is for a browser (the route hands out a handle).
    pub key: String,
    pub client: Option<String>,
    pub via: Option<String>,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub current: bool,
    pub push: bool,
    pub push_platform: Option<String>,
}

/// The live sessions of `principal_id` in `app`, newest first: browser
/// sessions (login passes that no device sign-in owns) and device
/// sign-ins, each with the push device linked to it. Another App's and
/// the box login's (`host`, NULL App) never show in an App's list.
/// `current_token_id`: the caller's pass.
pub fn sessions_for(principal_id: &str, app: &str, current_token_id: &str) -> Result<Vec<SessionRow>, String> {
    let app = normalize_app_target(Some(app));
    let now = now_secs();
    let mut out = with_conn(|conn| {
        let push: HashMap<(String, String), String> = {
            let mut st = conn
                .prepare(
                    "SELECT COALESCE(device_login_id, ''), COALESCE(session_token_id, ''), platform FROM app_push_devices
                      WHERE principal_id = ?1 AND app_id = ?2",
                )
                .map_err(|e| format!("session push: {e}"))?;
            let rows = st
                .query_map(params![principal_id, app], |r| Ok(((r.get::<_, String>(0)?, r.get::<_, String>(1)?), r.get::<_, String>(2)?)))
                .map_err(|e| format!("session push: {e}"))?;
            rows.collect::<rusqlite::Result<HashMap<_, _>>>().map_err(|e| format!("session push: {e}"))?
        };
        let push_for = |device: &str, token: &str| -> Option<String> {
            push.iter()
                .find(|((d, t), _)| (!device.is_empty() && d == device) || (d.is_empty() && !token.is_empty() && t == token))
                .map(|(_, p)| p.clone())
        };
        let mut rows = Vec::new();
        // An App's list never shows the box login: a NULL App is `host`.
        let mut st = conn
            .prepare(
                "SELECT t.id, t.created_at, t.expires_at, m.via, m.client_label, m.last_seen_at
                   FROM tokens t LEFT JOIN session_meta m ON m.token_id = t.id
                  WHERE t.principal_id = ?1 AND t.session = 1 AND t.revoked_at IS NULL
                    AND (t.expires_at IS NULL OR t.expires_at > ?2)
                    AND COALESCE(t.app_service_id, ?3) = ?4
                    AND NOT EXISTS (SELECT 1 FROM device_logins d WHERE d.session_token_id = t.id AND d.revoked_at IS NULL)",
            )
            .map_err(|e| format!("session list: {e}"))?;
        let it = st
            .query_map(params![principal_id, now, HOST_APP_ID, app], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(|e| format!("session list: {e}"))?;
        for row in it {
            let (id, created_at, expires_at, via, client, seen) = row.map_err(|e| format!("session row: {e}"))?;
            let platform = push_for("", &id);
            rows.push(SessionRow {
                kind: "browser",
                current: id == current_token_id,
                push: platform.is_some(),
                push_platform: platform,
                key: id,
                client,
                via,
                created_at,
                last_seen_at: seen,
                expires_at,
            });
        }
        Ok((rows, push))
    })
    .map(|(mut rows, push)| {
        let devices = app_login::devices_for(principal_id, &app).unwrap_or_default();
        for d in devices {
            let platform = push.iter().find(|((dl, _), _)| dl == &d.id).map(|(_, p)| p.clone());
            let current = d.session_token_id.as_deref() == Some(current_token_id);
            rows.push(SessionRow {
                kind: "device",
                key: d.id.clone(),
                client: clean_label(&d.name),
                via: Some(VIA_DEVICE.to_string()),
                created_at: d.created_at,
                last_seen_at: Some(d.last_used_at),
                expires_at: Some(d.expires_at),
                current,
                push: platform.is_some(),
                push_platform: platform,
            });
        }
        rows
    })?;
    for r in out.iter_mut().filter(|r| r.kind == "browser") {
        if let Some(t) = seen_in_memory(&r.key) {
            r.last_seen_at = Some(r.last_seen_at.map_or(t, |s| s.max(t)));
        }
    }
    out.sort_by(|a, b| b.current.cmp(&a.current).then(b.last_seen_at.unwrap_or(b.created_at).cmp(&a.last_seen_at.unwrap_or(a.created_at))));
    Ok(out)
}

/// The last sign-in of `principal_id` in `app` (the newest login session
/// that started with a password or a device sign-in; a refresh is not a
/// sign-in), or `None`.
pub fn last_sign_in(principal_id: &str, app: &str) -> Result<Option<i64>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let web: Option<i64> = conn
            .query_row(
                "SELECT MAX(t.created_at) FROM tokens t JOIN session_meta m ON m.token_id = t.id
                  WHERE t.principal_id = ?1 AND t.session = 1 AND COALESCE(t.app_service_id, ?2) = ?3 AND m.via != 'refresh'",
                params![principal_id, HOST_APP_ID, app],
                |r| r.get(0),
            )
            .map_err(|e| format!("last sign-in: {e}"))?;
        let device: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM device_logins WHERE principal_id = ?1 AND app_id = ?2",
                params![principal_id, app],
                |r| r.get(0),
            )
            .map_err(|e| format!("last sign-in: {e}"))?;
        Ok(web.max(device))
    })
}

/// End one browser session (`token_id`) of `principal_id` in `app`.
/// `false` when it isn't theirs, isn't in this App, is a device's pass, or
/// is already over (one answer for all).
pub fn revoke_browser_session(principal_id: &str, app: &str, token_id: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let mine: bool = tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM tokens t WHERE t.id = ?1 AND t.principal_id = ?2 AND t.session = 1
                    AND t.revoked_at IS NULL AND COALESCE(t.app_service_id, ?3) = ?4
                    AND NOT EXISTS (SELECT 1 FROM device_logins d WHERE d.session_token_id = t.id AND d.revoked_at IS NULL))",
                params![token_id, principal_id, HOST_APP_ID, app],
                |r| r.get(0),
            )
            .map_err(|e| format!("session lookup: {e}"))?;
        if !mine {
            return Ok(false);
        }
        let ended = super::live_session_rows(&tx, "id = ?1", &[&token_id])?;
        tx.execute("UPDATE tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL", params![now_secs(), token_id])
            .map_err(|e| format!("session revoke: {e}"))?;
        super::enqueue_session_revokes(&tx, &ended)?;
        super::app_push::prune_principal_conn(&tx, principal_id)?;
        tx.commit().map_err(|e| format!("session revoke commit: {e}"))?;
        Ok(true)
    })
}

/// Every other session and device sign-in of `principal_id` in `app`
/// ends; `keep_token_id` (the caller's pass) and its device sign-in stay.
/// Returns how many ended.
pub fn sign_out_others(principal_id: &str, app: &str, keep_token_id: &str) -> Result<usize, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let n = super::revoke_sessions_for_principal_except(&tx, principal_id, keep_token_id, Some(&app))?;
        tx.commit().map_err(|e| format!("sign out others commit: {e}"))?;
        Ok(n)
    })
}

/// The owner ends one browser session or device sign-in of anyone, by its
/// raw key (token id or device sign-in id). `false` when there is none.
pub fn revoke_by_owner(principal_id: &str, kind: &str, key: &str) -> Result<bool, String> {
    match kind {
        "device" => app_login::revoke_by_owner(key),
        _ => {
            let app: Option<String> = with_conn(|conn| {
                conn.query_row(
                    "SELECT COALESCE(app_service_id, ?3) FROM tokens WHERE id = ?1 AND principal_id = ?2 AND session = 1 AND revoked_at IS NULL",
                    params![key, principal_id, HOST_APP_ID],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("session lookup: {e}"))
            })?;
            match app {
                Some(app) => revoke_browser_session(principal_id, &app, key),
                None => Ok(false),
            }
        }
    }
}

#[cfg(test)]
#[path = "skin_profile_tests.rs"]
mod tests;
