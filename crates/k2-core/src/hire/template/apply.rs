//! `apply` (prd-agent-templates-v1 §3): one function, idempotent.
//!
//! Every step looks first and writes only what's missing. Nothing the
//! agent has (its ROLE.md, a note, a layer, a skill, a heartbeat, its tool)
//! is ever replaced. A dry run walks the same steps and writes nothing, so
//! its rows are the daemon's own truth for the review screen.
//!
//! Row statuses: `write` (dry run: would write), `done` (written), `skip`
//! (already there), `yours` (the agent's own file, kept), `suggest` (a
//! permission the template suggests; the permissions step, T3, turns it
//! into a grant or a card).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use super::super::converge::{self, ConvergeAuthority, ConvergeRequest};
use super::super::steps;
use super::super::{HireCaller, HireError};
use super::format::{sha256_text, Template};
use super::lock::{self, TemplateLock, LOCK_REL};
use super::render::render;

/// `apply`'s input. [`ApplyRequest::new`] fills the defaults.
#[derive(Debug, Clone, Default)]
pub struct ApplyRequest {
    /// Template id or alias (`writer`, `worker`, `k5`).
    pub template: String,
    /// Declared `[params]` (`about`); `person` overrides the hirer's name.
    pub params: BTreeMap<String, String>,
    /// The folder. `None`: `<hire root>/<handle>`.
    pub path: Option<String>,
    /// Display name; the handle seed when `handle` is unset.
    pub name: Option<String>,
    /// Folder / handle seed (`k5`).
    pub handle: Option<String>,
    /// Preset id. `None`: the template's `[agent] tool` (when not `default`).
    pub tool: Option<String>,
    pub model: Option<String>,
    pub dry_run: bool,
    /// The hire door's other fields (CLI `k2 agent hire`).
    pub db_access: Option<String>,
    pub no_wiki: bool,
    pub no_agents_md: bool,
    pub fanout: bool,
}

impl ApplyRequest {
    pub fn new(template: &str) -> Self {
        Self { template: template.to_string(), ..Default::default() }
    }
}

/// One step's line in the plan / report.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub step: &'static str,
    pub target: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn row(step: &'static str, target: impl Into<String>, status: &'static str, reason: Option<&str>) -> Row {
    Row { step, target: target.into(), status, reason: reason.map(str::to_string) }
}

/// The permissions part (T3 fills `granted` and `carded`; this slice only
/// reports `suggested`).
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPlan {
    /// The template turns on "Can ask for permission cards".
    pub cards: bool,
    pub suggested: Vec<String>,
    pub granted: Vec<String>,
    pub carded: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TemplateRef {
    pub id: String,
    pub version: u32,
    pub hash: String,
    pub kind: &'static str,
    pub name: String,
    pub origin: &'static str,
    pub author: String,
}

impl TemplateRef {
    pub fn of(t: &Template) -> Self {
        Self {
            id: t.meta.id.clone(),
            version: t.meta.version,
            hash: t.hash.clone(),
            kind: t.meta.kind.as_str(),
            name: t.meta.name.clone(),
            origin: t.origin.as_str(),
            author: t.meta.author.clone(),
        }
    }
}

/// The template's own steps on one workspace.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOutcome {
    pub template: TemplateRef,
    pub rows: Vec<Row>,
    pub changed: bool,
    /// ROLE.md was written by this call (`write-persona: done`).
    pub wrote_role: bool,
    /// The rendered charter (the K2:MANAGED block's body): the review's
    /// preview.
    pub charter: String,
    pub permissions: PermissionPlan,
    /// The rendered `[hello] brief`, only on the first create (the door
    /// launches the agent and delivers it).
    pub hello: Option<String>,
}

/// What [`apply`] returns.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub path: String,
    /// Empty in a dry run of a new folder.
    pub workspace_id: String,
    pub handle: String,
    pub name: String,
    /// Registered by this call (a dry run: would be).
    pub created: bool,
    pub changed: bool,
    pub dry_run: bool,
    pub template: TemplateRef,
    /// The converge steps (`create-dir`, `register-workspace`,
    /// `write-persona`, …), as `/cli/agents/hire` reports them.
    pub steps: Vec<converge::StepReport>,
    /// The template's rows (folder and register first).
    pub rows: Vec<Row>,
    pub charter: String,
    pub permissions: PermissionPlan,
    pub hello: Option<String>,
}

impl ApplyReport {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        v["ok"] = serde_json::json!(true);
        v
    }
}

/// The input to [`workspace_steps`].
pub struct StepInput<'a> {
    pub tpl: &'a Template,
    /// From [`super::bind_params`] (declared params + `person`).
    pub vars: &'a BTreeMap<String, String>,
    pub path: &'a str,
    pub caller: &'a HireCaller,
    /// `{{name}}` and the ROLE.md heading.
    pub display_name: String,
    /// This call registered the workspace (the hello goes out once).
    pub created: bool,
    /// False only in a dry run of a folder that isn't a workspace yet.
    pub registered: bool,
    pub dry_run: bool,
    pub no_wiki: bool,
    /// The hirer picked a tool / model: the template's suggestion stays out.
    pub tool_chosen: bool,
    pub model_chosen: bool,
}

/// One template per agent: a lock naming another id is a conflict.
pub fn check_lock(path: &str, tpl: &Template) -> Result<Option<TemplateLock>, HireError> {
    let l = lock::read(path)?;
    if let Some(l) = l.as_ref() {
        if l.id != tpl.meta.id {
            return Err(HireError::new(
                409,
                "template_conflict",
                format!(
                    "{path} was made from the '{}' template; an agent has one template (switching templates is not built yet)",
                    l.id
                ),
            ));
        }
    }
    Ok(l)
}

fn agent_type_for(role: &str) -> &'static str {
    // AT12: the lead keeps today's manager type; the rest are templates.
    if role == "lead" || role == "manager" { "manager" } else { "agent-template" }
}

/// The K2:MANAGED block's body: the charter, then the shared block.
pub fn charter(tpl: &Template, vars: &BTreeMap<String, String>) -> String {
    let mut out = render(tpl.role_md(), vars).trim_end().to_string();
    out.push('\n');
    if tpl.meta.shared_block {
        out.push('\n');
        out.push_str(render(super::AGENT_TEMPLATE_SHARED_BLOCK, vars).trim_end());
        out.push('\n');
    }
    out
}

/// The ROLE.md body under the generated frontmatter.
fn role_body(display_name: &str, charter: &str) -> String {
    format!(
        "# Persona — {display_name}\n\n{}\n{charter}{}\n\n## Your notes\n\n_Yours: K2 never changes anything below this line._\n",
        crate::skills::version::SKILL_BEGIN_MARKER,
        crate::skills::version::SKILL_END_MARKER,
    )
}

fn exists(path: &str, rel: &str) -> bool {
    Path::new(path).join(rel).exists()
}

/// The template's own steps on a workspace (converge has made and
/// registered it, unless this is a dry run of a new folder).
pub fn workspace_steps(inp: &StepInput<'_>) -> Result<WorkspaceOutcome, HireError> {
    let t = inp.tpl;
    let dry = inp.dry_run;
    let wrote: &'static str = if dry { "write" } else { "done" };
    let prev = check_lock(inp.path, t)?;
    let mut files: BTreeMap<String, String> = prev.as_ref().map(|l| l.files.clone()).unwrap_or_default();
    let mut rows = Vec::new();
    let mut vars = inp.vars.clone();
    vars.insert("name".into(), inp.display_name.clone());
    vars.entry("person".into()).or_insert_with(|| super::person_for(inp.caller));

    // ROLE.md: written only when the agent has none.
    let charter = charter(t, &vars);
    let role_rel = ".k2/agent/ROLE.md";
    let agent_dir = crate::workspace::agent_identity::workspace_agent_path(inp.path);
    let mut wrote_role = false;
    if crate::workspace::agent_identity::persona_present_in(&agent_dir) {
        if files.contains_key(role_rel) {
            rows.push(row("role", role_rel, "skip", Some("written by this template")));
        } else {
            rows.push(row("role", role_rel, "yours", Some("the agent's own ROLE.md; kept")));
        }
    } else {
        if !dry {
            let role = t.role();
            crate::workspace::agent::create(
                inp.path.to_string(),
                steps::technical_agent_name(inp.path),
                role.clone(),
                Some(role_body(&inp.display_name, &charter)),
                Some(agent_type_for(&role).to_string()),
            )
            .map_err(|e| HireError::internal(format!("persona: {e}")))?;
            files.insert(role_rel.into(), sha256_text(&charter));
            wrote_role = true;
        }
        rows.push(row("role", role_rel, wrote, None));
    }

    // Wiki starters: only the missing ones (AT3).
    let mut written_notes: Vec<String> = Vec::new();
    for (id, body) in t.wiki_notes() {
        let rel = format!(".k2/wiki/{id}");
        if inp.no_wiki {
            rows.push(row("wiki", rel, "skip", Some("--no-wiki")));
            continue;
        }
        if exists(inp.path, &rel) {
            rows.push(row("wiki", rel, "skip", Some("present")));
            continue;
        }
        let text = render(body, &vars);
        if !dry {
            crate::wiki::write_note(Path::new(inp.path), &id, &text).map_err(HireError::usage)?;
            files.insert(rel.clone(), sha256_text(&text));
        }
        written_notes.push(id.trim_end_matches(".md").to_string());
        rows.push(row("wiki", rel, wrote, None));
    }
    if !written_notes.is_empty() {
        let idx_rel = ".k2/wiki/_Index.md";
        let current = std::fs::read_to_string(Path::new(inp.path).join(idx_rel)).ok();
        let untouched = current.as_deref().is_none_or(|c| c == crate::wiki::SEED_INDEX_MD);
        if untouched {
            if !dry {
                let links = written_notes.iter().map(|n| format!("[[{n}]]")).collect::<Vec<_>>().join(" · ");
                let text = format!("{}- From the {} template: {links}\n", crate::wiki::SEED_INDEX_MD, t.meta.name);
                std::fs::write(Path::new(inp.path).join(idx_rel), &text)
                    .map_err(|e| HireError::internal(format!("{idx_rel}: {e}")))?;
                files.insert(idx_rel.into(), sha256_text(&text));
            }
            rows.push(row("wiki-index", idx_rel, wrote, None));
        } else {
            rows.push(row("wiki-index", idx_rel, "skip", Some("edited by the agent; left alone")));
        }
    }

    // Catalog layers: a duplicate is a no-op.
    let stacked: Vec<(String, String)> = if inp.registered {
        crate::workspace::context_layers::list_layers(inp.path)
            .map_err(steps::ctx_err)?
            .into_iter()
            .map(|l| (l.source, l.path))
            .collect()
    } else {
        Vec::new()
    };
    for id in &t.meta.context.catalog {
        let e = crate::workspace::context_layers::resolve_catalog_entry(id).map_err(steps::ctx_err)?;
        if stacked.iter().any(|(s, p)| *s == e.source || *p == e.path) {
            rows.push(row("catalog", id.clone(), "skip", Some("stacked")));
            continue;
        }
        if dry {
            rows.push(row("catalog", id.clone(), "write", None));
            continue;
        }
        let added = steps::add_context(inp.path, std::slice::from_ref(id))?;
        rows.push(row("catalog", id.clone(), if added { "done" } else { "skip" }, (!added).then_some("stacked")));
    }

    // The template's own layers: written only if missing (AT2).
    for (label, md) in t.layers() {
        let rel = format!(".k2/context/{label}.md");
        if exists(inp.path, &rel) {
            rows.push(row("layer", rel, "skip", Some("present")));
            continue;
        }
        let text = render(md, &vars);
        if !dry {
            steps::add_inline_layer(inp.path, &label, &text)?;
            files.insert(rel.clone(), sha256_text(&text));
        }
        rows.push(row("layer", rel, wrote, None));
    }

    // Skills: an existing name is skipped (AT21).
    for (name, body) in t.skills() {
        let dir = crate::workspace::agent_identity::skill_dir(inp.path, &name);
        let rel = dir
            .strip_prefix(inp.path)
            .map(|p| format!("{}/SKILL.md", p.to_string_lossy()))
            .unwrap_or_else(|_| format!(".k2/skills/{name}/SKILL.md"));
        if dir.exists() {
            rows.push(row("skill", name, "skip", Some("a skill with this name exists")));
            continue;
        }
        let text = render(body, &vars);
        if !dry {
            steps::write_skills(inp.path, &[super::super::recipe::SkillIn { name: name.clone(), body: text.clone() }])?;
            files.insert(rel, sha256_text(&text));
        }
        rows.push(row("skill", name, wrote, None));
    }

    // Heartbeats: an existing name is skipped (AT20).
    let have: Vec<String> = if inp.registered {
        crate::heartbeats::k2so_heartbeat_list(inp.path.to_string())
            .map_err(|e| HireError::internal(format!("heartbeats: {e}")))?
            .into_iter()
            .map(|h| h.name)
            .collect()
    } else {
        Vec::new()
    };
    for hb in &t.meta.heartbeats {
        if have.contains(&hb.name) {
            rows.push(row("heartbeat", hb.name.clone(), "skip", Some("a heartbeat with this name exists")));
            continue;
        }
        if !dry {
            let mut hb = hb.clone();
            hb.instructions = hb.instructions.as_deref().map(|i| render(i, &vars));
            steps::add_heartbeats(inp.path, &[hb])?;
        }
        rows.push(row("heartbeat", hb.name.clone(), wrote, None));
    }

    // Tool and model: the template's suggestion only when the hirer chose
    // none and the workspace has none (AT15).
    for (step, field, chosen, want) in [
        ("tool", "default_agent", inp.tool_chosen, t.meta.agent.tool.as_deref()),
        ("model", "default_model", inp.model_chosen, t.meta.agent.model.as_deref()),
    ] {
        let want = want.map(str::trim).filter(|s| !s.is_empty() && *s != "default");
        let Some(want) = want else {
            if step == "tool" && !chosen {
                rows.push(row(step, "default", "skip", Some("the server default")));
            }
            continue;
        };
        if chosen {
            rows.push(row(step, want, "skip", Some("the hirer chose one")));
            continue;
        }
        let current = if inp.registered { steps::current_setting(inp.path, field) } else { None };
        if current.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            rows.push(row(step, want, "skip", Some("already set")));
            continue;
        }
        if !dry {
            steps::set_setting(inp.path, field, want)?;
        }
        rows.push(row(step, want, wrote, None));
    }

    // Permissions: suggestions only in this slice (T3 decides).
    let permissions = PermissionPlan {
        cards: t.meta.permissions.cards,
        suggested: t.suggested(),
        granted: prev.as_ref().map(|l| l.granted.clone()).unwrap_or_default(),
        carded: prev.as_ref().map(|l| l.carded.clone()).unwrap_or_default(),
    };
    if permissions.cards {
        rows.push(row("permission", "permission-cards", "suggest", None));
    }
    for id in &permissions.suggested {
        rows.push(row("permission", id.clone(), "suggest", None));
    }

    // The lock: rewritten only when what it records changed.
    let next = TemplateLock {
        id: t.meta.id.clone(),
        version: t.meta.version,
        hash: t.hash.clone(),
        applied_at: super::super::now_secs(),
        by: inp.caller.actor(),
        catalog: t.meta.context.catalog.clone(),
        suggested: permissions.suggested.clone(),
        granted: permissions.granted.clone(),
        carded: permissions.carded.clone(),
        files,
    };
    let lock_changed = prev.as_ref().is_none_or(|p| !p.same_content(&next));
    let wrote_something = rows.iter().any(|r| r.status == "done");
    if lock_changed || (wrote_something && !dry) {
        if !dry {
            lock::write(inp.path, &next)?;
        }
        rows.push(row("lock", LOCK_REL, wrote, prev.is_some().then_some("updated")));
    } else {
        rows.push(row("lock", LOCK_REL, "skip", Some("up to date")));
    }

    // Hello: on the first create only.
    let hello = match t.meta.hello.as_ref() {
        Some(h) if inp.created => {
            rows.push(row("hello", "thread", wrote, None));
            Some(render(&h.brief, &vars).trim().to_string()).filter(|s| !s.is_empty())
        }
        Some(_) => {
            rows.push(row("hello", "thread", "skip", Some("sent on the first create only")));
            None
        }
        None => None,
    };

    let changed = rows.iter().any(|r| r.status == "done");
    Ok(WorkspaceOutcome {
        template: TemplateRef::of(t),
        rows,
        changed,
        wrote_role,
        charter,
        permissions,
        hello: if dry { None } else { hello },
    })
}

fn folder_free(path: &str, tpl: &Template) -> Result<(), HireError> {
    let p = Path::new(path);
    if !p.exists() {
        return Ok(());
    }
    if let Some(l) = lock::read(path)? {
        if l.id == tpl.meta.id {
            return Ok(()); // a re-run of this same hire
        }
    }
    let empty = std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(false);
    if empty && converge::lookup_project_row(path).is_none() {
        return Ok(());
    }
    Err(HireError::new(
        409,
        "folder_exists",
        format!("{path} already exists; pass it as <dir> to make it a {} agent, or pick another --name", tpl.meta.name),
    ))
}

/// Where a hire with no `<dir>` goes: `<root>/<slug of handle|name|id>`.
fn default_path(req: &ApplyRequest, tpl: &Template, auth: &ConvergeAuthority) -> Result<String, HireError> {
    let seed = req
        .handle
        .as_deref()
        .or(req.name.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&tpl.meta.id);
    let slug = crate::workspace_session_handles::slugify_address_token(seed)
        .map_err(|e| HireError::usage(format!("handle '{seed}': {e}")))?;
    let root = match auth.new_workspace_root.clone() {
        Some(r) => r,
        None => super::super::root::hire_root()?,
    };
    let path = root.join(&slug).to_string_lossy().into_owned();
    folder_free(&path, tpl)?;
    Ok(path)
}

/// Apply with no extra limits (the owner, a person, an agent on its own
/// box). See [`apply_with`].
pub fn apply(req: &ApplyRequest, caller: &HireCaller) -> Result<ApplyReport, HireError> {
    apply_with(req, caller, &ConvergeAuthority::unrestricted())
}

/// The one front door (§3): resolve the template, check the caller and the
/// params, pick the folder, then converge (folder, register, wiki seed, the
/// template's steps, name, tool, DB access). A dry run writes nothing.
pub fn apply_with(req: &ApplyRequest, caller: &HireCaller, auth: &ConvergeAuthority) -> Result<ApplyReport, HireError> {
    let tpl = super::resolve(&req.template)?;
    super::check_caller(&tpl, caller)?;
    let vars = super::bind_params(&tpl, &req.params, caller)?;
    let path = match req.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => converge::expand_hire_path(p)?,
        None => default_path(req, &tpl, auth)?,
    };
    check_lock(&path, &tpl)?;
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);

    if req.dry_run {
        let existing = converge::lookup_project_row(&path);
        let basename = Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let display = name.clone().unwrap_or_else(|| match &existing {
            Some(_) => {
                let d = crate::workspace::display::agent_display_name(&path);
                if d.is_empty() { basename.clone() } else { d }
            }
            None => basename.clone(),
        });
        let mut rows = vec![
            if Path::new(&path).exists() {
                row("folder", path.clone(), "skip", Some("exists"))
            } else {
                row("folder", path.clone(), "write", None)
            },
            if existing.is_some() {
                row("register", path.clone(), "skip", Some("already registered"))
            } else {
                row("register", path.clone(), "write", None)
            },
        ];
        let out = workspace_steps(&StepInput {
            tpl: &tpl,
            vars: &vars,
            path: &path,
            caller,
            display_name: display.clone(),
            created: existing.is_none(),
            registered: existing.is_some(),
            dry_run: true,
            no_wiki: req.no_wiki,
            tool_chosen: req.tool.as_deref().is_some_and(|s| !s.trim().is_empty()),
            model_chosen: req.model.as_deref().is_some_and(|s| !s.trim().is_empty()),
        })?;
        rows.extend(out.rows);
        let (ws_id, handle) = match &existing {
            Some((_, r)) => (r.id.clone(), r.handle.clone().unwrap_or_else(|| basename.clone())),
            None => (String::new(), basename.clone()),
        };
        return Ok(ApplyReport {
            path,
            workspace_id: ws_id,
            handle,
            name: display,
            created: existing.is_none(),
            changed: false,
            dry_run: true,
            template: out.template,
            steps: Vec::new(),
            rows,
            charter: out.charter,
            permissions: out.permissions,
            hello: None,
        });
    }

    let creq = ConvergeRequest {
        path: path.clone(),
        name,
        preset: req.tool.clone(),
        template: Some(tpl.meta.id.clone()),
        params: Some(req.params.clone()),
        default_model: req.model.clone(),
        db_access: req.db_access.clone(),
        no_wiki: Some(req.no_wiki),
        no_agents_md: Some(req.no_agents_md),
        fanout: Some(req.fanout),
        ..Default::default()
    };
    let out = converge::converge(&creq, caller, auth)?;
    let applied = out
        .template
        .clone()
        .ok_or_else(|| HireError::internal("converge ran without the template step"))?;
    let mut rows = vec![
        row("folder", out.path.clone(), if out.steps.iter().any(|s| s.step == "create-dir" && s.status == "done") { "done" } else { "skip" }, None),
        row("register", out.path.clone(), if out.created { "done" } else { "skip" }, None),
    ];
    rows.extend(applied.rows);
    Ok(ApplyReport {
        path: out.path,
        workspace_id: out.id,
        handle: out.handle,
        name: out.name,
        created: out.created,
        changed: out.changed,
        dry_run: false,
        template: applied.template,
        steps: out.steps,
        rows,
        charter: applied.charter,
        permissions: applied.permissions,
        hello: applied.hello,
    })
}
