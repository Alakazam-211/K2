//! Per-workspace hire steps, shared by converge (`/v1/w`, `k2 agent hire`)
//! and create (the recipe). Each returns whether it changed anything, so a
//! converge re-run reports `changed: false`.

use std::fs;
use std::path::Path;

use super::recipe::{HeartbeatIn, InlineLayer, SkillIn, WikiNote};
use super::HireError;
use crate::workspace::context_layers::{self, ContextError};

/// How the ROLE.md is made when the workspace has none yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonaSource {
    /// The default scaffold for the role.
    Default,
    /// One of the four hire types.
    Template(String),
    /// A ROLE.md body (frontmatter is generated).
    Body(String),
    /// A whole ROLE.md file as given (`k2 agent hire --persona <file>` and
    /// the CLI's template file, frontmatter included).
    Raw(String),
}

fn ctx_err(e: ContextError) -> HireError {
    let status = match &e {
        ContextError::NotFound(_) => 404,
        ContextError::DuplicateLayer(_) => 409,
        ContextError::Db(_) => 500,
        _ => 400,
    };
    HireError::new(status, e.code(), e.hint().to_string())
}

/// The technical agent name (persona frontmatter `name:`): the folder name
/// with anything but `[A-Za-z0-9_-]` turned into `-`.
pub fn technical_agent_name(path: &str) -> String {
    let base = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "workspace".into());
    let s: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if s.is_empty() {
        "agent".into()
    } else {
        s
    }
}

/// Write the persona if the workspace has none. Never replaces one (that
/// is `k2 agent set --persona`'s job, with backups).
pub fn ensure_persona(path: &str, role: Option<&str>, source: &PersonaSource) -> Result<bool, HireError> {
    let dir = crate::workspace::agent_identity::workspace_agent_path(path);
    if crate::workspace::agent_identity::persona_present_in(&dir) {
        return Ok(false);
    }
    let agent_name = technical_agent_name(path);
    let (role_s, agent_type, prompt) = match source {
        PersonaSource::Template(t) => (
            t.clone(),
            Some(if t == "manager" { "manager" } else { "agent-template" }.to_string()),
            super::templates::hire_template_body(t).map(str::to_string),
        ),
        PersonaSource::Body(b) => (role.unwrap_or("custom").to_string(), None, Some(b.clone())),
        PersonaSource::Raw(_) | PersonaSource::Default => {
            (role.map(str::to_string).unwrap_or_else(|| agent_name.clone()), None, None)
        }
    };
    crate::workspace::agent::create(path.to_string(), agent_name.clone(), role_s, prompt, agent_type)
        .map_err(|e| HireError::internal(format!("persona: {e}")))?;
    if let PersonaSource::Raw(content) = source {
        crate::workspace::agent_editor::k2so_agents_save_agent_md(path.to_string(), agent_name, content.clone())
            .map_err(|e| HireError::internal(format!("persona write: {e}")))?;
    }
    Ok(true)
}

pub fn set_display_name(path: &str, name: &str) -> Result<bool, HireError> {
    let name = name.trim();
    if name.is_empty() || crate::workspace::display::agent_display_name(path) == name {
        return Ok(false);
    }
    crate::workspace::display::set_agent_display_name(path, name).map_err(HireError::usage)?;
    Ok(true)
}

pub fn set_color(path: &str, color: &str) -> Result<bool, HireError> {
    let color = color.trim().to_ascii_lowercase();
    if !super::recipe::color_ok(&color) {
        return Err(HireError::usage("color: a hex color like #4f46e5"));
    }
    let db = crate::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE projects SET color = ?1 WHERE path = ?2 AND COALESCE(color, '') != ?1",
            rusqlite::params![color, path],
        )
        .map_err(|e| HireError::internal(format!("color: {e}")))?;
    Ok(n > 0)
}

/// `default_agent` / `default_model` / `db_agent_access` (the settings
/// allowlist validates the field).
pub fn set_setting(path: &str, field: &str, value: &str) -> Result<bool, HireError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(false);
    }
    let current = current_setting(path, field);
    if current.as_deref() == Some(value) {
        return Ok(false);
    }
    crate::workspace::settings::update_project_setting(path, field, value).map_err(HireError::usage)?;
    Ok(true)
}

fn current_setting(path: &str, field: &str) -> Option<String> {
    if field == "db_agent_access" {
        return Some(crate::workspace::settings::db_agent_access_for_path(path));
    }
    let col = match field {
        "default_agent" => "default_agent",
        "default_model" => "default_model",
        _ => return None,
    };
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        &format!("SELECT {col} FROM projects WHERE path = ?1"),
        rusqlite::params![path],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

pub fn write_wiki_notes(path: &str, notes: &[WikiNote]) -> Result<Vec<serde_json::Value>, HireError> {
    let mut out = Vec::new();
    for n in notes {
        let rel = crate::wiki::write_note(Path::new(path), &n.id, &n.body).map_err(HireError::usage)?;
        out.push(serde_json::json!({ "id": n.id, "path": rel }));
    }
    Ok(out)
}

pub fn add_context(path: &str, ids: &[String]) -> Result<bool, HireError> {
    let mut changed = false;
    for id in ids {
        let id = id.trim();
        if id.is_empty() {
            return Err(HireError::usage("context catalog id must not be empty"));
        }
        match context_layers::add_layer(path, None, Some(id), None) {
            Ok(_) => changed = true,
            Err(ContextError::DuplicateLayer(_)) => {}
            Err(e) => return Err(ctx_err(e)),
        }
    }
    Ok(changed)
}

pub fn sanitize_layer_label(label: &str) -> Result<String, String> {
    let t = label.trim();
    if t.is_empty() {
        return Err("missing layer label".into());
    }
    if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
        return Err("layer label must match [a-zA-Z0-9._-]+ (no spaces or other punctuation)".into());
    }
    Ok(t.to_string())
}

/// Write `.k2/context/<label>.md` then stack it. Returns whether the stack grew.
pub fn add_inline_layer(path: &str, label: &str, markdown: &str) -> Result<bool, HireError> {
    let safe = sanitize_layer_label(label).map_err(HireError::usage)?;
    let rel = format!(".k2/context/{safe}.md");
    let dest = Path::new(path).join(&rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| HireError::usage(format!("mkdir context: {e}")))?;
    }
    fs::write(&dest, markdown).map_err(|e| HireError::usage(format!("write context layer: {e}")))?;
    match context_layers::add_layer(path, Some(&rel), None, Some(&safe)) {
        Ok(_) => Ok(true),
        Err(ContextError::DuplicateLayer(_)) => Ok(false),
        Err(e) => Err(ctx_err(e)),
    }
}

pub fn add_inline_layers(path: &str, layers: &[InlineLayer]) -> Result<bool, HireError> {
    let mut changed = false;
    for l in layers {
        changed |= add_inline_layer(path, &l.label, &l.markdown)?;
    }
    Ok(changed)
}

/// Create each skill and write its SKILL.md text.
pub fn write_skills(path: &str, skills: &[SkillIn]) -> Result<Vec<String>, HireError> {
    let mut out = Vec::new();
    for sk in skills {
        crate::skills::crud::create(path, &sk.name, None).map_err(HireError::usage)?;
        crate::skills::crud::write_skill_md(path, &sk.name, &sk.body)
            .map_err(|e| HireError::internal(format!("skill {}: {e}", sk.name)))?;
        out.push(sk.name.clone());
    }
    Ok(out)
}

pub fn add_heartbeats(path: &str, hbs: &[HeartbeatIn]) -> Result<Vec<String>, HireError> {
    let mut out = Vec::new();
    for hb in hbs {
        crate::heartbeats::k2so_heartbeat_add_with_instructions(
            path.to_string(),
            hb.name.clone(),
            hb.frequency.clone(),
            super::recipe::heartbeat_spec_json(hb),
            hb.instructions.clone(),
        )
        .map_err(|e| HireError::usage(format!("heartbeat {}: {e}", hb.name)))?;
        out.push(hb.name.clone());
    }
    Ok(out)
}
