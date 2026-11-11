//! `template.toml` (schema 1) and the whole-folder validator
//! (prd-agent-templates-v1 §2.2–§2.3, AT4, AT6, AT18). Unknown keys fail
//! closed, like the recipe: a field this daemon doesn't understand never
//! half-applies.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::super::recipe::{self, HeartbeatIn, InlineLayer, MemberRecipe, SkillIn, WikiNote};
use super::render;

pub const SCHEMA: u32 = 1;
/// Params a template may declare (each one a `{{name}}` in its text).
pub const MAX_PARAMS: usize = 8;
pub const MAX_PARAM_CHARS: usize = 4096;
pub const DEFAULT_PARAM_MAX: usize = 500;
/// Placeholders every template has. `open_questions` is filled at apply:
/// the asks of the `[[questions]]` the hire left unanswered (empty when
/// none are open, so its line drops).
pub const BUILT_IN_VARS: &[&str] = &["name", "person", "open_questions"];
/// First-conversation questions a template may ask (`[[questions]]`).
pub const MAX_QUESTIONS: usize = 8;
pub const MAX_ASK_CHARS: usize = 200;
/// An answer given at hire (`--answer id=text`).
pub const MAX_ANSWER_CHARS: usize = 1000;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Shown in the picker.
    #[default]
    Agent,
    /// `k5`, `chief-of-staff`: hired by id only, never by an API key.
    System,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::System => "system",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentSection {
    /// ROLE.md frontmatter `role:`; `lead` makes the agent a manager (AT12).
    #[serde(default)]
    pub role: Option<String>,
    /// A preset id, or `default` (the server default). A suggestion: the
    /// hirer's pick wins.
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextSection {
    /// Context Catalog ids (`wiki:index`, `k5:pack`).
    #[serde(default)]
    pub catalog: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PermissionsSection {
    /// Turn on "Can ask for permission cards" for the hire.
    #[serde(default)]
    pub cards: bool,
    /// Standing V1 action ids, or `set:<name>` (a named starter set).
    /// Suggestions only: the hirer's reach decides (T3).
    #[serde(default)]
    pub suggest: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    pub label: String,
    #[serde(default = "default_param_max")]
    pub max: usize,
    #[serde(default)]
    pub required: bool,
}

fn default_param_max() -> usize {
    DEFAULT_PARAM_MAX
}

/// One first-conversation question (`[[questions]]`). The hirer may answer
/// it at hire (`--answer id=text`, `answers` in the body); the agent asks
/// its person the rest on its first start.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Question {
    /// `a-z`, `0-9`, `_`; unique in the template.
    pub id: String,
    /// The question, one line, as the agent would ask it.
    pub ask: String,
    /// Where an answer goes: `role` (the "What I know about my work"
    /// section of ROLE.md) or `wiki:<Note>[#<Heading>]` (a starter note of
    /// this template, under one of its headings).
    pub fills: String,
}

/// A parsed `fills`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fills {
    Role,
    /// `(note file id "About my person.md", heading)`.
    Wiki(String, Option<String>),
}

impl Fills {
    pub fn parse(s: &str) -> Result<Fills, String> {
        let s = s.trim();
        if s == "role" {
            return Ok(Fills::Role);
        }
        let Some(rest) = s.strip_prefix("wiki:") else {
            return Err(format!("fills '{s}' must be role or wiki:<Note>[#<Heading>]"));
        };
        let (note, heading) = match rest.split_once('#') {
            Some((n, h)) => (n.trim(), Some(h.trim().to_string())),
            None => (rest.trim(), None),
        };
        if note.is_empty() || heading.as_deref().is_some_and(str::is_empty) {
            return Err(format!("fills '{s}' must be role or wiki:<Note>[#<Heading>]"));
        }
        let file = if note.ends_with(".md") { note.to_string() } else { format!("{note}.md") };
        Ok(Fills::Wiki(file, heading))
    }
}

/// A note line that is the heading `h` (any level).
pub fn is_heading(line: &str, h: &str) -> bool {
    let t = line.trim_end();
    let hashes = t.chars().take_while(|c| *c == '#').count();
    hashes > 0 && t[hashes..].starts_with(' ') && t[hashes..].trim() == h
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HelloSection {
    pub brief: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemplateToml {
    pub schema: u32,
    pub id: String,
    pub version: u32,
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// `K2` (built-ins only), `user:<name>` or `agent:<handle>`.
    pub author: String,
    #[serde(default)]
    pub shared_block: bool,
    #[serde(default)]
    pub agent: AgentSection,
    #[serde(default)]
    pub context: ContextSection,
    #[serde(default)]
    pub permissions: PermissionsSection,
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
    #[serde(default)]
    pub hello: Option<HelloSection>,
    /// First-conversation questions, in the order the agent asks them.
    #[serde(default)]
    pub questions: Vec<Question>,
    /// Same fields as the recipe's heartbeats. No built-in uses one.
    #[serde(default)]
    pub heartbeats: Vec<HeartbeatIn>,
}

/// Where a template came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    Library,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Builtin => "builtin",
            Origin::Library => "library",
        }
    }
}

/// What one file in a template folder is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    Manifest,
    Role,
    /// A wiki note id (`About my person.md`).
    Wiki(String),
    /// An own layer's label.
    Context(String),
    /// A skill name.
    Skill(String),
}

pub const ALLOWED_PATHS: &str =
    "template.toml, ROLE.md, wiki/<Note>.md, context/<label>.md, skills/<name>/SKILL.md";

/// AT4: only §2.2's paths, no `..`, nothing absolute.
pub fn classify_path(rel: &str) -> Result<FileKind, String> {
    let bad = || format!("{rel}: not a template path (allowed: {ALLOWED_PATHS})");
    if rel.contains('\\') || rel.contains('\0') || rel.starts_with('/') || rel.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return Err(bad());
    }
    match rel {
        "template.toml" => return Ok(FileKind::Manifest),
        "ROLE.md" => return Ok(FileKind::Role),
        _ => {}
    }
    if let Some(note) = rel.strip_prefix("wiki/") {
        crate::wiki::validate_note_id(note).map_err(|_| bad())?;
        return Ok(FileKind::Wiki(note.to_string()));
    }
    if let Some(file) = rel.strip_prefix("context/") {
        let label = file.strip_suffix(".md").filter(|l| !l.contains('/')).ok_or_else(bad)?;
        let label = super::super::steps::sanitize_layer_label(label).map_err(|_| bad())?;
        return Ok(FileKind::Context(label));
    }
    if let Some(rest) = rel.strip_prefix("skills/") {
        let name = rest.strip_suffix("/SKILL.md").filter(|n| !n.contains('/')).ok_or_else(bad)?;
        if !slug_ok(name, 64, true) {
            return Err(bad());
        }
        return Ok(FileKind::Skill(name.to_string()));
    }
    Err(bad())
}

/// `[a-z0-9]` then `[a-z0-9-]` (and `_` when `underscore`), at most `max`.
pub fn slug_ok(s: &str, max: usize, underscore: bool) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || (underscore && c == '_'))
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn line_ok(s: &str, max_chars: usize) -> bool {
    let t = s.trim();
    !t.is_empty() && t.chars().count() <= max_chars && !t.chars().any(|c| c.is_control())
}

/// sha256 over the sorted file paths and bytes: for each file, its path,
/// a NUL, its length (u64 little-endian) and its bytes. Hex.
pub fn hash_files(files: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    for (path, body) in files {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update((body.len() as u64).to_le_bytes());
        h.update(body.as_bytes());
    }
    hex(&h.finalize())
}

pub fn sha256_text(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A loaded, validated template.
#[derive(Debug, Clone)]
pub struct Template {
    pub meta: TemplateToml,
    pub origin: Origin,
    /// The library folder (`~/.k2/agent-templates/<id>/v<N>/`); built-ins
    /// have none.
    pub dir: Option<std::path::PathBuf>,
    /// Every file, by its path in the folder.
    pub files: BTreeMap<String, String>,
    pub hash: String,
}

impl Template {
    pub fn id(&self) -> &str {
        &self.meta.id
    }

    pub fn role_md(&self) -> &str {
        self.files.get("ROLE.md").map(String::as_str).unwrap_or_default()
    }

    fn of_kind(&self) -> impl Iterator<Item = (FileKind, &str)> {
        self.files.iter().filter_map(|(p, b)| classify_path(p).ok().map(|k| (k, b.as_str())))
    }

    /// Starter notes: `(note id, body)`.
    pub fn wiki_notes(&self) -> Vec<(String, &str)> {
        self.of_kind().filter_map(|(k, b)| if let FileKind::Wiki(n) = k { Some((n, b)) } else { None }).collect()
    }

    /// Own layers: `(label, markdown)`.
    pub fn layers(&self) -> Vec<(String, &str)> {
        self.of_kind().filter_map(|(k, b)| if let FileKind::Context(n) = k { Some((n, b)) } else { None }).collect()
    }

    /// Skills: `(name, SKILL.md)`.
    pub fn skills(&self) -> Vec<(String, &str)> {
        self.of_kind().filter_map(|(k, b)| if let FileKind::Skill(n) = k { Some((n, b)) } else { None }).collect()
    }

    /// The declared params plus `name` and `person`.
    pub fn vars(&self) -> BTreeSet<String> {
        BUILT_IN_VARS.iter().map(|s| s.to_string()).chain(self.meta.params.keys().cloned()).collect()
    }

    /// `suggest`, with each `set:<name>` expanded (in order, no repeats).
    pub fn suggested(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.meta.permissions.suggest {
            let ids: Vec<String> = match s.strip_prefix("set:") {
                Some(name) => super::sets::named_set(name).unwrap_or_default().iter().map(|x| x.to_string()).collect(),
                None => vec![s.clone()],
            };
            for id in ids {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// The `role:` this template writes (its id when unset).
    pub fn role(&self) -> String {
        self.meta.agent.role.clone().filter(|r| !r.trim().is_empty()).unwrap_or_else(|| self.meta.id.clone())
    }

    /// The list / picker card.
    pub fn summary_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.meta.id,
            "version": self.meta.version,
            "name": self.meta.name,
            "summary": self.meta.summary,
            "kind": self.meta.kind.as_str(),
            "aliases": self.meta.aliases,
            "author": self.meta.author,
            "origin": self.origin.as_str(),
            "hash": self.hash,
        })
    }

    /// `show`: the card plus what the agent gets.
    pub fn show_json(&self, with_files: bool) -> serde_json::Value {
        let mut v = self.summary_json();
        v["sharedBlock"] = serde_json::json!(self.meta.shared_block);
        v["agent"] = serde_json::json!({
            "role": self.role(),
            "tool": self.meta.agent.tool.clone().unwrap_or_else(|| "default".into()),
            "model": self.meta.agent.model.clone().filter(|m| !m.is_empty()),
        });
        v["catalog"] = serde_json::json!(self.meta.context.catalog);
        v["permissions"] = serde_json::json!({
            "cards": self.meta.permissions.cards,
            "suggest": self.meta.permissions.suggest,
            "expanded": self.suggested(),
        });
        v["params"] = serde_json::Value::Object(
            self.meta
                .params
                .iter()
                .map(|(k, p)| (k.clone(), serde_json::json!({ "label": p.label, "max": p.max, "required": p.required })))
                .collect(),
        );
        v["wiki"] = serde_json::json!(self.wiki_notes().into_iter().map(|(n, _)| n).collect::<Vec<_>>());
        v["layers"] = serde_json::json!(self.layers().into_iter().map(|(n, _)| n).collect::<Vec<_>>());
        v["skills"] = serde_json::json!(self.skills().into_iter().map(|(n, _)| n).collect::<Vec<_>>());
        v["heartbeats"] = serde_json::json!(self.meta.heartbeats.iter().map(|h| serde_json::json!({ "name": h.name, "frequency": h.frequency })).collect::<Vec<_>>());
        v["hello"] = serde_json::json!(self.meta.hello.as_ref().map(|h| h.brief.clone()));
        v["questions"] = serde_json::json!(self
            .meta
            .questions
            .iter()
            .map(|q| serde_json::json!({ "id": q.id, "ask": q.ask, "fills": q.fills }))
            .collect::<Vec<_>>());
        v["dir"] = serde_json::json!(self.dir);
        if with_files {
            v["files"] = serde_json::json!(self.files);
        }
        v
    }
}

/// What the loader expects of one folder.
pub struct Expect<'a> {
    pub origin: Origin,
    /// The folder's id (built-in folder name or library `<id>/`).
    pub id: &'a str,
    /// Library `v<N>`.
    pub version: Option<u32>,
    /// Ids and aliases a library template may not take.
    pub reserved: &'a BTreeSet<String>,
}

/// Validate a whole template folder. The error names the file and what's
/// wrong with it.
pub fn validate(files: BTreeMap<String, String>, dir: Option<std::path::PathBuf>, ex: &Expect<'_>) -> Result<Template, String> {
    let id = ex.id;
    let err = |m: String| format!("template '{id}': {m}");
    let mut kinds = Vec::new();
    for rel in files.keys() {
        kinds.push(classify_path(rel).map_err(err)?);
    }
    let manifest = files.get("template.toml").ok_or_else(|| err("missing template.toml".into()))?;
    let meta: TemplateToml = toml::from_str(manifest).map_err(|e| err(format!("template.toml: {e}")))?;
    if meta.schema != SCHEMA {
        return Err(err(format!("template.toml: schema {} (this K2 reads schema {SCHEMA})", meta.schema)));
    }
    if meta.id != id {
        return Err(err(format!("template.toml: id '{}' doesn't match its folder '{id}'", meta.id)));
    }
    if !slug_ok(&meta.id, 40, false) {
        return Err(err("template.toml: id must be a slug (a-z, 0-9, -; at most 40)".into()));
    }
    if meta.version == 0 {
        return Err(err("template.toml: version starts at 1".into()));
    }
    if let Some(v) = ex.version {
        if meta.version != v {
            return Err(err(format!("template.toml: version {} doesn't match its folder v{v}", meta.version)));
        }
    }
    if !line_ok(&meta.name, 60) {
        return Err(err("template.toml: name is 1–60 characters, one line".into()));
    }
    if !line_ok(&meta.summary, 200) {
        return Err(err("template.toml: summary is 1–200 characters, one line".into()));
    }
    // AT18: only built-ins may say K2.
    let author = meta.author.trim();
    match ex.origin {
        Origin::Builtin if author != "K2" => return Err(err("template.toml: a built-in's author is K2".into())),
        Origin::Library if author.eq_ignore_ascii_case("k2") || author.to_ascii_lowercase().starts_with("k2:") => {
            return Err(err("template.toml: only K2's built-ins may say author = \"K2\"".into()))
        }
        Origin::Library => {
            let ok = ["user:", "agent:"].iter().any(|p| author.strip_prefix(p).is_some_and(|n| line_ok(n, 80)));
            if !ok {
                return Err(err("template.toml: author is user:<name> or agent:<handle>".into()));
            }
        }
        Origin::Builtin => {}
    }
    for a in &meta.aliases {
        if !slug_ok(a, 40, false) || a == &meta.id {
            return Err(err(format!("template.toml: alias '{a}' must be a slug other than the id")));
        }
    }
    if ex.origin == Origin::Library {
        if meta.kind == Kind::System {
            return Err(err("template.toml: only built-ins are kind = \"system\"".into()));
        }
        if let Some(r) = std::iter::once(&meta.id).chain(meta.aliases.iter()).find(|x| ex.reserved.contains(*x)) {
            return Err(err(format!("'{r}' is a built-in template id or alias")));
        }
    }
    if let Some(r) = meta.agent.role.as_deref() {
        if !slug_ok(r.trim(), 40, true) {
            return Err(err("template.toml: [agent] role must be a slug".into()));
        }
    }
    for (field, v) in [("tool", &meta.agent.tool), ("model", &meta.agent.model)] {
        if let Some(s) = v.as_deref().filter(|s| !s.is_empty()) {
            if !line_ok(s, 120) {
                return Err(err(format!("template.toml: [agent] {field} is 1–120 characters")));
            }
        }
    }
    for c in &meta.context.catalog {
        crate::workspace::context_layers::resolve_catalog_entry(c)
            .map_err(|e| err(format!("template.toml: [context] catalog: {}", e.hint())))?;
    }
    for s in &meta.permissions.suggest {
        check_suggest(s).map_err(|m| err(format!("template.toml: [permissions] suggest: {m}")))?;
    }
    if meta.params.len() > MAX_PARAMS {
        return Err(err(format!("template.toml: at most {MAX_PARAMS} [params]")));
    }
    for (k, p) in &meta.params {
        if !ident_ok(k) || BUILT_IN_VARS.contains(&k.as_str()) {
            return Err(err(format!("template.toml: param '{k}' must be a-z, 0-9, _ (not name, person or open_questions)")));
        }
        if !line_ok(&p.label, 80) || p.max == 0 || p.max > MAX_PARAM_CHARS {
            return Err(err(format!("template.toml: param '{k}': a 1–80 character label and max 1–{MAX_PARAM_CHARS}")));
        }
    }
    let role = files.get("ROLE.md").filter(|r| !r.trim().is_empty()).ok_or_else(|| err("ROLE.md is missing or empty".into()))?;
    check_questions(&meta.questions, &files).map_err(|m| err(format!("template.toml: [[questions]] {m}")))?;

    // The recipe's caps and shape rules, through the recipe's own validator.
    let mut m = MemberRecipe { persona: Some(role.clone()), ..Default::default() };
    let mut notes = Vec::new();
    let mut layers = Vec::new();
    let mut skills = Vec::new();
    for (k, (_, body)) in kinds.iter().zip(files.iter()) {
        match k {
            FileKind::Wiki(n) => notes.push(WikiNote { id: n.clone(), body: body.clone() }),
            FileKind::Context(l) => layers.push(InlineLayer { label: l.clone(), markdown: body.clone() }),
            FileKind::Skill(s) => skills.push(SkillIn { name: s.clone(), body: body.clone() }),
            _ => {}
        }
    }
    m.wiki_notes = Some(notes);
    m.layers = Some(layers);
    m.skills = Some(skills);
    m.heartbeats = Some(meta.heartbeats.clone());
    m.context = Some(meta.context.catalog.clone());
    m.brief = meta.hello.as_ref().map(|h| h.brief.clone());
    recipe::validate_member(&m, 0).map_err(|e| err(e.hint.replace("members[0].", "")))?;

    // Placeholders: {{name}}, {{person}} and the declared params, nowhere
    // else (template.toml's own text is checked through [hello]).
    let vars: BTreeSet<String> = BUILT_IN_VARS.iter().map(|s| s.to_string()).chain(meta.params.keys().cloned()).collect();
    let mut texts: Vec<(&str, &str)> = files.iter().filter(|(p, _)| p.as_str() != "template.toml").map(|(p, b)| (p.as_str(), b.as_str())).collect();
    if let Some(h) = meta.hello.as_ref() {
        texts.push(("[hello] brief", h.brief.as_str()));
    }
    for hb in &meta.heartbeats {
        if let Some(i) = hb.instructions.as_deref() {
            texts.push(("[[heartbeats]] instructions", i));
        }
    }
    for (where_, text) in texts {
        for ph in render::placeholders(text).map_err(|m| err(format!("{where_}: {m}")))? {
            if !vars.contains(&ph) {
                return Err(err(format!(
                    "{where_}: unknown placeholder {{{{{ph}}}}} (known: {})",
                    vars.iter().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
        }
    }
    let hash = hash_files(&files);
    Ok(Template { meta, origin: ex.origin, dir, files, hash })
}

/// `[[questions]]`: at most [`MAX_QUESTIONS`], unique ids, a one-line ask,
/// and a `fills` naming `role` or a starter note (and heading) this
/// template ships.
fn check_questions(qs: &[Question], files: &BTreeMap<String, String>) -> Result<(), String> {
    if qs.len() > MAX_QUESTIONS {
        return Err(format!("at most {MAX_QUESTIONS} questions"));
    }
    let mut seen = BTreeSet::new();
    for q in qs {
        if !ident_ok(&q.id) {
            return Err(format!("id '{}' must be a-z, 0-9, _ (starting with a letter, at most 32)", q.id));
        }
        if !seen.insert(q.id.as_str()) {
            return Err(format!("id '{}' is used twice", q.id));
        }
        if !line_ok(&q.ask, MAX_ASK_CHARS) {
            return Err(format!("'{}': ask is 1–{MAX_ASK_CHARS} characters, one line", q.id));
        }
        if q.ask.contains("{{") {
            return Err(format!("'{}': ask is plain text (no placeholders)", q.id));
        }
        match Fills::parse(&q.fills).map_err(|m| format!("'{}': {m}", q.id))? {
            Fills::Role => {}
            Fills::Wiki(note, heading) => {
                let Some(body) = files.get(&format!("wiki/{note}")) else {
                    let notes: Vec<&str> = files.keys().filter_map(|p| p.strip_prefix("wiki/")).collect();
                    return Err(format!(
                        "'{}': fills names wiki note '{note}', which this template doesn't ship (its notes: {})",
                        q.id,
                        if notes.is_empty() { "none".to_string() } else { notes.join(", ") }
                    ));
                };
                if let Some(h) = heading {
                    if !body.lines().any(|l| is_heading(l, &h)) {
                        return Err(format!("'{}': wiki note '{note}' has no heading '{h}'", q.id));
                    }
                }
            }
        }
    }
    Ok(())
}

/// AT6: plain ids must be standing V1 actions (when the daemon installed
/// its catalog); `set:<name>` must name a known starter set.
fn check_suggest(s: &str) -> Result<(), String> {
    if let Some(name) = s.strip_prefix("set:") {
        return match super::sets::named_set(name) {
            Some(_) => Ok(()),
            None => Err(format!("unknown permission set '{name}'")),
        };
    }
    let shape = !s.is_empty()
        && s.len() <= 64
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    if !shape {
        return Err(format!("'{s}' is not an action id"));
    }
    match super::action_kind(s) {
        super::ActionKind::Standing | super::ActionKind::NotChecked => Ok(()),
        super::ActionKind::OneTime => Err(format!("'{s}' is a one-time action; templates suggest standing switches only")),
        super::ActionKind::Unknown => Err(format!("'{s}' is not a K2 permission action")),
    }
}
