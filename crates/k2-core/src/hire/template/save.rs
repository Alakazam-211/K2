//! Save as template, `add` and `remove` (prd-agent-templates-v1 §7, slice
//! T6): the library side of agent templates.
//!
//! - **Save takes an allowlist** (AT19), never "everything but": the
//!   ROLE.md body, the stacked catalog ids, inline layers under
//!   `.k2/context/`, skills the workspace made (not K2-managed ones),
//!   heartbeats (name, schedule, WAKEUP text), the preset as `tool`, the
//!   standing switches that are on as `suggest` (the daemon reads those),
//!   and only the wiki notes picked. Threads, transcripts, vault secrets,
//!   mail, sign-ins, `.env` files, App logins and connections are never
//!   read.
//! - **The secret check** ([`crate::secret_paths::secret_line`]) runs on
//!   every source file taken, and names the file and line, never the secret.
//! - **Versions:** the same author saving an existing id writes `v<N+1>`;
//!   another author's id is `409 id_taken`; built-in ids and aliases are
//!   `409 id_reserved`. Earlier versions are kept.
//! - The result is validated by the same loader `list` and `apply` use, then
//!   written to `~/.k2/agent-templates/<id>/v<N>/` through a temp folder and
//!   one rename.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use crate::brand::{CLI_NAME, PRODUCT_NAME};

use super::super::recipe;
use super::super::HireError;
use super::format::{self, Expect, Origin, Template};
use crate::skills::version::{SKILL_BEGIN_MARKER, SKILL_END_MARKER};

/// A refusal from the template library routes: a [`HireError`] plus, for
/// `secret_found`, the file and line (never the secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    pub status: u16,
    pub code: &'static str,
    pub hint: String,
    pub file: Option<String>,
    pub line: Option<usize>,
}

impl TemplateError {
    pub fn new(status: u16, code: &'static str, hint: impl Into<String>) -> Self {
        Self { status, code, hint: hint.into(), file: None, line: None }
    }

    /// `{ok:false, error:{code, message, hint, file?, line?}}`.
    pub fn body(&self) -> serde_json::Value {
        let mut e = serde_json::json!({ "code": self.code, "message": self.hint, "hint": self.hint });
        if let Some(f) = &self.file {
            e["file"] = serde_json::json!(f);
        }
        if let Some(l) = self.line {
            e["line"] = serde_json::json!(l);
        }
        serde_json::json!({ "ok": false, "error": e })
    }
}

impl From<HireError> for TemplateError {
    fn from(e: HireError) -> Self {
        Self::new(e.status, e.code, e.hint)
    }
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.hint)
    }
}

type R<T> = Result<T, TemplateError>;

fn bad_id(id: &str) -> TemplateError {
    TemplateError::new(400, "bad_id", format!("'{id}' isn't a template id: a-z, 0-9 and -, starting with a letter or digit, at most 40"))
}

fn internal(m: impl Into<String>) -> TemplateError {
    TemplateError::new(500, "template_failed", m)
}

/// The secret check over one text: `422 secret_found` naming `file` and the
/// line. Shared by save and `add`.
pub fn check_secret(file: &str, text: &str) -> R<()> {
    match crate::secret_paths::secret_line(text) {
        None => Ok(()),
        Some((line, kind)) => Err(TemplateError {
            status: 422,
            code: "secret_found",
            hint: format!(
                "{file} line {line} holds what looks like {kind}. A template never carries secrets: remove it (keep secrets in the {PRODUCT_NAME} vault), then try again"
            ),
            file: Some(file.to_string()),
            line: Some(line),
        }),
    }
}

// ── ids, authors and versions ────────────────────────────────────────

/// Built-in ids, aliases and the reserved names are `409 id_reserved`; a
/// bad slug is `400 bad_id`.
pub fn check_id(id: &str) -> R<()> {
    if !format::slug_ok(id, 40, false) {
        return Err(bad_id(id));
    }
    if super::reserved_names().contains(id) {
        return Err(TemplateError::new(
            409,
            "id_reserved",
            format!("'{id}' is one of {PRODUCT_NAME}'s built-in template ids or old ids; pick another id"),
        ));
    }
    Ok(())
}

/// `user:<login>` or `agent:<handle>` (only built-ins say K2).
pub fn check_author(author: &str) -> R<()> {
    let ok = ["user:", "agent:"].iter().any(|p| {
        author.strip_prefix(p).is_some_and(|n| {
            let n = n.trim();
            !n.is_empty() && n.chars().count() <= 80 && !n.chars().any(|c| c.is_control() || c == '"' || c == '\\')
        })
    });
    if ok {
        Ok(())
    } else {
        Err(internal(format!("template author '{author}' must be user:<login> or agent:<handle>")))
    }
}

/// The author recorded in one library version's `template.toml`, if it
/// reads.
fn author_of(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("template.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("author").and_then(|a| a.as_str()).map(|a| a.trim().to_string())
}

/// Every version of a library id: `(version, folder)`, oldest first.
pub fn versions_of(id: &str) -> Vec<(u32, PathBuf)> {
    super::library_versions().into_iter().filter(|(i, _, _)| i == id).map(|(_, v, d)| (v, d)).collect()
}

/// The version `author` writes for `id`: 1 for a new id, `N+1` for its own
/// id, `409 id_taken` when another author holds it.
pub fn next_version(id: &str, author: &str) -> R<u32> {
    check_id(id)?;
    let have = versions_of(id);
    if have.iter().any(|(_, d)| author_of(d).as_deref() != Some(author)) {
        let by = have.iter().find_map(|(_, d)| author_of(d)).unwrap_or_else(|| "someone else".into());
        return Err(TemplateError::new(
            409,
            "id_taken",
            format!("the template id '{id}' belongs to {by}; save under another id"),
        ));
    }
    Ok(have.iter().map(|(v, _)| *v).max().unwrap_or(0) + 1)
}

/// `~/…` when under the home folder (what people read), else absolute.
/// The part under home is `/`-joined on every OS (`~/.k2/agent-templates/…`).
pub fn display_path(p: &Path) -> String {
    match crate::paths::home_dir().and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~/{}", slash_joined(&rest)),
        None => p.to_string_lossy().into_owned(),
    }
}

/// A relative path with `/` between its parts on every OS (Windows too), the
/// way template files, refusals and the CLI name them.
fn slash_joined(rel: &Path) -> String {
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

// ── the outcome ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub bytes: usize,
}

/// Something the workspace has that a template doesn't take, and why.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Skipped {
    pub what: &'static str,
    pub name: String,
    pub why: &'static str,
}

fn skip(what: &'static str, name: impl Into<String>, why: &'static str) -> Skipped {
    Skipped { what, name: name.into(), why }
}

/// The save / add response (the contract's shape).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    pub ok: bool,
    pub dry_run: bool,
    pub id: String,
    pub version: u32,
    pub author: String,
    pub path: String,
    pub hash: String,
    pub files: Vec<FileEntry>,
    pub catalog: Vec<String>,
    pub suggest: Vec<String>,
    pub tool: Option<String>,
    pub heartbeats: Vec<String>,
    pub skipped: Vec<Skipped>,
}

fn outcome(t: &Template, author: &str, dir: &Path, dry_run: bool, skipped: Vec<Skipped>) -> SaveOutcome {
    let mut files: Vec<FileEntry> = t.files.iter().map(|(p, b)| FileEntry { path: p.clone(), bytes: b.len() }).collect();
    // template.toml first, then ROLE.md, then the rest by path.
    files.sort_by_key(|f| (f.path != "template.toml", f.path != "ROLE.md", f.path.clone()));
    SaveOutcome {
        ok: true,
        dry_run,
        id: t.meta.id.clone(),
        version: t.meta.version,
        author: author.to_string(),
        path: display_path(dir),
        hash: format!("sha256:{}", t.hash),
        files,
        catalog: t.meta.context.catalog.clone(),
        suggest: t.meta.permissions.suggest.clone(),
        tool: t.meta.agent.tool.clone(),
        heartbeats: t.meta.heartbeats.iter().map(|h| h.name.clone()).collect(),
        skipped,
    }
}

// ── writing the library ──────────────────────────────────────────────

/// One library write at a time in this process (two saves of one id pick
/// different versions).
static LIBRARY_LOCK: Mutex<()> = Mutex::new(());

fn version_dir(id: &str, v: u32) -> PathBuf {
    super::library_dir().join(id).join(format!("v{v}"))
}

/// Write `files` to `<lib>/<id>/v<N>/` through a temp folder and one rename.
fn write_version(id: &str, v: u32, files: &BTreeMap<String, String>) -> R<PathBuf> {
    let dest = version_dir(id, v);
    if dest.exists() {
        return Err(TemplateError::new(409, "id_taken", format!("{} already exists", display_path(&dest))));
    }
    let parent = dest.parent().ok_or_else(|| internal("template folder has no parent"))?;
    std::fs::create_dir_all(parent).map_err(|e| internal(format!("{}: {e}", parent.display())))?;
    let tmp = parent.join(format!(".v{v}.tmp-{}", uuid::Uuid::new_v4().simple()));
    let res = (|| -> std::io::Result<()> {
        for (rel, body) in files {
            let p = tmp.join(rel);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&p, body)?;
        }
        std::fs::rename(&tmp, &dest)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(internal(format!("write {}: {e}", display_path(&dest))));
    }
    Ok(dest)
}

/// Validate a library template's files as `id` v`version` (the loader's
/// own check), mapping a cap to `413 too_large` and anything else to
/// `400 bad_template`.
fn validate_library(files: BTreeMap<String, String>, id: &str, version: u32) -> R<Template> {
    check_caps(&files)?;
    let reserved = super::reserved_names();
    format::validate(files, None, &Expect { origin: Origin::Library, id, version: Some(version), reserved: &reserved })
        .map_err(|e| TemplateError::new(400, "bad_template", e))
}

/// The recipe caps (persona and files 64 KiB, notes ≤ 40, skills ≤ 20,
/// heartbeats ≤ 12, layers ≤ 20).
fn check_caps(files: &BTreeMap<String, String>) -> R<()> {
    let too = |m: String| Err(TemplateError::new(413, "too_large", m));
    for (p, b) in files {
        if b.len() > recipe::MAX_PERSONA_BYTES {
            return too(format!("{p} is {} bytes; a template file is at most {}", b.len(), recipe::MAX_PERSONA_BYTES));
        }
    }
    let count = |pre: &str| files.keys().filter(|p| p.starts_with(pre)).count();
    for (pre, max, what) in [
        ("wiki/", recipe::MAX_NOTES, "wiki notes"),
        ("skills/", recipe::MAX_SKILLS, "skills"),
        ("context/", recipe::MAX_LAYERS, "context layers"),
    ] {
        if count(pre) > max {
            return too(format!("{} {what}; a template holds at most {max}", count(pre)));
        }
    }
    if let Some(m) = files.get("template.toml") {
        if let Ok(v) = toml::from_str::<toml::Value>(m) {
            let n = v.get("heartbeats").and_then(|h| h.as_array()).map_or(0, Vec::len);
            if n > recipe::MAX_HEARTBEATS {
                return too(format!("{n} heartbeats; a template holds at most {}", recipe::MAX_HEARTBEATS));
            }
        }
    }
    Ok(())
}

// ── save ─────────────────────────────────────────────────────────────

/// What the daemon resolved for a save.
#[derive(Debug, Clone, Default)]
pub struct SaveInput {
    /// The workspace's absolute path (registered).
    pub workspace: String,
    pub id: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    /// Wiki note ids to take (`Voice and style` or `Voice and style.md`).
    pub wiki: Vec<String>,
    pub dry_run: bool,
    /// `user:<login>` or `agent:<handle>`.
    pub author: String,
    /// Standing V1 actions now on for this workspace (the daemon reads them).
    pub suggest: Vec<String>,
    /// "Can ask for permission cards" is on.
    pub cards: bool,
}

/// `template.toml` as save writes it (fields in the order TOML needs:
/// values, then tables, then arrays of tables).
#[derive(Serialize)]
struct Manifest {
    schema: u32,
    id: String,
    version: u32,
    name: String,
    summary: String,
    kind: &'static str,
    author: String,
    shared_block: bool,
    agent: ManifestAgent,
    context: ManifestContext,
    permissions: ManifestPermissions,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    heartbeats: Vec<ManifestHeartbeat>,
}

#[derive(Serialize)]
struct ManifestAgent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    tool: String,
}

#[derive(Serialize)]
struct ManifestContext {
    catalog: Vec<String>,
}

#[derive(Serialize)]
struct ManifestPermissions {
    cards: bool,
    suggest: Vec<String>,
}

#[derive(Serialize)]
struct ManifestHeartbeat {
    name: String,
    frequency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spec: Option<toml::Value>,
}

/// One-line text: control characters out, at most `max` characters.
fn one_line(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(max).collect::<String>().trim().to_string()
}

/// The ROLE.md body a template takes: frontmatter and the `# Persona — …`
/// heading out. When the body has a K2:MANAGED block, its inside is the
/// charter and the "Your notes" part follows as written (K2's stock heading
/// and line dropped, so a hire from this template doesn't get two).
pub fn charter_from_role(text: &str) -> String {
    let body = crate::workspace::wake_prompts::strip_frontmatter(text);
    let drop_heading = |s: &str| -> String {
        let s = s.trim_start();
        match s.lines().next() {
            Some(l) if l.starts_with("# Persona") => s[l.len()..].trim_start().to_string(),
            _ => s.to_string(),
        }
    };
    let (Some(b), Some(e)) = (body.find(SKILL_BEGIN_MARKER), body.find(SKILL_END_MARKER)) else {
        return format!("{}\n", drop_heading(&body).trim_end());
    };
    if e < b {
        return format!("{}\n", drop_heading(&body).trim_end());
    }
    let inside = body[b + SKILL_BEGIN_MARKER.len()..e].trim();
    let notes: Vec<&str> = body[e + SKILL_END_MARKER.len()..]
        .lines()
        .filter(|l| {
            let t = l.trim();
            t != "## Your notes" && !t.starts_with("_Yours: K2 never changes")
        })
        .collect();
    let notes = notes.join("\n");
    let notes = notes.trim();
    if notes.is_empty() {
        format!("{inside}\n")
    } else {
        format!("{inside}\n\n{notes}\n")
    }
}

/// A taken file: where it goes in the template and where it came from.
struct Taken {
    rel: String,
    body: String,
}

fn read_text(abs: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(abs).ok()?;
    if !meta.is_file() {
        return None; // a symlink or folder is never followed
    }
    std::fs::read_to_string(abs).ok()
}

fn ws_rel(ws: &str, abs: &Path) -> String {
    abs.strip_prefix(ws).map(slash_joined).unwrap_or_else(|_| abs.to_string_lossy().into_owned())
}

/// Gather what a save takes from `inp.workspace` (every source text passes
/// the secret check first, named by its workspace path).
fn gather(inp: &SaveInput) -> R<(BTreeMap<String, String>, Vec<Skipped>, Manifest)> {
    let ws = inp.workspace.as_str();
    let mut taken: Vec<Taken> = Vec::new();
    let mut skipped = Vec::new();

    // ROLE.md.
    let agent_dir = crate::workspace::agent_identity::workspace_agent_path(ws);
    let role_abs = agent_dir.join(crate::workspace::agent_identity::PERSONA_MD_NAME);
    let role_text = read_text(&role_abs).ok_or_else(|| {
        TemplateError::new(400, "no_charter", format!("{} has no ROLE.md to save; give the agent a charter first", ws_rel(ws, &role_abs)))
    })?;
    check_secret(&ws_rel(ws, &role_abs), &role_text)?;
    let charter = charter_from_role(&role_text);
    if charter.trim().is_empty() {
        return Err(TemplateError::new(400, "no_charter", "the agent's ROLE.md is empty; give it a charter first"));
    }
    let role = crate::workspace::agent_identity::parse_frontmatter(&role_text)
        .get("role")
        .map(|r| r.trim().to_string())
        .filter(|r| format::slug_ok(r, 40, true));
    taken.push(Taken { rel: "ROLE.md".into(), body: charter });

    // Catalog ids and inline layers.
    let mut catalog: Vec<String> = Vec::new();
    let layers = crate::workspace::context_layers::list_layers(ws).map_err(|e| TemplateError::new(400, e.code(), e.hint().to_string()))?;
    for l in layers {
        if let Some(src) = l.source.strip_prefix("catalog:").map(|_| l.source.as_str()) {
            match crate::workspace::context_layers::resolve_catalog_entry(src) {
                Ok(e) => {
                    if !catalog.contains(&e.id) {
                        catalog.push(e.id);
                    }
                }
                Err(_) => skipped.push(skip("catalog", src, "unknown_catalog")),
            }
            continue;
        }
        if l.source != "user" {
            skipped.push(skip("layer", l.path.clone(), "unknown_source"));
            continue;
        }
        let file = l.path.strip_prefix(".k2/context/").or_else(|| l.path.strip_prefix(".k2so/context/"));
        let label = file
            .and_then(|f| f.strip_suffix(".md"))
            .filter(|f| !f.contains('/'))
            .and_then(|f| super::super::steps::sanitize_layer_label(f).ok());
        let Some(label) = label else {
            skipped.push(skip("layer", l.path.clone(), "outside_context"));
            continue;
        };
        let Some(text) = read_text(&Path::new(ws).join(&l.path)) else {
            skipped.push(skip("layer", l.path.clone(), "missing"));
            continue;
        };
        check_secret(&l.path, &text)?;
        taken.push(Taken { rel: format!("context/{label}.md"), body: text });
    }

    // Skills the workspace made (K2-managed ones stay K2's).
    let skills_root = crate::workspace::agent_identity::skills_dir(ws);
    let mut names: Vec<String> = std::fs::read_dir(&skills_root)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).collect())
        .unwrap_or_default();
    names.sort();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let abs = skills_root.join(&name).join("SKILL.md");
        if !skills_root.join(&name).is_dir() {
            continue;
        }
        if !format::slug_ok(&name, 64, true) {
            skipped.push(skip("skill", name, "bad_name"));
            continue;
        }
        let Some(text) = read_text(&abs) else {
            skipped.push(skip("skill", name, "missing"));
            continue;
        };
        let parsed = crate::skills::version::parse_skill(&text);
        if parsed.k2_skill.is_some() || parsed.has_markers {
            skipped.push(skip("skill", name, "k2_managed"));
            continue;
        }
        check_secret(&ws_rel(ws, &abs), &text)?;
        taken.push(Taken { rel: format!("skills/{name}/SKILL.md"), body: text });
    }

    // Heartbeats: name, schedule, WAKEUP text.
    let mut heartbeats = Vec::new();
    let hbs = crate::heartbeats::k2_heartbeat_list(ws.to_string()).unwrap_or_default();
    for h in hbs {
        let rel = h.wakeup_path.trim().to_string();
        let inside = !rel.starts_with('/') && !rel.split('/').any(|s| s == "..") && (rel.starts_with(".k2/heartbeats/") || rel.starts_with(".k2so/heartbeats/"));
        if !inside {
            skipped.push(skip("heartbeat", h.name.clone(), "outside_heartbeats"));
            continue;
        }
        let raw = read_text(&Path::new(ws).join(&rel)).unwrap_or_default();
        check_secret(&rel, &raw)?;
        let body = crate::workspace::wake_prompts::strip_frontmatter(&raw).trim().to_string();
        if body.len() > recipe::MAX_BRIEF_BYTES {
            return Err(TemplateError::new(
                413,
                "too_large",
                format!("{rel} is {} bytes; a heartbeat's WAKEUP text is at most {}", body.len(), recipe::MAX_BRIEF_BYTES),
            ));
        }
        let spec = serde_json::from_str::<serde_json::Value>(&h.spec_json).ok().and_then(|v| match v {
            serde_json::Value::Object(mut m) => {
                m.remove("frequency");
                m.retain(|_, v| !v.is_null());
                if m.is_empty() {
                    None
                } else {
                    toml::Value::try_from(serde_json::Value::Object(m)).ok()
                }
            }
            _ => None,
        });
        heartbeats.push(ManifestHeartbeat {
            name: h.name.clone(),
            frequency: h.frequency.clone(),
            instructions: (!body.is_empty()).then_some(body),
            spec,
        });
    }

    // Only the picked wiki notes.
    for raw in &inp.wiki {
        let id = raw.trim();
        let id = if id.ends_with(".md") { id.to_string() } else { format!("{id}.md") };
        crate::wiki::validate_note_id(&id).map_err(|_| TemplateError::new(400, "usage", format!("'{raw}' isn't a wiki note id")))?;
        let rel = format!(".k2/wiki/{id}");
        let Some(text) = read_text(&Path::new(ws).join(&rel)) else {
            return Err(TemplateError::new(404, "no_note", format!("no wiki note {rel} in this workspace (see: {CLI_NAME} wiki list)")));
        };
        check_secret(&rel, &text)?;
        if !taken.iter().any(|t| t.rel == format!("wiki/{id}")) {
            taken.push(Taken { rel: format!("wiki/{id}"), body: text });
        }
    }

    let tool = super::super::steps::current_setting(ws, "default_agent")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "default".into());
    let name = inp
        .name
        .as_deref()
        .map(|n| one_line(n, 60))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            let d = one_line(&crate::workspace::display::agent_display_name(ws), 60);
            if d.is_empty() { inp.id.clone() } else { d }
        });
    let summary = one_line(inp.summary.as_deref().unwrap_or_default(), 200);
    let manifest = Manifest {
        schema: format::SCHEMA,
        id: inp.id.clone(),
        version: 0,
        name,
        summary,
        kind: "agent",
        author: inp.author.clone(),
        // The shared block is already inside the charter it took.
        shared_block: false,
        agent: ManifestAgent { role, tool },
        context: ManifestContext { catalog },
        permissions: ManifestPermissions { cards: inp.cards, suggest: inp.suggest.clone() },
        heartbeats,
    };
    let files = taken.into_iter().map(|t| (t.rel, t.body)).collect();
    Ok((files, skipped, manifest))
}

/// `POST /cli/agent-templates/save`: take the workspace's setup, check it,
/// and write `<id>/v<N>` (or, in a dry run, say what would be written).
pub fn save(inp: &SaveInput) -> R<SaveOutcome> {
    check_id(&inp.id)?;
    check_author(&inp.author)?;
    let _guard = LIBRARY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let version = next_version(&inp.id, &inp.author)?;
    let (mut files, skipped, mut manifest) = gather(inp)?;
    manifest.version = version;
    let toml_text = format!(
        "# Saved by {PRODUCT_NAME} from an agent's setup ({CLI_NAME} agent template save).\n{}",
        toml::to_string(&manifest).map_err(|e| internal(format!("template.toml: {e}")))?
    );
    check_secret("template.toml", &toml_text)?;
    files.insert("template.toml".into(), toml_text);
    let t = validate_library(files, &inp.id, version)?;
    let dir = version_dir(&inp.id, version);
    if !inp.dry_run {
        write_version(&inp.id, version, &t.files)?;
    }
    Ok(outcome(&t, &inp.author, &dir, inp.dry_run, skipped))
}

// ── add ──────────────────────────────────────────────────────────────

/// Set a top-level `key = value` in `template.toml` text (before the first
/// table header), or insert it after the first line that sets `after`.
fn set_top_level(text: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    let mut in_tables = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            in_tables = true;
        }
        let is_key = !in_tables
            && t.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='))
            && !done;
        if is_key {
            out.push(format!("{key} = {value}"));
            done = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !done {
        // Before the first table (or at the end).
        let at = out.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(out.len());
        out.insert(at, format!("{key} = {value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// `POST /cli/agent-templates/add {dir}`: install a template folder into
/// the library. Same checks as save (paths, no symlinks, caps, the secret
/// check, the id rules). The installer is the library copy's author, and
/// it lands as the id's next version.
pub fn add(dir: &Path, author: &str, dry_run: bool) -> R<SaveOutcome> {
    check_author(author)?;
    if !dir.is_dir() {
        return Err(TemplateError::new(404, "no_folder", format!("{} is not a folder", dir.display())));
    }
    let mut files = super::read_folder(dir).map_err(|e| TemplateError::new(400, "bad_template", e))?;
    for (rel, body) in &files {
        format::classify_path(rel).map_err(|e| TemplateError::new(400, "bad_template", e))?;
        check_secret(rel, body)?;
    }
    let manifest = files.get("template.toml").cloned().ok_or_else(|| {
        TemplateError::new(400, "bad_template", format!("{} has no template.toml", dir.display()))
    })?;
    let id = toml::from_str::<toml::Value>(&manifest)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .ok_or_else(|| TemplateError::new(400, "bad_template", "template.toml has no id"))?;
    check_id(&id)?;
    let _guard = LIBRARY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let version = next_version(&id, author)?;
    let manifest = set_top_level(&set_top_level(&manifest, "version", &version.to_string()), "author", &toml_str(author));
    files.insert("template.toml".into(), manifest);
    let t = validate_library(files, &id, version)?;
    let dest = version_dir(&id, version);
    if !dry_run {
        write_version(&id, version, &t.files)?;
    }
    Ok(outcome(&t, author, &dest, dry_run, Vec::new()))
}

// ── remove ───────────────────────────────────────────────────────────

/// `POST /cli/agent-templates/remove {id, version?}`. `only_author`: an
/// agent removes only templates it authored (every version it would
/// remove names it).
pub fn remove(id: &str, version: Option<u32>, only_author: Option<&str>) -> R<serde_json::Value> {
    let id = id.trim();
    if !format::slug_ok(id, 40, false) {
        return Err(bad_id(id));
    }
    if super::reserved_names().contains(id) {
        return Err(TemplateError::new(409, "id_reserved", format!("'{id}' is one of {PRODUCT_NAME}'s built-in templates; it can't be removed")));
    }
    let _guard = LIBRARY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let have = versions_of(id);
    if have.is_empty() {
        return Err(TemplateError::new(404, "unknown_template", format!("no saved template '{id}' (see: {CLI_NAME} agent template list)")));
    }
    let doomed: Vec<(u32, PathBuf)> = match version {
        Some(v) => match have.iter().find(|(n, _)| *n == v) {
            Some(x) => vec![x.clone()],
            None => {
                let vs: Vec<String> = have.iter().map(|(n, _)| format!("v{n}")).collect();
                return Err(TemplateError::new(
                    404,
                    "unknown_template",
                    format!("'{id}' has no v{v} (it has {})", vs.join(", ")),
                ));
            }
        },
        None => have.clone(),
    };
    if let Some(a) = only_author {
        if let Some((v, _)) = doomed.iter().find(|(_, d)| author_of(d).as_deref() != Some(a)) {
            return Err(TemplateError::new(
                403,
                "not_author",
                format!("{id} v{v} was saved by someone else; an agent removes only templates it saved. Ask a person to remove it"),
            ));
        }
    }
    let mut removed = Vec::new();
    for (v, d) in &doomed {
        std::fs::remove_dir_all(d).map_err(|e| internal(format!("{}: {e}", display_path(d))))?;
        removed.push(*v);
    }
    let remaining: Vec<u32> = have.iter().map(|(v, _)| *v).filter(|v| !removed.contains(v)).collect();
    if remaining.is_empty() {
        let id_dir = super::library_dir().join(id);
        let empty = std::fs::read_dir(&id_dir).map(|mut d| d.next().is_none()).unwrap_or(false);
        if empty {
            let _ = std::fs::remove_dir(&id_dir);
        }
    }
    Ok(serde_json::json!({ "ok": true, "id": id, "removed": removed, "remaining": remaining }))
}

/// The suggestion list a save writes: each standing action once, in order.
pub fn dedup(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.into_iter().filter(|i| seen.insert(i.clone())).collect()
}

#[cfg(test)]
#[path = "save_tests.rs"]
mod tests;
