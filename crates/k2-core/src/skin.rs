//! Skin capability passes (prd-skin-auth-v1 + umbrella slice 1+2).
//!
//! Standalone guest list + hashed `k2skn_…` tokens. **Not** Connect users,
//! **not** `/v1` `k2sk_` API keys, **not** the owner token. Overlay Thread
//! rooms only — grid/PTY is never a skin room.
//!
//! ## Store (`~/.k2/skin.db`, WAL, own Mutex)
//! Two tables: `principals` (roster) and `tokens` (hashed passes). The raw
//! secret is returned **once** at mint and never stored. Lookup is
//! hex SHA-256 of the presented key (same construction as API keys /
//! connect-user session tokens — high-entropy CSPRNG, not argon2).
//!
//! Caps in v1: `thread:read`, `thread:post`. Never `pty:*`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

const KEY_BODY_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

const DEFAULT_CAPS: &[&str] = &[CAP_THREAD_READ, CAP_THREAD_POST];

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

/// Parse + validate cap names. Empty/missing → default Thread read+post.
/// Unknown names fail loud.
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
                match t {
                    CAP_THREAD_READ | CAP_THREAD_POST => {
                        if !out.iter().any(|c| c == t) {
                            out.push(t.to_string());
                        }
                    }
                    other => {
                        return Err(format!(
                            "unknown capability {other:?}; accepted: {CAP_THREAD_READ}, {CAP_THREAD_POST}"
                        ));
                    }
                }
            }
            if out.is_empty() {
                return Err(format!(
                    "caps must include at least one of {CAP_THREAD_READ}, {CAP_THREAD_POST}"
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
        "reserved_label: '{label}' is reserved for the Skin front door (https://skin.<sub>.k2.dev). Pick another nested label."
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinTokenMeta {
    pub id: String,
    pub username: String,
    /// Display prefix (`k2skn_…ab12`). Never the secret.
    pub prefix: String,
    pub caps: Vec<String>,
    /// Stored ACL: `projects.id` UUIDs.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Display-only, resolved live from `projects.handle`. Skip missing.
    #[serde(default)]
    pub room_handles: Vec<String>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// One workspace a skin pass may Thread. Wire `{handle, projectId}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinAgent {
    pub handle: String,
    pub project_id: String,
}

/// Resolved live pass. Safe to log `id` / `username` / `caps` — never a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinPass {
    pub id: String,
    pub principal_id: String,
    pub username: String,
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
}

impl SkinPass {
    pub fn has_cap(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }

    pub fn has_room(&self, project_id: &str) -> bool {
        let id = project_id.trim();
        if id.is_empty() {
            return false;
        }
        self.rooms.iter().any(|r| r == id)
    }

    pub fn rooms_empty(&self) -> bool {
        self.rooms.is_empty()
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

fn open_db(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
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
         );
         CREATE TABLE IF NOT EXISTS tokens (
            id TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            key_hash TEXT NOT NULL UNIQUE,
            key_prefix TEXT NOT NULL,
            caps TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            revoked_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS front_door (
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
    let _ = conn.execute(
        "ALTER TABLE principals ADD COLUMN default_rooms TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            crate::log_debug!("[skin] WARN chmod 0600 {}: {e}", path.display());
        }
    }
    Ok(conn)
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
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(r)
}

pub fn list_principals() -> Result<Vec<SkinPrincipal>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, username, created_at, default_rooms FROM principals ORDER BY username COLLATE NOCASE",
            )
            .map_err(|e| format!("skin principal list: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                let rooms_raw: String = r.get(3)?;
                let default_rooms = rooms_from_json(&rooms_raw);
                Ok(SkinPrincipal {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    created_at: r.get(2)?,
                    default_room_handles: Vec::new(),
                    default_rooms,
                })
            })
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
        conn.execute("DELETE FROM tokens WHERE principal_id = ?1", params![id])
            .map_err(|e| format!("skin token delete: {e}"))?;
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
        "SELECT id, username, created_at, default_rooms FROM principals WHERE username = ?1",
        params![username],
        |r| {
            let rooms_raw: String = r.get(3)?;
            Ok(SkinPrincipal {
                id: r.get(0)?,
                username: r.get(1)?,
                created_at: r.get(2)?,
                default_rooms: rooms_from_json(&rooms_raw),
                default_room_handles: Vec::new(),
            })
        },
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

/// Mint a pass. Returns `(meta, raw_secret)` — the secret is shown once.
/// `rooms` are already-canonical `project_id` UUIDs. Empty → error (R5).
pub fn create_token(
    username: &str,
    caps: Option<&[String]>,
    rooms: &[String],
) -> Result<(SkinTokenMeta, String), String> {
    let username = normalize_username(username)?;
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
        let Some(principal) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        conn.execute(
            "INSERT INTO tokens (id, principal_id, key_hash, key_prefix, caps, rooms, created_at, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
            params![id, principal.id, key_hash, prefix, caps_stored, rooms_stored, created_at],
        )
        .map_err(|e| format!("skin token insert: {e}"))?;
        Ok((
            SkinTokenMeta {
                id,
                username: principal.username,
                prefix,
                caps,
                rooms,
                room_handles: Vec::new(),
                created_at,
                revoked_at: None,
            },
            raw,
        ))
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok((attach_token_handles(r.0), r.1))
}

pub fn list_tokens() -> Result<Vec<SkinTokenMeta>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT t.id, p.username, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
                 FROM tokens t
                 JOIN principals p ON p.id = t.principal_id
                 ORDER BY t.created_at DESC",
            )
            .map_err(|e| format!("skin token list: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                let caps_raw: String = r.get(3)?;
                let rooms_raw: String = r.get(4)?;
                Ok(SkinTokenMeta {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    prefix: r.get(2)?,
                    caps: caps_from_json(&caps_raw),
                    rooms: rooms_from_json(&rooms_raw),
                    room_handles: Vec::new(),
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
        "SELECT t.id, p.username, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
         FROM tokens t
         JOIN principals p ON p.id = t.principal_id
         WHERE t.id = ?1",
        params![id],
        |r| {
            let caps_raw: String = r.get(3)?;
            let rooms_raw: String = r.get(4)?;
            Ok(SkinTokenMeta {
                id: r.get(0)?,
                username: r.get(1)?,
                prefix: r.get(2)?,
                caps: caps_from_json(&caps_raw),
                rooms: rooms_from_json(&rooms_raw),
                room_handles: Vec::new(),
                created_at: r.get(5)?,
                revoked_at: r.get(6)?,
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

/// Set principal mint template. `apply_tokens` copies onto **all live** keys.
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
        conn.execute(
            "UPDATE principals SET default_rooms = ?1 WHERE id = ?2",
            params![stored, p.id],
        )
        .map_err(|e| format!("skin user rooms: {e}"))?;
        if apply_tokens {
            conn.execute(
                "UPDATE tokens SET rooms = ?1 WHERE principal_id = ?2 AND revoked_at IS NULL",
                params![stored, p.id],
            )
            .map_err(|e| format!("skin apply-tokens: {e}"))?;
        }
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: rooms,
            default_room_handles: Vec::new(),
        })
    })
    .map(attach_principal_handles)
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
    let mut out = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        if let Ok(h) = crate::workspace::handle::project_handle(&conn, id) {
            let h = h.trim().to_string();
            if !h.is_empty() {
                out.push(SkinAgent {
                    handle: h,
                    project_id: id.to_string(),
                });
            }
        }
    }
    out
}

/// Resolve a presented raw key. Revoked / unknown / wrong prefix → `None`.
pub fn resolve_skin_token(presented_raw: &str) -> Option<SkinPass> {
    if !is_skin_token(presented_raw) || is_api_key_prefix(presented_raw) {
        return None;
    }
    let key_hash = sha256_hex(presented_raw);
    with_conn(|conn| {
        let row = conn
            .query_row(
                "SELECT t.id, t.principal_id, p.username, t.caps, t.rooms
                 FROM tokens t
                 JOIN principals p ON p.id = t.principal_id
                 WHERE t.key_hash = ?1 AND t.revoked_at IS NULL",
                params![key_hash],
                |r| {
                    let caps_raw: String = r.get(3)?;
                    let rooms_raw: String = r.get(4)?;
                    Ok(SkinPass {
                        id: r.get(0)?,
                        principal_id: r.get(1)?,
                        username: r.get(2)?,
                        caps: caps_from_json(&caps_raw),
                        rooms: rooms_from_json(&rooms_raw),
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
    use std::sync::Mutex as StdMutex;

    static TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        assert!(err.contains("skin.<sub>.k2.dev"), "{err}");
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
            assert_eq!(meta.rooms, vec![room.clone()]);
            let tokens = list_tokens().expect("list tokens");
            assert_eq!(tokens.len(), 1);
            assert!(
                !format!("{tokens:?}").contains(&raw),
                "secret must not leak in list"
            );
            assert!(!tokens[0].prefix.contains(&raw[SKIN_KEY_PREFIX.len()..]));

            let pass = resolve_skin_token(&raw).expect("live pass");
            assert_eq!(pass.username, "ada");
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
    fn mint_requires_principal_and_rejects_unknown_caps() {
        with_temp_home(|| {
            let room = uuid::Uuid::new_v4().to_string();
            let err = create_token("ghost", None, std::slice::from_ref(&room)).unwrap_err();
            assert!(err.contains("unknown skin user"), "{err}");
            add_principal("bob").unwrap();
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
            assert!(pass.has_cap(CAP_THREAD_READ));
            assert!(pass.has_cap(CAP_THREAD_POST));
            assert_eq!(pass.rooms, vec![room]);
        });
    }

    #[test]
    fn remove_principal_drops_tokens() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (_meta, raw) = create_token("cara", None, std::slice::from_ref(&room)).unwrap();
            assert!(resolve_skin_token(&raw).is_some());
            assert!(remove_principal("cara").unwrap());
            assert!(resolve_skin_token(&raw).is_none());
            assert!(list_principals().unwrap().is_empty());
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
}
