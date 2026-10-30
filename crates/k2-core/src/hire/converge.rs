//! Create-or-converge ONE workspace at a path (`POST /v1/w`,
//! `k2 agent hire <dir>` via `POST /cli/agents/hire`). Re-running the same
//! request changes nothing and reports `changed: false`. No undo: converge
//! heals missing pieces, it never takes a workspace down.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::recipe::{InlineLayer, WikiNote};
use super::steps::{self, PersonaSource};
use super::{HireCaller, HireError};

/// The converge request (the `/v1/w` body; the CLI sends the same shape).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvergeRequest {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    /// ROLE.md body.
    #[serde(default)]
    pub persona: Option<String>,
    /// A whole ROLE.md (frontmatter included) — `k2 agent hire --persona`.
    #[serde(default)]
    pub persona_raw: Option<String>,
    /// Role for the generated frontmatter (CLI `--template` / scaffold).
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub wiki: Option<Vec<WikiNote>>,
    #[serde(default)]
    pub context: Option<Vec<String>>,
    #[serde(default)]
    pub layers: Option<Vec<InlineLayer>>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub no_wiki: Option<bool>,
    #[serde(default)]
    pub no_agents_md: Option<bool>,
    #[serde(default)]
    pub fanout: Option<bool>,
    /// Persist create-only `db_agent_access` (`off`/`read`/`write`). Does
    /// NOT mint a database.
    #[serde(default)]
    pub db_access: Option<String>,
}

/// What the caller may do beyond the shape of the request.
#[derive(Debug, Clone, Default)]
pub struct ConvergeAuthority {
    /// `Some(dir)`: a NEW workspace folder must sit inside `dir` (API keys,
    /// T2/T19). `None`: any absolute path (the owner on their own box).
    pub new_workspace_root: Option<PathBuf>,
    /// May this caller create a new workspace at all?
    pub may_create: bool,
    /// Highest `db_agent_access` this caller may set (`None` = no cap).
    pub max_db: Option<&'static str>,
    /// Refusal text when `may_create` is false.
    pub create_refusal: Option<String>,
}

impl ConvergeAuthority {
    pub fn unrestricted() -> Self {
        Self { new_workspace_root: None, may_create: true, max_db: None, create_refusal: None }
    }
}

/// One reported step (`{step, status, reason?}`); statuses are `done`,
/// `skipped`, `pending` (dry run).
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct StepReport {
    pub step: &'static str,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn done(step: &'static str) -> StepReport {
    StepReport { step, status: "done", reason: None }
}
fn skipped(step: &'static str, why: &str) -> StepReport {
    StepReport { step, status: "skipped", reason: Some(why.to_string()) }
}

#[derive(Debug, Clone)]
pub struct ConvergeOutcome {
    pub path: String,
    pub id: String,
    pub handle: String,
    pub name: String,
    pub changed: bool,
    /// The workspace was registered by this call.
    pub created: bool,
    pub steps: Vec<StepReport>,
    pub wiki_notes: Vec<serde_json::Value>,
}

/// Expand `~`, require absolute, canonicalize an existing directory.
pub fn expand_hire_path(raw: &str) -> Result<String, HireError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(HireError::usage("missing path"));
    }
    let expanded = if t == "~" {
        crate::paths::home_dir().ok_or_else(|| HireError::usage("cannot resolve ~"))?
    } else if let Some(rest) = t.strip_prefix("~/") {
        crate::paths::home_dir().ok_or_else(|| HireError::usage("cannot resolve ~"))?.join(rest)
    } else {
        PathBuf::from(t)
    };
    if !expanded.is_absolute() {
        return Err(HireError::usage("path must be absolute or ~"));
    }
    if expanded.exists() {
        if !expanded.is_dir() {
            return Err(HireError::usage(format!("{} exists and is not a directory", expanded.display())));
        }
        return dunce::canonicalize(&expanded)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(|e| HireError::usage(format!("canonicalize: {e}")));
    }
    Ok(expanded.to_string_lossy().into_owned())
}

/// A registered workspace row (id, name, handle).
#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
}

fn lookup_exact(path: &str) -> Option<ProjectRow> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, name, handle FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| Ok(ProjectRow { id: r.get(0)?, name: r.get(1)?, handle: r.get(2)? }),
    )
    .ok()
}

fn path_lookup_candidates(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |p: String| {
        if !out.iter().any(|e| e == &p) {
            out.push(p);
        }
    };
    push(path.to_string());
    if let Ok(c) = dunce::canonicalize(&Path::new(path)) {
        push(c.to_string_lossy().into_owned());
    }
    if let Some(rest) = path.strip_prefix("/private") {
        if rest.starts_with('/') {
            push(rest.to_string());
        }
    } else if path.starts_with('/') {
        push(format!("/private{path}"));
    }
    out
}

/// Match a path to a registered `projects.path`, including macOS `/var` ↔
/// `/private/var` aliasing.
pub fn lookup_project_row(path: &str) -> Option<(String, ProjectRow)> {
    path_lookup_candidates(path)
        .into_iter()
        .find_map(|c| lookup_exact(&c).map(|row| (c, row)))
}

/// The authority checks that need only the request (before any write).
pub fn check_db_access(
    caller: &HireCaller,
    requested: Option<&str>,
    max_db: Option<&'static str>,
) -> Result<(), HireError> {
    let Some(req) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    if !super::recipe::DB_ACCESS.contains(&req) {
        return Err(HireError::usage("dbAccess must be 'off', 'read', or 'write'"));
    }
    let rank = super::recipe::db_rank(req);
    if caller.is_agent() && rank > 0 {
        // 0.44.4, now in the daemon: an agent never grants itself (or an
        // agent it hires) database access. Lowering to `off` is fine.
        return Err(HireError::new(
            403,
            "owner_only",
            "An agent can't grant 'db_agent_access'. Ask a human (owner or admin): Settings → Workspaces → (workspace) → Allow this agent to create databases, or `k2 agent hire <dir> --db-access write` from a terminal outside a K2 agent session.",
        ));
    }
    if let Some(max) = max_db {
        if rank > super::recipe::db_rank(max) {
            return Err(HireError::new(
                403,
                "exceeds_key",
                format!("dbAccess '{req}' is above this key's DB door ('{max}'); the owner mints a key with the db door to allow it"),
            ));
        }
    }
    Ok(())
}

/// "Manage Sew" at hire time (S30 K19): the owner, an Owner/Admin login or
/// a hire key may set it; an agent caller never turns it on (it would hand
/// an agent it hires, or itself, the Sew admin plane). Off is always fine.
pub fn check_chat_manage(caller: &HireCaller, requested: Option<bool>) -> Result<(), HireError> {
    if requested == Some(true) && caller.is_agent() {
        return Err(HireError::new(
            403,
            "owner_only",
            "An agent can't turn on 'Manage Chat / Voice Hosting' (chat_manage). Ask a human (owner or admin): Settings → Workspaces → (workspace) → Agent, or `k2 agent access set <workspace> chat_manage on` from a terminal outside a K2 agent session.",
        ));
    }
    Ok(())
}

/// Create-or-converge one workspace.
pub fn converge(
    req: &ConvergeRequest,
    caller: &HireCaller,
    auth: &ConvergeAuthority,
) -> Result<ConvergeOutcome, HireError> {
    let expanded = expand_hire_path(&req.path)?;
    let (path, existing) = match lookup_project_row(&expanded) {
        Some((registered, row)) => (registered, Some(row)),
        None => (expanded, None),
    };
    let template = req.template.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let persona = req.persona.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let persona_raw = req.persona_raw.as_deref().filter(|s| !s.trim().is_empty());
    if (template.is_some() as u8 + persona.is_some() as u8 + persona_raw.is_some() as u8) > 1 {
        return Err(HireError::usage("template and persona are mutually exclusive — pass one or the other"));
    }
    if let Some(t) = template {
        if super::templates::hire_template_body(t).is_none() {
            return Err(HireError::usage(format!("unknown template '{t}' — valid: worker, manager, qa, researcher")));
        }
    }
    check_db_access(caller, req.db_access.as_deref(), auth.max_db)?;
    if existing.is_none() {
        if !auth.may_create {
            return Err(HireError::new(
                403,
                "usage",
                auth.create_refusal.clone().unwrap_or_else(|| "this caller can't create a new workspace".into()),
            ));
        }
        if let Some(root) = auth.new_workspace_root.as_ref() {
            if !super::root::is_under(Path::new(&path), root) {
                return Err(HireError::new(
                    403,
                    "outside_hire_root",
                    format!(
                        "a new workspace made with an API key must be inside the hire root {} (the owner sets it: k2 agent hire-root set <dir>)",
                        root.display()
                    ),
                ));
            }
        }
    }

    let no_wiki = req.no_wiki.unwrap_or(false);
    let seed_agents_md = !req.no_agents_md.unwrap_or(false);
    let fanout = req.fanout.unwrap_or(false);
    let mut changed = false;
    let mut created = false;
    let mut steps_out: Vec<StepReport> = Vec::new();

    if existing.is_none() {
        let dir_exists = Path::new(&path).exists();
        let result = if dir_exists {
            steps_out.push(skipped("create-dir", "exists"));
            crate::workspace::lifecycle::open_workspace_ex(&path, !no_wiki, seed_agents_md, fanout)
        } else {
            if let Some(parent) = Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| HireError::usage(format!("create parent: {e}")))?;
                }
            }
            steps_out.push(done("create-dir"));
            crate::workspace::lifecycle::create_workspace_ex(&path, !no_wiki, seed_agents_md, fanout)
        };
        match result {
            Ok(_) => {
                changed = true;
                created = true;
                steps_out.push(done("register-workspace"));
            }
            Err(e) if crate::workspace::lifecycle::is_already_registered(&e) => {
                steps_out.push(skipped("register-workspace", "already-registered"));
            }
            Err(e) => return Err(HireError::usage(e)),
        }
    } else {
        steps_out.push(skipped("create-dir", "exists"));
        steps_out.push(skipped("register-workspace", "already-registered"));
    }

    if !no_wiki {
        let seeded = crate::wiki::seed_wiki(Path::new(&path)).map_err(|e| HireError::usage(format!("seed wiki: {e}")))?;
        if !seeded.is_empty() {
            changed = true;
        }
    }

    let source = if let Some(t) = template {
        PersonaSource::Template(t.to_string())
    } else if let Some(p) = persona {
        PersonaSource::Body(p.to_string())
    } else if let Some(raw) = persona_raw {
        PersonaSource::Raw(raw.to_string())
    } else {
        PersonaSource::Default
    };
    if steps::ensure_persona(&path, req.role.as_deref(), &source)? {
        changed = true;
        steps_out.push(done("write-persona"));
    } else {
        steps_out.push(skipped("write-persona", "exists"));
    }

    if let Some(name) = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if steps::set_display_name(&path, name)? {
            changed = true;
            steps_out.push(done("set-name"));
        } else {
            steps_out.push(skipped("set-name", "already named"));
        }
    }
    if let Some(preset) = req.preset.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let did = steps::set_setting(&path, "default_agent", preset)?;
        changed |= did;
        steps_out.push(if did { done("set-default-agent") } else { skipped("set-default-agent", "already set") });
    }
    if let Some(model) = req.default_model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let did = steps::set_setting(&path, "default_model", model)?;
        changed |= did;
        steps_out.push(if did { done("set-model") } else { skipped("set-model", "already set") });
    }

    let mut wiki_notes = Vec::new();
    if let Some(notes) = req.wiki.as_ref() {
        wiki_notes = steps::write_wiki_notes(&path, notes)?;
        if !wiki_notes.is_empty() {
            changed = true;
        }
    }
    if let Some(ids) = req.context.as_ref() {
        changed |= steps::add_context(&path, ids)?;
    }
    if let Some(layers) = req.layers.as_ref() {
        changed |= steps::add_inline_layers(&path, layers)?;
    }
    if let Some(access) = req.db_access.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let did = steps::set_setting(&path, "db_agent_access", access)?;
        changed |= did;
        steps_out.push(if did { done("set-db-access") } else { skipped("set-db-access", "already set") });
    }

    let row = lookup_project_row(&path)
        .map(|(_, r)| r)
        .ok_or_else(|| HireError::usage("workspace did not register"))?;
    let basename = Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let handle = row
        .handle
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&basename)
        .to_string();
    let display = {
        let d = crate::workspace::display::agent_display_name(&path);
        if d.is_empty() {
            row.name.clone()
        } else {
            d
        }
    };
    Ok(ConvergeOutcome {
        path,
        id: row.id,
        handle,
        name: display,
        changed,
        created,
        steps: steps_out,
        wiki_notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_callers_never_raise_db_access() {
        let agent = HireCaller::Agent { workspace_id: "w".into() };
        assert_eq!(check_db_access(&agent, Some("write"), None).unwrap_err().code, "owner_only");
        assert_eq!(check_db_access(&agent, Some("read"), None).unwrap_err().code, "owner_only");
        assert!(check_db_access(&agent, Some("off"), None).is_ok());
        assert!(check_db_access(&HireCaller::Owner, Some("write"), None).is_ok());
        assert_eq!(check_chat_manage(&agent, Some(true)).unwrap_err().code, "owner_only");
        assert!(check_chat_manage(&agent, Some(false)).is_ok() && check_chat_manage(&agent, None).is_ok());
        assert!(check_chat_manage(&HireCaller::Owner, Some(true)).is_ok());
        assert!(check_chat_manage(&HireCaller::ApiKey { id: "k".into(), label: None }, Some(true)).is_ok());
    }

    #[test]
    fn key_db_door_caps_db_access() {
        let key = HireCaller::ApiKey { id: "k".into(), label: None };
        assert_eq!(check_db_access(&key, Some("read"), Some("off")).unwrap_err().code, "exceeds_key");
        assert_eq!(check_db_access(&key, Some("write"), Some("read")).unwrap_err().code, "exceeds_key");
        assert!(check_db_access(&key, Some("read"), Some("read")).is_ok());
        assert!(check_db_access(&key, Some("off"), Some("off")).is_ok());
        assert_eq!(check_db_access(&key, Some("admin"), None).unwrap_err().code, "usage");
    }
}
