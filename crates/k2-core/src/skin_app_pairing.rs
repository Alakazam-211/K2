//! Phone pairing for custom Apps (`prd-app-phone-pairing-v1` + its
//! vs-live, 0.46.5).
//!
//! A person signed in to a custom App on the web pairs the App's own phone
//! app by QR, without typing a password on the phone. The phone gets
//! exactly what a device sign-in gets ([`super::app_login`]): a pass plus a
//! rotating `k2skr_` refresh token, bound to that App and that person.
//!
//! **Codes** (in memory only, by digest; a restart drops them):
//! - `K2A-XXXXX-XXXXX-XXXXX-XXXXX`: 20 Crockford characters (100 bits).
//!   A separate map from Companion's `K2P` codes
//!   ([`crate::connect_device_pairing`]), so a code of one kind can never be
//!   redeemed at the other's route, whatever its prefix.
//! - Bound at mint to the App, the person (`principal_id`), the web pass
//!   that minted it (`tokens.id`) and the host its QR names. The redeem
//!   re-checks the App, the pass and the person's grant
//!   ([`login_device_paired`]).
//! - [`CODE_TTL_SECS`] long; one-time (a redeem attempt consumes it,
//!   whatever the outcome); a check ([`check`]) never consumes.
//! - At most [`MAX_PENDING_PER_PERSON`] outstanding per (App, person),
//!   [`MAX_PENDING_PER_APP`] per App and [`MAX_OUTSTANDING`] in all: the
//!   oldest goes. At most [`STARTS_PER_HOUR`] starts per (App, person).
//! - Each code has a non-secret `pairingId` (`apr_…`), so the web page can
//!   poll "paired yet?" and audit rows can name the attempt without the
//!   code. Its state is kept [`STATUS_KEEP_SECS`] after the code ends.
//!
//! **Tables** (skin.db, additive, created at open, no numbered step, no
//! `SKIN_SCHEMA_VERSION` bump; an older K2 ignores both):
//! - `app_pairing_config`: per App, the owner's phone app ids (iOS app ids,
//!   Android packages with their signing fingerprints), the hosts, native
//!   webview origins, extra universal-link paths and store links. All
//!   non-secret.
//! - `device_login_pairings`: one row per device sign-in that started from
//!   a pairing (never the code). It lives as long as its `device_logins`
//!   row; both go with the person.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{app_login, immediate_tx, mint_app_session_conn, normalize_app_target, now_secs, with_conn, BASE62, HOST_APP_ID, HOST_LOGIN_DENIED};

/// Human prefix on the shown code. Optional when typed.
pub const CODE_PREFIX: &str = "K2A";
/// Random characters in a code (5 bits each).
pub const CODE_CHARS: usize = 20;
/// How long a code works (CP8: the web page re-starts).
pub const CODE_TTL_SECS: i64 = 300;
/// Outstanding codes per (App, person); the oldest goes (CP7).
pub const MAX_PENDING_PER_PERSON: usize = 3;
/// Outstanding codes per App; the oldest goes.
pub const MAX_PENDING_PER_APP: usize = 256;
/// Outstanding codes on the box; the oldest goes.
pub const MAX_OUTSTANDING: usize = 1024;
/// Starts per (App, person) an hour (CP7).
pub const STARTS_PER_HOUR: usize = 10;
/// A pairing's state stays readable this long after its code ends (CP10).
pub const STATUS_KEEP_SECS: i64 = 900;
/// A web pass at most this old may start without the password (CP4).
pub const FRESH_SIGNIN_SECS: i64 = 900;
/// The link path on the App's host (V8: one constant for the AASA
/// components, the assetlinks guide and the landing page).
pub const PAIR_PATH: &str = "/.well-known/k2-pair";
/// `pairingId`s start with this.
pub const ID_PREFIX: &str = "apr_";
/// Config limits (§5.1).
pub const MAX_IOS_APPS: usize = 10;
pub const MAX_ANDROID_APPS: usize = 10;
pub const MAX_FINGERPRINTS: usize = 5;
pub const MAX_NATIVE_ORIGINS: usize = 5;
pub const MAX_LINK_PATHS: usize = 20;
pub const MAX_HOSTS: usize = 20;
/// The Apple App Store link prefix the landing page may show.
pub const STORE_IOS_PREFIX: &str = "https://apps.apple.com/";
/// The Google Play link prefix the landing page may show.
pub const STORE_ANDROID_PREFIX: &str = "https://play.google.com/store/apps/details?id=";

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Ended pairings kept for status at most (oldest ended go first).
const MAX_TRACKED: usize = 4096;

// ── codes (in memory) ────────────────────────────────────────────────

/// One outstanding code (never the code itself: it is the map's key, by
/// digest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The App's service id.
    pub app: String,
    pub principal_id: String,
    /// The web pass that minted it (`tokens.id`).
    pub minted_from: String,
    /// The host the QR names.
    pub host: String,
    pub pairing_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    /// Mint order (the oldest goes first at a cap, even within a second).
    pub seq: u64,
}

/// A pairing's state, as the web page sees it (CP10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Pending,
    Paired { device_name: String, device_login_id: String, paired_at: i64 },
    /// It ran out, or a redeem used it and was refused.
    Expired,
    /// The person cancelled it, a newer code replaced it, or the owner
    /// turned pairing off or changed the App's phone apps.
    Cancelled,
}

impl State {
    pub fn as_str(&self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Paired { .. } => "paired",
            State::Expired => "expired",
            State::Cancelled => "cancelled",
        }
    }
}

struct Track {
    app: String,
    principal_id: String,
    state: State,
    expires_at: i64,
    ended_at: Option<i64>,
}

#[derive(Default)]
struct Store {
    /// digest(code body) → the pending code.
    codes: HashMap<String, Pending>,
    /// pairingId → its state.
    ids: HashMap<String, Track>,
    /// (App, person) → start times inside the last hour.
    starts: HashMap<(String, String), VecDeque<i64>>,
    /// The next mint's [`Pending::seq`].
    next_seq: u64,
}

impl Store {
    fn end(&mut self, pairing_id: &str, state: State, at: i64) {
        if let Some(t) = self.ids.get_mut(pairing_id) {
            if t.state == State::Pending {
                t.state = state;
                t.ended_at = Some(at);
            }
        }
    }

    fn drop_code(&mut self, key: &str, state: State, at: i64) {
        if let Some(p) = self.codes.remove(key) {
            self.end(&p.pairing_id, state, at);
        }
    }

    /// Expired codes end; ended states older than [`STATUS_KEEP_SECS`] go;
    /// start times older than an hour go.
    fn sweep(&mut self, now: i64) {
        let gone: Vec<(String, i64)> =
            self.codes.iter().filter(|(_, p)| p.expires_at <= now).map(|(k, p)| (k.clone(), p.expires_at)).collect();
        for (k, at) in gone {
            self.drop_code(&k, State::Expired, at);
        }
        self.ids.retain(|_, t| t.ended_at.is_none_or(|e| now - e < STATUS_KEEP_SECS));
        if self.ids.len() > MAX_TRACKED {
            let mut ended: Vec<(String, i64)> =
                self.ids.iter().filter_map(|(k, t)| t.ended_at.map(|e| (k.clone(), e))).collect();
            ended.sort_by_key(|(_, e)| *e);
            let extra = self.ids.len() - MAX_TRACKED;
            for (k, _) in ended.into_iter().take(extra) {
                self.ids.remove(&k);
            }
        }
        self.starts.retain(|_, q| {
            while q.front().is_some_and(|t| now - *t >= 3600) {
                q.pop_front();
            }
            !q.is_empty()
        });
    }

    fn oldest_where(&self, f: impl Fn(&Pending) -> bool) -> Option<String> {
        self.codes.iter().filter(|(_, p)| f(p)).min_by_key(|(_, p)| p.seq).map(|(k, _)| k.clone())
    }
}

fn store() -> MutexGuard<'static, Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::default())).lock().unwrap_or_else(|p| p.into_inner())
}

fn digest(body: &str) -> String {
    Sha256::digest(body.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The canonical form of a typed code: upper case, separators and the
/// optional `K2A` prefix dropped, Crockford look-alikes folded (`I`/`L` →
/// `1`, `O` → `0`). `None` unless exactly [`CODE_CHARS`] alphabet
/// characters remain (a Companion `K2P-…` code is too long: `None`).
pub fn normalize(raw: &str) -> Option<String> {
    let up: String = raw.chars().filter(|c| !matches!(c, '-' | ' ' | '_')).collect::<String>().to_ascii_uppercase();
    let body = match up.strip_prefix(CODE_PREFIX) {
        Some(rest) if rest.len() == CODE_CHARS => rest,
        _ => up.as_str(),
    };
    let folded: String = body
        .chars()
        .map(|c| match c {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        })
        .collect();
    (folded.len() == CODE_CHARS && folded.bytes().all(|b| ALPHABET.contains(&b))).then_some(folded)
}

fn random_body() -> String {
    let mut bytes = [0u8; CODE_CHARS];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect()
}

fn display(body: &str) -> String {
    let groups: Vec<&str> = (0..body.len()).step_by(5).map(|i| &body[i..(i + 5).min(body.len())]).collect();
    format!("{CODE_PREFIX}-{}", groups.join("-"))
}

fn new_pairing_id() -> String {
    let mut out = String::from(ID_PREFIX);
    let mut produced = 0;
    let mut scratch = [0u8; 32];
    while produced < 22 {
        OsRng.fill_bytes(&mut scratch);
        for &b in scratch.iter() {
            if b >= 248 {
                continue;
            }
            out.push(BASE62[(b % 62) as usize] as char);
            produced += 1;
            if produced == 22 {
                break;
            }
        }
    }
    out
}

/// True iff `raw` looks like a `pairingId`.
pub fn is_pairing_id(raw: &str) -> bool {
    raw.len() == ID_PREFIX.len() + 22 && raw.starts_with(ID_PREFIX) && raw[ID_PREFIX.len()..].bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A freshly minted code (shown once).
#[derive(Debug, Clone)]
pub struct Minted {
    /// `K2A-XXXXX-XXXXX-XXXXX-XXXXX`.
    pub code: String,
    pub pairing_id: String,
    pub host: String,
    pub expires_at: i64,
}

/// Why a start didn't mint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    /// [`STARTS_PER_HOUR`] reached for this (App, person).
    RateLimited { retry_after_ms: u64 },
}

/// Mint a code for `principal_id` on `app`, from the web pass
/// `minted_from`, for `host`. Authorisation (the pass, freshness, the
/// App's pairing being on, the host being one of the App's) is the
/// route's job.
pub fn mint(app: &str, principal_id: &str, minted_from: &str, host: &str) -> Result<Minted, MintError> {
    let now = now_secs();
    let mut s = store();
    s.sweep(now);
    let key = (app.to_string(), principal_id.to_string());
    if let Some(q) = s.starts.get(&key) {
        if q.len() >= STARTS_PER_HOUR {
            let oldest = q.front().copied().unwrap_or(now);
            let wait = (oldest + 3600 - now).max(1) as u64;
            return Err(MintError::RateLimited { retry_after_ms: wait * 1000 });
        }
    }
    s.starts.entry(key).or_default().push_back(now);
    while s.codes.values().filter(|p| p.app == app && p.principal_id == principal_id).count() >= MAX_PENDING_PER_PERSON {
        let Some(k) = s.oldest_where(|p| p.app == app && p.principal_id == principal_id) else { break };
        s.drop_code(&k, State::Cancelled, now);
    }
    while s.codes.values().filter(|p| p.app == app).count() >= MAX_PENDING_PER_APP {
        let Some(k) = s.oldest_where(|p| p.app == app) else { break };
        s.drop_code(&k, State::Cancelled, now);
    }
    while s.codes.len() >= MAX_OUTSTANDING {
        let Some(k) = s.oldest_where(|_| true) else { break };
        s.drop_code(&k, State::Cancelled, now);
    }
    let body = random_body();
    let pairing_id = new_pairing_id();
    let expires_at = now + CODE_TTL_SECS;
    s.next_seq += 1;
    let seq = s.next_seq;
    s.codes.insert(
        digest(&body),
        Pending {
            app: app.to_string(),
            principal_id: principal_id.to_string(),
            minted_from: minted_from.to_string(),
            host: host.to_string(),
            pairing_id: pairing_id.clone(),
            created_at: now,
            expires_at,
            seq,
        },
    );
    s.ids.insert(
        pairing_id.clone(),
        Track { app: app.to_string(), principal_id: principal_id.to_string(), state: State::Pending, expires_at, ended_at: None },
    );
    Ok(Minted { code: display(&body), pairing_id, host: host.to_string(), expires_at })
}

/// Why a check or a redeem refused. One wire answer for all of them
/// (`pair_code_invalid`, no oracle); the reason is for the audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Unknown, already used, cancelled or malformed.
    Unknown,
    Expired,
    /// The code is another App's.
    WrongApp,
}

impl Refused {
    pub fn as_str(self) -> &'static str {
        match self {
            Refused::Unknown => "unknown",
            Refused::Expired => "expired",
            Refused::WrongApp => "wrong_app",
        }
    }
}

/// What a still-valid code would pair as (`pairing.check`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub app: String,
    pub principal_id: String,
    pub pairing_id: String,
    /// Seconds left (at least 1).
    pub expires_in_secs: i64,
}

/// Look at `raw` for `app` WITHOUT consuming it (CP15).
pub fn check(raw: &str, app: &str) -> Result<Checked, Refused> {
    let Some(body) = normalize(raw) else { return Err(Refused::Unknown) };
    let now = now_secs();
    let s = store();
    let Some(p) = s.codes.get(&digest(&body)) else { return Err(Refused::Unknown) };
    if p.expires_at <= now {
        return Err(Refused::Expired);
    }
    if p.app != app {
        return Err(Refused::WrongApp);
    }
    Ok(Checked {
        app: p.app.clone(),
        principal_id: p.principal_id.clone(),
        pairing_id: p.pairing_id.clone(),
        expires_in_secs: (p.expires_at - now).max(1),
    })
}

/// Take `raw` for a redeem on `app`: the code is consumed by the attempt
/// whatever the outcome (CP16), so two racing redeems pair once. A
/// refused take ends the pairing (`expired`). On `Ok` the caller ends it
/// with [`finish_paired`] or [`finish_failed`].
pub fn take(raw: &str, app: &str) -> Result<Pending, Refused> {
    let Some(body) = normalize(raw) else { return Err(Refused::Unknown) };
    let now = now_secs();
    let mut s = store();
    let Some(p) = s.codes.remove(&digest(&body)) else { return Err(Refused::Unknown) };
    if p.expires_at <= now {
        s.end(&p.pairing_id, State::Expired, now);
        return Err(Refused::Expired);
    }
    if p.app != app {
        s.end(&p.pairing_id, State::Expired, now);
        return Err(Refused::WrongApp);
    }
    Ok(p)
}

/// A taken code paired a phone.
pub fn finish_paired(pairing_id: &str, device_name: &str, device_login_id: &str) {
    let now = now_secs();
    store().end(
        pairing_id,
        State::Paired { device_name: device_name.to_string(), device_login_id: device_login_id.to_string(), paired_at: now },
        now,
    );
}

/// A taken code was refused at the redeem's re-checks.
pub fn finish_failed(pairing_id: &str) {
    store().end(pairing_id, State::Expired, now_secs());
}

/// A pairing's state for its own person on its own App (CP10): another
/// person's or another App's id is `None`, exactly like an unknown one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// While pending: seconds left.
    pub expires_in_secs: Option<i64>,
}

pub fn status(app: &str, principal_id: &str, pairing_id: &str) -> Option<Status> {
    let now = now_secs();
    let mut s = store();
    s.sweep(now);
    let t = s.ids.get(pairing_id).filter(|t| t.app == app && t.principal_id == principal_id)?;
    let left = (t.state == State::Pending).then(|| (t.expires_at - now).max(1));
    Some(Status { state: t.state.clone(), expires_in_secs: left })
}

/// The person drops one of their own codes (CP11). `false` for a miss,
/// someone else's or an ended one (the route answers the same).
pub fn cancel(app: &str, principal_id: &str, pairing_id: &str) -> bool {
    let now = now_secs();
    let mut s = store();
    let key = s
        .codes
        .iter()
        .find(|(_, p)| p.pairing_id == pairing_id && p.app == app && p.principal_id == principal_id)
        .map(|(k, _)| k.clone());
    match key {
        Some(k) => {
            s.drop_code(&k, State::Cancelled, now);
            true
        }
        None => false,
    }
}

/// Every outstanding code of `app` dies (pairing off, the phone apps
/// changed, the App removed). Returns how many.
pub fn drop_app(app: &str) -> usize {
    let now = now_secs();
    let mut s = store();
    let keys: Vec<String> = s.codes.iter().filter(|(_, p)| p.app == app).map(|(k, _)| k.clone()).collect();
    for k in &keys {
        s.drop_code(k, State::Cancelled, now);
    }
    keys.len()
}

/// Outstanding codes of one App (status counts).
pub fn outstanding_for(app: &str) -> usize {
    store().codes.values().filter(|p| p.app == app).count()
}

/// Test seam: every outstanding code's map key (a 64-hex digest, never the
/// code).
#[cfg(any(test, feature = "test-util"))]
pub fn code_keys() -> Vec<String> {
    store().codes.keys().cloned().collect()
}

/// Test seam: make one pairing's code run out now.
#[cfg(any(test, feature = "test-util"))]
pub fn expire_for_test(pairing_id: &str) {
    let mut s = store();
    for p in s.codes.values_mut().filter(|p| p.pairing_id == pairing_id) {
        p.expires_at = now_secs() - 1;
    }
}

/// Test seam: forget the starts counted for one (App, person).
#[cfg(any(test, feature = "test-util"))]
pub fn reset_starts_for_test(app: &str, principal_id: &str) {
    store().starts.remove(&(app.to_string(), principal_id.to_string()));
}

// ── the owner's config (skin.db) ─────────────────────────────────────

/// skin.db DDL (run by the idempotent open block).
pub(super) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_pairing_config (
            app_id TEXT PRIMARY KEY,
            enabled INTEGER NOT NULL DEFAULT 1,
            ios_apps TEXT NOT NULL DEFAULT '[]',
            android_apps TEXT NOT NULL DEFAULT '[]',
            hosts TEXT,
            native_origins TEXT NOT NULL DEFAULT '[]',
            link_paths TEXT NOT NULL DEFAULT '[]',
            store_ios TEXT,
            store_android TEXT,
            updated_at INTEGER NOT NULL,
            updated_by TEXT
         );
         CREATE TABLE IF NOT EXISTS device_login_pairings (
            device_login_id TEXT PRIMARY KEY,
            app_id TEXT NOT NULL,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            pairing_id TEXT NOT NULL,
            minted_from TEXT NOT NULL,
            host TEXT NOT NULL,
            paired_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS device_login_pairings_app ON device_login_pairings(app_id, paired_at);",
    )
    .map_err(|e| format!("skin.db app pairing: {e}"))
}

/// One Android app the App's links open (assetlinks.json).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AndroidApp {
    pub package: String,
    /// SHA-256 signing certificate fingerprints, uppercase colon hex.
    pub sha256: Vec<String>,
}

/// One App's pairing config (all non-secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub app: String,
    /// The owner's off switch (`false` = turned off; on again = a set).
    pub enabled: bool,
    /// `TEAMID.bundle.id`.
    pub ios: Vec<String>,
    pub android: Vec<AndroidApp>,
    /// `None` = every pairing host of the App.
    pub hosts: Option<Vec<String>>,
    pub native_origins: Vec<String>,
    pub link_paths: Vec<String>,
    pub store_ios: Option<String>,
    pub store_android: Option<String>,
    pub updated_at: i64,
    pub updated_by: Option<String>,
}

impl Config {
    /// Pairing is on: not turned off, and at least one phone app named.
    pub fn is_on(&self) -> bool {
        self.enabled && (!self.ios.is_empty() || !self.android.is_empty())
    }
}

/// What the owner sends to set (checked by [`parse_input`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigInput {
    pub ios: Vec<String>,
    pub android: Vec<AndroidApp>,
    pub hosts: Option<Vec<String>>,
    pub native_origins: Vec<String>,
    pub link_paths: Vec<String>,
    pub store_ios: Option<String>,
    pub store_android: Option<String>,
}

fn json_list<T: serde::de::DeserializeOwned>(raw: &str) -> Vec<T> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn config_conn(conn: &Connection, app: &str) -> Result<Option<Config>, String> {
    conn.query_row(
        "SELECT app_id, enabled, ios_apps, android_apps, hosts, native_origins, link_paths, store_ios, store_android, updated_at, updated_by
           FROM app_pairing_config WHERE app_id = ?1",
        params![app],
        map_config,
    )
    .optional()
    .map_err(|e| format!("app pairing config: {e}"))
}

fn map_config(r: &rusqlite::Row<'_>) -> rusqlite::Result<Config> {
    let hosts: Option<String> = r.get(4)?;
    Ok(Config {
        app: r.get(0)?,
        enabled: r.get::<_, i64>(1)? != 0,
        ios: json_list(&r.get::<_, String>(2)?),
        android: json_list(&r.get::<_, String>(3)?),
        hosts: hosts.map(|h| json_list(&h)),
        native_origins: json_list(&r.get::<_, String>(5)?),
        link_paths: json_list(&r.get::<_, String>(6)?),
        store_ios: r.get(7)?,
        store_android: r.get(8)?,
        updated_at: r.get(9)?,
        updated_by: r.get(10)?,
    })
}

/// One App's config, if the owner ever set one.
pub fn config(app: &str) -> Result<Option<Config>, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| config_conn(conn, &app))
}

/// Every App's config (the daemon's link-file snapshot at boot).
pub fn all_configs() -> Result<Vec<Config>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT app_id, enabled, ios_apps, android_apps, hosts, native_origins, link_paths, store_ios, store_android, updated_at, updated_by
                   FROM app_pairing_config ORDER BY app_id",
            )
            .map_err(|e| format!("app pairing configs: {e}"))?;
        let rows = stmt.query_map([], map_config).map_err(|e| format!("app pairing configs: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("app pairing configs: {e}"))
    })
}

/// Set an App's config (the owner, after a fresh confirm: the route's
/// job). Turns pairing on. Every outstanding code of the App dies (§4.6).
pub fn set_config(app: &str, input: &ConfigInput, actor: &str) -> Result<Config, String> {
    let app = normalize_app_target(Some(app));
    if app == HOST_APP_ID {
        return Err("pairing is per published App, never the box login".into());
    }
    let r = with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_pairing_config (app_id, enabled, ios_apps, android_apps, hosts, native_origins, link_paths, store_ios, store_android, updated_at, updated_by)
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(app_id) DO UPDATE SET enabled = 1, ios_apps = ?2, android_apps = ?3, hosts = ?4, native_origins = ?5,
                    link_paths = ?6, store_ios = ?7, store_android = ?8, updated_at = ?9, updated_by = ?10",
            params![
                app,
                j(&input.ios),
                j(&input.android),
                input.hosts.as_ref().map(|h| j(h.as_slice())),
                j(&input.native_origins),
                j(&input.link_paths),
                input.store_ios,
                input.store_android,
                now_secs(),
                actor
            ],
        )
        .map_err(|e| format!("app pairing config write: {e}"))?;
        config_conn(conn, &app)?.ok_or_else(|| "app pairing config vanished".to_string())
    })?;
    drop_app(&app);
    Ok(r)
}

fn j<T: Serialize + ?Sized>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

/// Turn an App's pairing off (CP23). Every outstanding code dies; paired
/// phones stay (they are ordinary device sign-ins). `Ok(true)` when it
/// was on.
pub fn set_off(app: &str, actor: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    let was = with_conn(|conn| {
        let before = config_conn(conn, &app)?;
        conn.execute(
            "UPDATE app_pairing_config SET enabled = 0, updated_at = ?2, updated_by = ?3 WHERE app_id = ?1",
            params![app, now_secs(), actor],
        )
        .map_err(|e| format!("app pairing off: {e}"))?;
        Ok(before.is_some_and(|c| c.is_on()))
    })?;
    drop_app(&app);
    Ok(was)
}

/// An App went away: its config goes, and its codes die. Its pairing rows
/// stay with their device sign-ins (which the App's removal ends).
pub fn purge_app(app: &str) -> Result<usize, String> {
    let app = normalize_app_target(Some(app));
    drop_app(&app);
    with_conn(|conn| conn.execute("DELETE FROM app_pairing_config WHERE app_id = ?1", params![app]).map_err(|e| format!("app pairing purge: {e}")))
}

// ── validation (§5.1, V11) ───────────────────────────────────────────

fn str_list(v: &serde_json::Value, key: &str, max: usize) -> Result<Vec<String>, String> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(a)) => {
            if a.len() > max {
                return Err(format!("{key}: at most {max}"));
            }
            a.iter()
                .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or_else(|| format!("{key}: a list of strings")))
                .collect()
        }
        Some(_) => Err(format!("{key}: a list of strings")),
    }
}

/// `TEAMID.bundle.id`: a 10-character `[A-Z0-9]` team id, then a
/// reverse-DNS bundle id of `[A-Za-z0-9.-]`, 1 to 155 characters.
pub fn valid_ios_app(s: &str) -> bool {
    let Some((team, bundle)) = s.split_once('.') else { return false };
    team.len() == 10
        && team.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && (1..=155).contains(&bundle.len())
        && bundle.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && bundle.contains('.')
        && bundle.split('.').all(|seg| !seg.is_empty())
}

/// An Android package: `[a-zA-Z][a-zA-Z0-9_]*` segments, at least two, at
/// most 255 characters.
pub fn valid_package(s: &str) -> bool {
    let segs: Vec<&str> = s.split('.').collect();
    s.len() <= 255
        && segs.len() >= 2
        && segs.iter().all(|seg| {
            seg.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// A SHA-256 fingerprint as 32 colon-separated hex pairs; uppercase out.
pub fn normalize_fingerprint(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    (parts.len() == 32 && parts.iter().all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))).then(|| s.trim().to_ascii_uppercase())
}

/// A webview's Origin: `scheme://host[:port]`, no path. `https://` only
/// for `localhost` and `tauri.localhost` (Capacitor Android, Tauri); any
/// other https origin is a real website (V11). Lowercase out.
pub fn normalize_native_origin(s: &str) -> Option<String> {
    let o = s.trim().to_ascii_lowercase();
    let (scheme, rest) = o.split_once("://")?;
    if scheme.is_empty()
        || !scheme.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        || !scheme.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
    {
        return None;
    }
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
        return None;
    }
    let host = match rest.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => rest,
    };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')) {
        return None;
    }
    if scheme == "https" && !matches!(host, "localhost" | "tauri.localhost") {
        return None;
    }
    Some(o)
}

/// An extra universal-link path the App owns: `/`, then up to 200 of
/// `A-Za-z0-9._~*/-`; never under `/.well-known/` (K2's own there).
pub fn valid_link_path(s: &str) -> bool {
    s.starts_with('/')
        && s.len() <= 201
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~*/-".contains(&b))
        && !s.to_ascii_lowercase().starts_with("/.well-known")
}

/// A host name (lowercase, no port, no trailing dot).
pub fn normalize_host(s: &str) -> Option<String> {
    let h = s.trim().trim_end_matches('.').to_ascii_lowercase();
    (!h.is_empty()
        && h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')))
    .then_some(h)
}

fn store_url(s: &str, prefix: &str, field: &str) -> Result<String, String> {
    let s = s.trim();
    let ok = s.starts_with(prefix)
        && s.len() > prefix.len()
        && s.len() <= 512
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~/?=&%+:".contains(&b));
    if ok {
        Ok(s.to_string())
    } else {
        Err(format!("{field}: a link starting {prefix}"))
    }
}

/// The set body (`POST /cli/app/pairing/set`), checked. Errors name the
/// field. `app` and `confirmToken` are the route's.
pub fn parse_input(v: &serde_json::Value) -> Result<ConfigInput, String> {
    let Some(o) = v.as_object() else { return Err("send a JSON object".into()) };
    const KNOWN: &[&str] = &["app", "confirmToken", "ios", "android", "hosts", "nativeOrigins", "linkPaths", "store"];
    if let Some(k) = o.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return Err(format!("unknown field {shown}"));
    }
    let mut ios = Vec::new();
    for s in str_list(v, "ios", MAX_IOS_APPS)? {
        if !valid_ios_app(&s) {
            return Err("ios: each is TEAMID.bundle.id (a 10-character team id of A-Z and 0-9, then your app's bundle id)".into());
        }
        if !ios.contains(&s) {
            ios.push(s);
        }
    }
    let mut android: Vec<AndroidApp> = Vec::new();
    match v.get("android") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Array(a)) => {
            if a.len() > MAX_ANDROID_APPS {
                return Err(format!("android: at most {MAX_ANDROID_APPS} apps"));
            }
            for x in a {
                let package = x.get("package").and_then(|p| p.as_str()).map(str::trim).unwrap_or("");
                if !valid_package(package) {
                    return Err("android: package is a Java package name like com.you.app".into());
                }
                let fprs = str_list(x, "sha256", MAX_FINGERPRINTS).map_err(|e| format!("android.{e}"))?;
                if fprs.is_empty() {
                    return Err(format!("android: {package} needs at least one sha256 signing fingerprint"));
                }
                let mut sha256 = Vec::new();
                for f in fprs {
                    let n = normalize_fingerprint(&f)
                        .ok_or_else(|| "android.sha256: each is 32 bytes as colon hex (AB:CD:…), from your signing certificate".to_string())?;
                    if !sha256.contains(&n) {
                        sha256.push(n);
                    }
                }
                if x.as_object().is_some_and(|m| m.keys().any(|k| k != "package" && k != "sha256")) {
                    return Err("android: each is {package, sha256: [...]}".into());
                }
                match android.iter_mut().find(|a| a.package == package) {
                    Some(prev) => {
                        for f in sha256 {
                            if !prev.sha256.contains(&f) {
                                prev.sha256.push(f);
                            }
                        }
                        if prev.sha256.len() > MAX_FINGERPRINTS {
                            return Err(format!("android.sha256: at most {MAX_FINGERPRINTS} per package"));
                        }
                    }
                    None => android.push(AndroidApp { package: package.to_string(), sha256 }),
                }
            }
        }
        Some(_) => return Err("android: a list of {package, sha256: [...]}".into()),
    }
    if ios.is_empty() && android.is_empty() {
        return Err("ios and/or android: name at least one phone app (TEAMID.bundle.id, or {package, sha256})".into());
    }
    let hosts = match v.get("hosts") {
        None | Some(serde_json::Value::Null) => None,
        Some(_) => {
            let mut out = Vec::new();
            for h in str_list(v, "hosts", MAX_HOSTS)? {
                let n = normalize_host(&h).ok_or_else(|| "hosts: each is a host name, like jobs.you.k2.dev".to_string())?;
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            if out.is_empty() {
                return Err("hosts: leave it out for every host of the App, or name at least one".into());
            }
            Some(out)
        }
    };
    let mut native_origins = Vec::new();
    for s in str_list(v, "nativeOrigins", MAX_NATIVE_ORIGINS)? {
        let n = normalize_native_origin(&s).ok_or_else(|| {
            "nativeOrigins: each is scheme://host with no path, like tauri://localhost or capacitor://localhost (https only for localhost)"
                .to_string()
        })?;
        if !native_origins.contains(&n) {
            native_origins.push(n);
        }
    }
    let mut link_paths = Vec::new();
    for s in str_list(v, "linkPaths", MAX_LINK_PATHS)? {
        if !valid_link_path(&s) {
            return Err("linkPaths: each is a path like /jobs/* (A-Z a-z 0-9 . _ ~ * / -), never under /.well-known/".into());
        }
        if !link_paths.contains(&s) {
            link_paths.push(s);
        }
    }
    let (mut store_ios, mut store_android) = (None, None);
    match v.get("store") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(m)) => {
            if let Some(k) = m.keys().find(|k| *k != "ios" && *k != "android") {
                let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
                return Err(format!("store: ios and android only (not {shown})"));
            }
            if let Some(s) = m.get("ios").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()) {
                store_ios = Some(store_url(s, STORE_IOS_PREFIX, "store.ios")?);
            }
            if let Some(s) = m.get("android").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()) {
                store_android = Some(store_url(s, STORE_ANDROID_PREFIX, "store.android")?);
            }
        }
        Some(_) => return Err("store: {ios?, android?}".into()),
    }
    Ok(ConfigInput { ios, android, hosts, native_origins, link_paths, store_ios, store_android })
}

// ── the minting pass ─────────────────────────────────────────────────

/// A live login session pass, as the start route needs it (CP4: its age).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassInfo {
    pub principal_id: String,
    pub username: String,
    pub app: String,
    pub created_at: i64,
}

/// The live login session `token_id` (revoked, expired or a platform pass
/// is `None`).
pub fn session_info(token_id: &str) -> Result<Option<PassInfo>, String> {
    let now = now_secs();
    with_conn(|conn| {
        conn.query_row(
            "SELECT t.principal_id, p.username, COALESCE(t.app_service_id, ?3), t.created_at
               FROM tokens t JOIN principals p ON p.id = t.principal_id
              WHERE t.id = ?1 AND t.session = 1 AND t.revoked_at IS NULL AND (t.expires_at IS NULL OR t.expires_at > ?2)",
            params![token_id, now, HOST_APP_ID],
            |r| Ok(PassInfo { principal_id: r.get(0)?, username: r.get(1)?, app: r.get(2)?, created_at: r.get(3)? }),
        )
        .optional()
        .map_err(|e| format!("pairing pass lookup: {e}"))
    })
}

// ── the redeem's mint (CP17, CP18, V13) ──────────────────────────────

/// What a redeem's mint did.
#[derive(Debug)]
pub enum Paired {
    Ok { session: Box<app_login::DeviceSession>, principal_id: String },
    /// A re-check failed (the reason is for the audit trail; the wire
    /// answer is the generic `pair_code_invalid`).
    Refused(&'static str),
}

/// Mint the paired phone's device sign-in for `p` (a code [`take`] gave
/// back), in one skin.db transaction: the minting web pass is still live
/// and is this person's on this App, the App's pairing is on and its "Stay
/// signed in" isn't 0, the person still holds the App's grant (the mint
/// refuses otherwise; a tied person is held to the tie's ceiling), then the
/// chain, the pass's cap and the pairing row. `app_ws` is the App's
/// workspace, read by the caller **before** the skin mutex (A21). A test
/// guest pairs only inside its App's link context
/// ([`super::test_guests::with_link_login`]).
pub fn login_device_paired(p: &Pending, device: &app_login::Device, app_ws: Option<&str>) -> Result<Paired, String> {
    let app = normalize_app_target(Some(&p.app));
    if app == HOST_APP_ID {
        return Ok(Paired::Refused("host"));
    }
    let r = with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let now = now_secs();
        let web: Option<(String, String)> = tx
            .query_row(
                "SELECT principal_id, COALESCE(app_service_id, ?3) FROM tokens
                  WHERE id = ?1 AND session = 1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?2)",
                params![p.minted_from, now, HOST_APP_ID],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("pairing web pass: {e}"))?;
        if web.as_ref().is_none_or(|(pid, a)| *pid != p.principal_id || *a != app) {
            return Ok(Paired::Refused("web_pass_ended"));
        }
        if !config_conn(&tx, &app)?.is_some_and(|c| c.is_on()) {
            return Ok(Paired::Refused("pairing_off"));
        }
        let stay = app_login::settings_conn(&tx, &app)?.stay_signed_in_days;
        if stay == 0 {
            return Ok(Paired::Refused("device_signin_off"));
        }
        let username: Option<String> = tx
            .query_row("SELECT username FROM principals WHERE id = ?1", params![p.principal_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("pairing principal: {e}"))?;
        let Some(username) = username else { return Ok(Paired::Refused("person_gone")) };
        let session = match mint_app_session_conn(&tx, &username, &app, app_ws) {
            Ok(s) => s,
            Err(e) if e == HOST_LOGIN_DENIED => return Ok(Paired::Refused("grant")),
            Err(e) => return Err(e),
        };
        let (chain, refresh, expires) = app_login::new_chain_conn(&tx, &p.principal_id, &app, device, &session.meta.id, stay)?;
        let pass_expires = app_login::cap_pass_expiry(&tx, &session.meta.id, expires)?;
        tx.execute(
            "INSERT INTO device_login_pairings (device_login_id, app_id, principal_id, pairing_id, minted_from, host, paired_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![chain, app, p.principal_id, p.pairing_id, p.minted_from, p.host, now],
        )
        .map_err(|e| format!("pairing row: {e}"))?;
        tx.commit().map_err(|e| format!("pairing commit: {e}"))?;
        Ok(Paired::Ok {
            session: Box::new(app_login::DeviceSession {
                session,
                refresh,
                refresh_expires_at: expires,
                pass_expires_at: pass_expires,
                device_login_id: chain,
                device: device.clone(),
            }),
            principal_id: p.principal_id.clone(),
        })
    })?;
    Ok(match r {
        Paired::Ok { session, principal_id } => {
            let d = *session;
            Paired::Ok { session: Box::new(app_login::DeviceSession { session: app_login::finish(d.session), ..d }), principal_id }
        }
        other => other,
    })
}

// ── the owner's log and counts ───────────────────────────────────────

/// One phone that paired with an App (the owner's log). Never a code, a
/// pass or a refresh token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRow {
    pub device_login_id: String,
    pub pairing_id: String,
    pub principal_id: String,
    pub username: String,
    pub device_name: String,
    pub host: String,
    pub paired_at: i64,
    /// The device sign-in still works.
    pub live: bool,
    /// Why it ended, when it did.
    pub ended: Option<String>,
}

/// The App's pairings, newest first, at most `limit`.
pub fn pairings_for_app(app: &str, limit: usize) -> Result<Vec<PairingRow>, String> {
    let app = normalize_app_target(Some(app));
    let now = now_secs();
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT x.device_login_id, x.pairing_id, x.principal_id, COALESCE(p.username, ''), COALESCE(d.device_name, ''), x.host,
                        x.paired_at, d.revoked_at IS NULL AND d.expires_at > ?2, d.revoked_reason
                   FROM device_login_pairings x
                   LEFT JOIN device_logins d ON d.id = x.device_login_id
                   LEFT JOIN principals p ON p.id = x.principal_id
                  WHERE x.app_id = ?1
                  ORDER BY x.paired_at DESC, x.device_login_id
                  LIMIT ?3",
            )
            .map_err(|e| format!("app pairings: {e}"))?;
        let rows = stmt
            .query_map(params![app, now, limit as i64], |r| {
                Ok(PairingRow {
                    device_login_id: r.get(0)?,
                    pairing_id: r.get(1)?,
                    principal_id: r.get(2)?,
                    username: r.get(3)?,
                    device_name: r.get(4)?,
                    host: r.get(5)?,
                    paired_at: r.get(6)?,
                    live: r.get::<_, Option<bool>>(7)?.unwrap_or(false),
                    ended: r.get(8)?,
                })
            })
            .map_err(|e| format!("app pairings: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("app pairings: {e}"))
    })
}

/// `(live paired phones, pairings since `since`)` for one App.
pub fn counts(app: &str, since: i64) -> Result<(i64, i64), String> {
    let app = normalize_app_target(Some(app));
    let now = now_secs();
    with_conn(|conn| {
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM device_login_pairings x JOIN device_logins d ON d.id = x.device_login_id
                  WHERE x.app_id = ?1 AND d.revoked_at IS NULL AND d.expires_at > ?2",
                params![app, now],
                |r| r.get(0),
            )
            .map_err(|e| format!("app pairing counts: {e}"))?;
        let recent: i64 = conn
            .query_row("SELECT COUNT(*) FROM device_login_pairings WHERE app_id = ?1 AND paired_at >= ?2", params![app, since], |r| r.get(0))
            .map_err(|e| format!("app pairing counts: {e}"))?;
        Ok((live, recent))
    })
}

/// Has this App ever paired a phone? (the first-pairing inbox note)
pub fn first_pairing(app: &str) -> Result<bool, String> {
    let app = normalize_app_target(Some(app));
    with_conn(|conn| {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM device_login_pairings WHERE app_id = ?1", params![app], |r| r.get(0))
            .map_err(|e| format!("app pairing first: {e}"))?;
        Ok(n == 1)
    })
}

#[cfg(test)]
#[path = "skin_app_pairing_tests.rs"]
mod tests;
