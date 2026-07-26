//! Context hamburger — optional AGENTS.md layer stack (path references).
//!
//! Always-on workspace context is a **stack of markdown files** composed into
//! `.k2/AGENTS.md`. Pinned layers (primary AGENT.md, PROJECT.md, Tooling
//! footer) are always present and not stored here. Optional layers live in
//! SQLite table `project_context_layers` — **paths + order + enabled + source
//! + label**, never file bodies.
//!
//! See `.k2/prds/prd-context-hamburger-v1.md`.

use std::fs;
use std::path::{Component, Path, PathBuf};

use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Soft size warning threshold for the composed AGENTS.md body (64 KiB).
pub const SOFT_WARN_BYTES: u64 = 64 * 1024;

// ── Wire / API types ──────────────────────────────────────────────────

/// One optional context layer (DB row + disk existence/size).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextLayer {
    pub id: String,
    /// Workspace-relative path with `/` separators.
    pub path: String,
    pub enabled: bool,
    pub position: i64,
    /// `'user'` | `'preset:wiki-index'` | …
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub exists: bool,
    pub bytes: u64,
}

/// System layer info for UI/CLI display (AGENT / PROJECT / Tooling).
/// Enabled flags live on `projects.context_include_*` (default ON).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PinnedLayer {
    pub id: String,
    pub path: String,
    pub label: String,
    pub exists: bool,
    pub bytes: u64,
    /// When true, content is generated (Tooling footer) rather than a file.
    #[serde(default)]
    pub generated: bool,
    /// Whether this system layer is included in AGENTS.md compose.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the AI File Editor can open this path (false for tooling / wiki packs).
    #[serde(default = "default_true")]
    pub editable: bool,
}

fn default_true() -> bool {
    true
}

/// Built-in preset that resolves to a fixed workspace-relative path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextPreset {
    pub id: String,
    pub path: String,
    pub label: String,
    pub source: String,
}

/// Full list response: pinned + optional layers + soft-size estimate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LayerStack {
    pub pinned: Vec<PinnedLayer>,
    pub layers: Vec<ContextLayer>,
    pub soft_warn: bool,
    pub composed_bytes: u64,
}

/// Stable error codes for the context API (HTTP / CLI contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    BadUsage(String),
    NotFound(String),
    PathEscape(String),
    DuplicateLayer(String),
    PresetUnknown(String),
    Db(String),
}

impl ContextError {
    pub fn code(&self) -> &'static str {
        match self {
            ContextError::BadUsage(_) => "bad_usage",
            ContextError::NotFound(_) => "not_found",
            ContextError::PathEscape(_) => "path_escape",
            ContextError::DuplicateLayer(_) => "duplicate_layer",
            ContextError::PresetUnknown(_) => "preset_unknown",
            ContextError::Db(_) => "db_error",
        }
    }

    pub fn hint(&self) -> &str {
        match self {
            ContextError::BadUsage(h)
            | ContextError::NotFound(h)
            | ContextError::PathEscape(h)
            | ContextError::DuplicateLayer(h)
            | ContextError::PresetUnknown(h)
            | ContextError::Db(h) => h,
        }
    }
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.hint())
    }
}

impl std::error::Error for ContextError {}

// ── Presets ───────────────────────────────────────────────────────────

/// Built-in presets (v1 minimum).
pub fn list_presets() -> Vec<ContextPreset> {
    vec![
        ContextPreset {
            id: "wiki:index".into(),
            path: ".k2/wiki/_Index.md".into(),
            label: "Wiki index".into(),
            source: "preset:wiki-index".into(),
        },
        ContextPreset {
            id: "wiki:home".into(),
            path: ".k2/wiki/Home.md".into(),
            label: "Wiki home".into(),
            source: "preset:wiki-home".into(),
        },
    ]
}

fn resolve_preset(preset_id: &str) -> Result<ContextPreset, ContextError> {
    list_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| {
            ContextError::PresetUnknown(format!(
                "unknown preset '{preset_id}'; known: wiki:index, wiki:home"
            ))
        })
}

// ── Project resolution ────────────────────────────────────────────────

/// Resolve a registered project's `id` from its absolute path.
/// Fails if the workspace is not registered in `projects`.
pub fn resolve_project_id(project_path: &str) -> Result<String, ContextError> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::workspace::agent_identity::resolve_project_id(&conn, project_path).ok_or_else(|| {
        ContextError::NotFound(format!(
            "workspace not registered: {project_path}"
        ))
    })
}

// ── Path rules ────────────────────────────────────────────────────────

/// Normalize a path for storage: workspace-relative with `/` separators.
/// Rejects escape outside the workspace root.
pub fn normalize_layer_path(
    project_path: &str,
    raw: &str,
) -> Result<String, ContextError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ContextError::BadUsage("path must not be empty".into()));
    }

    let root = Path::new(project_path);
    let root_canon = root
        .canonicalize()
        .map_err(|e| ContextError::NotFound(format!("workspace path invalid: {e}")))?;

    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };

    // Walk components; reject `..` that would escape before the path exists.
    let mut cleaned = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::ParentDir => {
                if !cleaned.pop() {
                    return Err(ContextError::PathEscape(
                        "path escapes workspace root".into(),
                    ));
                }
            }
            Component::CurDir => {}
            Component::Normal(s) => cleaned.push(s),
            Component::RootDir => cleaned.push(comp.as_os_str()),
            Component::Prefix(p) => cleaned.push(p.as_os_str()),
        }
    }

    // If the path (or a parent) exists, canonicalize and require prefix.
    let resolved = if cleaned.exists() {
        cleaned
            .canonicalize()
            .map_err(|e| ContextError::PathEscape(format!("cannot resolve path: {e}")))?
    } else {
        // Walk up to first existing ancestor, canonicalize that, rejoin.
        let mut existing = cleaned.as_path();
        let mut suffix = Vec::new();
        loop {
            if existing.exists() {
                break;
            }
            match existing.file_name() {
                Some(name) => {
                    suffix.push(name.to_os_string());
                    existing = existing.parent().unwrap_or(Path::new("/"));
                }
                None => break,
            }
        }
        let base = existing
            .canonicalize()
            .unwrap_or_else(|_| existing.to_path_buf());
        let mut joined = base;
        for part in suffix.into_iter().rev() {
            joined.push(part);
        }
        joined
    };

    if !resolved.starts_with(&root_canon) {
        return Err(ContextError::PathEscape(format!(
            "path escapes workspace root: {}",
            raw
        )));
    }

    let rel = resolved
        .strip_prefix(&root_canon)
        .map_err(|_| ContextError::PathEscape("path escapes workspace root".into()))?;

    let rel_str = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    if rel_str.is_empty() {
        return Err(ContextError::BadUsage(
            "path must point at a file inside the workspace, not the root".into(),
        ));
    }

    Ok(rel_str)
}

fn abs_layer_path(project_path: &str, rel: &str) -> PathBuf {
    Path::new(project_path).join(rel)
}

fn disk_meta(project_path: &str, rel: &str) -> (bool, u64) {
    let p = abs_layer_path(project_path, rel);
    match fs::metadata(&p) {
        Ok(m) if m.is_file() => (true, m.len()),
        _ => (false, 0),
    }
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// ── Pinned info ───────────────────────────────────────────────────────

/// Read system-layer include flags (default ON if columns missing / project unknown).
pub fn system_include_flags(project_path: &str) -> (bool, bool, bool) {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COALESCE(context_include_agent, 1), \
                COALESCE(context_include_project, 1), \
                COALESCE(context_include_tooling, 1) \
         FROM projects WHERE path = ?1",
        params![project_path],
        |row| {
            Ok((
                row.get::<_, i64>(0).unwrap_or(1) != 0,
                row.get::<_, i64>(1).unwrap_or(1) != 0,
                row.get::<_, i64>(2).unwrap_or(1) != 0,
            ))
        },
    )
    .unwrap_or((true, true, true))
}

/// Build system-layer display info for a workspace (toggleable defaults).
pub fn pinned_info(project_path: &str) -> Vec<PinnedLayer> {
    use crate::workspace::agent_identity::{agent_dir, find_primary_agent};

    let (inc_agent, inc_project, inc_tooling) = system_include_flags(project_path);
    let mut out = Vec::with_capacity(3);

    // Agent persona
    let agent_rel = if let Some(primary) = find_primary_agent(project_path) {
        let abs = agent_dir(project_path, &primary).join("AGENT.md");
        let root = Path::new(project_path);
        abs.strip_prefix(root)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|_| ".k2/agent/AGENT.md".into())
    } else {
        // Prefer actual dot-dir name when present.
        let agent_md = crate::workspace_dot_dir(project_path).join("agent/AGENT.md");
        if agent_md.exists() {
            let root = Path::new(project_path);
            agent_md
                .strip_prefix(root)
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_else(|_| ".k2/agent/AGENT.md".into())
        } else {
            ".k2/agent/AGENT.md".into()
        }
    };
    let (exists, bytes) = disk_meta(project_path, &agent_rel);
    out.push(PinnedLayer {
        id: "pinned:agent".into(),
        path: agent_rel,
        label: "Agent (persona)".into(),
        exists,
        bytes,
        generated: false,
        enabled: inc_agent,
        editable: true,
    });

    // Project
    let project_md = crate::workspace_dot_dir(project_path).join("PROJECT.md");
    let project_rel = {
        let root = Path::new(project_path);
        project_md
            .strip_prefix(root)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|_| ".k2/PROJECT.md".into())
    };
    let (exists, bytes) = disk_meta(project_path, &project_rel);
    out.push(PinnedLayer {
        id: "pinned:project".into(),
        path: project_rel,
        label: "Project (knowledge)".into(),
        exists,
        bytes,
        generated: false,
        enabled: inc_project,
        editable: true,
    });

    // Tooling footer (generated k2-cli pointer)
    out.push(PinnedLayer {
        id: "pinned:tooling".into(),
        path: String::new(),
        label: "Tooling (k2-cli pointer)".into(),
        exists: true,
        bytes: 0,
        generated: true,
        enabled: inc_tooling,
        editable: false,
    });

    out
}

/// True when a layer id is a system (pinned) toggle: `pinned:agent|project|tooling`.
pub fn is_system_layer_id(id: &str) -> bool {
    matches!(
        id,
        "pinned:agent" | "pinned:project" | "pinned:tooling"
            | "agent" | "project" | "tooling"
    )
}

// ── List / stack ──────────────────────────────────────────────────────

fn row_to_layer(
    project_path: &str,
    id: String,
    path: String,
    enabled: i64,
    position: i64,
    source: String,
    label: Option<String>,
) -> ContextLayer {
    let (exists, bytes) = disk_meta(project_path, &path);
    ContextLayer {
        id,
        path,
        enabled: enabled != 0,
        position,
        source,
        label,
        exists,
        bytes,
    }
}

/// List optional layers for a registered project (all, ordered by position).
pub fn list_layers(project_path: &str) -> Result<Vec<ContextLayer>, ContextError> {
    let project_id = resolve_project_id(project_path)?;
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, path, enabled, position, source, label \
             FROM project_context_layers \
             WHERE project_id = ?1 \
             ORDER BY position ASC, created_at ASC",
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|e| ContextError::Db(e.to_string()))?;

    let mut layers = Vec::new();
    for row in rows {
        let (id, path, enabled, position, source, label) =
            row.map_err(|e| ContextError::Db(e.to_string()))?;
        layers.push(row_to_layer(
            project_path,
            id,
            path,
            enabled,
            position,
            source,
            label,
        ));
    }
    Ok(layers)
}

/// Enabled optional layers only (compose path). Empty if project unregistered.
pub fn list_enabled_layers(project_path: &str) -> Vec<ContextLayer> {
    match list_layers(project_path) {
        Ok(layers) => layers.into_iter().filter(|l| l.enabled).collect(),
        Err(_) => Vec::new(),
    }
}

/// Full stack view for list/show: pinned + optionals + soft-size estimate.
pub fn list_stack(project_path: &str) -> Result<LayerStack, ContextError> {
    let layers = list_layers(project_path)?;
    let pinned = pinned_info(project_path);
    let composed_bytes = estimate_composed_bytes(project_path, &pinned, &layers);
    Ok(LayerStack {
        soft_warn: composed_bytes > SOFT_WARN_BYTES,
        composed_bytes,
        pinned,
        layers,
    })
}

/// Rough byte estimate of the composed AGENTS.md (pinned files + enabled layers).
pub fn estimate_composed_bytes(
    project_path: &str,
    pinned: &[PinnedLayer],
    layers: &[ContextLayer],
) -> u64 {
    let mut total: u64 = 256; // header overhead
    for p in pinned.iter().filter(|p| p.enabled) {
        if p.generated {
            total += 512; // Tooling footer ballpark
        } else {
            total += p.bytes;
        }
    }
    for l in layers.iter().filter(|l| l.enabled) {
        if l.exists {
            total += l.bytes;
        }
    }
    // Also count real compose if cheap — prefer actual when AGENTS.md exists.
    let agents = crate::workspace_dot_dir(project_path).join("AGENTS.md");
    if let Ok(m) = fs::metadata(&agents) {
        // Prefer the larger of estimate vs last written file so soft-warn is conservative.
        total = total.max(m.len());
    }
    let _ = project_path;
    total
}

// ── Mutations ─────────────────────────────────────────────────────────

/// Add a layer by path or preset. Regenerates AGENTS.md on success.
pub fn add_layer(
    project_path: &str,
    path: Option<&str>,
    preset: Option<&str>,
    label: Option<&str>,
) -> Result<ContextLayer, ContextError> {
    let has_path = path.map(|p| !p.trim().is_empty()).unwrap_or(false);
    let has_preset = preset.map(|p| !p.trim().is_empty()).unwrap_or(false);

    if has_path == has_preset {
        return Err(ContextError::BadUsage(
            "provide exactly one of path or preset".into(),
        ));
    }

    let (rel_path, source, default_label) = if has_preset {
        let p = resolve_preset(preset.unwrap().trim())?;
        // Preset paths are fixed relative strings; still normalize to confirm
        // they land under the workspace (and rewrite `.k2/` vs `.k2so/` if needed).
        let rel = normalize_preset_path(project_path, &p.path)?;
        (rel, p.source, Some(p.label))
    } else {
        let rel = normalize_layer_path(project_path, path.unwrap())?;
        (rel, "user".to_string(), None)
    };

    let label = label
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(default_label);

    let project_id = resolve_project_id(project_path)?;
    let db = crate::db::shared();
    let conn = db.lock();

    // Duplicate check (normalized path).
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM project_context_layers \
             WHERE project_id = ?1 AND path = ?2",
            params![project_id, rel_path],
            |r| r.get(0),
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    if exists {
        return Err(ContextError::DuplicateLayer(format!(
            "layer already stacked: {rel_path}"
        )));
    }

    let next_pos: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM project_context_layers \
             WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;

    let id = Uuid::new_v4().to_string();
    let now = now_iso();
    conn.execute(
        "INSERT INTO project_context_layers \
         (id, project_id, path, enabled, position, source, label, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8)",
        params![id, project_id, rel_path, next_pos, source, label, now, now],
    )
    .map_err(|e| ContextError::Db(e.to_string()))?;

    drop(conn);

    // Regen AGENTS.md after mutation.
    crate::workspace::skill_regen::write_workspace_skill_file(project_path);

    let (exists, bytes) = disk_meta(project_path, &rel_path);
    Ok(ContextLayer {
        id,
        path: rel_path,
        enabled: true,
        position: next_pos,
        source,
        label,
        exists,
        bytes,
    })
}

/// Preset paths are authored as `.k2/...`. On legacy `.k2so/` workspaces,
/// rewrite the first segment so the file lands in the real dot-dir.
fn normalize_preset_path(project_path: &str, preset_path: &str) -> Result<String, ContextError> {
    let rel = preset_path.trim_start_matches("./");
    // Try the path as written first.
    if let Ok(n) = normalize_layer_path(project_path, rel) {
        return Ok(n);
    }
    // Rewrite `.k2/` → actual dot-dir basename when the workspace uses `.k2so`.
    let dot = crate::workspace_dot_dir(project_path);
    let dot_name = dot
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| ".k2".into());
    if rel.starts_with(".k2/") && dot_name != ".k2" {
        let rewritten = format!("{dot_name}/{}", &rel[".k2/".len()..]);
        return normalize_layer_path(project_path, &rewritten);
    }
    // Last resort: store the relative string after a soft normalize
    // (workspace may not exist yet for disk checks but is registered).
    if rel.contains("..") {
        return Err(ContextError::PathEscape(
            "path escapes workspace root".into(),
        ));
    }
    Ok(rel.replace('\\', "/"))
}

/// Remove a layer by id or by path. Regenerates AGENTS.md.
pub fn remove_layer(project_path: &str, id_or_path: &str) -> Result<(), ContextError> {
    let project_id = resolve_project_id(project_path)?;
    let id = resolve_layer_id(project_path, &project_id, id_or_path)?;

    let db = crate::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "DELETE FROM project_context_layers WHERE id = ?1 AND project_id = ?2",
            params![id, project_id],
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    if n == 0 {
        return Err(ContextError::NotFound(format!(
            "layer not found: {id_or_path}"
        )));
    }
    // Compact positions.
    renumber_positions(&conn, &project_id)?;
    drop(conn);

    crate::workspace::skill_regen::write_workspace_skill_file(project_path);
    Ok(())
}

/// Enable or disable a layer (optional DB row **or** system pinned id).
/// Regenerates AGENTS.md.
pub fn set_enabled(
    project_path: &str,
    id_or_path: &str,
    enabled: bool,
) -> Result<ContextLayer, ContextError> {
    // System layers: projects.context_include_* columns.
    if let Some(col) = system_flag_column(id_or_path) {
        let db = crate::db::shared();
        let conn = db.lock();
        let n = conn
            .execute(
                &format!("UPDATE projects SET {col} = ?1 WHERE path = ?2"),
                params![if enabled { 1 } else { 0 }, project_path],
            )
            .map_err(|e| ContextError::Db(e.to_string()))?;
        if n == 0 {
            return Err(ContextError::NotFound(format!(
                "workspace not registered: {project_path}"
            )));
        }
        drop(conn);
        crate::workspace::skill_regen::write_workspace_skill_file(project_path);
        // Return a synthetic layer row for the API shape.
        let pinned = pinned_info(project_path);
        let p = pinned
            .into_iter()
            .find(|p| p.id == normalize_system_id(id_or_path))
            .ok_or_else(|| ContextError::NotFound(id_or_path.to_string()))?;
        return Ok(ContextLayer {
            id: p.id,
            path: p.path,
            enabled: p.enabled,
            position: -1,
            source: "system".into(),
            label: Some(p.label),
            exists: p.exists,
            bytes: p.bytes,
        });
    }

    let project_id = resolve_project_id(project_path)?;
    let id = resolve_layer_id(project_path, &project_id, id_or_path)?;

    let db = crate::db::shared();
    let conn = db.lock();
    let now = now_iso();
    let n = conn
        .execute(
            "UPDATE project_context_layers SET enabled = ?1, updated_at = ?2 \
             WHERE id = ?3 AND project_id = ?4",
            params![if enabled { 1 } else { 0 }, now, id, project_id],
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    if n == 0 {
        return Err(ContextError::NotFound(format!(
            "layer not found: {id_or_path}"
        )));
    }
    drop(conn);

    crate::workspace::skill_regen::write_workspace_skill_file(project_path);
    get_layer(project_path, &id)
}

fn normalize_system_id(id: &str) -> String {
    match id {
        "agent" | "pinned:agent" => "pinned:agent".into(),
        "project" | "pinned:project" => "pinned:project".into(),
        "tooling" | "pinned:tooling" => "pinned:tooling".into(),
        other => other.to_string(),
    }
}

fn system_flag_column(id: &str) -> Option<&'static str> {
    match id {
        "pinned:agent" | "agent" => Some("context_include_agent"),
        "pinned:project" | "project" => Some("context_include_project"),
        "pinned:tooling" | "tooling" => Some("context_include_tooling"),
        _ => None,
    }
}

/// Move a layer to an absolute position or by direction.
///
/// `position` is preferred when present (0-based among optionals).
/// `direction` accepts `up` | `down` | `top` | `bottom`.
pub fn move_layer(
    project_path: &str,
    id_or_path: &str,
    position: Option<i64>,
    direction: Option<&str>,
) -> Result<ContextLayer, ContextError> {
    let project_id = resolve_project_id(project_path)?;
    let id = resolve_layer_id(project_path, &project_id, id_or_path)?;

    let mut layers = list_layers(project_path)?;
    if layers.is_empty() {
        return Err(ContextError::NotFound(format!(
            "layer not found: {id_or_path}"
        )));
    }
    let cur_idx = layers
        .iter()
        .position(|l| l.id == id)
        .ok_or_else(|| ContextError::NotFound(format!("layer not found: {id_or_path}")))?;

    let target = if let Some(pos) = position {
        if pos < 0 {
            return Err(ContextError::BadUsage(
                "position must be >= 0".into(),
            ));
        }
        (pos as usize).min(layers.len() - 1)
    } else if let Some(dir) = direction {
        match dir.trim().to_ascii_lowercase().as_str() {
            "up" => cur_idx.saturating_sub(1),
            "down" => (cur_idx + 1).min(layers.len() - 1),
            "top" => 0,
            "bottom" => layers.len() - 1,
            other => {
                return Err(ContextError::BadUsage(format!(
                    "direction must be up|down|top|bottom, got '{other}'"
                )));
            }
        }
    } else {
        return Err(ContextError::BadUsage(
            "provide position or direction (up|down|top|bottom)".into(),
        ));
    };

    if target != cur_idx {
        let item = layers.remove(cur_idx);
        layers.insert(target, item);
    }

    let db = crate::db::shared();
    let conn = db.lock();
    let now = now_iso();
    for (i, layer) in layers.iter().enumerate() {
        conn.execute(
            "UPDATE project_context_layers SET position = ?1, updated_at = ?2 WHERE id = ?3",
            params![i as i64, now, layer.id],
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    }
    drop(conn);

    crate::workspace::skill_regen::write_workspace_skill_file(project_path);
    get_layer(project_path, &id)
}

fn renumber_positions(conn: &rusqlite::Connection, project_id: &str) -> Result<(), ContextError> {
    let mut stmt = conn
        .prepare(
            "SELECT id FROM project_context_layers WHERE project_id = ?1 \
             ORDER BY position ASC, created_at ASC",
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let ids: Vec<String> = stmt
        .query_map(params![project_id], |r| r.get(0))
        .map_err(|e| ContextError::Db(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let now = now_iso();
    for (i, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE project_context_layers SET position = ?1, updated_at = ?2 WHERE id = ?3",
            params![i as i64, now, id],
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    }
    Ok(())
}

fn resolve_layer_id(
    project_path: &str,
    project_id: &str,
    id_or_path: &str,
) -> Result<String, ContextError> {
    let key = id_or_path.trim();
    if key.is_empty() {
        return Err(ContextError::BadUsage("id must not be empty".into()));
    }
    let db = crate::db::shared();
    let conn = db.lock();

    // Exact id match.
    if let Ok(id) = conn.query_row(
        "SELECT id FROM project_context_layers WHERE project_id = ?1 AND id = ?2",
        params![project_id, key],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(id);
    }

    // Id prefix (short uuid).
    if key.len() >= 4 && !key.contains('/') && !key.contains('.') {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM project_context_layers \
                 WHERE project_id = ?1 AND id LIKE ?2",
            )
            .map_err(|e| ContextError::Db(e.to_string()))?;
        let pattern = format!("{key}%");
        let matches: Vec<String> = stmt
            .query_map(params![project_id, pattern], |r| r.get(0))
            .map_err(|e| ContextError::Db(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ContextError::Db(e.to_string()))?;
        if matches.len() == 1 {
            return Ok(matches[0].clone());
        }
        if matches.len() > 1 {
            return Err(ContextError::BadUsage(format!(
                "id prefix '{key}' is ambiguous — use a longer prefix"
            )));
        }
    }

    // Path match (normalized if possible).
    let candidates = [
        key.to_string(),
        normalize_layer_path(project_path, key).unwrap_or_default(),
    ];
    for cand in &candidates {
        if cand.is_empty() {
            continue;
        }
        if let Ok(id) = conn.query_row(
            "SELECT id FROM project_context_layers WHERE project_id = ?1 AND path = ?2",
            params![project_id, cand],
            |r| r.get::<_, String>(0),
        ) {
            return Ok(id);
        }
    }

    Err(ContextError::NotFound(format!(
        "layer not found: {id_or_path}"
    )))
}

fn get_layer(project_path: &str, id: &str) -> Result<ContextLayer, ContextError> {
    let project_id = resolve_project_id(project_path)?;
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id, path, enabled, position, source, label \
         FROM project_context_layers WHERE id = ?1 AND project_id = ?2",
        params![id, project_id],
        |r| {
            Ok(row_to_layer(
                project_path,
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        },
    )
    .map_err(|_| ContextError::NotFound(format!("layer not found: {id}")))
}

// ── Compose helpers ───────────────────────────────────────────────────

/// Section title for an optional layer: label → first H1 → file stem.
pub fn layer_section_title(project_path: &str, layer: &ContextLayer) -> String {
    if let Some(ref label) = layer.label {
        let t = label.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let abs = abs_layer_path(project_path, &layer.path);
    if let Ok(raw) = fs::read_to_string(&abs) {
        for line in raw.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("# ") {
                let h = rest.trim();
                if !h.is_empty() {
                    return h.to_string();
                }
            }
        }
    }
    Path::new(&layer.path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| layer.path.clone())
}

/// Read a layer body for compose (frontmatter stripped). Missing → None.
pub fn read_layer_body(project_path: &str, layer: &ContextLayer) -> Option<String> {
    let abs = abs_layer_path(project_path, &layer.path);
    let raw = fs::read_to_string(&abs).ok()?;
    let body = crate::workspace::wake_prompts::strip_frontmatter(&raw);
    let body = body.trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Force regenerate AGENTS.md for a registered project.
pub fn regen(project_path: &str) -> Result<(), ContextError> {
    let _ = resolve_project_id(project_path)?;
    crate::workspace::skill_regen::write_workspace_skill_file(project_path);
    Ok(())
}

/// Composed AGENTS.md preview (does not write).
pub fn show_composed(project_path: &str) -> Result<String, ContextError> {
    let _ = resolve_project_id(project_path)?;
    Ok(crate::workspace::skill_regen::compose_agents_md_public(
        project_path,
    ))
}

/// Outline of sections in the composed body.
pub fn show_outline(project_path: &str) -> Result<Vec<String>, ContextError> {
    let body = show_composed(project_path)?;
    let mut sections = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            sections.push(rest.trim().to_string());
        }
    }
    Ok(sections)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn unique_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-ctx-layers-{}-{}-{}",
            tag,
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir_all(p.join(".k2/agent")).unwrap();
        fs::create_dir_all(p.join(".k2/wiki")).unwrap();
        p
    }

    fn register_project(path: &str) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "ctx-test", path],
        )
        .expect("insert project");
        id
    }

    fn cleanup_project(path: &str, project_id: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        let _ = conn.execute(
            "DELETE FROM project_context_layers WHERE project_id = ?1",
            params![project_id],
        );
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id]);
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn list_empty_stack_matches_pinned_only() {
        let root = unique_root("empty");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let stack = list_stack(path).expect("list_stack");
        assert!(stack.layers.is_empty(), "no optional layers yet");
        assert_eq!(stack.pinned.len(), 3);
        assert!(stack.pinned[0].label.contains("Agent"));
        assert!(stack.pinned[1].label.contains("Project"));
        assert!(stack.pinned[2].label.contains("Tooling"));
        assert!(stack.pinned[2].generated);
        assert!(stack.pinned[0].enabled);
        assert!(stack.pinned[1].enabled);
        assert!(stack.pinned[2].enabled);
        assert!(!stack.soft_warn);

        cleanup_project(path, &pid);
    }

    #[test]
    fn add_path_layer_roundtrip_and_duplicate() {
        let root = unique_root("add");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let layer_file = root.join("docs/arch.md");
        fs::create_dir_all(layer_file.parent().unwrap()).unwrap();
        fs::write(&layer_file, "# Architecture\n\nDiagrams live here.\n").unwrap();

        let layer = add_layer(path, Some("docs/arch.md"), None, Some("Arch"))
            .expect("add layer");
        assert_eq!(layer.path, "docs/arch.md");
        assert!(layer.enabled);
        assert_eq!(layer.position, 0);
        assert_eq!(layer.source, "user");
        assert_eq!(layer.label.as_deref(), Some("Arch"));
        assert!(layer.exists);
        assert!(layer.bytes > 0);

        let err = add_layer(path, Some("docs/arch.md"), None, None)
            .expect_err("duplicate must fail");
        assert_eq!(err.code(), "duplicate_layer");

        let stack = list_stack(path).unwrap();
        assert_eq!(stack.layers.len(), 1);

        remove_layer(path, &layer.id).expect("remove");
        assert!(list_layers(path).unwrap().is_empty());

        cleanup_project(path, &pid);
    }

    #[test]
    fn path_escape_rejected() {
        let root = unique_root("escape");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let err = add_layer(path, Some("../../etc/passwd"), None, None)
            .expect_err("escape must fail");
        assert_eq!(err.code(), "path_escape", "got {err}");

        cleanup_project(path, &pid);
    }

    #[test]
    fn preset_wiki_index_adds_and_rewrites_source() {
        let root = unique_root("preset");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        fs::write(
            root.join(".k2/wiki/_Index.md"),
            "# Wiki Index\n\n- note a\n",
        )
        .unwrap();

        let layer = add_layer(path, None, Some("wiki:index"), None).expect("preset add");
        assert_eq!(layer.source, "preset:wiki-index");
        assert_eq!(layer.path, ".k2/wiki/_Index.md");
        assert_eq!(layer.label.as_deref(), Some("Wiki index"));
        assert!(layer.exists);

        let err = add_layer(path, None, Some("wiki:nope"), None).expect_err("unknown");
        assert_eq!(err.code(), "preset_unknown");

        cleanup_project(path, &pid);
    }

    #[test]
    fn set_enabled_and_move_reorder() {
        let root = unique_root("move");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        for (name, body) in [("a.md", "# A\n"), ("b.md", "# B\n"), ("c.md", "# C\n")] {
            fs::write(root.join(name), body).unwrap();
            add_layer(path, Some(name), None, None).unwrap();
        }

        let layers = list_layers(path).unwrap();
        assert_eq!(layers.len(), 3);
        let id_a = layers[0].id.clone();
        let id_c = layers[2].id.clone();

        set_enabled(path, &id_a, false).unwrap();
        let a = get_layer(path, &id_a).unwrap();
        assert!(!a.enabled);

        // Move C to top.
        move_layer(path, &id_c, Some(0), None).unwrap();
        let layers = list_layers(path).unwrap();
        assert_eq!(layers[0].path, "c.md");
        assert_eq!(layers[0].position, 0);

        // Direction down.
        move_layer(path, &id_c, None, Some("down")).unwrap();
        let layers = list_layers(path).unwrap();
        assert_eq!(layers[1].path, "c.md");

        cleanup_project(path, &pid);
    }

    #[test]
    fn unregistered_project_fails_loud() {
        let err = list_stack("/tmp/k2-ctx-definitely-not-registered-xyz")
            .expect_err("must fail");
        assert_eq!(err.code(), "not_found");
    }

    #[test]
    fn list_presets_has_wiki_entries() {
        let presets = list_presets();
        assert!(presets.iter().any(|p| p.id == "wiki:index"));
        assert!(presets.iter().any(|p| p.id == "wiki:home"));
    }

    #[test]
    fn missing_layer_file_still_lists_with_exists_false() {
        let root = unique_root("missing");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        // Insert directly so we can reference a missing file without
        // needing the path to exist at add time (normalize still works).
        let layer = add_layer(path, Some("docs/gone.md"), None, None).unwrap();
        assert!(!layer.exists);
        assert_eq!(layer.bytes, 0);

        cleanup_project(path, &pid);
    }

    #[test]
    fn bad_usage_both_path_and_preset() {
        let root = unique_root("both");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let err = add_layer(path, Some("a.md"), Some("wiki:index"), None)
            .expect_err("both is bad_usage");
        assert_eq!(err.code(), "bad_usage");

        let err = add_layer(path, None, None, None).expect_err("neither");
        assert_eq!(err.code(), "bad_usage");

        cleanup_project(path, &pid);
    }
}
