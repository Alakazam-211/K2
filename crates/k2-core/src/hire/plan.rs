//! Plan a recipe hire before any write: every member's folder and
//! predicted handle, the team name, the links, and (for an API key) the
//! ceiling and quota checks. Any miss refuses with nothing written.
//!
//! Handles are predicted with the same allocator registration uses
//! (HT12). A rolled-back member's handle stays retired for good (0159), so
//! the plan names the handles up front.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::limits::HireLimits;
use super::links::{self, Ws};
use super::recipe::{HireRequest, MemberRecipe};
use super::{HireCaller, HireError};

#[derive(Debug, Clone)]
pub struct PlannedMember {
    pub recipe: MemberRecipe,
    pub role: String,
    pub handle: String,
    pub path: PathBuf,
    pub lead: bool,
    pub app_login: Option<PlannedAppLogin>,
}

/// S5: the App login a member gets (resolved, checked against the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAppLogin {
    pub app: String,
    pub username: String,
    pub email: Option<String>,
    /// Workspace ids or `self`; empty when a role decides the rooms.
    pub rooms: Vec<String>,
    pub role: Option<String>,
    pub password: bool,
}

#[derive(Debug, Clone)]
pub struct PlannedLink {
    pub target: Ws,
    pub kind: &'static str,
    pub scope: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub base: PathBuf,
    pub members: Vec<PlannedMember>,
    pub team_name: Option<String>,
    pub topology: &'static str,
    pub links: Vec<PlannedLink>,
    pub on_behalf_of: Option<String>,
}

impl Plan {
    /// The dry-run / preview shape.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "folder": self.base,
            "team": self.team_name.as_ref().map(|n| serde_json::json!({"name": n, "topology": self.topology})),
            "members": self.members.iter().map(|m| serde_json::json!({
                "role": m.role,
                "handle": m.handle,
                "path": m.path,
                "lead": m.lead,
                "launch": m.recipe.launch.unwrap_or(false) || m.recipe.brief.is_some(),
                "appLogin": m.app_login.as_ref().map(|a| serde_json::json!({
                    "app": a.app, "username": a.username, "rooms": a.rooms, "role": a.role,
                })),
            })).collect::<Vec<_>>(),
            "links": self.links.iter().map(|l| serde_json::json!({
                "workspace": l.target.id,
                "handle": l.target.handle,
                "kind": l.kind,
            })).collect::<Vec<_>>(),
            "note": "handles are reserved for good once used: a hire that rolls back keeps them retired",
        })
    }
}

/// Context the door supplies.
#[derive(Debug, Clone)]
pub struct PlanContext {
    pub caller: HireCaller,
    /// The key's envelope (`Some` for API-key callers; required for them).
    pub limits: Option<HireLimits>,
    /// The key's own DB door.
    pub key_has_db: bool,
}

/// Seed for a member's handle (HT12).
fn handle_seed(m: &MemberRecipe, role: &str, team: Option<&str>) -> String {
    if let Some(h) = m.handle.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return h.to_string();
    }
    match team {
        Some(t) => format!("{t}-{role}"),
        None => role.to_string(),
    }
}

/// The first free handle from `seed` that also has a free folder under
/// `base` and isn't already taken by an earlier member of this plan.
/// Handles (and app login names) planned by hires that are still applying.
/// Two concurrent hires with the same seed must not predict the same
/// handle: the second one skips what the first reserved.
static IN_FLIGHT: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);

fn in_flight_has(name: &str) -> bool {
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .is_some_and(|s| s.contains(name))
}

/// Holds a plan's handles and login names until the hire finishes.
#[derive(Debug, Default)]
pub struct Reservation(Vec<String>);

impl Reservation {
    pub fn reserve(plan: &Plan) -> Self {
        let mut names: Vec<String> = plan.members.iter().map(|m| format!("h:{}", m.handle.to_ascii_lowercase())).collect();
        names.extend(plan.members.iter().filter_map(|m| m.app_login.as_ref()).map(|a| format!("u:{}", a.username)));
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        let set = g.get_or_insert_with(HashSet::new);
        for n in &names {
            set.insert(n.clone());
        }
        Self(names)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut g = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(set) = g.as_mut() {
            for n in &self.0 {
                set.remove(n);
            }
        }
    }
}

fn predict_handle(conn: &Connection, seed: &str, base: &Path, taken: &HashSet<String>) -> Result<String, HireError> {
    let slug = crate::workspace_session_handles::slugify_address_token(seed)
        .map_err(|e| HireError::usage(format!("handle '{seed}': {e}")))?;
    for n in 1u32..=10_000 {
        let cand = if n == 1 { slug.clone() } else { format!("{slug}-{n}") };
        if taken.contains(&cand.to_ascii_lowercase())
            || base.join(&cand).exists()
            || in_flight_has(&format!("h:{}", cand.to_ascii_lowercase()))
        {
            continue;
        }
        let got = crate::workspace::handle::allocate_unique_handle(conn, &cand, None)
            .map_err(|e| HireError::internal(format!("handle: {e}")))?;
        if got == cand {
            return Ok(cand);
        }
    }
    Err(HireError::new(409, "handle_exhausted", format!("no free handle left for '{slug}'")))
}

fn group_name_taken(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM project_groups WHERE name = ?1 COLLATE NOCASE",
        rusqlite::params![name.trim()],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(true)
}

/// May an API key touch workspace `ws_id` through a link? Only workspaces
/// it created, or ones the owner named in `linkTargets`.
pub fn key_may_link(conn: &Connection, key_id: &str, limits: &HireLimits, ws_id: &str) -> bool {
    limits.names_link_target(ws_id)
        || super::ledger::creator_key_of(conn, ws_id).as_deref() == Some(key_id)
}

/// Resolve and check one member's App login (S5). A key may give only
/// what its ceiling names (App, whole rooms or roles); unknown and
/// not-allowed answer the same 403 for a key (no App oracle).
fn plan_app_login(
    a: &super::recipe::AppLoginIn,
    handle: &str,
    ctx: &PlanContext,
    limits: Option<&HireLimits>,
    taken: &HashSet<String>,
) -> Result<PlannedAppLogin, HireError> {
    let is_key = matches!(ctx.caller, HireCaller::ApiKey { .. });
    let refuse_key = || {
        HireError::new(
            403,
            "exceeds_key",
            format!("appLogin on {:?}: this key may give logins only on the Apps, rooms and roles its owner named", a.app),
        )
    };
    let app = match super::apps::resolve_app(&a.app)? {
        Some(id) => id,
        None if is_key => return Err(refuse_key()),
        None => return Err(HireError::new(404, "not_found", format!("appLogin: no published App {:?}", a.app))),
    };
    let role = a.role.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let rooms = match (&role, a.rooms.as_ref()) {
        (Some(_), _) => Vec::new(),
        (None, Some(r)) => super::apps::resolve_rooms(r)?,
        (None, None) => vec![super::apps::SELF_ROOM.to_string()],
    };
    if is_key {
        let lookup = |t: &str| super::limits::RoleShape::lookup(&app, t);
        let ok = limits.is_some_and(|l| l.allows_app_login(&app, &rooms, role.as_deref(), &lookup));
        if !ok {
            return Err(refuse_key());
        }
    }
    let username = crate::skin::normalize_username(a.username.as_deref().unwrap_or(handle))
        .map_err(|e| HireError::usage(format!("appLogin.username: {e}")))?;
    if taken.contains(&username)
        || in_flight_has(&format!("u:{username}"))
        || crate::skin::principal_exists(&username).map_err(HireError::internal)?
    {
        return Err(HireError::new(409, "username_taken", format!("app login '{username}' already exists; pass appLogin.username")));
    }
    let email = match a.email.as_deref() {
        Some(e) => crate::skin::normalize_email(e).map_err(HireError::usage)?,
        None => None,
    };
    Ok(PlannedAppLogin { app, username, email, rooms, role, password: a.password.unwrap_or(false) })
}

/// Build the plan. Holds no lock of its own; the caller decides.
pub fn plan(req: &HireRequest, members: Vec<MemberRecipe>, ctx: &PlanContext) -> Result<Plan, HireError> {
    let limits = match (&ctx.caller, &ctx.limits) {
        (HireCaller::ApiKey { .. }, None) => {
            return Err(HireError::new(404, "not_found", "no such route"));
        }
        (_, l) => l.clone(),
    };
    let base = super::root::base_dir(limits.as_ref().map(|l| l.folder.as_str()))?;
    let team_name = req.team.as_ref().map(|t| t.name.trim().to_string());
    let topology: &'static str = match req.team.as_ref().and_then(|t| t.topology.as_deref()) {
        Some("mesh") => "mesh",
        Some("none") => "none",
        _ => "star",
    };

    // Key ceiling: team size, DB door, quotas.
    if let (HireCaller::ApiKey { id, .. }, Some(l)) = (&ctx.caller, limits.as_ref()) {
        if members.len() as u32 > l.max_members_per_team {
            return Err(HireError::new(
                403,
                "exceeds_key",
                format!("{} members is over this key's maxMembersPerTeam ({})", members.len(), l.max_members_per_team),
            ));
        }
        let ceiling = l.db_ceiling(ctx.key_has_db);
        for (i, m) in members.iter().enumerate() {
            super::converge::check_db_access(&ctx.caller, m.db_access.as_deref(), Some(ceiling))
                .map_err(|e| HireError { hint: format!("members[{i}].{}", e.hint), ..e })?;
            super::converge::check_chat_manage(&ctx.caller, m.chat_manage)?;
        }
        let db = crate::db::shared();
        let conn = db.lock();
        let day = super::now_secs() - 86_400;
        let today = super::ledger::members_created_since(&conn, id, day);
        if today.saturating_add(members.len() as u32) > l.max_members_per_day {
            return Err(HireError::new(
                429,
                "quota_exceeded",
                format!("this key made {today} agents in the last 24 hours; maxMembersPerDay is {}", l.max_members_per_day),
            ));
        }
        let live = super::ledger::live_members(&conn, id);
        if live.saturating_add(members.len() as u32) > l.max_live_members {
            return Err(HireError::new(
                429,
                "quota_exceeded",
                format!("this key has {live} live agents; maxLiveMembers is {}", l.max_live_members),
            ));
        }
        if req.link_to.as_ref().is_some_and(|v| !v.is_empty()) && !l.link {
            return Err(HireError::new(403, "exceeds_key", "linkTo needs a key with the link scope"));
        }
    } else {
        for m in &members {
            super::converge::check_db_access(&ctx.caller, m.db_access.as_deref(), None)?;
            super::converge::check_chat_manage(&ctx.caller, m.chat_manage)?;
        }
    }

    let db = crate::db::shared();
    let conn = db.lock();
    if let Some(n) = team_name.as_deref() {
        if group_name_taken(&conn, n) {
            return Err(HireError::new(409, "group_name_taken", format!("a project named '{n}' already exists; pick another team name")));
        }
    }
    let team_slug = team_name
        .as_deref()
        .and_then(|n| crate::workspace_session_handles::slugify_address_token(n).ok());
    let explicit_lead = members.iter().any(|m| m.lead.unwrap_or(false));
    let mut taken: HashSet<String> = HashSet::new();
    let mut taken_logins: HashSet<String> = HashSet::new();
    let mut planned = Vec::with_capacity(members.len());
    for (i, m) in members.into_iter().enumerate() {
        let role = m.role.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("agent").to_string();
        let lead = if explicit_lead { m.lead.unwrap_or(false) } else { i == 0 };
        let seed = handle_seed(&m, &role, team_slug.as_deref());
        let handle = predict_handle(&conn, &seed, &base, &taken)?;
        taken.insert(handle.to_ascii_lowercase());
        let path = base.join(&handle);
        let app_login = match m.app_login.as_ref() {
            None => None,
            Some(a) => Some(plan_app_login(a, &handle, ctx, limits.as_ref(), &taken_logins)?),
        };
        if let Some(a) = app_login.as_ref() {
            taken_logins.insert(a.username.clone());
        }
        planned.push(PlannedMember { recipe: m, role, handle, path, lead, app_login });
    }
    // Lead first: the first member of a group becomes its PoC.
    planned.sort_by_key(|m| !m.lead);

    let mut links_out = Vec::new();
    for l in req.link_to.as_deref().unwrap_or(&[]) {
        let kind = links::parse_kind(l.kind.as_deref())?;
        let target = links::resolve_workspace(&conn, &l.workspace);
        match (&ctx.caller, limits.as_ref(), target) {
            (HireCaller::ApiKey { id, .. }, Some(lim), Some(t)) if key_may_link(&conn, id, lim, &t.id) => {
                links_out.push(PlannedLink { target: t, kind, scope: l.scope.clone() });
            }
            (HireCaller::ApiKey { .. }, _, _) => {
                // Same answer for "unknown" and "not yours": no oracle.
                return Err(HireError::new(
                    403,
                    "exceeds_key",
                    format!("linkTo '{}': this key may link only to workspaces it created or the owner named (linkTargets)", l.workspace),
                ));
            }
            (_, _, Some(t)) => links_out.push(PlannedLink { target: t, kind, scope: l.scope.clone() }),
            (_, _, None) => {
                return Err(HireError::new(404, "not_found", format!("linkTo '{}': no such workspace (or the name is ambiguous)", l.workspace)));
            }
        }
    }
    let on_behalf_of = req.on_behalf_of.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    Ok(Plan { base, members: planned, team_name, topology, links: links_out, on_behalf_of })
}
