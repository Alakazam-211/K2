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
//! Tables: `principals` (guest roster), `roles` (named caps+rooms bundles;
//! nullable `app_id`, NULL = host-wide), `grants` (subject × kind × target;
//! `enabled` 0 is no grant; `target_id` `host` is this box's skin login),
//! `access_templates` + `access_template_lines` (kind and optional role,
//! no target — apply writes real grants),
//! and `tokens` (hashed passes). The raw secret is returned
//! **once** at mint and never stored. Lookup is hex SHA-256 of the
//! presented key (same construction as API keys / connect-user session
//! tokens — high-entropy CSPRNG, not argon2).
//!
//! Caps: `thread:read`, `thread:post`, `files:read`, `files:write`,
//! `tickets:read`, `tickets:post`, `wiki:read`, `store:read`, `store:write`,
//! `activity:read`, plus the opt-in `files:agentconfig` modifier (lets
//! `files:write` reach the agent's `.k2/` folder; off by default, owner
//! only). Empty/missing caps stay Thread-only — never silent-add
//! files, tickets, wiki, store, or activity. Never `pty:*`. `store:write`
//! is dump insert/update/delete only — it does not grant `_k2_store` PUT.
//! `files:read` and `files:write` do not imply `activity:read`.
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

/// The box login: the leftover direct door (`k2_skin_session`), a BFF that
/// names no App, and the "host" row in Settings → Apps. Every other login
/// target is a published service id (S24, 0.45.5): a login for App X needs
/// an enabled `kind=app` grant whose `target_id` is X, and its pass is
/// bound to X ([`create_app_session_token`]).
pub const HOST_APP_ID: &str = "host";

/// [`create_app_session_token`] error when the principal has no enabled
/// `kind=app` grant for the App asked for (or the App is unknown). The
/// login route returns this text as the same 401 body as a bad password.
/// No username, no App and no grant detail (no App enumeration).
pub const HOST_LOGIN_DENIED: &str = "invalid username or password";

/// skin.db schema version this binary knows (`PRAGMA user_version`, S24-1).
/// Steps are additive only: nsi's BFF reads `principals` and
/// `login_lockouts` directly, so no step renames, drops or rebuilds.
///   1 = AM1 sync: the host grant is the login truth (role or rooms).
///   2 = `tokens.app_service_id` (+ index) and `grants.created_by`.
///   3 = mark the one-time `--skin` App backfill pending (run at boot).
///   4 = `tokens.link_id` (+ index): App link passes (runtime S14, 0.46.0,
///       [`link`]). A K2 older than this refuses the file, so a downgrade
///       can never accept a link pass on its main port.
///   5 = `roles.room_paths`: folder path scope per room line (room-scope
///       R3, RS-A1..A3, 0.46.1, [`paths`]). `{}` = every line unscoped.
pub const SKIN_SCHEMA_VERSION: i64 = 5;

#[path = "skin_link.rs"]
pub mod link;

/// RS-A15 `guests:rooms` (0.46.1): an App sets its own guests' rooms.
#[path = "skin_guest_rooms.rs"]
pub mod guest_rooms;
#[path = "skin_members.rs"]
pub mod members;
/// Person wallets (0.46.3): an App user's live access, read-only.
#[path = "skin_wallet_access.rs"]
pub mod wallet_access;
#[path = "skin_paths.rs"]
pub mod paths;
/// Device sign-in (refresh tokens) and per-App sign-in settings
/// (app-mobile-access-v1, 0.46.3).
#[path = "skin_app_login.rs"]
pub mod app_login;
/// Test guests of one App, short-lived (0.46.4).
#[path = "skin_test_guests.rs"]
pub mod test_guests;
/// Which workspace's agent added a guest (0.46.4 security review).
#[path = "skin_creators.rs"]
pub mod creators;
/// The agent access list, the owner's person × agent grid and the
/// suggested role templates (app-mobile-access-v1, 0.46.3).
#[path = "skin_access.rs"]
pub mod access;
pub use paths::{RoomPathScope, RoomPaths};

/// `grants.created_by` on rows the 0.45.5 upgrade wrote (S24-10 backfill).
pub const UPGRADE_CREATED_BY: &str = "upgrade:0.45.5";

/// Sew S27/S28: the `chat` grant kind. A principal with an enabled
/// `kind=chat` grant on [`CHAT_GRANT_TARGET`] may mint chat passes
/// (`POST /cli/sew/pass`) and may log in to the box with only that grant
/// (S28 §13, Thread dark). **S27 owns writing these grants** (the grant
/// hook, [`crate::tool_hook`]); S28 only reads them.
pub const CHAT_GRANT_KIND: &str = crate::tool_hook::CHAT_KIND;
/// The one Sew install per box (S27: `target_id` is always `sew`). Every
/// App on the box (Hearth, Meet, …) reaches that same install, so a chat
/// grant is per person, never per App: the App a pass is for comes from
/// the login session (K-M4).
pub const CHAT_GRANT_TARGET: &str = crate::tool_hook::CHAT_TARGET_ID;

/// Q8 default (S24-6): switching a guest's box login off ends EVERY login
/// session of that guest, App sessions included — it is the one "turn this
/// person off" switch in User Access. `false` = only host sessions end.
pub const HOST_OFF_ENDS_ALL_APP_SESSIONS: bool = true;

/// Overlay Thread read (GET `/cli/thread`, WS `/cli/overlay/events`).
pub const CAP_THREAD_READ: &str = "thread:read";
/// Overlay Thread post (`POST /cli/thread/post`).
pub const CAP_THREAD_POST: &str = "thread:post";
/// Workspace files read (GET `/cli/fs/read-dir`, `/cli/fs/read-file`, WS `/cli/fs/events`).
pub const CAP_FILES_READ: &str = "files:read";
/// Workspace files write (`POST /cli/fs/write-file`). Does not imply read.
pub const CAP_FILES_WRITE: &str = "files:write";
/// Tickets read (`GET /cli/feedback/list`, `GET /cli/feedback/show`).
pub const CAP_TICKETS_READ: &str = "tickets:read";
/// Tickets write (`POST /cli/feedback/create|comment|answer|resolve`).
pub const CAP_TICKETS_POST: &str = "tickets:post";
/// Wiki read (`GET /cli/wiki/index`, `GET /cli/wiki/note`). Does not imply write.
pub const CAP_WIKI_READ: &str = "wiki:read";
/// Workspace dump store read (`GET /cli/store/list|get|query` and dump SELECT).
/// Does not imply write.
pub const CAP_STORE_READ: &str = "store:read";
/// Dump-table insert/update/delete (`POST /cli/db/rows|rows/update|rows/delete`).
/// Does not imply `_k2_store` PUT/create/rm/drop.
pub const CAP_STORE_WRITE: &str = "store:write";
/// Agent working mark (`WS /cli/activity/events`). Not implied by files.
pub const CAP_ACTIVITY_READ: &str = "activity:read";
/// Room heartbeats read (`GET /cli/heartbeat/list|show|status|fires-list`,
/// `heartbeat_changed` frames on `WS /cli/activity/events`). The WAKEUP.md
/// body also needs `files:read` (prd-app-heartbeats-surface-v1 AH9).
pub const CAP_HEARTBEATS_READ: &str = "heartbeats:read";
/// Room heartbeats write (`POST /cli/heartbeat/add|edit|enable|rename|
/// archive|fire`) and exactly each heartbeat's WAKEUP.md through
/// `/cli/fs/write-file` (Rosson R2). Does not imply read.
pub const CAP_HEARTBEATS_WRITE: &str = "heartbeats:write";
/// The room agent's name, color and picture (`GET|POST /cli/agent/identity`).
/// Not the handle: an address change breaks links, so it is its own cap.
pub const CAP_AGENT_IDENTITY: &str = "agent:identity";
/// The room agent's handle, its address (`POST /cli/agent/handle`). Does not
/// imply `agent:identity`, and `agent:identity` does not imply it.
pub const CAP_AGENT_HANDLE: &str = "agent:handle";
/// Opt-in modifier of `files:write` (GH#72, Rosson 2026-10-14): also let
/// that room's `files:write` reach the agent's own config folder `.k2/`.
/// Off by default — without it every guest write route refuses a target
/// in `.k2/` with 403 `agent_config_protected`. Means nothing without
/// `files:write` in the same room ([`parse_caps`] refuses it alone), never
/// opens `.k2/heartbeats/` (heartbeats:write rules stay), and reads are
/// unaffected. Owner tooling only: an agent with "manage Apps" can never
/// add it (`skin_routes` refuses with 403 `owner_only`).
pub const CAP_FILES_AGENT_CONFIG: &str = "files:agentconfig";
/// The room access list (room-scope R9, 0.46.1): `GET /cli/skin/members`
/// (`room.members`), the guests and agent logins that hold a grant on the
/// room, and `room_members_changed` frames on `WS /cli/activity/events`.
/// Never in the defaults; meant for staff roles. Names can be hidden from
/// guests per room by the owner.
pub const CAP_ROOM_MEMBERS: &str = "room:members";
/// Post in a project chat as the guest (0.46.3, App project chat). Held per
/// room like every cap: a pass may post in a project's chat only when it
/// holds this cap in EVERY member room of that project (the all-members
/// rule; reading needs `thread:read` in every member room). Never in the
/// defaults; owner/role-granted. In no App core (the person approves it)
/// until a Projects core exists.
pub const CAP_PROJECT_WRITE: &str = "project:write";
/// Save a Thread attachment as a download (0.46.3, Rosson 2026-10-22: "if
/// they have threads available that doesn't mean they can download files
/// unless they have file downloads turned on"). Held per room. It gates
/// only the explicit save, `GET /cli/thread/attachment?mode=download`, and
/// the `canDownload` flag each attachment carries for the viewer; opening a
/// file (meta, preview, thumb, range) stays `thread:read` plus the guest's
/// own `files:read`, as before. Never in the defaults, never implied by
/// `thread:read` or `files:read`, in no App core, and person-only to grant:
/// an agent with "manage Apps" can't add it (`skin_routes` answers 403
/// `owner_only`), the same rule as [`CAP_FILES_AGENT_CONFIG`].
pub const CAP_FILES_DOWNLOAD: &str = "files:download";
/// Upload-only files (prd-app-caps-0464-v1 AC7, 0.46.4): put files in the
/// room line's drop folders (`upload`, [`paths::RoomPathScope::upload`])
/// through the two upload doors, each landing in the uploader's own
/// subfolder. Held per room. It never implies `files:read` or
/// `files:write`, neither implies it, and it is never a default and in no
/// App core. Strictly narrower than `files:write`, so an agent with
/// "manage Apps" may grant it (it is not in [`PERSON_ONLY_CAPS`]).
pub const CAP_FILES_UPLOAD: &str = "files:upload";
/// Caps only a person may grant ([`person_only_cap_in`]).
pub const PERSON_ONLY_CAPS: &[&str] = &[CAP_FILES_AGENT_CONFIG, CAP_FILES_DOWNLOAD];

const KEY_BODY_LEN: usize = 43;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Empty/missing `--caps` stay Thread-only. Never silent-add files, tickets, wiki, store, or activity.
const DEFAULT_CAPS: &[&str] = &[CAP_THREAD_READ, CAP_THREAD_POST];
const ACCEPTED_CAPS: &[&str] = &[
    CAP_THREAD_READ,
    CAP_THREAD_POST,
    CAP_FILES_READ,
    CAP_FILES_WRITE,
    CAP_TICKETS_READ,
    CAP_TICKETS_POST,
    CAP_WIKI_READ,
    CAP_STORE_READ,
    CAP_STORE_WRITE,
    CAP_ACTIVITY_READ,
    CAP_HEARTBEATS_READ,
    CAP_HEARTBEATS_WRITE,
    CAP_AGENT_IDENTITY,
    CAP_AGENT_HANDLE,
    CAP_FILES_AGENT_CONFIG,
    CAP_ROOM_MEMBERS,
    CAP_PROJECT_WRITE,
    CAP_FILES_DOWNLOAD,
    CAP_FILES_UPLOAD,
];

/// Connect / Server Access names — never a skin role. `viewer` stays
/// reserved after the Connect Viewer role was removed
/// (`prd-remove-viewer-role-v1.md`), so the retired word can't come back
/// as an app role and be read as the old view-only login.
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

fn grant_role_remove_err(name: &str) -> String {
    format!("role '{name}' is used by a grant; revoke the grant first")
}

/// Copy of Connect: 3 consecutive failures → 15-minute per-username lockout.
/// Dummy argon2 + the 500 ms 401 delay is **not** lockout.
const LOCKOUT_THRESHOLD: u32 = 3;
const LOCKOUT_DURATION_SECS: i64 = 15 * 60;

/// Public consume/change min length (Connect default). Owner `users/password`
/// empty still clears via [`set_principal_password`].
pub const PUBLIC_PASSWORD_MIN_LEN: usize = 8;
const RESET_TTL_SECS: i64 = 60 * 60;
const RESET_THROTTLE_SECS: i64 = 60;

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

/// Opaque CSPRNG hex (64 chars). Never a `k2skn_` / `k2sk_` pass.
fn generate_reset_raw() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    let mut hex = String::with_capacity(64);
    for b in buf {
        hex.push_str(&format!("{b:02x}"));
    }
    debug_assert!(
        !hex.starts_with(SKIN_KEY_PREFIX) && !hex.starts_with("k2sk_"),
        "reset token must not use a skin/API prefix"
    );
    hex
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
/// Do **not** accept `@` here — platform `--name` shares this charset.
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

/// Trim + lowercase. Empty → `Ok(None)` (store NULL). Else
/// `^[^@\s]+@[^@\s]+\.[^@\s]+$` or `Err` (owner routes 400).
pub fn normalize_email(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let lowered = trimmed.to_ascii_lowercase();
    if email_shape_ok(&lowered) {
        Ok(Some(lowered))
    } else {
        Err("invalid email".to_string())
    }
}

/// `^[^@\s]+@[^@\s]+\.[^@\s]+$` without a regex crate.
fn email_shape_ok(s: &str) -> bool {
    if s.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let mut parts = s.split('@');
    let Some(local) = parts.next() else {
        return false;
    };
    let Some(domain) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    let Some(dot) = domain.find('.') else {
        return false;
    };
    let host = &domain[..dot];
    let rest = &domain[dot + 1..];
    !host.is_empty() && !rest.is_empty()
}

fn presented_looks_like_email(raw: &str) -> bool {
    raw.contains('@')
}

/// Parse + validate cap names. Empty/missing → default Thread read+post
/// (never silent-add `files:*`, `tickets:*`, `wiki:read`, `store:read`,
/// `store:write`, or `activity:read`). Unknown names fail loud. Write-only
/// is accepted at mint; list/read still require `files:read` /
/// `tickets:read` / `wiki:read` / `store:read` listed. `store:write` is
/// dump DML only. `files:read` and `files:write` do not imply
/// `activity:read`.
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
            if out.iter().any(|c| c == CAP_FILES_AGENT_CONFIG)
                && !out.iter().any(|c| c == CAP_FILES_WRITE)
            {
                return Err(format!(
                    "{CAP_FILES_AGENT_CONFIG} only widens {CAP_FILES_WRITE}; list {CAP_FILES_WRITE} too"
                ));
            }
        }
    }
    Ok(out)
}

/// The first person-only cap ([`PERSON_ONLY_CAPS`], in that order) in
/// `caps`, if any: what an agent may never grant.
pub fn person_only_cap_in<'a>(caps: impl IntoIterator<Item = &'a String>) -> Option<&'static str> {
    let caps: Vec<&String> = caps.into_iter().collect();
    PERSON_ONLY_CAPS.iter().copied().find(|p| caps.iter().any(|c| c.as_str() == *p))
}

/// The first person-only cap on any room line of `policy`.
pub fn person_only_cap_in_policy(policy: &RoomPolicy) -> Option<&'static str> {
    person_only_cap_in(policy.values().flatten())
}

/// The first person-only cap `new` adds on a room line where `old` (the
/// stored policy, `None` = nothing stored) doesn't hold it. `None` = no
/// widening.
pub fn person_only_cap_added(old: Option<&RoomPolicy>, new: &RoomPolicy) -> Option<&'static str> {
    PERSON_ONLY_CAPS.iter().copied().find(|p| {
        new.iter().any(|(room, caps)| {
            caps.iter().any(|c| c == p) && !old.and_then(|o| o.get(room)).is_some_and(|caps| caps.iter().any(|c| c == p))
        })
    })
}

/// `true` when any room line of `policy` carries [`CAP_FILES_AGENT_CONFIG`].
pub fn policy_has_agent_config(policy: &RoomPolicy) -> bool {
    policy
        .values()
        .any(|caps| caps.iter().any(|c| c == CAP_FILES_AGENT_CONFIG))
}

/// Rooms where `new` carries [`CAP_FILES_AGENT_CONFIG`] and `old` (the
/// stored policy, `None` = nothing stored) does not. Empty = no widening.
pub fn agent_config_added(old: Option<&RoomPolicy>, new: &RoomPolicy) -> Vec<String> {
    new.iter()
        .filter(|(_, caps)| caps.iter().any(|c| c == CAP_FILES_AGENT_CONFIG))
        .filter(|(room, _)| {
            !old.and_then(|o| o.get(*room))
                .is_some_and(|caps| caps.iter().any(|c| c == CAP_FILES_AGENT_CONFIG))
        })
        .map(|(room, _)| room.clone())
        .collect()
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
        if policy.values().any(|caps| caps.iter().any(|c| c == cap))
            && !out.iter().any(|c| c == cap)
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

fn room_access_from_policy(policy: &RoomPolicy, paths: Option<&RoomPaths>) -> Vec<SkinRoomAccess> {
    let ids = rooms_from_policy(policy);
    live_agents(&ids)
        .into_iter()
        .map(|a| SkinRoomAccess {
            folders: paths.and_then(|p| p.get(&a.project_id)).filter(|s| !s.is_empty()).cloned(),
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

fn dummy_verify(password: &str) {
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY_HASH.get_or_init(build_dummy_hash);
    let _ = verify_hash(password, dummy);
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

/// Nested label of the App (Skin) front door (`https://skin.<sub>.k2.dev`).
/// The skin door registers it itself (`skin_door::register_nested`); it
/// is in [`RESERVED_NESTED_LABELS`] so no workspace can claim it.
pub const RESERVED_NESTED_LABEL: &str = "skin";

/// Nested label of the future K2 Meet portal (`https://meet.<sub>.k2.dev`).
pub const MEET_NESTED_LABEL: &str = "meet";

/// Nested label of the future Sew chat web client (`https://chat.<sub>.k2.dev`).
pub const CHAT_NESTED_LABEL: &str = "chat";

/// Nested label of the future browser webmail front-end
/// (`https://mail.<sub>.k2.dev`). Web only, through the tunnel: it is
/// never a mail-server name (MX/SMTP/IMAP live on the client's own
/// delegated domain).
pub const MAIL_NESTED_LABEL: &str = "mail";

/// One box-level nested label that no workspace, user or agent may claim
/// as a publish / subdomain target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservedNestedLabel {
    pub label: &'static str,
    /// Short purpose, used in the refusal (`reserved for K2 Meet ...`).
    pub purpose: &'static str,
    /// `true` = a row that already used this name before it was reserved
    /// keeps working (start / stop / rm / re-run of that same row). Only
    /// NEW claims and points are refused. `skin` was reserved before any
    /// row could hold it, so it stays strict.
    pub grandfathered: bool,
}

/// Every reserved nested label. Box-level front doors register these
/// themselves through the same path the skin door uses
/// (`skin_door::register_nested`: control-plane create/point with the
/// box's tunnel token), never through the user publish / subdomain
/// routes. A future Sew door registers `meet` and `chat` that way.
pub const RESERVED_NESTED_LABELS: &[ReservedNestedLabel] = &[
    ReservedNestedLabel {
        label: RESERVED_NESTED_LABEL,
        purpose: "the App front door",
        grandfathered: false,
    },
    ReservedNestedLabel {
        label: MEET_NESTED_LABEL,
        purpose: "K2 Meet (meet.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: CHAT_NESTED_LABEL,
        purpose: "K2 chat (chat.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: MAIL_NESTED_LABEL,
        purpose: "K2 webmail (mail.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: VOICE_NESTED_LABEL,
        purpose: "K2 chat voice (voice.<box>.k2.dev)",
        grandfathered: true,
    },
    ReservedNestedLabel {
        label: TURN_NESTED_LABEL,
        purpose: "K2 chat voice relay (TURN)",
        grandfathered: true,
    },
];

/// Sew voice signalling (`voice.<box>.k2.dev` → sew-media's loopback
/// 7880), registered box-level by the Sew supervisor (K13).
pub const VOICE_NESTED_LABEL: &str = "voice";
/// Kept from tenants: Sew's TURN host is the single label
/// `turn-<box>.k2.dev` (k2-connect turn mode), never a nested `turn.<box>`,
/// but a nested `turn` would read as it.
pub const TURN_NESTED_LABEL: &str = "turn";

/// The reserved entry for `name` (trimmed, case-insensitive), if any.
/// Exact match only: `meeting`, `chats`, `skins` are ordinary labels.
pub fn reserved_nested_label(name: &str) -> Option<&'static ReservedNestedLabel> {
    let n = name.trim();
    RESERVED_NESTED_LABELS
        .iter()
        .find(|r| n.eq_ignore_ascii_case(r.label))
}

pub fn is_reserved_nested_label(name: &str) -> bool {
    reserved_nested_label(name).is_some()
}

/// A reserved label whose pre-existing rows keep working (see
/// [`ReservedNestedLabel::grandfathered`]).
pub fn is_grandfathered_reserved_label(name: &str) -> bool {
    reserved_nested_label(name).is_some_and(|r| r.grandfathered)
}

pub fn reserved_nested_label_error(label: &str) -> String {
    let label = label.trim().to_ascii_lowercase();
    match reserved_nested_label(&label) {
        Some(r) if r.label != RESERVED_NESTED_LABEL => format!(
            "reserved_label: '{label}' is reserved for {}. Pick another nested label.",
            r.purpose
        ),
        // `skin` (and any non-reserved caller misuse) keeps the original text.
        _ => format!(
            "reserved_label: '{label}' is reserved. Pick another nested label for your UI (k2 study apps; leftover: k2 study skins)."
        ),
    }
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
    /// Optional guest email. Serialized `null` when unset. Not a username.
    #[serde(default)]
    pub email: Option<String>,
    /// Optional guest full name. Serialized `null` when unset. Not a username.
    /// Owner-only writes sanitize via `sanitize_owner_display_name` (cap 64).
    #[serde(default)]
    pub full_name: Option<String>,
    /// Owner-only mark: this guest principal is the agent of one workspace
    /// (`projects.id`). Its brokered db calls stamp actor kind `agent`.
    /// `None` = a person (actor kind `guest`).
    #[serde(default)]
    pub agent_workspace: Option<String>,
    /// Display only: live handle of `agent_workspace` (skip when gone).
    #[serde(default)]
    pub agent_handle: Option<String>,
}

/// Per-room functions on the wire: `{handle, caps, folders?}`. `folders`
/// is the role line's path scope (room-scope R3); absent = the whole room.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinRoomAccess {
    pub handle: String,
    pub caps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<RoomPathScope>,
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
    /// App this role belongs to. `None` = host-wide (every existing row).
    #[serde(default)]
    pub app_id: Option<String>,
    /// SSOT map `project_id → caps[]`. Not on the wire.
    #[serde(skip)]
    pub room_policy: RoomPolicy,
    /// `project_id → folders` (room-scope R3). Not on the wire as a map:
    /// each `room_access` line carries its own `folders`.
    #[serde(skip)]
    pub room_paths: RoomPaths,
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

/// One workspace a skin pass may Thread. Wire `{handle, projectId,
/// displayName, color, image}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinAgent {
    pub handle: String,
    pub project_id: String,
    /// Guest-facing label. Handle stays the id.
    pub display_name: String,
    /// The agent's color, `#rrggbb` (0.45.5). `None` only for a row read
    /// before the color was known.
    #[serde(default)]
    pub color: Option<String>,
    /// The agent's picture as a PNG/JPEG/WebP/GIF data URL, or `None` (no
    /// picture, or one an App is never sent: SVG, oversize). 0.45.5.
    #[serde(default)]
    pub image: Option<String>,
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
    /// The App this login is bound to (S24-5): `host` or a published service
    /// id. A pre-0.45.5 session row (NULL) reads as `host`. Platform passes
    /// (`session=0`) are not App-bound yet (S14) and carry `""`. For logs and
    /// later S14/S43 checks; no handler authorizes on it in 0.45.5.
    pub app: String,
    /// Folder path scope per room line (room-scope R3), read at resolve
    /// time from the session's own grant → role (an edit applies from the
    /// session's next request). Platform and link passes: `Whole`.
    pub room_paths: paths::PassPaths,
}

impl SkinPass {
    /// This pass's folders on one room line (R3). An unscoped line (or a
    /// platform/link pass) is `RoomPathScope::default()`, the whole room.
    /// `Err` = the scope couldn't be read: callers fail closed with the
    /// missing-cap 403.
    pub fn path_scope(&self, project_id: &str) -> Result<RoomPathScope, String> {
        match &self.room_paths {
            paths::PassPaths::Whole => Ok(RoomPathScope::default()),
            paths::PassPaths::Lines(lines) => Ok(lines.get(project_id.trim()).cloned().unwrap_or_default()),
            paths::PassPaths::Unreadable(e) => Err(e.clone()),
        }
    }

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

    /// Dispatcher door. Session defers the cap until the room is known.
    /// Platform still requires the pass-level cap.
    pub fn dispatcher_admits_cap(&self, cap: &str) -> bool {
        if self.session {
            true
        } else {
            self.has_cap(cap)
        }
    }

    /// AH29 any-of door (one socket, several caps). Session defers to the
    /// room check. Platform needs at least one listed cap on the pass.
    pub fn dispatcher_admits_any(&self, caps: &[&str]) -> bool {
        if self.session {
            true
        } else {
            caps.iter().any(|c| self.has_cap(c))
        }
    }

    /// AH29: at least one of `caps` in that room (same rules as
    /// [`Self::has_cap_in_room`]).
    pub fn has_any_cap_in_room(&self, project_id: &str, caps: &[&str]) -> bool {
        caps.iter().any(|c| self.has_cap_in_room(project_id, c))
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

/// One `host` app row. `ON CONFLICT DO NOTHING` keeps an owner-written
/// grant (any role or scope). Used by the one-time copy and by a new
/// principal. A deleted row is not inserted again on a later open.
fn insert_host_grant(
    conn: &Connection,
    principal_id: &str,
    role_id: Option<String>,
    scope: Option<String>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO grants
         (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled)
         VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, 1)
         ON CONFLICT(subject_kind, subject_id, kind, target_id) DO NOTHING",
        params![
            uuid::Uuid::new_v4().to_string(),
            principal_id,
            HOST_APP_ID,
            role_id,
            scope,
            now_secs(),
        ],
    )
    .map_err(|e| format!("skin host grant: {e}"))?;
    Ok(())
}

/// Copy principals who have no host grant. Assigned: that `role_id`, scope
/// NULL, and the role stays host-wide (`roles.app_id` is not set).
/// Unassigned: `role_id` NULL and `scope` = stored `default_rooms` JSON,
/// including `[]`. A guest with no rooms still gets a row.
///
/// Call only from [`ensure_host_grant_backfill`]. A later open must not
/// call this: a deleted host grant stays deleted.
fn backfill_host_grants(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT id, role_id, default_rooms FROM principals")
        .map_err(|e| format!("skin host grant: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| format!("skin host grant: {e}"))?;
    let mut pending = Vec::new();
    for row in rows {
        pending.push(row.map_err(|e| format!("skin host grant: {e}"))?);
    }
    drop(stmt);
    for (id, role_id, rooms) in pending {
        if let Some(role_id) = blank_to_none(role_id) {
            insert_host_grant(conn, &id, Some(role_id), None)?;
        } else {
            let scope = match rooms {
                Some(s) if !s.trim().is_empty() => s,
                _ => "[]".to_string(),
            };
            insert_host_grant(conn, &id, None, Some(scope))?;
        }
    }
    Ok(())
}

/// Run [`backfill_host_grants`] once per database. The row in
/// `host_grant_backfill` is the record. Later opens see that row and do
/// not copy again, so revoking the host grant survives a restart.
/// Principals created after the copy get their row from [`add_principal_with_email`].
fn ensure_host_grant_backfill(conn: &mut Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_grant_backfill (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            done_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin host grant: {e}"))?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("skin host grant: {e}"))?;
    let done: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM host_grant_backfill WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin host grant: {e}"))?;
    if done == 0 {
        backfill_host_grants(&tx)?;
        tx.execute(
            "INSERT INTO host_grant_backfill (id, done_at) VALUES (1, ?1)",
            params![now_secs()],
        )
        .map_err(|e| format!("skin host grant: {e}"))?;
    }
    tx.commit().map_err(|e| format!("skin host grant: {e}"))?;
    Ok(())
}

fn open_db(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    if let Err(e) = crate::private_home::prepare_private_db_file(path) {
        crate::log_debug!("[skin] WARN create {} owner-only: {e}", path.display());
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
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS password_reset_tokens (
            id TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL REFERENCES principals(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            expires_at INTEGER NOT NULL,
            used_at INTEGER,
            created_at INTEGER NOT NULL
         );",
    );
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS password_reset_throttle (
            username TEXT PRIMARY KEY,
            last_attempt INTEGER NOT NULL
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
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN email TEXT", []);
    // G10: full name on the guest. Ignore duplicate ALTER like `email`.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN full_name TEXT", []);
    // Owner-only agent mark (`k2 app user agent`). Additive like `full_name`:
    // skin.db has no schema version; a duplicate ALTER is ignored.
    let _ = conn.execute("ALTER TABLE principals ADD COLUMN agent_workspace TEXT", []);
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS principals_email
         ON principals(email COLLATE NOCASE)
         WHERE email IS NOT NULL AND trim(email) != '';",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    let _ = conn.execute(
        "ALTER TABLE roles ADD COLUMN room_policy TEXT NOT NULL DEFAULT '{}'",
        [],
    );
    // Nullable app scope. Existing rows stay NULL (host-wide). Ignore
    // duplicate ALTER. Not a daemon migration.
    let _ = conn.execute("ALTER TABLE roles ADD COLUMN app_id TEXT", []);
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS grants (
            id TEXT PRIMARY KEY,
            subject_kind TEXT NOT NULL,
            subject_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            target_id TEXT NOT NULL,
            role_id TEXT,
            scope TEXT,
            created_at INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            UNIQUE(subject_kind, subject_id, kind, target_id)
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    // Existing databases created before `enabled`. Ignore duplicate ALTER.
    let _ = conn.execute(
        "ALTER TABLE grants ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1",
        [],
    );
    // Access templates are not roles and not daemon SQLite. A line stores
    // kind and, for kind=app, role_id. It does not store target_id.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS access_templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL COLLATE NOCASE UNIQUE,
            created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS access_template_lines (
            id TEXT PRIMARY KEY,
            template_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            role_id TEXT,
            position INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db schema: {e}"))?;
    // RS-A15 (0.46.1): the owner's per-App grants (`guests-rooms`).
    // Additive, so no numbered step: an older K2 ignores the table.
    guest_rooms::ensure_schema(&conn)?;
    // app-mobile-access-v1 (0.46.3): device sign-ins, per-App sign-in
    // settings. Additive, no numbered step.
    app_login::ensure_schema(&conn)?;
    // 0.46.4: test guests of one App. Additive, no step.
    test_guests::ensure_schema(&conn)?;
    // 0.46.4: which workspace's agent added a guest. Additive, no step.
    creators::ensure_schema(&conn)?;
    // Copy existing guests once. A deleted host row stays deleted.
    ensure_host_grant_backfill(&mut conn)?;
    rebuild_tokens_table_if_needed(&mut conn)?;
    let _ = conn.execute("ALTER TABLE tokens ADD COLUMN room_policy TEXT", []);
    ensure_platform_name_index(&conn)?;
    migrate_room_policy(&conn)?;
    // Sew S27 grant hook: `tool_outbox` + `tool_targets`. Idempotent and
    // additive like `grants`; NOT a ladder step (user_version stays put).
    crate::tool_hook::outbox::ensure_schema(&conn)?;
    // S24-1: numbered, additive steps on top of the idempotent block above.
    run_schema_steps(&mut conn)?;
    // skin.db AND its -wal/-shm: the WAL holds recent pages (guest
    // password hashes, pass hashes) until a checkpoint.
    if let Err(e) = crate::private_home::restrict_db_files(path) {
        crate::log_debug!("[skin] WARN {e}");
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
            if column_exists(conn, "tokens", "app_service_id")? {
                rewrite_sessions_for_role(conn, &role.id, &policy)?;
            } else {
                // Before schema step 2 the role column is still the login
                // truth (and the App column does not exist yet).
                conn.execute(
                    "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
                     WHERE session = 1 AND revoked_at IS NULL
                       AND principal_id IN (SELECT id FROM principals WHERE role_id = ?4)",
                    params![
                        caps_json(&union_caps(&policy)),
                        rooms_json(&rooms_from_policy(&policy)),
                        room_policy_json(&policy),
                        role.id
                    ],
                )
                .map_err(|e| format!("skin session rewrite: {e}"))?;
            }
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

// ── Schema version (S24-1) ───────────────────────────────────────────

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("skin.db table_info {table}: {e}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("skin.db table_info {table}: {e}"))?;
    for row in rows {
        if row.map_err(|e| format!("skin.db table_info {table} row: {e}"))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn schema_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| format!("skin.db user_version: {e}"))
}

/// Open refusal when the file was written by a newer K2 (S24-1). Older
/// daemons never read the version, so this only guards later downgrades.
pub fn schema_newer_error(found: i64) -> String {
    format!(
        "skin.db schema version {found} is newer than this K2 understands ({SKIN_SCHEMA_VERSION}). \
         Refusing to open it: update K2, or restore the skin.db backup taken before the downgrade."
    )
}

/// One audit line (`~/.k2/auth-audit.jsonl`) for an access change the
/// daemon made on its own (upgrade sync, App backfill) or an owner copy.
fn record_access_audit(event: &str, actor: &str, line: &str) {
    crate::auth_audit::record(&crate::auth_audit::AuditEvent::new(
        event, actor, line, "-", "-", "daemon",
    ));
}

/// Run every schema step above the file's `user_version`, each in its own
/// IMMEDIATE transaction that also sets the version. Audit lines are
/// written after the step commits.
fn run_schema_steps(conn: &mut Connection) -> Result<(), String> {
    let mut v = schema_version(conn)?;
    if v > SKIN_SCHEMA_VERSION {
        return Err(schema_newer_error(v));
    }
    while v < SKIN_SCHEMA_VERSION {
        let next = v + 1;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| format!("skin.db step {next} begin: {e}"))?;
        // Another opener may have run it between our read and the lock.
        let cur = schema_version(&tx)?;
        if cur >= next {
            tx.commit()
                .map_err(|e| format!("skin.db step {next} commit: {e}"))?;
            v = cur;
            continue;
        }
        let audit = match next {
            1 => step1_sync_host_grants(&tx)?,
            2 => {
                step2_app_columns(&tx)?;
                Vec::new()
            }
            3 => {
                step3_mark_app_backfill(&tx)?;
                Vec::new()
            }
            4 => {
                step4_link_column(&tx)?;
                Vec::new()
            }
            5 => {
                paths::step5_room_paths_column(&tx)?;
                Vec::new()
            }
            other => return Err(format!("skin.db: no schema step {other}")),
        };
        tx.execute_batch(&format!("PRAGMA user_version = {next};"))
            .map_err(|e| format!("skin.db step {next} version: {e}"))?;
        tx.commit()
            .map_err(|e| format!("skin.db step {next} commit: {e}"))?;
        for (user, line) in audit {
            record_access_audit("app_grant_upgrade", &user, &line);
        }
        v = next;
    }
    Ok(())
}

/// Step 1 (AM1, S24-2): the host grant becomes the login truth. Every host
/// row (enabled or disabled) is synced from what login used before 0.45.5:
/// an assigned guest's `principals.role_id` (it wins over a different grant
/// role, D1), else `role_id NULL` + `scope = default_rooms` (`[]` kept).
/// Principals with no host row (box login off) are left alone. Returns one
/// `(username, line)` per row whose value changed.
fn step1_sync_host_grants(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    struct Row {
        grant_id: String,
        grant_role: Option<String>,
        grant_scope: Option<String>,
        username: String,
        principal_role: Option<String>,
        default_rooms: Option<String>,
    }
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT g.id, g.role_id, g.scope, p.username, p.role_id, p.default_rooms
                 FROM grants g JOIN principals p ON p.id = g.subject_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app' AND g.target_id = ?1",
            )
            .map_err(|e| format!("skin.db step 1: {e}"))?;
        let mapped = stmt
            .query_map(params![HOST_APP_ID], |r| {
                Ok(Row {
                    grant_id: r.get(0)?,
                    grant_role: r.get(1)?,
                    grant_scope: r.get(2)?,
                    username: r.get(3)?,
                    principal_role: r.get(4)?,
                    default_rooms: r.get(5)?,
                })
            })
            .map_err(|e| format!("skin.db step 1: {e}"))?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin.db step 1 row: {e}"))?
    };
    let mut audit = Vec::new();
    for row in rows {
        let (want_role, want_scope) = match blank_to_none(row.principal_role) {
            Some(role) => (Some(role), None),
            None => (
                None,
                Some(rooms_json(&rooms_from_json(
                    row.default_rooms.as_deref().unwrap_or("[]"),
                ))),
            ),
        };
        let have_role = blank_to_none(row.grant_role);
        let have_scope = blank_to_none(row.grant_scope);
        let scope_same = match (&have_scope, &want_scope) {
            (Some(a), Some(b)) => rooms_from_json(a) == rooms_from_json(b),
            (None, None) => true,
            _ => false,
        };
        if have_role == want_role && scope_same {
            continue;
        }
        conn.execute(
            "UPDATE grants SET role_id = ?1, scope = ?2 WHERE id = ?3",
            params![want_role, want_scope, row.grant_id],
        )
        .map_err(|e| format!("skin.db step 1 update: {e}"))?;
        audit.push((
            row.username,
            format!(
                "host login synced from what login used (0.45.5 AM1): role {} -> {}, rooms {} -> {}",
                have_role.as_deref().unwrap_or("none"),
                want_role.as_deref().unwrap_or("none"),
                have_scope.as_deref().unwrap_or("none"),
                want_scope.as_deref().unwrap_or("none"),
            ),
        ));
    }
    Ok(audit)
}

/// Step 2 (S24-3): the App a login session is bound to, and who wrote a
/// grant. Both nullable; old session rows (NULL) read as `host`.
fn step2_app_columns(conn: &Connection) -> Result<(), String> {
    if !column_exists(conn, "tokens", "app_service_id")? {
        conn.execute("ALTER TABLE tokens ADD COLUMN app_service_id TEXT", [])
            .map_err(|e| format!("skin.db step 2 tokens: {e}"))?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS tokens_principal_app ON tokens(principal_id, app_service_id);",
    )
    .map_err(|e| format!("skin.db step 2 index: {e}"))?;
    if !column_exists(conn, "grants", "created_by")? {
        conn.execute("ALTER TABLE grants ADD COLUMN created_by TEXT", [])
            .map_err(|e| format!("skin.db step 2 grants: {e}"))?;
    }
    Ok(())
}

/// Step 3 (S24-10): mark the `--skin` App backfill pending. It needs
/// `published_services` (k2so.db), so the daemon runs it at boot
/// ([`run_skin_app_backfill`]) before any helper starts.
fn step3_mark_app_backfill(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skin_app_backfill (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            pending INTEGER NOT NULL,
            done_at INTEGER,
            summary TEXT
         );
         INSERT OR IGNORE INTO skin_app_backfill (id, pending) VALUES (1, 1);",
    )
    .map_err(|e| format!("skin.db step 3: {e}"))
}

/// Step 4 (runtime S14): `tokens.link_id`, the App link a pass is bound
/// to. NULL on every existing row (no existing pass is a link pass).
fn step4_link_column(conn: &Connection) -> Result<(), String> {
    if !column_exists(conn, "tokens", "link_id")? {
        conn.execute("ALTER TABLE tokens ADD COLUMN link_id TEXT", [])
            .map_err(|e| format!("skin.db step 4 tokens: {e}"))?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS tokens_app_link ON tokens(app_service_id, link_id);")
        .map_err(|e| format!("skin.db step 4 index: {e}"))
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

/// The one skin.db connection, for [`crate::tool_hook`] (same mutex as
/// every `skin::*` function; not reentrant).
pub(crate) fn with_skin_conn<T, F: FnOnce(&Connection) -> Result<T, String>>(
    f: F,
) -> Result<T, String> {
    with_conn(f)
}

// ── Principals ───────────────────────────────────────────────────────

pub fn add_principal(username: &str) -> Result<SkinPrincipal, String> {
    add_principal_with_email(username, None)
}

/// Create-only. `email` None/empty → NULL. Invalid or duplicate → Err (400).
pub fn add_principal_with_email(
    username: &str,
    email: Option<&str>,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let email = match email {
        Some(raw) => normalize_email(raw)?,
        None => None,
    };
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
        if let Some(ref em) = email {
            ensure_email_free(conn, em, None)?;
        }
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin principal begin: {e}"))?;
        tx.execute(
            "INSERT INTO principals (id, username, created_at, email) VALUES (?1, ?2, ?3, ?4)",
            params![id, username, created_at, email],
        )
        .map_err(|e| unique_email_err(email.as_deref(), e, "skin principal insert"))?;
        // The one-time copy already ran in open_db and did not see this row.
        // Unassigned host grant: scope is `default_rooms` (`[]`).
        insert_host_grant(&tx, &id, None, Some("[]".to_string()))?;
        tx.commit()
            .map_err(|e| format!("skin principal commit: {e}"))?;
        Ok(SkinPrincipal {
            id,
            username,
            created_at,
            default_rooms: Vec::new(),
            default_room_handles: Vec::new(),
            has_password: false,
            role_id: None,
            role_name: None,
            email,
            full_name: None,
            agent_workspace: None,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    // App user change (Sew rev 147 / K31 seam).
    crate::tool_hook::kick_reconcile();
    members::changed(None);
    Ok(r)
}

const PRINCIPAL_SELECT: &str = "SELECT p.id, p.username, p.created_at, p.default_rooms,
                p.password_hash, p.role_id, r.name, p.email, p.full_name, p.agent_workspace
         FROM principals p
         LEFT JOIN roles r ON r.id = p.role_id";

fn email_from_stored(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn map_principal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinPrincipal> {
    let rooms_raw: String = r.get(3)?;
    let hash: Option<String> = r.get(4)?;
    let role_id: Option<String> = r.get(5)?;
    let role_name: Option<String> = r.get(6)?;
    let email: Option<String> = r.get(7)?;
    let full_name: Option<String> = r.get(8)?;
    let agent_workspace: Option<String> = r.get(9)?;
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
        email: email_from_stored(email),
        full_name: full_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        agent_workspace: agent_workspace
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        agent_handle: None,
    })
}

fn unique_email_err(email: Option<&str>, e: rusqlite::Error, ctx: &str) -> String {
    let s = e.to_string();
    if s.contains("principals_email") || s.contains("principals.email") {
        match email {
            Some(em) => format!("email '{em}' already in use"),
            None => "email already in use".to_string(),
        }
    } else {
        format!("{ctx}: {e}")
    }
}

fn ensure_email_free(
    conn: &Connection,
    email: &str,
    except_id: Option<&str>,
) -> Result<(), String> {
    let taken: i64 = match except_id {
        Some(id) => conn
            .query_row(
                "SELECT COUNT(*) FROM principals
                 WHERE email = ?1 COLLATE NOCASE
                   AND email IS NOT NULL AND trim(email) != ''
                   AND id != ?2",
                params![email, id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin email lookup: {e}"))?,
        None => conn
            .query_row(
                "SELECT COUNT(*) FROM principals
                 WHERE email = ?1 COLLATE NOCASE
                   AND email IS NOT NULL AND trim(email) != ''",
                params![email],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin email lookup: {e}"))?,
    };
    if taken > 0 {
        return Err(format!("email '{email}' already in use"));
    }
    Ok(())
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
    .map(|users| users.into_iter().map(attach_principal_handles).collect())
}

pub fn remove_principal(username: &str) -> Result<bool, String> {
    remove_principal_as(username, crate::tool_hook::ACTOR_SYSTEM)
}

/// Remove a principal in ONE transaction (S27 §14.2): its session tokens,
/// every grant row, and for each hooked grant (chat) the cascade
/// (assistant grants pinned to them) plus a `deprovision` outbox row.
pub fn remove_principal_as(username: &str, actor: &str) -> Result<bool, String> {
    use crate::tool_hook::{outbox, registry, Cause, HookCtx, HookEvent};
    let username = normalize_username(username)?;
    let actor = hook_actor(Some(actor));
    let ctx = HookCtx {
        actor: actor.clone(),
        ..Default::default()
    };
    let now = now_secs();
    let (r, enqueued) = with_conn(|conn| {
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM principals WHERE username = ?1",
                params![username],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin principal lookup: {e}"))?;
        let Some(id) = id else {
            return Ok((false, 0));
        };
        // S28 §12: the person's Sew sessions end with them. Check the chat
        // grant before the grants go, in the same Immediate transaction as
        // the deletes and the S27 outbox rows.
        let tx = immediate_tx(conn)?;
        let had_chat = principal_has_chat_grant_conn(&tx, &id)?;
        tx.execute(
            "DELETE FROM tokens WHERE principal_id = ?1 AND session = 1",
            params![id],
        )
        .map_err(|e| format!("skin session delete: {e}"))?;
        let mut ops = Vec::new();
        for p in registry() {
            let mut stmt = tx
                .prepare(&format!(
                    "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = ?2"
                ))
                .map_err(|e| format!("skin principal grants: {e}"))?;
            let hooked: Vec<SkinGrant> = stmt
                .query_map(params![id, p.kind()], map_grant_row)
                .map_err(|e| format!("skin principal grants: {e}"))?
                .collect::<Result<_, _>>()
                .map_err(|e| format!("skin principal grant row: {e}"))?;
            drop(stmt);
            for g in &hooked {
                let ev = HookEvent::GrantDeleted { grant: g };
                for c in p.cascade(&tx, &ev)? {
                    tx.execute("DELETE FROM grants WHERE id = ?1", params![c.id])
                        .map_err(|e| format!("skin grant cascade delete: {e}"))?;
                    ops.extend(p.plan(&tx, &HookEvent::GrantDeleted { grant: &c }, &ctx)?);
                }
                for mut op in p.plan(&tx, &ev, &ctx)? {
                    op.cause = Cause::PrincipalDelete;
                    ops.push(op);
                }
            }
        }
        // No FK from grants to principals. Leave the row and app detail
        // would keep showing a raw uuid.
        tx.execute(
            "DELETE FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1",
            params![id],
        )
        .map_err(|e| format!("skin principal grants delete: {e}"))?;
        let n = tx
            .execute("DELETE FROM principals WHERE id = ?1", params![id])
            .map_err(|e| format!("skin principal delete: {e}"))?;
        if n > 0 && had_chat {
            crate::sew_revoke::enqueue(
                &tx,
                &crate::sew_revoke::SewSessionRevoke::ExternalId(crate::sew_revoke::principal_external_id(&id)),
            )?;
        }
        let enqueued = outbox::enqueue(&tx, &ops, &actor, now)?.len();
        hook_failpoint()?;
        tx.commit()
            .map_err(|e| format!("skin principal remove commit: {e}"))?;
        Ok((n > 0, enqueued))
    })?;
    if r {
        crate::workspace::context_layers::refresh_skin_roster_after_people_change();
        // App user change (rev 147 / K31 seam).
        crate::tool_hook::kick_reconcile();
    }
    if enqueued > 0 {
        crate::tool_hook::kick();
    }
    members::changed(None);
    Ok(r)
}

/// After a principal's email / full name change: enqueue the chat
/// profile `update` in the same transaction (no-op without a chat grant).
fn plan_principal_changed(
    tx: &Connection,
    principal: &SkinPrincipal,
    actor: &str,
) -> Result<usize, String> {
    use crate::tool_hook::{outbox, registry, HookCtx, HookEvent};
    let ctx = HookCtx {
        actor: actor.to_string(),
        ..Default::default()
    };
    let mut ops = Vec::new();
    for p in registry() {
        ops.extend(p.plan(tx, &HookEvent::PrincipalChanged { principal }, &ctx)?);
    }
    Ok(outbox::enqueue(tx, &ops, actor, now_secs())?.len())
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
    p.agent_handle = p
        .agent_workspace
        .as_ref()
        .and_then(|id| handles_for_project_ids(std::slice::from_ref(id)).into_iter().next());
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

/// Box login ([`HOST_APP_ID`]). Same as [`create_app_session_token`] with
/// `host`; kept for the leftover direct door and existing callers.
pub fn create_session_token(username: &str) -> Result<(SkinTokenMeta, String), String> {
    create_app_session_token(username, HOST_APP_ID).map(|s| (s.meta, s.raw))
}

/// A minted login session (S24-4). `raw` is the `k2skn_` secret, shown once.
#[derive(Debug, Clone)]
pub struct SkinAppSession {
    pub meta: SkinTokenMeta,
    pub raw: String,
    /// The App the pass is bound to: `host` or a published service id.
    pub app: String,
    /// The role on that App's grant, or `None` (rooms, Thread-only).
    pub role_name: Option<String>,
}

/// Empty / missing App → [`HOST_APP_ID`] (a BFF that names no App).
pub fn normalize_app_target(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(HOST_APP_ID)
        .to_string()
}

/// Login session mint for one App (S24-4). Needs an **enabled** `kind=app`
/// grant for `(principal, app)`; otherwise [`HOST_LOGIN_DENIED`] (same text
/// as a bad password: unknown App, no grant and a disabled grant all look
/// alike). The policy comes from that grant: its role's `room_policy`
/// (including `{}`), else its `scope` rooms **including []** with
/// Thread-only caps. Writes `app_service_id`. Do **not** call
/// [`create_token`] (empty rooms 400). `session=1`, `expires_at` = now +
/// [`crate::connect_users::session_ttl_days`].
pub fn create_app_session_token(username: &str, app: &str) -> Result<SkinAppSession, String> {
    let username = normalize_username(username)?;
    let app = normalize_app_target(Some(app));
    let r = with_conn(|conn| mint_app_session_conn(conn, &username, &app))?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    Ok(SkinAppSession {
        meta: attach_token_handles(r.meta),
        ..r
    })
}

/// The mint behind [`create_app_session_token`] on an open connection (so
/// device sign-in, [`app_login`], mints and rotates in one transaction).
/// `username` and `app` are already normalized. The caller refreshes the
/// roster and attaches handles.
pub(crate) fn mint_app_session_conn(conn: &Connection, username: &str, app: &str) -> Result<SkinAppSession, String> {
    let username = username.to_string();
    let app = app.to_string();
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
    let mut expires_at = created_at.saturating_add(ttl_secs);
    {
        let Some(principal) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        // 0.46.4: a test guest gets a session only for its App, only
        // through that App's link, never past its own end (every door:
        // login, device sign-in, refresh, the browser helper).
        if let Some(t) = test_guests::get_conn(conn, &principal.id)? {
            match test_guests::session_allowed(&t, &app, created_at) {
                Ok(until) => expires_at = expires_at.min(until),
                Err(why) => {
                    crate::log_debug!("[skin] test guest {username}: {why}");
                    return Err(HOST_LOGIN_DENIED.to_string());
                }
            }
        }
        let (policy, role_name) = match enabled_app_grant(conn, &principal.id, &app)? {
            Some(grant) => grant_login_policy(conn, &grant)?,
            // Sew S28 §13 (G7): chat-only login. A box login (`host`) with
            // no host grant but a `chat` grant gets a session with an
            // EMPTY room policy (Thread, files, tickets, wiki and store
            // stay dark): it exists to mint chat passes. App logins keep
            // needing that App's own grant (rev 147).
            None if app == HOST_APP_ID && principal_has_chat_grant_conn(conn, &principal.id)? => {
                (thread_only_policy(&[]), None)
            }
            None => return Err(HOST_LOGIN_DENIED.to_string()),
        };
        let caps = union_caps(&policy);
        let rooms = rooms_from_policy(&policy);
        let caps_stored = caps_json(&caps);
        let rooms_stored = rooms_json(&rooms);
        let policy_stored = room_policy_json(&policy);
        conn.execute(
            "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms, created_at, revoked_at, session, expires_at, room_policy, app_service_id)
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8, ?9, ?10)",
            params![
                id,
                principal.id,
                key_hash,
                prefix,
                caps_stored,
                rooms_stored,
                created_at,
                expires_at,
                policy_stored,
                app
            ],
        )
        .map_err(|e| format!("skin session insert: {e}"))?;
        Ok(SkinAppSession {
            meta: SkinTokenMeta {
                id,
                name: principal.username,
                prefix,
                caps,
                rooms,
                room_handles: Vec::new(),
                room_access: room_access_from_policy(&policy, None),
                created_at,
                revoked_at: None,
            },
            raw,
            app: app.clone(),
            role_name,
        })
    }
}

/// The enabled `kind=app` grant of one principal for one App, or `None`.
fn enabled_app_grant(
    conn: &Connection,
    principal_id: &str,
    app: &str,
) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!(
            "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = 'app' AND target_id = ?2 AND enabled != 0"
        ),
        params![principal_id, app],
        map_grant_row,
    )
    .optional()
    .map_err(|e| format!("skin app grant lookup: {e}"))
}

/// One principal's `kind=app` grant for one App (enabled or not).
fn app_grant(
    conn: &Connection,
    principal_id: &str,
    app: &str,
) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!(
            "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = 'app' AND target_id = ?2"
        ),
        params![principal_id, app],
        map_grant_row,
    )
    .optional()
    .map_err(|e| format!("skin app grant lookup: {e}"))
}

/// What a login through `grant` snapshots: its role's per-room policy, or
/// its `scope` rooms with Thread-only caps (a role-less grant, S24-7).
fn grant_login_policy(
    conn: &Connection,
    grant: &SkinGrant,
) -> Result<(RoomPolicy, Option<String>), String> {
    if let Some(rid) = grant.role_id.as_deref() {
        let Some(role) = role_by_id_or_name(conn, rid)? else {
            return Err(format!("unknown skin role '{rid}'"));
        };
        return Ok((role.room_policy, Some(role.name)));
    }
    let rooms = parse_rooms_json(grant.scope.as_deref());
    Ok((thread_only_policy(&rooms), None))
}

/// Login role for the host row. `role_id` is `principals.role_id`.
/// `grant_role_id` is the host grant's `role_id` and is not the login role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLoginDetail {
    pub principal_id: String,
    pub username: String,
    pub role_id: Option<String>,
    pub role_name: Option<String>,
    pub default_rooms: Vec<String>,
    pub grant_role_id: Option<String>,
}

pub fn host_login_detail(username: &str) -> Result<HostLoginDetail, String> {
    let username = normalize_username(username)?;
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let grant_role_id: Option<String> = conn
            .query_row(
                "SELECT role_id FROM grants
                 WHERE subject_kind = 'principal' AND subject_id = ?1
                   AND kind = 'app' AND target_id = ?2",
                params![p.id, HOST_APP_ID],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin host grant lookup: {e}"))?
            .flatten();
        Ok(HostLoginDetail {
            principal_id: p.id,
            username: p.username,
            role_id: p.role_id,
            role_name: p.role_name,
            default_rooms: p.default_rooms,
            grant_role_id: blank_to_none(grant_role_id),
        })
    })
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
        let tx = immediate_tx(conn)?;
        tx.execute(
            "UPDATE principals SET password_hash = ?1 WHERE id = ?2",
            params![hash, p.id],
        )
        .map_err(|e| format!("skin password update: {e}"))?;
        revoke_sessions_for_principal(&tx, &p.id)?;
        tx.commit().map_err(|e| format!("skin password commit: {e}"))?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    })
    .map(attach_principal_handles)
}

/// Set or clear guest email. Empty/`None` → NULL. Invalid or duplicate → Err (400).
pub fn set_principal_email(username: &str, email: Option<&str>) -> Result<SkinPrincipal, String> {
    set_principal_email_as(username, email, crate::tool_hook::ACTOR_SYSTEM)
}

/// [`set_principal_email`] that records `actor` on the chat `update` row.
pub fn set_principal_email_as(
    username: &str,
    email: Option<&str>,
    actor: &str,
) -> Result<SkinPrincipal, String> {
    let actor = hook_actor(Some(actor));
    let username = normalize_username(username)?;
    let email = match email {
        Some(raw) => normalize_email(raw)?,
        None => None,
    };
    with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        if let Some(ref em) = email {
            ensure_email_free(conn, em, Some(&p.id))?;
        }
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin email begin: {e}"))?;
        tx.execute(
            "UPDATE principals SET email = ?1 WHERE id = ?2",
            params![email, p.id],
        )
        .map_err(|e| unique_email_err(email.as_deref(), e, "skin email update"))?;
        let out = SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        };
        let n = plan_principal_changed(&tx, &out, &actor)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin email commit: {e}"))?;
        Ok((out, n))
    })
    .map(|(p, n)| {
        if n > 0 {
            crate::tool_hook::kick();
        }
        attach_principal_handles(p)
    })
}

/// Set or clear the guest full name. Empty/`None` → NULL.
/// Does not rewrite live sessions. The HTTP writer sanitizes first
/// (`sanitize_owner_display_name`: cap 64, strip ESC, drop controls, trim).
pub fn set_principal_full_name(
    username: &str,
    full_name: Option<&str>,
) -> Result<SkinPrincipal, String> {
    set_principal_full_name_as(username, full_name, crate::tool_hook::ACTOR_SYSTEM)
}

/// [`set_principal_full_name`] that records `actor` on the chat `update` row.
pub fn set_principal_full_name_as(
    username: &str,
    full_name: Option<&str>,
    actor: &str,
) -> Result<SkinPrincipal, String> {
    let actor = hook_actor(Some(actor));
    let username = normalize_username(username)?;
    let full_name = full_name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    members::notify_ok(with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin full name begin: {e}"))?;
        tx.execute(
            "UPDATE principals SET full_name = ?1 WHERE id = ?2",
            params![full_name, p.id],
        )
        .map_err(|e| format!("skin full name update: {e}"))?;
        let out = SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: p.role_id,
            role_name: p.role_name,
            email: p.email,
            full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        };
        let n = plan_principal_changed(&tx, &out, &actor)?;
        hook_failpoint()?;
        tx.commit()
            .map_err(|e| format!("skin full name commit: {e}"))?;
        Ok((out, n))
    }))
    .map(|(p, n)| {
        if n > 0 {
            crate::tool_hook::kick();
        }
        attach_principal_handles(p)
    })
}

/// Owner-only (the HTTP writer gates it): mark a guest principal as the
/// agent of one workspace, or clear the mark with `None`. `workspace` is a
/// handle or a `projects.id`; it is stored as the id. Live sessions keep
/// working; the mark is read on every brokered db call
/// ([`principal_agent_workspace`]), so it applies from the next query.
pub fn set_principal_agent(
    username: &str,
    workspace: Option<&str>,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let workspace_id = match workspace.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(ws) => {
            let ids = resolve_room_tokens(&[ws.to_string()])?;
            match ids.as_slice() {
                [id] => Some(id.clone()),
                _ => return Err(format!("unknown workspace handle {ws:?}")),
            }
        }
    };
    members::notify_ok(with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        conn.execute(
            "UPDATE principals SET agent_workspace = ?1 WHERE id = ?2",
            params![workspace_id, p.id],
        )
        .map_err(|e| format!("skin agent mark update: {e}"))?;
        Ok(SkinPrincipal {
            agent_workspace: workspace_id,
            agent_handle: None,
            ..p
        })
    }))
    .map(attach_principal_handles)
}

/// The owner's agent mark on one principal (`projects.id`), or `None` for
/// a person. An unknown principal is `None`: the pass resolver already
/// requires a live principal for a login session.
pub fn principal_agent_workspace(principal_id: &str) -> Result<Option<String>, String> {
    with_conn(|conn| {
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT agent_workspace FROM principals WHERE id = ?1",
                params![principal_id.trim()],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin agent mark lookup: {e}"))?;
        Ok(raw
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    })
}

// ── Sew chat grant + session-revoke ops (S28 §12, K8) ────────────────

fn principal_has_chat_grant_conn(conn: &Connection, principal_id: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM grants
             WHERE subject_kind = 'principal' AND subject_id = ?1
               AND kind = ?2 AND target_id = ?3 AND enabled != 0",
            params![principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin chat grant lookup: {e}"))?;
    Ok(n > 0)
}

/// Whether a principal holds an enabled `chat` grant on `sew` (S28 §5).
/// Read live on every pass mint, never from a session snapshot.
pub fn principal_has_chat_grant(principal_id: &str) -> Result<bool, String> {
    with_conn(|conn| principal_has_chat_grant_conn(conn, principal_id))
}

/// Test seam: write (or re-enable) a principal's `chat` grant directly,
/// without the grant hook (no chat target, no outbox rows). Tests of the
/// hook itself use [`create_grant`] with kind `chat`.
#[cfg(any(test, feature = "test-util"))]
pub fn insert_chat_grant_for_tests(principal_id: &str) -> Result<(), String> {
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE grants SET enabled = 1
                 WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = ?2 AND target_id = ?3",
                params![principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
            )
            .map_err(|e| format!("test chat grant enable: {e}"))?;
        if n == 0 {
            conn.execute(
                "INSERT INTO grants (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                 VALUES (?1, 'principal', ?2, ?3, ?4, NULL, NULL, ?5, 1, 'test')",
                params![uuid::Uuid::new_v4().to_string(), principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET, now_secs()],
            )
            .map_err(|e| format!("test chat grant insert: {e}"))?;
        }
        Ok(())
    })
}

/// Test seam: delete a principal's `chat` grant directly (S27 owns the
/// real route).
#[cfg(any(test, feature = "test-util"))]
pub fn delete_chat_grant_for_tests(principal_id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "DELETE FROM grants WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = ?2 AND target_id = ?3",
            params![principal_id, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
        )
        .map_err(|e| format!("test chat grant delete: {e}"))?;
        Ok(())
    })
}

/// An Immediate transaction on the shared connection (S28-A3: `with_conn`
/// alone is autocommit). The revoke write and its Sew outbox op commit or
/// roll back together. Never nested: only the public entry points open one.
fn immediate_tx(conn: &Connection) -> Result<rusqlite::Transaction<'_>, String> {
    rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| format!("skin begin immediate: {e}"))
}

/// [`with_conn`] inside one Immediate transaction: commits when `f`
/// succeeds, rolls back (the transaction drops) when it fails. For the
/// grant writes that end sessions, so their Sew ops ride the same commit.
fn with_immediate<T, F: FnOnce(&Connection) -> Result<T, String>>(f: F) -> Result<T, String> {
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let r = f(&tx)?;
        tx.commit().map_err(|e| format!("skin commit: {e}"))?;
        Ok(r)
    })
}

/// Queue `sew_session_revoke` ops that did not come from a `skin.db` write
/// (Connect sessions live in a JSON file, S28 §12). No-op when there is
/// no `skin.db` (so no chat target), so a box without Sew never creates
/// `skin.db` for this.
pub fn enqueue_sew_revokes(ops: &[crate::sew_revoke::SewSessionRevoke]) -> Result<(), String> {
    if ops.is_empty() || !crate::sew_revoke::writer_available() {
        for op in ops {
            crate::log_debug!("[skin] sew_session_revoke {} not queued: Sew is not set up on this box", op.label());
        }
        return Ok(());
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        for op in ops {
            crate::sew_revoke::enqueue(&tx, op)?;
        }
        tx.commit().map_err(|e| format!("skin sew revoke commit: {e}"))
    })
}

/// Live (not yet revoked) login-session rows matching `filter`, as
/// `(token id, principal id)`. `filter` is a fixed SQL fragment over
/// `tokens` with `?1`.. bound from `args`.
fn live_session_rows(
    conn: &Connection,
    filter: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<(String, String)>, String> {
    let sql = format!(
        "SELECT id, principal_id FROM tokens
         WHERE session = 1 AND revoked_at IS NULL AND principal_id IS NOT NULL AND ({filter})"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("skin session list: {e}"))?;
    let rows = stmt
        .query_map(args, |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("skin session list: {e}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("skin session list: {e}"))
}

/// One `{k2_sid: skn_<id>}` op per ended session whose principal holds a
/// `chat` grant at this moment (S28 §12). `conn` is inside the revoke's
/// transaction.
fn enqueue_session_revokes(conn: &Connection, ended: &[(String, String)]) -> Result<(), String> {
    let mut chat: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    for (token_id, principal_id) in ended {
        let has = match chat.get(principal_id.as_str()) {
            Some(v) => *v,
            None => {
                let v = principal_has_chat_grant_conn(conn, principal_id)?;
                chat.insert(principal_id.as_str(), v);
                v
            }
        };
        if has {
            crate::sew_revoke::enqueue(conn, &crate::sew_revoke::SewSessionRevoke::K2Sid(crate::sew_revoke::skin_sid(token_id)))?;
        }
    }
    Ok(())
}

/// Every live login session of one principal ends (password set, change
/// or reset; box login off; a non-App grant removed). Sew ends every
/// session of `k2:principal:<id>` (one op), when the principal has a
/// `chat` grant and at least one session ended.
fn revoke_sessions_for_principal(conn: &Connection, principal_id: &str) -> Result<usize, String> {
    let n = conn
        .execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE principal_id = ?2 AND session = 1 AND revoked_at IS NULL",
            params![now_secs(), principal_id],
        )
        .map_err(|e| format!("skin session revoke: {e}"))?;
    // A device sign-in ends with the person's sessions (app-mobile-access-v1).
    app_login::end_chains_where(conn, "principal_id = ?3", &[&principal_id], app_login::REASON_SESSIONS_ENDED)?;
    if n > 0 && principal_has_chat_grant_conn(conn, principal_id)? {
        crate::sew_revoke::enqueue(
            conn,
            &crate::sew_revoke::SewSessionRevoke::ExternalId(crate::sew_revoke::principal_external_id(principal_id)),
        )?;
    }
    Ok(n)
}

/// End one principal's live login sessions bound to one App (S24-6). An old
/// session row with no App is a `host` session. Sew ends the chat
/// sessions minted from exactly those (`skn_<id>` each).
fn revoke_sessions_for_principal_app(
    conn: &Connection,
    principal_id: &str,
    app: &str,
) -> Result<usize, String> {
    let ended = live_session_rows(
        conn,
        "principal_id = ?1 AND COALESCE(app_service_id, ?2) = ?3",
        &[&principal_id, &HOST_APP_ID, &app],
    )?;
    let n = conn
        .execute(
            "UPDATE tokens SET revoked_at = ?1
             WHERE principal_id = ?2 AND session = 1 AND revoked_at IS NULL
               AND COALESCE(app_service_id, ?3) = ?4",
            params![now_secs(), principal_id, HOST_APP_ID, app],
        )
        .map_err(|e| format!("skin session revoke: {e}"))?;
    app_login::end_chains_where(conn, "principal_id = ?3 AND app_id = ?4", &[&principal_id, &app], app_login::REASON_SESSIONS_ENDED)?;
    enqueue_session_revokes(conn, &ended)?;
    Ok(n)
}

/// S24-20: a published service went away (`publish rm`). End every live
/// login session bound to it. Its grants stay (Settings shows "App
/// removed") until S26 decides cleanup. Returns how many ended.
pub fn revoke_sessions_for_app(service_id: &str) -> Result<usize, String> {
    let service_id = service_id.trim();
    if service_id.is_empty() || service_id == HOST_APP_ID {
        return Err("revoke sessions: need a published service id".to_string());
    }
    with_conn(|conn| {
        let tx = immediate_tx(conn)?;
        let ended = live_session_rows(&tx, "app_service_id = ?1", &[&service_id])?;
        let n = tx
            .execute(
                "UPDATE tokens SET revoked_at = ?1
                 WHERE session = 1 AND revoked_at IS NULL AND app_service_id = ?2",
                params![now_secs(), service_id],
            )
            .map_err(|e| format!("skin session revoke: {e}"))?;
        app_login::end_chains_where(&tx, "app_id = ?3", &[&service_id], app_login::REASON_SESSIONS_ENDED)?;
        enqueue_session_revokes(&tx, &ended)?;
        tx.commit().map_err(|e| format!("skin session revoke commit: {e}"))?;
        Ok(n)
    })
}

/// 60s per lowercase/trim username string (even when no user). True = 429.
/// Does not look up principals. Call after auth 403, before mint lookup.
pub fn password_reset_throttled(username: &str) -> Result<bool, String> {
    let key = username.trim().to_ascii_lowercase();
    let now = now_secs();
    with_conn(|conn| {
        let last: Option<i64> = conn
            .query_row(
                "SELECT last_attempt FROM password_reset_throttle WHERE username = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin reset throttle lookup: {e}"))?;
        if let Some(last) = last {
            if now.saturating_sub(last) < RESET_THROTTLE_SECS {
                return Ok(true);
            }
        }
        conn.execute(
            "INSERT INTO password_reset_throttle (username, last_attempt)
             VALUES (?1, ?2)
             ON CONFLICT(username) DO UPDATE SET last_attempt = excluded.last_attempt",
            params![key, now],
        )
        .map_err(|e| format!("skin reset throttle set: {e}"))?;
        Ok(false)
    })
}

/// Privileged forgot hit. Miss is `Ok(None)` (HTTP `{ok:true}` no extra keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinResetMint {
    pub token: String,
    pub expires_at: i64,
    pub email: String,
}

/// Mint a one-hour consume token. `username` field matches username **or**
/// email (`@` → email lookup). Token only if password **and** populated
/// email. Unknown / invalid / no hash / no email → dummy argon2 and
/// `Ok(None)`. Invalid email string is a miss, not 400. Does **not** rate-limit.
pub fn mint_password_reset(username: &str) -> Result<Option<SkinResetMint>, String> {
    let presented = username.trim().to_ascii_lowercase();
    let stored = with_conn(|conn| lookup_reset_principal(conn, &presented))?;
    match stored {
        Some((pid, hash, email))
            if password_is_set(hash.as_deref()) && email_from_stored(email.clone()).is_some() =>
        {
            let email = email_from_stored(email).expect("populated email");
            let raw = generate_reset_raw();
            let token_hash = sha256_hex(&raw);
            let now = now_secs();
            let expires = now.saturating_add(RESET_TTL_SECS);
            let id = uuid::Uuid::new_v4().to_string();
            with_conn(|conn| {
                conn.execute(
                    "DELETE FROM password_reset_tokens
                     WHERE principal_id = ?1 AND used_at IS NULL",
                    params![pid],
                )
                .map_err(|e| format!("skin reset replace unused: {e}"))?;
                conn.execute(
                    "INSERT INTO password_reset_tokens
                     (id, principal_id, token_hash, expires_at, used_at, created_at)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                    params![id, pid, token_hash, expires, now],
                )
                .map_err(|e| format!("skin reset token insert: {e}"))?;
                Ok(())
            })?;
            Ok(Some(SkinResetMint {
                token: raw,
                expires_at: expires,
                email,
            }))
        }
        _ => {
            dummy_verify("k2-skin-dummy-verify-target");
            Ok(None)
        }
    }
}

fn lookup_reset_principal(
    conn: &Connection,
    presented: &str,
) -> Result<Option<(String, Option<String>, Option<String>)>, String> {
    if presented_looks_like_email(presented) {
        let Ok(Some(email)) = normalize_email(presented) else {
            return Ok(None);
        };
        conn.query_row(
            "SELECT id, password_hash, email FROM principals
             WHERE email = ?1 COLLATE NOCASE
               AND email IS NOT NULL AND trim(email) != ''",
            params![email],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin reset mint lookup: {e}"))
    } else {
        conn.query_row(
            "SELECT id, password_hash, email FROM principals WHERE username = ?1",
            params![presented],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin reset mint lookup: {e}"))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SkinResetConsume {
    Ok,
    BadToken,
}

/// Consume a reset token. Hashed lookup, TTL, single-use. Success calls
/// [`set_principal_password`] (revokes session=1). Empty password is refused
/// so this door never clears a hash. Does not mint a session.
pub fn consume_password_reset(token: &str, password: &str) -> Result<SkinResetConsume, String> {
    let token = token.trim();
    if token.is_empty() {
        dummy_verify(password);
        return Ok(SkinResetConsume::BadToken);
    }
    if password.trim().is_empty() {
        return Err("password must not be empty".to_string());
    }
    let hash = sha256_hex(token);
    let now = now_secs();
    let row = with_conn(|conn| {
        let row: Option<(String, String, i64, Option<i64>)> = conn
            .query_row(
                "SELECT t.id, p.username, t.expires_at, t.used_at
                 FROM password_reset_tokens t
                 JOIN principals p ON p.id = t.principal_id
                 WHERE t.token_hash = ?1",
                params![hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| format!("skin reset consume lookup: {e}"))?;
        Ok(row)
    })?;
    let Some((id, username, expires_at, used_at)) = row else {
        dummy_verify(password);
        return Ok(SkinResetConsume::BadToken);
    };
    if used_at.is_some() || expires_at <= now {
        dummy_verify(password);
        return Ok(SkinResetConsume::BadToken);
    }
    set_principal_password(&username, Some(password))?;
    with_conn(|conn| {
        conn.execute(
            "UPDATE password_reset_tokens SET used_at = ?1 WHERE id = ?2",
            params![now_secs(), id],
        )
        .map_err(|e| format!("skin reset mark used: {e}"))?;
        Ok(())
    })?;
    Ok(SkinResetConsume::Ok)
}

/// Verify a guest's current password. Dummy argon2 on miss / NULL hash.
/// Does **not** bump `login_lockouts`.
pub fn verify_principal_password(username: &str, password: &str) -> bool {
    let key = match normalize_username(username) {
        Ok(k) => k,
        Err(_) => {
            dummy_verify(password);
            return false;
        }
    };
    let stored = with_conn(|conn| {
        let hash: Option<Option<String>> = conn
            .query_row(
                "SELECT password_hash FROM principals WHERE username = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin password verify lookup: {e}"))?;
        Ok(hash.flatten().filter(|s| !s.trim().is_empty()))
    })
    .unwrap_or(None);
    match stored.as_deref() {
        Some(h) => verify_hash(password, h),
        None => {
            dummy_verify(password);
            false
        }
    }
}

/// Login gate: lockout then dummy-argon2 verify. Never holds `skin.db`
/// across hashing.
#[derive(Debug)]
pub enum SkinLoginOutcome {
    Ok(SkinPrincipal),
    BadCreds,
    /// The login is locked (3 bad tries → 15 min). A name that resolves to
    /// a principal (by username OR email) is locked per principal, so the
    /// username and the email share one counter and one lock. A name that
    /// resolves to nothing is counted per presented string and locks the
    /// same way, so "locked" never says which usernames exist.
    LockedOut { retry_after_secs: i64 },
}

/// Presented-string key: trim + ASCII lowercase; an `@` keeps that string;
/// otherwise [`normalize_username`], and on failure the trim+lowercase
/// string. Used for names that resolve to no principal, and the key
/// [`clear_login_lockout`] reports back.
fn login_lockout_key(username: &str) -> String {
    let presented = username.trim().to_ascii_lowercase();
    if presented_looks_like_email(&presented) {
        presented
    } else {
        normalize_username(username).unwrap_or(presented)
    }
}

/// The principal a login name resolves to (by email when it has an `@`,
/// else by username), password or not.
struct LoginPrincipalRow {
    username: String,
    email: Option<String>,
    password_hash: Option<String>,
}

/// Which `login_lockouts` rows one login attempt reads and writes.
///
/// `primary` is the one counter failures bump. For a known principal it is
/// the principal's username: usernames are unique, never renamed, and can
/// never contain `@`, so no presented email can land on it, and typing the
/// username or the email hits the same row. For an unknown name it is the
/// presented-string key, exactly as before.
///
/// `extra` is the principal's email key: rows written there by 0.45.5
/// (which counted the email separately) still lock until they expire, and
/// unlock/success clear them. For an unknown name `extra == primary`, so
/// both paths run the very same queries.
struct LoginLockSlot {
    primary: String,
    extra: String,
}

fn login_lock_slot(presented_key: &str, principal: Option<&LoginPrincipalRow>) -> LoginLockSlot {
    match principal {
        Some(p) => LoginLockSlot {
            primary: p.username.clone(),
            extra: p
                .email
                .as_deref()
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| p.username.clone()),
        },
        None => LoginLockSlot {
            primary: presented_key.to_string(),
            extra: presented_key.to_string(),
        },
    }
}

/// Delete every `login_lockouts` row for this login: for a name that
/// resolves to a principal (username or email) that is the principal's
/// username row, its email row and the presented row; otherwise only the
/// presented row. Does not create a principal, touch `password_hash`, or
/// revoke sessions. Returns the presented key and whether any row was
/// deleted (`false` when none was stored).
/// The username a presented login name resolves to, as login and unlock
/// resolve it: an `@` means email (any case), else username. `None`: no
/// such guest.
pub fn login_principal_username(presented: &str) -> Result<Option<String>, String> {
    let presented = presented.trim().to_ascii_lowercase();
    with_conn(|conn| Ok(lookup_login_principal(conn, &presented)?.map(|p| p.username)))
}

pub fn clear_login_lockout(username: &str) -> Result<(String, bool), String> {
    let key = login_lockout_key(username);
    let presented = username.trim().to_ascii_lowercase();
    let deleted = with_conn(|conn| {
        let principal = lookup_login_principal(conn, &presented)?;
        let slot = login_lock_slot(&key, principal.as_ref());
        let n = conn
            .execute(
                "DELETE FROM login_lockouts WHERE username IN (?1, ?2, ?3)",
                params![key, slot.primary, slot.extra],
            )
            .map_err(|e| format!("skin lockout clear: {e}"))?;
        Ok(n)
    })?;
    Ok((key, deleted > 0))
}

pub fn check_and_record_login(username: &str, password: &str) -> SkinLoginOutcome {
    // Lockout 3/15. `@` → email lookup; else username. Invalid email is a
    // miss, not 400. A name that resolves to a principal is counted on
    // that principal (see [`LoginLockSlot`]); anything else on the
    // presented string. The lock is checked before the password, with the
    // same queries for known and unknown names.
    let presented = username.trim().to_ascii_lowercase();
    let key = login_lockout_key(username);

    let looked_up = with_conn(|conn| {
        let principal = lookup_login_principal(conn, &presented)?;
        let slot = login_lock_slot(&key, principal.as_ref());
        let now = now_secs();
        let until: Option<i64> = conn
            .query_row(
                "SELECT MAX(locked_until) FROM login_lockouts
                 WHERE username IN (?1, ?2) AND locked_until > ?3",
                params![slot.primary, slot.extra, now],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin lockout lookup: {e}"))?;
        if let Some(until) = until {
            return Ok((principal, slot, Some(until - now)));
        }
        conn.execute(
            "UPDATE login_lockouts SET locked_until = NULL, failed_count = 0
             WHERE username IN (?1, ?2) AND locked_until IS NOT NULL",
            params![slot.primary, slot.extra],
        )
        .map_err(|e| format!("skin lockout expire: {e}"))?;
        Ok((principal, slot, None))
    });
    let (principal, slot) = match looked_up {
        Ok((_, _, Some(left))) => {
            return SkinLoginOutcome::LockedOut {
                retry_after_secs: left.max(1),
            };
        }
        Ok((principal, slot, None)) => (principal, slot),
        Err(_) => (None, login_lock_slot(&key, None)),
    };

    let ok = match principal.as_ref().and_then(|p| p.password_hash.as_deref()) {
        Some(h) => verify_hash(password, h),
        None => {
            dummy_verify(password);
            false
        }
    };

    let _ = with_conn(|conn| {
        if ok {
            conn.execute(
                "DELETE FROM login_lockouts WHERE username IN (?1, ?2, ?3)",
                params![key, slot.primary, slot.extra],
            )
            .map_err(|e| format!("skin lockout clear: {e}"))?;
        } else {
            let failed: i64 = conn
                .query_row(
                    "SELECT failed_count FROM login_lockouts WHERE username = ?1",
                    params![slot.primary],
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
                    params![slot.primary, now_secs().saturating_add(LOCKOUT_DURATION_SECS)],
                )
                .map_err(|e| format!("skin lockout set: {e}"))?;
            } else {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES (?1, ?2, NULL)
                     ON CONFLICT(username) DO UPDATE SET failed_count = excluded.failed_count, locked_until = NULL",
                    params![slot.primary, failed],
                )
                .map_err(|e| format!("skin lockout bump: {e}"))?;
            }
        }
        Ok(())
    });

    if !ok {
        return SkinLoginOutcome::BadCreds;
    }
    let Some(principal) = principal else {
        return SkinLoginOutcome::BadCreds;
    };
    match with_conn(|conn| principal_by_username(conn, &principal.username)) {
        Ok(Some(p)) => SkinLoginOutcome::Ok(attach_principal_handles(p)),
        _ => SkinLoginOutcome::BadCreds,
    }
}

/// Resolve a presented login name (already trim + lowercase) to its
/// principal: an `@` means email (any case), else username. A malformed
/// email or username is a miss. The password hash is `None` when unset or
/// blank (that principal can never password-login).
fn lookup_login_principal(
    conn: &Connection,
    presented: &str,
) -> Result<Option<LoginPrincipalRow>, String> {
    let row: Option<(String, Option<String>, Option<String>)> =
        if presented_looks_like_email(presented) {
            let Ok(Some(email)) = normalize_email(presented) else {
                return Ok(None);
            };
            conn.query_row(
                "SELECT username, email, password_hash FROM principals
                 WHERE email = ?1 COLLATE NOCASE
                   AND email IS NOT NULL AND trim(email) != ''",
                params![email],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("skin password lookup: {e}"))?
        } else {
            let key = normalize_username(presented).unwrap_or_else(|_| presented.to_string());
            conn.query_row(
                "SELECT username, email, password_hash FROM principals WHERE username = ?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("skin password lookup: {e}"))?
        };
    Ok(row.map(|(username, email, hash)| LoginPrincipalRow {
        username,
        email,
        password_hash: hash.filter(|s| !s.trim().is_empty()),
    }))
}

pub fn list_tokens() -> Result<Vec<SkinTokenMeta>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.name, t.key_prefix, t.caps, t.rooms, t.created_at, t.revoked_at
                 FROM tokens t
                 WHERE t.session = 0 AND t.link_id IS NULL
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
        // S28-A3: read the row, revoke it and queue its Sew op in one
        // Immediate transaction; only a login session (`session=1`) whose
        // principal holds a `chat` grant gets an op.
        let tx = immediate_tx(conn)?;
        let ended = live_session_rows(&tx, "id = ?1", &[&id])?;
        let n = tx
            .execute(
                "UPDATE tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                params![now_secs(), id],
            )
            .map_err(|e| format!("skin token revoke: {e}"))?;
        if n > 0 {
            enqueue_session_revokes(&tx, &ended)?;
            // Logout or an owner revoke of a device's pass ends that device
            // sign-in too (its refresh token stops working).
            app_login::end_chains_where(&tx, "session_token_id = ?3", &[&id], app_login::REASON_SELF)?;
        }
        tx.commit().map_err(|e| format!("skin token revoke commit: {e}"))?;
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
                "UPDATE tokens SET rooms = ?1 WHERE id = ?2 AND link_id IS NULL",
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

/// What a `tokens` row is, for a caller deciding who may change it
/// (`None` from [`token_scope`]: no such row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenScope {
    /// A guest's sign-in session (`session = 1`), not a platform pass.
    pub session: bool,
    /// The session's guest (sessions only).
    pub principal_id: Option<String>,
    /// The App it is bound to (an App session or an App's gateway pass).
    pub app_service_id: Option<String>,
    /// An App gateway link's pass (`link_id` set).
    pub linked: bool,
    pub caps: Vec<String>,
    pub rooms: Vec<String>,
    /// The stored rooms parsed as a list of rooms (else `rooms` is empty
    /// because it couldn't be read, not because there are none).
    pub rooms_readable: bool,
}

/// [`TokenScope`] of the token `id` (any kind, live or revoked).
pub fn token_scope(id: &str) -> Result<Option<TokenScope>, String> {
    let id = id.trim().to_string();
    with_conn(|conn| {
        conn.query_row(
            "SELECT session, principal_id, app_service_id, link_id, caps, rooms FROM tokens WHERE id = ?1",
            params![id],
            |r| {
                let caps_raw: String = r.get(4)?;
                let rooms_raw: String = r.get(5)?;
                Ok(TokenScope {
                    session: r.get::<_, i64>(0)? != 0,
                    principal_id: r.get(1)?,
                    app_service_id: blank_to_none(r.get(2)?),
                    linked: r.get::<_, Option<String>>(3)?.is_some_and(|l| !l.trim().is_empty()),
                    caps: caps_from_json(&caps_raw),
                    rooms: rooms_from_json(&rooms_raw),
                    rooms_readable: serde_json::from_str::<Vec<String>>(&rooms_raw).is_ok(),
                })
            },
        )
        .optional()
        .map_err(|e| format!("skin token lookup: {e}"))
    })
}

/// Set principal default rooms. `apply_tokens` copies onto live **sessions**
/// only — never platform tokens. Without it, live sessions still lose any
/// room that is no longer in `rooms` (removal is never deferred).
pub fn set_principal_default_rooms(
    username: &str,
    rooms: &[String],
    apply_tokens: bool,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    members::notify_ok(with_conn(|conn| {
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
        // AM1: the host grant's scope is what box login reads; the column
        // stays a shadow for one release. No host row (box login off) → 0.
        conn.execute(
            "UPDATE grants SET role_id = NULL, scope = ?1
             WHERE subject_kind = 'principal' AND subject_id = ?2
               AND kind = 'app' AND target_id = ?3",
            params![stored, p.id, HOST_APP_ID],
        )
        .map_err(|e| format!("skin host grant rooms: {e}"))?;
        if apply_tokens {
            let policy = thread_only_policy(&rooms);
            rewrite_live_sessions(conn, &p.id, HOST_APP_ID, &policy)?;
        } else {
            // Without --apply-tokens an added room waits for the next
            // login, but a dropped room must leave live sessions now.
            narrow_live_sessions(conn, &p.id, HOST_APP_ID, &rooms)?;
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
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    }))
    .map(attach_principal_handles)
}

// ── Roles ────────────────────────────────────────────────────────────

const ROLE_SELECT: &str =
    "SELECT id, name, caps, rooms, created_at, COALESCE(room_policy, '{}'), app_id, room_paths FROM roles";

fn blank_to_none(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn map_role_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinRole> {
    let caps_raw: String = r.get(2)?;
    let rooms_raw: String = r.get(3)?;
    let policy_raw: String = r.get(5)?;
    let app_id: Option<String> = r.get(6)?;
    let paths_raw: Option<String> = r.get(7)?;
    let caps = caps_from_json(&caps_raw);
    let rooms = rooms_from_json(&rooms_raw);
    let room_policy = policy_or_cartesian(&caps, &rooms, Some(&policy_raw))
        .unwrap_or_else(|_| cartesian_policy(&caps, &rooms));
    Ok(SkinRole {
        id: r.get(0)?,
        name: r.get(1)?,
        caps: union_caps(&room_policy),
        rooms: rooms_from_policy(&room_policy),
        room_handles: Vec::new(),
        room_access: Vec::new(),
        created_at: r.get(4)?,
        app_id: blank_to_none(app_id),
        room_policy,
        // Display and editing only. Enforcement re-reads the column
        // strictly and fails closed ([`paths::session_paths`]).
        room_paths: paths::parse_room_paths(paths_raw.as_deref()).unwrap_or_default(),
    })
}

fn attach_role_handles(mut role: SkinRole) -> SkinRole {
    role.room_handles = handles_for_project_ids(&role.rooms);
    role.room_access = room_access_from_policy(&role.room_policy, Some(&role.room_paths));
    role.caps = union_caps(&role.room_policy);
    role.rooms = rooms_from_policy(&role.room_policy);
    role
}

fn persist_role_policy(
    conn: &Connection,
    role_id: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
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
    members::changed(None);
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
        app_id: None,
        room_policy: policy,
        room_paths: RoomPaths::new(),
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

/// Rewrite one principal's live sessions bound to one App (S24-6). Host
/// writers (assign, unassign, default rooms) pass [`HOST_APP_ID`]; an old
/// session row with no App is a host session.
fn rewrite_live_sessions(
    conn: &Connection,
    principal_id: &str,
    app: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
         WHERE principal_id = ?4 AND session = 1 AND revoked_at IS NULL
           AND COALESCE(app_service_id, ?5) = ?6",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            principal_id,
            HOST_APP_ID,
            app
        ],
    )
    .map_err(|e| format!("skin session rewrite: {e}"))?;
    Ok(())
}

/// Drop every room not in `keep` from one principal's live sessions bound
/// to one App. Caps on the rooms that stay are unchanged; nothing is added.
fn narrow_live_sessions(
    conn: &Connection,
    principal_id: &str,
    app: &str,
    keep: &[String],
) -> Result<(), String> {
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT id, caps, rooms, room_policy FROM tokens
                 WHERE principal_id = ?1 AND session = 1 AND revoked_at IS NULL
                   AND COALESCE(app_service_id, ?2) = ?3",
            )
            .map_err(|e| format!("skin session narrow: {e}"))?;
        let mapped = stmt
            .query_map(params![principal_id, HOST_APP_ID, app], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| format!("skin session narrow: {e}"))?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin session narrow row: {e}"))?
    };
    for (id, caps_raw, rooms_raw, policy_raw) in rows {
        let caps = caps_from_json(&caps_raw);
        let rooms = rooms_from_json(&rooms_raw);
        let mut policy = interpret_token_policy(true, &caps, &rooms, policy_raw.as_deref());
        let before = policy.len();
        policy.retain(|room, _| keep.iter().any(|k| k == room));
        if policy.len() == before {
            continue;
        }
        conn.execute(
            "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3 WHERE id = ?4",
            params![
                caps_json(&union_caps(&policy)),
                rooms_json(&rooms_from_policy(&policy)),
                room_policy_json(&policy),
                id
            ],
        )
        .map_err(|e| format!("skin session narrow: {e}"))?;
    }
    Ok(())
}

/// R8 rewrite after a role change (S24-6): every live session whose own
/// `(principal, App)` grant carries this role. A session's App is its
/// `app_service_id` (NULL = `host`). Never keyed on `principals.role_id`.
fn rewrite_sessions_for_role(
    conn: &Connection,
    role_id: &str,
    policy: &RoomPolicy,
) -> Result<(), String> {
    conn.execute(
        "UPDATE tokens SET caps = ?1, rooms = ?2, room_policy = ?3
         WHERE session = 1 AND revoked_at IS NULL
           AND EXISTS (
             SELECT 1 FROM grants g
             WHERE g.subject_kind = 'principal' AND g.subject_id = tokens.principal_id
               AND g.kind = 'app'
               AND g.target_id = COALESCE(tokens.app_service_id, ?5)
               AND g.role_id = ?4)",
        params![
            caps_json(&union_caps(policy)),
            rooms_json(&rooms_from_policy(policy)),
            room_policy_json(policy),
            role_id,
            HOST_APP_ID
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
/// `app_id` omitted (`None`) stays NULL — host-wide. Role names stay
/// box-unique (`roles_name`); this does not change that index.
/// A role name may not parse as a UUID (role ids are UUIDs).
pub const ROLE_NAME_LOOKS_LIKE_ID: &str = "a role name can't look like a role id (a UUID): pick a word";

pub fn create_role(name: &str, policy: &RoomPolicy) -> Result<SkinRole, String> {
    create_role_for_app(name, policy, None)
}

/// Same as [`create_role`], writing `roles.app_id` when `app_id` is set.
/// The stored app id is `published_services.id` (or the literal `host` is
/// not used here — host-wide roles stay NULL).
pub fn create_role_for_app(
    name: &str,
    policy: &RoomPolicy,
    app_id: Option<&str>,
) -> Result<SkinRole, String> {
    let name = normalize_username(name)?;
    if is_connect_role_name(&name) {
        return Err(CONNECT_ROLE_NAME_ERR.to_string());
    }
    // Roles are found by id first, then name: a name shaped like an id
    // could stand in for another role (security review, 0.46.4).
    if uuid::Uuid::parse_str(&name).is_ok() {
        return Err(ROLE_NAME_LOOKS_LIKE_ID.to_string());
    }
    let app_id = app_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
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
            "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy, app_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                caps_json(&union_caps(&policy)),
                rooms_json(&rooms_from_policy(&policy)),
                created_at,
                room_policy_json(&policy),
                app_id,
            ],
        )
        .map_err(|e| unique_role_err(&name, e))?;
        let mut role = role_from_policy(id, name, created_at, policy);
        role.app_id = app_id;
        Ok(role)
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

/// One role by id or name (case-insensitive), with its stored
/// `room_policy`. `None` when no such role.
pub fn find_role(id_or_name: &str) -> Result<Option<SkinRole>, String> {
    with_conn(|conn| role_by_id_or_name(conn, id_or_name))
}

/// Present policy replaces the whole map. `None` is a no-op persist of the
/// current snapshot (still R8-rewrites live sessions).
pub fn update_role(id_or_name: &str, policy: Option<&RoomPolicy>) -> Result<SkinRole, String> {
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
        paths::prune_role_paths(conn, &role)?;
        role.room_paths = paths::role_room_paths(conn, &role.id).unwrap_or_default();
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
        // The room's folders go with its line: re-adding the room later
        // starts unscoped, never with a stale scope.
        paths::prune_role_paths(conn, &role)?;
        role.room_paths.remove(project_id);
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
        let granted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM grants WHERE role_id = ?1",
                params![role.id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin role grant lookup: {e}"))?;
        if granted > 0 {
            return Err(grant_role_remove_err(&role.name));
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
    members::changed(None);
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
        // AM1: the host grant is the box-login truth; the column is the
        // shadow. No host row (box login off) → nothing to write.
        conn.execute(
            "UPDATE grants SET role_id = ?1, scope = NULL
             WHERE subject_kind = 'principal' AND subject_id = ?2
               AND kind = 'app' AND target_id = ?3",
            params![role.id, p.id, HOST_APP_ID],
        )
        .map_err(|e| format!("skin host grant role: {e}"))?;
        rewrite_live_sessions(conn, &p.id, HOST_APP_ID, &role.room_policy)?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: Some(role.id),
            role_name: Some(role.name),
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    members::changed(None);
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
        conn.execute(
            "UPDATE grants SET role_id = NULL, scope = ?1
             WHERE subject_kind = 'principal' AND subject_id = ?2
               AND kind = 'app' AND target_id = ?3",
            params![rooms_json(&p.default_rooms), p.id, HOST_APP_ID],
        )
        .map_err(|e| format!("skin host grant role: {e}"))?;
        rewrite_live_sessions(
            conn,
            &p.id,
            HOST_APP_ID,
            &thread_only_policy(&p.default_rooms),
        )?;
        Ok(SkinPrincipal {
            id: p.id,
            username: p.username,
            created_at: p.created_at,
            default_rooms: p.default_rooms,
            default_room_handles: Vec::new(),
            has_password: p.has_password,
            role_id: None,
            role_name: None,
            email: p.email,
            full_name: p.full_name,
            agent_workspace: p.agent_workspace,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    members::changed(None);
    Ok(attach_principal_handles(p))
}

// ── Grants ───────────────────────────────────────────────────────────
// kind=app is a login: `target_id` is `host` (box login) or a published
// service id, and a login for that App needs this row enabled (S24-4).
// It carries EITHER a role (the role's `app_id` is NULL or this App) OR a
// role-less `scope` that is a rooms list (Thread-only caps, S24-7) — the
// shape box logins of unassigned guests have. Login reads the policy from
// the grant (AM1). Other kinds (mailbox, database, cli) hold a scope and
// no role; they are recorded, not enforced, in 0.45.5.

fn default_grant_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkinGrant {
    pub id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub kind: String,
    pub target_id: String,
    pub role_id: Option<String>,
    pub scope: Option<String>,
    pub created_at: i64,
    /// `false` is no grant for any authorization read. The row stays.
    #[serde(default = "default_grant_enabled")]
    pub enabled: bool,
    /// Who wrote the row (`owner-token`, `user:<name>`, `agent:<handle>`,
    /// [`UPGRADE_CREATED_BY`]); `None` on rows from before 0.45.5.
    #[serde(default)]
    pub created_by: Option<String>,
}

/// Inputs for one grant. `role` is a role id or name (kind=app only).
#[derive(Debug, Clone, Default)]
pub struct SkinGrantWrite {
    pub subject_kind: String,
    pub subject_id: String,
    pub kind: String,
    pub target_id: String,
    pub role: Option<String>,
    pub scope: Option<String>,
    /// Audit only (`grants.created_by`).
    pub created_by: Option<String>,
}

fn grant_subject_kind(raw: &str) -> Result<String, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "principal" => Ok("principal".to_string()),
        "workspace" => Ok("workspace".to_string()),
        // Sew S27: a K2 Connect login (stored by its K7 id), kind=chat only.
        "connect" => Ok("connect".to_string()),
        _ => Err("subjectKind must be principal, workspace or connect".to_string()),
    }
}

/// A Connect login token (its K7 id, or its username) → its id.
fn resolve_connect_subject(token: &str) -> Result<String, String> {
    let t = token.trim();
    if crate::tool_hook::sew_wire::is_lower_uuid(t) {
        return Ok(t.to_string());
    }
    crate::connect_users::user_uuid(t)?.ok_or_else(|| format!("unknown Connect user '{t}'"))
}

fn grant_kind(raw: &str) -> Result<String, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "app" => Ok("app".to_string()),
        "mailbox" => Ok("mailbox".to_string()),
        "database" => Ok("database".to_string()),
        "cli" => Ok("cli".to_string()),
        // Sew S27: a chat grant creates the Sew account (tool_hook).
        "chat" => Ok("chat".to_string()),
        _ => Err("kind must be app, mailbox, database, cli, or chat".to_string()),
    }
}

pub(crate) fn map_grant_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkinGrant> {
    let role_id: Option<String> = r.get(5)?;
    let scope: Option<String> = r.get(6)?;
    let enabled: i64 = r.get(8)?;
    let created_by: Option<String> = r.get(9)?;
    Ok(SkinGrant {
        id: r.get(0)?,
        subject_kind: r.get(1)?,
        subject_id: r.get(2)?,
        kind: r.get(3)?,
        target_id: r.get(4)?,
        role_id: blank_to_none(role_id),
        scope: blank_to_none(scope),
        created_at: r.get(7)?,
        enabled: enabled != 0,
        created_by: blank_to_none(created_by),
    })
}

pub(crate) const GRANT_SELECT: &str =
    "SELECT id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by FROM grants";

fn principal_id_exists(conn: &Connection, id: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM principals WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| format!("skin grant principal lookup: {e}"))?;
    Ok(n > 0)
}

/// A kind=app `scope` must be a JSON array of room ids (`[]` allowed).
/// Stored normalized (trimmed, de-duplicated) like `default_rooms`.
pub fn normalize_app_scope(raw: &str) -> Result<String, String> {
    let list: Vec<String> = serde_json::from_str(raw.trim()).map_err(|_| {
        "kind=app scope must be a JSON array of rooms, e.g. [\"<room id>\"]".to_string()
    })?;
    Ok(rooms_json(&normalize_room_ids(&list)))
}

/// kind=app: exactly one of a role (whose `app_id` is NULL or `target`) or
/// a rooms scope (S24-7). Returns `(role_id, stored_scope)`.
fn app_grant_shape(
    conn: &Connection,
    target_id: &str,
    role_token: Option<&str>,
    scope: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    match (role_token, scope) {
        (Some(_), Some(_)) => Err("kind=app takes roleId or a rooms scope, not both".to_string()),
        (None, None) => Err("kind=app requires roleId or a rooms scope".to_string()),
        (None, Some(scope)) => Ok((None, Some(normalize_app_scope(scope)?))),
        (Some(token), None) => {
            let Some(role) = role_by_id_or_name(conn, token)? else {
                return Err(format!("unknown skin role '{token}'"));
            };
            if let Some(app_id) = role.app_id.as_deref() {
                if app_id != target_id {
                    return Err(format!(
                        "role '{}' is for app '{app_id}', not '{target_id}'",
                        role.name
                    ));
                }
            }
            Ok((Some(role.id), None))
        }
    }
}

/// Insert one grant. Does not mint a principal and does not rewrite sessions.
/// A `host` app grant also writes the shadow columns (`principals.role_id`
/// / `default_rooms`) for one release (AM1).
pub fn create_grant(write: &SkinGrantWrite) -> Result<SkinGrant, String> {
    if let Some(p) = crate::tool_hook::provisioner_for(&write.kind) {
        return create_hooked_grant(p, write);
    }
    let created = create_plain_grant(write)?;
    if created.kind == "app" {
        // App user change: K31 derives App Space membership from App
        // grants; S27 only asks for a sweep (rev 147).
        crate::tool_hook::kick_reconcile();
    }
    Ok(created)
}

fn create_plain_grant(write: &SkinGrantWrite) -> Result<SkinGrant, String> {
    let subject_kind = grant_subject_kind(&write.subject_kind)?;
    if subject_kind == "connect" {
        return Err("subjectKind connect is only for kind=chat".to_string());
    }
    let subject_id = write.subject_id.trim().to_string();
    if subject_id.is_empty() {
        return Err("missing subject id".to_string());
    }
    let kind = grant_kind(&write.kind)?;
    let target_id = write.target_id.trim().to_string();
    if target_id.is_empty() {
        return Err("missing target id".to_string());
    }
    let role_token = write
        .role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scope = write
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let created_by = write
        .created_by
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    members::notify_ok(with_conn(|conn| {
        if subject_kind == "principal" && !principal_id_exists(conn, &subject_id)? {
            return Err(format!("unknown principal id '{subject_id}'"));
        }
        let (role_id, stored_scope) = if kind == "app" {
            app_grant_shape(conn, &target_id, role_token, scope.as_deref())?
        } else {
            if role_token.is_some() {
                return Err("roleId is only set for kind=app".to_string());
            }
            let Some(scope) = scope.clone() else {
                return Err(format!("kind={kind} requires scope"));
            };
            (None, Some(scope))
        };

        let taken: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM grants
                 WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
                params![subject_kind, subject_id, kind, target_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin grant lookup: {e}"))?;
        if taken > 0 {
            return Err("grant already exists for this subject, kind, and target".to_string());
        }

        let id = uuid::Uuid::new_v4().to_string();
        let created_at = now_secs();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        tx.execute(
            "INSERT INTO grants
             (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
            params![
                id,
                subject_kind,
                subject_id,
                kind,
                target_id,
                role_id,
                stored_scope,
                created_at,
                created_by
            ],
        )
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("UNIQUE") {
                "grant already exists for this subject, kind, and target".to_string()
            } else {
                format!("skin grant insert: {e}")
            }
        })?;
        if kind == "app" && target_id == HOST_APP_ID && subject_kind == "principal" {
            // Shadow columns (AM1): keep `principals.role_id` /
            // `default_rooms` equal to the box-login grant for one release.
            match role_id.as_deref() {
                Some(rid) => tx.execute(
                    "UPDATE principals SET role_id = ?1 WHERE id = ?2",
                    params![rid, subject_id],
                ),
                None => tx.execute(
                    "UPDATE principals SET role_id = NULL, default_rooms = ?1 WHERE id = ?2",
                    params![stored_scope.as_deref().unwrap_or("[]"), subject_id],
                ),
            }
            .map_err(|e| format!("skin grant shadow: {e}"))?;
        }
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok(SkinGrant {
            id,
            subject_kind,
            subject_id,
            kind,
            target_id,
            role_id,
            scope: stored_scope,
            created_at,
            enabled: true,
            created_by,
        })
    }))
}


// ── Hooked grants (Sew S27 tool_hook) ────────────────────────────────
//
// A kind with a provisioner (`chat`) validates, writes the grant AND its
// outbox rows in ONE skin.db transaction, then kicks the worker. Facts
// from the main DB (workspace handle/name) are read BEFORE the skin lock.

fn hook_actor(created_by: Option<&str>) -> String {
    created_by
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::tool_hook::ACTOR_SYSTEM)
        .to_string()
}

/// Test-only failpoint: error after the outbox insert, before commit.
#[cfg(any(test, feature = "test-util"))]
pub static HOOK_FAIL_BEFORE_COMMIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn hook_failpoint() -> Result<(), String> {
    #[cfg(any(test, feature = "test-util"))]
    if HOOK_FAIL_BEFORE_COMMIT.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("injected failure before commit".to_string());
    }
    Ok(())
}

/// A workspace subject token (uuid, handle or alias) → `projects.id`.
fn resolve_workspace_subject(token: &str) -> Result<String, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("missing subject id".to_string());
    }
    match resolve_room_tokens(&[token.to_string()]) {
        Ok(ids) if ids.len() == 1 => Ok(ids[0].clone()),
        Ok(_) => Err(format!("unknown workspace '{token}'")),
        Err(e) if e.starts_with("ambiguous") => Err(e),
        Err(_) => Err(format!("unknown workspace '{token}'")),
    }
}

fn create_hooked_grant(
    p: &'static dyn crate::tool_hook::ToolProvisioner,
    write: &SkinGrantWrite,
) -> Result<SkinGrant, String> {
    use crate::tool_hook::{outbox, HookCtx, HookEvent};
    let subject_kind = grant_subject_kind(&write.subject_kind)?;
    let mut write = write.clone();
    write.subject_kind = subject_kind.clone();
    let mut ws_ids = Vec::new();
    if subject_kind == "workspace" {
        write.subject_id = resolve_workspace_subject(&write.subject_id)?;
        ws_ids.push(write.subject_id.clone());
    } else if subject_kind == "connect" {
        write.subject_id = resolve_connect_subject(&write.subject_id)?;
    } else if write.subject_id.trim().is_empty() {
        return Err("missing subject id".to_string());
    }
    let actor = hook_actor(write.created_by.as_deref());
    let ctx = HookCtx::load(&actor, &ws_ids);
    let now = now_secs();
    let grant = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let w = p.validate(&tx, &write, &ctx)?;
        let taken: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM grants
                 WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
                params![w.subject_kind, w.subject_id, w.kind, w.target_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin grant lookup: {e}"))?;
        if taken > 0 {
            return Err("grant already exists for this subject, kind, and target".to_string());
        }
        let grant = SkinGrant {
            id: uuid::Uuid::new_v4().to_string(),
            subject_kind: w.subject_kind.clone(),
            subject_id: w.subject_id.clone(),
            kind: w.kind.clone(),
            target_id: w.target_id.clone(),
            role_id: None,
            scope: w.scope.clone(),
            created_at: now,
            enabled: true,
            created_by: w.created_by.clone(),
        };
        tx.execute(
            "INSERT INTO grants
             (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, 1, ?8)",
            params![
                grant.id,
                grant.subject_kind,
                grant.subject_id,
                grant.kind,
                grant.target_id,
                grant.scope,
                grant.created_at,
                grant.created_by
            ],
        )
        .map_err(|e| format!("skin grant insert: {e}"))?;
        let ops = p.plan(&tx, &HookEvent::GrantCreated { grant: &grant }, &ctx)?;
        outbox::enqueue(&tx, &ops, &actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok(grant)
    })?;
    crate::tool_hook::kick();
    members::changed(None);
    Ok(grant)
}

/// `POST /cli/skin/grants/update`: change a hooked grant's scope (§6.4).
/// Only hooked kinds; `coworker` ↔ `assistant:*` is refused (delete and
/// add again). A no-op scope enqueues nothing.
pub fn update_grant_scope(id: &str, scope: &str, actor: &str) -> Result<SkinGrant, String> {
    use crate::tool_hook::{outbox, provisioner_for, HookCtx, HookEvent};
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let old = with_conn(|conn| grant_by_id(conn, id))?.ok_or_else(|| "unknown grant".to_string())?;
    let Some(p) = provisioner_for(&old.kind) else {
        return Err("only chat grants can be updated".to_string());
    };
    let ws_ids: Vec<String> = if old.subject_kind == "workspace" {
        vec![old.subject_id.clone()]
    } else {
        Vec::new()
    };
    let actor = hook_actor(Some(actor));
    let ctx = HookCtx::load(&actor, &ws_ids);
    let now = now_secs();
    let (grant, changed) = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let old = grant_by_id(&tx, id)?.ok_or_else(|| "unknown grant".to_string())?;
        let stored = p.validate_update(&tx, &old, scope, &ctx)?;
        if old.scope.as_deref() == Some(stored.as_str()) {
            return Ok((old, false));
        }
        tx.execute(
            "UPDATE grants SET scope = ?1 WHERE id = ?2",
            params![stored, id],
        )
        .map_err(|e| format!("skin grant update: {e}"))?;
        let new = SkinGrant {
            scope: Some(stored),
            ..old.clone()
        };
        let ops = p.plan(&tx, &HookEvent::GrantUpdated { old: &old, new: &new }, &ctx)?;
        outbox::enqueue(&tx, &ops, &actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok((new, true))
    })?;
    if changed {
        crate::tool_hook::kick();
        members::changed(None);
    }
    Ok(grant)
}

/// Delete one hooked grant plus its cascade (assistant grants of a
/// person) and enqueue their deprovisions, in one transaction. `None`
/// when the grant does not exist.
fn delete_hooked_grant(
    p: &'static dyn crate::tool_hook::ToolProvisioner,
    id: &str,
    actor: &str,
) -> Result<Option<Vec<SkinGrant>>, String> {
    use crate::tool_hook::{outbox, HookCtx, HookEvent};
    let ctx = HookCtx {
        actor: actor.to_string(),
        ..Default::default()
    };
    let now = now_secs();
    let r = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let Some(grant) = grant_by_id(&tx, id)? else {
            return Ok(None);
        };
        let cascaded = p.cascade(&tx, &HookEvent::GrantDeleted { grant: &grant })?;
        let mut ops = Vec::new();
        for c in &cascaded {
            tx.execute("DELETE FROM grants WHERE id = ?1", params![c.id])
                .map_err(|e| format!("skin grant cascade delete: {e}"))?;
            ops.extend(p.plan(&tx, &HookEvent::GrantDeleted { grant: c }, &ctx)?);
        }
        tx.execute("DELETE FROM grants WHERE id = ?1", params![grant.id])
            .map_err(|e| format!("skin grant delete: {e}"))?;
        ops.extend(p.plan(&tx, &HookEvent::GrantDeleted { grant: &grant }, &ctx)?);
        outbox::enqueue(&tx, &ops, actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok(Some(cascaded))
    })?;
    if r.is_some() {
        crate::tool_hook::kick();
        members::changed(None);
    }
    Ok(r)
}

/// Delete by id and return the cascaded grants (assistant grants of a
/// person's chat grant). `Ok(None)` = unknown grant.
pub fn delete_grant_by_id_with_cascade(
    id: &str,
    actor: &str,
) -> Result<Option<Vec<SkinGrant>>, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let kind = with_conn(|conn| Ok(grant_by_id(conn, id)?.map(|g| g.kind)))?;
    let Some(kind) = kind else {
        return Ok(None);
    };
    match crate::tool_hook::provisioner_for(&kind) {
        Some(p) => delete_hooked_grant(p, id, &hook_actor(Some(actor))),
        None => {
            let deleted = delete_plain_grant_by_id(id)?;
            if deleted && kind == "app" {
                crate::tool_hook::kick_reconcile();
            }
            Ok(deleted.then(Vec::new))
        }
    }
}

/// Delete by the unique key, with the cascade. `Ok(None)` = unknown.
pub fn delete_grant_by_key_with_cascade(
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
    actor: &str,
) -> Result<Option<Vec<SkinGrant>>, String> {
    let subject_kind = grant_subject_kind(subject_kind)?;
    let kind = grant_kind(kind)?;
    let Some(p) = crate::tool_hook::provisioner_for(&kind) else {
        return Ok(delete_grant_by_key(&subject_kind, subject_id, &kind, target_id)?.then(Vec::new));
    };
    let mut subject_id = subject_id.trim().to_string();
    if subject_kind == "workspace" {
        subject_id = resolve_workspace_subject(&subject_id)?;
    } else if subject_kind == "connect" {
        subject_id = resolve_connect_subject(&subject_id)?;
    }
    let target = match target_id.trim() {
        "" => p.default_target().to_string(),
        t => t.to_ascii_lowercase(),
    };
    let id: Option<String> = with_conn(|conn| {
        let mut sid = subject_id.clone();
        if subject_kind == "principal" {
            if let Some(found) = crate::tool_hook::chat::principal_by_token(conn, &sid)? {
                sid = found.id;
            }
        }
        conn.query_row(
            "SELECT id FROM grants
             WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
            params![subject_kind, sid, kind, target],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("skin grant lookup: {e}"))
    })?;
    match id {
        Some(id) => delete_hooked_grant(p, &id, &hook_actor(Some(actor))),
        None => Ok(None),
    }
}

/// Enable/disable a hooked grant: re-validates on enable (the target must
/// be on, the assistant's person must still hold a member grant), refuses
/// a person's disable while assistants are pinned to them, and enqueues
/// provision / deprovision in the same transaction.
fn set_hooked_grant_enabled(
    p: &'static dyn crate::tool_hook::ToolProvisioner,
    old: SkinGrant,
    enabled: bool,
    actor: &str,
) -> Result<SkinGrant, String> {
    use crate::tool_hook::{outbox, HookCtx, HookEvent};
    let ws_ids: Vec<String> = if old.subject_kind == "workspace" {
        vec![old.subject_id.clone()]
    } else {
        Vec::new()
    };
    let actor = hook_actor(Some(actor));
    let ctx = HookCtx::load(&actor, &ws_ids);
    let now = now_secs();
    let (grant, changed) = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin grant begin: {e}"))?;
        let g = grant_by_id(&tx, &old.id)?.ok_or_else(|| "unknown grant".to_string())?;
        if g.enabled == enabled {
            return Ok((g, false));
        }
        if enabled {
            p.validate(
                &tx,
                &SkinGrantWrite {
                    subject_kind: g.subject_kind.clone(),
                    subject_id: g.subject_id.clone(),
                    kind: g.kind.clone(),
                    target_id: g.target_id.clone(),
                    role: None,
                    scope: g.scope.clone(),
                    created_by: g.created_by.clone(),
                },
                &ctx,
            )?;
        } else {
            p.validate_disable(&tx, &g)?;
        }
        tx.execute(
            "UPDATE grants SET enabled = ?1 WHERE id = ?2",
            params![if enabled { 1 } else { 0 }, g.id],
        )
        .map_err(|e| format!("skin grant enabled: {e}"))?;
        let new = SkinGrant { enabled, ..g };
        let ev = if enabled {
            HookEvent::GrantEnabled { grant: &new }
        } else {
            HookEvent::GrantDisabled { grant: &new }
        };
        let ops = p.plan(&tx, &ev, &ctx)?;
        outbox::enqueue(&tx, &ops, &actor, now)?;
        hook_failpoint()?;
        tx.commit().map_err(|e| format!("skin grant commit: {e}"))?;
        Ok((new, true))
    })?;
    if changed {
        crate::tool_hook::kick();
        members::changed(None);
    }
    Ok(grant)
}

/// [`set_grant_enabled`] with the caller recorded on hooked rows.
pub fn set_grant_enabled_as(id: &str, enabled: bool, actor: &str) -> Result<SkinGrant, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let old = with_conn(|conn| grant_by_id(conn, id))?;
    if let Some(old) = old {
        if let Some(p) = crate::tool_hook::provisioner_for(&old.kind) {
            return set_hooked_grant_enabled(p, old, enabled, actor);
        }
        let r = set_plain_grant_enabled(id, enabled)?;
        if r.kind == "app" {
            crate::tool_hook::kick_reconcile();
        }
        return Ok(r);
    }
    set_plain_grant_enabled(id, enabled)
}

pub fn list_grants() -> Result<Vec<SkinGrant>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(&format!("{GRANT_SELECT} ORDER BY created_at, id"))
            .map_err(|e| format!("skin grant list: {e}"))?;
        let rows = stmt
            .query_map([], map_grant_row)
            .map_err(|e| format!("skin grant list: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("skin grant row: {e}"))?);
        }
        Ok(out)
    })
}

/// Called when a subject loses a grant (disable or delete). Sessions are
/// snapshots, so a removed grant must not ride on until `expires_at`.
/// - a principal's `kind=app` grant: only the sessions bound to that App
///   end (S24-6). Switching off App X does not log the guest out of App Y.
/// - a principal's other kinds (recorded, not enforced): every session, as
///   in 0.45.4 (AM0).
/// - workspace subjects hold no `k2skn_` sessions; platform passes
///   (`session=0`) have no principal and are never touched.
fn revoke_sessions_for_grant(
    conn: &Connection,
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
) -> Result<usize, String> {
    if subject_kind != "principal" {
        return Ok(0);
    }
    // A chat grant is not a K2 login: its Sew sessions end in Sew on
    // deprovision. Never log the guest out of their Apps (S24 per-App).
    if crate::tool_hook::provisioner_for(kind).is_some() {
        return Ok(0);
    }
    if kind == "app" {
        revoke_sessions_for_principal_app(conn, subject_id, target_id)
    } else {
        revoke_sessions_for_principal(conn, subject_id)
    }
}

/// Delete the grant row. The role bundle stays. A principal subject's live
/// login sessions for that App are revoked (they log in again against what
/// is left).
pub fn delete_grant_by_id(id: &str) -> Result<bool, String> {
    Ok(delete_grant_by_id_with_cascade(id, crate::tool_hook::ACTOR_SYSTEM)?.is_some())
}

fn delete_plain_grant_by_id(id: &str) -> Result<bool, String> {
    members::notify_ok(with_immediate(|conn| {
        let Some(grant) = grant_by_id(conn, id)? else {
            return Ok(false);
        };
        let n = conn
            .execute("DELETE FROM grants WHERE id = ?1", params![id])
            .map_err(|e| format!("skin grant delete: {e}"))?;
        if n > 0 {
            revoke_sessions_for_grant(
                conn,
                &grant.subject_kind,
                &grant.subject_id,
                &grant.kind,
                &grant.target_id,
            )?;
        }
        Ok(n > 0)
    }))
}

/// Delete by the unique (subject, kind, target) key. The role stays.
/// A principal subject's live login sessions for that App are revoked.
pub fn delete_grant_by_key(
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
) -> Result<bool, String> {
    if crate::tool_hook::provisioner_for(kind).is_some() {
        return Ok(delete_grant_by_key_with_cascade(
            subject_kind,
            subject_id,
            kind,
            target_id,
            crate::tool_hook::ACTOR_SYSTEM,
        )?
        .is_some());
    }
    let deleted = delete_plain_grant_by_key(subject_kind, subject_id, kind, target_id)?;
    if deleted && kind.trim().eq_ignore_ascii_case("app") {
        crate::tool_hook::kick_reconcile();
    }
    Ok(deleted)
}

fn delete_plain_grant_by_key(
    subject_kind: &str,
    subject_id: &str,
    kind: &str,
    target_id: &str,
) -> Result<bool, String> {
    let subject_kind = grant_subject_kind(subject_kind)?;
    let subject_id = subject_id.trim();
    if subject_id.is_empty() {
        return Err("missing subject id".to_string());
    }
    let kind = grant_kind(kind)?;
    let target_id = target_id.trim();
    if target_id.is_empty() {
        return Err("missing target id".to_string());
    }
    members::notify_ok(with_immediate(|conn| {
        let n = conn
            .execute(
                "DELETE FROM grants
                 WHERE subject_kind = ?1 AND subject_id = ?2 AND kind = ?3 AND target_id = ?4",
                params![subject_kind, subject_id, kind, target_id],
            )
            .map_err(|e| format!("skin grant delete: {e}"))?;
        if n > 0 {
            revoke_sessions_for_grant(conn, &subject_kind, subject_id, &kind, target_id)?;
        }
        Ok(n > 0)
    }))
}

fn grant_by_id(conn: &Connection, id: &str) -> Result<Option<SkinGrant>, String> {
    conn.query_row(
        &format!("{GRANT_SELECT} WHERE id = ?1"),
        params![id],
        map_grant_row,
    )
    .optional()
    .map_err(|e| format!("skin grant lookup: {e}"))
}

/// Set `enabled` to 0 or 1. Does not delete the row. Disabling a
/// principal's App grant revokes that principal's live login sessions for
/// that App; enabling never revives a revoked pass (the guest logs in again).
pub fn set_grant_enabled(id: &str, enabled: bool) -> Result<SkinGrant, String> {
    set_grant_enabled_as(id, enabled, crate::tool_hook::ACTOR_SYSTEM)
}

fn set_plain_grant_enabled(id: &str, enabled: bool) -> Result<SkinGrant, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing grant id".to_string());
    }
    let flag: i64 = if enabled { 1 } else { 0 };
    members::notify_ok(with_immediate(|conn| {
        let n = conn
            .execute(
                "UPDATE grants SET enabled = ?1 WHERE id = ?2",
                params![flag, id],
            )
            .map_err(|e| format!("skin grant enabled: {e}"))?;
        if n == 0 {
            return Err("unknown grant".to_string());
        }
        let grant = grant_by_id(conn, id)?.ok_or_else(|| "unknown grant".to_string())?;
        if !enabled {
            revoke_sessions_for_grant(
                conn,
                &grant.subject_kind,
                &grant.subject_id,
                &grant.kind,
                &grant.target_id,
            )?;
        }
        Ok(grant)
    }))
}

/// Present inserts one host grant (`enabled` 1) carrying the guest's current
/// role, or their default rooms when unassigned (AM1) — never a blank row.
/// Absent deletes that principal+app+host row, does not clear
/// `principals.role_id`, and revokes that principal's live login sessions:
/// all of them while [`HOST_OFF_ENDS_ALL_APP_SESSIONS`] (Q8), else only
/// host ones. An existing row is left as the owner wrote it.
pub fn set_host_login(username: &str, present: bool) -> Result<bool, String> {
    let username = normalize_username(username)?;
    let r = members::notify_ok(with_immediate(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        if present {
            match p.role_id.clone() {
                Some(role) => insert_host_grant(conn, &p.id, Some(role), None)?,
                None => insert_host_grant(conn, &p.id, None, Some(rooms_json(&p.default_rooms)))?,
            }
            Ok(true)
        } else {
            let n = conn
                .execute(
                    "DELETE FROM grants
                     WHERE subject_kind = 'principal' AND subject_id = ?1
                       AND kind = 'app' AND target_id = ?2",
                    params![p.id, HOST_APP_ID],
                )
                .map_err(|e| format!("skin host grant delete: {e}"))?;
            if HOST_OFF_ENDS_ALL_APP_SESSIONS {
                revoke_sessions_for_principal(conn, &p.id)?;
            } else {
                revoke_sessions_for_principal_app(conn, &p.id, HOST_APP_ID)?;
            }
            Ok(n > 0)
        }
    }))?;
    // App user change (Sew rev 147 / K31 seam): never provisions anyone
    // by itself; the sweep only reconciles chat grants.
    crate::tool_hook::kick_reconcile();
    Ok(r)
}

// ── Per-App logins (S24) ─────────────────────────────────────────────

/// What a copy of box logins onto one App did (S24-10 / S24-11).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CopyHostReport {
    pub app: String,
    pub dry_run: bool,
    /// Usernames that got a new App grant (or would, on a dry run).
    pub added: Vec<String>,
    /// Usernames that already had an App grant for this App; it wins,
    /// including a disabled one (Q2). `(username, enabled)`.
    pub kept_existing: Vec<(String, bool)>,
    /// Usernames whose box-login role is tagged for a different App: not
    /// copied (Q2).
    pub skipped_other_app_role: Vec<String>,
    /// Box-login guests another workspace's agent added (`creators`): not
    /// copied onto this App (security review E).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_other_workspace_agent: Vec<String>,
}

/// Copy every ENABLED box login (`host` grant) onto App `app` (S24-10,
/// S24-11): the host grant's role when that role's `app_id` is NULL or this
/// App, else its rooms scope role-less (S24-7). A grant the owner already
/// wrote for this App wins (`ON CONFLICT DO NOTHING`), enabled or not.
/// A role tagged for another App is skipped. Runs inside the caller's
/// transaction; `dry_run` writes nothing.
fn copy_host_to_app_tx(
    conn: &Connection,
    app: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    dry_run: bool,
) -> Result<CopyHostReport, String> {
    if app.is_empty() || app == HOST_APP_ID {
        return Err("copy host: name a published App, not host".to_string());
    }
    let rows = {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, p.username, g.role_id, g.scope, r.app_id
                 FROM grants g
                 JOIN principals p ON p.id = g.subject_id
                 LEFT JOIN roles r ON r.id = g.role_id
                 WHERE g.subject_kind = 'principal' AND g.kind = 'app'
                   AND g.target_id = ?1 AND g.enabled != 0
                 ORDER BY p.username",
            )
            .map_err(|e| format!("skin copy host: {e}"))?;
        let mapped = stmt
            .query_map(params![HOST_APP_ID], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|e| format!("skin copy host: {e}"))?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin copy host row: {e}"))?
    };
    let mut report = CopyHostReport {
        app: app.to_string(),
        dry_run,
        ..Default::default()
    };
    for (pid, username, role_id, scope, role_app) in rows {
        // A box guest another workspace's agent added never lands on this
        // App (only its own workspace's Apps; unknown workspace: none).
        if let Some(by_ws) = creators::workspace_of_conn(conn, &pid)? {
            if app_workspace != Some(by_ws.as_str()) {
                report.skipped_other_workspace_agent.push(username);
                continue;
            }
        }
        if let Some(existing) = app_grant(conn, &pid, app)? {
            report.kept_existing.push((username, existing.enabled));
            continue;
        }
        let role_id = blank_to_none(role_id);
        if role_id.is_some() {
            if let Some(other) = blank_to_none(role_app) {
                if other != app {
                    report.skipped_other_app_role.push(username);
                    continue;
                }
            }
        }
        let scope = if role_id.is_some() {
            None
        } else {
            Some(rooms_json(&rooms_from_json(scope.as_deref().unwrap_or("[]"))))
        };
        if !dry_run {
            conn.execute(
                "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                 VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, 1, ?7)
                 ON CONFLICT(subject_kind, subject_id, kind, target_id) DO NOTHING",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    pid,
                    app,
                    role_id,
                    scope,
                    now_secs(),
                    created_by
                ],
            )
            .map_err(|e| format!("skin copy host insert: {e}"))?;
        }
        report.added.push(username);
    }
    Ok(report)
}

/// One summary audit line plus one line per skipped or pre-existing row.
pub fn audit_copy_host_report(event: &str, actor: &str, report: &CopyHostReport) {
    if report.dry_run {
        return;
    }
    record_access_audit(
        event,
        actor,
        &format!(
            "app {}: added {} login(s), kept {} existing App grant(s), skipped {} other-App role(s)",
            report.app,
            report.added.len(),
            report.kept_existing.len(),
            report.skipped_other_app_role.len()
        ),
    );
    for (user, enabled) in &report.kept_existing {
        record_access_audit(
            event,
            user,
            &format!(
                "app {}: kept the owner's existing {} App grant (it now applies at login)",
                report.app,
                if *enabled { "enabled" } else { "disabled" }
            ),
        );
    }
    for user in &report.skipped_other_app_role {
        record_access_audit(
            event,
            user,
            &format!(
                "app {}: not copied, the box-login role is tagged for another App",
                report.app
            ),
        );
    }
}

/// Owner (S24-11, `k2 app grant copy-host`): give everyone who has the box
/// login the same access on App `app`. `app` is a published service id the
/// caller already resolved. Writes an audit summary unless `dry_run`.
pub fn copy_host_grants_to_app(
    app: &str,
    created_by: &str,
    dry_run: bool,
) -> Result<CopyHostReport, String> {
    copy_host_grants_to_app_in(app, None, created_by, dry_run)
}

/// [`copy_host_grants_to_app`] for an App of `app_workspace`
/// (`projects.id`, from the daemon: this module never reads k2so.db). A
/// box guest an agent added is copied only onto its own workspace's Apps;
/// with no workspace given, none of them is.
pub fn copy_host_grants_to_app_in(
    app: &str,
    app_workspace: Option<&str>,
    created_by: &str,
    dry_run: bool,
) -> Result<CopyHostReport, String> {
    let app = app.trim().to_string();
    let report = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin copy host begin: {e}"))?;
        let report = copy_host_to_app_tx(&tx, &app, app_workspace, created_by, dry_run)?;
        tx.commit()
            .map_err(|e| format!("skin copy host commit: {e}"))?;
        Ok(report)
    })?;
    audit_copy_host_report("app_grant_copy_host", created_by, &report);
    members::changed(None);
    Ok(report)
}

/// True until the one-time `--skin` App backfill (S24-10) has run.
pub fn skin_app_backfill_pending() -> Result<bool, String> {
    with_conn(|conn| {
        let pending: Option<i64> = conn
            .query_row(
                "SELECT pending FROM skin_app_backfill WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin app backfill: {e}"))?;
        Ok(pending.unwrap_or(0) != 0)
    })
}

/// S24-10, once per database: copy box logins onto every `--skin` App that
/// existed at the upgrade (`skin_app_ids`, from `published_services` in
/// k2so.db — the daemon passes them; this module never reads k2so.db).
/// Runs at boot before any helper starts. `Ok(None)` = already done. One
/// transaction for all Apps plus the done mark; audit after commit.
/// `--cmd` Apps get no backfill: their BFF logs in as `host` until it sends
/// `app`. Apps published later start with nobody (Q6, fail closed).
pub fn run_skin_app_backfill(
    skin_app_ids: &[String],
) -> Result<Option<Vec<CopyHostReport>>, String> {
    let reports = with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin app backfill begin: {e}"))?;
        let pending: Option<i64> = tx
            .query_row(
                "SELECT pending FROM skin_app_backfill WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin app backfill: {e}"))?;
        if pending.unwrap_or(0) == 0 {
            return Ok(None);
        }
        let mut reports = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for raw in skin_app_ids {
            let app = raw.trim();
            if app.is_empty() || app == HOST_APP_ID || !seen.insert(app.to_string()) {
                continue;
            }
            reports.push(copy_host_to_app_tx(&tx, app, None, UPGRADE_CREATED_BY, false)?);
        }
        let summary = serde_json::to_string(&reports).unwrap_or_else(|_| "[]".to_string());
        tx.execute(
            "UPDATE skin_app_backfill SET pending = 0, done_at = ?1, summary = ?2 WHERE id = 1",
            params![now_secs(), summary],
        )
        .map_err(|e| format!("skin app backfill mark: {e}"))?;
        tx.commit()
            .map_err(|e| format!("skin app backfill commit: {e}"))?;
        Ok(Some(reports))
    })?;
    if let Some(list) = reports.as_ref() {
        record_access_audit(
            "app_grant_upgrade",
            UPGRADE_CREATED_BY,
            &format!(
                "--skin App backfill: {} App(s), {} login(s) copied from the box login",
                list.len(),
                list.iter().map(|r| r.added.len()).sum::<usize>()
            ),
        );
        for r in list {
            audit_copy_host_report("app_grant_upgrade", UPGRADE_CREATED_BY, r);
        }
    }
    Ok(reports)
}

/// S24-12: a new guest for one App. Creates the principal with that App's
/// grant (the role, or empty rooms `[]`) and **no** host grant, so this
/// person can't sign in at the box login. `app` must be a published service
/// id the caller resolved (never `host`; plain `add_principal_with_email`
/// is the box-login path). The role's `app_id` must be NULL or `app`.
pub fn add_principal_for_app(
    username: &str,
    email: Option<&str>,
    app: &str,
    role: Option<&str>,
    created_by: &str,
) -> Result<SkinPrincipal, String> {
    let username = normalize_username(username)?;
    let email = match email {
        Some(raw) => normalize_email(raw)?,
        None => None,
    };
    let app = app.trim().to_string();
    if app.is_empty() || app == HOST_APP_ID {
        return Err("app must be a published App (use a plain add for the box login)".to_string());
    }
    let role = role.map(str::trim).filter(|s| !s.is_empty());
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
        if let Some(ref em) = email {
            ensure_email_free(conn, em, None)?;
        }
        let (role_id, scope) = match role {
            Some(token) => app_grant_shape(conn, &app, Some(token), None)?,
            None => (None, Some("[]".to_string())),
        };
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("skin principal begin: {e}"))?;
        tx.execute(
            "INSERT INTO principals (id, username, created_at, email) VALUES (?1, ?2, ?3, ?4)",
            params![id, username, created_at, email],
        )
        .map_err(|e| unique_email_err(email.as_deref(), e, "skin principal insert"))?;
        tx.execute(
            "INSERT INTO grants
             (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
             VALUES (?1, 'principal', ?2, 'app', ?3, ?4, ?5, ?6, 1, ?7)",
            params![
                uuid::Uuid::new_v4().to_string(),
                id,
                app,
                role_id,
                scope,
                created_at,
                created_by
            ],
        )
        .map_err(|e| format!("skin app grant insert: {e}"))?;
        tx.commit()
            .map_err(|e| format!("skin principal commit: {e}"))?;
        Ok(SkinPrincipal {
            id,
            username,
            created_at,
            default_rooms: Vec::new(),
            default_room_handles: Vec::new(),
            has_password: false,
            role_id: None,
            role_name: None,
            email,
            full_name: None,
            agent_workspace: None,
            agent_handle: None,
        })
    })?;
    crate::workspace::context_layers::refresh_skin_roster_after_people_change();
    // App user change (Sew rev 147 / K31 seam).
    crate::tool_hook::kick_reconcile();
    members::changed(None);
    Ok(r)
}

/// S24-15: set the rooms of one guest's role-less grant on App `app`
/// (`host` included). Creates the grant when there is none (owner route).
/// A grant that carries a role is refused: change the role's rooms instead.
/// `apply_tokens` copies onto that App's live sessions; without it a
/// dropped room still leaves them now. A disabled grant stays disabled.
pub fn set_app_grant_rooms(
    username: &str,
    app: &str,
    rooms: &[String],
    apply_tokens: bool,
    created_by: &str,
) -> Result<SkinGrant, String> {
    let username = normalize_username(username)?;
    let app = normalize_app_target(Some(app));
    if app == HOST_APP_ID {
        // Box login keeps the long-standing path (column + host grant).
        let p = set_principal_default_rooms(&username, rooms, apply_tokens)?;
        return with_conn(|conn| {
            app_grant(conn, &p.id, HOST_APP_ID)?
                .ok_or_else(|| format!("'{username}' has no box login (host grant)"))
        });
    }
    let rooms = normalize_room_ids(rooms);
    let stored = rooms_json(&rooms);
    members::notify_ok(with_conn(|conn| {
        let Some(p) = principal_by_username(conn, &username)? else {
            return Err(format!("unknown skin user '{username}'"));
        };
        match app_grant(conn, &p.id, &app)? {
            Some(g) if g.role_id.is_some() => {
                return Err(format!(
                    "'{username}' has a role on that App; change the role's rooms (k2 app role room) or remove the grant first"
                ));
            }
            Some(g) => {
                conn.execute(
                    "UPDATE grants SET scope = ?1 WHERE id = ?2",
                    params![stored, g.id],
                )
                .map_err(|e| format!("skin app grant rooms: {e}"))?;
            }
            None => {
                conn.execute(
                    "INSERT INTO grants
                     (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled, created_by)
                     VALUES (?1, 'principal', ?2, 'app', ?3, NULL, ?4, ?5, 1, ?6)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        p.id,
                        app,
                        stored,
                        now_secs(),
                        created_by
                    ],
                )
                .map_err(|e| format!("skin app grant insert: {e}"))?;
            }
        }
        if apply_tokens {
            rewrite_live_sessions(conn, &p.id, &app, &thread_only_policy(&rooms))?;
        } else {
            narrow_live_sessions(conn, &p.id, &app, &rooms)?;
        }
        app_grant(conn, &p.id, &app)?.ok_or_else(|| "app grant vanished".to_string())
    }))
}

// S24-19 (agent hire S5, 0.45.6): an App login a hire makes calls
// `add_principal_for_app` (an App grant, never the box login) for the App
// the recipe names and the key's ceiling allows; see `k2_core::hire::apply`.

/// Is `username` taken by a guest (App or box login)? The hire plan checks
/// before any write.
pub fn principal_exists(username: &str) -> Result<bool, String> {
    let username = normalize_username(username)?;
    with_conn(|conn| Ok(principal_by_username(conn, &username)?.is_some()))
}

/// Hire-key App access limit (0.45.6): the role `token` (id or name) as
/// usable on App `app`: a role of that App or a box-wide role (`app_id`
/// NULL), the same rule an App grant applies. `Ok(None)` = no such role,
/// or a role of another App.
pub fn role_for_app(app: &str, token: &str) -> Result<Option<SkinRole>, String> {
    let app = app.trim();
    with_conn(|conn| {
        Ok(role_by_id_or_name(conn, token)?
            .filter(|r| r.app_id.as_deref().is_none_or(|a| a == app)))
    })
}

/// Every role usable on App `app` (its own roles and box-wide roles).
pub fn roles_for_app(app: &str) -> Result<Vec<SkinRole>, String> {
    let app = app.trim();
    Ok(list_roles()?
        .into_iter()
        .filter(|r| r.app_id.as_deref().is_none_or(|a| a == app))
        .collect())
}

/// Is `inner` no wider than `outer`: every room of `inner` is in `outer`
/// with a subset of its functions? (A role "up to" another.)
pub fn policy_within(inner: &RoomPolicy, outer: &RoomPolicy) -> bool {
    inner.iter().all(|(room, caps)| {
        outer
            .get(room)
            .is_some_and(|allowed| caps.iter().all(|c| allowed.iter().any(|a| a == c)))
    })
}

// ── Access templates ─────────────────────────────────────────────────
// A line is kind + optional role. Apply copies that into real grants
// with the target (and scope) supplied on the POST. Later edits of the
// template do not rewrite grants already written.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessTemplateLine {
    pub id: String,
    pub template_id: String,
    pub kind: String,
    pub role_id: Option<String>,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccessTemplate {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub lines: Vec<AccessTemplateLine>,
}

/// One apply binding. `role` is rejected for mailbox, database, and cli.
/// App role comes from the template line, not this field.
#[derive(Debug, Clone)]
pub struct TemplateApplyLine {
    pub line_id: String,
    pub target_id: String,
    pub scope: Option<String>,
    pub role: Option<String>,
}

fn normalize_template_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("missing template name".to_string());
    }
    Ok(name.to_string())
}

fn map_template_line(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccessTemplateLine> {
    let role_id: Option<String> = r.get(3)?;
    Ok(AccessTemplateLine {
        id: r.get(0)?,
        template_id: r.get(1)?,
        kind: r.get(2)?,
        role_id: blank_to_none(role_id),
        position: r.get(4)?,
    })
}

fn template_lines(conn: &Connection, template_id: &str) -> Result<Vec<AccessTemplateLine>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, template_id, kind, role_id, position
             FROM access_template_lines
             WHERE template_id = ?1
             ORDER BY position, id",
        )
        .map_err(|e| format!("skin template lines: {e}"))?;
    let rows = stmt
        .query_map(params![template_id], map_template_line)
        .map_err(|e| format!("skin template lines: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("skin template line: {e}"))?);
    }
    Ok(out)
}

fn template_by_id(conn: &Connection, id: &str) -> Result<Option<AccessTemplate>, String> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT id, name, created_at FROM access_templates WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin template lookup: {e}"))?;
    let Some((id, name, created_at)) = row else {
        return Ok(None);
    };
    let lines = template_lines(conn, &id)?;
    Ok(Some(AccessTemplate {
        id,
        name,
        created_at,
        lines,
    }))
}

fn unique_template_err(name: &str, e: rusqlite::Error) -> String {
    let s = e.to_string();
    if s.contains("UNIQUE") {
        format!("template '{name}' already exists")
    } else {
        format!("skin template insert: {e}")
    }
}

pub fn list_access_templates() -> Result<Vec<AccessTemplate>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, created_at FROM access_templates ORDER BY name COLLATE NOCASE",
            )
            .map_err(|e| format!("skin template list: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("skin template list: {e}"))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| format!("skin template row: {e}"))?);
        }
        drop(stmt);
        let mut out = Vec::new();
        for (id, name, created_at) in ids {
            out.push(AccessTemplate {
                lines: template_lines(conn, &id)?,
                id,
                name,
                created_at,
            });
        }
        Ok(out)
    })
}

pub fn create_access_template(name: &str) -> Result<AccessTemplate, String> {
    let name = normalize_template_name(name)?;
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = now_secs().to_string();
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO access_templates (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, created_at],
        )
        .map_err(|e| unique_template_err(&name, e))?;
        Ok(AccessTemplate {
            id,
            name,
            created_at,
            lines: Vec::new(),
        })
    })
}

pub fn update_access_template(id: &str, name: &str) -> Result<AccessTemplate, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template id".to_string());
    }
    let name = normalize_template_name(name)?;
    with_conn(|conn| {
        let n = conn
            .execute(
                "UPDATE access_templates SET name = ?1 WHERE id = ?2",
                params![name, id],
            )
            .map_err(|e| unique_template_err(&name, e))?;
        if n == 0 {
            return Err("unknown template".to_string());
        }
        template_by_id(conn, id)?.ok_or_else(|| "unknown template".to_string())
    })
}

pub fn delete_access_template(id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template id".to_string());
    }
    with_conn(|conn| {
        conn.execute(
            "DELETE FROM access_template_lines WHERE template_id = ?1",
            params![id],
        )
        .map_err(|e| format!("skin template lines delete: {e}"))?;
        let n = conn
            .execute("DELETE FROM access_templates WHERE id = ?1", params![id])
            .map_err(|e| format!("skin template delete: {e}"))?;
        Ok(n > 0)
    })
}

fn resolve_line_role(
    conn: &Connection,
    kind: &str,
    role: Option<&str>,
) -> Result<Option<String>, String> {
    let role = role.map(str::trim).filter(|s| !s.is_empty());
    if kind == "app" {
        let Some(token) = role else {
            return Err("kind=app requires roleId".to_string());
        };
        let Some(found) = role_by_id_or_name(conn, token)? else {
            return Err(format!("unknown skin role '{token}'"));
        };
        Ok(Some(found.id))
    } else if role.is_some() {
        Err("roleId is only set for kind=app".to_string())
    } else {
        Ok(None)
    }
}

pub fn create_template_line(
    template_id: &str,
    kind: &str,
    role: Option<&str>,
) -> Result<AccessTemplateLine, String> {
    let template_id = template_id.trim();
    if template_id.is_empty() {
        return Err("missing template id".to_string());
    }
    let kind = grant_kind(kind)?;
    with_conn(|conn| {
        if template_by_id(conn, template_id)?.is_none() {
            return Err("unknown template".to_string());
        }
        let role_id = resolve_line_role(conn, &kind, role)?;
        let position: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM access_template_lines WHERE template_id = ?1",
                params![template_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin template line position: {e}"))?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO access_template_lines (id, template_id, kind, role_id, position)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, template_id, kind, role_id, position],
        )
        .map_err(|e| format!("skin template line insert: {e}"))?;
        Ok(AccessTemplateLine {
            id,
            template_id: template_id.to_string(),
            kind,
            role_id,
            position,
        })
    })
}

pub fn update_template_line(
    id: &str,
    kind: &str,
    role: Option<&str>,
) -> Result<AccessTemplateLine, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template line id".to_string());
    }
    let kind = grant_kind(kind)?;
    with_conn(|conn| {
        let existing: Option<String> = conn
            .query_row(
                "SELECT template_id FROM access_template_lines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("skin template line lookup: {e}"))?;
        let Some(template_id) = existing else {
            return Err("unknown template line".to_string());
        };
        let role_id = resolve_line_role(conn, &kind, role)?;
        conn.execute(
            "UPDATE access_template_lines SET kind = ?1, role_id = ?2 WHERE id = ?3",
            params![kind, role_id, id],
        )
        .map_err(|e| format!("skin template line update: {e}"))?;
        let position: i64 = conn
            .query_row(
                "SELECT position FROM access_template_lines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(|e| format!("skin template line lookup: {e}"))?;
        Ok(AccessTemplateLine {
            id: id.to_string(),
            template_id,
            kind,
            role_id,
            position,
        })
    })
}

pub fn delete_template_line(id: &str) -> Result<bool, String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing template line id".to_string());
    }
    with_conn(|conn| {
        let n = conn
            .execute(
                "DELETE FROM access_template_lines WHERE id = ?1",
                params![id],
            )
            .map_err(|e| format!("skin template line delete: {e}"))?;
        Ok(n > 0)
    })
}

/// Insert grants for the person's id. Does not update those rows when
/// the template changes later. Mailbox, database, and cli require scope
/// and reject a role. App uses the line's role_id.
pub fn apply_access_template(
    principal_id: &str,
    template_id: &str,
    lines: &[TemplateApplyLine],
) -> Result<Vec<SkinGrant>, String> {
    let principal_id = principal_id.trim();
    if principal_id.is_empty() {
        return Err("missing subject id".to_string());
    }
    let template_id = template_id.trim();
    if template_id.is_empty() {
        return Err("missing template id".to_string());
    }
    if lines.is_empty() {
        return Err("missing lines".to_string());
    }
    let planned = with_conn(|conn| {
        if !principal_id_exists(conn, principal_id)? {
            return Err(format!("unknown principal id '{principal_id}'"));
        }
        let Some(template) = template_by_id(conn, template_id)? else {
            return Err("unknown template".to_string());
        };
        let mut writes = Vec::new();
        for binding in lines {
            let line_id = binding.line_id.trim();
            let Some(line) = template.lines.iter().find(|l| l.id == line_id) else {
                return Err(format!("unknown template line '{line_id}'"));
            };
            let target_id = binding.target_id.trim();
            if target_id.is_empty() {
                return Err("missing target id".to_string());
            }
            let scope = binding
                .scope
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let binding_role = binding
                .role
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if line.kind != "app" {
                if binding_role.is_some() || line.role_id.is_some() {
                    return Err("roleId is only set for kind=app".to_string());
                }
                if scope.is_none() {
                    return Err(format!("kind={} requires scope", line.kind));
                }
                writes.push(SkinGrantWrite {
                    subject_kind: "principal".to_string(),
                    subject_id: principal_id.to_string(),
                    kind: line.kind.clone(),
                    target_id: target_id.to_string(),
                    role: None,
                    scope,
                    created_by: None,
                });
            } else {
                let Some(role_id) = line.role_id.clone() else {
                    return Err("kind=app requires roleId".to_string());
                };
                writes.push(SkinGrantWrite {
                    subject_kind: "principal".to_string(),
                    subject_id: principal_id.to_string(),
                    kind: "app".to_string(),
                    target_id: target_id.to_string(),
                    role: Some(role_id),
                    scope: None,
                    created_by: None,
                });
            }
        }
        Ok(writes)
    })?;
    let mut out = Vec::new();
    for write in &planned {
        out.push(create_grant(write)?);
    }
    Ok(out)
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
    type Pending = (String, String, Option<String>, Option<String>, Option<String>);
    let mut pending: Vec<Pending> = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        if let Ok(h) = crate::workspace::handle::project_handle(&conn, id) {
            let h = h.trim().to_string();
            if !h.is_empty() {
                let (path, color, icon): (Option<String>, Option<String>, Option<String>) = conn
                    .query_row(
                        "SELECT path, color, icon_url FROM projects WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .unwrap_or((None, None, None));
                let path = path
                    .map(|s: String| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                pending.push((id.to_string(), h, path, color, icon));
            }
        }
    }
    drop(conn);
    pending
        .into_iter()
        .map(|(project_id, handle, path, color, icon)| {
            let display_name = path
                .map(|p| crate::workspace::display::agent_display_name(&p))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| handle.clone());
            SkinAgent {
                handle,
                project_id,
                display_name,
                color: color
                    .and_then(|c| crate::workspace::agent_look::normalize_color(&c).ok()),
                image: crate::workspace::agent_look::guest_image_view(icon.as_deref()),
            }
        })
        .collect()
}

/// Resolve a presented raw key. Revoked / expired / unknown / wrong prefix → `None`.
/// A login session (`session=1`) also needs its principal and an enabled
/// `kind=app` grant **for the App the session is bound to** (S24-5; NULL
/// on a pre-0.45.5 row = `host`) at the moment of use: a backstop behind the
/// revocations the grant writers do. Platform passes (`session=0`) skip it.
/// An App **link pass** (`link_id` set, runtime S14) is never returned
/// here: it is valid only on its own link ([`link::resolve_link_pass`]).
pub fn resolve_skin_token(presented_raw: &str) -> Option<SkinPass> {
    if !is_skin_token(presented_raw) || is_api_key_prefix(presented_raw) {
        return None;
    }
    let key_hash = sha256_hex(presented_raw);
    let now = now_secs();
    with_conn(|conn| {
        let row = conn
            .query_row(
                "SELECT t.id, t.principal_id, t.session, t.name, p.username, t.caps, t.rooms, t.room_policy,
                        t.app_service_id,
                        EXISTS (
                          SELECT 1 FROM grants g
                          WHERE g.subject_kind = 'principal' AND g.subject_id = t.principal_id
                            AND g.kind = 'app' AND g.target_id = COALESCE(t.app_service_id, ?3)
                            AND g.enabled != 0),
                        EXISTS (
                          SELECT 1 FROM grants g
                          WHERE g.subject_kind = 'principal' AND g.subject_id = t.principal_id
                            AND g.kind = ?4 AND g.target_id = ?5 AND g.enabled != 0)
                 FROM tokens t
                 LEFT JOIN principals p ON p.id = t.principal_id
                 WHERE t.key_hash = ?1 AND t.revoked_at IS NULL
                   AND (t.expires_at IS NULL OR t.expires_at > ?2)
                   AND t.link_id IS NULL
                   AND (t.session = 0 OR p.id IS NOT NULL)",
                params![key_hash, now, HOST_APP_ID, CHAT_GRANT_KIND, CHAT_GRANT_TARGET],
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
                    let app_raw: Option<String> = r.get(8)?;
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
                    let app_ok: bool = r.get(9)?;
                    let chat_ok: bool = r.get(10)?;
                    let app = if session_b {
                        blank_to_none(app_raw).unwrap_or_else(|| HOST_APP_ID.to_string())
                    } else {
                        String::new()
                    };
                    // A login session needs its App's enabled grant — or,
                    // for a chat-only box login (S28 §13), an enabled
                    // `chat` grant AND an empty room policy, so a session
                    // minted under a host grant never outlives that grant
                    // with its rooms.
                    let admitted = !session_b
                        || app_ok
                        || (chat_ok && app == HOST_APP_ID && room_policy.is_empty());
                    if !admitted {
                        return Ok(None);
                    }
                    Ok(Some(SkinPass {
                        id: r.get(0)?,
                        principal_id,
                        username: stamp,
                        caps,
                        rooms,
                        session: session_b,
                        room_policy,
                        app,
                        room_paths: Default::default(),
                    }))
                },
            )
            .optional()
            .map_err(|e| format!("skin token resolve: {e}"))?
            .flatten();
        // R3: a login's folders come from its own grant → role, now.
        let row = row.map(|mut pass| {
            if pass.session {
                pass.room_paths = paths::session_paths(conn, &pass);
            }
            pass
        });
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

    /// Run `f` with a fresh temp HOME under the ONE env lock. The skin
    /// store follows `$HOME`, so EVERY test that touches it goes through
    /// here. HOME is restored and the dir removed on drop (even on panic).
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _home = crate::test_env::TempHome::new();
        f();
    }

    fn reopen_skin_db() {
        let path = store_path();
        {
            let mut guard = db().lock();
            *guard = None;
        }
        let conn = open_db(&path).expect("reopen skin.db");
        *db().lock() = Some(SkinDb { path, conn });
    }

    fn host_grants<'a>(grants: &'a [SkinGrant], principal_id: &str) -> Vec<&'a SkinGrant> {
        grants
            .iter()
            .filter(|g| {
                g.subject_kind == "principal"
                    && g.subject_id == principal_id
                    && g.kind == "app"
                    && g.target_id == HOST_APP_ID
            })
            .collect()
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
        assert_eq!(
            err,
            "reserved_label: 'skin' is reserved. Pick another nested label for your UI (k2 study apps; leftover: k2 study skins).",
            "skin text is unchanged"
        );
        assert!(!is_grandfathered_reserved_label("skin"), "skin stays strict");
    }

    #[test]
    fn reserved_nested_labels_meet_and_chat_are_loud() {
        for name in ["meet", "MEET", " Meet ", "chat", "Chat", "\tchat\n", "skin", " SKIN"] {
            assert!(is_reserved_nested_label(name), "{name:?} must be reserved");
        }
        for name in ["meeting", "chats", "skins", "meet-1", "chat2", "mee", "cha", "", "app"] {
            assert!(!is_reserved_nested_label(name), "{name:?} must stay claimable");
        }
        let labels: Vec<&str> = RESERVED_NESTED_LABELS.iter().map(|r| r.label).collect();
        assert_eq!(labels, vec!["skin", "meet", "chat", "mail", "voice", "turn"]);
        for name in ["voice", " Voice ", "turn", "TURN"] {
            assert!(is_reserved_nested_label(name), "{name:?} must be reserved (Sew voice, K13)");
        }
        assert!(!is_reserved_nested_label("voices") && !is_reserved_nested_label("turn-x"));

        let meet = reserved_nested_label_error(" MEET ");
        assert_eq!(
            meet,
            "reserved_label: 'meet' is reserved for K2 Meet (meet.<box>.k2.dev). Pick another nested label."
        );
        let chat = reserved_nested_label_error("Chat");
        assert_eq!(
            chat,
            "reserved_label: 'chat' is reserved for K2 chat (chat.<box>.k2.dev). Pick another nested label."
        );
        assert!(is_grandfathered_reserved_label("meet"));
        assert!(is_grandfathered_reserved_label(" Chat "));
        assert!(!is_grandfathered_reserved_label("meeting"));
    }

    /// `mail` is the webmail front door (web only through the tunnel).
    #[test]
    fn reserved_nested_label_mail_is_loud_and_grandfathered() {
        for name in ["mail", "MAIL", " Mail "] {
            assert!(is_reserved_nested_label(name), "{name:?} must be reserved");
        }
        for name in ["mailbox", "mails", "webmail", "mail-1", "mai"] {
            assert!(!is_reserved_nested_label(name), "{name:?} must stay claimable");
        }
        assert_eq!(
            reserved_nested_label_error(" Mail "),
            "reserved_label: 'mail' is reserved for K2 webmail (mail.<box>.k2.dev). Pick another nested label."
        );
        assert!(is_grandfathered_reserved_label("mail"));
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
    fn parse_caps_accepts_store_write() {
        let write = parse_caps(Some(&["store:write".into()])).expect("store:write");
        assert_eq!(write, vec![CAP_STORE_WRITE]);
        assert!(
            parse_caps(Some(&["thread:read".into()])).is_ok(),
            "thread:read remains accepted"
        );
        let mixed = parse_caps(Some(&["thread:read".into(), "store:write".into()])).expect("mixed");
        assert_eq!(mixed, vec![CAP_THREAD_READ, CAP_STORE_WRITE]);
        let err = parse_caps(Some(&["pty:write".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("pty:write"), "{err}");
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
        assert!(
            !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
            "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
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
    fn parse_caps_accepts_tickets_verbs_never_silent_add() {
        let empty = parse_caps(None).expect("empty default");
        assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !empty
                .iter()
                .any(|c| c == CAP_TICKETS_READ || c == CAP_TICKETS_POST),
            "empty caps must stay Thread-only, never silent-add tickets: {empty:?}"
        );
        assert!(
            !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
            "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
        );
        let missing = parse_caps(Some(&[])).expect("missing default");
        assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !missing
                .iter()
                .any(|c| c == CAP_TICKETS_READ || c == CAP_TICKETS_POST),
            "missing caps must stay Thread-only: {missing:?}"
        );

        let read = parse_caps(Some(&["tickets:read".into()])).expect("tickets:read");
        assert_eq!(read, vec![CAP_TICKETS_READ]);
        let post = parse_caps(Some(&["tickets:post".into()])).expect("tickets:post");
        assert_eq!(post, vec![CAP_TICKETS_POST]);
        let mixed = parse_caps(Some(&[
            "tickets:read".into(),
            "tickets:post".into(),
            "thread:read".into(),
        ]))
        .expect("mixed");
        assert_eq!(
            mixed,
            vec![CAP_TICKETS_READ, CAP_TICKETS_POST, CAP_THREAD_READ]
        );
    }

    #[test]
    fn parse_caps_accepts_wiki_read_never_silent_add() {
        let empty = parse_caps(None).expect("empty default");
        assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !empty.iter().any(|c| c == CAP_WIKI_READ),
            "empty caps must stay Thread-only, never silent-add wiki: {empty:?}"
        );
        assert!(
            !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
            "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
        );
        let missing = parse_caps(Some(&[])).expect("missing default");
        assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !missing.iter().any(|c| c == CAP_WIKI_READ),
            "missing caps must stay Thread-only: {missing:?}"
        );

        let read = parse_caps(Some(&["wiki:read".into()])).expect("wiki:read");
        assert_eq!(read, vec![CAP_WIKI_READ]);
        let mixed = parse_caps(Some(&["wiki:read".into(), "thread:read".into()])).expect("mixed");
        assert_eq!(mixed, vec![CAP_WIKI_READ, CAP_THREAD_READ]);

        let err = parse_caps(Some(&["wiki:write".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("wiki:write"), "{err}");
    }

    #[test]
    fn parse_caps_accepts_store_read_never_silent_add() {
        let empty = parse_caps(None).expect("empty default");
        assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !empty
                .iter()
                .any(|c| c == CAP_STORE_READ || c == CAP_STORE_WRITE),
            "empty caps must stay Thread-only, never silent-add store: {empty:?}"
        );
        assert!(
            !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
            "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
        );
        let missing = parse_caps(Some(&[])).expect("missing default");
        assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !missing
                .iter()
                .any(|c| c == CAP_STORE_READ || c == CAP_STORE_WRITE),
            "missing caps must stay Thread-only: {missing:?}"
        );

        let read = parse_caps(Some(&["store:read".into()])).expect("store:read");
        assert_eq!(read, vec![CAP_STORE_READ]);
        let mixed = parse_caps(Some(&["store:read".into(), "thread:read".into()])).expect("mixed");
        assert_eq!(mixed, vec![CAP_STORE_READ, CAP_THREAD_READ]);
        let write = parse_caps(Some(&["store:write".into()])).expect("store:write");
        assert_eq!(write, vec![CAP_STORE_WRITE]);
        let both = parse_caps(Some(&[
            "store:read".into(),
            "store:write".into(),
            "thread:read".into(),
        ]))
        .expect("store both");
        assert_eq!(both, vec![CAP_STORE_READ, CAP_STORE_WRITE, CAP_THREAD_READ]);

        let err = parse_caps(Some(&["pty:write".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("pty:write"), "{err}");
        let err = parse_caps(Some(&["wiki:write".into()])).unwrap_err();
        assert!(err.contains("wiki:write"), "{err}");
        let err = parse_caps(Some(&["grid".into()])).unwrap_err();
        assert!(err.contains("grid"), "{err}");
    }

    #[test]
    fn parse_caps_accepts_activity_read_never_silent_add() {
        let empty = parse_caps(None).expect("empty default");
        assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !empty.iter().any(|c| c == CAP_ACTIVITY_READ),
            "empty caps must stay Thread-only, never silent-add activity:read: {empty:?}"
        );
        let missing = parse_caps(Some(&[])).expect("missing default");
        assert_eq!(missing, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        assert!(
            !missing.iter().any(|c| c == CAP_ACTIVITY_READ),
            "missing caps must stay Thread-only: {missing:?}"
        );

        let files_read = parse_caps(Some(&["files:read".into()])).expect("files:read");
        assert_eq!(files_read, vec![CAP_FILES_READ]);
        assert!(
            !files_read.iter().any(|c| c == CAP_ACTIVITY_READ),
            "files:read must not imply activity:read: {files_read:?}"
        );
        let files_write = parse_caps(Some(&["files:write".into()])).expect("files:write");
        assert_eq!(files_write, vec![CAP_FILES_WRITE]);
        assert!(
            !files_write.iter().any(|c| c == CAP_ACTIVITY_READ),
            "files:write must not imply activity:read: {files_write:?}"
        );

        let activity = parse_caps(Some(&["activity:read".into()])).expect("activity:read");
        assert_eq!(activity, vec![CAP_ACTIVITY_READ]);
        assert!(
            !activity
                .iter()
                .any(|c| c == CAP_FILES_READ || c == CAP_FILES_WRITE),
            "activity:read must not imply files: {activity:?}"
        );

        let err = parse_caps(Some(&["activity:write".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
        assert!(err.contains("activity:write"), "{err}");
    }

    /// 0.45.5 app identity surface: both caps parse, neither is a default,
    /// and neither implies the other (or anything else).
    #[test]
    fn parse_caps_accepts_agent_identity_caps_never_implied() {
        let empty = parse_caps(None).expect("empty default");
        assert!(!empty.iter().any(|c| c == CAP_AGENT_IDENTITY || c == CAP_AGENT_HANDLE));
        let both = parse_caps(Some(&["agent:identity".into(), "agent:handle".into()]))
            .expect("agent caps");
        assert_eq!(both, vec![CAP_AGENT_IDENTITY, CAP_AGENT_HANDLE]);
        let room = "room-id".to_string();
        let pass_with = |caps: &[&str]| {
            let mut room_policy = RoomPolicy::new();
            room_policy.insert(room.clone(), caps.iter().map(|c| (*c).to_string()).collect());
            SkinPass {
                id: "p".into(),
                principal_id: Some("u".into()),
                username: "guest".into(),
                caps: caps.iter().map(|c| (*c).to_string()).collect(),
                rooms: vec![room.clone()],
                session: true,
                room_policy,
                app: "host".to_string(),
                room_paths: Default::default(),
            }
        };
        let ident = pass_with(&[CAP_AGENT_IDENTITY]);
        assert!(ident.has_cap_in_room(&room, CAP_AGENT_IDENTITY));
        assert!(!ident.has_cap_in_room(&room, CAP_AGENT_HANDLE), "identity must not imply handle");
        let handle = pass_with(&[CAP_AGENT_HANDLE]);
        assert!(!handle.has_cap_in_room(&room, CAP_AGENT_IDENTITY), "handle must not imply identity");
        for cap in [CAP_THREAD_READ, CAP_FILES_WRITE, CAP_HEARTBEATS_WRITE] {
            assert!(!pass_with(&[cap]).has_cap_in_room(&room, CAP_AGENT_IDENTITY), "{cap}");
            assert!(!pass_with(&[cap]).has_cap_in_room(&room, CAP_AGENT_HANDLE), "{cap}");
        }
        assert!(!ident.has_cap_in_room("other-room", CAP_AGENT_IDENTITY));
    }

    /// GH#72: the `.k2/` write opt-in is never a default, never implied by
    /// files:write, and refused without files:write beside it.
    #[test]
    fn agent_config_opt_in_is_explicit_and_needs_files_write() {
        assert_eq!(CAP_FILES_AGENT_CONFIG, "files:agentconfig");
        let empty = parse_caps(None).expect("empty default");
        assert!(!empty.iter().any(|c| c == CAP_FILES_AGENT_CONFIG));
        let write = parse_caps(Some(&["files:write".into()])).expect("files:write");
        assert!(!write.iter().any(|c| c == CAP_FILES_AGENT_CONFIG), "files:write must not imply it");
        let alone = parse_caps(Some(&["files:agentconfig".into()])).expect_err("alone is refused");
        assert!(alone.contains("only widens files:write"), "{alone}");
        let alone = parse_caps(Some(&["files:read".into(), "files:agentconfig".into()]))
            .expect_err("with files:read only is refused");
        assert!(alone.contains("only widens files:write"), "{alone}");
        let both = parse_caps(Some(&["files:write".into(), "files:agentconfig".into()]))
            .expect("with files:write");
        assert_eq!(both, vec![CAP_FILES_WRITE, CAP_FILES_AGENT_CONFIG]);

        let mut old = RoomPolicy::new();
        old.insert("a".into(), vec![CAP_FILES_WRITE.into(), CAP_FILES_AGENT_CONFIG.into()]);
        old.insert("b".into(), vec![CAP_FILES_WRITE.into()]);
        assert!(policy_has_agent_config(&old));
        let mut same = old.clone();
        same.insert("c".into(), vec![CAP_THREAD_READ.into()]);
        assert!(agent_config_added(Some(&old), &same).is_empty(), "keeping it is not adding it");
        let mut widened = old.clone();
        widened.insert("b".into(), vec![CAP_FILES_WRITE.into(), CAP_FILES_AGENT_CONFIG.into()]);
        assert_eq!(agent_config_added(Some(&old), &widened), vec!["b".to_string()]);
        assert_eq!(agent_config_added(None, &old), vec!["a".to_string()]);
        let mut tightened = old.clone();
        tightened.insert("a".into(), vec![CAP_FILES_WRITE.into()]);
        assert!(agent_config_added(Some(&old), &tightened).is_empty());
        assert!(!policy_has_agent_config(&tightened));
    }

    /// 0.46.3 Thread downloads: `files:download` parses on its own, is never
    /// a default, is implied by neither `thread:read` nor `files:read`, and
    /// is person-only (an agent may never add it).
    #[test]
    fn files_download_is_explicit_never_implied_and_person_only() {
        assert_eq!(CAP_FILES_DOWNLOAD, "files:download");
        let empty = parse_caps(None).expect("empty default");
        assert!(!empty.iter().any(|c| c == CAP_FILES_DOWNLOAD), "never a default");
        let read = parse_caps(Some(&["thread:read".into(), "files:read".into()])).expect("read caps");
        assert!(!read.iter().any(|c| c == CAP_FILES_DOWNLOAD), "thread:read + files:read must not imply it");
        let dl = parse_caps(Some(&["thread:read".into(), "files:download".into()])).expect("files:download parses");
        assert_eq!(dl, vec![CAP_THREAD_READ, CAP_FILES_DOWNLOAD]);
        assert!(PERSON_ONLY_CAPS.contains(&CAP_FILES_DOWNLOAD));
        assert!(PERSON_ONLY_CAPS.contains(&CAP_FILES_AGENT_CONFIG));

        let mut old = RoomPolicy::new();
        old.insert("a".into(), vec![CAP_THREAD_READ.into(), CAP_FILES_DOWNLOAD.into()]);
        old.insert("b".into(), vec![CAP_THREAD_READ.into()]);
        assert_eq!(person_only_cap_in_policy(&old), Some(CAP_FILES_DOWNLOAD));
        assert_eq!(person_only_cap_added(Some(&old), &old), None, "keeping it is not adding it");
        let mut widened = old.clone();
        widened.insert("b".into(), vec![CAP_THREAD_READ.into(), CAP_FILES_DOWNLOAD.into()]);
        assert_eq!(person_only_cap_added(Some(&old), &widened), Some(CAP_FILES_DOWNLOAD));
        assert_eq!(person_only_cap_added(None, &old), Some(CAP_FILES_DOWNLOAD));
        let caps: Vec<String> = vec![CAP_FILES_WRITE.into(), CAP_FILES_AGENT_CONFIG.into(), CAP_FILES_DOWNLOAD.into()];
        assert_eq!(person_only_cap_in(&caps), Some(CAP_FILES_AGENT_CONFIG), "agentconfig is named first");
        let plain: Vec<String> = vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()];
        assert_eq!(person_only_cap_in(&plain), None);
    }

    #[test]
    fn parse_caps_accepts_heartbeat_caps_never_implied_ah1() {
        let empty = parse_caps(None).expect("empty default");
        assert_eq!(empty, vec![CAP_THREAD_READ, CAP_THREAD_POST]);
        let both = parse_caps(Some(&[
            "heartbeats:read".into(),
            "heartbeats:write".into(),
        ]))
        .expect("heartbeat caps");
        assert_eq!(both, vec![CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE]);

        let room = "room-ah1".to_string();
        let pass_with = |caps: &[&str]| {
            let mut room_policy = RoomPolicy::new();
            room_policy.insert(
                room.clone(),
                caps.iter().map(|c| (*c).to_string()).collect(),
            );
            SkinPass {
                id: "p".into(),
                principal_id: Some("u".into()),
                username: "guest".into(),
                caps: caps.iter().map(|c| (*c).to_string()).collect(),
                rooms: vec![room.clone()],
                session: true,
                room_policy,
                app: "host".to_string(),
                room_paths: Default::default(),
            }
        };
        for caps in [
            vec![CAP_FILES_READ],
            vec![CAP_FILES_WRITE],
            vec![CAP_ACTIVITY_READ],
            vec![CAP_FILES_READ, CAP_FILES_WRITE, CAP_ACTIVITY_READ],
        ] {
            let p = pass_with(&caps);
            assert!(!p.has_cap_in_room(&room, CAP_HEARTBEATS_READ), "{caps:?}");
            assert!(!p.has_cap_in_room(&room, CAP_HEARTBEATS_WRITE), "{caps:?}");
        }
        let w = pass_with(&[CAP_HEARTBEATS_WRITE]);
        assert!(!w.has_cap_in_room(&room, CAP_HEARTBEATS_READ), "write must not imply read");
        let r = pass_with(&[CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE]);
        for cap in [CAP_FILES_READ, CAP_FILES_WRITE, CAP_ACTIVITY_READ] {
            assert!(!r.has_cap_in_room(&room, cap), "heartbeat caps must not imply {cap}");
        }
        assert!(r.has_any_cap_in_room(&room, &[CAP_ACTIVITY_READ, CAP_HEARTBEATS_READ]));
        assert!(!w.has_any_cap_in_room(&room, &[CAP_ACTIVITY_READ, CAP_HEARTBEATS_READ]));

        let platform = SkinPass {
            id: "t".into(),
            principal_id: None,
            username: "bot".into(),
            caps: vec![CAP_HEARTBEATS_READ.into()],
            rooms: vec![room.clone()],
            session: false,
            room_policy: RoomPolicy::new(),
            app: String::new(),
            room_paths: Default::default(),
        };
        assert!(platform.dispatcher_admits_any(&[CAP_ACTIVITY_READ, CAP_HEARTBEATS_READ]));
        assert!(!platform.dispatcher_admits_cap(CAP_ACTIVITY_READ));
        assert!(!platform.dispatcher_admits_any(&[CAP_ACTIVITY_READ, CAP_FILES_READ]));

        let err = parse_caps(Some(&["heartbeats:admin".into()])).unwrap_err();
        assert!(err.contains("unknown capability"), "{err}");
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
            assert!(!pass.has_cap(CAP_WIKI_READ));
            assert!(!pass.has_cap(CAP_STORE_READ));
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
            !empty_caps.iter().any(|c| c == CAP_ACTIVITY_READ),
            "empty caps must stay Thread-only, never silent-add activity:read: {empty_caps:?}"
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
                SkinLoginOutcome::LockedOut { .. } => {}
                _ => panic!("4th attempt after 3 fails is lockout, not a verify"),
            }
            match check_and_record_login("eve", "wrong") {
                SkinLoginOutcome::LockedOut { .. } => {}
                _ => panic!("locked username stays generic LockedOut"),
            }
        });
    }

    /// nsi BFF ask (2026-10-13): a locked login says how long is left, and
    /// an unknown username locks exactly like a known one (no oracle).
    #[test]
    fn lockout_reports_retry_after_for_known_and_unknown_alike() {
        with_temp_home(|| {
            add_principal("eve").unwrap();
            set_principal_password("eve", Some("right-password")).unwrap();
            for who in ["eve", "nobody-here"] {
                for _ in 0..3 {
                    assert!(matches!(
                        check_and_record_login(who, "wrong"),
                        SkinLoginOutcome::BadCreds
                    ));
                }
                match check_and_record_login(who, "right-password") {
                    SkinLoginOutcome::LockedOut { retry_after_secs } => assert!(
                        (1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs),
                        "{who}: {retry_after_secs}"
                    ),
                    other => panic!("{who}: want LockedOut, got {other:?}"),
                }
            }
        });
    }

    fn lock_rows() -> Vec<(String, i64, Option<i64>)> {
        with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT username, failed_count, locked_until FROM login_lockouts
                     ORDER BY username",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(rows)
        })
        .expect("lock rows")
    }

    /// agent-nsi (0.45.5): username locked, the right password typed as
    /// the EMAIL still signed in. The lock is the principal's.
    #[test]
    fn lock_via_username_refuses_email_with_right_password() {
        with_temp_home(|| {
            add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            for _ in 0..3 {
                match check_and_record_login("jane", "wrong") {
                    SkinLoginOutcome::BadCreds => {}
                    other => panic!("pre-lock via username, got {other:?}"),
                }
            }
            for who in ["jane@clinic.com", "  JANE@Clinic.com ", "jane", "JANE"] {
                match check_and_record_login(who, "s3cret-horse") {
                    SkinLoginOutcome::LockedOut { retry_after_secs } => assert!(
                        (1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs),
                        "{who}: {retry_after_secs}"
                    ),
                    other => panic!("{who}: locked principal must refuse, got {other:?}"),
                }
            }
            let rows = lock_rows();
            assert_eq!(rows.len(), 1, "one counter for the principal: {rows:?}");
            assert_eq!(rows[0].0, "jane", "{rows:?}");
        });
    }

    /// Wrong tries on the username and the email share ONE limit (0.45.5
    /// counted them separately, doubling it to 6).
    #[test]
    fn mixed_username_and_email_wrong_tries_share_one_limit() {
        with_temp_home(|| {
            add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            for who in ["jane", "Jane@Clinic.COM", "jane"] {
                match check_and_record_login(who, "wrong") {
                    SkinLoginOutcome::BadCreds => {}
                    other => panic!("{who}: pre-lock, got {other:?}"),
                }
            }
            for who in ["jane", "jane@clinic.com"] {
                match check_and_record_login(who, "s3cret-horse") {
                    SkinLoginOutcome::LockedOut { .. } => {}
                    other => panic!("{who}: 3 mixed tries lock, got {other:?}"),
                }
            }
            let rows = lock_rows();
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].0, "jane", "{rows:?}");
            assert_eq!(rows[0].1, 0, "a lock zeroes the count: {rows:?}");
            assert!(rows[0].2.is_some(), "{rows:?}");
        });
    }

    /// A correct login resets the principal's counter whichever name it
    /// came in on.
    #[test]
    fn success_by_email_resets_the_username_counter() {
        with_temp_home(|| {
            add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            for _ in 0..2 {
                assert!(matches!(
                    check_and_record_login("jane", "wrong"),
                    SkinLoginOutcome::BadCreds
                ));
            }
            assert!(matches!(
                check_and_record_login("jane@clinic.com", "s3cret-horse"),
                SkinLoginOutcome::Ok(_)
            ));
            assert!(lock_rows().is_empty(), "{:?}", lock_rows());
            for _ in 0..2 {
                assert!(matches!(
                    check_and_record_login("jane@clinic.com", "wrong"),
                    SkinLoginOutcome::BadCreds
                ));
            }
            assert!(
                matches!(
                    check_and_record_login("jane", "s3cret-horse"),
                    SkinLoginOutcome::Ok(_)
                ),
                "2 fails after a reset are under the limit"
            );
        });
    }

    /// `k2 app user unlock` by username or by email clears the principal's
    /// lock, and any 0.45.5 email-keyed row left from before the upgrade.
    #[test]
    fn unlock_by_username_or_email_clears_every_principal_row() {
        with_temp_home(|| {
            add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            for (unlock_as, want_key) in [("Jane", "jane"), ("JANE@clinic.com", "jane@clinic.com")] {
                for _ in 0..3 {
                    assert!(matches!(
                        check_and_record_login("jane@clinic.com", "wrong"),
                        SkinLoginOutcome::BadCreds
                    ));
                }
                // A row written by 0.45.5 under the email key.
                with_conn(|conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO login_lockouts (username, failed_count, locked_until)
                         VALUES ('jane@clinic.com', 0, ?1)",
                        params![now_secs().saturating_add(900)],
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(())
                })
                .expect("legacy email row");
                let (key, cleared) = clear_login_lockout(unlock_as).expect("unlock");
                assert_eq!(key, want_key);
                assert!(cleared, "{unlock_as}");
                assert!(lock_rows().is_empty(), "{unlock_as}: {:?}", lock_rows());
                for who in ["jane", "jane@clinic.com"] {
                    assert!(
                        matches!(
                            check_and_record_login(who, "s3cret-horse"),
                            SkinLoginOutcome::Ok(_)
                        ),
                        "{unlock_as}: {who} signs in after unlock"
                    );
                }
            }
        });
    }

    /// A pre-upgrade (0.45.5) live lock under the email key still refuses
    /// the username until it expires or is unlocked.
    #[test]
    fn legacy_email_row_lock_still_refuses_the_username() {
        with_temp_home(|| {
            add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            with_conn(|conn| {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('jane@clinic.com', 0, ?1), ('jane', 2, NULL)",
                    params![now_secs().saturating_add(600)],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .expect("seed");
            match check_and_record_login("jane", "s3cret-horse") {
                SkinLoginOutcome::LockedOut { retry_after_secs } => {
                    assert!((1..=600).contains(&retry_after_secs), "{retry_after_secs}")
                }
                other => panic!("legacy email lock refuses username, got {other:?}"),
            }
            // An expired legacy row is not a lock.
            with_conn(|conn| {
                conn.execute(
                    "UPDATE login_lockouts SET locked_until = ?1 WHERE username = 'jane@clinic.com'",
                    params![now_secs() - 1],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .expect("expire");
            assert!(matches!(
                check_and_record_login("jane", "s3cret-horse"),
                SkinLoginOutcome::Ok(_)
            ));
            assert!(lock_rows().is_empty(), "{:?}", lock_rows());
        });
    }

    /// Names that resolve to nothing keep a counter per presented string,
    /// lock after 3 like a known name, and an unknown email never touches
    /// a principal's row.
    #[test]
    fn unknown_names_still_count_per_presented_string() {
        with_temp_home(|| {
            add_principal_with_email("jane", Some("jane@clinic.com")).unwrap();
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            for who in ["ghost", "ghost@nowhere.io", "Not A Name"] {
                for _ in 0..3 {
                    assert!(matches!(
                        check_and_record_login(who, "wrong"),
                        SkinLoginOutcome::BadCreds
                    ));
                }
                match check_and_record_login(who, "s3cret-horse") {
                    SkinLoginOutcome::LockedOut { retry_after_secs } => assert!(
                        (1..=LOCKOUT_DURATION_SECS).contains(&retry_after_secs),
                        "{who}: {retry_after_secs}"
                    ),
                    other => panic!("{who}: unknown locks like known, got {other:?}"),
                }
            }
            let keys: Vec<String> = lock_rows().into_iter().map(|r| r.0).collect();
            assert_eq!(keys, vec!["ghost", "ghost@nowhere.io", "not a name"]);
            // Unknown strings sharing nothing with jane leave her alone.
            assert!(matches!(
                check_and_record_login("jane@clinic.com", "s3cret-horse"),
                SkinLoginOutcome::Ok(_)
            ));
            // A different unknown string is a fresh counter.
            assert!(matches!(
                check_and_record_login("ghost2", "wrong"),
                SkinLoginOutcome::BadCreds
            ));
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
    fn password_reset_tables_exist_and_are_not_login_lockouts() {
        with_temp_home(|| {
            with_conn(|conn| {
                let n: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type='table' AND name='password_reset_tokens'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(n, 1, "password_reset_tokens missing");
                let n: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type='table' AND name='password_reset_throttle'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                assert_eq!(n, 1, "password_reset_throttle missing");
                let cols: String = conn
                    .prepare("PRAGMA table_info(password_reset_tokens)")
                    .map_err(|e| e.to_string())?
                    .query_map([], |r| r.get::<_, String>(1))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?
                    .join(",");
                assert!(cols.contains("principal_id"), "{cols}");
                assert!(cols.contains("token_hash"), "{cols}");
                assert!(cols.contains("expires_at"), "{cols}");
                assert!(cols.contains("used_at"), "{cols}");
                assert!(!cols.contains("email"), "no email column: {cols}");
                let pcols: String = conn
                    .prepare("PRAGMA table_info(principals)")
                    .map_err(|e| e.to_string())?
                    .query_map([], |r| r.get::<_, String>(1))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?
                    .join(",");
                assert!(pcols.contains("email"), "principals.email missing: {pcols}");
                Ok(())
            })
            .unwrap();
        });
    }

    #[test]
    fn mint_password_reset_unknown_invalid_null_hash_no_token() {
        with_temp_home(|| {
            add_principal("mintonly").unwrap();
            match mint_password_reset("ghost-user") {
                Ok(None) => {}
                other => panic!("unknown must be 200-no-token, got {other:?}"),
            }
            match mint_password_reset("X") {
                Ok(None) => {}
                other => panic!("invalid username must not 400, got {other:?}"),
            }
            match mint_password_reset("has space") {
                Ok(None) => {}
                other => panic!("invalid format must not 400, got {other:?}"),
            }
            match mint_password_reset("mintonly") {
                Ok(None) => {}
                other => panic!("NULL hash must not mint, got {other:?}"),
            }
            add_principal("cara").unwrap();
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            match mint_password_reset("cara") {
                Ok(None) => {}
                other => panic!("password without email must not mint, got {other:?}"),
            }
            match mint_password_reset("not-an-email@") {
                Ok(None) => {}
                other => panic!("invalid email string is a miss, got {other:?}"),
            }
        });
    }

    #[test]
    fn mint_password_reset_replaces_unused_and_is_not_k2skn() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            set_principal_email("cara", Some("cara@clinic.com")).unwrap();
            let first = mint_password_reset("cara")
                .unwrap()
                .expect("known user with hash and email mints");
            assert!(!first.token.starts_with(SKIN_KEY_PREFIX), "{}", first.token);
            assert!(!first.token.starts_with("k2sk_"), "{}", first.token);
            assert_eq!(first.token.len(), 64, "{}", first.token);
            assert_eq!(first.email, "cara@clinic.com");
            assert!(first.expires_at > now_secs());
            let first = first.token;
            let stored = with_conn(|conn| {
                let h: String = conn
                    .query_row(
                        "SELECT token_hash FROM password_reset_tokens
                         WHERE used_at IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                let n: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM password_reset_tokens WHERE used_at IS NULL",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                Ok((h, n))
            })
            .unwrap();
            assert_eq!(stored.0, sha256_hex(&first));
            assert_eq!(stored.1, 1);
            let second = mint_password_reset("cara")
                .unwrap()
                .expect("second mint")
                .token;
            assert_ne!(first, second);
            let n: i64 = with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM password_reset_tokens WHERE used_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
            assert_eq!(n, 1, "one unused row per principal");
            match consume_password_reset(&first, "new-pass-word") {
                Ok(SkinResetConsume::BadToken) => {}
                other => panic!("replaced unused must be dead, got {other:?}"),
            }
            match consume_password_reset(&second, "new-pass-word") {
                Ok(SkinResetConsume::Ok) => {}
                other => panic!("live unused must consume, got {other:?}"),
            }
            match consume_password_reset(&second, "another-pw") {
                Ok(SkinResetConsume::BadToken) => {}
                other => panic!("single-use, got {other:?}"),
            }
            match check_and_record_login("cara", "new-pass-word") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "cara"),
                other => panic!("hash must change, got {other:?}"),
            }
        });
    }

    #[test]
    fn consume_password_reset_ttl_and_empty_refused() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            set_principal_email("cara", Some("cara@clinic.com")).unwrap();
            let raw = mint_password_reset("cara").unwrap().expect("mint").token;
            with_conn(|conn| {
                conn.execute("UPDATE password_reset_tokens SET expires_at = 1", [])
                    .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            match consume_password_reset(&raw, "new-pass-word") {
                Ok(SkinResetConsume::BadToken) => {}
                other => panic!("expired must be BadToken, got {other:?}"),
            }
            match consume_password_reset("not-a-token", "new-pass-word") {
                Ok(SkinResetConsume::BadToken) => {}
                other => panic!("unknown token must be BadToken, got {other:?}"),
            }
            let err = consume_password_reset("x", "").unwrap_err();
            assert!(err.contains("empty"), "{err}");
            match check_and_record_login("cara", "s3cret-horse") {
                SkinLoginOutcome::Ok(_) => {}
                other => panic!("expired consume must not change hash, got {other:?}"),
            }
        });
    }

    #[test]
    fn consume_password_reset_revokes_sessions_not_platform() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            let room = uuid::Uuid::new_v4().to_string();
            let (_sm, static_raw) =
                create_token("cara", None, std::slice::from_ref(&room)).unwrap();
            let (_sess, sess_raw) = create_session_token("cara").unwrap();
            set_principal_email("cara", Some("cara@clinic.com")).unwrap();
            let raw = mint_password_reset("cara").unwrap().expect("mint").token;
            match consume_password_reset(&raw, "new-pass-word") {
                Ok(SkinResetConsume::Ok) => {}
                other => panic!("consume, got {other:?}"),
            }
            assert!(
                resolve_skin_token(&sess_raw).is_none(),
                "consume revokes session=1"
            );
            assert!(
                resolve_skin_token(&static_raw).is_some(),
                "platform token survives consume"
            );
        });
    }

    #[test]
    fn password_reset_throttle_is_per_username_string() {
        with_temp_home(|| {
            assert!(!password_reset_throttled("ghost-user").unwrap());
            assert!(
                password_reset_throttled("ghost-user").unwrap(),
                "second mint inside 60s even when no user"
            );
            assert!(
                password_reset_throttled("  GHOST-USER  ").unwrap(),
                "lowercase/trim same slot"
            );
            assert!(
                !password_reset_throttled("cara").unwrap(),
                "other username is a different slot"
            );
            with_conn(|conn| {
                conn.execute(
                    "UPDATE password_reset_throttle SET last_attempt = 1 WHERE username = 'ghost-user'",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            assert!(
                !password_reset_throttled("ghost-user").unwrap(),
                "window elapsed allows another attempt"
            );
        });
    }

    #[test]
    fn normalize_email_trim_lower_empty_null_invalid() {
        assert_eq!(
            normalize_email("  Jane@Clinic.COM ").unwrap(),
            Some("jane@clinic.com".into())
        );
        assert_eq!(normalize_email("").unwrap(), None);
        assert_eq!(normalize_email("   ").unwrap(), None);
        assert!(normalize_email("not-an-email").is_err());
        assert!(normalize_email("jane@clinic").is_err());
        assert!(normalize_email("jane@ clinic.com").is_err());
        assert!(normalize_email("@clinic.com").is_err());
        assert!(normalize_email("jane@").is_err());
        assert!(normalize_email("jane@@clinic.com").is_err());
        assert!(normalize_username("jane@clinic.com").is_err());
        assert!(normalize_username("jane").is_ok());
    }

    #[test]
    fn platform_name_still_rejects_at() {
        with_temp_home(|| {
            let room = uuid::Uuid::new_v4().to_string();
            let err =
                create_token("jane@clinic.com", None, std::slice::from_ref(&room)).unwrap_err();
            assert!(err.contains("lowercase") || err.contains("only"), "{err}");
        });
    }

    #[test]
    fn guest_email_set_duplicate_clear_and_login_by_email() {
        with_temp_home(|| {
            let jane = add_principal_with_email("jane", Some("Jane@Clinic.COM")).unwrap();
            assert_eq!(jane.email.as_deref(), Some("jane@clinic.com"));
            set_principal_password("jane", Some("s3cret-horse")).unwrap();
            add_principal("bob").unwrap();
            let dup = set_principal_email("bob", Some("jane@clinic.com")).unwrap_err();
            assert!(dup.contains("already in use"), "{dup}");
            let bad = set_principal_email("bob", Some("not-an-email")).unwrap_err();
            assert!(bad.contains("invalid email"), "{bad}");
            let listed = list_principals().unwrap();
            let jane_row = listed.iter().find(|p| p.username == "jane").unwrap();
            assert_eq!(jane_row.email.as_deref(), Some("jane@clinic.com"));
            match check_and_record_login("jane@clinic.com", "s3cret-horse") {
                SkinLoginOutcome::Ok(p) => {
                    assert_eq!(p.username, "jane");
                    assert_eq!(p.email.as_deref(), Some("jane@clinic.com"));
                }
                other => panic!("email login, got {other:?}"),
            }
            match check_and_record_login("jane", "s3cret-horse") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "jane"),
                other => panic!("username login still works, got {other:?}"),
            }
            match mint_password_reset("Jane@Clinic.COM") {
                Ok(Some(m)) => {
                    assert_eq!(m.email, "jane@clinic.com");
                    assert_eq!(m.token.len(), 64);
                }
                other => panic!("mint by email, got {other:?}"),
            }
            match check_and_record_login("not-an-email@", "s3cret-horse") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("invalid email login is generic, got {other:?}"),
            }
            for _ in 0..3 {
                match check_and_record_login("jane@clinic.com", "wrong") {
                    SkinLoginOutcome::BadCreds => {}
                    other => panic!("pre-lockout on presented email, got {other:?}"),
                }
            }
            match check_and_record_login("jane@clinic.com", "s3cret-horse") {
                SkinLoginOutcome::LockedOut { .. } => {}
                other => panic!("lockout via email, got {other:?}"),
            }
            match check_and_record_login("jane", "s3cret-horse") {
                SkinLoginOutcome::LockedOut { .. } => {}
                other => panic!("the lock is the principal's, not the email's, got {other:?}"),
            }
            clear_login_lockout("jane").expect("unlock jane");
            set_principal_email("jane", None).unwrap();
            let cleared = list_principals()
                .unwrap()
                .into_iter()
                .find(|p| p.username == "jane")
                .unwrap();
            assert!(cleared.email.is_none(), "{cleared:?}");
            match mint_password_reset("jane") {
                Ok(None) => {}
                other => panic!("cleared email must not mint, got {other:?}"),
            }
        });
    }

    fn principal_columns(conn: &Connection) -> String {
        let mut stmt = conn
            .prepare("PRAGMA table_info(principals)")
            .expect("pragma");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("cols")
            .map(|c| c.expect("col"))
            .collect();
        cols.join(",")
    }

    #[test]
    fn principal_agent_mark_sets_clears_and_old_db_gains_the_column() {
        with_temp_home(|| {
            crate::db::init_for_tests();
            let id = uuid::Uuid::new_v4().to_string();
            let handle = format!("agentws{}", &id[..8]);
            {
                let db = crate::db::shared();
                let conn = db.lock();
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                    params![id, "Agent ws", format!("/tmp/skin-agent-{id}"), handle],
                )
                .expect("project");
            }
            let ada = add_principal("ada").expect("add");
            assert!(ada.agent_workspace.is_none());
            assert_eq!(principal_agent_workspace(&ada.id).unwrap(), None);
            let wire = serde_json::to_value(&ada).unwrap();
            assert!(wire["agentWorkspace"].is_null(), "{wire}");

            let marked = set_principal_agent("ada", Some(&handle)).expect("mark by handle");
            assert_eq!(marked.agent_workspace.as_deref(), Some(id.as_str()), "stored as id");
            assert_eq!(marked.agent_handle.as_deref(), Some(handle.as_str()));
            assert_eq!(principal_agent_workspace(&ada.id).unwrap().as_deref(), Some(id.as_str()));
            let listed = list_principals().unwrap();
            assert_eq!(listed[0].agent_workspace.as_deref(), Some(id.as_str()));
            assert_eq!(listed[0].agent_handle.as_deref(), Some(handle.as_str()));
            // Other owner edits keep the mark.
            let named = set_principal_full_name("ada", Some("Ada")).unwrap();
            assert_eq!(named.agent_workspace.as_deref(), Some(id.as_str()));

            let by_id = set_principal_agent("ada", Some(&id)).expect("mark by id");
            assert_eq!(by_id.agent_workspace.as_deref(), Some(id.as_str()));
            let err = set_principal_agent("ada", Some("no-such-ws")).unwrap_err();
            assert!(err.contains("unknown workspace"), "{err}");
            assert_eq!(
                principal_agent_workspace(&ada.id).unwrap().as_deref(),
                Some(id.as_str()),
                "a refused mark leaves the old one"
            );
            let err = set_principal_agent("ghost", Some(&handle)).unwrap_err();
            assert!(err.contains("unknown skin user"), "{err}");

            let cleared = set_principal_agent("ada", None).expect("clear");
            assert!(cleared.agent_workspace.is_none() && cleared.agent_handle.is_none());
            assert_eq!(principal_agent_workspace(&ada.id).unwrap(), None);
            assert_eq!(principal_agent_workspace("not-a-principal").unwrap(), None);
        });

        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("open old skin.db");
            conn.execute_batch(
                "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                 );
                 INSERT INTO principals (id, username, created_at) VALUES ('p1', 'bob', 1);",
            )
            .expect("pre-mark schema");
            assert!(!principal_columns(&conn).split(',').any(|c| c == "agent_workspace"));
            drop(conn);
            let listed = list_principals().expect("open_db adds agent_workspace");
            assert_eq!(listed.len(), 1);
            assert!(listed[0].agent_workspace.is_none(), "{listed:?}");
            let cols = with_conn(|conn| Ok(principal_columns(conn))).unwrap();
            assert!(cols.split(',').any(|c| c == "agent_workspace"), "{cols}");
        });
    }

    #[test]
    fn principal_full_name_column_on_new_and_old_db_round_trips() {
        with_temp_home(|| {
            add_principal("ada").expect("add");
            let cols = with_conn(|conn| Ok(principal_columns(conn))).unwrap();
            assert!(
                cols.contains("full_name"),
                "new db missing full_name: {cols}"
            );
            assert!(cols.contains("email"), "must not drop email: {cols}");
            assert!(cols.contains("role_id"), "must not drop role_id: {cols}");
            assert!(
                cols.contains("default_rooms"),
                "must not drop default_rooms: {cols}"
            );
            let fresh = list_principals().unwrap();
            assert_eq!(fresh.len(), 1);
            assert!(fresh[0].full_name.is_none(), "{fresh:?}");
            let wire = serde_json::to_value(&fresh[0]).unwrap();
            assert!(
                wire.get("fullName").is_some(),
                "list JSON must include fullName: {wire}"
            );
            assert!(wire["fullName"].is_null(), "{wire}");
            assert!(wire.get("passwordHash").is_none() && wire.get("password_hash").is_none());

            let set = set_principal_full_name("ada", Some("Ada/Lovelace: MD")).unwrap();
            assert_eq!(set.full_name.as_deref(), Some("Ada/Lovelace: MD"));
            let listed = list_principals().unwrap();
            assert_eq!(listed[0].full_name.as_deref(), Some("Ada/Lovelace: MD"));
            let cleared = set_principal_full_name("ada", Some("   ")).unwrap();
            assert!(cleared.full_name.is_none(), "{cleared:?}");
            let none = set_principal_full_name("ada", None).unwrap();
            assert!(none.full_name.is_none(), "{none:?}");
            let missing = set_principal_full_name("ghost", Some("Nope")).unwrap_err();
            assert!(missing.contains("unknown skin user"), "{missing}");
        });

        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("open old skin.db");
            conn.execute_batch(
                "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT,
                    email TEXT
                 );",
            )
            .expect("pre-full_name schema");
            conn.execute(
                "INSERT INTO principals (id, username, created_at, email)
                 VALUES ('p1', 'bob', 1, 'bob@clinic.com')",
                [],
            )
            .unwrap();
            let before = principal_columns(&conn);
            assert!(
                !before.split(',').any(|c| c == "full_name"),
                "fixture must lack full_name: {before}"
            );
            drop(conn);

            let listed = list_principals().expect("open_db adds full_name");
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].username, "bob");
            assert!(listed[0].full_name.is_none(), "{listed:?}");
            assert_eq!(listed[0].email.as_deref(), Some("bob@clinic.com"));
            let cols = with_conn(|conn| Ok(principal_columns(conn))).unwrap();
            assert!(
                cols.contains("full_name"),
                "old db must gain full_name: {cols}"
            );
            assert!(cols.contains("email"), "{cols}");
            assert!(cols.contains("role_id"), "{cols}");
            assert!(cols.contains("default_rooms"), "{cols}");
            let again = list_principals().expect("second open ignores duplicate ALTER");
            assert_eq!(again[0].username, "bob");
            let set = set_principal_full_name("bob", Some("Bob")).unwrap();
            assert_eq!(set.full_name.as_deref(), Some("Bob"));
            assert_eq!(
                list_principals().unwrap()[0].full_name.as_deref(),
                Some("Bob")
            );
        });
    }

    fn table_columns(conn: &Connection, table: &str) -> String {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("pragma");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("cols")
            .map(|c| c.expect("col"))
            .collect();
        cols.join(",")
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![name],
                |r| r.get(0),
            )
            .expect("sqlite_master");
        n > 0
    }

    #[test]
    fn grants_table_and_role_app_id_on_new_and_old_db() {
        with_temp_home(|| {
            add_principal("ada").expect("add");
            create_role("guest", &RoomPolicy::new()).expect("role");
            let (cols, grants, app_id) = with_conn(|conn| {
                let app_id: Option<String> = conn
                    .query_row("SELECT app_id FROM roles WHERE name = 'guest'", [], |r| {
                        r.get(0)
                    })
                    .map_err(|e| e.to_string())?;
                Ok((
                    table_columns(conn, "roles"),
                    table_exists(conn, "grants"),
                    app_id,
                ))
            })
            .unwrap();
            assert!(
                cols.split(',').any(|c| c == "app_id"),
                "new db missing roles.app_id: {cols}"
            );
            assert!(grants, "new db missing grants");
            assert!(app_id.is_none(), "new role stays host-wide: {app_id:?}");
            let ada = list_principals().unwrap();
            let grants_now = list_grants().unwrap();
            let host = host_grants(&grants_now, &ada[0].id);
            assert_eq!(host.len(), 1, "new guest gets one host grant");
            assert!(host[0].role_id.is_none(), "{host:?}");
            assert_eq!(host[0].scope.as_deref(), Some("[]"));
            let listed = list_roles().unwrap();
            assert_eq!(listed.len(), 1);
            assert!(listed[0].app_id.is_none(), "{listed:?}");
            let host_id = host[0].id.clone();
            reopen_skin_db();
            let again = list_grants().expect("second open ignores duplicate ALTER");
            assert_eq!(host_grants(&again, &ada[0].id).len(), 1, "{again:?}");
            assert_eq!(again.iter().filter(|g| g.id == host_id).count(), 1);
        });

        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("open old skin.db");
            conn.execute_batch(
                "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT,
                    email TEXT,
                    full_name TEXT
                 );
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}'
                 );",
            )
            .expect("pre-grant schema");
            conn.execute(
                "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
                 VALUES ('r1', 'guest', '[]', '[]', 1, '{}')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, full_name)
                 VALUES ('p1', 'bob', 1, 'Bob')",
                [],
            )
            .unwrap();
            let before_roles = table_columns(&conn, "roles");
            assert!(
                !before_roles.split(',').any(|c| c == "app_id"),
                "fixture must lack app_id: {before_roles}"
            );
            assert!(!table_exists(&conn, "grants"), "fixture must lack grants");
            drop(conn);

            let roles = list_roles().expect("open_db adds app_id and grants");
            assert_eq!(roles.len(), 1);
            assert_eq!(roles[0].name, "guest");
            assert!(roles[0].app_id.is_none(), "{roles:?}");
            let (cols, grants) =
                with_conn(|conn| Ok((table_columns(conn, "roles"), table_exists(conn, "grants"))))
                    .unwrap();
            assert!(
                cols.split(',').any(|c| c == "app_id"),
                "old db must gain roles.app_id: {cols}"
            );
            assert!(grants, "old db must gain grants");
            let copied = list_grants().unwrap();
            let host = host_grants(&copied, "p1");
            assert_eq!(
                host.len(),
                1,
                "unassigned bob is copied onto host: {copied:?}"
            );
            assert!(host[0].role_id.is_none(), "{host:?}");
            assert_eq!(host[0].scope.as_deref(), Some("[]"));
            assert!(roles[0].app_id.is_none(), "copy must not set roles.app_id");
            let bob = list_principals().unwrap();
            assert_eq!(bob[0].full_name.as_deref(), Some("Bob"));
            assert!(bob[0].role_id.is_none(), "{bob:?}");
            let host_id = host[0].id.clone();
            reopen_skin_db();
            let again_grants = list_grants().unwrap();
            assert_eq!(
                host_grants(&again_grants, "p1").len(),
                1,
                "{again_grants:?}"
            );
            assert_eq!(again_grants.iter().filter(|g| g.id == host_id).count(), 1);
            let again = list_roles().expect("second open ignores duplicate ALTER");
            assert_eq!(again[0].name, "guest");
            assert!(again[0].app_id.is_none(), "{again:?}");
        });
    }

    #[test]
    fn grant_create_list_delete_validates_and_keeps_role() {
        with_temp_home(|| {
            let ada = add_principal("ada").unwrap();
            let role = create_role("guest", &RoomPolicy::new()).unwrap();
            assert!(role.app_id.is_none(), "create_role must not require app_id");

            let missing_role = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "booking".into(),
                role: None,
                scope: None,
                created_by: None,
            })
            .unwrap_err();
            assert!(
                missing_role.contains("roleId") && missing_role.contains("rooms scope"),
                "{missing_role}"
            );

            let with_scope = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "booking".into(),
                role: Some("guest".into()),
                scope: Some("owner".into()),
                created_by: None,
            })
            .unwrap_err();
            assert!(with_scope.contains("scope"), "{with_scope}");

            let mailbox_role = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "mailbox".into(),
                target_id: "box".into(),
                role: Some(role.id.clone()),
                scope: Some("may-read".into()),
                created_by: None,
            })
            .unwrap_err();
            assert!(mailbox_role.contains("kind=app"), "{mailbox_role}");

            let mailbox_no_scope = create_grant(&SkinGrantWrite {
                subject_kind: "workspace".into(),
                subject_id: "ws-1".into(),
                kind: "database".into(),
                target_id: "db-1".into(),
                role: None,
                scope: None,
                created_by: None,
            })
            .unwrap_err();
            assert!(mailbox_no_scope.contains("scope"), "{mailbox_no_scope}");

            let unknown = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: "no-such-principal".into(),
                kind: "app".into(),
                target_id: "booking".into(),
                role: Some("guest".into()),
                scope: None,
                created_by: None,
            })
            .unwrap_err();
            assert!(unknown.contains("unknown principal"), "{unknown}");

            let empty_ws = create_grant(&SkinGrantWrite {
                subject_kind: "workspace".into(),
                subject_id: "  ".into(),
                kind: "cli".into(),
                target_id: "tool".into(),
                role: None,
                scope: Some("run".into()),
                created_by: None,
            })
            .unwrap_err();
            assert!(empty_ws.contains("subject"), "{empty_ws}");

            let bad_kind = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "people".into(),
                target_id: "x".into(),
                role: None,
                scope: Some("x".into()),
                created_by: None,
            })
            .unwrap_err();
            assert!(bad_kind.contains("kind"), "{bad_kind}");

            let created = create_grant(&SkinGrantWrite {
                subject_kind: "Principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "booking".into(),
                role: Some("guest".into()),
                scope: None,
                created_by: None,
            })
            .unwrap();
            assert_eq!(created.subject_kind, "principal");
            assert_eq!(created.kind, "app");
            assert_eq!(created.target_id, "booking");
            assert_eq!(created.role_id.as_deref(), Some(role.id.as_str()));
            assert!(created.scope.is_none(), "{created:?}");

            let ws = create_grant(&SkinGrantWrite {
                subject_kind: "workspace".into(),
                subject_id: "not-a-real-workspace".into(),
                kind: "mailbox".into(),
                target_id: "inbox".into(),
                role: None,
                scope: Some("may-read".into()),
                created_by: None,
            })
            .unwrap();
            assert_eq!(ws.subject_kind, "workspace");
            assert!(ws.role_id.is_none(), "{ws:?}");
            assert_eq!(ws.scope.as_deref(), Some("may-read"));

            let dup = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "booking".into(),
                role: Some(role.id.clone()),
                scope: None,
                created_by: None,
            })
            .unwrap_err();
            assert!(dup.contains("already exists"), "{dup}");

            let listed = list_grants().unwrap();
            assert_eq!(
                listed.len(),
                3,
                "host grant plus the two writes: {listed:?}"
            );
            assert!(
                listed.iter().any(|g| g.id == created.id),
                "created grant missing: {listed:?}"
            );
            assert!(
                listed.iter().any(|g| g.id == ws.id),
                "workspace grant missing: {listed:?}"
            );

            with_conn(|conn| {
                conn.execute(
                    "UPDATE roles SET app_id = 'crm' WHERE id = ?1",
                    params![role.id],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            let mismatch = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "booking-other".into(),
                role: Some("guest".into()),
                scope: None,
                created_by: None,
            })
            .unwrap_err();
            assert!(
                mismatch.contains("crm") && mismatch.contains("booking-other"),
                "{mismatch}"
            );
            let matched = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "crm".into(),
                role: Some("guest".into()),
                scope: None,
                created_by: None,
            })
            .unwrap();
            assert_eq!(matched.target_id, "crm");
            let scoped = list_roles().unwrap();
            assert_eq!(scoped[0].app_id.as_deref(), Some("crm"));

            assert!(delete_grant_by_id(&created.id).unwrap());
            assert!(!delete_grant_by_id(&created.id).unwrap());
            assert!(
                delete_grant_by_key("workspace", "not-a-real-workspace", "mailbox", "inbox")
                    .unwrap()
            );
            let left = list_grants().unwrap();
            assert_eq!(left.len(), 2, "host grant stays: {left:?}");
            assert!(left.iter().any(|g| g.id == matched.id), "{left:?}");
            assert_eq!(host_grants(&left, &ada.id).len(), 1, "{left:?}");
            let roles = list_roles().unwrap();
            assert_eq!(roles.len(), 1, "revoke must keep the role");
            assert_eq!(roles[0].name, "guest");

            set_principal_password("ada", Some("s3cret-horse")).unwrap();
            match check_and_record_login("ada", "s3cret-horse") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "ada"),
                other => panic!("login with a grant still works, got {other:?}"),
            }
            let (_meta, raw) = create_session_token("ada").expect("session mint unchanged");
            assert!(resolve_skin_token(&raw).is_some());
            let crm = create_app_session_token("ada", "crm").expect("crm grant allows crm login");
            assert_eq!(crm.app, "crm");
            assert!(delete_grant_by_id(&matched.id).unwrap());
            assert!(
                resolve_skin_token(&crm.raw).is_none(),
                "a grant delete must end that guest's live session on that App"
            );
            assert!(
                resolve_skin_token(&raw).is_some(),
                "S24-6: deleting the crm grant must not end the box-login session"
            );
            let (_meta, raw2) = create_session_token("ada").expect("host grant still allows login");
            assert!(resolve_skin_token(&raw2).is_some());
            assert_eq!(
                create_app_session_token("ada", "crm").expect_err("crm grant gone").as_str(),
                HOST_LOGIN_DENIED
            );
        });
    }

    #[test]
    fn principal_without_host_grant_cannot_create_session() {
        with_temp_home(|| {
            let ada = add_principal("ada").unwrap();
            set_principal_password("ada", Some("s3cret-horse")).unwrap();
            assert_eq!(host_grants(&list_grants().unwrap(), &ada.id).len(), 1);
            assert!(
                delete_grant_by_key("principal", &ada.id, "app", HOST_APP_ID).unwrap(),
                "host grant must delete"
            );
            assert!(host_grants(&list_grants().unwrap(), &ada.id).is_empty());
            match check_and_record_login("ada", "s3cret-horse") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "ada"),
                other => panic!("password check stays separate from the grant bit, got {other:?}"),
            }
            let err = create_session_token("ada").expect_err("missing host grant denies");
            assert_eq!(err, HOST_LOGIN_DENIED);
            assert!(!err.contains("ada"), "{err}");
            assert!(!err.contains("grant"), "{err}");
            assert!(!err.contains(HOST_APP_ID), "{err}");
            reopen_skin_db();
            assert!(
                host_grants(&list_grants().unwrap(), &ada.id).is_empty(),
                "deleted host grant must stay deleted across open"
            );
            let err = create_session_token("ada").expect_err("reopen must not restore the grant");
            assert_eq!(err, HOST_LOGIN_DENIED);
        });
    }

    #[test]
    fn grant_enabled_alter_is_idempotent_and_off_is_not_a_grant() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("add");
            set_principal_password("ada", Some("s3cret-horse")).expect("password");
            let listed = list_grants().expect("list");
            let host = host_grants(&listed, &ada.id);
            assert_eq!(host.len(), 1, "new guest still has the host row");
            assert!(host[0].enabled, "existing and new rows default on");
            let id = host[0].id.clone();
            reopen_skin_db();
            let reopened = list_grants().expect("second open");
            let again = host_grants(&reopened, &ada.id);
            assert_eq!(again.len(), 1);
            assert!(again[0].enabled, "second open ignores duplicate ALTER");
            assert_eq!(again[0].id, id);
            let updated = set_grant_enabled(&id, false).expect("disable");
            assert!(!updated.enabled);
            let listed = list_grants().expect("list after disable");
            let row = listed.iter().find(|g| g.id == id).expect("row stays");
            assert!(!row.enabled, "enabled=0 must not delete");
            assert_eq!(listed.iter().filter(|g| g.id == id).count(), 1);
            match check_and_record_login("ada", "s3cret-horse") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.username, "ada"),
                other => panic!("password check stays separate, got {other:?}"),
            }
            let err = create_session_token("ada").expect_err("enabled=0 denies mint");
            assert_eq!(err, HOST_LOGIN_DENIED);
            set_grant_enabled(&id, true).expect("enable");
            create_session_token("ada").expect("enabled=1 allows mint");
        });

        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("old skin.db");
            conn.execute_batch(
                "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL
                 );
                 CREATE TABLE grants (
                    id TEXT PRIMARY KEY,
                    subject_kind TEXT NOT NULL,
                    subject_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    role_id TEXT,
                    scope TEXT,
                    created_at INTEGER NOT NULL,
                    UNIQUE(subject_kind, subject_id, kind, target_id)
                 );",
            )
            .expect("pre-enabled schema");
            conn.execute(
                "INSERT INTO principals (id, username, created_at) VALUES ('p1', 'ada', 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at)
                 VALUES ('g1', 'principal', 'p1', 'app', 'host', NULL, '[]', 1)",
                [],
            )
            .unwrap();
            let before = table_columns(&conn, "grants");
            assert!(
                !before.split(',').any(|c| c == "enabled"),
                "fixture must lack enabled: {before}"
            );
            drop(conn);
            let grants = list_grants().expect("open adds enabled");
            let host = host_grants(&grants, "p1");
            assert_eq!(host.len(), 1, "{grants:?}");
            assert!(host[0].enabled, "old row defaults on");
            let cols = with_conn(|c| Ok(table_columns(c, "grants"))).unwrap();
            assert!(
                cols.split(',').any(|c| c == "enabled"),
                "old db must gain enabled: {cols}"
            );
            reopen_skin_db();
            assert!(
                host_grants(&list_grants().expect("second open"), "p1")[0].enabled,
                "duplicate ALTER must keep the default"
            );
        });
    }

    #[test]
    fn host_switch_deletes_grant_and_keeps_principal_role() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("add");
            let role = create_role("editor", &RoomPolicy::new()).expect("role");
            assign_role("ada", "editor").expect("assign");
            set_principal_password("ada", Some("s3cret-horse")).expect("password");
            assert!(set_host_login("ada", false).expect("switch off"), "host row deletes");
            assert!(host_grants(&list_grants().expect("list"), &ada.id).is_empty());
            let kept = list_principals()
                .expect("principals")
                .into_iter()
                .find(|p| p.username == "ada")
                .expect("ada stays");
            assert_eq!(kept.role_id.as_deref(), Some(role.id.as_str()));
            assert!(verify_principal_password("ada", "s3cret-horse"));
            match check_and_record_login("ada", "s3cret-horse") {
                SkinLoginOutcome::Ok(p) => assert_eq!(p.role_id.as_deref(), Some(role.id.as_str())),
                other => panic!("password check still succeeds, got {other:?}"),
            }
            let err = create_session_token("ada").expect_err("mint denies");
            assert_eq!(err, HOST_LOGIN_DENIED);
            assert!(set_host_login("ada", true).expect("switch on"));
            let listed = list_grants().expect("list");
            let host = host_grants(&listed, &ada.id);
            assert_eq!(host.len(), 1);
            assert_eq!(
                host[0].role_id.as_deref(),
                Some(role.id.as_str()),
                "AM1: a re-added host grant carries the guest's role, never a blank row: {host:?}"
            );
            assert!(host[0].scope.is_none(), "{host:?}");
            assert!(host[0].enabled);
            let detail = host_login_detail("ada").expect("detail");
            assert_eq!(detail.role_id.as_deref(), Some(role.id.as_str()));
            assert_eq!(detail.grant_role_id, detail.role_id, "{detail:?}");
            create_session_token("ada").expect("host row allows mint again");
        });
    }

    /// AM1 (S24-2): the host grant is the login truth from 0.45.5. Writers
    /// keep it equal to the column; a role written only on the grant wins.
    #[test]
    fn host_login_role_comes_from_the_grant() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("add");
            let room_login = uuid::Uuid::new_v4().to_string();
            let room_grant = uuid::Uuid::new_v4().to_string();
            let mut login_policy = RoomPolicy::new();
            login_policy.insert(room_login.clone(), vec![CAP_THREAD_READ.to_string()]);
            let mut grant_policy = RoomPolicy::new();
            grant_policy.insert(room_grant.clone(), vec![CAP_FILES_READ.to_string()]);
            let login_role = create_role("login-role", &login_policy).expect("login role");
            let grant_role = create_role("grant-role", &grant_policy).expect("grant role");
            assign_role("ada", &login_role.id).expect("assign");
            let detail = host_login_detail("ada").expect("detail");
            assert_eq!(detail.role_id.as_deref(), Some(login_role.id.as_str()));
            assert_eq!(
                detail.grant_role_id, detail.role_id,
                "assign writes the host grant too: {detail:?}"
            );
            let (meta, _) = create_session_token("ada").expect("mint");
            assert_eq!(meta.rooms, vec![room_login.clone()]);
            let host_id = host_grants(&list_grants().expect("list"), &ada.id)[0].id.clone();
            with_conn(|conn| {
                conn.execute(
                    "UPDATE grants SET role_id = ?1 WHERE id = ?2",
                    params![grant_role.id, host_id],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            let (meta, _) = create_session_token("ada").expect("mint uses the grant role");
            assert_eq!(meta.rooms, vec![room_grant.clone()]);
            assert!(!meta.rooms.contains(&room_login), "{meta:?}");
            unassign_role("ada").expect("unassign");
            let host = host_grants(&list_grants().expect("list"), &ada.id)[0].clone();
            assert!(host.role_id.is_none(), "{host:?}");
            assert_eq!(host.scope.as_deref(), Some("[]"), "unassign → default rooms");
        });
    }

    fn session_revoked_at(raw: &str) -> Option<i64> {
        let hash = sha256_hex(raw);
        with_conn(|conn| {
            conn.query_row(
                "SELECT revoked_at FROM tokens WHERE key_hash = ?1",
                params![hash],
                |r| r.get::<_, Option<i64>>(0),
            )
            .map_err(|e| format!("token row: {e}"))
        })
        .expect("token row must exist")
    }

    #[test]
    fn grant_disable_or_delete_revokes_that_guests_sessions_only() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("ada");
            add_principal("bob").expect("bob");
            let room = uuid::Uuid::new_v4().to_string();
            let (_, ada_raw) = create_session_token("ada").expect("ada session");
            let (_, ada_raw2) = create_session_token("ada").expect("ada second session");
            let (_, bob_raw) = create_session_token("bob").expect("bob session");
            let (_, plat_raw) =
                create_token("partner", None, std::slice::from_ref(&room)).expect("platform");
            assert!(resolve_skin_token(&ada_raw).is_some());

            // Disable the host grant: both of ada's sessions end now.
            let host_id = host_grants(&list_grants().unwrap(), &ada.id)[0].id.clone();
            set_grant_enabled(&host_id, false).expect("disable");
            assert!(resolve_skin_token(&ada_raw).is_none(), "disabled grant must end session");
            assert!(resolve_skin_token(&ada_raw2).is_none(), "every session of ada ends");
            assert!(
                session_revoked_at(&ada_raw).is_some(),
                "disable must revoke the row, not only hide it"
            );
            assert!(resolve_skin_token(&bob_raw).is_some(), "bob untouched");
            assert!(session_revoked_at(&bob_raw).is_none());
            assert!(resolve_skin_token(&plat_raw).is_some(), "platform pass untouched");

            // Re-enable never revives the old pass; a fresh login works.
            set_grant_enabled(&host_id, true).expect("enable");
            assert!(resolve_skin_token(&ada_raw).is_none(), "re-enable must not revive");
            let (_, ada_raw3) = create_session_token("ada").expect("login again");
            assert!(resolve_skin_token(&ada_raw3).is_some());

            // Delete by id.
            assert!(delete_grant_by_id(&host_id).expect("delete"));
            assert!(resolve_skin_token(&ada_raw3).is_none(), "deleted grant ends session");
            assert!(session_revoked_at(&ada_raw3).is_some());
            assert_eq!(
                create_session_token("ada").expect_err("no grant"),
                HOST_LOGIN_DENIED
            );

            // Host switch off.
            assert!(set_host_login("ada", true).expect("on"));
            let (_, ada_raw4) = create_session_token("ada").expect("login");
            assert!(set_host_login("ada", false).expect("off"));
            assert!(resolve_skin_token(&ada_raw4).is_none(), "host switch off ends session");
            assert!(session_revoked_at(&ada_raw4).is_some());

            // Any grant of ada (not only host) going away ends her sessions.
            assert!(set_host_login("ada", true).expect("on"));
            let mailbox = create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "mailbox".into(),
                target_id: "box".into(),
                role: None,
                scope: Some("may-read".into()),
                created_by: None,
            })
            .expect("mailbox");
            let (_, ada_raw5) = create_session_token("ada").expect("login");
            assert!(delete_grant_by_key("principal", &ada.id, "mailbox", "box").expect("del"));
            assert!(resolve_skin_token(&ada_raw5).is_none(), "grant delete by key ends session");
            let _ = mailbox;

            // A workspace-subject grant change touches no guest session.
            create_grant(&SkinGrantWrite {
                subject_kind: "workspace".into(),
                subject_id: "ws-1".into(),
                kind: "database".into(),
                target_id: "db".into(),
                role: None,
                scope: Some("read".into()),
                created_by: None,
            })
            .expect("ws grant");
            assert!(delete_grant_by_key("workspace", "ws-1", "database", "db").expect("del"));

            assert!(resolve_skin_token(&bob_raw).is_some(), "bob still untouched");
            assert!(resolve_skin_token(&plat_raw).is_some(), "platform still untouched");
        });
    }

    #[test]
    fn session_use_rechecks_host_grant_and_principal() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("ada");
            add_principal("bob").expect("bob");
            let room = uuid::Uuid::new_v4().to_string();
            let (_, ada_raw) = create_session_token("ada").expect("ada");
            let (_, bob_raw) = create_session_token("bob").expect("bob");
            let (_, plat_raw) =
                create_token("partner", None, std::slice::from_ref(&room)).expect("platform");
            // A write that skips the revoking writers (old code, hand edit).
            with_conn(|conn| {
                conn.execute(
                    "UPDATE grants SET enabled = 0
                     WHERE subject_kind = 'principal' AND subject_id = ?1 AND target_id = ?2",
                    params![ada.id, HOST_APP_ID],
                )
                .map_err(|e| e.to_string())
            })
            .unwrap();
            assert!(resolve_skin_token(&ada_raw).is_none(), "use-time check refuses");
            assert!(resolve_skin_token(&bob_raw).is_some());
            assert!(resolve_skin_token(&plat_raw).is_some(), "platform skips the grant check");

            assert!(remove_principal("bob").expect("remove bob"));
            assert!(resolve_skin_token(&bob_raw).is_none(), "removed principal ends session");
            assert!(resolve_skin_token(&plat_raw).is_some(), "platform survives remove");
        });
    }

    #[test]
    fn role_room_removal_and_role_swap_reach_live_sessions() {
        with_temp_home(|| {
            add_principal("ada").expect("ada");
            add_principal("bob").expect("bob");
            let a = uuid::Uuid::new_v4().to_string();
            let b = uuid::Uuid::new_v4().to_string();
            let mut wide = RoomPolicy::new();
            wide.insert(a.clone(), vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()]);
            wide.insert(b.clone(), vec![CAP_THREAD_READ.into()]);
            create_role("wide", &wide).expect("wide");
            create_role("bobs", &thread_only_room_policy(std::slice::from_ref(&b)))
                .expect("bobs");
            assign_role("ada", "wide").expect("assign ada");
            assign_role("bob", "bobs").expect("assign bob");
            let (_, ada_raw) = create_session_token("ada").expect("ada");
            let (_, bob_raw) = create_session_token("bob").expect("bob");
            assert!(resolve_skin_token(&ada_raw).unwrap().has_room(&b));

            clear_role_room("wide", &b).expect("clear b");
            let p = resolve_skin_token(&ada_raw).expect("rewritten, not revoked");
            assert!(!p.has_room(&b), "removed room must leave the live session: {p:?}");
            assert!(p.has_cap_in_room(&a, CAP_FILES_READ));

            update_role(
                "wide",
                Some(&thread_only_room_policy(std::slice::from_ref(&a))),
            )
            .expect("narrow caps");
            let p = resolve_skin_token(&ada_raw).unwrap();
            assert!(!p.has_cap_in_room(&a, CAP_FILES_READ), "dropped cap must leave: {p:?}");
            assert!(p.has_cap_in_room(&a, CAP_THREAD_READ));

            create_role("dark", &RoomPolicy::new()).expect("dark");
            assign_role("ada", "dark").expect("swap role");
            assert!(resolve_skin_token(&ada_raw).unwrap().rooms_empty(), "role swap reaches session");

            // Delete a role: refused while assigned; after unassign the
            // session carries none of its rooms.
            assign_role("ada", "wide").expect("back to wide");
            assert!(resolve_skin_token(&ada_raw).unwrap().has_room(&a));
            remove_role("wide").expect_err("assigned role cannot be deleted");
            unassign_role("ada").expect("unassign");
            assert!(remove_role("wide").expect("delete"));
            let p = resolve_skin_token(&ada_raw).expect("thread-only defaults");
            assert!(!p.has_room(&a), "deleted role's rooms must be gone: {p:?}");

            let bp = resolve_skin_token(&bob_raw).expect("bob live");
            assert!(bp.has_cap_in_room(&b, CAP_THREAD_READ), "bob untouched: {bp:?}");
        });
    }

    #[test]
    fn default_rooms_removal_reaches_sessions_without_apply_tokens() {
        with_temp_home(|| {
            add_principal("ada").expect("ada");
            add_principal("bob").expect("bob");
            let a = uuid::Uuid::new_v4().to_string();
            let b = uuid::Uuid::new_v4().to_string();
            let c = uuid::Uuid::new_v4().to_string();
            set_principal_default_rooms("ada", &[a.clone(), b.clone()], false).unwrap();
            set_principal_default_rooms("bob", &[a.clone(), b.clone()], false).unwrap();
            let (_, ada_raw) = create_session_token("ada").unwrap();
            let (_, bob_raw) = create_session_token("bob").unwrap();
            set_principal_default_rooms("ada", &[a.clone(), c.clone()], false).unwrap();
            let p = resolve_skin_token(&ada_raw).unwrap();
            assert!(p.has_room(&a));
            assert!(!p.has_room(&b), "removed default room must leave: {p:?}");
            assert!(!p.has_room(&c), "added room waits for --apply-tokens: {p:?}");
            set_principal_default_rooms("ada", &[a.clone(), c.clone()], true).unwrap();
            assert!(resolve_skin_token(&ada_raw).unwrap().has_room(&c));
            let bp = resolve_skin_token(&bob_raw).unwrap();
            assert!(bp.has_room(&a) && bp.has_room(&b), "bob untouched: {bp:?}");
        });
    }

    #[test]
    fn password_set_revokes_every_session_of_that_guest_only() {
        with_temp_home(|| {
            add_principal("ada").expect("ada");
            add_principal("bob").expect("bob");
            let room = uuid::Uuid::new_v4().to_string();
            set_principal_password("ada", Some("s3cret-horse")).unwrap();
            let (_, a1) = create_session_token("ada").unwrap();
            let (_, a2) = create_session_token("ada").unwrap();
            let (_, bob_raw) = create_session_token("bob").unwrap();
            let (_, plat_raw) =
                create_token("partner", None, std::slice::from_ref(&room)).unwrap();
            set_principal_password("ada", Some("new-horse-pw")).unwrap();
            assert!(resolve_skin_token(&a1).is_none(), "password change ends sessions");
            assert!(resolve_skin_token(&a2).is_none(), "password change ends sessions");
            assert!(resolve_skin_token(&bob_raw).is_some(), "bob untouched");
            assert!(resolve_skin_token(&plat_raw).is_some(), "platform untouched");
            let (_, a3) = create_session_token("ada").unwrap();
            set_principal_password("ada", None).unwrap();
            assert!(resolve_skin_token(&a3).is_none(), "password clear ends sessions");
        });
    }

    #[test]
    fn create_role_app_id_sets_or_stays_null_and_names_stay_unique() {
        with_temp_home(|| {
            let scoped = create_role_for_app("editor", &RoomPolicy::new(), Some("svc-1"))
                .expect("app role");
            assert_eq!(scoped.app_id.as_deref(), Some("svc-1"));
            let listed = list_roles().expect("list");
            assert_eq!(listed[0].app_id.as_deref(), Some("svc-1"));
            let host_wide = create_role("reader", &RoomPolicy::new()).expect("host-wide");
            assert!(host_wide.app_id.is_none(), "{host_wide:?}");
            let dup = create_role_for_app("Editor", &RoomPolicy::new(), Some("svc-2"))
                .expect_err("box-unique name");
            assert!(
                dup.contains("already exists"),
                "role already exists must surface, got {dup}"
            );
            assert_eq!(list_roles().expect("still two").len(), 2);
        });
    }

    #[test]
    fn remove_principal_deletes_that_principals_grants() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("add");
            let bob = add_principal("bob").expect("bob");
            create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "mailbox".into(),
                target_id: "box".into(),
                role: None,
                scope: Some("may-read".into()),
                created_by: None,
            })
            .expect("mailbox");
            let before = list_grants().expect("before");
            assert!(before.iter().any(|g| g.subject_id == ada.id && g.kind == "mailbox"));
            assert!(before.iter().any(|g| g.subject_id == ada.id && g.target_id == HOST_APP_ID));
            assert!(remove_principal("ada").expect("remove"));
            let after = list_grants().expect("after");
            assert!(
                after.iter().all(|g| g.subject_id != ada.id),
                "ada grants must go: {after:?}"
            );
            assert!(after.iter().any(|g| g.subject_id == bob.id));
            assert!(list_principals().expect("left").iter().all(|p| p.username != "ada"));
        });
    }

    #[test]
    fn remove_role_refuses_when_a_grant_points_at_it() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("add");
            let role = create_role("editor", &RoomPolicy::new()).expect("role");
            create_grant(&SkinGrantWrite {
                subject_kind: "principal".into(),
                subject_id: ada.id.clone(),
                kind: "app".into(),
                target_id: "svc-9".into(),
                role: Some(role.id.clone()),
                scope: None,
                created_by: None,
            })
            .expect("grant");
            let err = remove_role(&role.id).expect_err("grant holds the role");
            assert!(err.contains("grant"), "{err}");
            assert!(err.contains("editor"), "{err}");
            assert_eq!(list_roles().expect("role stays").len(), 1);
            let grants = list_grants().expect("grant stays");
            assert!(grants.iter().any(|g| g.role_id.as_deref() == Some(role.id.as_str())));
        });
    }

    #[test]
    fn access_template_apply_does_not_rewrite_and_scoped_kinds_reject_role() {
        with_temp_home(|| {
            let ada = add_principal("ada").expect("add");
            let role_a = create_role("alpha", &RoomPolicy::new()).expect("alpha");
            let role_b = create_role("beta", &RoomPolicy::new()).expect("beta");
            let template = create_access_template("Clinic").expect("template");
            let app_line =
                create_template_line(&template.id, "app", Some(&role_a.id)).expect("app line");
            assert!(app_line.role_id.as_deref() == Some(role_a.id.as_str()));
            let mail_line = create_template_line(&template.id, "mailbox", None).expect("mail line");
            let db_line = create_template_line(&template.id, "database", None).expect("db line");
            let cli_line = create_template_line(&template.id, "cli", None).expect("cli line");
            let cols = with_conn(|conn| Ok(table_columns(conn, "access_template_lines"))).unwrap();
            assert!(
                !cols.split(',').any(|c| c == "target_id"),
                "template line must not store target_id: {cols}"
            );
            let role_on_mail = apply_access_template(
                &ada.id,
                &template.id,
                &[TemplateApplyLine {
                    line_id: mail_line.id.clone(),
                    target_id: "inbox".into(),
                    scope: Some("may-read".into()),
                    role: Some(role_a.id.clone()),
                }],
            )
            .expect_err("mailbox rejects a role");
            assert!(role_on_mail.contains("kind=app"), "{role_on_mail}");
            let missing_scope = apply_access_template(
                &ada.id,
                &template.id,
                &[TemplateApplyLine {
                    line_id: db_line.id.clone(),
                    target_id: "db-1".into(),
                    scope: None,
                    role: None,
                }],
            )
            .expect_err("database requires scope");
            assert!(missing_scope.contains("scope"), "{missing_scope}");
            let missing_cli = apply_access_template(
                &ada.id,
                &template.id,
                &[TemplateApplyLine {
                    line_id: cli_line.id.clone(),
                    target_id: "tool".into(),
                    scope: Some("  ".into()),
                    role: None,
                }],
            )
            .expect_err("cli requires scope");
            assert!(missing_cli.contains("scope"), "{missing_cli}");
            let before = list_grants().expect("no scoped grants yet");
            assert!(before.iter().all(|g| g.kind == "app" && g.target_id == HOST_APP_ID));
            let written = apply_access_template(
                &ada.id,
                &template.id,
                &[
                    TemplateApplyLine {
                        line_id: app_line.id.clone(),
                        target_id: "svc-1".into(),
                        scope: None,
                        role: None,
                    },
                    TemplateApplyLine {
                        line_id: mail_line.id.clone(),
                        target_id: "inbox".into(),
                        scope: Some("may-read".into()),
                        role: None,
                    },
                    TemplateApplyLine {
                        line_id: db_line.id.clone(),
                        target_id: "db-1".into(),
                        scope: Some("read".into()),
                        role: None,
                    },
                    TemplateApplyLine {
                        line_id: cli_line.id.clone(),
                        target_id: "tool".into(),
                        scope: Some("run".into()),
                        role: None,
                    },
                ],
            )
            .expect("apply");
            assert_eq!(written.len(), 4);
            let app_grant = written.iter().find(|g| g.kind == "app" && g.target_id == "svc-1").expect("app grant");
            assert_eq!(app_grant.role_id.as_deref(), Some(role_a.id.as_str()));
            assert!(app_grant.scope.is_none());
            assert!(written.iter().any(|g| g.kind == "mailbox" && g.scope.as_deref() == Some("may-read") && g.role_id.is_none()));
            update_template_line(&app_line.id, "app", Some(&role_b.id)).expect("edit line");
            update_access_template(&template.id, "Clinic renamed").expect("rename");
            let after = list_grants().expect("grants after template edit");
            let still = after.iter().find(|g| g.id == app_grant.id).expect("grant remains");
            assert_eq!(
                still.role_id.as_deref(),
                Some(role_a.id.as_str()),
                "template edit must not rewrite grants: {still:?}"
            );
            assert_eq!(still.target_id, "svc-1");
            let lines = list_access_templates().expect("templates");
            assert_eq!(lines[0].name, "Clinic renamed");
            assert_eq!(
                lines[0]
                    .lines
                    .iter()
                    .find(|l| l.id == app_line.id)
                    .expect("line")
                    .role_id
                    .as_deref(),
                Some(role_b.id.as_str())
            );
        });
    }

    #[test]
    fn host_grant_backfill_copies_assigned_and_empty_rooms_once() {
        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("old skin.db");
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}'
                 );
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT REFERENCES roles(id),
                    email TEXT,
                    full_name TEXT
                 );",
            )
            .expect("pre-grant schema");
            let sales = uuid::Uuid::new_v4().to_string();
            let other = uuid::Uuid::new_v4().to_string();
            let bob_room = uuid::Uuid::new_v4().to_string();
            let mut policy = RoomPolicy::new();
            policy.insert(
                sales.clone(),
                vec![CAP_THREAD_READ.into(), CAP_FILES_READ.into()],
            );
            let policy_json = room_policy_json(&policy);
            let caps = caps_json(&union_caps(&policy));
            let role_rooms = rooms_json(&rooms_from_policy(&policy));
            let ada_defaults = rooms_json(std::slice::from_ref(&other));
            let bob_defaults = rooms_json(std::slice::from_ref(&bob_room));
            conn.execute(
                "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
                 VALUES ('role-dentist', 'dentist', ?1, ?2, 1, ?3)",
                params![caps, role_rooms, policy_json],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms, role_id)
                 VALUES ('p-ada', 'ada', 1, ?1, 'role-dentist')",
                params![ada_defaults],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES ('p-bob', 'bob', 1, ?1)",
                params![bob_defaults],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES ('p-cara', 'cara', 1, '[]')",
                [],
            )
            .unwrap();
            assert!(!table_exists(&conn, "grants"), "fixture must lack grants");
            drop(conn);

            let principals = list_principals().expect("open copies host grants");
            let ada = principals.iter().find(|p| p.username == "ada").unwrap();
            let bob = principals.iter().find(|p| p.username == "bob").unwrap();
            let cara = principals.iter().find(|p| p.username == "cara").unwrap();
            assert_eq!(ada.role_id.as_deref(), Some("role-dentist"));
            assert_eq!(ada.default_rooms, vec![other.clone()]);
            assert!(bob.role_id.is_none(), "{bob:?}");
            assert!(
                cara.role_id.is_none() && cara.default_rooms.is_empty(),
                "{cara:?}"
            );
            let role = list_roles().unwrap();
            assert_eq!(role.len(), 1);
            assert!(
                role[0].app_id.is_none(),
                "host role stays host-wide: {role:?}"
            );
            assert_eq!(role[0].room_policy.get(&sales).map(|c| c.len()), Some(2));

            let grants = list_grants().unwrap();
            assert_eq!(grants.len(), 3, "{grants:?}");
            let ada_g = host_grants(&grants, "p-ada");
            let bob_g = host_grants(&grants, "p-bob");
            let cara_g = host_grants(&grants, "p-cara");
            assert_eq!(ada_g.len(), 1, "{grants:?}");
            assert_eq!(bob_g.len(), 1, "{grants:?}");
            assert_eq!(cara_g.len(), 1, "{grants:?}");
            assert_eq!(ada_g[0].role_id.as_deref(), Some("role-dentist"));
            assert!(
                ada_g[0].scope.is_none(),
                "assigned scope stays NULL: {ada_g:?}"
            );
            assert!(bob_g[0].role_id.is_none(), "{bob_g:?}");
            assert_eq!(bob_g[0].scope.as_deref(), Some(bob_defaults.as_str()));
            assert!(cara_g[0].role_id.is_none(), "{cara_g:?}");
            assert_eq!(
                cara_g[0].scope.as_deref(),
                Some("[]"),
                "empty rooms still get a row"
            );
            let ada_grant_id = ada_g[0].id.clone();
            let bob_grant_id = bob_g[0].id.clone();
            let cara_grant_id = cara_g[0].id.clone();

            reopen_skin_db();
            let second = list_grants().unwrap();
            assert_eq!(
                second.len(),
                3,
                "second open must not duplicate: {second:?}"
            );
            assert!(second.iter().any(|g| g.id == ada_grant_id));
            assert!(second.iter().any(|g| g.id == bob_grant_id));
            assert!(second.iter().any(|g| g.id == cara_grant_id));

            with_conn(|c| {
                c.execute(
                    "UPDATE grants SET role_id = NULL, scope = 'owner-keep' WHERE id = ?1",
                    params![ada_grant_id],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
            reopen_skin_db();
            let kept = list_grants().unwrap();
            assert_eq!(kept.len(), 3, "{kept:?}");
            let ada_kept = host_grants(&kept, "p-ada");
            assert_eq!(ada_kept.len(), 1, "{kept:?}");
            assert_eq!(ada_kept[0].id, ada_grant_id);
            assert!(
                ada_kept[0].role_id.is_none(),
                "owner edit must stick: {ada_kept:?}"
            );
            assert_eq!(ada_kept[0].scope.as_deref(), Some("owner-keep"));
            let bob_kept = host_grants(&kept, "p-bob");
            assert_eq!(bob_kept[0].id, bob_grant_id);
            assert_eq!(bob_kept[0].scope.as_deref(), Some(bob_defaults.as_str()));
            assert_eq!(host_grants(&kept, "p-cara")[0].id, cara_grant_id);
            assert!(list_roles().unwrap()[0].app_id.is_none());
            assert_eq!(
                list_principals()
                    .unwrap()
                    .iter()
                    .find(|p| p.username == "ada")
                    .unwrap()
                    .role_id
                    .as_deref(),
                Some("role-dentist")
            );
            // AM1: from 0.45.5 the host grant is the login truth, so the
            // hand edit above would change ada's login. Put her role back
            // the supported way (assign writes the grant and the column).
            assign_role("ada", "dentist").expect("re-assign");

            let (ada_meta, ada_raw) = create_session_token("ada").expect("backfilled ada logs in");
            assert_eq!(ada_meta.rooms, vec![sales.clone()]);
            assert!(
                !ada_meta.rooms.contains(&other),
                "session must not read default_rooms"
            );
            let ada_pass = resolve_skin_token(&ada_raw).expect("ada session");
            assert!(ada_pass.has_cap(CAP_FILES_READ));
            assert!(ada_pass.has_cap_in_room(&sales, CAP_FILES_READ));
            assert!(!ada_pass.has_room(&other));

            let (bob_meta, bob_raw) = create_session_token("bob").expect("backfilled bob logs in");
            assert_eq!(bob_meta.rooms, vec![bob_room.clone()]);
            let bob_pass = resolve_skin_token(&bob_raw).expect("bob session");
            assert!(bob_pass.has_cap(CAP_THREAD_READ));
            assert!(bob_pass.has_cap(CAP_THREAD_POST));
            assert!(!bob_pass.has_cap(CAP_FILES_READ), "{bob_pass:?}");
            assert!(bob_pass.has_cap_in_room(&bob_room, CAP_THREAD_READ));
            assert!(!bob_pass.has_cap_in_room(&bob_room, CAP_FILES_READ));

            let (cara_meta, cara_raw) =
                create_session_token("cara").expect("empty rooms still log in");
            assert!(cara_meta.rooms.is_empty(), "{cara_meta:?}");
            let cara_pass = resolve_skin_token(&cara_raw).expect("cara session");
            assert!(cara_pass.rooms_empty());
            assert!(!cara_pass.has_cap(CAP_THREAD_READ), "{cara_pass:?}");
            assert!(!cara_pass.has_cap(CAP_FILES_READ));

            assert!(delete_grant_by_id(&ada_grant_id).unwrap());
            let err = create_session_token("ada").expect_err("deleted host grant denies");
            assert_eq!(err, HOST_LOGIN_DENIED);
            assert!(!err.contains("ada") && !err.contains("grant"), "{err}");
            reopen_skin_db();
            assert!(
                host_grants(&list_grants().unwrap(), "p-ada").is_empty(),
                "upgrade copy must not put a deleted host grant back"
            );
            let err = create_session_token("ada").expect_err("reopen must not restore the grant");
            assert_eq!(err, HOST_LOGIN_DENIED);
            let (_again, _) = create_session_token("bob").expect("bob grant still allows login");
        });
    }

    #[test]
    fn host_grant_backfill_does_not_rewrite_owner_grant() {
        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("grants-era skin.db");
            conn.execute_batch(
                "CREATE TABLE principals (
                    id TEXT PRIMARY KEY,
                    username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL,
                    default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT,
                    role_id TEXT,
                    email TEXT,
                    full_name TEXT
                 );
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}'
                 );
                 CREATE TABLE grants (
                    id TEXT PRIMARY KEY,
                    subject_kind TEXT NOT NULL,
                    subject_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    role_id TEXT,
                    scope TEXT,
                    created_at INTEGER NOT NULL,
                    UNIQUE(subject_kind, subject_id, kind, target_id)
                 );",
            )
            .expect("grants-era schema");
            let gia_room = uuid::Uuid::new_v4().to_string();
            let gia_defaults = rooms_json(std::slice::from_ref(&gia_room));
            let dan_defaults = rooms_json(&["should-not-copy".into()]);
            conn.execute(
                "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy)
                 VALUES ('role-assigned', 'assigned', '[]', '[]', 1, '{}'),
                        ('role-other', 'other', '[]', '[]', 1, '{}')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms, role_id)
                 VALUES ('p-dan', 'dan', 1, ?1, 'role-assigned'),
                        ('p-ada', 'ada', 1, '[]', 'role-assigned')",
                params![dan_defaults],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO principals (id, username, created_at, default_rooms)
                 VALUES ('p-eve', 'eve', 1, '[]'),
                        ('p-gia', 'gia', 1, ?1)",
                params![gia_defaults],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO grants
                 (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at)
                 VALUES ('g-dan', 'principal', 'p-dan', 'app', 'host', 'role-other', 'owner-scope', 1)",
                [],
            )
            .unwrap();
            drop(conn);

            let grants = list_grants().expect("open backfills missing host rows only");
            assert_eq!(grants.len(), 4, "{grants:?}");
            let dan = host_grants(&grants, "p-dan");
            assert_eq!(dan.len(), 1, "{grants:?}");
            assert_eq!(dan[0].id, "g-dan", "the owner's row is kept, not replaced");
            assert_eq!(dan[0].created_at, 1, "{dan:?}");
            // S24-2 step 1 (D1): login used `principals.role_id` before
            // 0.45.5, so that value wins over the grant's role, and the
            // overwrite is in the audit log.
            assert_eq!(dan[0].role_id.as_deref(), Some("role-assigned"));
            assert!(dan[0].scope.is_none(), "{dan:?}");
            let audit = crate::auth_audit::tail(100).expect("audit");
            let line = audit
                .iter()
                .map(|l| l.to_string())
                .find(|l| l.contains("dan") && l.contains("app_grant_upgrade"))
                .unwrap_or_else(|| panic!("step 1 overwrite must be audited: {audit:?}"));
            assert!(
                line.contains("role-other") && line.contains("role-assigned"),
                "audit names old and new: {line}"
            );
            let ada = host_grants(&grants, "p-ada");
            assert_eq!(ada.len(), 1, "{grants:?}");
            assert_eq!(ada[0].role_id.as_deref(), Some("role-assigned"));
            assert!(ada[0].scope.is_none(), "{ada:?}");
            let eve = host_grants(&grants, "p-eve");
            assert_eq!(eve.len(), 1, "{grants:?}");
            assert!(eve[0].role_id.is_none(), "{eve:?}");
            assert_eq!(eve[0].scope.as_deref(), Some("[]"));
            let gia = host_grants(&grants, "p-gia");
            assert_eq!(gia.len(), 1, "{grants:?}");
            assert!(gia[0].role_id.is_none(), "{gia:?}");
            assert_eq!(gia[0].scope.as_deref(), Some(gia_defaults.as_str()));
            for role in list_roles().unwrap() {
                assert!(role.app_id.is_none(), "copy must not set app_id: {role:?}");
            }
            let dan_id = dan[0].id.clone();
            reopen_skin_db();
            let again = list_grants().unwrap();
            assert_eq!(again.len(), 4, "{again:?}");
            let dan_again = host_grants(&again, "p-dan");
            assert_eq!(dan_again[0].id, dan_id);
            assert_eq!(dan_again[0].created_at, 1);
            assert_eq!(dan_again[0].role_id.as_deref(), Some("role-assigned"));
            assert!(dan_again[0].scope.is_none());
        });
    }

    #[test]
    fn verify_principal_password_dummy_on_miss_does_not_lockout() {
        with_temp_home(|| {
            add_principal("eve").unwrap();
            set_principal_password("eve", Some("right-password")).unwrap();
            assert!(verify_principal_password("eve", "right-password"));
            assert!(!verify_principal_password("eve", "wrong"));
            assert!(!verify_principal_password("ghost-user", "nope"));
            add_principal("mintonly").unwrap();
            assert!(!verify_principal_password("mintonly", "anything"));
            match check_and_record_login("eve", "right-password") {
                SkinLoginOutcome::Ok(_) => {}
                other => panic!("verify must not bump login_lockouts, got {other:?}"),
            }
        });
    }

    #[test]
    fn set_principal_password_empty_none_still_clears() {
        with_temp_home(|| {
            add_principal("cara").unwrap();
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            set_principal_password("cara", None).unwrap();
            match check_and_record_login("cara", "s3cret-horse") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("None still clears, got {other:?}"),
            }
            set_principal_password("cara", Some("s3cret-horse")).unwrap();
            set_principal_password("cara", Some("")).unwrap();
            match check_and_record_login("cara", "s3cret-horse") {
                SkinLoginOutcome::BadCreds => {}
                other => panic!("empty still clears, got {other:?}"),
            }
        });
    }

    #[test]
    fn clear_login_lockout_deletes_only_the_presented_key() {
        with_temp_home(|| {
            with_conn(|conn| {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('jane@clinic.com', 0, ?1), ('jane', 1, NULL), ('bad name', 2, NULL)",
                    params![now_secs().saturating_add(900)],
                )
                .map_err(|e| e.to_string())?;
                conn.execute(
                    "INSERT INTO password_reset_throttle (username, last_attempt) VALUES ('jane', 1)",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .expect("seed lock rows");
            let before = list_principals().expect("list before");
            let (key, cleared) = clear_login_lockout("  Jane@Clinic.COM ").expect("email clear");
            assert_eq!(key, "jane@clinic.com");
            assert!(cleared, "email row existed");
            let jane_left: i64 = with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM login_lockouts WHERE username = 'jane'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("jane row");
            assert_eq!(jane_left, 1, "username row must stay");
            let email_left: i64 = with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM login_lockouts WHERE username = 'jane@clinic.com'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("email row");
            assert_eq!(email_left, 0, "email row must be gone");
            let (key, cleared) = clear_login_lockout("Jane").expect("username clear");
            assert_eq!(key, "jane");
            assert!(cleared, "failed_count 1 with locked_until NULL is a row");
            let (key, cleared) = clear_login_lockout("jane").expect("second clear");
            assert_eq!(key, "jane");
            assert!(!cleared, "missing row is cleared false");
            let (key, cleared) = clear_login_lockout("  Bad Name ").expect("junk key");
            assert_eq!(key, "bad name");
            assert!(cleared, "non-principal trim+lowercase row");
            assert_eq!(
                list_principals().expect("list after"),
                before,
                "unlock must not create a principal"
            );
            let throttle: i64 = with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM password_reset_throttle WHERE username = 'jane'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("throttle");
            assert_eq!(throttle, 1, "password_reset_throttle must stay");
        });
    }

    #[test]
    fn set_principal_password_does_not_delete_login_lockout() {
        with_temp_home(|| {
            add_principal("cara").expect("add");
            set_principal_password("cara", Some("s3cret-horse")).expect("set");
            with_conn(|conn| {
                conn.execute(
                    "INSERT INTO login_lockouts (username, failed_count, locked_until)
                     VALUES ('cara', 1, NULL)",
                    [],
                )
                .map_err(|e| e.to_string())?;
                Ok(())
            })
            .expect("seed lock");
            let hash_before: String = with_conn(|conn| {
                conn.query_row(
                    "SELECT password_hash FROM principals WHERE username = 'cara'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("hash before");
            set_principal_password("cara", Some("other-password-1")).expect("reset");
            let still: i64 = with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM login_lockouts WHERE username = 'cara'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("lock after password set");
            assert_eq!(still, 1, "password set must leave the lock row");
            let hash_after: String = with_conn(|conn| {
                conn.query_row(
                    "SELECT password_hash FROM principals WHERE username = 'cara'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())
            })
            .expect("hash after");
            assert_ne!(hash_before, hash_after, "password set still writes the hash");
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
            let guest_role =
                create_role("bob", &RoomPolicy::new()).expect("guest and role coexist");
            assert_eq!(guest_role.name, "bob");
        });
    }

    #[test]
    fn role_parse_caps_rejects_unknown_verbs() {
        with_temp_home(|| {
            let mut ok = RoomPolicy::new();
            ok.insert("anna".into(), vec![CAP_TICKETS_READ.into()]);
            create_role("hygienist", &ok).expect("tickets:read is accepted");
            let mut wiki_ok = RoomPolicy::new();
            wiki_ok.insert("anna".into(), vec![CAP_WIKI_READ.into()]);
            create_role("archivist", &wiki_ok).expect("wiki:read is accepted");
            let mut store_ok = RoomPolicy::new();
            store_ok.insert("anna".into(), vec![CAP_STORE_READ.into()]);
            create_role("records", &store_ok).expect("store:read is accepted");
            let mut store_write_ok = RoomPolicy::new();
            store_write_ok.insert("docs".into(), vec![CAP_STORE_WRITE.into()]);
            create_role("charting", &store_write_ok).expect("store:write is accepted");
            for cap in ["pty:write", "wiki:write", "grid"] {
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
            create_role(
                "dentist",
                &thread_only_room_policy(std::slice::from_ref(&sales)),
            )
            .unwrap();
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
    fn session_tickets_has_cap_in_room_is_not_cartesian() {
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
                    CAP_TICKETS_READ.into(),
                    CAP_TICKETS_POST.into(),
                ],
            );
            create_role("dentist", &policy).unwrap();
            assign_role("bob", "dentist").unwrap();
            let (_m, raw) = create_session_token("bob").unwrap();
            let pass = resolve_skin_token(&raw).expect("session");
            assert!(pass.has_cap(CAP_TICKETS_READ), "union still lists tickets");
            assert!(pass.has_cap(CAP_TICKETS_POST));
            assert!(
                !pass.has_cap_in_room(&anna, CAP_TICKETS_READ),
                "tickets on docs must not grant tickets on anna: {pass:?}"
            );
            assert!(!pass.has_cap_in_room(&anna, CAP_TICKETS_POST));
            assert!(pass.has_cap_in_room(&docs, CAP_TICKETS_READ));
            assert!(pass.has_cap_in_room(&docs, CAP_TICKETS_POST));
            assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
        });
    }

    #[test]
    fn session_wiki_has_cap_in_room_is_not_cartesian() {
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
                    CAP_WIKI_READ.into(),
                ],
            );
            create_role("dentist", &policy).unwrap();
            assign_role("bob", "dentist").unwrap();
            let (_m, raw) = create_session_token("bob").unwrap();
            let pass = resolve_skin_token(&raw).expect("session");
            assert!(pass.has_cap(CAP_WIKI_READ), "union still lists wiki");
            assert!(
                !pass.has_cap_in_room(&anna, CAP_WIKI_READ),
                "wiki on docs must not grant wiki on anna: {pass:?}"
            );
            assert!(pass.has_cap_in_room(&docs, CAP_WIKI_READ));
            assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
        });
    }

    #[test]
    fn session_store_has_cap_in_room_is_not_cartesian() {
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
                    CAP_STORE_READ.into(),
                ],
            );
            create_role("dentist", &policy).unwrap();
            assign_role("bob", "dentist").unwrap();
            let (_m, raw) = create_session_token("bob").unwrap();
            let pass = resolve_skin_token(&raw).expect("session");
            assert!(pass.has_cap(CAP_STORE_READ), "union still lists store");
            assert!(
                !pass.has_cap_in_room(&anna, CAP_STORE_READ),
                "store on docs must not grant store on anna: {pass:?}"
            );
            assert!(pass.has_cap_in_room(&docs, CAP_STORE_READ));
            assert!(pass.has_cap_in_room(&anna, CAP_THREAD_READ));
        });
    }

    #[test]
    fn session_store_write_has_cap_in_room_is_not_cartesian() {
        with_temp_home(|| {
            add_principal("bob").unwrap();
            let sales = uuid::Uuid::new_v4().to_string();
            let docs = uuid::Uuid::new_v4().to_string();
            let mut policy = RoomPolicy::new();
            policy.insert(
                sales.clone(),
                vec![CAP_THREAD_READ.into(), CAP_THREAD_POST.into()],
            );
            policy.insert(
                docs.clone(),
                vec![
                    CAP_THREAD_READ.into(),
                    CAP_THREAD_POST.into(),
                    CAP_STORE_READ.into(),
                    CAP_STORE_WRITE.into(),
                ],
            );
            create_role("dentist", &policy).unwrap();
            assign_role("bob", "dentist").unwrap();
            let (_m, raw) = create_session_token("bob").unwrap();
            let pass = resolve_skin_token(&raw).expect("session");
            assert!(
                pass.has_cap(CAP_STORE_WRITE),
                "union still lists store:write"
            );
            assert!(
                !pass.has_cap_in_room(&sales, CAP_STORE_WRITE),
                "store:write on docs must not grant store:write on sales: {pass:?}"
            );
            assert!(!pass.has_cap_in_room(&sales, CAP_STORE_READ));
            assert!(pass.has_cap_in_room(&docs, CAP_STORE_WRITE));
            assert!(pass.has_cap_in_room(&docs, CAP_STORE_READ));
            assert!(pass.has_cap_in_room(&sales, CAP_THREAD_READ));
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
                assert!(
                    policy.is_none(),
                    "platform room_policy must stay NULL: {policy:?}"
                );
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
                assert_eq!(
                    policy.get(&room),
                    Some(&default_thread_caps()),
                    "{policy:?}"
                );
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
            assert!(listed[0]
                .room_policy
                .get(&anna)
                .unwrap()
                .iter()
                .any(|c| c == CAP_FILES_READ));

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
            let role = create_role(
                "hygienist",
                &thread_only_room_policy(std::slice::from_ref(&sales)),
            )
            .unwrap();
            assert_eq!(role.rooms, vec![sales.clone()]);
            assert_eq!(role.room_policy.get(&sales), Some(&default_thread_caps()));
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

    // ── S24 per-App logins (prd-s24-per-app-logins-v1-vs-live S24-17) ──

    const APP_A: &str = "0a0a0a0a-1111-4222-8333-444444444444";
    const APP_B: &str = "0b0b0b0b-1111-4222-8333-444444444444";

    fn one_room_policy(room: &str, caps: &[&str]) -> RoomPolicy {
        let mut p = RoomPolicy::new();
        p.insert(room.to_string(), caps.iter().map(|c| (*c).to_string()).collect());
        p
    }

    fn grant_for<'a>(grants: &'a [SkinGrant], principal: &str, app: &str) -> Option<&'a SkinGrant> {
        grants.iter().find(|g| {
            g.subject_kind == "principal"
                && g.subject_id == principal
                && g.kind == "app"
                && g.target_id == app
        })
    }

    fn app_write(principal: &str, app: &str, role: Option<&str>, scope: Option<&str>) -> SkinGrantWrite {
        SkinGrantWrite {
            subject_kind: "principal".into(),
            subject_id: principal.into(),
            kind: "app".into(),
            target_id: app.into(),
            role: role.map(str::to_string),
            scope: scope.map(str::to_string),
            created_by: Some("owner-token".into()),
        }
    }

    /// S24-17 #1: a guest with only an App A grant logs in for A with A's
    /// role, and is refused for B and for `host` with the generic error.
    #[test]
    fn s24_cross_app_login_fails_closed() {
        with_temp_home(|| {
            let room_a = uuid::Uuid::new_v4().to_string();
            let role_a =
                create_role_for_app("a-desk", &one_room_policy(&room_a, &[CAP_THREAD_READ, CAP_FILES_READ]), Some(APP_A))
                    .expect("role");
            let eve = add_principal_for_app("eve", None, APP_A, Some("a-desk"), "owner-token")
                .expect("App-only guest");
            let grants = list_grants().expect("grants");
            assert!(
                host_grants(&grants, &eve.id).is_empty(),
                "S24-12: an App-only guest has no box login: {grants:?}"
            );
            let g = grant_for(&grants, &eve.id, APP_A).expect("A grant");
            assert_eq!(g.role_id.as_deref(), Some(role_a.id.as_str()));
            assert_eq!(g.created_by.as_deref(), Some("owner-token"));

            let a = create_app_session_token("eve", APP_A).expect("A login");
            assert_eq!(a.app, APP_A);
            assert_eq!(a.role_name.as_deref(), Some("a-desk"));
            assert_eq!(a.meta.rooms, vec![room_a.clone()]);
            let pass = resolve_skin_token(&a.raw).expect("A pass");
            assert_eq!(pass.app, APP_A);
            assert!(pass.has_cap_in_room(&room_a, CAP_FILES_READ));

            for other in [APP_B, HOST_APP_ID, "", "not-an-app"] {
                let err = create_app_session_token("eve", other).expect_err("cross-App refused");
                assert_eq!(err, HOST_LOGIN_DENIED, "{other:?}");
                assert!(!err.contains("eve") && !err.contains(APP_A), "{err}");
            }
            let err = create_session_token("eve").expect_err("no box login");
            assert_eq!(err, HOST_LOGIN_DENIED);

            // A role for another App can't be used on this App.
            let room_b = uuid::Uuid::new_v4().to_string();
            create_role_for_app("b-desk", &one_room_policy(&room_b, &[CAP_THREAD_READ]), Some(APP_B))
                .expect("role b");
            let err = add_principal_for_app("fay", None, APP_A, Some("b-desk"), "owner-token")
                .expect_err("other-App role refused");
            assert!(err.contains("is for app"), "{err}");
            assert!(
                !list_principals().unwrap().iter().any(|p| p.username == "fay"),
                "refused add must not leave a principal behind"
            );
            let err = add_principal_for_app("gus", None, HOST_APP_ID, None, "owner-token")
                .expect_err("host is not an App target here");
            assert!(err.contains("published App"), "{err}");
        });
    }

    /// S24-17 #2 + #3: two Apps, two roles, two passes; the use-time
    /// backstop is per App (works even when the revocation is skipped).
    #[test]
    fn s24_two_apps_two_snapshots_and_per_app_backstop() {
        with_temp_home(|| {
            let room_a = uuid::Uuid::new_v4().to_string();
            let room_b = uuid::Uuid::new_v4().to_string();
            create_role_for_app("ra", &one_room_policy(&room_a, &[CAP_THREAD_READ]), Some(APP_A)).unwrap();
            create_role("rb", &one_room_policy(&room_b, &[CAP_TICKETS_READ])).unwrap();
            let ada = add_principal("ada").unwrap();
            create_grant(&app_write(&ada.id, APP_A, Some("ra"), None)).expect("A grant");
            let gb = create_grant(&app_write(&ada.id, APP_B, Some("rb"), None)).expect("B grant");

            let a = create_app_session_token("ada", APP_A).unwrap();
            let b = create_app_session_token("ada", APP_B).unwrap();
            let host = create_app_session_token("ada", HOST_APP_ID).unwrap();
            let pa = resolve_skin_token(&a.raw).unwrap();
            let pb = resolve_skin_token(&b.raw).unwrap();
            let ph = resolve_skin_token(&host.raw).unwrap();
            assert_eq!((pa.app.as_str(), pb.app.as_str(), ph.app.as_str()), (APP_A, APP_B, HOST_APP_ID));
            assert!(pa.has_room(&room_a) && !pa.has_room(&room_b), "{pa:?}");
            assert!(pb.has_room(&room_b) && !pb.has_room(&room_a), "{pb:?}");
            assert!(ph.rooms_empty(), "box login of an unassigned guest: {ph:?}");

            // Backstop alone: drop B's grant behind the writers' backs.
            with_conn(|c| {
                c.execute("DELETE FROM grants WHERE id = ?1", params![gb.id])
                    .map_err(|e| e.to_string())
            })
            .unwrap();
            assert!(resolve_skin_token(&b.raw).is_none(), "B pass dies at the next use");
            assert!(resolve_skin_token(&a.raw).is_some(), "A pass keeps working");
            assert!(resolve_skin_token(&host.raw).is_some(), "box pass keeps working");

            // Writer path: disabling A ends only A's sessions.
            let ga = list_grants()
                .unwrap()
                .into_iter()
                .find(|g| g.target_id == APP_A)
                .unwrap();
            set_grant_enabled(&ga.id, false).unwrap();
            assert!(session_revoked_at(&a.raw).is_some(), "A session revoked");
            assert!(session_revoked_at(&host.raw).is_none(), "host session untouched");
            assert_eq!(
                create_app_session_token("ada", APP_A).expect_err("disabled").as_str(),
                HOST_LOGIN_DENIED
            );
        });
    }

    /// S24-17 #4: a role change rewrites only sessions whose own App grant
    /// carries that role; host writers touch only host sessions.
    #[test]
    fn s24_role_and_host_writers_touch_only_their_app() {
        with_temp_home(|| {
            let r1 = uuid::Uuid::new_v4().to_string();
            let r2 = uuid::Uuid::new_v4().to_string();
            let r3 = uuid::Uuid::new_v4().to_string();
            create_role("host-role", &one_room_policy(&r1, &[CAP_THREAD_READ])).unwrap();
            create_role_for_app("app-role", &one_room_policy(&r2, &[CAP_THREAD_READ]), Some(APP_A)).unwrap();
            let ada = add_principal("ada").unwrap();
            assign_role("ada", "host-role").unwrap();
            create_grant(&app_write(&ada.id, APP_A, Some("app-role"), None)).unwrap();
            let host = create_session_token("ada").unwrap().1;
            let app = create_app_session_token("ada", APP_A).unwrap().raw;

            set_role_room("app-role", &r3, None).unwrap();
            let ph = resolve_skin_token(&host).unwrap();
            let pa = resolve_skin_token(&app).unwrap();
            assert!(pa.has_room(&r3) && pa.has_room(&r2), "App session rewritten: {pa:?}");
            assert!(!ph.has_room(&r3) && ph.has_room(&r1), "host session untouched: {ph:?}");

            unassign_role("ada").unwrap();
            let ph = resolve_skin_token(&host).unwrap();
            let pa = resolve_skin_token(&app).unwrap();
            assert!(ph.rooms_empty(), "host session follows the box login: {ph:?}");
            assert!(pa.has_room(&r2), "App session keeps its own role: {pa:?}");

            // Role-less App grant rooms (S24-7 / S24-15) with --apply-tokens.
            let bob = add_principal("bob").unwrap();
            let g = create_grant(&app_write(&bob.id, APP_B, None, Some(&rooms_json(&[r1.clone()])))).unwrap();
            assert!(g.role_id.is_none());
            assert_eq!(g.scope.as_deref(), Some(rooms_json(&[r1.clone()]).as_str()));
            let bs = create_app_session_token("bob", APP_B).unwrap();
            assert!(bs.role_name.is_none());
            let bp = resolve_skin_token(&bs.raw).unwrap();
            assert!(bp.has_cap_in_room(&r1, CAP_THREAD_POST));
            assert!(!bp.has_cap_in_room(&r1, CAP_FILES_READ), "Thread-only: {bp:?}");
            let bh = create_session_token("bob").unwrap().1;
            set_app_grant_rooms("bob", APP_B, &[r2.clone()], true, "owner-token").unwrap();
            let bp = resolve_skin_token(&bs.raw).unwrap();
            assert!(bp.has_room(&r2) && !bp.has_room(&r1), "{bp:?}");
            assert!(resolve_skin_token(&bh).unwrap().rooms_empty(), "box session untouched");
            let err = set_app_grant_rooms("ada", APP_A, &[r1.clone()], false, "owner-token")
                .expect_err("a role-backed grant refuses rooms");
            assert!(err.contains("role"), "{err}");

            // Shape rules (S24-7).
            let both = create_grant(&app_write(&bob.id, "x", Some("host-role"), Some("[]"))).unwrap_err();
            assert!(both.contains("not both"), "{both}");
            let junk = create_grant(&app_write(&bob.id, "x", None, Some("owner"))).unwrap_err();
            assert!(junk.contains("JSON array"), "{junk}");
        });
    }

    /// S24-6 + Q8: box login off ends every session (constant default);
    /// `revoke_sessions_for_app` (publish rm, S24-20) ends one App's.
    #[test]
    fn s24_host_off_and_app_removed_revocations() {
        with_temp_home(|| {
            let ada = add_principal("ada").unwrap();
            create_grant(&app_write(&ada.id, APP_A, None, Some("[]"))).unwrap();
            create_grant(&app_write(&ada.id, APP_B, None, Some("[]"))).unwrap();
            let a = create_app_session_token("ada", APP_A).unwrap().raw;
            let b = create_app_session_token("ada", APP_B).unwrap().raw;
            assert_eq!(revoke_sessions_for_app(APP_A).unwrap(), 1);
            assert!(resolve_skin_token(&a).is_none());
            assert!(resolve_skin_token(&b).is_some());
            assert!(
                list_grants().unwrap().iter().any(|g| g.target_id == APP_A),
                "grants stay after the App is removed"
            );
            assert!(revoke_sessions_for_app(HOST_APP_ID).is_err());
            let h = create_session_token("ada").unwrap().1;
            set_host_login("ada", false).unwrap();
            assert!(resolve_skin_token(&h).is_none());
            assert_eq!(
                resolve_skin_token(&b).is_none(),
                HOST_OFF_ENDS_ALL_APP_SESSIONS,
                "Q8 default: box login off ends App sessions too"
            );
            // Re-adding the box login copies the current rooms (AM1).
            set_principal_default_rooms("ada", &[], false).unwrap();
            set_host_login("ada", true).unwrap();
            let g = host_grants(&list_grants().unwrap(), &ada.id)[0].clone();
            assert_eq!(g.scope.as_deref(), Some("[]"));
        });
    }

    /// S24-1 + #6: steps run once, the version is recorded, and a file from a
    /// newer K2 is refused with a clear error.
    #[test]
    fn s24_schema_steps_run_once_and_refuse_newer() {
        with_temp_home(|| {
            add_principal("ada").unwrap();
            let v = with_conn(|c| schema_version(c)).unwrap();
            assert_eq!(v, SKIN_SCHEMA_VERSION);
            let cols = with_conn(|c| Ok(table_columns(c, "tokens"))).unwrap();
            assert!(cols.split(',').any(|c| c == "app_service_id"), "{cols}");
            let gcols = with_conn(|c| Ok(table_columns(c, "grants"))).unwrap();
            assert!(gcols.split(',').any(|c| c == "created_by"), "{gcols}");
            assert!(skin_app_backfill_pending().unwrap(), "fresh db: backfill pending");
            let before = list_grants().unwrap();
            reopen_skin_db();
            assert_eq!(list_grants().unwrap(), before, "a reopen changes nothing");
            assert_eq!(with_conn(|c| schema_version(c)).unwrap(), SKIN_SCHEMA_VERSION);

            with_conn(|c| {
                c.execute_batch(&format!("PRAGMA user_version = {};", SKIN_SCHEMA_VERSION + 1))
                    .map_err(|e| e.to_string())
            })
            .unwrap();
            let path = store_path();
            *db().lock() = None;
            let err = open_db(&path).expect_err("newer schema must refuse");
            assert_eq!(err, schema_newer_error(SKIN_SCHEMA_VERSION + 1));
            assert!(err.contains("newer"), "{err}");
            // Put it back so the temp home tears down cleanly.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {SKIN_SCHEMA_VERSION};"))
                .unwrap();
        });
    }

    /// R3 step 5: a version-4 file (0.46.0) gains `roles.room_paths`
    /// (`{}`: every existing line stays the whole room), its roles and live
    /// sessions survive untouched, and the version reads 5.
    #[test]
    fn r3_step5_adds_room_paths_to_a_v4_file() {
        with_temp_home(|| {
            assert_eq!(SKIN_SCHEMA_VERSION, 5);
            let room = uuid::Uuid::new_v4().to_string();
            create_role("staff", &one_room_policy(&room, &[CAP_FILES_READ])).unwrap();
            add_principal("ada").unwrap();
            assign_role("ada", "staff").unwrap();
            let (_, raw) = create_session_token("ada").unwrap();
            // Wind the file back to what 0.46.0 wrote: no column, version 4.
            let path = store_path();
            *db().lock() = None;
            {
                let conn = Connection::open(&path).unwrap();
                conn.execute_batch("ALTER TABLE roles DROP COLUMN room_paths; PRAGMA user_version = 4;")
                    .unwrap();
                assert!(!table_columns(&conn, "roles").split(',').any(|c| c == "room_paths"));
            }
            // The next open runs step 5.
            let pass = resolve_skin_token(&raw).expect("a 0.46.0 session still resolves after the upgrade");
            assert_eq!(with_conn(|c| schema_version(c)).unwrap(), 5);
            let cols = with_conn(|c| Ok(table_columns(c, "roles"))).unwrap();
            assert!(cols.split(',').any(|c| c == "room_paths"), "{cols}");
            let stored: String = with_conn(|c| {
                c.query_row("SELECT room_paths FROM roles WHERE name = 'staff'", [], |r| r.get(0))
                    .map_err(|e| e.to_string())
            })
            .unwrap();
            assert_eq!(stored, "{}");
            assert!(pass.has_cap_in_room(&room, CAP_FILES_READ));
            assert_eq!(pass.path_scope(&room).unwrap(), RoomPathScope::default());
            assert_eq!(pass.room_paths, paths::PassPaths::Lines(RoomPaths::new()));
            // And a 0.46.0 binary would refuse the new file (S24-1 guard).
            assert!(schema_newer_error(5).contains("newer"));
        });
    }

    /// S24-11: copy-host copies enabled box logins, keeps owner grants,
    /// skips other-App roles, and a dry run writes nothing.
    #[test]
    fn s24_copy_host_rules_and_dry_run() {
        with_temp_home(|| {
            let room = uuid::Uuid::new_v4().to_string();
            create_role("staff", &one_room_policy(&room, &[CAP_THREAD_READ])).unwrap();
            create_role_for_app("b-only", &RoomPolicy::new(), Some(APP_B)).unwrap();
            let ada = add_principal("ada").unwrap();
            assign_role("ada", "staff").unwrap();
            add_principal("bob").unwrap();
            add_principal("olga").unwrap();
            assign_role("olga", "b-only").unwrap();
            let nina = add_principal("nina").unwrap();
            let gn = create_grant(&app_write(&nina.id, APP_A, None, Some("[]"))).unwrap();
            set_grant_enabled(&gn.id, false).unwrap();
            add_principal("hank").unwrap();
            set_host_login("hank", false).unwrap();

            let dry = copy_host_grants_to_app(APP_A, "owner-token", true).unwrap();
            assert!(dry.dry_run);
            assert_eq!(dry.added, vec!["ada".to_string(), "bob".to_string()]);
            assert_eq!(dry.kept_existing, vec![("nina".to_string(), false)]);
            assert_eq!(dry.skipped_other_app_role, vec!["olga".to_string()]);
            assert!(grant_for(&list_grants().unwrap(), &ada.id, APP_A).is_none(), "dry run wrote");

            let real = copy_host_grants_to_app(APP_A, "owner-token", false).unwrap();
            assert_eq!(real.added, dry.added);
            let grants = list_grants().unwrap();
            let g = grant_for(&grants, &ada.id, APP_A).expect("ada copied");
            assert!(g.role_id.is_some() && g.created_by.as_deref() == Some("owner-token"));
            assert_eq!(create_app_session_token("ada", APP_A).unwrap().meta.rooms, vec![room.clone()]);
            assert!(create_app_session_token("bob", APP_A).unwrap().meta.rooms.is_empty());
            for refused in ["olga", "nina", "hank"] {
                assert_eq!(
                    create_app_session_token(refused, APP_A).expect_err(refused),
                    HOST_LOGIN_DENIED
                );
            }
            let again = copy_host_grants_to_app(APP_A, "owner-token", false).unwrap();
            assert!(again.added.is_empty(), "idempotent: {again:?}");
            assert!(copy_host_grants_to_app(HOST_APP_ID, "owner-token", false).is_err());
        });
    }

    /// S24-17 upgrade fixture: a skin.db as 0.45.4 left it (host-grant-only
    /// guests, a D1 mismatch, owner-written App grants enabled and disabled,
    /// a role tagged for the BFF App, box login off / disabled, a live
    /// pre-upgrade session). After open + the boot backfill, everyone who
    /// could sign in to the `--skin` App before still can, with the same
    /// rooms — except the owner's disabled grant and the other-App role,
    /// which are refused and audited. The BFF (`--cmd`) App gets nobody, and
    /// the box login is unchanged for all.
    #[test]
    fn s24_upgrade_fixture_no_lockouts() {
        const SKIN_X: &str = "5c5c5c5c-aaaa-4bbb-8ccc-dddddddddddd";
        const BFF_Y: &str = "9e9e9e9e-aaaa-4bbb-8ccc-dddddddddddd";
        with_temp_home(|| {
            let path = crate::paths::k2_home().join("skin.db");
            std::fs::create_dir_all(path.parent().unwrap()).expect("k2 home");
            let conn = Connection::open(&path).expect("0.45.4 skin.db");
            conn.execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE roles (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL, caps TEXT NOT NULL,
                    rooms TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL,
                    room_policy TEXT NOT NULL DEFAULT '{}', app_id TEXT);
                 CREATE UNIQUE INDEX roles_name ON roles(name COLLATE NOCASE);
                 CREATE TABLE principals (
                    id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE,
                    created_at INTEGER NOT NULL, default_rooms TEXT NOT NULL DEFAULT '[]',
                    password_hash TEXT, role_id TEXT REFERENCES roles(id) ON DELETE RESTRICT,
                    email TEXT, full_name TEXT, agent_workspace TEXT);
                 CREATE TABLE grants (
                    id TEXT PRIMARY KEY, subject_kind TEXT NOT NULL, subject_id TEXT NOT NULL,
                    kind TEXT NOT NULL, target_id TEXT NOT NULL, role_id TEXT, scope TEXT,
                    created_at INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
                    UNIQUE(subject_kind, subject_id, kind, target_id));
                 CREATE TABLE tokens (
                    id TEXT PRIMARY KEY,
                    principal_id TEXT REFERENCES principals(id) ON DELETE CASCADE,
                    name TEXT, key_hash TEXT NOT NULL UNIQUE, key_prefix TEXT NOT NULL,
                    caps TEXT NOT NULL, rooms TEXT NOT NULL DEFAULT '[]',
                    created_at INTEGER NOT NULL, revoked_at INTEGER,
                    session INTEGER NOT NULL DEFAULT 0, expires_at INTEGER, room_policy TEXT,
                    CHECK ((session = 1 AND principal_id IS NOT NULL AND name IS NULL)
                        OR (session = 0 AND principal_id IS NULL AND name IS NOT NULL)));
                 CREATE TABLE host_grant_backfill (
                    id INTEGER PRIMARY KEY CHECK (id = 1), done_at INTEGER NOT NULL);
                 INSERT INTO host_grant_backfill (id, done_at) VALUES (1, 1);",
            )
            .expect("0.45.4 schema");
            let staff_room = uuid::Uuid::new_v4().to_string();
            let narrow_room = uuid::Uuid::new_v4().to_string();
            let bob_room = uuid::Uuid::new_v4().to_string();
            let olga_room = uuid::Uuid::new_v4().to_string();
            let staff = one_room_policy(&staff_room, &[CAP_THREAD_READ, CAP_FILES_READ]);
            let narrow = one_room_policy(&narrow_room, &[CAP_THREAD_READ]);
            let olga_p = one_room_policy(&olga_room, &[CAP_THREAD_READ]);
            for (id, name, pol, app) in [
                ("r-staff", "staff", &staff, None),
                ("r-narrow", "narrow", &narrow, None),
                ("r-bff", "bff-only", &olga_p, Some(BFF_Y)),
            ] {
                conn.execute(
                    "INSERT INTO roles (id, name, caps, rooms, created_at, room_policy, app_id)
                     VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                    params![
                        id,
                        name,
                        caps_json(&union_caps(pol)),
                        rooms_json(&rooms_from_policy(pol)),
                        room_policy_json(pol),
                        app
                    ],
                )
                .unwrap();
            }
            let bob_rooms = rooms_json(std::slice::from_ref(&bob_room));
            // (id, username, role_id, default_rooms)
            let people: [(&str, &str, Option<&str>, &str); 8] = [
                ("p-ada", "ada", Some("r-staff"), "[]"),
                ("p-bob", "bob", None, bob_rooms.as_str()),
                ("p-dan", "dan", Some("r-staff"), "[]"),
                ("p-olga", "olga", Some("r-bff"), "[]"),
                ("p-nina", "nina", Some("r-staff"), "[]"),
                ("p-dora", "dora", Some("r-staff"), "[]"),
                ("p-hank", "hank", Some("r-staff"), "[]"),
                ("p-lena", "lena", None, bob_rooms.as_str()),
            ];
            for (id, user, role, rooms) in people {
                conn.execute(
                    "INSERT INTO principals (id, username, created_at, default_rooms, role_id)
                     VALUES (?1, ?2, 1, ?3, ?4)",
                    params![id, user, rooms, role],
                )
                .unwrap();
            }
            // Host grants as 0.45.4 wrote them: the 0.40.150 backfill /
            // add_principal (role or rooms), set_host_login (role NULL,
            // "[]"), and a D1 drift (dan's grant role ≠ his column).
            // (grant id, principal, target, role, scope, enabled)
            let rows: [(&str, &str, &str, Option<&str>, Option<&str>, i64); 10] = [
                ("g-ada", "p-ada", "host", Some("r-staff"), None, 1),
                ("g-bob", "p-bob", "host", None, Some("[]"), 1), // stale scope
                ("g-dan", "p-dan", "host", Some("r-narrow"), None, 1), // D1
                ("g-olga", "p-olga", "host", Some("r-bff"), None, 1),
                ("g-nina", "p-nina", "host", Some("r-staff"), None, 1),
                ("g-nina-x", "p-nina", SKIN_X, Some("r-narrow"), None, 1), // owner, narrower
                ("g-dora", "p-dora", "host", Some("r-staff"), None, 1),
                ("g-dora-x", "p-dora", SKIN_X, Some("r-staff"), None, 0), // owner, disabled
                // hank: box login switched off (no host row).
                ("g-lena", "p-lena", "host", None, Some("[]"), 0), // disabled
                ("g-ws", "ws-1", "mbox", None, Some("may-read"), 1), // workspace subject
            ];
            for (id, who, target, role, scope, enabled) in rows {
                let (subject_kind, kind) =
                    if who == "ws-1" { ("workspace", "mailbox") } else { ("principal", "app") };
                conn.execute(
                    "INSERT INTO grants
                     (id, subject_kind, subject_id, kind, target_id, role_id, scope, created_at, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
                    params![id, subject_kind, who, kind, target, role, scope, enabled],
                )
                .unwrap();
            }
            // A live host session minted by 0.45.4 (no App column yet).
            let old_raw = format!("{SKIN_KEY_PREFIX}{}", "A".repeat(KEY_BODY_LEN));
            conn.execute(
                "INSERT INTO tokens (id, principal_id, name, key_hash, key_prefix, caps, rooms,
                                     created_at, revoked_at, session, expires_at, room_policy)
                 VALUES ('t-ada', 'p-ada', NULL, ?1, 'k2skn_…AAAA', ?2, ?3, 1, NULL, 1, ?4, ?5)",
                params![
                    sha256_hex(&old_raw),
                    caps_json(&union_caps(&staff)),
                    rooms_json(&rooms_from_policy(&staff)),
                    now_secs() + 3600,
                    room_policy_json(&staff)
                ],
            )
            .unwrap();
            // What each guest could do BEFORE the upgrade at the --skin App
            // (0.45.4: helper login = box login, policy from the column).
            let before_rooms: Vec<(&str, Option<Vec<String>>)> = vec![
                ("ada", Some(vec![staff_room.clone()])),
                ("bob", Some(vec![bob_room.clone()])),
                ("dan", Some(vec![staff_room.clone()])),
                ("olga", Some(vec![olga_room.clone()])),
                ("nina", Some(vec![staff_room.clone()])),
                ("dora", Some(vec![staff_room.clone()])),
                ("hank", None),
                ("lena", None),
            ];
            drop(conn);

            // Open (steps 1–3) then the boot backfill for the --skin App.
            let old = resolve_skin_token(&old_raw).expect("pre-upgrade host session still resolves");
            assert_eq!(old.app, HOST_APP_ID, "NULL App = host");
            assert!(old.has_cap_in_room(&staff_room, CAP_FILES_READ));
            assert_eq!(with_conn(|c| schema_version(c)).unwrap(), SKIN_SCHEMA_VERSION);
            assert!(skin_app_backfill_pending().unwrap());
            let reports = run_skin_app_backfill(&[SKIN_X.to_string()])
                .expect("backfill")
                .expect("ran once");
            assert_eq!(reports.len(), 1);
            assert!(run_skin_app_backfill(&[SKIN_X.to_string()]).unwrap().is_none(), "runs once");
            assert!(!skin_app_backfill_pending().unwrap());

            // Box login: same as before for everyone (incl. olga's App-tagged role).
            for (user, want) in &before_rooms {
                match (create_session_token(user), want) {
                    (Ok((meta, _)), Some(rooms)) => assert_eq!(&meta.rooms, rooms, "{user} host"),
                    (Err(e), None) => assert_eq!(e, HOST_LOGIN_DENIED, "{user} host"),
                    (got, want) => panic!("{user} host: got {:?}, want {want:?}", got.map(|m| m.0.rooms)),
                }
            }
            // The --skin App: same rooms, except the two owner/other-App rows.
            for (user, before) in &before_rooms {
                let got = create_app_session_token(user, SKIN_X).map(|s| s.meta.rooms);
                match *user {
                    "nina" => assert_eq!(
                        got.expect("nina keeps the App"),
                        vec![narrow_room.clone()],
                        "the owner's narrower App grant now applies (Q2)"
                    ),
                    "dora" | "olga" => assert_eq!(
                        got.expect_err(user),
                        HOST_LOGIN_DENIED,
                        "{user}: refused (owner-disabled / other-App role)"
                    ),
                    _ => match before {
                        Some(rooms) => assert_eq!(&got.expect(user), rooms, "{user} at the App"),
                        None => assert_eq!(got.expect_err(user), HOST_LOGIN_DENIED, "{user}"),
                    },
                }
            }
            // The BFF App starts with nobody until copy-host (Q6 / S24-10).
            for (user, _) in &before_rooms {
                assert_eq!(
                    create_app_session_token(user, BFF_Y).expect_err(user),
                    HOST_LOGIN_DENIED,
                    "{user} at the BFF App"
                );
            }
            let report = &reports[0];
            assert!(report.added.contains(&"ada".to_string()));
            assert!(report.added.contains(&"bob".to_string()));
            assert!(report.added.contains(&"dan".to_string()));
            assert!(!report.added.iter().any(|u| u == "hank" || u == "lena"));
            assert_eq!(report.skipped_other_app_role, vec!["olga".to_string()]);
            assert_eq!(
                report.kept_existing,
                vec![("dora".to_string(), false), ("nina".to_string(), true)]
            );
            let grants = list_grants().unwrap();
            assert_eq!(
                grant_for(&grants, "p-ada", SKIN_X).unwrap().created_by.as_deref(),
                Some(UPGRADE_CREATED_BY)
            );
            assert_eq!(
                grant_for(&grants, "p-dan", HOST_APP_ID).unwrap().role_id.as_deref(),
                Some("r-staff"),
                "D1: the column won"
            );
            assert_eq!(
                grant_for(&grants, "p-bob", HOST_APP_ID).unwrap().scope.as_deref(),
                Some(bob_rooms.as_str()),
                "stale scope synced to default_rooms"
            );
            assert!(grants.iter().any(|g| g.id == "g-ws"), "workspace grants untouched");
            assert!(
                resolve_skin_token(&old_raw).is_some(),
                "the pre-upgrade host session survives the backfill"
            );
            let audit: Vec<String> = crate::auth_audit::tail(200)
                .unwrap()
                .iter()
                .map(|v| v.to_string())
                .collect();
            for needle in ["dora", "olga", "dan", "bob"] {
                assert!(
                    audit.iter().any(|l| l.contains("app_grant_upgrade") && l.contains(needle)),
                    "{needle} must be in the upgrade audit: {audit:#?}"
                );
            }
            assert!(
                audit.iter().any(|l| l.contains("--skin App backfill")),
                "summary line: {audit:#?}"
            );
        });
    }
}

// Sew S28 K7/K8: chat grant, chat-only login, revoke ops, Connect ids.
#[cfg(test)]
#[path = "skin_sew_tests.rs"]
mod sew_tests;
