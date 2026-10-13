//! Apply a planned recipe hire, all-or-nothing, and the teardown of what a
//! hire made.
//!
//! - **Apply** runs the steps in order and keeps an undo list. On any
//!   failure it runs the undo list in reverse: links and relations,
//!   the project group, the registered workspaces, then the folders it
//!   created go to the Trash (never `rm`). The ledger row records `failed`
//!   and the reason. Handles used by rolled-back members stay retired
//!   (0159) — the plan named them.
//! - **Idempotent by key**: the same idempotency key and the same request
//!   return the stored result (`changed: false`); the same key with a
//!   different request is `409 key_conflict`. Keys are scoped per API key.
//! - **Launch and first brief** need the daemon's sessions: [`Instantiated`]
//!   lists them and the daemon runs them after the durable steps commit.
//!   A failed launch never undoes the hire (the agents exist; the owner
//!   or app can brief them again).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use super::ledger;
use super::links::{self, NewLink};
use super::plan::{Plan, PlanContext};
use super::recipe::HireRequest;
use super::steps::{self, PersonaSource};
use super::{HireCaller, HireError};

// ── Trash seam ────────────────────────────────────────────────────────

static TRASH_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Tests (and smoke runs) send rolled-back folders to `dir` instead of the
/// OS Trash. `K2_HIRE_TRASH_DIR` does the same for a whole daemon process.
pub fn set_trash_override(dir: Option<PathBuf>) {
    *TRASH_OVERRIDE.write().unwrap_or_else(|p| p.into_inner()) = dir;
}

fn trash_dir_override() -> Option<PathBuf> {
    if let Some(d) = TRASH_OVERRIDE.read().unwrap_or_else(|p| p.into_inner()).clone() {
        return Some(d);
    }
    std::env::var_os("K2_HIRE_TRASH_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Move a folder this hire created to the Trash (or the override dir).
pub fn trash_created_folder(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if let Some(dir) = trash_dir_override() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("trash dir {}: {e}", dir.display()))?;
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "folder".into());
        let dest = dir.join(format!("{name}-{}", uuid::Uuid::new_v4().simple()));
        return std::fs::rename(path, &dest).map_err(|e| format!("move {} to {}: {e}", path.display(), dest.display()));
    }
    crate::safe_delete::trash(path)
}

// ── Undo ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Undo {
    Folder(PathBuf),
    Workspace { path: String, id: String },
    Group(String),
    Link(String),
    InstanceEdges(String),
}

fn run_undo(steps: Vec<Undo>) -> Vec<String> {
    let mut errors = Vec::new();
    for step in steps.into_iter().rev() {
        let r: Result<(), String> = match &step {
            Undo::Link(id) => {
                let db = crate::db::shared();
                let conn = db.lock();
                links::remove(&conn, id).map(|_| ()).map_err(|e| e.hint)
            }
            Undo::InstanceEdges(inst) => {
                let db = crate::db::shared();
                let conn = db.lock();
                conn.execute(
                    "DELETE FROM workspace_relations WHERE origin_link_id = ?1",
                    rusqlite::params![format!("{}{inst}", links::INSTANCE_ORIGIN_PREFIX)],
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
            Undo::Group(id) => crate::project_groups::delete_group(id),
            Undo::Workspace { path, id } => {
                {
                    let db = crate::db::shared();
                    let conn = db.lock();
                    let _ = conn.execute(
                        "UPDATE project_groups SET poc_workspace_id = NULL WHERE poc_workspace_id = ?1",
                        rusqlite::params![id],
                    );
                }
                crate::workspace::lifecycle::remove_workspace_db_only(path).map(|_| ())
            }
            Undo::Folder(p) => trash_created_folder(p),
        };
        if let Err(e) = r {
            crate::log_debug!("[hire] undo {step:?} failed: {e}");
            errors.push(format!("{step:?}: {e}"));
        }
    }
    errors
}

// ── Apply ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HiredMember {
    pub role: String,
    pub workspace_id: String,
    pub handle: String,
    pub path: String,
    pub name: String,
    pub lead: bool,
}

/// A launch/brief the daemon runs after the hire commits.
#[derive(Debug, Clone)]
pub struct LaunchJob {
    pub path: String,
    pub handle: String,
    pub brief: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Instantiated {
    /// The response body (`ok`, `changed`, `instance`, `team`, `members`, `links`, …).
    pub body: serde_json::Value,
    pub launches: Vec<LaunchJob>,
    pub changed: bool,
}

fn persona_source(m: &super::recipe::MemberRecipe) -> PersonaSource {
    if let Some(t) = m.template.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        PersonaSource::Template(t.to_string())
    } else if let Some(p) = m.persona.as_deref().filter(|s| !s.trim().is_empty()) {
        PersonaSource::Body(p.to_string())
    } else {
        PersonaSource::Default
    }
}

/// Test seam: fail the apply right before member `n` (0-based) and before
/// the group/link steps (`n == members.len()` fails after all members).
#[cfg(any(test, feature = "test-util"))]
static FAIL_AT: std::sync::Mutex<Option<(String, usize)>> = std::sync::Mutex::new(None);

#[cfg(any(test, feature = "test-util"))]
pub fn set_fail_at(team_or_handle: Option<(&str, usize)>) {
    *FAIL_AT.lock().unwrap_or_else(|p| p.into_inner()) = team_or_handle.map(|(k, n)| (k.to_string(), n));
}

fn injected_failure(plan: &Plan, step: usize) -> Option<HireError> {
    #[cfg(any(test, feature = "test-util"))]
    {
        let key = plan
            .team_name
            .clone()
            .or_else(|| plan.members.first().map(|m| m.handle.clone()))
            .unwrap_or_default();
        if let Some((k, n)) = FAIL_AT.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            if k == key && n == step {
                return Some(HireError::internal(format!("injected failure at step {step}")));
            }
        }
    }
    let _ = (plan, step);
    None
}

fn apply_plan(plan: &Plan, instance_id: &str, caller: &HireCaller, undo: &mut Vec<Undo>) -> Result<(Vec<HiredMember>, Option<String>, Vec<links::Link>, Vec<LaunchJob>), HireError> {
    std::fs::create_dir_all(&plan.base)
        .map_err(|e| HireError::internal(format!("hire root {}: {e}", plan.base.display())))?;
    let mut hired = Vec::with_capacity(plan.members.len());
    let mut launches = Vec::new();
    for (i, m) in plan.members.iter().enumerate() {
        if let Some(e) = injected_failure(plan, i) {
            return Err(e);
        }
        let path_s = m.path.to_string_lossy().into_owned();
        std::fs::create_dir(&m.path).map_err(|e| {
            HireError::new(409, "folder_exists", format!("{}: {e} (another hire took it; retry)", m.path.display()))
        })?;
        undo.push(Undo::Folder(m.path.clone()));
        let seed_wiki = m.recipe.wiki.unwrap_or(true);
        let reg = crate::workspace::lifecycle::register_workspace_ex(&path_s, seed_wiki, true, false)
            .map_err(|e| HireError::internal(format!("register {}: {e}", m.handle)))?;
        let reg: serde_json::Value = serde_json::from_str(&reg).unwrap_or_default();
        let ws_id = reg["projectId"].as_str().unwrap_or_default().to_string();
        if ws_id.is_empty() {
            return Err(HireError::internal(format!("register {}: no project id", m.handle)));
        }
        undo.push(Undo::Workspace { path: path_s.clone(), id: ws_id.clone() });
        let actual_handle: String = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row("SELECT COALESCE(handle, '') FROM projects WHERE id = ?1", rusqlite::params![ws_id], |r| r.get(0))
                .unwrap_or_default()
        };
        if !actual_handle.eq_ignore_ascii_case(&m.handle) {
            return Err(HireError::new(
                409,
                "handle_changed",
                format!("planned handle '{}' was taken while hiring (got '{actual_handle}'); retry", m.handle),
            ));
        }
        {
            let db = crate::db::shared();
            let conn = db.lock();
            ledger::add_member(&conn, instance_id, &ws_id, &m.role, &m.handle, &path_s, m.lead)?;
        }
        let r = &m.recipe;
        steps::ensure_persona(&path_s, Some(&m.role), &persona_source(r))?;
        if let Some(n) = r.name.as_deref() {
            steps::set_display_name(&path_s, n)?;
        }
        if let Some(c) = r.color.as_deref() {
            steps::set_color(&path_s, c)?;
        }
        if let Some(p) = r.preset.as_deref() {
            steps::set_setting(&path_s, "default_agent", p)?;
        }
        if let Some(md) = r.model.as_deref() {
            steps::set_setting(&path_s, "default_model", md)?;
        }
        if let Some(d) = r.db_access.as_deref() {
            steps::set_setting(&path_s, "db_agent_access", d)?;
        }
        if let Some(n) = r.wiki_notes.as_ref() {
            steps::write_wiki_notes(&path_s, n)?;
        }
        if let Some(c) = r.context.as_ref() {
            steps::add_context(&path_s, c)?;
        }
        if let Some(l) = r.layers.as_ref() {
            steps::add_inline_layers(&path_s, l)?;
        }
        if let Some(s) = r.skills.as_ref() {
            steps::write_skills(&path_s, s)?;
        }
        if let Some(h) = r.heartbeats.as_ref() {
            steps::add_heartbeats(&path_s, h)?;
        }
        let name = {
            let d = crate::workspace::display::agent_display_name(&path_s);
            if d.is_empty() { m.handle.clone() } else { d }
        };
        if r.launch.unwrap_or(false) || r.brief.is_some() {
            launches.push(LaunchJob { path: path_s.clone(), handle: m.handle.clone(), brief: r.brief.clone() });
        }
        hired.push(HiredMember {
            role: m.role.clone(),
            workspace_id: ws_id,
            handle: m.handle.clone(),
            path: path_s,
            name,
            lead: m.lead,
        });
    }
    if let Some(e) = injected_failure(plan, plan.members.len()) {
        return Err(e);
    }

    let mut group_id = None;
    let lead_id = hired.iter().find(|h| h.lead).or(hired.first()).map(|h| h.workspace_id.clone()).unwrap_or_default();
    let others: Vec<String> = hired.iter().filter(|h| h.workspace_id != lead_id).map(|h| h.workspace_id.clone()).collect();
    if let Some(team) = plan.team_name.as_deref() {
        let g = crate::project_groups::create_group(team).map_err(|e| {
            if e.starts_with("name_taken") {
                HireError::new(409, "group_name_taken", e)
            } else {
                HireError::internal(format!("team group: {e}"))
            }
        })?;
        undo.push(Undo::Group(g.id.clone()));
        crate::project_groups::add_member(&g.id, &lead_id).map_err(|e| HireError::internal(format!("team lead: {e}")))?;
        for o in &others {
            crate::project_groups::add_member(&g.id, o).map_err(|e| HireError::internal(format!("team member: {e}")))?;
        }
        crate::project_groups::set_poc(&g.id, &lead_id).map_err(|e| HireError::internal(format!("team PoC: {e}")))?;
        {
            let db = crate::db::shared();
            let conn = db.lock();
            ledger::set_group(&conn, instance_id, &g.id);
            undo.push(Undo::InstanceEdges(instance_id.to_string()));
            links::write_team_edges(&conn, instance_id, &lead_id, &others, plan.topology)?;
        }
        group_id = Some(g.id);
    }

    let mut made_links = Vec::new();
    for pl in &plan.links {
        let db = crate::db::shared();
        let conn = db.lock();
        let (link, created) = links::create(
            &conn,
            caller,
            &NewLink {
                instance_id: Some(instance_id),
                team_group_id: group_id.as_deref(),
                team_lead: &lead_id,
                team_members: others.clone(),
                consumer: &pl.target.id,
                kind: pl.kind,
                topology: "lead",
                scope: pl.scope.as_ref(),
                on_behalf_of: plan.on_behalf_of.as_deref(),
            },
        )?;
        if created {
            undo.push(Undo::Link(link.id.clone()));
        }
        made_links.push(link);
    }
    Ok((hired, group_id, made_links, launches))
}

/// The hire door's whole job: idempotency, plan, ledger, apply, undo.
///
/// `raw` is the request JSON as sent (hashed for idempotency, minus `key`
/// and `dryRun`); `idem_header` is the `Idempotency-Key` header.
pub fn instantiate(
    raw: &serde_json::Value,
    idem_header: Option<&str>,
    ctx: &PlanContext,
) -> Result<Instantiated, HireError> {
    let req: HireRequest = serde_json::from_value(raw.clone()).map_err(|e| HireError::usage(format!("invalid hire request: {e}")))?;
    let members = super::recipe::validate_request(&req)?;
    let idem = idem_header
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| req.key.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
    if let Some(k) = idem.as_deref() {
        super::recipe::validate_idem_key(k)?;
    }
    let mut hashed = raw.clone();
    if let Some(o) = hashed.as_object_mut() {
        o.remove("key");
        o.remove("dryRun");
    }
    let req_hash = ledger::request_hash(&hashed);
    let caller = &ctx.caller;
    let dry_run = req.dry_run.unwrap_or(false);

    // Idempotent replay.
    if let Some(k) = idem.as_deref() {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(prev) = ledger::by_idem(&conn, caller.api_key_id(), k) {
            if prev.request_hash != req_hash {
                return Err(HireError::new(
                    409,
                    "key_conflict",
                    format!("idempotency key '{k}' was used for a different hire (instance {}); use a new key", prev.id),
                ));
            }
            if prev.status == "applying" {
                return Err(HireError::new(409, "hire_in_progress", "a hire with this idempotency key is still running; retry in a moment"));
            }
            let mut body: serde_json::Value = prev
                .result_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| serde_json::json!({ "ok": true, "instance": prev.id }));
            body["changed"] = serde_json::json!(false);
            body["status"] = serde_json::json!(prev.status);
            return Ok(Instantiated { body, launches: Vec::new(), changed: false });
        }
    }

    // Plan + claim the ledger row under one DB lock so two concurrent hires
    // by one key can't both pass the quota.
    let instance_id = format!("inst_{}", uuid::Uuid::new_v4().simple());
    let plan = {
        let db = crate::db::shared();
        let conn = db.lock();
        let plan = super::plan::plan(&req, members, ctx)?;
        if dry_run {
            return Ok(Instantiated {
                body: serde_json::json!({ "ok": true, "dryRun": true, "plan": plan.to_json() }),
                launches: Vec::new(),
                changed: false,
            });
        }
        ledger::insert_applying(
            &conn,
            &instance_id,
            caller,
            idem.as_deref(),
            &req_hash,
            plan.on_behalf_of.as_deref(),
            plan.team_name.as_deref(),
            plan.members.len(),
        )?;
        plan
    };

    let mut undo: Vec<Undo> = Vec::new();
    match apply_plan(&plan, &instance_id, caller, &mut undo) {
        Ok((hired, group_id, made_links, launches)) => {
            let body = serde_json::json!({
                "ok": true,
                "changed": true,
                "instance": instance_id,
                "status": "active",
                "team": plan.team_name.as_ref().map(|n| serde_json::json!({ "groupId": group_id, "name": n, "topology": plan.topology })),
                "members": hired,
                "links": made_links,
                "onBehalfOf": plan.on_behalf_of.as_ref().map(|v| serde_json::json!({ "value": v, "verified": false })),
            });
            let db = crate::db::shared();
            let conn = db.lock();
            ledger::finish(&conn, &instance_id, "active", None, Some(&body.to_string()));
            ledger::event(
                &conn,
                "agent-hire",
                caller,
                Some(&instance_id),
                None,
                plan.on_behalf_of.as_deref(),
                "ok",
                None,
                Some(&serde_json::json!({ "members": body["members"].as_array().map(|a| a.len()).unwrap_or(0), "team": plan.team_name })),
            );
            for l in &made_links {
                ledger::event(&conn, "agent-link", caller, Some(&instance_id), Some(&l.id), plan.on_behalf_of.as_deref(), "ok", None, None);
            }
            Ok(Instantiated { body, launches, changed: true })
        }
        Err(e) => {
            let undo_errors = run_undo(undo);
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "UPDATE agent_instance_members SET status = 'rolled_back' WHERE instance_id = ?1",
                rusqlite::params![instance_id],
            );
            let reason = if undo_errors.is_empty() {
                e.to_string()
            } else {
                format!("{e}; undo left: {}", undo_errors.join("; "))
            };
            ledger::finish(&conn, &instance_id, "failed", Some(&reason), None);
            ledger::event(&conn, "agent-hire", caller, Some(&instance_id), None, plan.on_behalf_of.as_deref(), "failed", Some(&reason), None);
            let mut out = e;
            out.hint = format!(
                "{} — nothing was kept (instance {instance_id} rolled back{}; its handles stay retired)",
                out.hint,
                if undo_errors.is_empty() { String::new() } else { format!(", but: {}", undo_errors.join("; ")) }
            );
            Err(out)
        }
    }
}

// ── Teardown ──────────────────────────────────────────────────────────

/// What the daemon's retire step says about one member.
#[derive(Debug, Clone)]
pub enum RetireOutcome {
    Retired,
    /// A guard refused (uncommitted work, secrets …); the text says why.
    Blocked(String),
    /// The workspace was already gone.
    Gone,
}

/// Tear down an instance: unlink everything, clear the group PoC and
/// delete the group, then retire each member through `retire_member` (the
/// daemon's archive-retire, never forced for keys). A refusal leaves the
/// instance `retire_blocked`.
pub fn teardown(
    instance_id: &str,
    caller: &HireCaller,
    retire_member: &dyn Fn(&str) -> RetireOutcome,
) -> Result<serde_json::Value, HireError> {
    let inst = {
        let db = crate::db::shared();
        let conn = db.lock();
        ledger::get(&conn, instance_id)
    }
    .ok_or_else(|| HireError::new(404, "not_found", format!("no hire instance {instance_id}")))?;
    if let HireCaller::ApiKey { id, .. } = caller {
        if inst.api_key_id.as_deref() != Some(id.as_str()) {
            return Err(HireError::new(404, "not_found", format!("no hire instance {instance_id}")));
        }
    }
    if !matches!(inst.status.as_str(), "active" | "retire_blocked") {
        return Err(HireError::new(409, "not_active", format!("instance {instance_id} is {}", inst.status)));
    }
    let mut unlinked = Vec::new();
    {
        let db = crate::db::shared();
        let conn = db.lock();
        for l in links::links_of_instance(&conn, instance_id) {
            links::remove(&conn, &l.id)?;
            ledger::event(&conn, "agent-unlink", caller, Some(instance_id), Some(&l.id), None, "ok", Some("teardown"), None);
            unlinked.push(l.id);
        }
        let _ = conn.execute(
            "DELETE FROM workspace_relations WHERE origin_link_id = ?1",
            rusqlite::params![format!("{}{instance_id}", links::INSTANCE_ORIGIN_PREFIX)],
        );
    }
    if let Some(g) = inst.group_id.as_deref() {
        // Gets past the PoC guard: the team goes, the members retire next.
        let _ = crate::project_groups::delete_group(g);
    }
    let mut members_out = Vec::new();
    let mut blocked = Vec::new();
    // Reverse order: helpers before the lead.
    for m in inst.members.iter().rev().filter(|m| m.status == "live") {
        let outcome = retire_member(&m.path);
        let db = crate::db::shared();
        let conn = db.lock();
        match &outcome {
            RetireOutcome::Retired | RetireOutcome::Gone => {
                ledger::set_member_status(&conn, instance_id, &m.workspace_id, "retired");
                members_out.push(serde_json::json!({ "handle": m.handle, "status": "retired" }));
            }
            RetireOutcome::Blocked(why) => {
                blocked.push(format!("{}: {why}", m.handle));
                members_out.push(serde_json::json!({ "handle": m.handle, "status": "blocked", "reason": why }));
            }
        }
    }
    let db = crate::db::shared();
    let conn = db.lock();
    let status = if blocked.is_empty() { "retired" } else { "retire_blocked" };
    let reason = (!blocked.is_empty()).then(|| blocked.join("; "));
    ledger::finish(&conn, instance_id, status, reason.as_deref(), None);
    ledger::event(&conn, "agent-retire", caller, Some(instance_id), None, None, if blocked.is_empty() { "ok" } else { "blocked" }, reason.as_deref(), None);
    Ok(serde_json::json!({
        "ok": blocked.is_empty(),
        "instance": instance_id,
        "status": status,
        "members": members_out,
        "unlinked": unlinked,
        "hint": (!blocked.is_empty()).then(|| "some members were not retired (a guard refused); the owner decides: k2 agent retire <handle> --force from a terminal outside K2"),
    }))
}
