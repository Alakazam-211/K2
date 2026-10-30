//! `/cli/sew/admin/*` — the agent management plane for Sew chat (S30 K19)
//! — and `/cli/sew/on-behalf*` (a person's after-the-fact review of what
//! an assistant said for them, Rosson 2026-10-27).
//!
//! **Who may manage (the CM gate):** the owner token, an Owner/Admin
//! Connect login, or an agent (or K2 shell-tab) passport whose workspace
//! has "Manage Sew" (`chat_manage`) on. Turning that switch on is
//! `/cli/agent-access/set` (Owner/Admin), never an agent verb: the one
//! bootstrap step.
//!
//! **No self-loosening** (the 0.45.0 IT-agent rule): an agent never grants
//! its own workspace chat access, raises its own level, or moves its own
//! approval mode from `required` to `none`. It may lower its own level,
//! require approval for itself, or revoke its own access.
//!
//! - `agents*`: K2's per-Space lock on agent workspaces
//!   (`sew_chat_access`). A grant also makes sure the workspace has its Sew
//!   account (S27's `chat` grant, scope `coworker`), then resolves the
//!   Space AS that workspace's agent: Sew roles still decide WHERE, so a
//!   Space it can't see is Sew's masked 404.
//! - `people*`: wraps S27's person `chat` grants (`member` / `guest`) for
//!   App users (skin principals) and Connect logins. A login linked to the
//!   owner is the owner in chat and gets no grant. App-scoped Space
//!   membership comes from `k2 app grant` (K31), never from here.
//! - `log`: K2's administration audit (no message content).
//! - `on-behalf*`: the person themselves (today the owner: the owner token
//!   or an owner-linked login) lists the posts an assistant made on their
//!   behalf and reviews them after the fact. Those posts never waited for
//!   them (ticket 411eda53); the review is metadata a UI may show. Approving
//!   needs a vault confirm (`chat-behalf-approve`, `secretId` = the write's
//!   `act_…` id), so "approved by <person>" is person proof; declining
//!   needs none (it never widens anything).
//!
//! Every change writes a `sew_chat_audit` row.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::connect_users::Role;
use k2_core::sew_chat::{self as core, person, store, ApprovalMode, Level};

use super::routes::{backend, credential, internal, now, parse_body, sew_err, usage, ChatErr, R};
use super::sew::ActAs;
use crate::compute_routes::Who;

/// The routes this module serves.
pub const ROUTES: &[&str] = &[
    "/cli/sew/admin/agents",
    "/cli/sew/admin/agents/grant",
    "/cli/sew/admin/agents/revoke",
    "/cli/sew/admin/agents/set",
    "/cli/sew/admin/log",
    "/cli/sew/admin/people",
    "/cli/sew/admin/people/grant",
    "/cli/sew/admin/people/revoke",
    "/cli/sew/admin/people/update",
    "/cli/sew/on-behalf",
    "/cli/sew/on-behalf/review",
];

pub const POST_ROUTES: &[&str] = &[
    "/cli/sew/admin/agents/grant",
    "/cli/sew/admin/agents/revoke",
    "/cli/sew/admin/agents/set",
    "/cli/sew/admin/people/grant",
    "/cli/sew/admin/people/revoke",
    "/cli/sew/admin/people/update",
    "/cli/sew/on-behalf/review",
];

/// A caller that cleared the CM gate.
#[derive(Debug, Clone)]
pub(crate) enum Admin {
    /// The owner token (`owner_token`, not proof a person acted) or an
    /// Owner/Admin login (`login`).
    Person { label: String, kind: &'static str },
    /// A passport with Manage Sew on.
    Agent { project_id: String, label: String },
}

impl Admin {
    pub(crate) fn label(&self) -> &str {
        match self {
            Admin::Person { label, .. } | Admin::Agent { label, .. } => label,
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Admin::Person { kind, .. } => kind,
            Admin::Agent { .. } => "agent",
        }
    }

    fn own_project(&self) -> Option<&str> {
        match self {
            Admin::Agent { project_id, .. } => Some(project_id),
            Admin::Person { .. } => None,
        }
    }
}

/// Refresh a workspace's chat teaching (AGENTS.md line + `k2-chat` skill)
/// after one of its gates changed. A path that isn't a folder here (a test
/// row) is skipped.
pub(crate) fn teach_refresh(ws_path: &str) {
    if std::path::Path::new(ws_path).is_dir() {
        k2_core::sew_chat::teach::refresh_for_path(ws_path);
    }
}

fn chat_manage_off() -> ChatErr {
    ChatErr::new(
        "chat_manage_off",
        403,
        "managing chat needs the Manage Chat / Voice Hosting switch on your workspace — your owner or an admin turns it on \
         (k2 agent access set <workspace> chat_manage on)",
    )
}

/// The CM gate.
pub(crate) fn admin(who: &Who) -> R<Admin> {
    credential(who)?;
    match who {
        Who::Owner => Ok(Admin::Person { label: "owner".into(), kind: "owner_token" }),
        Who::Login { username, role } if *role >= Role::Admin => {
            Ok(Admin::Person { label: format!("login:{username}"), kind: "login" })
        }
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => {
            if !k2_core::workspace::settings::agents_can_manage_chat(project_id) {
                return Err(chat_manage_off());
            }
            Ok(Admin::Agent {
                project_id: project_id.clone(),
                label: format!("agent:{}", crate::compute_routes::ws_handle(project_id)),
            })
        }
        _ => Err(ChatErr::new("owner_only", 403, "only the owner, an admin, or an agent with Manage Chat / Voice Hosting manages chat")),
    }
}

/// Record one administration step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn audit(
    a: &Admin,
    project_id: Option<&str>,
    op: &str,
    space_id: Option<&str>,
    target: Option<&str>,
    detail: Option<String>,
    outcome: &str,
    reason: Option<&str>,
) -> R<()> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::insert_audit(
        &conn,
        &store::AuditRow {
            id: core::new_id("aud_"),
            project_id: project_id.map(str::to_string),
            actor: a.label().to_string(),
            actor_kind: a.kind().to_string(),
            op: op.to_string(),
            space_id: space_id.map(str::to_string),
            target: target.map(str::to_string),
            detail,
            outcome: outcome.to_string(),
            reason: reason.map(str::to_string),
            created_at: now(),
        },
    )
    .map_err(internal)
}

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<Value> {
    if path.starts_with("/cli/sew/on-behalf") {
        return match path {
            "/cli/sew/on-behalf" => on_behalf_list(who, params),
            "/cli/sew/on-behalf/review" => on_behalf_review(who, body),
            _ => Err(ChatErr::new("not_found", 404, "no such route")),
        };
    }
    let a = admin(who)?;
    match path {
        "/cli/sew/admin/agents" => agents_list(params),
        "/cli/sew/admin/agents/grant" => agents_grant(&a, body),
        "/cli/sew/admin/agents/set" => agents_set(&a, body),
        "/cli/sew/admin/agents/revoke" => agents_revoke(&a, body),
        "/cli/sew/admin/people" => people_list(),
        "/cli/sew/admin/people/grant" => people_grant(&a, body),
        "/cli/sew/admin/people/update" => people_update(&a, body),
        "/cli/sew/admin/people/revoke" => people_revoke(&a, body),
        "/cli/sew/admin/log" => log(params),
        _ => Err(ChatErr::new("not_found", 404, "no such route")),
    }
}

fn param<'a>(p: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    p.get(k).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn workspace(token: &str) -> R<crate::compute_routes::Ws> {
    crate::compute_routes::ws_by_token(token)
        .ok_or_else(|| ChatErr::new("not_found", 404, format!("no workspace '{}' on this box", token.trim())))
}

fn level_word(l: Level) -> &'static str {
    l.as_str()
}

fn access_json(r: &store::Access) -> Value {
    json!({
        "workspace": crate::compute_routes::ws_handle(&r.project_id),
        "workspaceId": r.project_id,
        "space": r.space_slug,
        "spaceId": r.space_id,
        "level": level_word(r.level),
        "approval": r.approval.as_str(),
        "updatedBy": r.updated_by,
        "updatedAt": r.updated_at,
    })
}

fn self_loosening(why: &str) -> ChatErr {
    ChatErr::new(
        "self_loosening",
        403,
        format!("an agent can't loosen its own chat access ({why}) — ask your owner or an admin, or another agent with Manage Chat / Voice Hosting"),
    )
}

// ── agents ───────────────────────────────────────────────────────────

fn agents_list(params: &HashMap<String, String>) -> R<Value> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_all_access(&conn).map_err(internal)?
    };
    let ws = match param(params, "workspace") {
        Some(t) => Some(workspace(t)?.id),
        None => None,
    };
    let space = param(params, "space");
    let out: Vec<Value> = rows
        .iter()
        .filter(|r| ws.as_deref().is_none_or(|w| r.project_id == w))
        .filter(|r| space.is_none_or(|s| r.space_slug.eq_ignore_ascii_case(s)))
        .map(access_json)
        .collect();
    Ok(json!({ "ok": true, "access": out }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GrantBody {
    workspace: String,
    space: String,
    level: String,
    #[serde(default)]
    approval: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SetBody {
    workspace: String,
    space: String,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    approval: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevokeBody {
    workspace: String,
    space: String,
}

/// Make sure the workspace has its Sew account (S27 `chat` grant). Returns
/// `created` or `exists`.
fn ensure_chat_account(ws_id: &str, by: &str) -> R<&'static str> {
    #[cfg(test)]
    if let Some(v) = test_seams::account() {
        return v.map_err(|e| ChatErr::new("chat_unavailable", 503, e));
    }
    let has = k2_core::skin::list_grants()
        .map_err(|e| ChatErr::new("chat_unavailable", 503, format!("can't read chat grants: {e}")))?
        .into_iter()
        .any(|g| g.kind == k2_core::tool_hook::CHAT_KIND && g.subject_kind == "workspace" && g.subject_id.eq_ignore_ascii_case(ws_id));
    if has {
        return Ok("exists");
    }
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: "workspace".into(),
        subject_id: ws_id.into(),
        kind: k2_core::tool_hook::CHAT_KIND.into(),
        target_id: k2_core::tool_hook::CHAT_TARGET_ID.into(),
        role: None,
        scope: Some("coworker".into()),
        created_by: Some(by.into()),
    })
    .map_err(|e| ChatErr::new("chat_unavailable", 503, format!("couldn't create the workspace's chat account: {e}")))?;
    Ok("created")
}

/// Resolve `<space-slug>` AS the workspace's agent (Sew roles decide where).
fn resolve_space_as(ws_id: &str, slug: &str, a: &Admin, fresh_account: bool) -> R<super::sew::Space> {
    let sew = backend()?;
    let act = ActAs {
        workspace_uuid: ws_id.to_ascii_lowercase(),
        k2_session: "k2-admin".into(),
        on_behalf_of: None,
        actor: Some(a.label().to_string()),
    };
    let spaces = match sew.spaces(&act) {
        Ok(s) => s,
        Err(e) if e.status == 403 && fresh_account => {
            return Err(ChatErr::new(
                "chat_account_pending",
                409,
                "created the workspace's chat account; Sew sets it up in a few seconds — run the same grant again",
            ))
        }
        Err(e) => return Err(sew_err(e)),
    };
    spaces.into_iter().find(|s| s.slug.eq_ignore_ascii_case(slug.trim())).ok_or_else(|| {
        ChatErr::new(
            "not_found",
            404,
            format!(
                "that workspace isn't in a Space '{}' — add it first (k2 chat space members add {} <workspace>), \
                 or check the slug",
                slug.trim(),
                slug.trim()
            ),
        )
    })
}

fn agents_grant(a: &Admin, body: &[u8]) -> R<Value> {
    let b: GrantBody = parse_body(body)?;
    let level = Level::parse(&b.level).map_err(usage)?;
    let approval = match b.approval.as_deref() {
        Some(x) => ApprovalMode::parse(x).map_err(usage)?,
        None => ApprovalMode::None,
    };
    let ws = workspace(&b.workspace)?;
    if a.own_project() == Some(ws.id.as_str()) {
        audit(a, Some(&ws.id), "agents.grant", None, Some(&b.space), None, "refused", Some("self_loosening"))?;
        return Err(self_loosening("granting itself access"));
    }
    backend()?;
    let account = ensure_chat_account(&ws.id, a.label())?;
    let space = resolve_space_as(&ws.id, &b.space, a, account == "created")?;
    let existing = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_access(&conn, &ws.id, &space.id).map_err(internal)?
    };
    if existing.is_some() {
        return Err(ChatErr::new(
            "exists",
            409,
            format!("{} already has access in {} — change it with access set", ws.handle, space.slug),
        ));
    }
    let row = store::Access {
        project_id: ws.id.clone(),
        space_id: space.id.clone(),
        space_slug: space.slug.clone(),
        level,
        approval,
        updated_by: a.label().to_string(),
        updated_at: now(),
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_access(&conn, &row).map_err(internal)?;
    }
    audit(a, Some(&ws.id), "agents.grant", Some(&space.id), Some(&space.slug), Some(format!("{}/{}", level.as_str(), approval.as_str())), "ok", None)?;
    teach_refresh(&ws.path);
    Ok(json!({
        "ok": true,
        "workspace": ws.handle,
        "space": space.slug,
        "level": level.as_str(),
        "approval": approval.as_str(),
        "chatAccount": account,
    }))
}

fn stored_access(ws_id: &str, slug: &str) -> R<store::Access> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::list_access(&conn, ws_id)
        .map_err(internal)?
        .into_iter()
        .find(|r| r.space_slug.eq_ignore_ascii_case(slug.trim()))
        .ok_or_else(|| ChatErr::new("not_found", 404, format!("no chat access in '{}' for that workspace", slug.trim())))
}

fn agents_set(a: &Admin, body: &[u8]) -> R<Value> {
    let b: SetBody = parse_body(body)?;
    if b.level.is_none() && b.approval.is_none() {
        return Err(usage("set --level and/or --approval"));
    }
    let level = b.level.as_deref().map(Level::parse).transpose().map_err(usage)?;
    let approval = b.approval.as_deref().map(ApprovalMode::parse).transpose().map_err(usage)?;
    let ws = workspace(&b.workspace)?;
    let old = stored_access(&ws.id, &b.space)?;
    let new_level = level.unwrap_or(old.level);
    let new_approval = approval.unwrap_or(old.approval);
    if a.own_project() == Some(ws.id.as_str()) {
        let why = if new_level > old.level {
            Some("raising its own level")
        } else if old.approval == ApprovalMode::Required && new_approval == ApprovalMode::None {
            Some("dropping its own approvals")
        } else {
            None
        };
        if let Some(why) = why {
            audit(a, Some(&ws.id), "agents.set", Some(&old.space_id), Some(&old.space_slug), None, "refused", Some("self_loosening"))?;
            return Err(self_loosening(why));
        }
    }
    let row = store::Access { level: new_level, approval: new_approval, updated_by: a.label().to_string(), updated_at: now(), ..old.clone() };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::set_access(&conn, &row).map_err(internal)?;
    }
    audit(
        a,
        Some(&ws.id),
        "agents.set",
        Some(&row.space_id),
        Some(&row.space_slug),
        Some(format!("{}/{} -> {}/{}", old.level.as_str(), old.approval.as_str(), new_level.as_str(), new_approval.as_str())),
        "ok",
        None,
    )?;
    teach_refresh(&ws.path);
    Ok(json!({
        "ok": true,
        "workspace": ws.handle,
        "space": row.space_slug,
        "level": new_level.as_str(),
        "approval": new_approval.as_str(),
    }))
}

fn agents_revoke(a: &Admin, body: &[u8]) -> R<Value> {
    let b: RevokeBody = parse_body(body)?;
    let ws = workspace(&b.workspace)?;
    let old = stored_access(&ws.id, &b.space)?;
    let cancelled = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::revoke_access(&conn, &ws.id, &old.space_id, a.label(), now()).map_err(internal)?
    };
    audit(a, Some(&ws.id), "agents.revoke", Some(&old.space_id), Some(&old.space_slug), Some(format!("cancelled {cancelled}")), "ok", None)?;
    teach_refresh(&ws.path);
    Ok(json!({ "ok": true, "workspace": ws.handle, "space": old.space_slug, "cancelled": cancelled }))
}

// ── people (wraps S27 person chat grants) ─────────────────────────────

/// A person subject: an App user (skin principal) or a Connect login.
struct PersonSubject {
    kind: &'static str,
    id: String,
    user: String,
}

fn person_subject(user: &str) -> R<PersonSubject> {
    let u = user.trim().trim_start_matches('@');
    if u.is_empty() {
        return Err(usage("name a person: a K2 username"));
    }
    let principals = k2_core::skin::list_principals().map_err(internal)?;
    if let Some(p) = principals.into_iter().find(|p| p.username.eq_ignore_ascii_case(u)) {
        return Ok(PersonSubject { kind: "principal", id: p.id, user: p.username });
    }
    if let Some(id) = k2_core::connect_users::user_uuid(u).map_err(internal)? {
        if person::login_is_owner(u) {
            return Err(ChatErr::new(
                "is_owner",
                409,
                "that login is the owner, who is always in chat as the owner — no grant needed",
            ));
        }
        return Ok(PersonSubject { kind: "connect", id, user: u.to_string() });
    }
    Err(ChatErr::new("not_found", 404, format!("no person '{u}' on this box (App users: k2 app users; logins: k2 users)")))
}

fn people_list() -> R<Value> {
    let grants = k2_core::skin::list_grants().map_err(internal)?;
    let principals: HashMap<String, String> =
        k2_core::skin::list_principals().map_err(internal)?.into_iter().map(|p| (p.id, p.username)).collect();
    let logins: HashMap<String, String> = k2_core::connect_users::chat_member_facts()
        .map_err(internal)?
        .into_iter()
        .map(|(id, username, _)| (id, username))
        .collect();
    let mut out = Vec::new();
    for g in grants.iter().filter(|g| g.kind == k2_core::tool_hook::CHAT_KIND) {
        let user = match g.subject_kind.as_str() {
            "principal" => principals.get(&g.subject_id).cloned(),
            "connect" => logins.get(&g.subject_id).cloned(),
            _ => continue,
        };
        out.push(json!({
            "user": user.unwrap_or_else(|| g.subject_id.clone()),
            "subjectKind": g.subject_kind,
            "scope": g.scope,
            "enabled": g.enabled,
        }));
    }
    Ok(json!({ "ok": true, "people": out }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PeopleBody {
    user: String,
    #[serde(default)]
    guest: Option<bool>,
}

fn scope_of(guest: Option<bool>) -> &'static str {
    if guest == Some(true) {
        "guest"
    } else {
        "member"
    }
}

fn grant_err(e: String) -> ChatErr {
    if e.contains("Linux only") || e.contains("not enabled") || e.contains("not on") {
        ChatErr::new("chat_unavailable", 503, e)
    } else if e.contains("already") || e.contains("UNIQUE") {
        ChatErr::new("exists", 409, e)
    } else {
        ChatErr::new("invalid_state", 422, e)
    }
}

fn people_grant(a: &Admin, body: &[u8]) -> R<Value> {
    let b: PeopleBody = parse_body(body)?;
    let s = person_subject(&b.user)?;
    let scope = scope_of(b.guest);
    k2_core::skin::create_grant(&k2_core::skin::SkinGrantWrite {
        subject_kind: s.kind.into(),
        subject_id: s.id.clone(),
        kind: k2_core::tool_hook::CHAT_KIND.into(),
        target_id: k2_core::tool_hook::CHAT_TARGET_ID.into(),
        role: None,
        scope: Some(scope.into()),
        created_by: Some(a.label().into()),
    })
    .map_err(grant_err)?;
    audit(a, None, "people.grant", None, Some(&s.user), Some(scope.into()), "ok", None)?;
    Ok(json!({ "ok": true, "user": s.user, "scope": scope }))
}

fn chat_grant_of(s: &PersonSubject) -> R<k2_core::skin::SkinGrant> {
    k2_core::skin::list_grants()
        .map_err(internal)?
        .into_iter()
        .find(|g| g.kind == k2_core::tool_hook::CHAT_KIND && g.subject_kind == s.kind && g.subject_id == s.id)
        .ok_or_else(|| ChatErr::new("not_found", 404, format!("{} has no chat access here", s.user)))
}

fn people_update(a: &Admin, body: &[u8]) -> R<Value> {
    let b: PeopleBody = parse_body(body)?;
    let guest = b.guest.ok_or_else(|| usage("say --guest on|off"))?;
    let s = person_subject(&b.user)?;
    let g = chat_grant_of(&s)?;
    let scope = scope_of(Some(guest));
    k2_core::skin::update_grant_scope(&g.id, scope, a.label()).map_err(grant_err)?;
    audit(a, None, "people.update", None, Some(&s.user), Some(scope.into()), "ok", None)?;
    Ok(json!({ "ok": true, "user": s.user, "scope": scope }))
}

fn people_revoke(a: &Admin, body: &[u8]) -> R<Value> {
    let b: PeopleBody = parse_body(body)?;
    if b.guest.is_some() {
        return Err(usage("revoke takes only a user"));
    }
    let s = person_subject(&b.user)?;
    let gone = k2_core::skin::delete_grant_by_key_with_cascade(
        s.kind,
        &s.id,
        k2_core::tool_hook::CHAT_KIND,
        k2_core::tool_hook::CHAT_TARGET_ID,
        a.label(),
    )
    .map_err(grant_err)?;
    if gone.is_none() {
        return Err(ChatErr::new("not_found", 404, format!("{} has no chat access here", s.user)));
    }
    audit(a, None, "people.revoke", None, Some(&s.user), None, "ok", None)?;
    Ok(json!({ "ok": true, "user": s.user, "revoked": true }))
}

fn log(params: &HashMap<String, String>) -> R<Value> {
    let limit: i64 = match param(params, "limit") {
        Some(l) => l.parse().ok().filter(|n| (1..=500).contains(n)).ok_or_else(|| usage("limit is 1–500"))?,
        None => 50,
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::audit_recent(&conn, limit).map_err(internal)?
    };
    Ok(json!({
        "ok": true,
        "entries": rows.iter().map(|r| json!({
            "at": r.created_at,
            "actor": r.actor,
            "actorKind": r.actor_kind,
            "op": r.op,
            "workspace": r.project_id.as_deref().map(crate::compute_routes::ws_handle),
            "space": r.space_id,
            "target": r.target,
            "detail": r.detail,
            "outcome": r.outcome,
            "reason": r.reason,
        })).collect::<Vec<_>>(),
    }))
}

// ── on-behalf reviews (the person themselves) ────────────────────────

/// The person a caller is, for their own reviews: the owner token or an
/// owner-linked login is the owner. Other logins and every passport are
/// refused (an App user's review needs an App surface; not built).
fn the_person(who: &Who) -> R<(String, String, String)> {
    credential(who)?;
    match who {
        Who::Owner => Ok((person::OWNER_EXTERNAL_ID.into(), "owner".into(), "owner".into())),
        Who::Login { username, .. } if person::login_is_owner(username) => {
            Ok((person::OWNER_EXTERNAL_ID.into(), format!("login:{username}"), format!("connect:{username}")))
        }
        Who::Agent { .. } | Who::Shell { .. } => Err(ChatErr::new(
            "person_only",
            403,
            "only the person an assistant spoke for reviews what it said for them",
        )),
        _ => Err(ChatErr::new(
            "person_only",
            403,
            "only the owner can do this here today (assistants are pinned to the owner or to App users)",
        )),
    }
}

fn on_behalf_list(who: &Who, params: &HashMap<String, String>) -> R<Value> {
    let (person, _, _) = the_person(who)?;
    let state = match params.get("state").map(String::as_str) {
        None | Some("") | Some("all") => None,
        Some(s @ ("pending" | "approved" | "declined")) => Some(s),
        Some(other) => return Err(usage(format!("state is pending, approved, declined or all (not {other:?})"))),
    };
    let limit = match params.get("limit") {
        Some(l) => l.parse::<i64>().map_err(|_| usage("limit is a number"))?,
        None => 50,
    };
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::list_behalf(&conn, &person, state, limit).map_err(internal)?
    };
    Ok(json!({
        "ok": true,
        "person": person,
        "items": rows.iter().map(|b| {
            let mut v = super::routes::behalf_json(b);
            v["workspace"] = json!(crate::compute_routes::ws_handle(&b.project_id));
            v["workspaceId"] = json!(b.project_id);
            v["action"] = json!(b.action);
            v["messageId"] = json!(b.message_id.as_deref().map(super::sew::wire_msg_id));
            v["outboxId"] = json!(b.outbox_id);
            v["createdAt"] = json!(b.created_at);
            v
        }).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReviewBody {
    /// The write's `act_…` id, or the `msg_…` it posted (its newest write).
    id: String,
    /// `approve` or `decline`.
    decision: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    confirm_token: Option<String>,
}

fn on_behalf_review(who: &Who, body: &[u8]) -> R<Value> {
    let (person, label, stamp) = the_person(who)?;
    let b: ReviewBody = parse_body(body)?;
    let state = match b.decision.as_str() {
        "approve" | "approved" => "approved",
        "decline" | "declined" => "declined",
        other => return Err(usage(format!("decision is approve or decline (not {other:?})"))),
    };
    let note = b.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.len() > 500) {
        return Err(usage("a note is at most 500 bytes"));
    }
    let id = b.id.trim();
    let lookup = super::sew::sew_msg_id(id).unwrap_or_else(|| id.to_string());
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::get_behalf(&conn, &lookup).map_err(internal)?
    };
    // Someone else's item reads exactly like a missing one.
    let Some(row) = row.filter(|r| r.person == person) else {
        return Err(ChatErr::new("not_found", 404, format!("no post made on your behalf is {id}")));
    };
    if row.message_id.is_none() {
        return Err(ChatErr::new(
            "not_posted",
            409,
            "that post has not reached chat yet (it waits for its Space's approval); review it once it posts",
        ));
    }
    let kind = if matches!(who, Who::Owner) { "owner_token" } else { "login" };
    // "Approved by <person>" is person proof: a fresh vault confirm bound to
    // this write. Declining never widens anything and needs none.
    let method = if state == "approved" {
        Some(super::routes_approvals::take_confirm(&stamp, b.confirm_token.as_deref(), core::CONFIRM_BEHALF_APPROVE, &row.action_id)?)
    } else {
        None
    };
    let t = now();
    let changed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        store::review_behalf(&conn, &row.action_id, state, &label, kind, method.as_deref(), note, t).map_err(internal)?
    };
    if !changed {
        return Err(ChatErr::new("not_found", 404, format!("no post made on your behalf is {id}")));
    }
    let a = Admin::Person { label: label.clone(), kind };
    let op = if state == "approved" { "on_behalf.approve" } else { "on_behalf.decline" };
    audit(&a, Some(&row.project_id), op, None, Some(&row.action_id), method.clone(), "ok", None)?;
    let after = store::Behalf {
        state: state.into(),
        decided_by: Some(label),
        decided_kind: Some(kind.into()),
        confirm_method: method,
        note: note.map(str::to_string),
        decided_at: Some(t),
        ..row
    };
    Ok(json!({ "ok": true, "onBehalf": super::routes::behalf_json(&after) }))
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::cell::RefCell;
    thread_local! { static ACCOUNT: RefCell<Option<Result<&'static str, String>>> = const { RefCell::new(None) }; }
    /// What `ensure_chat_account` answers (None = the real skin.db path).
    pub(crate) fn set_account(v: Option<Result<&'static str, String>>) {
        ACCOUNT.with(|c| *c.borrow_mut() = v);
    }
    pub(super) fn account() -> Option<Result<&'static str, String>> {
        ACCOUNT.with(|c| c.borrow().clone())
    }
}
