//! Person wallets (prd-app-user-llm-logins-v1 rev 2): what the wallet
//! needs to know about an App user from skin.db, read live.
//!
//! - [`person`]: does the principal exist, is it a person (not an agent's
//!   own login), and which rooms it holds through each App it has an
//!   enabled `kind=app` grant on (the role's rooms, or the grant's scope
//!   rooms for a role-less grant).
//! - [`people_holding_room`]: every person (agent logins excluded) that
//!   holds a room through any enabled App grant (the single-person rule).
//!
//! Read-only; nothing here writes skin.db.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};

use super::{grant_login_policy, map_grant_row, with_conn, SkinGrant, GRANT_SELECT, HOST_APP_ID};

/// One App user, as the wallet sees them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Person {
    pub principal_id: String,
    pub username: String,
    pub full_name: Option<String>,
    /// `principals.agent_workspace` is set: an agent's own login, never a
    /// person (no wallet).
    pub is_agent_login: bool,
    /// App service id → rooms (`projects.id`) the person holds through that
    /// App's enabled grant. `host` (the box login) is listed too; callers
    /// decide whether it counts. A tied person (agent-app-users R2) shows
    /// only what their ceiling lets them use, never the wider access the
    /// owner gave them that waits for their password reset.
    pub apps: BTreeMap<String, Vec<String>>,
    /// The workspace an agent tie holds this person to (`None`: untied).
    pub tied_to: Option<String>,
    /// Tied, and their grants reach past the ceiling: the extra access
    /// waits for their own password reset.
    pub pending_reset: bool,
}

impl Person {
    /// Holds `room` through `app`'s enabled grant.
    pub fn holds_room_via(&self, app: &str, room: &str) -> bool {
        self.apps.get(app).is_some_and(|rooms| rooms.iter().any(|r| r == room))
    }

    /// Holds at least one enabled grant on a published App (not `host`).
    pub fn has_any_app(&self) -> bool {
        self.apps.keys().any(|a| a != HOST_APP_ID)
    }

    /// Holds an enabled grant on `app`.
    pub fn has_app(&self, app: &str) -> bool {
        self.apps.contains_key(app)
    }

    /// First name for copy ("Patty's Claude"): full name's first word,
    /// else the username.
    pub fn first_name(&self) -> String {
        self.full_name
            .as_deref()
            .and_then(|n| n.split_whitespace().next())
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.username)
            .to_string()
    }
}

fn enabled_app_grants(conn: &Connection, principal_id: &str) -> Result<Vec<SkinGrant>, String> {
    let mut st = conn
        .prepare(&format!(
            "{GRANT_SELECT} WHERE subject_kind = 'principal' AND subject_id = ?1 AND kind = 'app' AND enabled != 0"
        ))
        .map_err(|e| format!("skin wallet grants: {e}"))?;
    let rows = st
        .query_map(params![principal_id], map_grant_row)
        .map_err(|e| format!("skin wallet grants: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("skin wallet grants: {e}"))
}

fn person_in(conn: &Connection, principal_id: &str) -> Result<Option<Person>, String> {
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT username, full_name, agent_workspace FROM principals WHERE id = ?1",
            params![principal_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("skin wallet principal: {e}"))?;
    let Some((username, full_name, agent_ws)) = row else { return Ok(None) };
    let mut apps = BTreeMap::new();
    for g in enabled_app_grants(conn, principal_id)? {
        // A grant whose role is gone reads as no rooms (fail closed).
        let rooms: Vec<String> = match grant_login_policy(conn, &g) {
            Ok((policy, _)) => policy.keys().cloned().collect(),
            Err(_) => Vec::new(),
        };
        apps.insert(g.target_id.clone(), rooms);
    }
    Ok(Some(Person {
        principal_id: principal_id.to_string(),
        username,
        full_name: full_name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        is_agent_login: agent_ws.map(|s| !s.trim().is_empty()).unwrap_or(false),
        apps,
        tied_to: None,
        pending_reset: false,
    }))
}

/// Hold a tied person to their ceiling (agent-app-users R2): Apps outside
/// it are dropped, rooms other than the tie's are dropped. Runs after the
/// skin mutex is released (the App → workspace reads the daemon DB, A21).
fn clamp_person(mut p: Person, tie: Option<super::ties::Tie>) -> Result<Person, String> {
    let Some(tie) = tie else { return Ok(p) };
    let mut apps = BTreeMap::new();
    for (app, rooms) in std::mem::take(&mut p.apps) {
        let ws = super::ties::app_workspace(&app);
        if !super::ties::app_inside(&tie, &app, ws.as_deref()) {
            continue;
        }
        apps.insert(app, rooms.into_iter().filter(|r| *r == tie.workspace_id).collect());
    }
    p.apps = apps;
    let grants = super::list_grants()?;
    let roles = super::list_roles()?;
    p.pending_reset = super::ties::pending(&tie, &p.principal_id, &grants, &roles, &|a: &str| super::ties::app_workspace(a));
    p.tied_to = Some(tie.workspace_id);
    Ok(p)
}

/// The App user behind `principal_id`, or `None` when no such principal.
pub fn person(principal_id: &str) -> Result<Option<Person>, String> {
    let pid = principal_id.trim();
    let (p, tie) = with_conn(|conn| Ok((person_in(conn, pid)?, super::ties::get_conn(conn, pid)?)))?;
    p.map(|p| clamp_person(p, tie)).transpose()
}

/// The App user named `username`, or `None`.
pub fn person_by_username(username: &str) -> Result<Option<Person>, String> {
    let u = super::normalize_username(username)?;
    let (p, tie) = with_conn(|conn| {
        let id: Option<String> = conn
            .query_row("SELECT id FROM principals WHERE username = ?1", params![u], |r| r.get(0))
            .optional()
            .map_err(|e| format!("skin wallet principal: {e}"))?;
        match id {
            Some(id) => Ok((person_in(conn, &id)?, super::ties::get_conn(conn, &id)?)),
            None => Ok((None, None)),
        }
    })?;
    p.map(|p| clamp_person(p, tie)).transpose()
}

/// Every person (agent logins excluded) holding `room` through any enabled
/// App grant, the box login included. Sorted principal ids.
pub fn people_holding_room(room: &str) -> Result<Vec<String>, String> {
    let room = room.trim();
    with_conn(|conn| {
        let mut st = conn
            .prepare("SELECT id FROM principals WHERE agent_workspace IS NULL OR TRIM(agent_workspace) = ''")
            .map_err(|e| format!("skin wallet people: {e}"))?;
        let ids: Vec<String> = st
            .query_map([], |r| r.get(0))
            .map_err(|e| format!("skin wallet people: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("skin wallet people: {e}"))?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(p) = person_in(conn, &id)? {
                if p.apps.values().any(|rooms| rooms.iter().any(|r| r == room)) {
                    out.push(id);
                }
            }
        }
        out.sort();
        Ok(out)
    })
}
