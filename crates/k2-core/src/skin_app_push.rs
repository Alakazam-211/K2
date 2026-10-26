//! Push notifications for custom Apps: the device list and the per-App
//! push settings (`prd-custom-app-push-v1` §4–§6, 0.46.4).
//!
//! A person's own phone app (their bundle id, their Apple team, their
//! Firebase project) is the front end of a custom K2 App. A guest signed in
//! through it registers the phone's push token here; the daemon later sends
//! to it with the App owner's own Apple/Google keys (the daemon's
//! `app_push`). This module is storage only.
//!
//! **Tables** (skin.db, additive, created at open, no numbered step, V15):
//! - `app_push_devices`: one row per (App, guest, device id). The App comes
//!   from the caller's pass and the guest from its principal id, never from
//!   a body (CP3). A push token belongs to one guest per App: registering it
//!   for guest B deletes guest A's row (CP5). Rows go with the guest
//!   (`ON DELETE CASCADE`), with their device sign-in, or, without one,
//!   with the pass that registered them (CP6, [`prune_principal_conn`] in
//!   the revoke transactions plus the hourly [`sweep`]).
//! - `app_push_config`: per App, the owner's switches and the **metadata**
//!   of its keys (ids, fingerprints, when set; never a key: the keys live in
//!   the vault, system namespace).
//!
//! **Never returned.** A push token is read only by the daemon's sender
//! ([`targets`]): [`PushDevice`] skips it when serialized and redacts it in
//! `Debug`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::{immediate_tx, normalize_app_target, now_secs, with_conn, BASE62, HOST_APP_ID};

/// `channel`: the App's own phone app (the App's keys; the daemon sends).
pub const CHANNEL_APP: &str = "app";
/// `channel`: K2 Companion (§10; not in 0.46.4: `channel_unavailable`).
pub const CHANNEL_COMPANION: &str = "k2-companion";
pub const PLATFORM_APNS: &str = "apns";
pub const PLATFORM_FCM: &str = "fcm";
pub const ENV_PRODUCTION: &str = "production";
pub const ENV_SANDBOX: &str = "sandbox";
/// Push devices per (App, guest); the least recently seen goes (CP8).
pub const MAX_DEVICES_PER_GUEST: usize = 20;
/// Push devices per App unless the owner sets another number (CP8).
pub const DEFAULT_MAX_DEVICES_PER_APP: i64 = 10_000;
/// The most the owner may set per App.
pub const MAX_DEVICES_PER_APP_CEILING: i64 = 1_000_000;
/// A vendor token is at most this many characters (the relay's rule).
pub const MAX_TOKEN_LEN: usize = 4096;
/// The shortest token accepted (real ones are far longer).
pub const MIN_TOKEN_LEN: usize = 16;
pub const MAX_DEVICE_ID_LEN: usize = 128;
pub const MAX_MUTED_TOPICS: usize = 32;
pub const MAX_TOPIC_LEN: usize = 32;
/// Device ids start with this (`apd_<22 base62>`).
pub const ID_PREFIX: &str = "apd_";

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_push_devices (
            id TEXT PRIMARY KEY,
            app_id TEXT NOT NULL,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            device_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            platform TEXT NOT NULL,
            environment TEXT NOT NULL,
            push_token TEXT NOT NULL,
            previews INTEGER,
            muted_topics TEXT NOT NULL DEFAULT '[]',
            session_token_id TEXT,
            device_login_id TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            UNIQUE (app_id, principal_id, device_id),
            UNIQUE (app_id, platform, push_token)
         );
         CREATE INDEX IF NOT EXISTS app_push_devices_person ON app_push_devices(app_id, principal_id);
         CREATE INDEX IF NOT EXISTS app_push_devices_principal ON app_push_devices(principal_id);
         CREATE INDEX IF NOT EXISTS app_push_devices_session ON app_push_devices(session_token_id);
         CREATE INDEX IF NOT EXISTS app_push_devices_login ON app_push_devices(device_login_id);
         CREATE TABLE IF NOT EXISTS app_push_config (
            app_id TEXT PRIMARY KEY,
            enabled INTEGER NOT NULL DEFAULT 1,
            owner_on INTEGER NOT NULL DEFAULT 0,
            agent_replies INTEGER NOT NULL DEFAULT 0,
            previews INTEGER NOT NULL DEFAULT 1,
            max_devices INTEGER,
            apns_key_id TEXT, apns_team_id TEXT, apns_bundle_id TEXT, apns_key_fpr TEXT, apns_set_at INTEGER, apns_ok_at INTEGER,
            fcm_project_id TEXT, fcm_client_email TEXT, fcm_key_fpr TEXT, fcm_set_at INTEGER, fcm_ok_at INTEGER,
            cred_error TEXT, cred_error_at INTEGER,
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );",
    )
    .map_err(|e| format!("skin.db app push: {e}"))
}

// ── validation ───────────────────────────────────────────────────────

/// One registration as the guest sent it, checked (CP2). `previews` and
/// `muted_topics` are `None` when the body left them out (keep the stored
/// choice); `Some(None)` previews = back to the default (on).
#[derive(Clone, PartialEq, Eq)]
pub struct Registration {
    pub device_id: String,
    pub platform: String,
    pub push_token: String,
    pub environment: String,
    pub channel: String,
    pub previews: Option<Option<bool>>,
    pub muted_topics: Option<Vec<String>>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("device_id", &self.device_id)
            .field("platform", &self.platform)
            .field("push_token", &"<redacted>")
            .field("environment", &self.environment)
            .field("channel", &self.channel)
            .field("previews", &self.previews)
            .field("muted_topics", &self.muted_topics)
            .finish()
    }
}

/// A device id: 1..=[`MAX_DEVICE_ID_LEN`] of `[A-Za-z0-9._:-]`.
pub fn valid_device_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_DEVICE_ID_LEN && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// A topic: 1..=[`MAX_TOPIC_LEN`] of `[a-z0-9._-]`.
pub fn valid_topic(t: &str) -> bool {
    !t.is_empty() && t.len() <= MAX_TOPIC_LEN && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Check a vendor token's alphabet and length (the relay's rules): APNs is
/// hex (stored lower case), FCM is base64url plus `:`. The error never
/// echoes the token.
pub fn normalize_push_token(platform: &str, raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.len() < MIN_TOKEN_LEN || t.len() > MAX_TOKEN_LEN {
        return Err(format!("pushToken must be {MIN_TOKEN_LEN} to {MAX_TOKEN_LEN} characters"));
    }
    match platform {
        PLATFORM_APNS => {
            if !t.bytes().all(|b| b.is_ascii_hexdigit()) || t.len() % 2 != 0 {
                return Err("an APNs pushToken is the device token as hex (an even number of 0-9, a-f)".into());
            }
            Ok(t.to_ascii_lowercase())
        }
        PLATFORM_FCM => {
            if !t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':')) {
                return Err("an FCM pushToken may contain only letters, digits, '-', '_' and ':'".into());
            }
            Ok(t.to_string())
        }
        _ => Err("platform must be apns or fcm".into()),
    }
}

/// `mutedTopics`: at most [`MAX_MUTED_TOPICS`], each [`valid_topic`];
/// de-duplicated, sorted.
pub fn parse_muted_topics(v: &serde_json::Value) -> Result<Vec<String>, String> {
    let Some(a) = v.as_array() else { return Err("mutedTopics must be an array of strings".into()) };
    if a.len() > MAX_MUTED_TOPICS {
        return Err(format!("mutedTopics holds at most {MAX_MUTED_TOPICS} topics"));
    }
    let mut out: Vec<String> = Vec::new();
    for x in a {
        let Some(s) = x.as_str() else { return Err("mutedTopics must be an array of strings".into()) };
        if !valid_topic(s) {
            return Err(format!("a topic is 1 to {MAX_TOPIC_LEN} of a-z, 0-9, '.', '_' and '-'"));
        }
        if !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// `previews`: absent = keep, `null` = the default (on), else a bool.
fn parse_previews(v: &serde_json::Value) -> Result<Option<Option<bool>>, String> {
    match v.get("previews") {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(serde_json::Value::Bool(b)) => Ok(Some(Some(*b))),
        Some(_) => Err("previews must be true, false or null".into()),
    }
}

/// The register body (CP2). Errors name the field, never echo a value.
pub fn parse_registration(v: &serde_json::Value) -> Result<Registration, String> {
    if !v.is_object() {
        return Err("send a JSON object {deviceId, platform, pushToken, environment?}".into());
    }
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty());
    let device_id = s("deviceId").ok_or("deviceId is required (your app's install id; use the device sign-in's device.id)")?;
    if !valid_device_id(device_id) {
        return Err(format!("deviceId must be 1 to {MAX_DEVICE_ID_LEN} of letters, digits, '.', '_', ':' and '-'"));
    }
    let platform = s("platform").ok_or("platform is required: apns or fcm")?;
    if platform != PLATFORM_APNS && platform != PLATFORM_FCM {
        return Err("platform must be apns or fcm".into());
    }
    if v.get("token").is_some() {
        return Err("the field is pushToken (token is the pass's name)".into());
    }
    // The App and the guest come from the pass, never a body (CP3): an
    // unknown field (an `app`, a `principalId`) is refused, never read.
    const KNOWN: &[&str] = &["deviceId", "platform", "pushToken", "environment", "channel", "previews", "mutedTopics"];
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !KNOWN.contains(&k.as_str()))) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return Err(format!("unknown field {shown} (the App and the person come from the pass)"));
    }
    let raw = s("pushToken").ok_or("pushToken is required (the native APNs or FCM token)")?;
    let push_token = normalize_push_token(platform, raw)?;
    let environment = s("environment").unwrap_or(ENV_PRODUCTION);
    if environment != ENV_PRODUCTION && environment != ENV_SANDBOX {
        return Err("environment must be production or sandbox".into());
    }
    if platform == PLATFORM_FCM && environment != ENV_PRODUCTION {
        return Err("environment is production for fcm (sandbox is an APNs setting)".into());
    }
    let channel = s("channel").unwrap_or(CHANNEL_APP);
    if channel != CHANNEL_APP && channel != CHANNEL_COMPANION {
        return Err("channel must be app or k2-companion".into());
    }
    let muted_topics = match v.get("mutedTopics") {
        None | Some(serde_json::Value::Null) => None,
        Some(m) => Some(parse_muted_topics(m)?),
    };
    Ok(Registration {
        device_id: device_id.to_string(),
        platform: platform.to_string(),
        push_token,
        environment: environment.to_string(),
        channel: channel.to_string(),
        previews: parse_previews(v)?,
        muted_topics,
    })
}

/// The prefs body (CP11): `{deviceId, previews?, mutedTopics?}`.
pub fn parse_prefs(v: &serde_json::Value) -> Result<(String, Option<Option<bool>>, Option<Vec<String>>), String> {
    const KNOWN: &[&str] = &["deviceId", "previews", "mutedTopics"];
    if let Some(k) = v.as_object().and_then(|o| o.keys().find(|k| !KNOWN.contains(&k.as_str()))) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return Err(format!("unknown field {shown}"));
    }
    let device_id = v.get("deviceId").and_then(|x| x.as_str()).map(str::trim).unwrap_or("");
    if !valid_device_id(device_id) {
        return Err("deviceId is required".into());
    }
    let muted = match v.get("mutedTopics") {
        None | Some(serde_json::Value::Null) => None,
        Some(m) => Some(parse_muted_topics(m)?),
    };
    Ok((device_id.to_string(), parse_previews(v)?, muted))
}

// ── rows ─────────────────────────────────────────────────────────────

/// One push device. Serialized for its guest and the owner; the token is
/// never serialized and is redacted in `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushDevice {
    pub id: String,
    pub app: String,
    pub principal_id: String,
    pub username: String,
    pub device_id: String,
    pub channel: String,
    pub platform: String,
    pub environment: String,
    /// The guest's own choice (`None` = the default, on).
    pub previews: Option<bool>,
    pub muted_topics: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
    #[serde(skip)]
    pub push_token: String,
    #[serde(skip)]
    pub session_token_id: Option<String>,
    #[serde(skip)]
    pub device_login_id: Option<String>,
}

impl std::fmt::Debug for PushDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushDevice")
            .field("id", &self.id)
            .field("app", &self.app)
            .field("principal_id", &self.principal_id)
            .field("device_id", &self.device_id)
            .field("channel", &self.channel)
            .field("platform", &self.platform)
            .field("environment", &self.environment)
            .field("push_token", &"<redacted>")
            .finish()
    }
}

const DEVICE_SELECT: &str = "SELECT d.id, d.app_id, d.principal_id, COALESCE(p.username, ''), d.device_id, d.channel, d.platform,
        d.environment, d.previews, d.muted_topics, d.created_at, d.updated_at, d.last_seen_at, d.push_token,
        d.session_token_id, d.device_login_id
   FROM app_push_devices d LEFT JOIN principals p ON p.id = d.principal_id";

fn map_device(r: &rusqlite::Row<'_>) -> rusqlite::Result<PushDevice> {
    let muted: String = r.get(9)?;
    let previews: Option<i64> = r.get(8)?;
    Ok(PushDevice {
        id: r.get(0)?,
        app: r.get(1)?,
        principal_id: r.get(2)?,
        username: r.get(3)?,
        device_id: r.get(4)?,
        channel: r.get(5)?,
        platform: r.get(6)?,
        environment: r.get(7)?,
        previews: previews.map(|p| p != 0),
        muted_topics: serde_json::from_str(&muted).unwrap_or_default(),
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        last_seen_at: r.get(12)?,
        push_token: r.get(13)?,
        session_token_id: r.get(14)?,
        device_login_id: r.get(15)?,
    })
}

fn devices_where(conn: &Connection, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<PushDevice>, String> {
    let sql = format!("{DEVICE_SELECT} WHERE {filter} ORDER BY d.last_seen_at DESC, d.id");
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("push devices: {e}"))?;
    let rows = stmt.query_map(args, map_device).map_err(|e| format!("push devices: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("push devices: {e}"))
}

fn new_id() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    const LEN: usize = 22;
    const REJECT_AT: u8 = 248;
    let mut out = String::with_capacity(ID_PREFIX.len() + LEN);
    out.push_str(ID_PREFIX);
    let mut produced = 0;
    let mut scratch = [0u8; 32];
    while produced < LEN {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b >= REJECT_AT {
                continue;
            }
            out.push(BASE62[(b % 62) as usize] as char);
            produced += 1;
            if produced == LEN {
                break;
            }
        }
    }
    out
}

/// Why a registration was refused (beyond a store error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// The App holds its device ceiling (`limit` = the ceiling).
    AppFull { limit: i64 },
    /// Another guest of this App still holds this token on a live sign-in
    /// (review B-L4): that guest signs out first (`push.unregister`, or the
    /// sign-in ends). Never a silent takeover.
    TokenInUse,
    Store(String),
}

impl From<String> for RegisterError {
    fn from(e: String) -> Self {
        RegisterError::Store(e)
    }
}

/// The live device sign-in whose current pass is `token_id` (CP6).
fn live_device_login_conn(conn: &Connection, token_id: &str, now: i64) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT id FROM device_logins WHERE session_token_id = ?1 AND revoked_at IS NULL AND expires_at > ?2",
        params![token_id, now],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| format!("push device login: {e}"))
}

/// Register (or refresh) one guest's device on one App (CP3–CP6, CP8).
/// `app` and `principal_id` come from the caller's pass; `session_token_id`
/// is that pass's id. In one transaction: any other guest's row holding the
/// same token on this App goes (CP5); the (App, guest, device) row is
/// upserted (created_at kept); the guest keeps at most
/// [`MAX_DEVICES_PER_GUEST`] (least recently seen go). A new row past the
/// App's ceiling is [`RegisterError::AppFull`].
pub fn register(app: &str, principal_id: &str, session_token_id: &str, reg: &Registration) -> Result<PushDevice, RegisterError> {
    let app = normalize_app_target(Some(app));
    if app == HOST_APP_ID {
        return Err(RegisterError::Store("push is per published App, never the box login".into()));
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let now = now_secs();
        let login = live_device_login_conn(&tx, session_token_id, now)?;
        // CP5: one token, one guest (per App). Another guest's row holding
        // it is first pruned if its sign-in ended; one still live refuses
        // the register (review B-L4). This guest's own row under another
        // device id moves.
        let holders: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT DISTINCT principal_id FROM app_push_devices WHERE app_id = ?1 AND platform = ?2 AND push_token = ?3 AND principal_id != ?4")
                .map_err(|e| format!("push token holders: {e}"))?;
            let rows = stmt
                .query_map(params![app, reg.platform, reg.push_token, principal_id], |r| r.get::<_, String>(0))
                .map_err(|e| format!("push token holders: {e}"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("push token holders: {e}"))?
        };
        for h in &holders {
            prune_principal_conn(&tx, h)?;
        }
        let held: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM app_push_devices WHERE app_id = ?1 AND platform = ?2 AND push_token = ?3 AND principal_id != ?4",
                params![app, reg.platform, reg.push_token, principal_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("push token holders: {e}"))?;
        if held > 0 {
            return Ok(Err(RegisterError::TokenInUse));
        }
        tx.execute(
            "DELETE FROM app_push_devices WHERE app_id = ?1 AND platform = ?2 AND push_token = ?3
               AND principal_id = ?4 AND device_id != ?5",
            params![app, reg.platform, reg.push_token, principal_id, reg.device_id],
        )
        .map_err(|e| format!("push token move: {e}"))?;
        let existing: Option<(String, Option<i64>, String)> = tx
            .query_row(
                "SELECT id, previews, muted_topics FROM app_push_devices WHERE app_id = ?1 AND principal_id = ?2 AND device_id = ?3",
                params![app, principal_id, reg.device_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("push device lookup: {e}"))?;
        let id = match existing {
            Some((id, old_previews, old_muted)) => {
                let previews: Option<i64> = match reg.previews {
                    None => old_previews,
                    Some(p) => p.map(i64::from),
                };
                let muted = match &reg.muted_topics {
                    None => old_muted,
                    Some(m) => serde_json::to_string(m).map_err(|e| format!("muted topics: {e}"))?,
                };
                tx.execute(
                    "UPDATE app_push_devices SET channel = ?2, platform = ?3, environment = ?4, push_token = ?5, previews = ?6,
                            muted_topics = ?7, session_token_id = ?8, device_login_id = ?9, updated_at = ?10, last_seen_at = ?10
                      WHERE id = ?1",
                    params![id, reg.channel, reg.platform, reg.environment, reg.push_token, previews, muted, session_token_id, login, now],
                )
                .map_err(|e| format!("push device update: {e}"))?;
                id
            }
            None => {
                let limit = max_devices_conn(&tx, &app)?;
                let count: i64 = tx
                    .query_row("SELECT COUNT(*) FROM app_push_devices WHERE app_id = ?1", params![app], |r| r.get(0))
                    .map_err(|e| format!("push device count: {e}"))?;
                if count >= limit {
                    return Ok(Err(RegisterError::AppFull { limit }));
                }
                let id = new_id();
                let previews: Option<i64> = reg.previews.flatten().map(i64::from);
                let muted = serde_json::to_string(reg.muted_topics.as_deref().unwrap_or(&[])).map_err(|e| format!("muted topics: {e}"))?;
                tx.execute(
                    "INSERT INTO app_push_devices (id, app_id, principal_id, device_id, channel, platform, environment, push_token,
                            previews, muted_topics, session_token_id, device_login_id, created_at, updated_at, last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?13)",
                    params![
                        id,
                        app,
                        principal_id,
                        reg.device_id,
                        reg.channel,
                        reg.platform,
                        reg.environment,
                        reg.push_token,
                        previews,
                        muted,
                        session_token_id,
                        login,
                        now
                    ],
                )
                .map_err(|e| format!("push device insert: {e}"))?;
                id
            }
        };
        // CP8: at most MAX_DEVICES_PER_GUEST per (App, guest).
        let mine = devices_where(&tx, "d.app_id = ?1 AND d.principal_id = ?2", &[&app, &principal_id])?;
        if mine.len() > MAX_DEVICES_PER_GUEST {
            for d in mine.iter().filter(|d| d.id != id).rev().take(mine.len() - MAX_DEVICES_PER_GUEST) {
                tx.execute("DELETE FROM app_push_devices WHERE id = ?1", params![d.id]).map_err(|e| format!("push device evict: {e}"))?;
            }
        }
        let row = devices_where(&tx, "d.id = ?1", &[&id])?.into_iter().next().ok_or_else(|| "push device vanished".to_string())?;
        tx.commit().map_err(|e| format!("push device commit: {e}"))?;
        Ok(Ok(row))
    })?
}

/// A guest removes one of their own devices (CP10). Any other row (another
/// guest's, another App's, none) is `Ok(false)`: the same answer.
pub fn unregister(app: &str, principal_id: &str, device_id: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let n = conn
            .execute(
                "DELETE FROM app_push_devices WHERE app_id = ?1 AND principal_id = ?2 AND device_id = ?3",
                params![app, principal_id, device_id.trim()],
            )
            .map_err(|e| format!("push unregister: {e}"))?;
        Ok(n > 0)
    })
}

/// A guest's own device row (CP11).
pub fn get_own(app: &str, principal_id: &str, device_id: &str) -> Result<Option<PushDevice>, String> {
    let app = normalize_app_target(Some(app));
    let device_id = device_id.trim().to_string();
    with_conn(|conn| {
        Ok(devices_where(conn, "d.app_id = ?1 AND d.principal_id = ?2 AND d.device_id = ?3", &[&app, &principal_id, &device_id])?
            .into_iter()
            .next())
    })
}

/// Change a guest's own device choices without a token (CP11). `None`
/// keeps a field. `Ok(None)` = no such device of theirs.
pub fn set_prefs(
    app: &str,
    principal_id: &str,
    device_id: &str,
    previews: Option<Option<bool>>,
    muted: Option<&[String]>,
) -> Result<Option<PushDevice>, String> {
    let app = normalize_app_target(Some(app));
    let device_id = device_id.trim().to_string();
    with_conn(|conn| {
        let now = now_secs();
        let found = devices_where(conn, "d.app_id = ?1 AND d.principal_id = ?2 AND d.device_id = ?3", &[&app, &principal_id, &device_id])?;
        let Some(cur) = found.into_iter().next() else { return Ok(None) };
        let previews: Option<i64> = match previews {
            None => cur.previews.map(i64::from),
            Some(p) => p.map(i64::from),
        };
        let muted = serde_json::to_string(muted.unwrap_or(&cur.muted_topics)).map_err(|e| format!("muted topics: {e}"))?;
        conn.execute(
            "UPDATE app_push_devices SET previews = ?2, muted_topics = ?3, updated_at = ?4 WHERE id = ?1",
            params![cur.id, previews, muted, now],
        )
        .map_err(|e| format!("push prefs: {e}"))?;
        Ok(devices_where(conn, "d.id = ?1", &[&cur.id])?.into_iter().next())
    })
}

/// Every device of one App, or of one of its guests (the owner's view).
pub fn devices_for_app(app: &str, principal_id: Option<&str>) -> Result<Vec<PushDevice>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| match principal_id {
        Some(p) => devices_where(conn, "d.app_id = ?1 AND d.principal_id = ?2", &[&app, &p]),
        None => devices_where(conn, "d.app_id = ?1", &[&app]),
    })
}

/// How many devices an App holds, per guest (the link's `push.devices`
/// and the owner's status): `(principal_id, count, platforms)`.
pub fn device_counts(app: &str) -> Result<Vec<(String, i64, Vec<String>)>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT principal_id, COUNT(*), GROUP_CONCAT(DISTINCT platform) FROM app_push_devices
                  WHERE app_id = ?1 GROUP BY principal_id ORDER BY principal_id",
            )
            .map_err(|e| format!("push device counts: {e}"))?;
        let rows = stmt
            .query_map(params![app], |r| {
                let p: Option<String> = r.get(2)?;
                let mut plats: Vec<String> = p.unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
                plats.sort();
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, plats))
            })
            .map_err(|e| format!("push device counts: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("push device counts: {e}"))
    })
}

/// The owner removes one device by its id (`apd_…`). Another App's id is
/// `Ok(false)`.
pub fn remove_by_owner(app: &str, id: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let n = conn
            .execute("DELETE FROM app_push_devices WHERE app_id = ?1 AND id = ?2", params![app, id.trim()])
            .map_err(|e| format!("push device remove: {e}"))?;
        Ok(n > 0)
    })
}

/// The sender drops dead tokens (Apple/Google said the token is gone,
/// CP21). Only rows of `app`, and only if they still hold that token (a
/// re-registration since the send keeps its row).
pub fn delete_dead(app: &str, id: &str, push_token: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let n = conn
            .execute(
                "DELETE FROM app_push_devices WHERE app_id = ?1 AND id = ?2 AND push_token = ?3",
                params![app, id, push_token],
            )
            .map_err(|e| format!("push device prune: {e}"))?;
        Ok(n > 0)
    })
}

// ── recipients (CP24, CP25, CP40) ────────────────────────────────────

/// The principals among `names` (principal ids or usernames) that hold a
/// live, enabled `kind=app` grant on `app`, de-duplicated in order, and how
/// many names didn't (an unknown name, a Connect name, another App's guest:
/// one count, no oracle).
pub fn resolve_guests(app: &str, names: &[String]) -> Result<(Vec<String>, usize), String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let mut out: Vec<String> = Vec::new();
        let mut missing = 0usize;
        for raw in names {
            let n = raw.trim();
            let found: Option<String> = if n.is_empty() {
                None
            } else {
                conn.query_row(
                    "SELECT p.id FROM principals p JOIN grants g
                       ON g.subject_kind = 'principal' AND g.subject_id = p.id AND g.kind = 'app' AND g.target_id = ?2 AND g.enabled != 0
                      WHERE p.id = ?1 OR p.username = lower(?1) LIMIT 1",
                    params![n, app],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("push recipients: {e}"))?
            };
            match found {
                Some(id) if !out.contains(&id) => out.push(id),
                Some(_) => {}
                None => missing += 1,
            }
        }
        Ok((out, missing))
    })
}

/// This App's guests holding any grant line on `room` (a project id): the
/// role's room lines, or a role-less grant's rooms (CP25). With `cap`, only
/// those whose line on that room holds it (agent replies: `thread:read`).
pub fn room_guests(app: &str, room: &str, cap: Option<&str>) -> Result<Vec<String>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let grants: Vec<super::SkinGrant> = {
            let mut stmt = conn
                .prepare(&format!(
                    "{} WHERE subject_kind = 'principal' AND kind = 'app' AND target_id = ?1 AND enabled != 0",
                    super::GRANT_SELECT
                ))
                .map_err(|e| format!("push room recipients: {e}"))?;
            let rows = stmt.query_map(params![app], super::map_grant_row).map_err(|e| format!("push room recipients: {e}"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("push room recipients: {e}"))?
        };
        let mut out = Vec::new();
        for g in &grants {
            let (policy, _) = super::grant_login_policy(conn, g)?;
            let holds = match (policy.get(room), cap) {
                (Some(caps), Some(c)) => caps.iter().any(|x| x == c),
                (Some(_), None) => true,
                (None, _) => false,
            };
            if holds && !out.contains(&g.subject_id) {
                out.push(g.subject_id.clone());
            }
        }
        Ok(out)
    })
}

/// The `app`-channel devices of these principals on `app`, each still
/// holding a live, enabled `kind=app` grant on it at this moment (CP24).
/// The only reader of push tokens (the daemon's sender).
pub fn targets(app: &str, principal_ids: &[String]) -> Result<Vec<PushDevice>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let mut out = Vec::new();
        let filter = format!("d.app_id = ?1 AND d.principal_id = ?2 AND d.channel = 'app' AND {LIVE_GRANT} AND {LIVE_SIGN_IN}");
        for pid in principal_ids {
            out.extend(devices_where(conn, &filter, &[&app, pid])?);
        }
        Ok(out)
    })
}

/// The guest still holds a live, enabled grant on the row's App.
const LIVE_GRANT: &str = "EXISTS (SELECT 1 FROM grants g WHERE g.subject_kind = 'principal' AND g.subject_id = d.principal_id
       AND g.kind = 'app' AND g.target_id = d.app_id AND g.enabled != 0)";

/// The row's device sign-in (or, without one, the pass that registered it)
/// is still live right now: an expired pass never waits for the hourly
/// sweep (review B-L2).
const LIVE_SIGN_IN: &str = "((d.device_login_id IS NOT NULL AND EXISTS (SELECT 1 FROM device_logins dl
         WHERE dl.id = d.device_login_id AND dl.revoked_at IS NULL AND dl.expires_at > CAST(strftime('%s', 'now') AS INTEGER)))
   OR (d.device_login_id IS NULL AND EXISTS (SELECT 1 FROM tokens t
         WHERE t.id = d.session_token_id AND t.revoked_at IS NULL
           AND (t.expires_at IS NULL OR t.expires_at > CAST(strftime('%s', 'now') AS INTEGER)))))";

/// At delivery (review B-L1): does the row still exist on this App with
/// this exact token, its guest's grant still enabled and its sign-in still
/// live? Revoke, disable, removal and a token change since the send was
/// queued all answer `false`.
pub fn still_deliverable(app: &str, id: &str, push_token: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let sql = format!(
            "SELECT COUNT(*) FROM app_push_devices d WHERE d.app_id = ?1 AND d.id = ?2 AND d.push_token = ?3 AND d.channel = 'app'
               AND {LIVE_GRANT} AND {LIVE_SIGN_IN}"
        );
        let n: i64 = conn.query_row(&sql, params![app, id, push_token], |r| r.get(0)).map_err(|e| format!("push delivery check: {e}"))?;
        Ok(n > 0)
    })
}

/// The Apps with agent-reply pushes on (and not turned off by the owner).
pub fn apps_with_agent_replies() -> Result<Vec<String>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT app_id FROM app_push_config WHERE agent_replies = 1 AND enabled = 1 ORDER BY app_id")
            .map_err(|e| format!("push config: {e}"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| format!("push config: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("push config: {e}"))
    })
}

// ── cleanup (CP6) ────────────────────────────────────────────────────

/// The rows that outlived their sign-in: a row tied to a device sign-in
/// goes when that sign-in has ended or expired; a row without one goes when
/// the pass that registered it has. `scope` is a fixed SQL fragment over
/// `app_push_devices` (`?2`.. bound from `args`); `?1` is now.
fn prune_where(conn: &Connection, scope: &str, args: &[&dyn rusqlite::ToSql]) -> Result<usize, String> {
    let now = now_secs();
    let mut all: Vec<&dyn rusqlite::ToSql> = vec![&now];
    all.extend_from_slice(args);
    let sql = format!(
        "DELETE FROM app_push_devices WHERE ({scope}) AND (
            (device_login_id IS NOT NULL AND NOT EXISTS (
                SELECT 1 FROM device_logins d
                 WHERE d.id = app_push_devices.device_login_id AND d.revoked_at IS NULL AND d.expires_at > ?1))
         OR (device_login_id IS NULL AND NOT EXISTS (
                SELECT 1 FROM tokens t
                 WHERE t.id = app_push_devices.session_token_id AND t.revoked_at IS NULL
                   AND (t.expires_at IS NULL OR t.expires_at > ?1)))
         )"
    );
    conn.execute(&sql, all.as_slice()).map_err(|e| format!("push device prune: {e}"))
}

/// Prune one principal's stale rows, inside the revoke transaction that
/// just ended their sessions or device sign-ins (CP6, A2).
pub(super) fn prune_principal_conn(conn: &Connection, principal_id: &str) -> Result<usize, String> {
    prune_where(conn, "principal_id = ?2", &[&principal_id])
}

/// The hourly sweep (CP6): every row whose sign-in or pass ended without a
/// revoke (plain expiry never reaches the revoke hooks).
pub fn sweep() -> Result<usize, String> {
    with_conn(|conn| prune_where(conn, "1 = 1", &[]))
}

/// An App went away (uninstall, purge, `publish rm`): both tables' rows.
/// Returns how many devices went.
pub fn purge_app(app: &str) -> Result<usize, String> {
    let app = normalize_app_target(Some(app));
    if app == HOST_APP_ID {
        return Ok(0);
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let n = tx.execute("DELETE FROM app_push_devices WHERE app_id = ?1", params![app]).map_err(|e| format!("push purge: {e}"))?;
        tx.execute("DELETE FROM app_push_config WHERE app_id = ?1", params![app]).map_err(|e| format!("push purge: {e}"))?;
        tx.commit().map_err(|e| format!("push purge commit: {e}"))?;
        Ok(n)
    })
}

// ── per-App settings ─────────────────────────────────────────────────

/// The APNs key's metadata (never the key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApnsMeta {
    pub key_id: String,
    pub team_id: String,
    pub bundle_id: String,
    /// First 8 bytes of the key's SHA-256, hex.
    pub key_fingerprint: String,
    pub set_at: i64,
    /// The first send Apple accepted with this key (`None` = not yet). Until
    /// then a token "from another app" is read as a wrong bundle id, not a
    /// dead device (security review A-M1).
    pub proven_at: Option<i64>,
}

/// The Firebase service account's metadata (never the key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FcmMeta {
    pub project_id: String,
    pub client_email: String,
    pub key_fingerprint: String,
    pub set_at: i64,
    /// The first send Google accepted with this service account.
    pub proven_at: Option<i64>,
}

/// One App's push settings (defaults when the owner never set any).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushConfig {
    pub app: String,
    /// The owner's kill switch (`k2 app push off`). Default on: the state
    /// is also gated on the `push:send` approval or `owner_on`.
    pub enabled: bool,
    /// The owner turned push on by hand (`k2 app push on`), for an App
    /// without a manifest (a `--cmd` service).
    pub owner_on: bool,
    /// Agent-reply pushes (§9). Default off.
    pub agent_replies: bool,
    /// Agent-reply previews for this App. Default on.
    pub previews: bool,
    /// The owner's device ceiling; `None` = [`DEFAULT_MAX_DEVICES_PER_APP`].
    pub max_devices: Option<i64>,
    pub apns: Option<ApnsMeta>,
    pub fcm: Option<FcmMeta>,
    /// The last credential refusal, a reason code only.
    pub cred_error: Option<String>,
    pub cred_error_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub updated_by: Option<String>,
}

impl PushConfig {
    fn defaults(app: &str) -> Self {
        PushConfig {
            app: app.to_string(),
            enabled: true,
            owner_on: false,
            agent_replies: false,
            previews: true,
            max_devices: None,
            apns: None,
            fcm: None,
            cred_error: None,
            cred_error_at: None,
            updated_at: None,
            updated_by: None,
        }
    }
}

fn config_conn(conn: &Connection, app: &str) -> Result<PushConfig, String> {
    let row = conn
        .query_row(
            "SELECT enabled, owner_on, agent_replies, previews, max_devices,
                    apns_key_id, apns_team_id, apns_bundle_id, apns_key_fpr, apns_set_at,
                    fcm_project_id, fcm_client_email, fcm_key_fpr, fcm_set_at,
                    cred_error, cred_error_at, updated_at, updated_by, apns_ok_at, fcm_ok_at
               FROM app_push_config WHERE app_id = ?1",
            params![app],
            |r| {
                let apns = match (r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, Option<String>>(7)?) {
                    (Some(key_id), Some(team_id), Some(bundle_id)) => Some(ApnsMeta {
                        key_id,
                        team_id,
                        bundle_id,
                        key_fingerprint: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                        set_at: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
                        proven_at: r.get(18)?,
                    }),
                    _ => None,
                };
                let fcm = match (r.get::<_, Option<String>>(10)?, r.get::<_, Option<String>>(11)?) {
                    (Some(project_id), Some(client_email)) => Some(FcmMeta {
                        project_id,
                        client_email,
                        key_fingerprint: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
                        set_at: r.get::<_, Option<i64>>(13)?.unwrap_or(0),
                        proven_at: r.get(19)?,
                    }),
                    _ => None,
                };
                Ok(PushConfig {
                    app: app.to_string(),
                    enabled: r.get::<_, i64>(0)? != 0,
                    owner_on: r.get::<_, i64>(1)? != 0,
                    agent_replies: r.get::<_, i64>(2)? != 0,
                    previews: r.get::<_, i64>(3)? != 0,
                    max_devices: r.get(4)?,
                    apns,
                    fcm,
                    cred_error: r.get(14)?,
                    cred_error_at: r.get(15)?,
                    updated_at: r.get(16)?,
                    updated_by: r.get(17)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("push config: {e}"))?;
    Ok(row.unwrap_or_else(|| PushConfig::defaults(app)))
}

fn max_devices_conn(conn: &Connection, app: &str) -> Result<i64, String> {
    Ok(config_conn(conn, app)?.max_devices.unwrap_or(DEFAULT_MAX_DEVICES_PER_APP))
}

/// Has anyone set push up for this App (a stored settings row: switches, a
/// device cap or Apple/Google keys)? The hand-over guard refuses while one
/// exists.
pub fn is_configured(app: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        conn.query_row("SELECT 1 FROM app_push_config WHERE app_id = ?1", params![app], |_| Ok(()))
            .optional()
            .map(|o| o.is_some())
            .map_err(|e| format!("push config: {e}"))
    })
}

/// One App's push settings.
pub fn config(app: &str) -> Result<PushConfig, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| config_conn(conn, &app))
}

fn ensure_row(conn: &Connection, app: &str, actor: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_push_config (app_id, updated_at, updated_by) VALUES (?1, ?2, ?3) ON CONFLICT(app_id) DO NOTHING",
        params![app, now_secs(), actor],
    )
    .map_err(|e| format!("push config write: {e}"))?;
    Ok(())
}

/// The owner's (and the App agent's) switches. `None` keeps a field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Switches {
    pub enabled: Option<bool>,
    pub owner_on: Option<bool>,
    pub agent_replies: Option<bool>,
    pub previews: Option<bool>,
}

/// Write switches; returns the settings after.
pub fn set_switches(app: &str, s: Switches, actor: &str) -> Result<PushConfig, String> {
    let app = normalize_app_target(Some(app));
    if app == HOST_APP_ID {
        return Err("push is per published App, never the box login (host)".into());
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        ensure_row(&tx, &app, actor)?;
        let cur = config_conn(&tx, &app)?;
        tx.execute(
            "UPDATE app_push_config SET enabled = ?2, owner_on = ?3, agent_replies = ?4, previews = ?5, updated_at = ?6, updated_by = ?7
              WHERE app_id = ?1",
            params![
                app,
                s.enabled.unwrap_or(cur.enabled) as i64,
                s.owner_on.unwrap_or(cur.owner_on) as i64,
                s.agent_replies.unwrap_or(cur.agent_replies) as i64,
                s.previews.unwrap_or(cur.previews) as i64,
                now_secs(),
                actor
            ],
        )
        .map_err(|e| format!("push config write: {e}"))?;
        let after = config_conn(&tx, &app)?;
        tx.commit().map_err(|e| format!("push config commit: {e}"))?;
        Ok(after)
    })
}

/// The owner's device ceiling for one App (`None` = the default).
pub fn set_max_devices(app: &str, max: Option<i64>, actor: &str) -> Result<PushConfig, String> {
    let app = normalize_app_target(Some(app));
    if let Some(m) = max {
        if !(1..=MAX_DEVICES_PER_APP_CEILING).contains(&m) {
            return Err(format!("push devices per App is 1 to {MAX_DEVICES_PER_APP_CEILING}"));
        }
    }
    with_conn(|conn| {
        ensure_row(conn, &app, actor)?;
        conn.execute(
            "UPDATE app_push_config SET max_devices = ?2, updated_at = ?3, updated_by = ?4 WHERE app_id = ?1",
            params![app, max, now_secs(), actor],
        )
        .map_err(|e| format!("push config write: {e}"))?;
        config_conn(conn, &app)
    })
}

/// Record (or clear, `None`) the APNs key's metadata. Clears a stored
/// credential error.
pub fn set_apns_meta(app: &str, meta: Option<&ApnsMeta>, actor: &str) -> Result<PushConfig, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        ensure_row(conn, &app, actor)?;
        conn.execute(
            "UPDATE app_push_config SET apns_key_id = ?2, apns_team_id = ?3, apns_bundle_id = ?4, apns_key_fpr = ?5, apns_set_at = ?6, apns_ok_at = NULL,
                    cred_error = NULL, cred_error_at = NULL, updated_at = ?7, updated_by = ?8
              WHERE app_id = ?1",
            params![
                app,
                meta.map(|m| m.key_id.clone()),
                meta.map(|m| m.team_id.clone()),
                meta.map(|m| m.bundle_id.clone()),
                meta.map(|m| m.key_fingerprint.clone()),
                meta.map(|m| m.set_at),
                now_secs(),
                actor
            ],
        )
        .map_err(|e| format!("push config write: {e}"))?;
        config_conn(conn, &app)
    })
}

/// Record (or clear, `None`) the Firebase service account's metadata.
/// Clears a stored credential error.
pub fn set_fcm_meta(app: &str, meta: Option<&FcmMeta>, actor: &str) -> Result<PushConfig, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        ensure_row(conn, &app, actor)?;
        conn.execute(
            "UPDATE app_push_config SET fcm_project_id = ?2, fcm_client_email = ?3, fcm_key_fpr = ?4, fcm_set_at = ?5, fcm_ok_at = NULL,
                    cred_error = NULL, cred_error_at = NULL, updated_at = ?6, updated_by = ?7
              WHERE app_id = ?1",
            params![
                app,
                meta.map(|m| m.project_id.clone()),
                meta.map(|m| m.client_email.clone()),
                meta.map(|m| m.key_fingerprint.clone()),
                meta.map(|m| m.set_at),
                now_secs(),
                actor
            ],
        )
        .map_err(|e| format!("push config write: {e}"))?;
        config_conn(conn, &app)
    })
}

/// Record the first accepted send with the platform's current key
/// (`Ok(true)` = this call proved it). `fpr` is the fingerprint of the key
/// the send went out with: a send that used a key since replaced proves
/// nothing. New keys reset it.
pub fn mark_proven(app: &str, platform: &str, fpr: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    let sql = match platform {
        PLATFORM_APNS => {
            "UPDATE app_push_config SET apns_ok_at = ?2 WHERE app_id = ?1 AND apns_key_id IS NOT NULL AND apns_key_fpr = ?3 AND apns_ok_at IS NULL"
        }
        PLATFORM_FCM => {
            "UPDATE app_push_config SET fcm_ok_at = ?2 WHERE app_id = ?1 AND fcm_project_id IS NOT NULL AND fcm_key_fpr = ?3 AND fcm_ok_at IS NULL"
        }
        _ => return Err("platform is apns or fcm".into()),
    };
    with_conn(|conn| Ok(conn.execute(sql, params![app, now_secs(), fpr]).map_err(|e| format!("push config write: {e}"))? > 0))
}

/// Store the last credential refusal (a reason code such as
/// `apns:InvalidProviderToken`), never a key or a token.
pub fn set_cred_error(app: &str, code: &str) -> Result<(), String> {
    let app = normalize_app_target(Some(app));
    let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.')).take(96).collect();
    with_conn(|conn| {
        ensure_row(conn, &app, "k2:app-push")?;
        conn.execute(
            "UPDATE app_push_config SET cred_error = ?2, cred_error_at = ?3 WHERE app_id = ?1",
            params![app, code, now_secs()],
        )
        .map_err(|e| format!("push config write: {e}"))?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "skin_app_push_tests.rs"]
mod tests;
