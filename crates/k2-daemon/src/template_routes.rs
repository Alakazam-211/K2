//! The agent-template library routes (prd-agent-templates-v1 §6.2, §7;
//! contract `agent-templates-t5-t7-contract.md`):
//!
//! ```text
//! POST /cli/agent-templates/save    {workspace, id, name?, summary?, wiki?, dryRun?}
//! POST /cli/agent-templates/add     {dir, dryRun?}
//! POST /cli/agent-templates/remove  {id, version?}
//! POST /cli/agent-templates/upgrade {workspace, dryRun?}          (T5)
//! POST /cli/agent-templates/diff    {workspace} | {id, from, to}  (T5)
//! ```
//!
//! **Who.** People at the Admin floor (the owner token, an Owner or Admin
//! login; `route_policy` refuses a Member login first). An agent passport
//! (or a K2 shell's) may save **its own workspace only** (403
//! `own_workspace_only`), add only from a folder inside its workspace, and
//! remove only templates it authored (403 `not_author`). The author is
//! `user:<login>` for a person (the owner token: the linked login, else
//! `owner`) and `agent:<handle>` for an agent; never K2.
//!
//! A template carries no authority, so saving needs no approval step; every
//! write leaves one `auth-audit.jsonl` line.

use std::path::{Path, PathBuf};

use k2_core::connect_users::Role;
use k2_core::hire::template::save::{self, TemplateError};

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use k2_core::brand::CLI_NAME;

pub const CLI_SAVE: &str = "/cli/agent-templates/save";
pub const CLI_ADD: &str = "/cli/agent-templates/add";
pub const CLI_REMOVE: &str = "/cli/agent-templates/remove";
/// T5: POST because the workspace is a path.
pub const CLI_UPGRADE: &str = "/cli/agent-templates/upgrade";
pub const CLI_DIFF: &str = "/cli/agent-templates/diff";

/// Every route here (all POST-only).
pub const PATHS: &[&str] = &[CLI_SAVE, CLI_ADD, CLI_REMOVE, CLI_UPGRADE, CLI_DIFF];

/// The standing V1 actions K2 can read on a workspace (each has its own
/// switch): what Save as template turns into `suggest`.
const READABLE_STANDING: &[&str] =
    &["agents.manage", "projects.manage", "connections.create", "organization.manage", "compute.use", "gardens.manage"];

pub(crate) fn respond(e: &TemplateError) -> CliResponse {
    CliResponse { status: crate::hire_routes::status_line(e.status), content_type: "application/json", body: e.body().to_string() }
}

fn fail(status: u16, code: &'static str, hint: impl Into<String>) -> CliResponse {
    respond(&TemplateError::new(status, code, hint))
}

/// Who is asking, as an author.
#[derive(Debug, Clone)]
pub(crate) enum Caller {
    Person { author: String },
    Agent { workspace_id: String, author: String },
}

impl Caller {
    pub(crate) fn author(&self) -> &str {
        match self {
            Caller::Person { author } | Caller::Agent { author, .. } => author,
        }
    }
}

pub(crate) fn caller(who: &Who) -> Result<Caller, CliResponse> {
    match who {
        Who::Owner => Ok(Caller::Person {
            author: format!("user:{}", k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken)),
        }),
        Who::Login { username, role: Role::Owner | Role::Admin } => Ok(Caller::Person { author: format!("user:{username}") }),
        Who::Login { .. } => Err(fail(403, "role_required", "agent templates are saved and managed by an Owner or Admin")),
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => Ok(Caller::Agent {
            workspace_id: project_id.clone(),
            author: format!("agent:{}", crate::compute_routes::ws_handle(project_id)),
        }),
        _ => Err(CliResponse::forbidden()),
    }
}

/// A registered workspace by id, handle, alias, name or path.
pub(crate) fn resolve_workspace(token: &str) -> Option<k2_core::hire::links::Ws> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(w) = k2_core::hire::links::resolve_workspace(&conn, t) {
        return Some(w);
    }
    if t.contains('/') || t.starts_with('~') {
        let expanded = expand(t);
        let mut tries = vec![expanded.to_string_lossy().trim_end_matches('/').to_string()];
        if let Ok(c) = std::fs::canonicalize(&expanded) {
            tries.push(c.to_string_lossy().into_owned());
        }
        for p in tries {
            if let Some(w) = k2_core::hire::links::resolve_workspace(&conn, &p) {
                return Some(w);
            }
        }
    }
    None
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => k2_core::paths::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(p)),
        None => PathBuf::from(p),
    }
}

/// The workspace a request names, held to the caller: an agent may name
/// only its own (403 `own_workspace_only`).
pub(crate) fn workspace_for(c: &Caller, token: Option<&str>) -> Result<k2_core::hire::links::Ws, CliResponse> {
    let Some(token) = token.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(fail(400, "usage", "missing 'workspace' (a path or handle)"));
    };
    let Some(ws) = resolve_workspace(token) else {
        return Err(fail(404, "no_workspace", format!("no registered workspace '{token}' (see: {CLI_NAME} agent list)")));
    };
    if let Caller::Agent { workspace_id, .. } = c {
        if &ws.id != workspace_id {
            return Err(fail(403, "own_workspace_only", "an agent works on its own workspace only; a person can do this for another agent"));
        }
    }
    Ok(ws)
}

fn body_json(body: &[u8]) -> Result<serde_json::Value, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_slice(body).map_err(|e| fail(400, "usage", format!("invalid JSON body: {e}")))
}

fn s<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|x| !x.is_empty())
}

fn flag(v: &serde_json::Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn audit(event: &str, c: &Caller, outcome: &str) {
    crate::hire_routes::audit(event, c.author(), outcome);
}

/// `/cli/agent-templates/{save,add,remove}`. POST-only (the dispatcher's
/// 405 guard runs first; this repeats it for the cell socket).
pub(crate) fn handle(path: &str, who: &Who, is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let c = match caller(who) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let v = match body_json(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match path {
        CLI_SAVE => save_route(&c, &v),
        CLI_ADD => add_route(&c, &v),
        CLI_REMOVE => remove_route(&c, &v),
        CLI_UPGRADE => upgrade_route(&c, &v),
        CLI_DIFF => diff_route(&c, &v),
        _ => CliResponse::not_found(),
    }
}

/// The standing switches on for a workspace, as V1 action ids, and whether
/// "Can ask for permission cards" is on.
fn switches_on(workspace_id: &str) -> (Vec<String>, bool) {
    let mut on = Vec::new();
    for action in READABLE_STANDING {
        let standing = crate::hire_routes::template_action_kind(action) == k2_core::hire::template::ActionKind::Standing;
        let Some(switch) = k2_core::hire::starter_permissions::standing_switch(action) else { continue };
        if standing && crate::first_run_grants::switch_on(workspace_id, switch) {
            on.push(action.to_string());
        }
    }
    (on, crate::first_run_grants::switch_on(workspace_id, "permission_cards"))
}

fn save_route(c: &Caller, v: &serde_json::Value) -> CliResponse {
    let Some(id) = s(v, "id") else {
        return fail(400, "bad_id", "missing 'id' (the new template's id: a-z, 0-9, -)");
    };
    if let Err(e) = save::check_id(id) {
        return respond(&e);
    }
    let ws = match workspace_for(c, s(v, "workspace")) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let wiki: Vec<String> = match v.get("wiki") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(a)) => {
            let mut out = Vec::new();
            for x in a {
                match x.as_str() {
                    Some(n) => out.push(n.to_string()),
                    None => return fail(400, "usage", "'wiki' is a list of note ids"),
                }
            }
            out
        }
        Some(_) => return fail(400, "usage", "'wiki' is a list of note ids"),
    };
    let (suggest, cards) = switches_on(&ws.id);
    let inp = save::SaveInput {
        workspace: ws.path.clone(),
        id: id.to_string(),
        name: s(v, "name").map(str::to_string),
        summary: v.get("summary").and_then(|x| x.as_str()).map(str::to_string),
        wiki,
        dry_run: flag(v, "dryRun"),
        author: c.author().to_string(),
        suggest,
        cards,
    };
    match save::save(&inp) {
        Ok(out) => {
            if !out.dry_run {
                audit("agent-template-save", c, &format!("ok {} v{} from {}", out.id, out.version, ws.handle));
            }
            CliResponse::ok_json(serde_json::to_value(&out).unwrap_or_default().to_string())
        }
        Err(e) => {
            if e.code == "secret_found" {
                audit("agent-template-save", c, &format!("deny secret_found {id} from {}", ws.handle));
            }
            respond(&e)
        }
    }
}

fn add_route(c: &Caller, v: &serde_json::Value) -> CliResponse {
    let Some(raw) = s(v, "dir") else {
        return fail(400, "usage", "missing 'dir' (a template folder: template.toml, ROLE.md, …)");
    };
    let dir = expand(raw);
    if !dir.is_absolute() {
        return fail(400, "usage", "'dir' must be an absolute path");
    }
    let Ok(dir) = std::fs::canonicalize(&dir) else {
        return fail(404, "no_folder", format!("{raw} is not a folder"));
    };
    if let Caller::Agent { workspace_id, .. } = c {
        let own = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::Project::get(&conn, workspace_id).ok().map(|p| p.path)
        };
        let inside = own.and_then(|p| std::fs::canonicalize(p).ok()).is_some_and(|p| dir.starts_with(&p));
        if !inside {
            return fail(403, "own_workspace_only", "an agent adds a template only from a folder inside its own workspace");
        }
    }
    match save::add(Path::new(&dir), c.author(), flag(v, "dryRun")) {
        Ok(out) => {
            if !out.dry_run {
                audit("agent-template-add", c, &format!("ok {} v{}", out.id, out.version));
            }
            CliResponse::ok_json(serde_json::to_value(&out).unwrap_or_default().to_string())
        }
        Err(e) => respond(&e),
    }
}

fn remove_route(c: &Caller, v: &serde_json::Value) -> CliResponse {
    let Some(id) = s(v, "id") else {
        return fail(400, "bad_id", "missing 'id'");
    };
    let version = match v.get("version") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => match x.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0) {
            Some(n) => Some(n),
            None => return fail(400, "usage", "'version' is a whole number from 1"),
        },
    };
    let only = match c {
        Caller::Agent { author, .. } => Some(author.as_str()),
        Caller::Person { .. } => None,
    };
    match save::remove(id, version, only) {
        Ok(out) => {
            audit("agent-template-remove", c, &format!("ok {id} {:?}", out["removed"]));
            CliResponse::ok_json(out.to_string())
        }
        Err(e) => respond(&e),
    }
}

/// `upgrade {workspace, dryRun?}`: the workspace's template to its newest
/// version (a library template only here; built-ins also at start). An
/// agent upgrades its own workspace only (AT14).
fn upgrade_route(c: &Caller, v: &serde_json::Value) -> CliResponse {
    let ws = match workspace_for(c, s(v, "workspace")) {
        Ok(w) => w,
        Err(r) => return r,
    };
    let dry = flag(v, "dryRun");
    match k2_core::hire::template::upgrade(&ws.path, None, c.author(), dry, dry) {
        Ok(r) => {
            if !dry && (r.changed || r.from != r.to) {
                audit("agent-template-upgrade", c, &format!("ok {} {} {}→{}", ws.handle, r.id, r.from, r.to));
                crate::v1_hire::emit_hire_watches();
            }
            CliResponse::ok_json(serde_json::to_value(&r).unwrap_or_default().to_string())
        }
        Err(e) => respond(&e),
    }
}

/// `diff {workspace}`: what `upgrade` would do, with line diffs (writes
/// nothing). `diff {id, from, to}`: two versions of a template.
fn diff_route(c: &Caller, v: &serde_json::Value) -> CliResponse {
    if v.get("workspace").is_some() {
        let ws = match workspace_for(c, s(v, "workspace")) {
            Ok(w) => w,
            Err(r) => return r,
        };
        return match k2_core::hire::template::upgrade(&ws.path, None, c.author(), true, true) {
            Ok(r) => CliResponse::ok_json(serde_json::to_value(&r).unwrap_or_default().to_string()),
            Err(e) => respond(&e),
        };
    }
    let num = |k: &str| v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|t| t.trim_start_matches('v').parse().ok()))).and_then(|n| u32::try_from(n).ok());
    let (Some(id), Some(a), Some(b)) = (s(v, "id"), num("from"), num("to")) else {
        return fail(400, "usage", "pass {workspace} (what an upgrade would do) or {id, from, to} (two versions)");
    };
    match k2_core::hire::template::upgrade::diff_versions(id, a, b) {
        Ok(out) => CliResponse::ok_json(out.to_string()),
        Err(e) => respond(&e),
    }
}

#[cfg(test)]
#[path = "template_routes_tests.rs"]
mod tests;
