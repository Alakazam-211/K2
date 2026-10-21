//! "Which agents each person can reach, clearly defined"
//! (app-mobile-access-v1, 0.46.3; `research-app-mobile-surface-parity-v1`
//! §3.2, §5 C).
//!
//! - [`can_of`]: what a person may do in one room, from its caps, so an App
//!   draws buttons without knowing cap names (`me.get` → `agents[].can`).
//! - [`grid`]: the owner's person × agent view of one App (Settings → Apps
//!   → *App* → People, `k2 app access`), built from the same grant → role →
//!   room line the doors read, so it can't drift from what a pass gets.
//! - Suggested role templates ([`TEMPLATES`]; room-scope Q2 as changed by
//!   Rosson 2026-10-20, "not every app will be the same"): never created on
//!   their own. The owner or the App's agent makes a real, editable role
//!   from one ([`create_from_template`], `k2 app roles add --from <id>`).

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

use super::{
    create_role_for_app, grant_login_policy, map_grant_row, normalize_app_target, normalize_username, paths,
    with_conn, RoomPathScope, RoomPolicy, SkinRole, CAP_ACTIVITY_READ, CAP_ACTIVITY_ROOM, CAP_AGENT_IDENTITY, CAP_FILES_READ,
    CAP_FILES_WRITE, CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE, CAP_ROOM_MEMBERS, CAP_STORE_READ, CAP_STORE_WRITE,
    CAP_THREAD_POST, CAP_THREAD_READ, CAP_TICKETS_POST, CAP_TICKETS_READ, CAP_WIKI_READ, GRANT_SELECT,
};

// ── can ──────────────────────────────────────────────────────────────

fn level(caps: &BTreeSet<&str>, read: &str, write: &str) -> &'static str {
    if caps.contains(write) {
        "write"
    } else if caps.contains(read) {
        "read"
    } else {
        "none"
    }
}

/// What one room's caps let a person do (`me.get` → `agents[].can`).
/// `files`, `tickets`, `heartbeats` and `store` are `none` / `read` /
/// `write` (tickets `write` = may post); the rest are booleans.
pub fn can_of(caps: &[String]) -> Value {
    let c: BTreeSet<&str> = caps.iter().map(String::as_str).collect();
    json!({
        "chat": c.contains(CAP_THREAD_READ),
        "send": c.contains(CAP_THREAD_POST),
        "files": level(&c, CAP_FILES_READ, CAP_FILES_WRITE),
        "tickets": level(&c, CAP_TICKETS_READ, CAP_TICKETS_POST),
        // `activity`: the pinned-Chat ear (counts only); `activityRoom`
        // (0.46.3): the room-wide counts, every session in the room.
        "activity": c.contains(CAP_ACTIVITY_READ),
        "activityRoom": c.contains(CAP_ACTIVITY_ROOM),
        "members": c.contains(CAP_ROOM_MEMBERS),
        "heartbeats": level(&c, CAP_HEARTBEATS_READ, CAP_HEARTBEATS_WRITE),
        "wiki": c.contains(CAP_WIKI_READ),
        "store": level(&c, CAP_STORE_READ, CAP_STORE_WRITE),
        "identity": c.contains(CAP_AGENT_IDENTITY),
    })
}

// ── suggested role templates ─────────────────────────────────────────

/// One suggested role: a template the owner or the App's agent turns into
/// a real, editable role. Never created on its own.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleTemplate {
    /// The template id (`--from`), also the default name suffix.
    pub id: &'static str,
    pub label: &'static str,
    /// Who it is for.
    pub description: &'static str,
    /// The caps on each room line the role starts with (empty = no room
    /// lines: the App's own policy decides).
    pub caps: &'static [&'static str],
    /// Folders each room line starts with (room-scope R3), when any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folders: Option<TemplateFolders>,
    /// What today's caps can't express yet (shown with the suggestion).
    #[serde(skip_serializing_if = "str::is_empty")]
    pub note: &'static str,
}

/// A template's starting folders on each room line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFolders {
    pub read: &'static [&'static str],
    pub write: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_dir: Option<&'static str>,
}

const MANAGER_CAPS: &[&str] = &[
    CAP_THREAD_READ,
    CAP_THREAD_POST,
    CAP_FILES_READ,
    CAP_FILES_WRITE,
    CAP_TICKETS_READ,
    CAP_TICKETS_POST,
    CAP_STORE_READ,
    CAP_STORE_WRITE,
    CAP_ACTIVITY_READ,
    CAP_ROOM_MEMBERS,
];

/// The suggestions (from a customer's agent, agent-nsi 2026-10-20), mapped
/// to the caps that exist today. The list is data: more come later.
pub const TEMPLATES: &[RoleTemplate] = &[
    RoleTemplate {
        id: "owner",
        label: "Owner",
        description: "Runs the business and sees everything in the App: chat, files, tickets, data, activity, heartbeats and who is in each room.",
        caps: &[
            CAP_THREAD_READ,
            CAP_THREAD_POST,
            CAP_FILES_READ,
            CAP_FILES_WRITE,
            CAP_TICKETS_READ,
            CAP_TICKETS_POST,
            CAP_STORE_READ,
            CAP_STORE_WRITE,
            CAP_ACTIVITY_READ,
            CAP_ROOM_MEMBERS,
            CAP_HEARTBEATS_READ,
            CAP_WIKI_READ,
        ],
        folders: None,
        note: "Approve/sign/send and download as their own caps arrive later; for now reading a file is downloading it.",
    },
    RoleTemplate {
        id: "manager",
        label: "Manager",
        description: "Runs a set of jobs or accounts: add the agents for their jobs and the shared desks.",
        caps: MANAGER_CAPS,
        folders: None,
        note: "Approve/sign/send arrives later. Data writes reach every row of the room's tables (no per-job rows yet): narrow it with rooms and folders.",
    },
    RoleTemplate {
        id: "staff",
        label: "Staff",
        description: "Does daily work at one desk: their own agent plus one shared desk.",
        caps: &[
            CAP_THREAD_READ,
            CAP_THREAD_POST,
            CAP_FILES_READ,
            CAP_FILES_WRITE,
            CAP_TICKETS_READ,
            CAP_TICKETS_POST,
            CAP_STORE_READ,
            CAP_ACTIVITY_READ,
        ],
        folders: None,
        note: "Data is read-only here: writing only their own rows arrives later. Download as its own cap arrives later.",
    },
    RoleTemplate {
        id: "field",
        label: "Field",
        description: "On site, mostly on a phone: the agent for their job, an Inbox to upload into and a short list to read.",
        caps: &[
            CAP_THREAD_READ,
            CAP_THREAD_POST,
            CAP_FILES_READ,
            CAP_FILES_WRITE,
            CAP_TICKETS_READ,
            CAP_TICKETS_POST,
            CAP_STORE_WRITE,
        ],
        folders: Some(TemplateFolders { read: &["Plans", "Start Here"], write: &["Inbox"], upload_dir: Some("Inbox") }),
        note: "Upload-only arrives later; for now Field can read and write in Inbox. Data writes reach every row of the room's tables until own-row writes land.",
    },
    RoleTemplate {
        id: "client",
        label: "Client",
        description: "A customer who sees their own work only: one client-facing agent, a Shared folder to read and an Inbox to upload into.",
        caps: &[CAP_THREAD_READ, CAP_THREAD_POST, CAP_FILES_READ, CAP_FILES_WRITE, CAP_TICKETS_READ, CAP_TICKETS_POST],
        folders: Some(TemplateFolders { read: &["Shared"], write: &["Inbox"], upload_dir: Some("Inbox") }),
        note: "Upload-only arrives later; for now a Client can read and write in Inbox. No data access yet: reading only their own rows arrives later.",
    },
    RoleTemplate {
        id: "vendor",
        label: "Vendor or partner",
        description: "An outside party who sends paperwork into one intake agent's Uploads folder.",
        caps: &[CAP_FILES_READ, CAP_FILES_WRITE],
        folders: Some(TemplateFolders { read: &[], write: &["Uploads"], upload_dir: Some("Uploads") }),
        note: "No Thread or tickets. Upload-only and a folder per vendor arrive later; for now everyone on this role shares Uploads (read and write).",
    },
    RoleTemplate {
        id: "auditor",
        label: "Viewer or auditor",
        description: "Read-only oversight: reads the Thread, files, data and activity in the rooms granted.",
        caps: &[CAP_THREAD_READ, CAP_FILES_READ, CAP_STORE_READ, CAP_ACTIVITY_READ],
        folders: None,
        note: "Read-only Thread is thread:read without thread:post. Download as its own cap arrives later.",
    },
    RoleTemplate {
        id: "app",
        label: "App (service account)",
        description: "The App's own pass: no room lines of its own; the App's manifest and your grants (guests:rooms, data) decide.",
        caps: &[],
        folders: None,
        note: "Never person-only actions.",
    },
];

/// One template by id (case-insensitive).
pub fn template(id: &str) -> Option<&'static RoleTemplate> {
    let id = id.trim();
    TEMPLATES.iter().find(|t| t.id.eq_ignore_ascii_case(id))
}

/// A role-name base from an App's name: lowercase, `[a-z0-9_-]`, other
/// characters to `-`, at most 40 characters; `app` when nothing is left.
pub fn name_base(app_name: &str) -> String {
    let mut out = String::new();
    for c in app_name.trim().to_ascii_lowercase().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
        if out.len() >= 40 {
            break;
        }
    }
    let t = out.trim_matches('-').to_string();
    if t.len() < 2 {
        "app".to_string()
    } else {
        t
    }
}

fn name_taken(conn: &Connection, name: &str) -> Result<bool, String> {
    conn.query_row("SELECT COUNT(*) FROM roles WHERE name = ?1 COLLATE NOCASE", params![name], |r| r.get::<_, i64>(0))
        .map(|n| n > 0)
        .map_err(|e| format!("role name: {e}"))
}

/// Create a role from a template for one App (`host` = a box-wide role).
/// `name` (optional) is the role's name; by default `<app name>-<template>`
/// with `-2`, `-3`… when taken (an explicit name that's taken is `Err`).
/// `rooms` are project ids: each gets the template's caps and folders (a
/// template with no caps gets no room lines). From then on it is an
/// ordinary role: edit it like any other.
pub fn create_from_template(
    template_id: &str,
    app: &str,
    app_name: &str,
    name: Option<&str>,
    rooms: &[String],
) -> Result<SkinRole, String> {
    let Some(t) = template(template_id) else {
        let ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        return Err(format!("unknown role template {template_id:?} (one of: {})", ids.join(", ")));
    };
    let app = normalize_app_target(Some(app));
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => normalize_username(n)?,
        None => {
            let base = name_base(app_name);
            with_conn(|conn| {
                let mut name = normalize_username(&format!("{base}-{}", t.id))?;
                let mut n = 2;
                while name_taken(conn, &name)? {
                    name = normalize_username(&format!("{base}-{}-{n}", t.id))?;
                    n += 1;
                }
                Ok(name)
            })?
        }
    };
    let mut policy = RoomPolicy::new();
    if !t.caps.is_empty() {
        for r in rooms.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
            policy.insert(r.to_string(), t.caps.iter().map(|c| (*c).to_string()).collect());
        }
    }
    let app_id = (app != super::HOST_APP_ID).then_some(app.as_str());
    let mut role = create_role_for_app(&name, &policy, app_id)?;
    if let Some(f) = &t.folders {
        let scope = RoomPathScope {
            read: f.read.iter().map(|s| (*s).to_string()).collect(),
            write: f.write.iter().map(|s| (*s).to_string()).collect(),
            upload_dir: f.upload_dir.map(str::to_string),
        };
        let rooms: Vec<String> = role.room_policy.keys().cloned().collect();
        for room in rooms {
            role = paths::set_role_room_paths(&role.id, &room, &scope)?;
        }
    }
    Ok(role)
}

// ── the owner's grid ─────────────────────────────────────────────────

/// One agent (room) column of the grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridAgent {
    pub room: String,
    pub handle: String,
    pub display_name: String,
    pub color: Option<String>,
}

/// One person × agent cell: what that person may do in that room.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridCell {
    pub room: String,
    pub handle: String,
    pub caps: Vec<String>,
    pub can: Value,
    /// The room line's folders (absent = the whole room).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folders: Option<RoomPathScope>,
}

/// One person row of the grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridPerson {
    pub principal_id: String,
    pub username: String,
    pub full_name: Option<String>,
    /// `guest`, or `agent` for a login marked as an agent's.
    pub kind: &'static str,
    /// The grant is on (a disabled grant reaches nothing: no cells).
    pub enabled: bool,
    pub grant_id: String,
    pub role_id: Option<String>,
    pub role: Option<String>,
    /// Only live rooms, in the grid's agent order.
    pub cells: Vec<GridCell>,
}

/// One App's person × agent grid.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    pub app: String,
    pub agents: Vec<GridAgent>,
    pub people: Vec<GridPerson>,
}

struct RawPerson {
    principal_id: String,
    username: String,
    full_name: Option<String>,
    agent: bool,
    enabled: bool,
    grant_id: String,
    role_id: Option<String>,
    role: Option<String>,
    policy: RoomPolicy,
    paths: paths::RoomPaths,
}

fn raw_people(conn: &Connection, app: &str) -> Result<Vec<RawPerson>, String> {
    let grants = {
        let mut stmt = conn
            .prepare(&format!(
                "{GRANT_SELECT} WHERE subject_kind = 'principal' AND kind = 'app' AND target_id = ?1 ORDER BY created_at, id"
            ))
            .map_err(|e| format!("access grid: {e}"))?;
        let rows = stmt.query_map(params![app], map_grant_row).map_err(|e| format!("access grid: {e}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| format!("access grid: {e}"))?
    };
    let mut out = Vec::new();
    for g in grants {
        let who: Option<(String, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT username, full_name, agent_workspace FROM principals WHERE id = ?1",
                params![g.subject_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("access grid principal: {e}"))?;
        let Some((username, full_name, agent_ws)) = who else { continue };
        // The same policy a login through this grant snapshots; a grant
        // whose role is gone reaches nothing (login refuses it too).
        let (policy, role) = grant_login_policy(conn, &g).unwrap_or_default();
        let paths = match g.role_id.as_deref() {
            Some(rid) => paths::role_room_paths(conn, rid).unwrap_or_default(),
            None => paths::RoomPaths::new(),
        };
        out.push(RawPerson {
            principal_id: g.subject_id.clone(),
            username,
            full_name: full_name.filter(|s| !s.trim().is_empty()),
            agent: agent_ws.is_some_and(|w| !w.trim().is_empty()),
            enabled: g.enabled,
            grant_id: g.id.clone(),
            role_id: g.role_id.clone(),
            role,
            policy,
            paths,
        });
    }
    Ok(out)
}

fn cells_for(policy: &RoomPolicy, paths: &paths::RoomPaths, agents: &[GridAgent]) -> Vec<GridCell> {
    agents
        .iter()
        .filter_map(|a| {
            let caps = policy.get(&a.room)?;
            Some(GridCell {
                room: a.room.clone(),
                handle: a.handle.clone(),
                caps: caps.clone(),
                can: can_of(caps),
                folders: paths.get(&a.room).filter(|s| !s.is_empty()).cloned(),
            })
        })
        .collect()
}

/// One App's grid: every person with a grant on it (enabled or not), and
/// every live agent any of them reaches. `app` is `host` or a published
/// service id.
pub fn grid(app: &str) -> Result<Grid, String> {
    let app = normalize_app_target(Some(app));
    let raw = with_conn(|conn| raw_people(conn, &app))?;
    let mut rooms: Vec<String> = Vec::new();
    for p in &raw {
        for r in p.policy.keys() {
            if !rooms.contains(r) {
                rooms.push(r.clone());
            }
        }
    }
    let mut agents: Vec<GridAgent> = super::live_agents(&rooms)
        .into_iter()
        .map(|a| GridAgent { room: a.project_id, handle: a.handle, display_name: a.display_name, color: a.color })
        .collect();
    agents.sort_by(|a, b| a.handle.cmp(&b.handle));
    let mut raw = raw;
    raw.sort_by(|a, b| a.username.cmp(&b.username));
    let people = raw
        .into_iter()
        .map(|p| GridPerson {
            cells: if p.enabled { cells_for(&p.policy, &p.paths, &agents) } else { Vec::new() },
            principal_id: p.principal_id,
            username: p.username,
            full_name: p.full_name,
            kind: if p.agent { "agent" } else { "guest" },
            enabled: p.enabled,
            grant_id: p.grant_id,
            role_id: p.role_id,
            role: p.role,
        })
        .collect();
    Ok(Grid { app, agents, people })
}

/// One person's row of an App's grid (`k2 app access <app> <user>`), or
/// `None` when they hold no grant on it.
pub fn person(app: &str, username: &str) -> Result<Option<GridPerson>, String> {
    let username = normalize_username(username)?;
    Ok(grid(app)?.people.into_iter().find(|p| p.username == username))
}

#[cfg(test)]
#[path = "skin_access_tests.rs"]
mod tests;
