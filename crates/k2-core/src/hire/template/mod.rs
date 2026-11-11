//! Agent templates (prd-agent-templates-v1, slices T1 + T2): hireable agent
//! types as folders of plain files, one front door, idempotent apply.
//!
//! - **Built-ins** ship inside the binary: every folder of
//!   `crates/k2-core/agent-templates/` (embedded by `build.rs`).
//! - **Library** templates (people's and agents' own) live in
//!   `~/.k2/agent-templates/<id>/v<N>/`; the newest valid version is the
//!   one an id resolves to.
//! - [`resolve`] takes an id or an alias (`worker` → `general`,
//!   `manager` → `lead`, `qa` → `reviewer`): every hire door goes through it
//!   (AT11).
//! - [`apply`] is the one function every door calls (§3): it folds in
//!   converge (folder, register, wiki seed), then the template's own steps
//!   ([`apply::workspace_steps`]), and writes `.k2/agent/template.lock`.
//!   A dry run returns the same rows without writing: that is the review.
//! - A template never carries authority. It *suggests* standing switches;
//!   the permissions step (T3) decides from the hirer's reach. This slice
//!   reports the suggestions and grants nothing.

pub mod apply;
pub mod format;
pub mod lock;
pub mod render;
pub mod sets;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use apply::{apply, apply_with, ApplyReport, ApplyRequest, PermissionPlan, QuestionState, Row, TemplateRef, WorkspaceOutcome};
pub use format::{Fills, Kind, Origin, Question, Template, TemplateToml};
pub use lock::{TemplateLock, LOCK_REL};

use super::{HireCaller, HireError};

include!(concat!(env!("OUT_DIR"), "/agent_templates.rs"));

/// Ids no library template may take: the built-ins K2 Onboarding ships
/// (`k5`, `chief-of-staff`) even before they land here.
const RESERVED_IDS: &[&str] = &["k5", "chief-of-staff", "k2", "system"];

// ── the action catalog seam ──────────────────────────────────────────

/// What the daemon's permission catalog says about an action id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Standing,
    OneTime,
    Unknown,
    /// No catalog installed (k2-core unit tests): shape only.
    NotChecked,
}

static ACTION_CHECK: OnceLock<fn(&str) -> ActionKind> = OnceLock::new();

/// The daemon installs its V1 action lookup at start (the action specs
/// live in k2-daemon), so the loader refuses a one-time or unknown action
/// in `suggest` (AT6). Later calls are ignored.
pub fn install_action_check(f: fn(&str) -> ActionKind) {
    let _ = ACTION_CHECK.set(f);
}

pub(crate) fn action_kind(id: &str) -> ActionKind {
    match ACTION_CHECK.get() {
        Some(f) => f(id),
        None => ActionKind::NotChecked,
    }
}

// ── loading ──────────────────────────────────────────────────────────

/// `~/.k2/agent-templates`.
pub fn library_dir() -> PathBuf {
    crate::paths::k2_home().join("agent-templates")
}

/// Built-in ids and every alias (what library templates may not take).
pub fn reserved_names() -> BTreeSet<String> {
    let mut out: BTreeSet<String> = RESERVED_IDS.iter().map(|s| s.to_string()).collect();
    for (id, files) in AGENT_TEMPLATE_SOURCES {
        out.insert(id.to_string());
        if let Some(t) = files.iter().find(|(p, _)| *p == "template.toml") {
            if let Ok(meta) = toml::from_str::<TemplateToml>(t.1) {
                out.extend(meta.aliases);
            }
        }
    }
    out
}

/// Every built-in, validated. A built-in that fails validation is an error
/// here (the "every built-in loads" test keeps the list clean).
pub fn builtins() -> Vec<Result<Template, String>> {
    let reserved = BTreeSet::new();
    AGENT_TEMPLATE_SOURCES
        .iter()
        .map(|(id, files)| {
            let files: BTreeMap<String, String> = files.iter().map(|(p, b)| (p.to_string(), b.to_string())).collect();
            format::validate(files, None, &format::Expect { origin: Origin::Builtin, id, version: None, reserved: &reserved })
        })
        .collect()
}

/// One library folder's problem, for `list`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoadProblem {
    pub dir: String,
    pub error: String,
}

/// Read a library folder: plain files only, no symlinks (AT4).
fn read_folder(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for ent in rd {
            let p = ent.map_err(|e| format!("{}: {e}", d.display()))?.path();
            let meta = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let rel = p
                .strip_prefix(dir)
                .map_err(|_| format!("{}: outside the template", p.display()))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if meta.file_type().is_symlink() {
                return Err(format!("{rel}: a template may not hold a symlink"));
            }
            if meta.is_dir() {
                stack.push(p);
                continue;
            }
            if meta.len() > super::recipe::MAX_PERSONA_BYTES as u64 {
                return Err(format!("{rel}: over {} bytes", super::recipe::MAX_PERSONA_BYTES));
            }
            let bytes = std::fs::read(&p).map_err(|e| format!("{rel}: {e}"))?;
            let text = String::from_utf8(bytes).map_err(|_| format!("{rel}: not UTF-8 text"))?;
            out.insert(rel, text);
        }
    }
    Ok(out)
}

/// Every library template version: `(id, version, result)`, sorted.
fn library_versions() -> Vec<(String, u32, PathBuf)> {
    let root = library_dir();
    let mut out = Vec::new();
    let Ok(ids) = std::fs::read_dir(&root) else {
        return out;
    };
    for id_ent in ids.flatten() {
        let id_dir = id_ent.path();
        let Some(id) = id_dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
        if id.starts_with('.') || !id_dir.is_dir() {
            continue;
        }
        let Ok(vers) = std::fs::read_dir(&id_dir) else { continue };
        for v_ent in vers.flatten() {
            let v_dir = v_ent.path();
            let n = v_dir
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix('v'))
                .and_then(|n| n.parse::<u32>().ok());
            if let Some(n) = n {
                out.push((id.clone(), n, v_dir));
            }
        }
    }
    out.sort();
    out
}

fn load_library(id: &str, version: u32, dir: &Path, reserved: &BTreeSet<String>) -> Result<Template, String> {
    let files = read_folder(dir)?;
    format::validate(files, Some(dir.to_path_buf()), &format::Expect { origin: Origin::Library, id, version: Some(version), reserved })
}

/// Library templates: the newest valid version of each id, plus what
/// failed to load.
pub fn library() -> (Vec<Template>, Vec<LoadProblem>) {
    let reserved = reserved_names();
    let mut newest: BTreeMap<String, Template> = BTreeMap::new();
    let mut problems = Vec::new();
    for (id, v, dir) in library_versions() {
        match load_library(&id, v, &dir, &reserved) {
            Ok(t) => {
                newest.insert(id, t);
            }
            Err(e) => problems.push(LoadProblem { dir: dir.to_string_lossy().into_owned(), error: e }),
        }
    }
    (newest.into_values().collect(), problems)
}

/// Every version of one library id (`show`).
pub fn library_history(id: &str) -> Vec<u32> {
    library_versions().into_iter().filter(|(i, _, _)| i == id).map(|(_, v, _)| v).collect()
}

fn unknown(id: &str) -> HireError {
    HireError::new(
        404,
        "unknown_template",
        format!("no agent template '{id}' (see: k2 agent template list --all)"),
    )
}

/// Resolve an id or alias: built-ins (ids, then aliases), then the newest
/// library version. A built-in that fails to load is an internal error,
/// never a silent miss.
pub fn resolve(id_or_alias: &str) -> Result<Template, HireError> {
    let want = id_or_alias.trim();
    if want.is_empty() {
        return Err(HireError::usage("missing template id"));
    }
    let mut by_alias = None;
    for t in builtins() {
        let t = t.map_err(HireError::internal)?;
        if t.meta.id == want {
            return Ok(t);
        }
        if by_alias.is_none() && t.meta.aliases.iter().any(|a| a == want) {
            by_alias = Some(t);
        }
    }
    if let Some(t) = by_alias {
        return Ok(t);
    }
    if !format::slug_ok(want, 40, false) {
        return Err(unknown(want));
    }
    let reserved = reserved_names();
    let newest = library_versions().into_iter().filter(|(i, _, _)| i == want).max_by_key(|(_, v, _)| *v);
    match newest {
        Some((id, v, dir)) => load_library(&id, v, &dir, &reserved)
            .map_err(|e| HireError::new(400, "bad_template", e)),
        None => Err(unknown(want)),
    }
}

/// `GET /cli/agent-templates`: built-ins first (General first), then the
/// library ("Yours"). `kind = "system"` only with `all`.
pub fn list_json(all: bool) -> Result<serde_json::Value, HireError> {
    let mut items = Vec::new();
    let mut builtin: Vec<Template> = Vec::new();
    for t in builtins() {
        builtin.push(t.map_err(HireError::internal)?);
    }
    builtin.sort_by_key(|t| (t.meta.id != "general", t.meta.kind == Kind::System, t.meta.name.clone()));
    let (lib, problems) = library();
    for t in builtin.iter().chain(lib.iter()) {
        if t.meta.kind == Kind::System && !all {
            continue;
        }
        items.push(t.summary_json());
    }
    Ok(serde_json::json!({ "ok": true, "templates": items, "problems": problems }))
}

/// `GET /cli/agent-templates/show`.
pub fn show_json(id: &str, with_files: bool) -> Result<serde_json::Value, HireError> {
    let t = resolve(id)?;
    let mut v = t.show_json(with_files);
    if t.origin == Origin::Library {
        v["versions"] = serde_json::json!(library_history(t.id()));
    }
    Ok(serde_json::json!({ "ok": true, "template": v }))
}

// ── who may use which ────────────────────────────────────────────────

/// An API key never hires a system template (`k5`, `chief-of-staff`).
pub fn check_caller(t: &Template, caller: &HireCaller) -> Result<(), HireError> {
    if t.meta.kind == Kind::System && matches!(caller, HireCaller::ApiKey { .. }) {
        return Err(HireError::new(
            403,
            "system_template",
            format!("'{}' is a system template; an API key can't hire it", t.meta.id),
        ));
    }
    Ok(())
}

/// The hirer's display name: the `{{person}}` placeholder. Agents and API
/// keys hire for the server's owner.
pub fn person_for(caller: &HireCaller) -> String {
    let login = match caller {
        HireCaller::Login { username } => username.as_str(),
        _ => "owner",
    };
    let n = crate::person_name::load_effective(login).display_name;
    if n.trim().is_empty() { "your person".to_string() } else { n }
}

/// Check the hirer's params against the template's `[params]` and return
/// every placeholder's value: the declared params (empty when not given),
/// plus `person` (given, or the hirer's name). `name` is filled at apply.
pub fn bind_params(t: &Template, given: &BTreeMap<String, String>, caller: &HireCaller) -> Result<BTreeMap<String, String>, HireError> {
    let declared: Vec<&String> = t.meta.params.keys().collect();
    let takes = || {
        if declared.is_empty() {
            "it takes no params".to_string()
        } else {
            format!("it takes: {}", declared.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
        }
    };
    let mut out = BTreeMap::new();
    for (k, v) in given {
        let bad_text = v.chars().any(|c| c.is_control() && c != '\n');
        match k.as_str() {
            "person" => {
                if v.trim().is_empty() || v.chars().count() > 80 || v.contains('\n') || bad_text {
                    return Err(HireError::usage("param person: 1–80 characters, one line"));
                }
            }
            "name" => return Err(HireError::usage("the agent's name is --name (the `name` field), not a param")),
            _ => {
                let Some(spec) = t.meta.params.get(k) else {
                    return Err(HireError::usage(format!("template '{}' has no param '{k}'; {}", t.meta.id, takes())));
                };
                if v.chars().count() > spec.max || bad_text {
                    return Err(HireError::usage(format!("param {k}: at most {} characters, no control characters", spec.max)));
                }
            }
        }
        out.insert(k.clone(), v.trim().to_string());
    }
    for (k, spec) in &t.meta.params {
        let have = out.get(k).is_some_and(|v| !v.is_empty());
        if spec.required && !have {
            return Err(HireError::usage(format!("template '{}' needs param {k} ({})", t.meta.id, spec.label)));
        }
        out.entry(k.clone()).or_default();
    }
    out.entry("person".to_string()).or_insert_with(|| person_for(caller));
    Ok(out)
}

/// Check the hirer's answers against the template's `[[questions]]`: every
/// id must be one of them (the refusal names the valid ids), every answer
/// non-empty, at most [`format::MAX_ANSWER_CHARS`], with no control
/// characters but newlines. Returns them trimmed.
pub fn bind_answers(t: &Template, given: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, HireError> {
    let ids: Vec<&str> = t.meta.questions.iter().map(|q| q.id.as_str()).collect();
    let mut out = BTreeMap::new();
    for (k, v) in given {
        let k = k.trim();
        if !ids.contains(&k) {
            let valid = if ids.is_empty() {
                "it asks no questions".to_string()
            } else {
                format!("its question ids: {}", ids.join(", "))
            };
            return Err(HireError::usage(format!(
                "template '{}' has no question '{k}'; {valid} (see: k2 agent template show {})",
                t.meta.id, t.meta.id
            )));
        }
        let v = v.replace("\r\n", "\n");
        let v = v.trim();
        if v.is_empty() {
            return Err(HireError::usage(format!(
                "answer {k} is empty; leave out a question you can't answer (the agent asks its person)"
            )));
        }
        if v.chars().count() > format::MAX_ANSWER_CHARS || v.chars().any(|c| c.is_control() && c != '\n') {
            return Err(HireError::usage(format!(
                "answer {k}: at most {} characters, no control characters",
                format::MAX_ANSWER_CHARS
            )));
        }
        out.insert(k.to_string(), v.to_string());
    }
    Ok(out)
}
