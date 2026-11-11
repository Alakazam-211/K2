//! `hire::template::apply`: make an agent from a built-in template
//! (prd-agent-templates-v1 §3). **This is a shim** over [`super::converge`]
//! for the two system-shaped templates K5 at first run needs, `k5` and
//! `chief-of-staff` (prd-setup-checklist-v1 §10, C7/C10). K2's templates
//! slice T1/T2 replaces it with the real loader and apply, keeping this
//! signature ([`ApplyRequest`], [`apply`], [`ApplyReport`]), so nothing
//! that calls it changes.
//!
//! What the shim does, in order:
//! 1. reads the template folder (`crates/k2-core/agent-templates/<id>/`,
//!    embedded), parsing only the `template.toml` keys it needs;
//! 2. checks the params against `[params]` and fills `{{name}}`,
//!    `{{person}}` and the declared params into `ROLE.md` and `[hello]`;
//! 3. converges the folder (`<hire root>/<handle>` unless a path is given):
//!    create or open, register, seed the wiki;
//! 4. writes ROLE.md as frontmatter, then the charter in a `K2:MANAGED`
//!    block, then an empty "Your notes" part. An existing ROLE.md is never
//!    replaced;
//! 5. sets the tool (`default_agent`) on create, or when unset;
//! 6. stacks the `[context] catalog` ids. An id this K2 doesn't know yet
//!    (`k5:pack` arrives with T4) is skipped with a log line, not an error.
//!
//! It **grants nothing**: `permissions.suggested` lists what the template
//! suggests, and the first-run routes grant the starter sets themselves
//! (`k2-daemon` `first_run_grants`). `hello` is the rendered `[hello].brief`,
//! `Some` only on first create; the caller launches the agent and delivers
//! it. No `.k2/agent/template.lock` yet (T2).

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::converge::{self, ConvergeAuthority, ConvergeRequest};
use super::{HireCaller, HireError};

/// One apply request (the agent-templates API, T2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyRequest {
    /// Template id or alias: `k5`, `chief-of-staff`.
    pub template: String,
    /// Declared `[params]` (`about`). `person` overrides the hirer's name.
    pub params: BTreeMap<String, String>,
    /// `None`: `<hire root>/<handle>`.
    pub path: Option<String>,
    /// Display name; seeds the handle when `handle` is `None`.
    pub name: Option<String>,
    /// Folder and handle seed (`k5`).
    pub handle: Option<String>,
    /// Preset id; `None`: the template's `[agent].tool` (`default` leaves it unset).
    pub tool: Option<String>,
    pub model: Option<String>,
    pub dry_run: bool,
}

impl ApplyRequest {
    pub fn new(template: &str) -> Self {
        Self { template: template.to_string(), ..Self::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TemplateRef {
    pub id: String,
    pub version: i64,
    pub hash: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ApplyStep {
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PermissionsReport {
    pub cards: bool,
    pub suggested: Vec<String>,
    pub granted: Vec<String>,
    pub carded: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub path: String,
    pub workspace_id: String,
    pub handle: String,
    pub name: String,
    pub created: bool,
    pub changed: bool,
    pub dry_run: bool,
    pub template: TemplateRef,
    pub steps: Vec<ApplyStep>,
    pub permissions: PermissionsReport,
    pub hello: Option<String>,
}

/// One built-in template's files.
struct Builtin {
    id: &'static str,
    toml: &'static str,
    role: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "k5",
        toml: include_str!("../../agent-templates/k5/template.toml"),
        role: include_str!("../../agent-templates/k5/ROLE.md"),
    },
    Builtin {
        id: "chief-of-staff",
        toml: include_str!("../../agent-templates/chief-of-staff/template.toml"),
        role: include_str!("../../agent-templates/chief-of-staff/ROLE.md"),
    },
];

/// The shared "Working with your person and the team" block
/// (prd-agent-templates-v1 §5), appended when `shared_block = true`.
const SHARED_BLOCK: &str = "## Working with your person and the team

- Your person is {{person}}. Talk with them on the Thread. Ask when something is unclear, and say plainly when you're blocked.
- Report what you did and how you checked it.
- Connected agents are peers: reach them with `k2 msg`, and never answer another agent's prompt.
- When K2 refuses you, ask with the card it prints; don't work around a refusal.
- Never put secrets in chat or the wiki.
- Keep lasting notes in your wiki so a new session can pick up.
";

const BEGIN: &str = crate::skills::version::SKILL_BEGIN_MARKER;
const END: &str = crate::skills::version::SKILL_END_MARKER;

/// What the shim reads of `template.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub id: String,
    pub version: i64,
    pub name: String,
    pub kind: String,
    pub shared_block: bool,
    pub role: String,
    pub tool: String,
    pub catalog: Vec<String>,
    pub cards: bool,
    pub suggest: Vec<String>,
    /// `(key, max, required)`.
    pub params: Vec<(String, usize, bool)>,
    pub hello: Option<String>,
}

fn bad(e: impl std::fmt::Display) -> HireError {
    HireError::internal(format!("built-in template: {e}"))
}

/// Parse one `template.toml` (the keys the shim uses; T1's loader checks
/// the rest).
pub fn parse(raw: &str) -> Result<Parsed, HireError> {
    let v: toml::Value = toml::from_str(raw).map_err(bad)?;
    let s = |t: &toml::Value, k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string);
    let list = |t: Option<&toml::Value>, k: &str| -> Vec<String> {
        t.and_then(|t| t.get(k))
            .and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(toml::Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let agent = v.get("agent");
    let perms = v.get("permissions");
    let mut params = Vec::new();
    if let Some(t) = v.get("params").and_then(toml::Value::as_table) {
        for (k, spec) in t {
            let max = spec.get("max").and_then(toml::Value::as_integer).unwrap_or(500).max(0) as usize;
            let required = spec.get("required").and_then(toml::Value::as_bool).unwrap_or(false);
            params.push((k.clone(), max, required));
        }
    }
    Ok(Parsed {
        id: s(&v, "id").ok_or_else(|| bad("no id"))?,
        version: v.get("version").and_then(toml::Value::as_integer).ok_or_else(|| bad("no version"))?,
        name: s(&v, "name").ok_or_else(|| bad("no name"))?,
        kind: s(&v, "kind").unwrap_or_else(|| "agent".into()),
        shared_block: v.get("shared_block").and_then(toml::Value::as_bool).unwrap_or(false),
        role: agent.and_then(|a| s(a, "role")).unwrap_or_else(|| "agent".into()),
        tool: agent.and_then(|a| s(a, "tool")).unwrap_or_else(|| "default".into()),
        catalog: list(v.get("context"), "catalog"),
        cards: perms.and_then(|p| p.get("cards")).and_then(toml::Value::as_bool).unwrap_or(false),
        suggest: list(perms, "suggest"),
        params,
        hello: v.get("hello").and_then(|h| s(h, "brief")),
    })
}

fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id.trim())
}

/// The built-in ids this shim knows.
pub fn builtin_ids() -> Vec<&'static str> {
    BUILTINS.iter().map(|b| b.id).collect()
}

/// sha256 over the sorted file paths and bytes (prd-agent-templates-v1 §2.3).
fn hash(b: &Builtin) -> String {
    let mut h = Sha256::new();
    for (path, bytes) in [("ROLE.md", b.role), ("template.toml", b.toml)] {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(bytes.as_bytes());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Fill `{{key}}` from `values`. Any other `{{…}}` is an error (a template
/// bug, caught by the built-in test). A line whose placeholder is empty is
/// dropped (an optional param left out), as the T2 loader does.
pub fn render(text: &str, values: &BTreeMap<String, String>) -> Result<String, HireError> {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let mut rendered = String::with_capacity(line.len());
        let mut rest = line;
        let mut empty = false;
        while let Some(i) = rest.find("{{") {
            rendered.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            let j = after.find("}}").ok_or_else(|| bad("an unclosed {{"))?;
            let key = after[..j].trim();
            let val = values.get(key).ok_or_else(|| bad(format!("unknown placeholder {{{{{key}}}}}")))?;
            empty |= val.trim().is_empty();
            rendered.push_str(val);
            rest = &after[j + 2..];
        }
        rendered.push_str(rest);
        if !empty {
            out.push_str(&rendered);
        }
    }
    Ok(out)
}

/// The hirer's display name, for `{{person}}`.
fn person_for(caller: &HireCaller) -> String {
    let people = crate::notify_prefs::People::load().unwrap_or(crate::notify_prefs::People { users: Vec::new(), linked: Vec::new() });
    match caller {
        HireCaller::Login { username } => crate::person_name::effective(username, &people).display_name,
        HireCaller::ApiKey { label: Some(l), .. } => l.clone(),
        // The owner token, and an agent hiring for its person (on a
        // single-owner box, the owner).
        _ => crate::person_name::effective(crate::notify_prefs::OWNER, &people).display_name,
    }
}

/// The values every placeholder is filled from: the declared params (with
/// limits), `{{name}}` and `{{person}}`.
fn values(p: &Parsed, req: &ApplyRequest, name: &str, caller: &HireCaller) -> Result<BTreeMap<String, String>, HireError> {
    let mut out = BTreeMap::new();
    for (key, val) in &req.params {
        if key == "person" || key == "name" {
            continue;
        }
        let Some((_, max, _)) = p.params.iter().find(|(k, _, _)| k == key) else {
            return Err(HireError::usage(format!("the {} template takes no '{key}'", p.id)));
        };
        let val = val.trim();
        if val.chars().count() > *max {
            return Err(HireError::usage(format!("'{key}' is at most {max} characters")));
        }
        out.insert(key.clone(), val.to_string());
    }
    for (key, _, required) in &p.params {
        if *required && out.get(key).is_none_or(|v| v.is_empty()) {
            return Err(HireError::usage(format!("the {} template needs '{key}'", p.id)));
        }
        out.entry(key.clone()).or_default();
    }
    out.insert("name".into(), name.to_string());
    let person = req.params.get("person").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    out.insert("person".into(), person.unwrap_or_else(|| person_for(caller)));
    Ok(out)
}

/// The ROLE.md body (frontmatter is the persona writer's): the heading, the
/// charter in a `K2:MANAGED` block, then the person's own part (T2's shape).
pub fn role_body(name: &str, charter: &str) -> String {
    format!("# Persona — {name}\n\n{BEGIN}\n{}\n{END}\n\n## Your notes\n\n", charter.trim_end())
}

fn step(step: &str, target: Option<&str>, status: &str, reason: Option<String>) -> ApplyStep {
    ApplyStep { step: step.into(), target: target.map(str::to_string), status: status.into(), reason }
}

fn default_agent_of(path: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT default_agent FROM projects WHERE path = ?1", rusqlite::params![path], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

/// Make (or converge) one agent from template `req.template`.
pub fn apply(req: &ApplyRequest, caller: &HireCaller) -> Result<ApplyReport, HireError> {
    let b = builtin(&req.template).ok_or_else(|| {
        HireError::new(404, "unknown_template", format!("no template '{}' (built-in: {})", req.template.trim(), builtin_ids().join(", ")))
    })?;
    let p = parse(b.toml)?;
    if p.kind == "system" && matches!(caller, HireCaller::ApiKey { .. }) {
        return Err(HireError::new(403, "system_template", format!("an API key can't hire the {} template", p.id)));
    }
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&p.name).to_string();
    let handle_seed = req.handle.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&name);
    let handle = crate::workspace_session_handles::slugify_address_token(handle_seed).map_err(HireError::usage)?;
    let path = match req.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => super::root::hire_root()?.join(&handle).to_string_lossy().into_owned(),
    };
    let values = values(&p, req, &name, caller)?;
    let mut charter = render(b.role, &values)?;
    if p.shared_block {
        charter = format!("{}\n\n{}", charter.trim_end(), render(SHARED_BLOCK, &values)?);
    }
    let hello = p.hello.as_deref().map(|h| render(h, &values)).transpose()?;
    let template = TemplateRef { id: p.id.clone(), version: p.version, hash: hash(b), kind: p.kind.clone() };
    let mut suggested = Vec::new();
    for s in &p.suggest {
        match s.strip_prefix("set:") {
            Some(set) => suggested.extend(
                super::starter_permissions::named_set(set)
                    .ok_or_else(|| bad(format!("unknown permission set {set}")))?
                    .iter()
                    .map(|s| s.to_string()),
            ),
            None => suggested.push(s.clone()),
        }
    }
    let permissions = PermissionsReport { cards: p.cards, suggested, ..PermissionsReport::default() };
    let tool = req
        .tool
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| (p.tool != "default").then(|| p.tool.clone()));

    let existing = converge::lookup_project_row(&converge::expand_hire_path(&path)?);
    // No path given: `<hire root>/<handle>` must be new, or an agent this
    // template already made (T2 checks its lock; the shim, registration).
    if req.path.as_deref().map(str::trim).is_none_or(str::is_empty) && existing.is_none() && Path::new(&path).exists() {
        return Err(HireError::new(
            409,
            "folder_exists",
            format!("{path} already exists and isn't a {} agent: pick another name or handle", p.name),
        ));
    }
    if req.dry_run {
        return Ok(ApplyReport {
            path,
            workspace_id: existing.map(|(_, r)| r.id).unwrap_or_default(),
            handle,
            name,
            created: false,
            changed: false,
            dry_run: true,
            template,
            steps: vec![step("converge", None, "pending", None)],
            permissions,
            hello: None,
        });
    }
    // The tool is set on create, or while the workspace has none.
    let set_tool = existing.as_ref().is_none_or(|(p, _)| default_agent_of(p).is_none());
    let creq = ConvergeRequest {
        path: path.clone(),
        name: Some(name.clone()),
        preset: if set_tool { tool.clone() } else { None },
        persona: Some(role_body(&name, &charter)),
        role: Some(p.role.clone()),
        default_model: if existing.is_none() { req.model.clone() } else { None },
        ..ConvergeRequest::default()
    };
    let out = converge::converge(&creq, caller, &ConvergeAuthority::unrestricted())?;
    let mut steps: Vec<ApplyStep> =
        out.steps.iter().map(|s| step(s.step, None, s.status, s.reason.clone())).collect();
    let mut changed = out.changed;
    for id in &p.catalog {
        match crate::workspace::context_layers::add_layer(&out.path, None, Some(id), None) {
            Ok(_) => {
                changed = true;
                steps.push(step("catalog", Some(id), "done", None));
            }
            Err(crate::workspace::context_layers::ContextError::DuplicateLayer(_)) => {
                steps.push(step("catalog", Some(id), "skipped", Some("already stacked".into())));
            }
            Err(crate::workspace::context_layers::ContextError::CatalogUnknown(_)) => {
                // `k5:pack` lands with K2's templates slice T4; until then
                // the agent goes without it rather than not at all.
                crate::log_debug!("[hire/template] {}: catalog id {id} is not on this K2 yet; skipped", p.id);
                steps.push(step("catalog", Some(id), "skipped", Some("not on this K2 yet".into())));
            }
            Err(e) => return Err(HireError::new(400, e.code(), e.hint().to_string())),
        }
    }
    Ok(ApplyReport {
        path: out.path,
        workspace_id: out.id,
        handle: out.handle,
        name: out.name,
        created: out.created,
        changed,
        dry_run: false,
        template,
        steps,
        permissions,
        hello: if out.created { hello } else { None },
    })
}

/// True when `path` already holds a ROLE.md (a template never replaces one).
pub fn has_persona(path: &str) -> bool {
    crate::workspace::agent_identity::persona_present_in(&crate::workspace::agent_identity::workspace_agent_path(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses_and_renders_with_only_known_placeholders() {
        for b in BUILTINS {
            let p = parse(b.toml).expect("parse");
            assert_eq!(p.id, b.id);
            assert_eq!(p.version, 1);
            let mut vals: BTreeMap<String, String> =
                p.params.iter().map(|(k, _, _)| (k.clone(), format!("<{k}>"))).collect();
            vals.insert("name".into(), "<name>".into());
            vals.insert("person".into(), "<person>".into());
            let role = render(b.role, &vals).unwrap_or_else(|e| panic!("{}: {e}", b.id));
            assert!(!role.contains("{{"), "{}: {role}", b.id);
            assert!(!b.role.starts_with("---"), "{}: ROLE.md is the charter body only, no frontmatter", b.id);
            render(SHARED_BLOCK, &vals).expect("shared block");
            let hello = p.hello.as_deref().expect("a hello brief");
            render(hello, &vals).unwrap_or_else(|e| panic!("{} hello: {e}", b.id));
            assert!(p.cards, "{}: cards on", b.id);
            for s in &p.suggest {
                let set = s.strip_prefix("set:").expect("a named set");
                assert!(super::super::starter_permissions::named_set(set).is_some(), "{set}");
            }
        }
        let k5 = parse(builtin("k5").expect("k5").toml).expect("k5");
        assert_eq!(k5.kind, "system");
        assert_eq!(k5.catalog, vec!["k5:pack", "users:roster", "heartbeats:roster"]);
        let cos = parse(builtin("chief-of-staff").expect("cos").toml).expect("cos");
        assert_eq!(cos.kind, "agent");
        assert!(cos.params.iter().any(|(k, max, req)| k == "about" && *max == 500 && !req));
    }

    #[test]
    fn render_refuses_an_unknown_placeholder() {
        let vals: BTreeMap<String, String> = [("name".to_string(), "K5".to_string())].into();
        assert_eq!(render("hi {{name}}!", &vals).expect("ok"), "hi K5!");
        assert!(render("hi {{nope}}", &vals).is_err());
        assert!(render("hi {{name", &vals).is_err());
        let with_empty: BTreeMap<String, String> = [("name".to_string(), "K5".to_string()), ("about".to_string(), String::new())].into();
        assert_eq!(render("a {{name}}\nWhat they do: {{about}}\nb\n", &with_empty).expect("ok"), "a K5\nb\n", "an empty param drops its line");
    }

    #[test]
    fn the_role_body_is_a_managed_block_then_the_persons_part() {
        let body = role_body("K5", "charter\n");
        assert!(body.starts_with("# Persona — K5\n\n"), "{body}");
        assert!(body.contains(BEGIN), "{body}");
        let end = body.find(END).expect("end marker");
        assert!(body[..end].contains("charter"));
        assert!(body[end..].contains("## Your notes"), "{body}");
    }

    #[test]
    fn params_are_checked_against_the_template() {
        let p = parse(builtin("chief-of-staff").expect("cos").toml).expect("cos");
        let mut req = ApplyRequest::new("chief-of-staff");
        req.params.insert("about".into(), "x".repeat(501));
        assert_eq!(values(&p, &req, "Max", &HireCaller::Owner).expect_err("too long").code, "usage");
        req.params.insert("about".into(), "Runs a bakery".into());
        req.params.insert("person".into(), "Rosa".into());
        let v = values(&p, &req, "Max", &HireCaller::Owner).expect("ok");
        assert_eq!((v["about"].as_str(), v["person"].as_str(), v["name"].as_str(), v["k5"].as_str()), ("Runs a bakery", "Rosa", "Max", ""));
        req.params.insert("color".into(), "red".into());
        assert_eq!(values(&p, &req, "Max", &HireCaller::Owner).expect_err("undeclared").code, "usage");
    }

    /// End to end on a temp home: the folder under the hire root, ROLE.md
    /// as frontmatter + managed charter + the person's part, the name and
    /// the tool, the known catalog ids stacked and `k5:pack` skipped, the
    /// hello on create only, and a re-apply that keeps the person's edits.
    #[test]
    fn applying_k5_makes_the_agent_once_and_never_replaces_its_role() {
        let _home = crate::test_env::TempHome::new();
        let _db = crate::db::scoped_for_test();
        let mut req = ApplyRequest::new("k5");
        req.handle = Some("k5".into());
        req.tool = Some("codex".into());
        req.params.insert("person".into(), "Rosa".into());
        let out = apply(&req, &HireCaller::Owner).expect("apply");
        let root = super::super::root::hire_root().expect("root");
        assert!(out.created && out.changed, "{out:?}");
        assert!(Path::new(&out.path).starts_with(&root), "{} under {}", out.path, root.display());
        assert_eq!(out.name, "K5");
        assert_eq!(out.template.id, "k5");
        assert_eq!(out.template.kind, "system");
        let hello = out.hello.as_deref().expect("hello on create");
        assert!(hello.contains("Rosa") && !hello.contains("{{"), "{hello}");
        assert!(out.permissions.cards);
        assert_eq!(out.permissions.suggested, super::super::starter_permissions::K5_STARTER_PERMISSIONS);
        assert!(out.permissions.granted.is_empty(), "the shim grants nothing");
        assert_eq!(default_agent_of(&out.path).as_deref(), Some("codex"));

        let role_path = crate::workspace::agent_identity::persona_md_in(crate::workspace::agent_identity::workspace_agent_path(&out.path));
        let role = std::fs::read_to_string(&role_path).expect("ROLE.md");
        assert!(role.starts_with("---\n"), "frontmatter first: {role}");
        let begin = role.find(BEGIN).expect("managed begin");
        let end = role.find(END).expect("managed end");
        assert!(begin < end);
        assert!(role[begin..end].contains("You are K5, K2's own agent"), "{role}");
        assert!(role[begin..end].contains("Rosa"), "{{{{person}}}} filled: {role}");
        assert!(role[end..].contains("## Your notes"), "{role}");
        assert!(!role.contains("{{"), "{role}");

        let catalog: Vec<(&str, &str)> = out
            .steps
            .iter()
            .filter(|s| s.step == "catalog")
            .map(|s| (s.target.as_deref().unwrap_or(""), s.status.as_str()))
            .collect();
        assert_eq!(catalog, vec![("k5:pack", "skipped"), ("users:roster", "done"), ("heartbeats:roster", "done")]);

        // The person edits their part; a re-apply changes nothing of it,
        // sends no hello and keeps the tool it has.
        std::fs::write(&role_path, format!("{role}my own line\n")).expect("edit");
        let mut again = req.clone();
        again.tool = Some("grok".into());
        let out2 = apply(&again, &HireCaller::Owner).expect("re-apply");
        assert!(!out2.created);
        assert_eq!(out2.workspace_id, out.workspace_id);
        assert_eq!(out2.hello, None);
        assert!(std::fs::read_to_string(&role_path).expect("ROLE.md").ends_with("my own line\n"));
        assert_eq!(default_agent_of(&out.path).as_deref(), Some("codex"), "the tool is set once");

        // A folder already there that isn't this template's agent: 409.
        std::fs::create_dir_all(root.join("taken")).expect("mkdir");
        let mut taken = ApplyRequest::new("k5");
        taken.handle = Some("taken".into());
        assert_eq!(apply(&taken, &HireCaller::Owner).expect_err("taken").code, "folder_exists");
    }

    #[test]
    fn an_api_key_cant_hire_a_system_template_and_an_unknown_id_is_404() {
        let key = HireCaller::ApiKey { id: "k".into(), label: None };
        assert_eq!(apply(&ApplyRequest::new("k5"), &key).expect_err("system").code, "system_template");
        assert_eq!(apply(&ApplyRequest::new("nope"), &HireCaller::Owner).expect_err("unknown").status, 404);
    }
}
