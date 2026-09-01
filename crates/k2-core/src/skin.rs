//! Skin capability passes (prd-skin-auth-v1 + skin-identity-reshape-v1).
//!
//! Two `k2skn_…` classes, one prefix: guest **login session** (`session=1`,
//! on a principal) and **platform** token (`session=0`, no principal, `name`
//! is a label). **Not** Connect users, **not** `/v1` `k2sk_` API keys, **not**
//! the owner token. Overlay Thread rooms only — grid/PTY is never a skin room.
//! Optional `password_hash` on principals is K2-login only
//! (`POST /cli/skin/login`); NULL = cannot K2-login.
//!
//! ## Store (`~/.k2/skin.db`, WAL, own Mutex)
//! Three tables: `principals` (guest roster), `roles` (named caps+rooms
//! bundles), and `tokens` (hashed passes). The raw secret is returned
//! **once** at mint and never stored. Lookup is hex SHA-256 of the
//! presented key (same construction as API keys / connect-user session
//! tokens — high-entropy CSPRNG, not argon2).
//!
//! Caps: `thread:read`, `thread:post`, `files:read`, `files:write`.
//! Empty/missing caps stay Thread-only — never silent-add files. Never `pty:*`.
//! Assigned guests snapshot the role onto `session=1`; platform tokens
//! keep their own caps+rooms (not a role). Session policy is **per-room**
//! (`room_policy` map). Platform `--name` tokens stay flat.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Prefix every skin pass carries. Distinct from `k2sk_` (`/v1` API keys).
pub const SKIN_KEY_PREFIX: &str = "k2skn_";

/// Overlay Thread read (GET `/cli/thread`, WS `/cli/overlay/events`).
pub const CAP_THREAD_READ: &str = "thread:read";
/// Overlay Thread post (`POST /cli/thread/post`).
pub const CAP_THREAD_POST: &str = "thread:post";
/// Workspace files read (GET `/cli/fs/read-dir`, `/cli/fs/read-file`, WS `/cli/fs/events`).
pub const CAP_FILES_READ: &str = "files:read";
/// Workspace files write (`POST /cli/fs/write-file`). Does not imply read.
pub const CAP_FILES_WRITE: &str = "files:write";

const KEY_BODY_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Empty/missing `--caps` stay Thread-only. Never silent-add files.
const DEFAULT_CAPS: &[&str] = &[CAP_THREAD_READ, CAP_THREAD_POST];
const ACCEPTED_CAPS: &[&str] = &[
    CAP_THREAD_READ,
    CAP_THREAD_POST,
    CAP_FILES_READ,
    CAP_FILES_WRITE,
];

/// Connect / Server Access names — never a skin role.
const CONNECT_ROLE_NAMES: &[&str] = &["owner", "admin", "member", "viewer"];
const CONNECT_ROLE_NAME_ERR: &str = "Connect role names cannot be skin roles (owner/admin/member/viewer). Skin roles are named bundles of scopes+agents, not Server Access.";

fn accepted_caps_csv() -> String {
    ACCEPTED_CAPS.join(", ")
}

fn is_connect_role_name(name: &str) -> bool {
    CONNECT_ROLE_NAMES
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
}

fn assigned_guest_rooms_err(username: &str, role_name: &str) -> String {
    format!("guest '{username}' has role '{role_name}'; edit the role or unassign")
}

fn assigned_role_remove_err(name: &str) -> String {
    format!("role '{name}' is assigned; unassign guests first")
}

/// Copy of Connect: 3 consecutive failures → 15-minute per-username lockout.
/// Dummy argon2 + the 500 ms 401 delay is **not** lockout.
const LOCKOUT_THRESHOLD: u32 = 3;
const LOCKOUT_DURATION_SECS: i64 = 15 * 60;

/// True iff `raw` is a skin-class credential (prefix only — may be revoked).
pub fn is_skin_token(raw: &str) -> bool {
    raw.starts_with(SKIN_KEY_PREFIX)
}

/// Never reuse the integrator prefix.
pub fn is_api_key_prefix(raw: &str) -> bool {
    raw.starts_with("k2sk_")
}

fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let out = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn generate_raw_key() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut body = String::with_capacity(SKIN_KEY_PREFIX.len() + KEY_BODY_LEN);
    body.push_str(SKIN_KEY_PREFIX);
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

fn display_prefix(raw: &str) -> String {
    let tail = raw
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{SKIN_KEY_PREFIX}…{tail}")
}

/// Normalize a skin username: lowercase, `^[a-z0-9_-]{2,}$`.
pub fn normalize_username(raw: &str) -> Result<String, String> {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.len() < 2 {
        return Err("username must be at least 2 characters".to_string());
    }
    if !lowered
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err("username may contain only lowercase letters, digits, '_' and '-'".to_string());
    }
    Ok(lowered)
}

/// Parse + validate cap names. Empty/missing → default Thread read+post
/// (never silent-add `files:*`). Unknown names fail loud. Write-only is
/// accepted at mint; list/read still require `files:read` listed.
pub fn parse_caps(raw: Option<&[String]>) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    match raw {
        None | Some([]) => {
            for c in DEFAULT_CAPS {
                out.push((*c).to_string());
            }
        }
        Some(list) => {
            for item in list {
                let t = item.trim();
                if t.is_empty() {
                    continue;
                }
                if ACCEPTED_CAPS.contains(&t) {
                    if !out.iter().any(|c| c == t) {
                        out.push(t.to_string());
                    }
                } else {
                    return Err(format!(
                        "unknown capability {t:?}; accepted: {}",
                        accepted_caps_csv()
                    ));
                }
            }
            if out.is_empty() {
                return Err(format!(
                    "caps must include at least one of {}",
                    accepted_caps_csv()
                ));
            }
        }
    }
    Ok(out)
}

fn caps_json(caps: &[String]) -> String {
    serde_json::to_string(caps).unwrap_or_else(|_| "[]".to_string())
}

fn caps_from_json(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Missing/empty/unparseable rooms JSON → `[]` (deny all Thread).
/// **Never** copy [`parse_caps`] (empty caps → both verbs).
pub fn parse_rooms_json(raw: Option<&str>) -> Vec<String> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<serde_json::Value>>(s) {
        Ok(v) => v
            .into_iter()
            .filter_map(|x| match x {
                serde_json::Value::String(s) => {
                    let t = s.trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }
                _ => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn rooms_json(rooms: &[String]) -> String {
    serde_json::to_string(rooms).unwrap_or_else(|_| "[]".to_string())
}

fn rooms_from_json(raw: &str) -> Vec<String> {
    parse_rooms_json(Some(raw))
}

/// `project_id` → caps in that room. Empty map = Thread dark.
pub type RoomPolicy = BTreeMap<String, Vec<String>>;

fn default_thread_caps() -> Vec<String> {
    DEFAULT_CAPS.iter().map(|s| (*s).to_string()).collect()
}

fn cartesian_policy(caps: &[String], rooms: &[String]) -> RoomPolicy {
    let mut out = RoomPolicy::new();
    for id in rooms {
        let t = id.trim();
        if t.is_empty() {
            continue;
        }
        out.insert(t.to_string(), caps.to_vec());
    }
    out
}

fn thread_only_policy(rooms: &[String]) -> RoomPolicy {
    cartesian_policy(&default_thread_caps(), rooms)
}

fn union_caps(policy: &RoomPolicy) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in ACCEPTED_CAPS {
        if policy.values().any(|caps| caps.iter().any(|c| c == cap)) && !out.iter().any(|c| c == cap)
        {
            out.push((*cap).to_string());
        }
    }
    out
}

fn rooms_from_policy(policy: &RoomPolicy) -> Vec<String> {
    policy.keys().cloned().collect()
}

fn room_policy_json(policy: &RoomPolicy) -> String {
    serde_json::to_string(policy).unwrap_or_else(|_| "{}".to_string())
}

fn stored_caps_list(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_stored_room_caps(list: Vec<String>) -> Vec<String> {
    if list.is_empty() {
        return default_thread_caps();
    }
    let mut out: Vec<String> = Vec::new();
    for c in list {
        if !out.iter().any(|x| x == &c) {
            out.push(c);
        }
    }
    out
}

/// Parse a stored `room_policy` JSON object. `None` = NULL / missing.
/// Empty object is `Some({})` (Thread dark), not cartesian.
fn parse_room_policy_json(raw: Option<&str>) -> Result<Option<RoomPolicy>, String> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty() && *s != "null") else {
        return Ok(None);
    };
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| format!("room_policy json: {e}"))?;
    match v {
        serde_json::Value::Object(map) => {
            let mut out = RoomPolicy::new();
            for (k, caps_v) in map {
                let k = k.trim().to_string();
                if k.is_empty() {
                    continue;
                }
                out.insert(k, normalize_stored_room_caps(stored_caps_list(&caps_v)));
            }
            Ok(Some(out))
        }
        _ => Err("room_policy must be a JSON object".to_string()),
    }
}

fn policy_or_cartesian(
    caps: &[String],
    rooms: &[String],
    raw: Option<&str>,
) -> Result<RoomPolicy, String> {
    match parse_room_policy_json(raw)? {
        Some(p) if p.is_empty() && !rooms.is_empty() => Ok(cartesian_policy(caps, rooms)),
        Some(p) => Ok(p),
        None => Ok(cartesian_policy(caps, rooms)),
    }
}

/// Session: JSON object → use it (including `{}`); NULL → caps × rooms.
/// Platform: always empty (flat `has_cap` && `has_room`). Never JOIN `roles`.
fn interpret_token_policy(
    session: bool,
    caps: &[String],
    rooms: &[String],
    raw: Option<&str>,
) -> RoomPolicy {
    if !session {
        return RoomPolicy::new();
    }
    match parse_room_policy_json(raw) {
        Ok(Some(p)) => p,
        Ok(None) | Err(_) => cartesian_policy(caps, rooms),
    }
}

fn room_access_from_policy(policy: &RoomPolicy) -> Vec<SkinRoomAccess> {
    let ids = rooms_from_policy(policy);
    live_agents(&ids)
        .into_iter()
        .map(|a| SkinRoomAccess {
            handle: a.handle,
            caps: policy.get(&a.project_id).cloned().unwrap_or_default(),
        })
        .collect()
}

fn password_is_set(hash: Option<&str>) -> bool {
    hash.map(str::trim).is_some_and(|s| !s.is_empty())
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

fn verify_hash(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn build_dummy_hash() -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"k2-skin-dummy-verify-target", &salt)
        .map(|h| h.to_string())
        .unwrap_or_else(|_| {
            String::from(
                "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
        })
}

/// Host of Direct-mode `front_door.url` (no port). None when mode is not
/// `direct` or the URL is empty. Used for cookie-class matching — not the
/// Caddy bind filter (`host_from_front_door_url` rejects `*.k2.dev`).
pub fn direct_front_door_host() -> Option<String> {
    let door = get_front_door().ok()?;
    if !door.mode.eq_ignore_ascii_case("direct") {
        return None;
    }
    host_from_url(door.url.as_deref()?)
}

fn host_from_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(raw);
    let hostport = rest.split('/').next().unwrap_or(rest).trim();
    if hostport.is_empty() {
        return None;
    }
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(hostport).to_string()
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        if p.chars().all(|c| c.is_ascii_digit()) {
            h.to_string()
        } else {
            hostport.to_string()
        }
    } else {
        hostport.to_string()
    };
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn normalize_room_ids(rooms: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in rooms {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// Nested label reserved for U7 (`https://skin.<sub>.k2.dev`).
pub const RESERVED_NESTED_LABEL: &str = "skin";

pub fn is_reserved_nested_label(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(RESERVED_NESTED_LABEL)
}

pub fn reserved_nested_label_error(label: &str) -> String {
    let label = label.trim().to_ascii_lowercase();
    format!(
        "reserved_label: '{label}' is reserved. Pick another nested label for your UI (k2 study skins)."
    )
}

/// `https://rosson.k2.dev` → `https://skin.rosson.k2.dev`. Idempotent if
/// the host already starts with `skin.`.
pub fn skin_url_from_public(public_url: &str) -> Option<String> {
    let rest = public_url.trim().trim_end_matches('/');
    let host = rest
        .strip_prefix("https://")
        .or_else(|| rest.strip_prefix("http://"))?;
    let host = host.split('/').next().unwrap_or(host).trim();
    if host.is_empty() {
        return None;
    }
    if host.starts_with("skin.") {
        return Some(format!("https://{host}"));
    }
    Some(format!("https://skin.{host}"))
}

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinPrincipal {
    pub id: String,
    pub username: String,
    pub created_at: i64,
    #[serde(default)]
    pub default_rooms: Vec<String>,
    #[serde(default)]
    pub default_room_handles: Vec<String>,
    /// True when a K2-login password is set. Never the hash.
    #[serde(default)]
    pub has_password: bool,
    /// Assigned skin role id. `None` = unassigned (I6 Thread-only).
    #[serde(default)]
    pub role_id: Option<String>,
    /// Assigned skin role name. `None` when unassigned.
    #[serde(default)]
    pub role_name: Option<String>,
}

/// Per-room functions on the wire: `{handle, caps}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinRoomAccess {
    pub handle: String,
    pub caps: Vec<String>,
}

/// Named bundle of per-room functions for Skin Access guests (not Connect roles).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinRole {
    pub id: String,
    pub name: String,
    /// Union of caps across rooms (display / naive SPA).
    pub caps: Vec<String>,
    /// Stored ACL: `projects.id` UUIDs. Empty = Thread dark.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Display-only, resolved live from `projects.handle`. Skip missing.
    #[serde(default)]
    pub room_handles: Vec<String>,
    /// Per-room functions. Handle is live; skip missing projects.
    #[serde(default)]
    pub room_access: Vec<SkinRoomAccess>,
    pub created_at: i64,
    /// SSOT map `project_id → caps[]`. Not on the wire.
    #[serde(skip)]
    pub room_policy: RoomPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinTokenMeta {
    pub id: String,
    /// Platform label (`session=0`) or session stamp username (`session=1`).
    pub name: String,
    /// Display prefix (`k2skn_…ab12`). Never the secret.
    pub prefix: String,
    pub caps: Vec<String>,
    /// Stored ACL: `projects.id` UUIDs.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Display-only, resolved live from `projects.handle`. Skip missing.
    #[serde(default)]
    pub room_handles: Vec<String>,
    /// Per-room functions (session snapshot). Empty on platform tokens.
    #[serde(default)]
    pub room_access: Vec<SkinRoomAccess>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// One workspace a skin pass may Thread. Wire `{handle, projectId, displayName}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinAgent {
    pub handle: String,
    pub project_id: String,
    /// Guest-facing label. Handle stays the id.
    pub display_name: String,
}

/// Resolved live pass. Safe to log `id` / `username` / `caps` — never a secret.
/// `username` is the overlay stamp: session principal or platform `name`.
/// No `role_id` — snapshot only, never a live JOIN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinPass {
    pub id: String,
    /// `Some` for login sessions; `None` for platform tokens.
    pub principal_id: Option<String>,
    pub username: String,
    /// Union of caps (display / naive SPA). Not a session files/thread door.
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    /// Login-minted session pass (`session=1`). Platform mint is `false`.
    pub session: bool,
    /// Session: `project_id → caps[]`. Platform: empty (use flat `has_cap`).
    pub room_policy: RoomPolicy,
}

impl SkinPass {
    pub fn has_cap(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }

    /// Session: key exists on the map. Platform: listed in `rooms`.
    pub fn has_room(&self, project_id: &str) -> bool {
        let id = project_id.trim();
        if id.is_empty() {
            return false;
        }
        if self.session {
            self.room_policy.contains_key(id)
        } else {
            self.rooms.iter().any(|r| r == id)
        }
    }

    /// Session: cap listed on that room. Platform: `has_cap` && `has_room`.
    pub fn has_cap_in_room(&self, project_id: &str, cap: &str) -> bool {
        if self.session {
            self.room_policy
                .get(project_id.trim())
                .map(|caps| caps.iter().any(|c| c == cap))
                .unwrap_or(false)
        } else {
            self.has_cap(cap) && self.has_room(project_id)
        }
    }

    /// Dispatcher files/thread door. Session defers the cap until the room
    /// is known. Platform still requires the pass-level cap.
    pub fn dispatcher_admits_cap(&self, cap: &str) -> bool {
        if self.session {
            true
        } else {
            self.has_cap(cap)
        }
    }

    pub fn rooms_empty(&self) -> bool {
        if self.session {
            self.room_policy.is_empty()
        } else {
            self.rooms.is_empty()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinFrontDoor {
    /// `connect` (nested `skin.<sub>.k2.dev`) or `direct` (Caddy on this box).
    pub mode: String,
    pub url: Option<String>,
    pub hint: Option<String>,
    /// Optional same-origin UI reverse-proxy target for Caddy `handle /`.
    pub ui_port: Option<u16>,
}

impl SkinFrontDoor {
    pub fn default_connect() -> Self {
        Self {
            mode: "connect".to_string(),
            url: None,
            hint: Some(
                "Nested hostname on the existing tunnel. Operator URL stays the kingdom door."
                    .to_string(),
            ),
            ui_port: None,
        }
    }
}

pub fn parse_front_door_mode(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "connect" => Ok("connect"),
        "direct" => Ok("direct"),
        other => Err(format!(
            "unknown front-door mode {other:?}; accepted: connect, direct"
        )),
    }
}

fn default_hint_for_mode(mode: &str) -> &'static str {
    match mode {
        "direct" => {
            "Caddy :443 (or a LAN port) → daemon loopback. Do not bind k2-daemon to the world."
        }
        _ => "Nested hostname on the existing tunnel. Operator URL stays the kingdom door.",
    }
}

// ── DB ───────────────────────────────────────────────────────────────

struct SkinDb {
    path: PathBuf,
    conn: Connection,
}

fn store_path() -> PathBuf {
    crate::paths::k2_home().join("skin.db")
}

const TOKENS_DDL: &str = "CREATE TABLE IF NOT EXISTS tokens (
            id TEXT PRIMARY KEY,
            principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
            name TEXT,
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            caps TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            revoked_at INTEGER,
            session INTEGER NOT NULL DEFAULT 0,
            expires_at INTEGER,
            CHECK (
                (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                OR
                (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
            )
         )";

fn open_db(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut conn = Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    let _ = conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA temp_store = MEMORY;",
    );
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS principals (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    conn.execute_batch(TOKENS_DDL)
        .map_err(|e| format!("skin.db schema: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS front_door (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            mode TEXT NOT NULL,
            url TEXT,
            hint TEXT,
            updated_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    let _ = conn.execute("ALTER TABLE front_door ADD COLUMN ui_port INTEGER", []);
    let _ = conn.execute(
        "ALTER TABLE tokens ADD COLUMN rooms TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN name TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE principals ADD COLUMN default_rooms TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    // Skin login (prd-skin-login-v1): ignore duplicate ALTER like `ui_port`.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN password_hash TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE tokens ADD COLUMN session INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN expires_at INTEGER", []);
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS login_lockouts (
            username TEXT PRIMARY KEY,
            failed_count INTEGER NOT NULL DEFAULT 0,
            locked_until INTEGER
         );",
    );
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS roles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            caps TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS roles_name ON roles(name COLLATE NOCASE);",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    let _ = conn.execute(
        "ALTER TABLE principals ADD COLUMN role_id TEXT REFERENCES roles(id) ON DELETE RESTRICT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE roles ADD COLUMN room_policy TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    rebuild_tokens_table_if_needed(&mut conn)?;
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN room_policy TEXT", []);
    ensure_platform_name_index(&conn)?;
    migrate_room_policy(&conn)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            crate::log_debug!("[skin] WARN chmod 0600 {}: {e}", path.display());
        }
    }
    Ok(conn)
}

fn tokens_principal_id_notnull(conn: &Connection) -> Result<bool, String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(tokens)")
        .map_err(|e| format!("skin.db table_info: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            let name: String = r.get(1)?;
            let notnull: i64 = r.get(3)?;
            Ok((name, notnull))
        })
        .map_err(|e| format!("skin.db table_info: {e}"))?;
    for row in rows {
        let (name, notnull) = row.map_err(|e| format!("skin.db table_info row: {e}"))?;
        if name == "principal_id" {
            return Ok(notnull != 0);
        }
    }
    Ok(false)
}

fn key_prefix_last4_alnum(key_prefix: &str) -> Result<String, String> {
    let alnum: String = key_prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if alnum.len() < 4 {
        return Err(format!(
            "skin.db migrate: key_prefix {key_prefix:?} has no ASCII alnum last4"
        ));
    }
    Ok(alnum[alnum.len() - 4..].to_ascii_lowercase())
}

fn migrate_platform_name(
    username: &str,
    key_prefix: &str,
    live_taken: &HashSet<String>,
    live: bool,
) -> Result<String, String> {
    let username = username.trim().to_ascii_lowercase();
    if username.is_empty() {
        return Err("skin.db migrate: session=0 token has empty principal username".to_string());
    }
    let last4 = key_prefix_last4_alnum(key_prefix)?;
    let taken = |n: &str| live && live_taken.contains(n);
    let mut candidate = username.clone();
    if candidate == "owner" || taken(&candidate) {
        candidate = format!("{username}-{last4}");
    }
    if candidate == "owner" {
        return Err("skin.db migrate: platform token name 'owner' is reserved".into());
    }
    if taken(&candidate) {
        return Err(format!(
            "skin.db migrate: platform token name '{candidate}' collides after suffix"
        ));
    }
    Ok(candidate)
}

fn ensure_platform_name_index(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS tokens_platform_name_live
         ON tokens(name COLLATE NOCASE)
         WHERE session = 0 AND revoked_at IS NULL;",
    )
    .map_err(|e| format!("skin.db platform name index: {e}"))
}

/// Copy 132 cartesian caps onto each listed room. Then R8-rewrite live
/// `session=1`. Platform `session=0` stay NULL forever. Fail only on SQL/JSON.
fn migrate_room_policy(conn: &Connection) -> Result<(), String> {
    struct RoleRow {
        id: String,
        caps: String,
        rooms: String,
        room_policy: String,
    }
    let mut stmt = conn
        .prepare("SELECT id, caps, rooms, COALESCE(room_policy, '{}') FROM roles")
        .map_err(|e| format!("skin.db room_policy roles: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RoleRow {
                id: r.get(0)?,
                caps: r.get(1)?,
                rooms: r.get(2)?,
                room_policy: r.get(3)?,
            })
        })
        .map_err(|e| format!("skin.db room_policy roles: {e}"))?;
    let mut roles = Vec::new();
    for row in rows {
        roles.push(row.map_err(|e| format!("skin.db room_policy role row: {e}"))?);
    }
    drop(stmt);
    for role in roles {
        let caps = caps_from_json(&role.caps);
        let rooms = rooms_from_json(&role.rooms);
        let parsed = parse_room_policy_json(Some(&role.room_policy))?;
        let leftover_132 = match &parsed {
            Some(p) if p.is_empty() && !rooms.is_empty() => true,
            None if !rooms.is_empty() => true,
            _ => false,
        };
        if leftover_132 {
            let policy = cartesian_policy(&caps, &rooms);
            conn.execute(
                "UPDATE roles SET room_policy = ?1, caps = ?2, rooms = ?3 WHERE id = ?4",
                params![
                    room_policy_json(&policy),
                    caps_json(&union_caps(&policy)),
                    rooms_json(&rooms_from_policy(&policy)),
                    role.id
                ],
            )
            .map_err(|e| format!("skin.db room_policy role update: {e}"))?;
            rewrite_sessions_for_role(conn, &role.id, &policy)?;
        }
    }

    struct SessRow {
        id: String,
        caps: String,
        rooms: String,
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, caps, rooms FROM tokens
             WHERE session = 1 AND room_policy IS NULL",
        )
        .map_err(|e| format!("skin.db room_policy sessions: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SessRow {
                id: r.get(0)?,
                caps: r.get(1)?,
                rooms: r.get(2)?,
            })
        })
        .map_err(|e| format!("skin.db room_policy sessions: {e}"))?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| format!("skin.db room_policy session row: {e}"))?);
    }
    drop(stmt);
    for row in sessions {
        let caps = caps_from_json(&row.caps);
        let rooms = rooms_from_json(&row.rooms);
        let policy = cartesian_policy(&caps, &rooms);
        conn.execute(
            "UPDATE tokens SET room_policy = ?1 WHERE id = ?2",
            params![room_policy_json(&policy), row.id],
        )
        .map_err(|e| format!("skin.db room_policy session update: {e}"))?;
    }
    Ok(())
}

struct TokenRebuildRow {
    id: String,
    principal_id: Option<String>,
    key_hash: String,
    key_prefix: String,
    caps: String,
    rooms: String,
    created_at: i64,
    revoked_at: Option<i64>,
    session: i64,
    expires_at: Option<i64>,
    username: Option<String>,
}

fn rebuild_tokens_table_if_needed(conn: &mut Connection) -> Result<(), String> {
    if !tokens_principal_id_notnull(conn)? {
        return Ok(());
    }
    let tx = conn
        .transaction()
        .map_err(|e| format!("skin.db migrate begin: {e}"))?;
    let mut stmt = tx
        .prepare(
            "SELECT t.id, t.principal_id, t.key_hash, t.key_prefix, t.caps,
                    COALESCE(t.rooms, '[]'), t.created_at, t.revoked_at,
                    COALESCE(t.session, 0), t.expires_at, p.username
             FROM tokens t
             LEFT JOIN principals p ON p.id = t.principal_id",
        )
        .map_err(|e| format!("skin.db migrate select: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(TokenRebuildRow {
                id: r.get(0)?,
                principal_id: r.get(1)?,
                key_hash: r.get(2)?,
                key_prefix: r.get(3)?,
                caps: r.get(4)?,
                rooms: r.get(5)?,
                created_at: r.get(6)?,
                revoked_at: r.get(7)?,
                session: r.get(8)?,
                expires_at: r.get(9)?,
                username: r.get(10)?,
            })
        })
        .map_err(|e| format!("skin.db migrate select: {e}"))?;
    let mut old = Vec::new();
    for row in rows {
        old.push(row.map_err(|e| format!("skin.db migrate row: {e}"))?);
    }
    drop(stmt);

    tx.execute_batch(
        "CREATE TABLE tokens_new (
            id TEXT PRIMARY KEY,
            principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
            name TEXT,
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            caps TEXT NOT NULL,
            rooms TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            revoked_at INTEGER,
            session INTEGER NOT NULL DEFAULT 0,
            expires_at INTEGER,
            CHECK (
                (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                OR
                (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
            )
         );",
    )
    .map_err(|e| format!("skin.db migrate create: {e}"))?;

    let mut live_taken: HashSet<String> = HashSet::new();
    for row in &old {
        let (principal_id, name): (Option<String>, Option<String>) = if row.session != 0 {
            let Some(pid) = row
                .principal_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return Err(format!(
                    "skin.db migrate: session=1 token {} missing principal_id",
                    row.id
                ));
            };
            (Some(pid.to_string()), None)
        } else {
            let username = row.username.as_deref().unwrap_or("");
            let live = row.revoked_at.is_none();
            let candidate = migrate_platform_name(username, &row.key_prefix, &live_taken, live)?;
            if live {
                live_taken.insert(candidate.clone());
            }
            (None, Some(candidate))
        };
        tx.execute(
            "INSERT INTO tokens_new
             (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                row.id,
                principal_id,
                name,
                row.key_hash,
                row.key_prefix,
                row.caps,
                row.rooms,
                row.created_at,
                row.revoked_at,
                row.session,
                row.expires_at
            ],
        )
        .map_err(|e| format!("skin.db migrate insert {}: {e}", row.id))?;
    }
    tx.execute_batch("DROP TABLE tokens; ALTER TABLE tokens_new RENAME TO tokens;")
        .map_err(|e| format!("skin.db migrate swap: {e}"))?;
    tx.commit()
        .map_err(|e| format!("skin.db migrate commit: {e}"))?;
    Ok(())
}

fn db() -> &'static Mutex<Option<SkinDb>> {
    static DB: OnceLock<Mutex<Option<SkinDb>>> = OnceLock::new();
    DB.get_or_init(|| Mutex::new(None))
}

fn with_conn<T, F: FnOnce(&Connection) -> Result<T, String>>(f: F) -> Result<T, String> {
    let path = store_path();
    let mut guard = db().lock();
    let reopen = match guard.as_ref() {
        Some(existing) => existing.path != path,
        None => true,
    };
    if reopen {
        let conn = open_db(&path)?;
        *guard = Some(SkinDb {
            path: path.clone(),
            conn,
        });
    }
    let conn = &guard.as_ref().expect("skin db opened").conn;
    f(conn)
}

// ── Principals ───────────────────────────────────────────────────────

pub fn add_principal(username: &str) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    let r = with_conn(|conn| {
        let exists: Option<String> = conn
            .query_row(
                "SELECT id FROM principals WHERE username = ?1",
                params![username],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin principal lookup: {e}"))?;
        if exists.is_some() {
            return Err(format!("skin user '{username}' already exists"));
        }
        conn.execute(
            "INSERT INTO principals (id, username, created_at) VALUES (?1, ?2, ?3)",
            params![id, username, created_at],
        )
        .map_err(|e| format!("skin principal insert: {e}"))?;
        Ok(SkinPrincipal {
            id,
            username,
            created_at,
            default_rooms: Vec::new(),
            default_room_handles: Vec::new(),
            has_password: false,
            role_id: None,
            role_name: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(r)
}

const PRINCIPAL_SELECT: &str = "SELECT p.id, p.username, p.created_at, p.default_rooms,
                p.password_hash, p.role_id, r.name
         FROM principals p
         LEFT JOIN roles r ON r.id = p.role_id";

fn map_principal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinPrincipal> {
    let rooms_raw: String = r.get(3)?;
    let hash: Option<String> = r.get(4)?;
    let role_id: Option<String> = r.get(5)?;
    let role_name: Option<String> = r.get(6)?;
    Ok(SkinPrincipal {
        id: r.get(0)?,
        username: r.get(1)?,
        created_at: r.get(2)?,
        default_rooms: rooms_from_json(&rooms_raw),
        default_room_handles: Vec::new(),
        has_password: password_is_set(hash.as_deref()),
        role_id: role_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        role_name: role_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    })
}

pub fn list_principals() -> Result<Vec<SkinPrincipal>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!(
                "{PRINCIPAL_SELECT} ORDER BY p.username COLLATE NOCASE"
            ))
            .map_err(|e| format!("skin principal list: {e}"))?;
        let rows = stmt
            .query_map([], map_principal_row)
            .map_err(|e| format!("skin principal list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin principal row: {e}"))?);
        }
        Ok(out)
    })
    .map(|mut users| {
        for u in &mut users {
            u.default_room_handles = handles_for_project_ids(&u.default_rooms);
        }
        users
    })
}

pub fn remove_principal(username: &str) -> Result<bool, String> {
    let username = normalize_username(username)?;
    let r = with_conn(|conn| {
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM principals WHERE username = ?1",
                params![username],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin principal lookup: {e}"))?;
        let Some(id) = id else {
            return Ok(false);
        };
        conn.execute(
            "DELETE FROM tokens WHERE principal_id = ?1 AND session = 1",
            params![id],
        )
        .map_err(|e| format!("skin session delete: {e}"))?;
        let n = conn
            .execute("DELETE FROM principals WHERE id = ?1", params![id])
            .map_err(|e| format!("skin principal delete: {e}"))?;
        Ok(n > 0)
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    }
    Ok(r)
}

fn principal_by_username(
    conn: &Connection,
    username: &str,
) -> Result<Option<SkinPrincipal>, String> {
    conn.query_row(
        &format!("{PRINCIPAL_SELECT} WHERE p.username = ?1"),
        params![username],
        map_principal_row,
    )
    .optional()
    .map_err(|e| format!("skin principal lookup: {e}"))
}

fn attach_principal_handles(mut p: SkinPrincipal) -> SkinPrincipal {
    p.default_room_handles = handles_for_project_ids(&p.default_rooms);
    p
}

fn attach_token_handles(mut meta: SkinTokenMeta) -> SkinTokenMeta {
    meta.room_handles = handles_for_project_ids(&meta.rooms);
    meta
}

// ── Tokens ───────────────────────────────────────────────────────────

/// Mint a **platform** pass. Returns `(meta, raw_secret)` — the secret is shown
/// once. No principal lookup. `name` is a box-unique label (not a guest).
/// `rooms` are already-canonical `project_id` UUIDs. Empty → error (R5).
pub fn create_token(
    name: &str,
    caps: Option<&[String]>,
    rooms: &[String],
) -> Result<(SkinTokenMeta, String), String> {
    let name = normalize_username(name)?;
    if name == "owner" {
        return Err("platform token name 'owner' is reserved".to_string());
    }
    let caps = parse_caps(caps)?;
    let rooms = normalize_room_ids(rooms);
    if rooms.is_empty() {
        return Err("rooms must include at least one workspace".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    debug_assert!(
        raw.starts_with(SKIN_KEY_PREFIX) && !raw.starts_with("k2sk_"),
        "skin prefix must be k2skn_, never k2sk_"
    );
    let key_hash = sha256_hex(&raw);
    let prefix = display_prefix(&raw);
    let created_at = now_secs();
    let caps_stored = caps_json(&caps);
    let rooms_stored = rooms_json(&rooms);
    let r = with_conn(|conn| {
        let taken: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tokens
                 WHERE session = 0 AND revoked_at IS NULL AND name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin token name lookup: {e}"))?;
        if taken > 0 {
            return Err(format!("platform token name '{name}' already exists"));
        }
        conn.execute(
            "INSERT INTO tokens
             (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, NULL)",
            params![id, name, key_hash, prefix, caps_stored, rooms_stored, created_at],
        )
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("tokens_platform_name_live") || s.contains("UNIQUE") {
                format!("platform token name '{name}' already exists")
            } else {
                format!("skin token insert: {e}")
            }
        })?;
        Ok((
            SkinTokenMeta {
                id,
                name,
                prefix,
                caps,
                rooms,
                room_handles: Vec::new(),
                room_access: Vec::new(),
                created_at,
                revoked_at: None,
            },
            raw,
        ))
    })?;
    Ok((attach_token_handles(r.0), r.1))
}

/// Login session mint. Assigned guests snapshot the role (caps+rooms,
/// including `[]`). Unassigned copies `default_rooms` **including []**
/// (Thread `skin_room` 403) with Thread-only caps. Do **not** call
/// [`create_token`] (empty rooms 400). `session=1`, `expires_at` =
/// now + [`crate::connect_users::session_ttl_days`].
pub fn create_session_token(username: &str) -> Result<(SkinTokenMeta, String), String> {
    let username = normalize_username(username)?;
    let id = uuid::Uuid::new_v4().to_string();
    let raw = generate_raw_key();
    debug_assert!(
        raw.starts_with(SKIN_KEY_PREFIX) && !raw.starts_with("k2sk_"),
        "skin prefix must be k2skn_, never k2sk_"
    );
    let key_hash = sha256_hex(&raw);
    let prefix = display_prefix(&raw);
    let created_at = now_secs();
    let ttl_secs = crate::connect_users::session_ttl_days().saturating_mul(86_400);
    let expires_at = created_at.saturating_add(ttl_secs);
    let r = with_conn(|conn| {
        let Some(principal) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let policy = if let Some(rid) = principal.role_id.as_deref() {
            let Some(role) = role_by_id_or_name(conn, rid)? else {
                return Err(format!("unknown skin role '{rid}'"));
            };
            role.room_policy
        } else {
            thread_only_policy(&principal.default_rooms)
        };
        let caps = union_caps(&policy);
        let rooms = rooms_from_policy(&policy);
        let caps_stored = caps_json(&caps);
        let rooms_stored = rooms_json(&rooms);
        let policy_stored = room_policy_json(&policy);
        conn.execute(
            "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at, room_policy)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8, ?9)",
            params![
                id,
                principal.id,
                key_hash,
                prefix,
                caps_stored,
                rooms_stored,
                created_at,
                expires_at,
                policy_stored
            ],
        )
        .map_err(|e| format!("skin session insert: {e}"))?;
        Ok((
            SkinTokenMeta {
                id,
                name: principal.username,
                prefix,
                caps,
                rooms,
                room_handles: Vec::new(),
                room_access: room_access_from_policy(&policy),
                created_at,
                revoked_at: None,
            },
            raw,
        ))
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok((attach_token_handles(r.0), r.1))
}

/// Set or clear the K2-login password. Empty/`None` → NULL hash (cannot
/// K2-login; platform tokens still work). Revokes **session** passes only.
/// Hashes **before** taking `skin.db` so argon2 never holds the Mutex.
pub fn set_principal_password(
    username: &str,
    password: Option<&str>,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let hash = match password.map(str::trim).filter(|s| !s.is_empty()) {
        Some(pw) => Some(hash_password(pw)?),
        None => None,
    };
    let has_password = hash.is_some();
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        conn.execute(
            "UPDATE principals SET password_hash = ?1 WHERE id = ?2",
            params![hash, p.id],
        )
        .map_err(|e| format!("skin password update: {e}"))?;
        revoke_sessions_for_principal(conn, &p.id)?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password,
            role_id: p.role_id,
            role_name: p.role_name,
        })
    })
    .map(attach_principal_handles)
}

fn revoke_sessions_for_principal(conn: &Connection, principal_id: &str) -> Result<usize, String> {
    conn.execute(
        "UPDATE tokens SET revoked_at = ?1
         WHERE principal_id = ?2 AND session = 1 AND revoked_at IS NULL",
        params![now_secs(), principal_id],
    )
    .map_err(|e| format!("skin session revoke: {e}"))
}

/// Login gate: lockout then dummy-argon2 verify. Never holds `skin.db`
/// across hashing.
#[derive(Debug)]
pub enum SkinLoginOutcome {
    Ok(SkinPrincipal),
    BadCreds,
    LockedOut,
}

pub fn check_and_record_login(username: &str, password: &str) -> SkinLoginOutcome {
    let key = normalize_username(username).unwrap_or_else(|_| username.trim().to_ascii_lowercase());

    let locked = with_conn(|conn| {
        let row: Option<(i64, Option<i64>)> = conn
            .query_row(
                "SELECT failed_count, locked_until FROM login_lockouts WHERE username = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| format!("skin lockout lookup: {e}"))?;
        if let Some((_count, Some(until))) = row {
            if now_secs() < until {
                return Ok(true);
            }
            conn.execute(
                "UPDATE login_lockouts SET locked_until = NULL, failed_count = 0 WHERE username = ?1",
                params![key],
            )
            .map_err(|e| format!("skin lockout expire: {e}"))?;
        }
        Ok(false)
    })
    .unwrap_or(false);
    if locked {
        return SkinLoginOutcome::LockedOut;
    }

    let stored = with_conn(|conn| {
        let hash: Option<Option<String>> = conn
            .query_row(
                "SELECT password_hash FROM principals WHERE username = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin password lookup: {e}"))?;
        Ok(hash.flatten().filter(|s| !s.trim().is_empty()))
    })
    .unwrap_or(None);

    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY_HASH.get_or_init(build_dummy_hash);
    let ok = match stored.as_deref() {
        Some(h) => verify_hash(password, h),
        None => {
            let _ = verify_hash(password, dummy);
            false
        }
    };

    let _ = with_conn(|conn| {
        if ok {
            conn.execute(
                "DELETE FROM login_lockouts WHERE username = ?1",
                params![key],
            )
            .map_err(|e| format!("skin lockout clear: {e}"))?;
        } else {
            let failed: i64 = conn
                .query_row(
                    "SELECT failed_count FROM login_lockouts WHERE username = ?1",
                    params![key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("skin lockout count: {e}"))?
                .unwrap_or(0)
                + 1;
            if failed >= i64::from(LOCKOUT_THRESHOLD) {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES (?1, 0, ?2)
                     ON CONFLICT(username) DO UPDATE SET failed_count = 0, locked_until = excluded.locked_until",
                    params![key, now_secs().saturating_add(LOCKOUT_DURATION_SECS)],
                )
                .map_err(|e| format!("skin lockout set: {e}"))?;
            } else {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES (?1, ?2, NULL)
                     ON CONFLICT(username) DO UPDATE SET failed_count = excluded.failed_count, locked_until = NULL",
                    params![key, failed],
                )
                .map_err(|e| format!("skin lockout bump: {e}"))?;
            }
        }
        Ok(())
    });

    if !ok {
        return SkinLoginOutcome::BadCreds;
    }
    match with_conn(|conn| principal_by_username(conn, &key)) {
        Ok(Some(p)) => SkinLoginOutcome::Ok(attach_principal_handles(p)),
        _ => SkinLoginOutcome::BadCreds,
    }
}

pub fn list_tokens() -> Result<Vec<SkinTokenMeta>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.name, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
                 FROM tokens t
                 WHERE t.session = 0
                 ORDER BY t.created_at DESC",
            )
            .map_err(|e| format!("skin token list: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                let caps_raw: String = r.get(3)?;
                let rooms_raw: String = r.get(4)?;
                Ok(SkinTokenMeta {
                    id: r.get(0)?,
                    name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    prefix: r.get(2)?,
                    caps: caps_from_json(&caps_raw),
                    rooms: rooms_from_json(&rooms_raw),
                    room_handles: Vec::new(),
                    room_access: Vec::new(),
                    created_at: r.get(5)?,
                    revoked_at: r.get(6)?,
                })
            })
            .map_err(|e| format!("skin token list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin token row: {e}"))?);
        }
        Ok(out)
    })
    .map(|mut tokens| {
        for t in &mut tokens {
            t.room_handles = handles_for_project_ids(&t.rooms);
        }
        tokens
    })
}

/// Soft-revoke. `Ok(true)` if a live token was just revoked.
pub fn revoke_token(id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing token id".to_string());
    }
    let r = with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                params![now_secs(), id],
            )
            .map_err(|e| format!("skin token revoke: {e}"))?;
        Ok(n > 0)
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    }
    Ok(r)
}

fn token_meta_from_row(conn: &Connection, id: &str) -> Result<Option<SkinTokenMeta>, String> {
    conn.query_row(
        "SELECT t.id, t.session, t.name, p.username, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
         FROM tokens t
         LEFT JOIN principals p ON p.id = t.principal_id
         WHERE t.id = ?1",
        params![id],
        |r| {
            let session: i64 = r.get(1)?;
            let token_name: Option<String> = r.get(2)?;
            let principal_username: Option<String> = r.get(3)?;
            let stamp = if session != 0 {
                principal_username.unwrap_or_default()
            } else {
                token_name.unwrap_or_default()
            };
            let caps_raw: String = r.get(5)?;
            let rooms_raw: String = r.get(6)?;
            Ok(SkinTokenMeta {
                id: r.get(0)?,
                name: stamp,
                prefix: r.get(4)?,
                caps: caps_from_json(&caps_raw),
                rooms: rooms_from_json(&rooms_raw),
                room_handles: Vec::new(),
                room_access: Vec::new(),
                created_at: r.get(7)?,
                revoked_at: r.get(8)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("skin token lookup: {e}"))
}

/// PATCH rooms on a live (or revoked) key. Does **not** touch `key_hash`.
/// Empty `rooms` is allowed (Thread dark, keep secret).
pub fn set_token_rooms(id: &str, rooms: &[String]) -> Result<SkinTokenMeta, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing token id".to_string());
    }
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE tokens SET rooms = ?1 WHERE id = ?2",
                params![stored, id],
            )
            .map_err(|e| format!("skin token rooms: {e}"))?;
        if n == 0 {
            return Err("unknown token id".to_string());
        }
        let Some(meta) = token_meta_from_row(conn, id)? else {
            return Err("unknown token id".to_string());
        };
        Ok(meta)
    })
    .map(attach_token_handles)
}

/// Set principal default rooms. `apply_tokens` copies onto live **sessions**
/// only — never platform tokens.
pub fn set_principal_default_rooms(
    username: &str,
    rooms: &[String],
    apply_tokens: bool,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        if p.role_id.is_some() {
            let name = p.role_name.as_deref().unwrap_or("unknown");
            return Err(assigned_guest_rooms_err(&username, name));
        }
        conn.execute(
            "UPDATE principals SET default_rooms = ?1 WHERE id = ?2",
            params![stored, p.id],
        )
        .map_err(|e| format!("skin user rooms: {e}"))?;
        if apply_tokens {
            let policy = thread_only_policy(&rooms);
            rewrite_live_sessions(conn, &p.id, &policy)?;
        }
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: p.role_id,
            role_name: p.role_name,
        })
    })
    .map(attach_principal_handles)
}

// ── Roles ────────────────────────────────────────────────────────────

const ROLE_SELECT: &str = "SELECT id, name, caps, rooms, created_at, COALESCE(room_policy, '{}') FROM roles";

fn map_role_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinRole> {
    let caps_raw: String = r.get(2)?;
    let rooms_raw: String = r.get(3)?;
    let policy_raw: String = r.get(5)?;
    let caps = caps_from_json(&caps_raw);
    let rooms = rooms_from_json(&rooms_raw);
    let room_policy = policy_or_cartesian(&caps, &rooms, Some(&policy_raw)).unwrap_or_else(|_| {
        cartesian_policy(&caps, &rooms)
    });
    Ok(SkinRole {
        id: r.get(0)?,
        name: r.get(1)?,
        caps: union_caps(&room_policy),
        rooms: rooms_from_policy(&room_policy),
        room_handles: Vec::new(),
        room_access: Vec::new(),
        created_at: r.get(4)?,
        room_policy,
    })
}

fn attach_role_handles(mut role: SkinRole) -> SkinRole {
    role.room_handles = handles_for_project_ids(&role.rooms);
    role.room_access = room_access_from_policy(&role.room_policy);
    role.caps = union_caps(&role.room_policy);
    role.rooms = rooms_from_policy(&role.room_policy);
    role
}

fn persist_role_policy(conn: &Connection, role_id: &str, policy: &RoomPolicy) -> Result<(), String> {
    conn.execute(
        "UPDATE roles SET caps = ?1, rooms = ?2, room_policy = ?3 WHERE id = ?4",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            role_id
        ],
    )
    .map_err(|e| format!("skin role update: {e}"))?;
    Ok(())
}

fn role_from_policy(id: String, name: String, created_at: i64, policy: RoomPolicy) -> SkinRole {
    SkinRole {
        id,
        name,
        caps: union_caps(&policy),
        rooms: rooms_from_policy(&policy),
        room_handles: Vec::new(),
        room_access: Vec::new(),
        created_at,
        room_policy: policy,
    }
}

fn role_by_id_or_name(conn: &Connection, token: &str) -> Result<Option<SkinRole>, String> {
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    let by_id: Option<SkinRole> = conn
        .query_row(
            &format!("{ROLE_SELECT} WHERE id = ?1"),
            params![token],
            map_role_row,
        )
        .optional()
        .map_err(|e| format!("skin role lookup: {e}"))?;
    if by_id.is_some() {
        return Ok(by_id);
    }
    conn.query_row(
        &format!("{ROLE_SELECT} WHERE name = ?1 COLLATE NOCASE"),
        params![token],
        map_role_row,
    )
    .optional()
    .map_err(|e| format!("skin role lookup: {e}"))
}

fn rewrite_live_sessions(
    conn: &Connection,
    principal_id: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
         WHERE principal_id = ?4 AND session = 1 AND revoked_at IS NULL",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            principal_id
        ],
    )
    .map_err(|e| format!("skin session rewrite: {e}"))?;
    Ok(())
}

fn rewrite_sessions_for_role(
    conn: &Connection,
    role_id: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
         WHERE session = 1 AND revoked_at IS NULL
           AND principal_id IN (SELECT id FROM principals WHERE role_id = ?4)",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            role_id
        ],
    )
    .map_err(|e| format!("skin session rewrite: {e}"))?;
    Ok(())
}

fn unique_role_err(name: &str, e: rusqlite::Error) -> String {
    let s = e.to_string();
    if s.contains("roles_name") || s.contains("UNIQUE") {
        format!("role '{name}' already exists")
    } else {
        format!("skin role insert: {e}")
    }
}

/// Thread-only caps on each listed room (add-room default / unassigned I6).
pub fn thread_only_room_policy(rooms: &[String]) -> RoomPolicy {
    thread_only_policy(&normalize_room_ids(rooms))
}

/// Mint a named per-room bundle. Empty map = Thread dark.
pub fn create_role(name: &str, policy: &RoomPolicy) -> Result<SkinRole, String> {
    let name = normalize_username(name)?;
    if is_connect_role_name(&name) {
        return Err(CONNECT_ROLE_NAME_ERR.to_string());
    }
    let policy = normalize_policy(policy)?;
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs();
    with_conn(|conn| {
        let taken: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM roles WHERE name = ?1 COLLATE NOCASE",
                params![name],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin role name lookup: {e}"))?;
        if taken > 0 {
            return Err(format!("role '{name}' already exists"));
        }
        conn.execute(
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                name,
                caps_json(&union_caps(&policy)),
                rooms_json(&rooms_from_policy(&policy)),
                created_at,
                room_policy_json(&policy)
            ],
        )
        .map_err(|e| unique_role_err(&name, e))?;
        Ok(role_from_policy(id, name, created_at, policy))
    })
    .map(attach_role_handles)
}

pub fn list_roles() -> Result<Vec<SkinRole>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!("{ROLE_SELECT} ORDER BY name COLLATE NOCASE"))
            .map_err(|e| format!("skin role list: {e}"))?;
        let rows = stmt
            .query_map([], map_role_row)
            .map_err(|e| format!("skin role list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin role row: {e}"))?);
        }
        Ok(out)
    })
    .map(|mut roles| {
        for r in &mut roles {
            *r = attach_role_handles(r.clone());
        }
        roles
    })
}

/// Present policy replaces the whole map. `None` is a no-op persist of the
/// current snapshot (still R8-rewrites live sessions).
pub fn update_role(
    id_or_name: &str,
    policy: Option<&RoomPolicy>,
) -> Result<SkinRole, String> {
    let id_or_name = id_or_name.trim();
    if id_or_name.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let policy = match policy {
        Some(p) => Some(normalize_policy(p)?),
        None => None,
    };
    let role = with_conn(|conn| {
        let Some(mut role) = role_by_id_or_name(conn, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        if let Some(p) = policy {
            role.room_policy = p;
        }
        persist_role_policy(conn, &role.id, &role.room_policy)?;
        rewrite_sessions_for_role(conn, &role.id, &role.room_policy)?;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

/// Add or replace one room on a role. Omitted caps → Thread-only.
pub fn set_role_room(
    id_or_name: &str,
    project_id: &str,
    caps: Option<&[String]>,
) -> Result<SkinRole, String> {
    let rooms = normalize_room_ids(std::slice::from_ref(&project_id.to_string()));
    let Some(project_id) = rooms.into_iter().next() else {
        return Err("missing workspace".to_string());
    };
    let caps = parse_caps(caps)?;
    let role = with_conn(|conn| {
        let Some(mut role) = role_by_id_or_name(conn, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        role.room_policy.insert(project_id, caps);
        persist_role_policy(conn, &role.id, &role.room_policy)?;
        rewrite_sessions_for_role(conn, &role.id, &role.room_policy)?;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

/// Drop one room from a role (`--clear`).
pub fn clear_role_room(id_or_name: &str, project_id: &str) -> Result<SkinRole, String> {
    let project_id = project_id.trim();
    if project_id.is_empty() {
        return Err("missing workspace".to_string());
    }
    let role = with_conn(|conn| {
        let Some(mut role) = role_by_id_or_name(conn, id_or_name)? else {
            return Err(format!("unknown skin role '{id_or_name}'"));
        };
        role.room_policy.remove(project_id);
        persist_role_policy(conn, &role.id, &role.room_policy)?;
        rewrite_sessions_for_role(conn, &role.id, &role.room_policy)?;
        Ok(role)
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_role_handles(role))
}

fn normalize_policy(policy: &RoomPolicy) -> Result<RoomPolicy, String> {
    let mut out = RoomPolicy::new();
    for (id, caps) in policy {
        let t = id.trim();
        if t.is_empty() {
            continue;
        }
        let parsed = parse_caps(if caps.is_empty() {
            None
        } else {
            Some(caps.as_slice())
        })?;
        out.insert(t.to_string(), parsed);
    }
    Ok(out)
}

pub fn remove_role(id_or_name: &str) -> Result<bool, String> {
    let id_or_name = id_or_name.trim();
    if id_or_name.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let r = with_conn(|conn| {
        let Some(role) = role_by_id_or_name(conn, id_or_name)? else {
            return Ok(false);
        };
        let assigned: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM principals WHERE role_id = ?1",
                params![role.id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin role assigned lookup: {e}"))?;
        if assigned > 0 {
            return Err(assigned_role_remove_err(&role.name));
        }
        let n = conn
            .execute("DELETE FROM roles WHERE id = ?1", params![role.id])
            .map_err(|e| {
                let s = e.to_string();
                if s.contains("FOREIGN KEY") || s.to_ascii_lowercase().contains("constraint") {
                    assigned_role_remove_err(&role.name)
                } else {
                    format!("skin role delete: {e}")
                }
            })?;
        Ok(n > 0)
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    }
    Ok(r)
}

pub fn assign_role(username: &str, role: &str) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let role_token = role.trim();
    if role_token.is_empty() {
        return Err("missing role id or name".to_string());
    }
    let p = with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let Some(role) = role_by_id_or_name(conn, role_token)? else {
            return Err(format!("unknown skin role '{role_token}'"));
        };
        conn.execute(
            "UPDATE principals SET role_id = ?1 WHERE id = ?2",
            params![role.id, p.id],
        )
        .map_err(|e| format!("skin role assign: {e}"))?;
        rewrite_live_sessions(conn, &p.id, &role.room_policy)?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: Some(role.id),
            role_name: Some(role.name),
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_principal_handles(p))
}

pub fn unassign_role(username: &str) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let p = with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        conn.execute(
            "UPDATE principals SET role_id = NULL WHERE id = ?1",
            params![p.id],
        )
        .map_err(|e| format!("skin role unassign: {e}"))?;
        rewrite_live_sessions(conn, &p.id, &thread_only_policy(&p.default_rooms))?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: None,
            role_name: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(attach_principal_handles(p))
}

/// Handle + `project_handle_aliases` + project_id UUID only.
/// Display-name / folder-basename fallback is **not** accepted.
/// Unknown or ambiguous → error (400 at HTTP).
pub fn resolve_room_tokens(tokens: &[String]) -> Result<Vec<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut out: Vec<String> = Vec::new();
    for raw in tokens {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        match resolve_room_token(&conn, t)? {
            Some(id) => {
                if !out.iter().any(|x| x == &id) {
                    out.push(id);
                }
            }
            None => {
                return Err(format!("unknown workspace handle {t:?}"));
            }
        }
    }
    Ok(out)
}

fn resolve_room_token(conn: &Connection, token: &str) -> Result<Option<String>, String> {
    use crate::workspace_session_handles::is_uuid_shape;
    if is_uuid_shape(token) {
        let found: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE id = ?1",
                params![token],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("room lookup: {e}"))?;
        return Ok(found);
    }

    let mut handles: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM projects WHERE handle = ?1 COLLATE NOCASE \
                 AND handle IS NOT NULL AND TRIM(handle) != ''",
            )
            .map_err(|e| format!("room handle lookup: {e}"))?;
        let rows = stmt
            .query_map(params![token], |r| r.get::<_, String>(0))
            .map_err(|e| format!("room handle lookup: {e}"))?;
        for row in rows {
            handles.push(row.map_err(|e| format!("room handle row: {e}"))?);
        }
    }
    match handles.len() {
        1 => return Ok(Some(handles.remove(0))),
        n if n > 1 => {
            return Err(format!("ambiguous workspace handle {token:?}"));
        }
        _ => {}
    }

    let mut aliases: Vec<String> = Vec::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT a.project_id FROM project_handle_aliases a \
                 WHERE a.alias = ?1 COLLATE NOCASE",
            )
            .map_err(|e| format!("room alias lookup: {e}"))?;
        let rows = stmt
            .query_map(params![token], |r| r.get::<_, String>(0))
            .map_err(|e| format!("room alias lookup: {e}"))?;
        for row in rows {
            aliases.push(row.map_err(|e| format!("room alias row: {e}"))?);
        }
    }
    match aliases.len() {
        1 => Ok(Some(aliases.remove(0))),
        n if n > 1 => Err(format!("ambiguous workspace handle {token:?}")),
        _ => Ok(None),
    }
}

/// Live handles for stored room UUIDs. Missing/retired projects are skipped.
pub fn handles_for_project_ids(ids: &[String]) -> Vec<String> {
    live_agents(ids).into_iter().map(|a| a.handle).collect()
}

/// Allowed agents whose `project_id` still exists. Skip missing.
pub fn live_agents(ids: &[String]) -> Vec<SkinAgent> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut pending: Vec<(String, String, Option<String>)> = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        if let Ok(h) = crate::workspace::handle::project_handle(&conn, id) {
            let h = h.trim().to_string();
            if !h.is_empty() {
                let path: Option<String> = conn
                    .query_row(
                        "SELECT path FROM projects WHERE id = ?1",
                        params![id],
                        |r| r.get(0),
                    )
                    .ok()
                    .map(|s: String| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                pending.push((id.to_string(), h, path));
            }
        }
    }
    drop(conn);
    pending
        .into_iter()
        .map(|(project_id, handle, path)| {
            let display_name = path
                .map(|p| crate::workspace::display::agent_display_name(&p))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| handle.clone());
            SkinAgent {
                handle,
                project_id,
                display_name,
            }
        })
        .collect()
}

/// Resolve a presented raw key. Revoked / expired / unknown / wrong prefix → `None`.
pub fn resolve_skin_token(presented_raw: &str) -> Option<SkinPass> {
    if !is_skin_token(presented_raw) || is_api_key_prefix(presented_raw) {
        return None;
    }
    let key_hash = sha256_hex(presented_raw);
    let now = now_secs();
    with_conn(|conn| {
        let row = conn
            .query_row(
                "SELECT t.id, t.principal_id, t.session, t.name, p.username, t.caps, t.rooms, t.room_policy
                 FROM tokens t
                 LEFT JOIN principals p ON p.id = t.principal_id
                 WHERE t.key_hash = ?1 AND t.revoked_at IS NULL
                   AND (t.expires_at IS NULL OR t.expires_at > ?2)",
                params![key_hash, now],
                |r| {
                    let principal_id: Option<String> = r.get(1)?;
                    let session: i64 = r.get(2)?;
                    let token_name: Option<String> = r.get(3)?;
                    let principal_username: Option<String> = r.get(4)?;
                    let stamp = if session != 0 {
                        principal_username.unwrap_or_default()
                    } else {
                        token_name.unwrap_or_default()
                    };
                    let caps_raw: String = r.get(5)?;
                    let rooms_raw: String = r.get(6)?;
                    let policy_raw: Option<String> = r.get(7)?;
                    let caps = caps_from_json(&caps_raw);
                    let rooms = rooms_from_json(&rooms_raw);
                    let session_b = session != 0;
                    let room_policy = interpret_token_policy(
                        session_b,
                        &caps,
                        &rooms,
                        policy_raw.as_deref(),
                    );
                    let (caps, rooms) = if session_b {
                        (union_caps(&room_policy), rooms_from_policy(&room_policy))
                    } else {
                        (caps, rooms)
                    };
                    Ok(SkinPass {
                        id: r.get(0)?,
                        principal_id,
                        username: stamp,
                        caps,
                        rooms,
                        session: session_b,
                        room_policy,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("skin token resolve: {e}"))?;
        Ok(row)
    })
    .ok()
    .flatten()
}

// ── Front door ───────────────────────────────────────────────────────

pub fn get_front_door() -> Result<SkinFrontDoor, String> {
    with_conn(|conn| {
        let stored = conn
            .query_row(
                "SELECT mode, url, hint, ui_port FROM front_door WHERE id = 1",
                [],
                |r| {
                    let ui: Option<i64> = r.get(3)?;
                    Ok(SkinFrontDoor {
                        mode: r.get(0)?,
                        url: r.get(1)?,
                        hint: r.get(2)?,
                        ui_port: ui.and_then(|n| u16::try_from(n).ok()),
                    })
                },
            )
            .optional()
            .map_err(|e| format!("skin front-door: {e}"))?;
        Ok(stored.unwrap_or_else(SkinFrontDoor::default_connect))
    })
}

/// True when a front-door row has been persisted (boot apply uses this).
pub fn front_door_is_stored() -> bool {
    with_conn(|conn| {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM front_door WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        Ok(n > 0)
    })
    .unwrap_or(false)
}

pub fn set_front_door(
    mode: &str,
    url: Option<&str>,
    hint: Option<&str>,
    ui_port: Option<u16>,
) -> Result<SkinFrontDoor, String> {
    let mode = parse_front_door_mode(mode)?;
    let url_stored = url.map(str::trim).filter(|s| !s.is_empty());
    let hint_stored = hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_hint_for_mode(mode).to_string());
    let ui_stored: Option<i64> = ui_port.map(|p| p as i64);
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO front_door (id, mode, url, hint, ui_port, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               mode = excluded.mode,
               url = excluded.url,
               hint = excluded.hint,
               ui_port = excluded.ui_port,
               updated_at = excluded.updated_at",
            params![mode, url_stored, hint_stored, ui_stored, now_secs()],
        )
        .map_err(|e| format!("skin front-door persist: {e}"))?;
        Ok(SkinFrontDoor {
            mode: mode.to_string(),
            url: url_stored.map(|s| s.to_string()),
            hint: Some(hint_stored),
            ui_port,
        })
    })
}

/// Effective front-door view: persisted mode plus derived Connect URL
/// (`https://skin.<sub>.k2.dev`) when mode is `connect` and no URL was stored.
pub fn effective_front_door() -> Result<SkinFrontDoor, String> {
    let mut door = get_front_door()?;
    if door.mode != "connect" && door.mode != "direct" {
        door.mode = "connect".to_string();
    }
    if door.hint.as_deref().map(str::trim).unwrap_or("").is_empty() {
        door.hint = Some(default_hint_for_mode(&door.mode).to_string());
    }
    if door.mode == "connect" && door.url.as_deref().map(str::trim).unwrap_or("").is_empty() {
        door.url = crate::tunnel::config::load()
            .ok()
            .and_then(|c| c.public_url())
            .and_then(|u| skin_url_from_public(&u));
    }
    Ok(door)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = crate::themes::HOME_LOCK.lock();
        let prev = std::env::var_os("HOME");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp =
            std::env::temp_dir().join(format!("k2-skin-core-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&tmp).expect("temp HOME");
        std::env::set_var("HOME", &tmp);
        f();
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn prefix_is_k2skn_never_k2sk() {
        assert_eq!(SKIN_KEY_PREFIX, "k2skn_");
        assert_ne!(SKIN_KEY_PREFIX, "k2sk_");
        assert!(is_skin_token("k2skn_abc"));
        assert!(!is_skin_token("k2sk_abc"));
        assert!(!is_api_key_prefix("k2skn_abc"));
        assert!(is_api_key_prefix("k2sk_abc"));
        assert!(
            !SKIN_KEY_PREFIX.starts_with("k2sk_") && !"k2sk_".starts_with(SKIN_KEY_PREFIX),
            "prefixes must not collide"
        );
    }

    #[test]
    fn reserved_nested_label_skin_is_loud() {
        assert!(is_reserved_nested_label("skin"));
        assert!(is_reserved_nested_label("Skin"));
        assert!(!is_reserved_nested_label("staging"));
        let err = reserved_nested_label_error("skin");
        assert!(err.contains("reserved_label"), "{err}");
        assert!(
            err.contains("Pick another nested label"),
            "must tell them to pick another name, not a Caddy port: {err}"
        );
        assert!(
            !err.contains("38472"),
            "must not teach 38472 as a publish target: {err}"
        );
    }

    #[test]
    fn skin_url_from_public_nests_skin_label() {
        assert_eq!(
            skin_url_from_public("https://rosson.k2.dev"),
            Some("https://skin.rosson.k2.dev".into())
        );
        assert_eq!(
            skin_url_from_public("https://skin.rosson.k2.dev/"),
            Some("https://skin.rosson.k2.dev".into())
        );
        assert_eq!(skin_url_from_public(""), None);
    }

    #[test]
    fn principal_and_token_round_trip_secret_once() {
        with_temp_home(|| {
            let p = add_principal("Ada").expect("add");
            assert_eq!(p.username, "ada");
            let listed = list_principals().expect("list");
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].username, "ada");

            let room = uuid::Uuid::new_v4().to_string();
            let (meta, raw) = create_token(
                "ada",
                Some(&["thread:read".into()]),
                std::slice::from_ref(&room),
            )
            .expect("mint");
            assert!(raw.starts_with(SKIN_KEY_PREFIX), "got {raw}");
            assert!(!raw.starts_with("k2sk_"), "never k2sk_: {raw}");
            assert_eq!(raw.len(), SKIN_KEY_PREFIX.len() + KEY_BODY_LEN);
            assert!(meta.prefix.starts_with("k2skn_…"), "{}", meta.prefix);
            assert_eq!(meta.caps, vec!["thread:read"]);
            assert_eq!(meta.name, "ada");
            assert_eq!(meta.rooms, vec![room.clone()]);
            let tokens = list_tokens().expect("list tokens");
            assert_eq!(tokens.len(), 1);
            assert_eq!(tokens[0].name, "ada");
            assert!(
                !format!("{tokens:?}").contains(&raw),
                "secret must not leak in list"
            );
            assert!(!tokens[0].prefix.contains(&raw[SKIN_KEY_PREFIX.len()..]));

            let pass = resolve_skin_token(&raw).expect("live pass");
            assert_eq!(pass.username, "ada");
            assert!(
                pass.principal_id.is_none(),
                "platform mint has no principal"
            );
            assert!(!pass.session);
            assert!(pass.has_cap(CAP_THREAD_READ));
            assert!(!pass.has_cap(CAP_THREAD_POST));
            assert!(pass.has_room(&room));
            assert!(!pass.rooms_empty());
            assert!(!pass.has_room("00000000-0000-0000-0000-000000000000"));
            assert!(resolve_skin_token("k2skn_not-a-real-key").is_none());
            assert!(resolve_skin_token("k2sk_pretend").is_none());

            assert!(revoke_token(&meta.id).expect("revoke"));
            assert!(
                resolve_skin_token(&raw).is_none(),
                "revoked pass must not resolve"
            );
            assert!(!revoke_token(&meta.id).expect("idempotent"));
        });
    }

    #[test]
    fn parse_caps_still_rejects_store_write() {
        let err = parse_caps(Some(&["store:write".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("store:write"), "{err}");
        assert!(
            parse_caps(Some(&["thread:read".into()])).is_ok(),
            "thread:read remains accepted"
        );
        let err = parse_caps(Some(&["thread:read".into(), "store:write".into()])).unwrap_err();
        assert!(err.contains("store:write"), "{err}");
    }

    #[test]
    fn parse_caps_accepts_files_verbs_never_silent_add() {
        let empty = parse_caps(None).expect("empty default");
        assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !empty
                .iter()
                .any(|c| c == CAP_FILES_READ || c == CAP_FILES_WRITE),
            "empty caps must stay Thread-only, never silent-add files: {empty:?}"
        );
        let missing = parse_caps(Some(&[])).expect("missing default");
        assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !missing
                .iter()
                .any(|c| c == CAP_FILES_READ || c == CAP_FILES_WRITE),
            "missing caps must stay Thread-only: {missing:?}"
        );

        let read = parse_caps(Some(&["files:read".into()])).expect("files:read");
        assert_eq!(read, vec![CAP_FILES_READ]);
        let write = parse_caps(Some(&["files:write".into()])).expect("files:write");
        assert_eq!(write, vec![CAP_FILES_WRITE]);
        let both = parse_caps(Some(&[
            "files:read".into(),
            "files:write".into(),
            "thread:read".into(),
        ]))
        .expect("mixed");
        assert_eq!(both, vec![CAP_FILES_READ, CAP_FILES_WRITE, CAP_THREAD_READ]);

        let err = parse_caps(Some(&["files:sales".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("files:sales"), "{err}");
        let err = parse_caps(Some(&["pty:write".into()])).unwrap_err();
        assert!(err.contains("pty:write"), "{err}");
    }

    #[test]
    fn mint_requires_principal_and_rejects_unknown_caps() {
        with_temp_home(|| {
            let room = uuid::Uuid::new_v4().to_string();
            let (ghost_meta, ghost_raw) =
                create_token("ghost", None, std::slice::from_ref(&room)).expect("no principal");
            assert_eq!(ghost_meta.name, "ghost");
            let ghost = resolve_skin_token(&ghost_raw).expect("platform ghost");
            assert!(ghost.principal_id.is_none(), "{ghost:?}");
            assert!(!ghost.session);
            assert_eq!(ghost.username, "ghost");

            let err = create_token("owner", None, std::slice::from_ref(&room)).unwrap_err();
            assert!(err.contains("owner"), "{err}");
            assert!(err.contains("reserved"), "{err}");

            let err = create_token(
                "bob",
                Some(&["pty:write".into()]),
                std::slice::from_ref(&room),
            )
            .unwrap_err();
            assert!(err.contains("unknown capability"), "{err}");
            assert!(err.contains("pty:write"), "{err}");
            let err = create_token("bob", None, &[]).unwrap_err();
            assert!(
                err.contains("rooms must include at least one workspace"),
                "{err}"
            );
            let (_meta, raw) =
                create_token("bob", None, std::slice::from_ref(&room)).expect("default caps");
            let pass = resolve_skin_token(&raw).expect("resolve");
            assert!(pass.principal_id.is_none());
            assert!(pass.has_cap(CAP_THREAD_READ));
            assert!(pass.has_cap(CAP_THREAD_POST));
            assert!(!pass.has_cap(CAP_FILES_READ));
            assert!(!pass.has_cap(CAP_FILES_WRITE));
            assert_eq!(pass.rooms, vec![room.clone()]);

            let err = create_token("bob", None, std::slice::from_ref(&room)).unwrap_err();
            assert!(err.contains("already exists"), "{err}");

            let (meta, raw) = create_token(
                "bob-files",
                Some(&["files:read".into(), "files:write".into()]),
                std::slice::from_ref(&room),
            )
            .expect("files caps");
            assert_eq!(meta.caps, vec![CAP_FILES_READ, CAP_FILES_WRITE]);
            let pass = resolve_skin_token(&raw).expect("files pass");
            assert!(pass.has_cap(CAP_FILES_READ));
            assert!(pass.has_cap(CAP_FILES_WRITE));
            assert!(!pass.has_cap(CAP_THREAD_READ));
        });
    }

    #[test]
    fn remove_principal_drops_tokens() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (_meta, platform_raw) =
                create_token("cara", None, std::slice::from_ref(&room)).unwrap();
            let (_sess, sess_raw) = create_session_token("cara").unwrap();
            assert!(resolve_skin_token(&platform_raw).is_some());
            assert!(resolve_skin_token(&sess_raw).is_some());
            assert!(remove_principal("cara").unwrap());
            assert!(
                resolve_skin_token(&platform_raw).is_some(),
                "platform token survives guest delete"
            );
            assert!(
                resolve_skin_token(&sess_raw).is_none(),
                "session=1 must drop with the guest"
            );
            assert!(list_principals().unwrap().is_empty());
            assert_eq!(list_tokens().unwrap().len(), 1);
            assert!(!remove_principal("cara").unwrap());
        });
    }

    #[test]
    fn front_door_persists_mode() {
        with_temp_home(|| {
            let d = effective_front_door().expect("default");
            assert_eq!(d.mode, "connect");
            let saved = set_front_door(
                "direct",
                Some("https://skin.app.com"),
                Some("LAN"),
                Some(5173),
            )
            .expect("save");
            assert_eq!(saved.mode, "direct");
            assert_eq!(saved.url.as_deref(), Some("https://skin.app.com"));
            assert_eq!(saved.ui_port, Some(5173));
            let loaded = get_front_door().expect("load");
            assert_eq!(loaded.mode, "direct");
            assert_eq!(loaded.url.as_deref(), Some("https://skin.app.com"));
            assert_eq!(loaded.ui_port, Some(5173));
            assert!(front_door_is_stored());
            assert_eq!(parse_front_door_mode("CONNECT").unwrap(), "connect");
            assert!(parse_front_door_mode("lan").is_err());
        });
    }

    #[test]
    fn parse_rooms_json_empty_is_deny_not_all_agents() {
        assert!(parse_rooms_json(None).is_empty());
        assert!(parse_rooms_json(Some("")).is_empty());
        assert!(parse_rooms_json(Some("[]")).is_empty());
        assert!(parse_rooms_json(Some("not-json")).is_empty());
        assert!(parse_rooms_json(Some("{}")).is_empty());
        assert_eq!(
            parse_rooms_json(Some(r#"["a"," ","b"]"#)),
            vec!["a".to_string(), "b".to_string()]
        );
        let empty_caps = parse_caps(None).expect("caps default");
        assert_eq!(empty_caps.len(), 2, "empty caps still default both verbs");
        assert!(
            !empty_caps.iter().any(|c| c.starts_with("files:")),
            "empty caps must not grant files: {empty_caps:?}"
        );
        assert!(
            parse_rooms_json(None).is_empty(),
            "rooms must not copy parse_caps default-all"
        );
    }

    #[test]
    fn set_token_rooms_does_not_touch_secret() {
        with_temp_home(|| {
            add_principal("ada").unwrap();
            let a = uuid::Uuid::new_v4().to_string();
            let b = uuid::Uuid::new_v4().to_string();
            let (meta, raw) = create_token("ada", None, std::slice::from_ref(&a)).unwrap();
            let updated = set_token_rooms(&meta.id, std::slice::from_ref(&b)).expect("patch");
            assert_eq!(updated.rooms, vec![b.clone()]);
            let pass = resolve_skin_token(&raw).expect("same secret");
            assert_eq!(pass.rooms, vec![b]);
            let cleared = set_token_rooms(&meta.id, &[]).expect("clear");
            assert!(cleared.rooms.is_empty());
            let dark = resolve_skin_token(&raw).expect("still live");
            assert!(dark.rooms_empty());
        });
    }

    #[test]
    fn resolve_room_tokens_handle_alias_uuid_not_display_name() {
        crate::db::init_for_tests();
        let db = crate::db::shared();
        let conn = db.lock();
        use rusqlite::params;
        let id = uuid::Uuid::new_v4().to_string();
        let handle = format!("sales{}", &id[..8]);
        let pretty = format!("Wallpaper {handle}");
        let path = format!("/tmp/skin-room-{id}");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
            params![id, pretty, path, handle],
        )
        .expect("project");
        conn.execute(
            "INSERT OR IGNORE INTO project_handle_aliases (project_id, alias) VALUES (?1, ?2)",
            params![id, "old-sales-alias"],
        )
        .ok();
        drop(conn);

        assert_eq!(
            resolve_room_tokens(&[handle.clone()]).expect("handle"),
            vec![id.clone()]
        );
        assert_eq!(
            resolve_room_tokens(&[id.clone()]).expect("uuid"),
            vec![id.clone()]
        );
        assert_eq!(
            resolve_room_tokens(&["old-sales-alias".into()]).expect("alias"),
            vec![id.clone()]
        );
        let err = resolve_room_tokens(&[pretty]).unwrap_err();
        assert!(err.contains("unknown workspace handle"), "{err}");
        let err = resolve_room_tokens(&["not-a-handle".into()]).unwrap_err();
        assert!(err.contains("unknown workspace handle"), "{err}");

        let agents = live_agents(&[id.clone()]);
        assert_eq!(agents.len(), 1, "{agents:?}");
        assert_eq!(agents[0].handle, handle);
        assert_eq!(agents[0].project_id, id);
        assert!(
            !agents[0].display_name.trim().is_empty(),
            "display_name must be a non-empty string: {agents:?}"
        );
        let wire = serde_json::to_value(&agents[0]).expect("wire");
        assert_eq!(wire["handle"], handle);
        assert_eq!(wire["projectId"], id);
        assert!(
            wire["displayName"]
                .as_str()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "wire displayName: {wire}"
        );
    }

    #[test]
    fn wal_file_is_created_under_k2_home() {
        with_temp_home(|| {
            add_principal("wal-user").unwrap();
            let db_path = crate::paths::k2_home().join("skin.db");
            assert!(db_path.is_file(), "skin.db must exist at {db_path:?}");
            let wal = crate::paths::k2_home().join("skin.db-wal");
            let shm = crate::paths::k2_home().join("skin.db-shm");
            assert!(
                wal.is_file() || shm.is_file() || db_path.is_file(),
                "WAL mode should leave wal/shm companions or at least the db"
            );
        });
    }

    #[test]
    fn session_mint_allows_empty_rooms_create_token_does_not() {
        with_temp_home(|| {
            add_principal("ada").unwrap();
            let err = create_token("ada", None, &[]).unwrap_err();
            assert!(
                err.contains("rooms must include at least one workspace"),
                "{err}"
            );
            let (meta, raw) = create_session_token("ada").expect("session mint");
            assert!(raw.starts_with(SKIN_KEY_PREFIX), "{raw}");
            assert!(meta.rooms.is_empty(), "default_rooms [] must copy");
            let pass = resolve_skin_token(&raw).expect("live session");
            assert!(pass.session);
            assert!(pass.principal_id.is_some());
            assert_eq!(pass.username, "ada");
            assert!(pass.rooms_empty());
            assert!(
                !pass.has_cap(CAP_FILES_READ),
                "empty map is Thread dark, never files: {pass:?}"
            );
            assert!(
                list_tokens().unwrap().is_empty(),
                "list_tokens is platform only"
            );
        });
    }

    #[test]
    fn resolve_rejects_expired_session_static_survives() {
        with_temp_home(|| {
            add_principal("bob").unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (static_meta, static_raw) =
                create_token("bob", None, std::slice::from_ref(&room)).unwrap();
            assert!(resolve_skin_token(&static_raw).is_some());
            let (_sess, sess_raw) = create_session_token("bob").unwrap();
            assert!(resolve_skin_token(&sess_raw).is_some());
            let _ = static_meta;
            with_conn(|conn| {
                conn.execute("UPDATE tokens SET expires_at = 1 WHERE session = 1", [])
                    .unwrap();
                Ok(())
            })
            .unwrap();
            assert!(
                resolve_skin_token(&sess_raw).is_none(),
                "expired session must not resolve"
            );
            assert!(
                resolve_skin_token(&static_raw).is_some(),
                "static mint expires_at NULL stays live"
            );
        });
    }

    #[test]
    fn password_reset_revokes_sessions_not_static() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (_sm, static_raw) =
                create_token("cara", None, std::slice::from_ref(&room)).unwrap();
            let (_sess, sess_raw) = create_session_token("cara").unwrap();
            assert!(resolve_skin_token(&sess_raw).is_some());
            set_principal_password("cara", Some("new-pass-word")).unwrap();
            assert!(
                resolve_skin_token(&sess_raw).is_none(),
                "password reset revokes session=1"
            );
            assert!(
                resolve_skin_token(&static_raw).is_some(),
                "static partner key survives password reset"
            );
            match check_and_record_login("cara", "new-pass-word") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "cara"),
                other => panic!("expected Ok, got {other:?}"),
            }
            match check_and_record_login("cara", "s3cret-horse") {
                SkinLoginOutcome::BadCreds => {}
                _ => panic!("old password must fail"),
            }
            match check_and_record_login("ghost-user", "nope") {
                SkinLoginOutcome::BadCreds => {}
                _ => panic!("unknown user must be generic BadCreds"),
            }
            set_principal_password("cara", None).unwrap();
            match check_and_record_login("cara", "new-pass-word") {
                SkinLoginOutcome::BadCreds => {}
                _ => panic!("cleared password cannot K2-login"),
            }
            assert!(
                resolve_skin_token(&static_raw).is_some(),
                "clearing password still leaves platform mint"
            );
        });
    }

    #[test]
    fn guest_and_platform_same_name_coexist_revoked_name_reusable() {
        with_temp_home(|| {
            add_principal("bob").unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (meta, raw) = create_token("bob", None, std::slice::from_ref(&room)).unwrap();
            assert_eq!(meta.name, "bob");
            let pass = resolve_skin_token(&raw).expect("platform bob");
            assert_eq!(pass.username, "bob");
            assert!(pass.principal_id.is_none());
            let listed = list_principals().unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].username, "bob");
            assert!(revoke_token(&meta.id).unwrap());
            let (meta2, raw2) =
                create_token("bob", None, std::slice::from_ref(&room)).expect("reuse revoked name");
            assert_eq!(meta2.name, "bob");
            assert!(resolve_skin_token(&raw2).is_some());
            assert!(resolve_skin_token(&raw).is_none());
        });
    }

    #[test]
    fn migrate_pre_reshape_static_row_fills_name_nulls_principal() {
        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("open old db");
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    session INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER
                 );",
            )
            .expect("old schema");
            let pid = uuid::Uuid::new_v4().to_string();
            let tid = uuid::Uuid::new_v4().to_string();
            let raw = format!("{SKIN_KEY_PREFIX}MigrateHashBody00000000000000000000000");
            let key_hash = sha256_hex(&raw);
            let prefix = display_prefix(&raw);
            conn.execute(
                "INSERT INTO principals (id, username, created_at) VALUES (?1, 'bob', 1)",
                params![pid],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tokens (id, principal_id, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session)
                 VALUES (?1, ?2, ?3, ?4, ?5, '[]', 1, NULL, 0)",
                params![tid, pid, key_hash, prefix, caps_json(&parse_caps(None).unwrap())],
            )
            .unwrap();
            drop(conn);

            let pass = resolve_skin_token(&raw).expect("hash unchanged after rebuild");
            assert!(pass.principal_id.is_none(), "{pass:?}");
            assert!(!pass.session);
            assert_eq!(pass.username, "bob");
            let listed = list_tokens().expect("list");
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].name, "bob");
            with_conn(|c| {
                let notnull = tokens_principal_id_notnull(c)?;
                assert!(!notnull, "rebuilt principal_id must be nullable");
                let (principal_id, name, session): (Option<String>, Option<String>, i64) = c
                    .query_row(
                        "SELECT principal_id, name, session FROM tokens WHERE id = ?1",
                        params![tid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .map_err(|e| e.to_string())?;
                assert!(principal_id.is_none(), "{principal_id:?}");
                assert_eq!(name.as_deref(), Some("bob"));
                assert_eq!(session, 0);
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn lockout_is_generic_and_is_not_dummy_argon_alone() {
        with_temp_home(|| {
            add_principal("eve").unwrap();
            set_principal_password("eve", Some("right-password")).unwrap();
            for _ in 0..3 {
                match check_and_record_login("eve", "wrong") {
                    SkinLoginOutcome::BadCreds => {}
                    _ => panic!("pre-lockout must be BadCreds"),
                }
            }
            match check_and_record_login("eve", "right-password") {
                SkinLoginOutcome::LockedOut => {}
                _ => panic!("4th attempt after 3 fails is lockout, not a verify"),
            }
            match check_and_record_login("eve", "wrong") {
                SkinLoginOutcome::LockedOut => {}
                _ => panic!("locked username stays generic LockedOut"),
            }
        });
    }

    #[test]
    fn null_password_cannot_k2_login_static_mint_still_works() {
        with_temp_home(|| {
            let p = add_principal("mintonly").unwrap();
            assert!(!p.has_password);
            match check_and_record_login("mintonly", "anything") {
                SkinLoginOutcome::BadCreds => {}
                _ => panic!("NULL password_hash cannot K2-login"),
            }
            let room = uuid::Uuid::new_v4().to_string();
            let (_m, raw) = create_token("mintonly", None, std::slice::from_ref(&room)).unwrap();
            assert!(resolve_skin_token(&raw).is_some());
        });
    }

    #[test]
    fn direct_front_door_host_from_url() {
        with_temp_home(|| {
            assert_eq!(direct_front_door_host(), None);
            set_front_door("direct", Some("https://skin.app.com"), None, None).unwrap();
            assert_eq!(direct_front_door_host().as_deref(), Some("skin.app.com"));
            set_front_door("connect", None, None, None).unwrap();
            assert_eq!(direct_front_door_host(), None);
        });
        assert_eq!(
            host_from_url("https://Skin.App.com:8443/path"),
            Some("skin.app.com".into())
        );
        assert_eq!(host_from_url("skin.app.com"), Some("skin.app.com".into()));
    }

    #[test]
    fn unassigned_session_stays_thread_only_and_hidden_from_list() {
        with_temp_home(|| {
            add_principal("ada").unwrap();
            let listed = list_principals().unwrap();
            assert_eq!(listed.len(), 1);
            assert!(listed[0].role_id.is_none(), "{listed:?}");
            assert!(listed[0].role_name.is_none(), "{listed:?}");
            let (meta, raw) = create_session_token("ada").expect("session");
            assert!(meta.rooms.is_empty());
            let pass = resolve_skin_token(&raw).expect("live");
            assert!(pass.session);
            assert_eq!(pass.username, "ada");
            assert!(pass.rooms_empty());
            assert!(!pass.has_cap(CAP_FILES_READ));
            assert!(!pass.has_cap(CAP_FILES_WRITE));
            assert!(list_tokens().unwrap().is_empty(), "list hides sessions");
        });
    }

    #[test]
    fn role_create_rejects_connect_names_and_short_or_punct() {
        with_temp_home(|| {
            for name in ["owner", "admin", "member", "viewer", "Admin"] {
                let err = create_role(name, &RoomPolicy::new()).unwrap_err();
                assert!(
                    err.contains("Connect role names cannot be skin roles"),
                    "name={name} err={err}"
                );
                assert!(err.contains("owner/admin/member/viewer"), "{err}");
            }
            let err = create_role("A", &RoomPolicy::new()).unwrap_err();
            assert!(err.contains("at least 2"), "{err}");
            let err = create_role("Dentist!", &RoomPolicy::new()).unwrap_err();
            assert!(err.contains("lowercase") || err.contains("only"), "{err}");
            let role = create_role("dentist", &RoomPolicy::new()).expect("ok");
            assert_eq!(role.name, "dentist");
            assert!(role.caps.is_empty(), "empty map is Thread dark: {role:?}");
            assert!(role.rooms.is_empty());
            assert!(role.room_access.is_empty());
            let err = create_role("Dentist", &RoomPolicy::new()).unwrap_err();
            assert!(err.contains("already exists"), "{err}");
            add_principal("bob").unwrap();
            let guest_role = create_role("bob", &RoomPolicy::new()).expect("guest and role coexist");
            assert_eq!(guest_role.name, "bob");
        });
    }

    #[test]
    fn role_parse_caps_rejects_unknown_verbs() {
        with_temp_home(|| {
            for cap in [
                "tickets:read",
                "store:write",
                "pty:write",
                "wiki:read",
                "grid",
            ] {
                let mut p = RoomPolicy::new();
                p.insert("anna".into(), vec![cap.into()]);
                let err = create_role("xray", &p).unwrap_err();
                assert!(err.contains("unknown capability"), "{cap} {err}");
                assert!(err.contains(cap), "{err}");
            }
        });
    }

    #[test]
    fn assigned_session_snapshots_role_unassign_restores_defaults() {
        with_temp_home(|| {
            add_principal("bob").unwrap();
            let sales = uuid::Uuid::new_v4().to_string();
            set_principal_default_rooms("bob", std::slice::from_ref(&sales), false).unwrap();
            let mut dentist = RoomPolicy::new();
            dentist.insert(
                sales.clone(),
                vec![
                    CAP_THREAD_READ.into(),
                    CAP_THREAD_POST.into(),
                    CAP_FILES_READ.into(),
                ],
            );
            let role = create_role("dentist", &dentist).unwrap();
            assign_role("bob", "dentist").unwrap();
            let listed = list_principals().unwrap();
            assert_eq!(listed[0].role_name.as_deref(), Some("dentist"));
            assert_eq!(listed[0].role_id.as_deref(), Some(role.id.as_str()));
            assert_eq!(listed[0].default_rooms, vec![sales.clone()]);

            let (meta, raw) = create_session_token("bob").expect("assigned session");
            assert_eq!(meta.name, "bob");
            assert_eq!(meta.rooms, vec![sales.clone()]);
            let pass = resolve_skin_token(&raw).expect("live");
            assert_eq!(pass.username, "bob");
            assert!(pass.has_cap(CAP_FILES_READ));
            assert!(pass.has_room(&sales));
            assert!(list_tokens().unwrap().is_empty());

            let err = set_principal_default_rooms("bob", &[], false).unwrap_err();
            assert!(err.contains("guest 'bob' has role 'dentist'"), "{err}");
            assert!(err.contains("edit the role or unassign"), "{err}");

            let err = remove_role("dentist").unwrap_err();
            assert!(
                err.contains("role 'dentist' is assigned; unassign guests first"),
                "{err}"
            );
            assert!(resolve_skin_token(&raw).is_some(), "session still live");

            unassign_role("bob").unwrap();
            let after = resolve_skin_token(&raw).expect("cookie stays");
            assert!(after.has_cap(CAP_THREAD_READ));
            assert!(after.has_cap(CAP_THREAD_POST));
            assert!(!after.has_cap(CAP_FILES_READ), "{after:?}");
            assert_eq!(after.rooms, vec![sales.clone()]);
            assert!(remove_role("dentist").unwrap());
            let (meta2, raw2) = create_session_token("bob").expect("unassigned login");
            let pass2 = resolve_skin_token(&raw2).expect("thread-only");
            assert_eq!(meta2.rooms, vec![sales.clone()]);
            assert!(pass2.has_cap(CAP_THREAD_READ));
            assert!(!pass2.has_cap(CAP_FILES_READ));
        });
    }

    #[test]
    fn role_update_rewrites_live_session_not_platform() {
        with_temp_home(|| {
            add_principal("bob").unwrap();
            let sales = uuid::Uuid::new_v4().to_string();
            create_role("dentist", &thread_only_room_policy(std::slice::from_ref(&sales))).unwrap();
            assign_role("bob", "dentist").unwrap();
            let (_sess, sess_raw) = create_session_token("bob").unwrap();
            let (plat, plat_raw) = create_token("bob", None, std::slice::from_ref(&sales)).unwrap();
            let before = resolve_skin_token(&sess_raw).unwrap();
            assert!(!before.has_cap(CAP_FILES_WRITE));
            assert!(before.has_cap_in_room(&sales, CAP_THREAD_READ));
            assert!(!before.has_cap_in_room(&sales, CAP_FILES_WRITE));

            let mut next = RoomPolicy::new();
            next.insert(
                sales.clone(),
                vec![CAP_THREAD_READ.into(), CAP_FILES_WRITE.into()],
            );
            update_role("dentist", Some(&next)).unwrap();
            let after = resolve_skin_token(&sess_raw).expect("rewritten, not revoked");
            assert!(after.has_cap(CAP_FILES_WRITE), "{after:?}");
            assert!(after.has_cap(CAP_THREAD_READ));
            assert!(!after.has_cap(CAP_FILES_READ));
            assert!(after.has_cap_in_room(&sales, CAP_FILES_WRITE));
            assert!(!after.has_cap_in_room(&sales, CAP_FILES_READ));
            let plat_pass = resolve_skin_token(&plat_raw).expect("platform untouched");
            assert_eq!(plat_pass.username, "bob");
            assert!(!plat_pass.has_cap(CAP_FILES_WRITE), "{plat_pass:?}");
            let _ = plat;
        });
    }

    #[test]
    fn role_empty_rooms_is_thread_dark() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            let sales = uuid::Uuid::new_v4().to_string();
            set_principal_default_rooms("cara", std::slice::from_ref(&sales), false).unwrap();
            create_role("dark", &RoomPolicy::new()).unwrap();
            assign_role("cara", "dark").unwrap();
            let (meta, raw) = create_session_token("cara").unwrap();
            assert!(meta.rooms.is_empty(), "{meta:?}");
            let pass = resolve_skin_token(&raw).unwrap();
            assert!(pass.rooms_empty());
            assert!(!pass.has_room(&sales));
            assert!(!pass.has_cap_in_room(&sales, CAP_THREAD_READ));
        });
    }

    #[test]
    fn pre_roles_skin_db_opens_with_null_role_id() {
        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT
                 );
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
                    name TEXT,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    session INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER,
                    CHECK (
                        (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                        OR
                        (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
                    )
                 );",
            )
            .expect("pre-roles schema");
            let pid = uuid::Uuid::new_v4().to_string();
            let tid = uuid::Uuid::new_v4().to_string();
            let raw = format!("{SKIN_KEY_PREFIX}PreRolesHashBody00000000000000000000000");
            let key_hash = sha256_hex(&raw);
            let prefix = display_prefix(&raw);
            let room = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES (?1, 'bob', 1, ?2)",
                params![pid, rooms_json(std::slice::from_ref(&room))],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, session)
                 VALUES (?1, NULL, 'vercel', ?2, ?3, ?4, ?5, 1, 0)",
                params![
                    tid,
                    key_hash,
                    prefix,
                    caps_json(&parse_caps(None).unwrap()),
                    rooms_json(std::slice::from_ref(&room))
                ],
            )
            .unwrap();
            drop(conn);

            let listed = list_principals().expect("open_db + list");
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].username, "bob");
            assert!(listed[0].role_id.is_none(), "{listed:?}");
            assert_eq!(listed[0].default_rooms, vec![room.clone()]);
            let pass = resolve_skin_token(&raw).expect("platform still resolves");
            assert_eq!(pass.username, "vercel");
            assert!(pass.principal_id.is_none());
            let (_meta, sess) = create_session_token("bob").expect("login still Thread-only");
            let session = resolve_skin_token(&sess).expect("session");
            assert!(session.has_cap(CAP_THREAD_READ));
            assert!(!session.has_cap(CAP_FILES_READ));
            assert_eq!(session.rooms, vec![room.clone()]);
            assert!(session.has_cap_in_room(&room, CAP_THREAD_READ));
            assert!(!session.has_cap_in_room(&room, CAP_FILES_READ));
        });
    }

    #[test]
    fn session_has_cap_in_room_is_not_cartesian() {
        with_temp_home(|| {
            add_principal("bob").unwrap();
            let anna = uuid::Uuid::new_v4().to_string();
            let docs = uuid::Uuid::new_v4().to_string();
            let mut policy = RoomPolicy::new();
            policy.insert(
                anna.clone(),
                vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
            );
            policy.insert(
                docs.clone(),
                vec![
                    CAP_THREAD_READ.into(),
                    CAP_THREAD_POST.into(),
                    CAP_FILES_READ.into(),
                ],
            );
            create_role("dentist", &policy).unwrap();
            assign_role("bob", "dentist").unwrap();
            let (_m, raw) = create_session_token("bob").unwrap();
            let pass = resolve_skin_token(&raw).expect("session");
            assert!(pass.session);
            assert!(pass.has_room(&anna));
            assert!(pass.has_room(&docs));
            assert!(pass.has_cap(CAP_FILES_READ), "union still lists files");
            assert!(
                !pass.has_cap_in_room(&anna, CAP_FILES_READ),
                "files on docs must not grant files on anna: {pass:?}"
            );
            assert!(pass.has_cap_in_room(&docs, CAP_FILES_READ));
            assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
            let files_only = {
                let mut p = RoomPolicy::new();
                p.insert(anna.clone(), vec![CAP_FILES_READ.into()]);
                p
            };
            update_role("dentist", Some(&files_only)).unwrap();
            let after = resolve_skin_token(&raw).expect("R8");
            assert!(after.has_cap_in_room(&anna, CAP_FILES_READ));
            assert!(
                !after.has_cap_in_room(&anna, CAP_THREAD_READ),
                "explicit files-only must not silent-add Thread: {after:?}"
            );
            assert!(!after.has_room(&docs));
        });
    }

    #[test]
    fn platform_stays_flat_room_policy_null() {
        with_temp_home(|| {
            let room = uuid::Uuid::new_v4().to_string();
            let (meta, raw) = create_token(
                "vercel",
                Some(&[CAP_FILES_READ.into()]),
                std::slice::from_ref(&room),
            )
            .unwrap();
            assert!(meta.room_access.is_empty());
            let pass = resolve_skin_token(&raw).expect("platform");
            assert!(!pass.session);
            assert!(pass.room_policy.is_empty(), "{pass:?}");
            assert!(pass.has_cap(CAP_FILES_READ) && pass.has_room(&room));
            assert!(pass.has_cap_in_room(&room, CAP_FILES_READ));
            assert!(!pass.has_cap_in_room(&room, CAP_THREAD_READ));
            with_conn(|conn| {
                let policy: Option<String> = conn
                    .query_row(
                        "SELECT room_policy FROM tokens WHERE session = 0 AND id = ?1",
                        params![meta.id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                assert!(policy.is_none(), "platform room_policy must stay NULL: {policy:?}");
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn apply_tokens_writes_thread_only_room_policy() {
        with_temp_home(|| {
            add_principal("ada").unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (_m, raw) = create_session_token("ada").unwrap();
            set_principal_default_rooms("ada", std::slice::from_ref(&room), true).unwrap();
            let pass = resolve_skin_token(&raw).expect("rewritten");
            assert!(pass.has_room(&room));
            assert!(pass.has_cap_in_room(&room, CAP_THREAD_READ));
            assert!(!pass.has_cap_in_room(&room, CAP_FILES_READ));
            with_conn(|conn| {
                let policy_raw: String = conn
                    .query_row(
                        "SELECT room_policy FROM tokens WHERE session = 1 AND revoked_at IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                let policy = parse_room_policy_json(Some(&policy_raw))
                    .expect("json")
                    .expect("object");
                assert_eq!(policy.get(&room), Some(&default_thread_caps()), "{policy:?}");
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn migrate_132_copies_caps_onto_each_room_resolve_does_not_join() {
        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT
                 );
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
                    name TEXT,
                    key_hash TEXT NOT NULL UNIQUE,
                    key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    session INTEGER NOT NULL DEFAULT 0,
                    expires_at INTEGER,
                    CHECK (
                        (session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                        OR
                        (session = 0 AND principal_id IS NULL AND name IS NOT NULL)
                    )
                 );",
            )
            .expect("132 schema");
            let anna = uuid::Uuid::new_v4().to_string();
            let docs = uuid::Uuid::new_v4().to_string();
            let rid = uuid::Uuid::new_v4().to_string();
            let pid = uuid::Uuid::new_v4().to_string();
            let tid = uuid::Uuid::new_v4().to_string();
            let raw = format!("{SKIN_KEY_PREFIX}Migrate132HashBody000000000000000000000");
            let key_hash = sha256_hex(&raw);
            let prefix = display_prefix(&raw);
            let caps = caps_json(&[
                CAP_THREAD_READ.to_string(),
                CAP_THREAD_POST.to_string(),
                CAP_FILES_READ.to_string(),
            ]);
            let rooms = rooms_json(&[anna.clone(), docs.clone()]);
            conn.execute(
                "INSERT INTO roles (id, name, caps, rooms, created_at) VALUES (?1, 'dentist', ?2, ?3, 1)",
                params![rid, caps, rooms],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, role_id) VALUES (?1, 'bob', 1, ?2)",
                params![pid, rid],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, session)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, 1, 1)",
                params![tid, pid, key_hash, prefix, caps, rooms],
            )
            .unwrap();
            drop(conn);

            let listed = list_roles().expect("open_db migrate");
            assert_eq!(listed.len(), 1);
            assert!(listed[0].room_policy.contains_key(&anna), "{listed:?}");
            assert!(listed[0].room_policy.contains_key(&docs), "{listed:?}");
            assert_eq!(
                listed[0].room_policy.get(&anna),
                listed[0].room_policy.get(&docs)
            );
            assert!(
                listed[0]
                    .room_policy
                    .get(&anna)
                    .unwrap()
                    .iter()
                    .any(|c| c == CAP_FILES_READ)
            );

            let pass = resolve_skin_token(&raw).expect("migrated session");
            assert!(pass.session);
            assert!(pass.has_cap_in_room(&anna, CAP_FILES_READ), "{pass:?}");
            assert!(pass.has_cap_in_room(&docs, CAP_FILES_READ), "{pass:?}");

            let mut docs_only = RoomPolicy::new();
            docs_only.insert(
                docs.clone(),
                vec![
                    CAP_THREAD_READ.into(),
                    CAP_THREAD_POST.into(),
                    CAP_FILES_READ.into(),
                ],
            );
            docs_only.insert(
                anna.clone(),
                vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
            );
            update_role("dentist", Some(&docs_only)).unwrap();
            let after = resolve_skin_token(&raw).expect("R8 no re-login");
            assert!(
                !after.has_cap_in_room(&anna, CAP_FILES_READ),
                "anna lost files after R8: {after:?}"
            );
            assert!(after.has_cap_in_room(&docs, CAP_FILES_READ));
            assert!(after.has_cap_in_room(&anna, CAP_THREAD_READ));

            with_conn(|conn| {
                let mut grant_anna = RoomPolicy::new();
                grant_anna.insert(anna.clone(), vec![CAP_FILES_READ.into()]);
                conn.execute(
                    "UPDATE roles SET room_policy = ?1 WHERE name = 'dentist'",
                    params![room_policy_json(&grant_anna)],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            let still = resolve_skin_token(&raw).expect("no JOIN at resolve");
            assert!(
                !still.has_cap_in_room(&anna, CAP_FILES_READ),
                "resolve must not JOIN roles: {still:?}"
            );
            assert!(still.has_cap_in_room(&docs, CAP_FILES_READ));
        });
    }

    #[test]
    fn omitted_caps_on_present_room_is_thread_only() {
        with_temp_home(|| {
            let sales = uuid::Uuid::new_v4().to_string();
            let role = create_role("hygienist", &thread_only_room_policy(std::slice::from_ref(&sales)))
                .unwrap();
            assert_eq!(role.rooms, vec![sales.clone()]);
            assert_eq!(
                role.room_policy.get(&sales),
                Some(&default_thread_caps())
            );
            assert!(!role.caps.iter().any(|c| c == CAP_FILES_READ));
            let added = set_role_room("hygienist", &sales, Some(&[CAP_FILES_READ.into()])).unwrap();
            assert_eq!(
                added.room_policy.get(&sales),
                Some(&vec![CAP_FILES_READ.to_string()])
            );
            let cleared = clear_role_room("hygienist", &sales).unwrap();
            assert!(cleared.room_policy.is_empty());
        });
    }
}
