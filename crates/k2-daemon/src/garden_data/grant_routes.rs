//! Garden data routes: grant, revoke, admin, admin/revoke, spaces, show,
//! space/delete, space/move (contract §4.5, §5.2; PRD §5.5). Owner: B1.
//!
//! - The person (owner token) grants anything, names grant admins
//!   (delegations) and manages spaces.
//! - A grant admin (an agent holding a delegation in the space) may grant
//!   and revoke inside its delegation only: prefix inside its own, level at
//!   most its own, never itself, never another admin, never a row it didn't
//!   make, 30 changes a minute.
//! - A project grant is convenience-level (Rosson 2026-10-14): any agent
//!   can join a K2 project, so the answer carries
//!   `warning: "project_grant_convenience"`.
//!
//! Every grant change is one audit line.

use std::collections::HashMap;

use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{json, Value};

use k2_core::garden_data::grants::{self, GrantRow, NewGrant, SubjectKind};
use k2_core::garden_data::snapshots::PreOp;
use k2_core::garden_data::{keys, DataError, Level, Space, StoreBackend, PROJECT_GRANT_WARNING};

use super::agent_routes::{data_space, param, parse};
use super::audit;
use super::budget::{self, BudgetKey, Spend};
use super::{notify_changed, refused, store};
use crate::zen_data_routes::DataWho;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantBody {
    widget: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    project: Option<String>,
    level: String,
    #[serde(default)]
    prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeBody {
    widget: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminBody {
    widget: String,
    agent: String,
    level: String,
    #[serde(default)]
    prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminRevokeBody {
    widget: String,
    agent: String,
    #[serde(default)]
    prefix: String,
    #[serde(default, rename = "dryRun")]
    dry_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpaceDeleteBody {
    widget: String,
    #[serde(default)]
    confirm: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpaceMoveBody {
    from: String,
    to: String,
}

pub fn handle(
    path: &str,
    who: &DataWho,
    body: &[u8],
    params: &HashMap<String, String>,
) -> Result<Value, DataError> {
    match path {
        "/cli/zen/data/grant" => grant(who, body),
        "/cli/zen/data/revoke" => revoke(who, body),
        "/cli/zen/data/admin" => admin(who, body),
        "/cli/zen/data/admin/revoke" => admin_revoke(who, body),
        "/cli/zen/data/spaces" => spaces(),
        "/cli/zen/data/show" => show(params),
        "/cli/zen/data/space/delete" => space_delete(who, body),
        "/cli/zen/data/space/move" => space_move(who, body),
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

// ── resolution ────────────────────────────────────────────────────────

fn level(s: &str) -> Result<Level, DataError> {
    Level::parse(s.trim()).ok_or_else(|| DataError::BadRequest("level is read or write".into()))
}

/// An agent on this computer → its `projects.id`.
pub(crate) fn resolve_agent(token: &str) -> Result<String, DataError> {
    let t = token.trim();
    if t.contains("::") {
        return Err(DataError::NotFound("agent_not_found"));
    }
    crate::compute_routes::ws_by_token(t).map(|w| w.id).ok_or(DataError::NotFound("agent_not_found"))
}

/// A K2 project by id or exact (case-insensitive) name → `(id, name)`.
pub(crate) fn resolve_project(token: &str) -> Result<(String, String), DataError> {
    let t = token.trim();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, name FROM project_groups WHERE id = ?1 OR name = ?1 COLLATE NOCASE ORDER BY id = ?1 DESC LIMIT 1",
        [t],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()?
    .ok_or(DataError::NotFound("project_not_found"))
}

fn subject(agent: Option<&str>, project: Option<&str>) -> Result<(SubjectKind, String), DataError> {
    let agent = agent.map(str::trim).filter(|s| !s.is_empty());
    let project = project.map(str::trim).filter(|s| !s.is_empty());
    match (agent, project) {
        (Some(a), None) => Ok((SubjectKind::Workspace, resolve_agent(a)?)),
        (None, Some(p)) => Ok((SubjectKind::Project, resolve_project(p)?.0)),
        _ => Err(DataError::BadRequest("name exactly one of agent or project".into())),
    }
}

fn subject_name(row: &GrantRow) -> String {
    match row.subject_kind {
        SubjectKind::Workspace => crate::compute_routes::ws_by_id(&row.subject_id)
            .map(|w| w.handle)
            .unwrap_or_else(|| "gone".into()),
        SubjectKind::Project => {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT name FROM project_groups WHERE id = ?1", [&row.subject_id], |r| r.get(0))
                .optional()
                .ok()
                .flatten()
                .unwrap_or_else(|| "gone".into())
        }
    }
}

fn now() -> i64 {
    super::now()
}

fn grant_line(event: &str, who: &DataWho, row: &GrantRow, extra: &str) {
    let via = row.via_grant.map(|v| format!(" via={v}")).unwrap_or_default();
    audit::line(
        event,
        &who.audit_label(),
        &format!(
            "space={} prefix={:?} subject={}:{} level={} admin={} granted_by={}{via}{extra}",
            row.space,
            row.prefix,
            row.subject_kind.as_str(),
            row.subject_id,
            row.level.as_str(),
            row.admin,
            row.granted_by
        ),
    );
}

fn refusal(who: &DataWho, space: &Space, e: DataError) -> DataError {
    audit::line("zen.data.refused", &who.audit_label(), &format!("space={space} code={}", e.code()));
    e
}

// ── grant / revoke ────────────────────────────────────────────────────

fn grant(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: GrantBody = parse(body)?;
    let space = data_space(&b.widget)?;
    let lvl = level(&b.level)?;
    keys::validate_prefix(&b.prefix)?;
    let (kind, sid) = subject(b.agent.as_deref(), b.project.as_deref())?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let existing = grants::find(&conn, &space, &b.prefix, kind, &sid)?;
    let new = match who.project_id() {
        None => NewGrant {
            space: space.clone(),
            prefix: b.prefix.clone(),
            subject_kind: kind,
            subject_id: sid,
            level: lvl,
            // The person's grant on a delegation row keeps it a delegation.
            admin: existing.as_ref().is_some_and(|e| e.admin),
            via_grant: None,
            granted_by: "owner".into(),
        },
        Some(caller) => {
            let req = NewGrant {
                space: space.clone(),
                prefix: b.prefix.clone(),
                subject_kind: kind,
                subject_id: sid,
                level: lvl,
                admin: false,
                via_grant: None,
                granted_by: format!("agent:{caller}"),
            };
            let delegations = grants::delegations_of(&conn, &space, caller)?;
            let mut chosen: Option<i64> = None;
            let mut why = DataError::NotGranted { key: None };
            for d in &delegations {
                match grants::check_delegate_grant(d, &req, caller) {
                    Ok(()) => {
                        chosen = Some(d.id);
                        break;
                    }
                    Err(e) => why = e,
                }
            }
            let Some(deleg) = chosen else { return Err(refusal(who, &space, why)) };
            if let Some(e) = &existing {
                let mine = e.via_grant.is_some_and(|v| delegations.iter().any(|d| d.id == v));
                if !mine {
                    return Err(refusal(
                        who,
                        &space,
                        refused("grant_exists", 409, "that grant was made by the person (or another admin); you can't change it"),
                    ));
                }
            }
            budget::spend(&BudgetKey::Delegate { project_id: caller.to_string() }, Spend::GrantChange)?;
            NewGrant { via_grant: Some(deleg), ..req }
        }
    };
    let (row, created) = grants::upsert(&conn, &new, now())?;
    grant_line("zen.data.grant", who, &row, "");
    let mut out = json!({ "grant": row, "created": created });
    if kind == SubjectKind::Project {
        out["warning"] = json!(PROJECT_GRANT_WARNING);
    }
    Ok(out)
}

fn revoke(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: RevokeBody = parse(body)?;
    let space = data_space(&b.widget)?;
    keys::validate_prefix(&b.prefix)?;
    let (kind, sid) = subject(b.agent.as_deref(), b.project.as_deref())?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = grants::find(&conn, &space, &b.prefix, kind, &sid)?.ok_or(DataError::NotFound("grant_not_found"))?;
    if row.admin {
        return Err(refused(
            "use_admin_revoke",
            409,
            "that's a grant admin; the person ends it with `k2 zen data unadmin` (its grants go with it)",
        ));
    }
    if let Some(caller) = who.project_id() {
        let mine = grants::delegations_of(&conn, &space, caller)?;
        if !row.via_grant.is_some_and(|v| mine.iter().any(|d| d.id == v)) {
            return Err(refusal(who, &space, refused("not_own", 403, "you can revoke only the grants you made")));
        }
        budget::spend(&BudgetKey::Delegate { project_id: caller.to_string() }, Spend::GrantChange)?;
    }
    grants::remove(&conn, row.id)?;
    grant_line("zen.data.revoke", who, &row, "");
    Ok(json!({ "revoked": 1 }))
}

// ── delegations (person only; the gate already refused passports) ─────

fn admin(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: AdminBody = parse(body)?;
    let space = data_space(&b.widget)?;
    let lvl = level(&b.level)?;
    keys::validate_prefix(&b.prefix)?;
    let ws = resolve_agent(&b.agent)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let new = NewGrant {
        space,
        prefix: b.prefix,
        subject_kind: SubjectKind::Workspace,
        subject_id: ws,
        level: lvl,
        admin: true,
        via_grant: None,
        granted_by: "owner".into(),
    };
    let (row, created) = grants::upsert(&conn, &new, now())?;
    grant_line("zen.data.admin", who, &row, "");
    Ok(json!({ "grant": row, "created": created }))
}

fn admin_revoke(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: AdminRevokeBody = parse(body)?;
    let space = data_space(&b.widget)?;
    keys::validate_prefix(&b.prefix)?;
    let ws = resolve_agent(&b.agent)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = grants::find(&conn, &space, &b.prefix, SubjectKind::Workspace, &ws)?
        .filter(|r| r.admin)
        .ok_or(DataError::NotFound("grant_not_found"))?;
    if b.dry_run {
        return Ok(json!({ "revoked": 0, "cascaded": grants::made_under(&conn, row.id)? }));
    }
    let cascaded = grants::remove(&conn, row.id)?;
    grant_line("zen.data.admin_revoke", who, &row, &format!(" cascaded={cascaded}"));
    Ok(json!({ "revoked": 1, "cascaded": cascaded }))
}

// ── spaces (person only) ──────────────────────────────────────────────

/// The live hash's maker for Settings, or null when the folder is gone.
/// Until B2's authorship lands (`authors::maker_of` answers `not_built`),
/// a present widget shows null too.
fn made_by_view(widget: &str) -> Result<Value, DataError> {
    if widget.starts_with("k2:") {
        return Ok(json!({ "kind": "k2" }));
    }
    let Some(hash) = live_hash(widget) else { return Ok(Value::Null) };
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::garden_data::authors::maker_of(&conn, widget, &hash) {
        Ok(k2_core::garden_data::MadeBy::Agent { project_id }) => Ok(json!({
            "kind": "agent",
            "handle": crate::compute_routes::ws_by_id(&project_id).map(|w| w.handle).unwrap_or_else(|| "gone".into()),
        })),
        Ok(m) => Ok(serde_json::to_value(m).unwrap_or(Value::Null)),
        Err(DataError::NotBuilt) => Ok(Value::Null),
        Err(e) => Err(e),
    }
}

/// The widget's live bundle hash, when Zen is set up and the widget has a
/// good version.
fn live_hash(widget: &str) -> Option<String> {
    let files = k2_core::zen::ZenFiles::local();
    if !files.is_set_up() {
        return None;
    }
    files.widget_status(widget).bundle.map(|b| b.hash)
}

fn spaces() -> Result<Value, DataError> {
    let all = store().spaces()?;
    let summary = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        grants::summary(&conn)?
    };
    let mut rows = Vec::new();
    let mut saves = Vec::new();
    for s in all {
        if s.space.starts_with("save:") {
            saves.push(json!({ "widget": s.widget, "bytes": s.bytes, "keys": s.keys }));
            continue;
        }
        let (count, admins) = summary
            .iter()
            .find(|(sp, _, _)| *sp == s.space)
            .map(|(_, n, a)| (*n, a.clone()))
            .unwrap_or((0, Vec::new()));
        let admins: Vec<String> = admins
            .iter()
            .map(|id| crate::compute_routes::ws_by_id(id).map(|w| w.handle).unwrap_or_else(|| "gone".into()))
            .collect();
        rows.push(json!({
            "widget": s.widget,
            "space": s.space,
            "bytes": s.bytes,
            "keys": s.keys,
            "rev": s.rev,
            "updatedAt": s.updated_at,
            "orphanedAt": s.orphaned_at,
            "grants": count,
            "admins": admins,
            "madeBy": if s.orphaned_at.is_some() { Value::Null } else { made_by_view(&s.widget)? },
        }));
    }
    Ok(json!({ "spaces": rows, "saves": saves }))
}

fn show(params: &HashMap<String, String>) -> Result<Value, DataError> {
    let space = data_space(param(params, "widget").unwrap_or(""))?;
    let info = store().info(&space)?.ok_or(DataError::NotFound("space_not_found"))?;
    let (rows, delegations) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let rows = grants::list_space(&conn, &space)?;
        let mut ds = Vec::new();
        for r in rows.iter().filter(|r| r.admin) {
            ds.push(json!({ "grant": r, "madeUnder": grants::made_under(&conn, r.id)? }));
        }
        (rows, ds)
    };
    let grants: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
            v["subjectName"] = json!(subject_name(r));
            v
        })
        .collect();
    Ok(json!({
        "space": info,
        "grants": grants,
        "delegations": delegations,
        "madeBy": if info.orphaned_at.is_some() { Value::Null } else { made_by_view(space.widget())? },
        "liveHash": live_hash(space.widget()),
    }))
}

/// A pre-op snapshot (B3's `export_routes::pre_op_snapshot`): its file
/// name, or null when no store file exists yet. A failure stops the
/// operation.
fn pre_op(op: PreOp) -> Result<Value, DataError> {
    Ok(json!(super::export_routes::pre_op_snapshot(op)?))
}

fn space_delete(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: SpaceDeleteBody = parse(body)?;
    let space = data_space(&b.widget)?;
    if b.confirm.trim() != space.widget() {
        return Err(refused("confirm_required", 400, format!("confirm with the widget's name: \"{}\"", space.widget())));
    }
    let save = Space::save(space.widget())?;
    if store().info(&space)?.is_none() && store().info(&save)?.is_none() {
        return Err(DataError::NotFound("space_not_found"));
    }
    let snapshot = pre_op(PreOp::Delete)?;
    store().drop_space(&space)?;
    store().drop_space(&save)?;
    let removed = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        grants::delete_for_space(&conn, &space)?
    };
    let rev = store().info(&space)?.map_or(0, |i| i.rev);
    notify_changed(space.as_str(), rev.max(1));
    audit::line("zen.data.space_delete", &who.audit_label(), &format!("space={space} grants={removed}"));
    Ok(json!({ "deleted": space.as_str(), "snapshot": snapshot, "grantsRemoved": removed }))
}

fn space_move(who: &DataWho, body: &[u8]) -> Result<Value, DataError> {
    let b: SpaceMoveBody = parse(body)?;
    let from = data_space(&b.from)?;
    let to = data_space(&b.to)?;
    if from == to {
        return Err(DataError::BadRequest("from and to are the same widget".into()));
    }
    if store().info(&from)?.is_none() {
        return Err(DataError::NotFound("space_not_found"));
    }
    let snapshot = pre_op(PreOp::Move)?;
    store().move_space(&from, &to)?;
    let (from_save, to_save) = (Space::save(from.widget())?, Space::save(to.widget())?);
    if store().info(&from_save)?.is_some() {
        store().move_space(&from_save, &to_save)?;
    }
    let moved = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        grants::move_space(&conn, &from, &to)?
    };
    if let Some(i) = store().info(&to)? {
        notify_changed(to.as_str(), i.rev);
    }
    audit::line("zen.data.space_move", &who.audit_label(), &format!("from={from} to={to} grants={moved}"));
    Ok(json!({ "moved": true, "grantsMoved": moved, "snapshot": snapshot }))
}

#[cfg(test)]
#[path = "grant_routes_tests.rs"]
pub(crate) mod tests;
