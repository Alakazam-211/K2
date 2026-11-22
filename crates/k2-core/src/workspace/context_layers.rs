//! Context management stack — optional AGENTS.md layer stack (path references).
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

use crate::brand::CLI_NAME;
use crate::cli_name;

/// Soft size warning threshold for the composed AGENTS.md body (64 KiB).
pub const SOFT_WARN_BYTES: u64 = 64 * 1024;

/// Hard max for a host catalog pack `layer.md` (create + scan).
pub const USER_PACK_LAYER_MAX_BYTES: u64 = 16 * 1024;

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
    /// `'user'` | `'catalog:wiki-index'` | …
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
    /// Generated body shown in Settings when there is no file (Tooling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Built-in (or installed) catalog entry offered in Browse catalog / `k2 agent context catalog`.
///
/// Wire shape matches pack-metadata foundation: required id/path/label/source/kind;
/// optional description/version/author/tags for catalog UX (marketplace-ready).
///
/// **`recommended` is first-party only** — never trust a marketplace pack's
/// self-declared recommendation. Only K2 built-ins (or a future signed
/// allowlist) may set this true when constructing the list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextCatalogEntry {
    pub id: String,
    pub path: String,
    pub label: String,
    pub source: String,
    /// `"live"` | `"static"` | `"path"`
    pub kind: String,
    /// K2-controlled “nice experience” recommendation. Not a free-form tag.
    #[serde(default)]
    pub recommended: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Host pack directory for user packs (`~/.k2/context/catalog/packs/<slug>`).
    /// Absent on builtins. Not a workspace path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
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
    CatalogUnknown(String),
    PackTooLarge(String),
    Db(String),
}

impl ContextError {
    pub fn code(&self) -> &'static str {
        match self {
            ContextError::BadUsage(_) => "bad_usage",
            ContextError::NotFound(_) => "not_found",
            ContextError::PathEscape(_) => "path_escape",
            ContextError::DuplicateLayer(_) => "duplicate_layer",
            ContextError::CatalogUnknown(_) => "catalog_unknown",
            ContextError::PackTooLarge(_) => "pack_too_large",
            ContextError::Db(_) => "db_error",
        }
    }

    pub fn hint(&self) -> &str {
        match self {
            ContextError::BadUsage(h)
            | ContextError::NotFound(h)
            | ContextError::PathEscape(h)
            | ContextError::DuplicateLayer(h)
            | ContextError::CatalogUnknown(h)
            | ContextError::PackTooLarge(h)
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

// ── Catalog ───────────────────────────────────────────────────────────

/// Path for the live-regenerated connected-agents roster pack.
pub const CONNECTIONS_ROSTER_PATH: &str = ".k2/context/catalog/connections-roster.md";
pub const CONNECTIONS_ROSTER_SOURCE: &str = "catalog:connections-roster";
pub const CONNECTIONS_ROSTER_ID: &str = "connections:roster";

pub const HEARTBEATS_ROSTER_PATH: &str = ".k2/context/catalog/heartbeats-roster.md";
pub const HEARTBEATS_ROSTER_SOURCE: &str = "catalog:heartbeats-roster";
pub const HEARTBEATS_ROSTER_ID: &str = "heartbeats:roster";

pub const SKILLS_ROSTER_PATH: &str = ".k2/context/catalog/skills-roster.md";
pub const SKILLS_ROSTER_SOURCE: &str = "catalog:skills-roster";
pub const SKILLS_ROSTER_ID: &str = "skills:roster";

pub const USERS_ROSTER_PATH: &str = ".k2/context/catalog/users-roster.md";
pub const USERS_ROSTER_SOURCE: &str = "catalog:users-roster";
pub const USERS_ROSTER_ID: &str = "users:roster";

pub const SKIN_ROSTER_PATH: &str = ".k2/context/catalog/skin-roster.md";
pub const SKIN_ROSTER_SOURCE: &str = "catalog:skin-roster";
/// Canonical catalog id. Leftover alias [`SKIN_ROSTER_ID`] still resolves.
pub const APPS_ROSTER_ID: &str = "apps:roster";
/// Leftover alias of [`APPS_ROSTER_ID`]. Path stays `skin-roster.md`.
pub const SKIN_ROSTER_ID: &str = "skin:roster";

/// "Apps you maintain" (prd-app-handoff-v1 §4.11, 0.46.5): the Apps this
/// workspace's agent maintains, with a recent handover's notes. Live. A
/// handover stacks it on the agent that takes an App.
pub const APPS_MAINTAINED_PATH: &str = ".k2/context/catalog/apps-maintained.md";
pub const APPS_MAINTAINED_SOURCE: &str = "catalog:apps-maintained";
pub const APPS_MAINTAINED_ID: &str = "apps:maintained";

pub const WIKI_HYGIENE_PATH: &str = ".k2/context/catalog/wiki-hygiene.md";
pub const WIKI_HYGIENE_SOURCE: &str = "catalog:wiki-hygiene";
pub const WIKI_HYGIENE_ID: &str = "wiki:hygiene";

pub const SUBAGENTS_PACK_PATH: &str = ".k2/context/catalog/always-use-subagents.md";
pub const SUBAGENTS_PACK_SOURCE: &str = "catalog:subagents";
pub const SUBAGENTS_PACK_ID: &str = "subagents:pack";

/// "Browser and computer use" (0.46.11): agent-browser and agent-desktop,
/// where to get them, how to start, and the rules. Opt-in, like subagents.
/// Before 0.46.11 it was a user pack on one server with the same slug, so a
/// workspace that stacked `user:browser-and-computer-use` already has this
/// path; [`migrate_old_browser_user_pack_rows`] moves its row over.
pub const BROWSER_PACK_PATH: &str = ".k2/context/catalog/browser-and-computer-use.md";
pub const BROWSER_PACK_SOURCE: &str = "catalog:browser";
pub const BROWSER_PACK_ID: &str = "browser:pack";
/// The source tag of the old user pack this one replaces.
const OLD_BROWSER_USER_PACK_SOURCE: &str = "catalog:user:browser-and-computer-use";

/// Which live-generated pack a layer is (if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveKind {
    Connections,
    Heartbeats,
    Skills,
    Users,
    Skin,
    AppsMaintained,
}

fn builtin_catalog_entry(
    id: &str,
    path: &str,
    label: &str,
    source: &str,
    kind: &str,
    description: &str,
    version: Option<&str>,
    recommended: bool,
    tags: &[&str],
) -> ContextCatalogEntry {
    ContextCatalogEntry {
        id: id.into(),
        path: path.into(),
        label: label.into(),
        source: source.into(),
        kind: kind.into(),
        recommended,
        description: Some(description.into()),
        version: version.map(|v| v.into()),
        author: Some("K2".into()),
        // Free-form discovery tags only — never put "recommended" here.
        tags: tags.iter().map(|t| (*t).into()).collect(),
        dir: None,
    }
}

/// Built-in catalog entries: wiki seeds + lean packs + live rosters.
///
/// Live rosters (connections / heartbeats / skills / users / skin) rebuild on every
/// AGENTS.md compose so always-on context tracks a changing workspace.
pub fn list_builtin_catalog() -> Vec<ContextCatalogEntry> {
    vec![
        builtin_catalog_entry(
            "wiki:index",
            ".k2/wiki/_Index.md",
            "Wiki index",
            "catalog:wiki-index",
            "path",
            "Workspace wiki map — links and structure for .k2/wiki/.",
            Some("1.0.0"),
            true, // recommended
            &["wiki", "knowledge"],
        ),
        builtin_catalog_entry(
            "wiki:home",
            ".k2/wiki/Home.md",
            "Wiki home",
            "catalog:wiki-home",
            "path",
            "Wiki landing page for this workspace.",
            Some("1.0.0"),
            false,
            &["wiki", "knowledge"],
        ),
        builtin_catalog_entry(
            WIKI_HYGIENE_ID,
            WIKI_HYGIENE_PATH,
            "Wiki hygiene",
            WIKI_HYGIENE_SOURCE,
            "static",
            "Standing orders for keeping .k2/wiki/ healthy — link, index, no orphans; don’t dump the vault into AGENTS.md.",
            Some("1.0.0"),
            true, // recommended
            &["wiki", "knowledge", "hygiene"],
        ),
        builtin_catalog_entry(
            SUBAGENTS_PACK_ID,
            SUBAGENTS_PACK_PATH,
            "Always use subagents",
            SUBAGENTS_PACK_SOURCE,
            "static",
            "Standing order: do heavy work in subagent worktrees; review and cherry-pick onto main.",
            Some("1.0.0"),
            true, // recommended
            &["workflow", "subagents", "context"],
        ),
        builtin_catalog_entry(
            BROWSER_PACK_ID,
            BROWSER_PACK_PATH,
            "Browser and computer use",
            BROWSER_PACK_SOURCE,
            "static",
            "agent-browser (web) and agent-desktop (desktop apps): open-source CLIs, where to get them, how to start, and the safety rules.",
            Some("1.0.0"),
            false, // opt-in
            &["browser", "computer-use", "tools"],
        ),
        builtin_catalog_entry(
            "manager:pack",
            ".k2/context/catalog/manager.md",
            "Workspace Manager",
            "catalog:manager",
            "static",
            "Lean always-on standing orders for coordinating connected workspaces. Full playbook stays a loadable skill.",
            Some("1.0.0"),
            false,
            &["role", "manager"],
        ),
        // 0.46.12 (prd-agent-templates-v1 §5, AT9): the old "K2 Agent"
        // planner pack is now Kessel, K2's own agent. `k2:pack` and its
        // aliases resolve here; a workspace that already stacked the old
        // `catalog:k2-agent` row keeps its file and the row still composes
        // (AT17: the meaning changes for new adds only).
        builtin_catalog_entry(
            KESSEL_PACK_ID,
            KESSEL_PACK_PATH,
            "Kessel: K2's own agent",
            KESSEL_PACK_SOURCE,
            "static",
            "Lean always-on orders for Kessel, the agent that runs K2 for its person: setup, settings and agent management.",
            Some("1.0.0"),
            false,
            &["role", "kessel"],
        ),
        builtin_catalog_entry(
            CONNECTIONS_ROSTER_ID,
            CONNECTIONS_ROSTER_PATH,
            "Connected agents roster",
            CONNECTIONS_ROSTER_SOURCE,
            "live",
            "Live list of connected workspace-agents (local + remote). Regenerates whenever AGENTS.md is rewritten.",
            None,
            true, // recommended
            &["live", "roster", "connections"],
        ),
        builtin_catalog_entry(
            HEARTBEATS_ROSTER_ID,
            HEARTBEATS_ROSTER_PATH,
            "Heartbeats roster",
            HEARTBEATS_ROSTER_SOURCE,
            "live",
            "Live catalog of scheduled heartbeats (name, frequency, WAKEUP path) — not full WAKEUP bodies.",
            None,
            true, // recommended
            &["live", "roster", "heartbeats"],
        ),
        builtin_catalog_entry(
            SKILLS_ROSTER_ID,
            SKILLS_ROSTER_PATH,
            "Skills roster",
            SKILLS_ROSTER_SOURCE,
            "live",
            "Live catalog of .k2/skills/ profiles to load on demand — not full skill dumps.",
            None,
            false,
            &["live", "roster", "skills"],
        ),
        builtin_catalog_entry(
            USERS_ROSTER_ID,
            USERS_ROSTER_PATH,
            "User roster",
            USERS_ROSTER_SOURCE,
            "live",
            concat!("Live list of humans on this K2 box (username with their real name when set, e.g. z3thon — Rosson Long; role; disabled). Regenerates whenever AGENTS.md is rewritten. Do not `", cli_name!(), " msg` these names."),
            None,
            false,
            &["live", "roster", "users"],
        ),
        builtin_catalog_entry(
            APPS_ROSTER_ID,
            SKIN_ROSTER_PATH,
            "App guest roster",
            SKIN_ROSTER_SOURCE,
            "live",
            concat!("Live list of app guests (username, role, password, default rooms). Not Connect / Server Access. Regenerates whenever AGENTS.md is rewritten. Do not `", cli_name!(), " msg` these names. Leftover catalog id: skin:roster."),
            None,
            false,
            &["live", "roster", "app", "skin"],
        ),
        builtin_catalog_entry(
            APPS_MAINTAINED_ID,
            APPS_MAINTAINED_PATH,
            "Apps you maintain",
            APPS_MAINTAINED_SOURCE,
            "live",
            "Live list of the Apps this agent maintains (hostnames, state, live Seed), with the previous agent's notes for 72 hours after a handover. Regenerates whenever AGENTS.md is rewritten.",
            None,
            false,
            &["live", "apps"],
        ),
    ]
}

/// Catalog SSOT: builtins (unchanged) ∪ scanned host user packs.
///
/// User packs live at `k2_home()/context/catalog/packs/<slug>/{pack.toml,layer.md}`.
/// Invalid / `kind=live` / reserved-id collisions are skipped (logged).
/// Creating a pack does **not** stack it on any workspace.
pub fn list_catalog() -> Vec<ContextCatalogEntry> {
    let mut out = list_builtin_catalog();
    out.extend(scan_user_packs());
    out
}

/// Created host catalog pack (library only — not stacked).
#[derive(Debug, Clone)]
pub struct CreatedUserPack {
    pub entry: ContextCatalogEntry,
    pub dir: PathBuf,
}

/// Kessel's pack (0.46.12): the id, its file and its source tag.
pub const KESSEL_PACK_ID: &str = "kessel:pack";
pub const KESSEL_PACK_PATH: &str = ".k2/context/catalog/kessel.md";
pub const KESSEL_PACK_SOURCE: &str = "catalog:kessel";

const RESERVED_ID_PREFIXES: &[&str] = &[
    "wiki:",
    "manager:",
    "k2:",
    "kessel:",
    "connections:",
    "heartbeats:",
    "skills:",
    "users:",
    "apps:",
    "skin:",
    "subagents:",
    "browser:",
    "catalog:",
    "pinned:",
    "preset:",
];

const RESERVED_ALIASES: &[&str] = &[
    "roster",
    "people",
    "manager",
    "k2",
    "k2-agent",
    "kessel",
    "hygiene",
    "subagents",
    "connected-agents",
    "workspace-manager",
    "k2so-agent",
    "connections",
    "connections-roster",
    "heartbeats",
    "heartbeats-roster",
    "skills",
    "skills-roster",
    "skills-index",
    "users",
    "users-roster",
    "people-roster",
    "user-roster",
    "apps",
    "apps:roster",
    "apps-roster",
    "skin-roster",
    "skins-roster",
    "skin-users",
    "skin:roster",
    "apps-maintained",
    "apps:maintained",
    "wiki-hygiene",
    "always-use-subagents",
    "use-subagents",
    "browser",
    "browser-and-computer-use",
    "wiki-index",
    "wiki-home",
    "catalog",
    "pinned",
    "preset",
];

const RESERVED_PATH_STEMS: &[&str] = &[
    "manager",
    "k2-agent",
    "kessel",
    "wiki-hygiene",
    "always-use-subagents",
    "browser-and-computer-use",
    "connections-roster",
    "heartbeats-roster",
    "skills-roster",
    "users-roster",
    "skin-roster",
    "apps-maintained",
];

/// Host library root: `~/.k2/context/catalog/packs`.
pub fn host_catalog_packs_dir() -> PathBuf {
    crate::paths::k2_home().join("context").join("catalog").join("packs")
}

fn user_pack_source(slug: &str) -> String {
    format!("catalog:user:{slug}")
}

fn user_pack_id(slug: &str) -> String {
    format!("user:{slug}")
}

fn user_pack_workspace_path(slug: &str) -> String {
    format!(".k2/context/catalog/{slug}.md")
}

fn slug_from_user_source(source: &str) -> Option<&str> {
    source.strip_prefix("catalog:user:")
}

/// Parse `user:<slug>` or a bare filesystem-safe slug into `(id, slug)`.
pub fn normalize_user_pack_id(raw: &str) -> Result<(String, String), ContextError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ContextError::BadUsage(
            "pack id must not be empty (expected user:<slug>)".into(),
        ));
    }
    let slug = if let Some(rest) = raw.strip_prefix("user:") {
        rest
    } else {
        raw
    };
    validate_user_pack_identity(raw, slug)?;
    Ok((user_pack_id(slug), slug.to_string()))
}

fn validate_user_pack_identity(raw_id: &str, slug: &str) -> Result<(), ContextError> {
    let lower_raw = raw_id.trim().to_ascii_lowercase();
    let lower_slug = slug.to_ascii_lowercase();

    if RESERVED_ID_PREFIXES
        .iter()
        .any(|p| lower_raw.starts_with(p) || lower_slug.starts_with(p))
    {
        return Err(ContextError::BadUsage(format!(
            "reserved catalog id prefix: {raw_id}"
        )));
    }
    if RESERVED_ALIASES
        .iter()
        .any(|a| lower_raw == *a || lower_slug == *a)
    {
        return Err(ContextError::BadUsage(format!(
            "reserved catalog id alias: {raw_id}"
        )));
    }
    if RESERVED_PATH_STEMS.iter().any(|s| lower_slug == *s) {
        return Err(ContextError::BadUsage(format!(
            "reserved catalog path stem: {slug}"
        )));
    }
    if !is_filesystem_safe_slug(slug) {
        return Err(ContextError::BadUsage(format!(
            "slug must be filesystem-safe [a-z0-9][a-z0-9_-]*: {slug}"
        )));
    }
    Ok(())
}

fn is_filesystem_safe_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 64 {
        return false;
    }
    let mut chars = slug.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn title_case_slug(slug: &str) -> String {
    slug.split(|c| c == '-' || c == '_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut cs = p.chars();
            match cs.next() {
                Some(first) => {
                    let mut s = first.to_ascii_uppercase().to_string();
                    s.push_str(&cs.as_str().to_ascii_lowercase());
                    s
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The owner as an author: the shared stamp rule (the linked account's
/// username, 0.46.0), else `user` when nothing is linked.
fn owner_author_label() -> String {
    match crate::person_name::sender_identity_now(crate::person_name::Sender::OwnerToken) {
        s if s == crate::notify_prefs::OWNER => "user".to_string(),
        s => s,
    }
}

#[derive(Debug, Deserialize)]
struct PackToml {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Parse pack.toml. `kind = "live"` is invalid. `recommended` is never honored.
pub fn parse_user_pack_toml(raw: &str) -> Result<ParsedUserPack, ContextError> {
    let parsed: PackToml = toml::from_str(raw).map_err(|e| {
        ContextError::BadUsage(format!("invalid pack.toml: {e}"))
    })?;
    let kind = parsed
        .kind
        .as_deref()
        .unwrap_or("static")
        .trim()
        .to_ascii_lowercase();
    if kind == "live" {
        return Err(ContextError::BadUsage(
            "kind = \"live\" is invalid for user catalog packs".into(),
        ));
    }
    if kind != "static" && kind != "path" {
        return Err(ContextError::BadUsage(format!(
            "unsupported pack kind '{kind}' (user packs must be static)"
        )));
    }
    let tags: Vec<String> = parsed
        .tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("recommended"))
        .collect();
    Ok(ParsedUserPack {
        id: parsed.id,
        name: parsed.name,
        description: parsed.description,
        version: parsed.version.or_else(|| Some("1.0.0".into())),
        kind,
        author: parsed.author,
        tags,
    })
}

#[derive(Debug, Clone)]
pub struct ParsedUserPack {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub kind: String,
    pub author: Option<String>,
    pub tags: Vec<String>,
}

fn stub_layer_md() -> &'static str {
    concat!("Standing orders for this catalog pack. Keep this layer lean — prefer links over dumps.\n\
     This pack is library-only until someone stacks it (`", cli_name!(), " agent context add <id>`).\n")
}

fn stub_pack_toml(id: &str, name: &str, author: &str, tags: &[String]) -> String {
    let tags_lit = if tags.is_empty() {
        "[]".to_string()
    } else {
        let inner = tags
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(", ");
        format!("[{inner}]")
    };
    format!(
        "id = \"{id}\"\n\
         name = \"{name}\"\n\
         description = \"\"\n\
         version = \"1.0.0\"\n\
         kind = \"static\"\n\
         license = \"MIT\"\n\
         author = \"{author}\"\n\
         tags = {tags_lit}\n"
    )
}

fn check_layer_size(bytes: u64, slug: &str) -> Result<(), ContextError> {
    if bytes > USER_PACK_LAYER_MAX_BYTES {
        return Err(ContextError::PackTooLarge(format!(
            "layer.md for '{slug}' is {bytes} bytes (max {USER_PACK_LAYER_MAX_BYTES})"
        )));
    }
    Ok(())
}

/// Stub a host user pack dir. Does **not** stack on any workspace.
pub fn create_user_pack(
    id: &str,
    label: Option<&str>,
    tags: &[String],
) -> Result<CreatedUserPack, ContextError> {
    create_user_pack_inner(id, label, tags, None, None)
}

/// Test/internal: allow injecting kind or layer body so create-time
/// `kind=live` / `pack_too_large` can fail loud.
pub fn create_user_pack_inner(
    id: &str,
    label: Option<&str>,
    tags: &[String],
    kind_override: Option<&str>,
    layer_override: Option<&str>,
) -> Result<CreatedUserPack, ContextError> {
    let (canon_id, slug) = normalize_user_pack_id(id)?;
    if let Some(kind) = kind_override {
        if kind.eq_ignore_ascii_case("live") {
            return Err(ContextError::BadUsage(
                "kind = \"live\" is invalid for user catalog packs".into(),
            ));
        }
    }
    let layer = layer_override.unwrap_or_else(|| stub_layer_md());
    check_layer_size(layer.len() as u64, &slug)?;

    let dir = host_catalog_packs_dir().join(&slug);
    if dir.exists() {
        return Err(ContextError::BadUsage(format!(
            "catalog pack already exists: {slug}"
        )));
    }
    fs::create_dir_all(&dir).map_err(|e| {
        ContextError::Db(format!(
            "cannot create catalog pack dir {}: {e}",
            dir.display()
        ))
    })?;

    let name = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| title_case_slug(&slug));
    let author = owner_author_label();
    let toml_kind = kind_override.unwrap_or("static");
    let mut toml_body = stub_pack_toml(&canon_id, &name, &author, tags);
    if toml_kind != "static" {
        toml_body = toml_body.replace("kind = \"static\"", &format!("kind = \"{toml_kind}\""));
    }
    // Re-validate the toml we are about to write (rejects kind=live if
    // a caller rewrote the stub).
    parse_user_pack_toml(&toml_body)?;

    let toml_path = dir.join("pack.toml");
    let layer_path = dir.join("layer.md");
    fs::write(&toml_path, toml_body).map_err(|e| {
        ContextError::Db(format!("cannot write {}: {e}", toml_path.display()))
    })?;
    fs::write(&layer_path, layer).map_err(|e| {
        let _ = fs::remove_dir_all(&dir);
        ContextError::Db(format!("cannot write {}: {e}", layer_path.display()))
    })?;

    let entry = match user_pack_entry_from_dir(&dir, &slug) {
        Ok(e) => e,
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
    };
    Ok(CreatedUserPack { entry, dir })
}

/// Remove a host user pack dir. Does **not** touch stacked workspace copies.
///
/// A pack whose slug a built-in took later (0.46.11:
/// `browser-and-computer-use`) is skipped by the scan, but its folder can
/// still be deleted here, so it isn't stuck on disk.
pub fn delete_user_pack(id: &str) -> Result<(), ContextError> {
    let slug = match normalize_user_pack_id(id) {
        Ok((_canon, slug)) => slug,
        Err(e) => {
            let raw = id.trim();
            let slug = raw.strip_prefix("user:").unwrap_or(raw);
            if OLD_BROWSER_USER_PACK_SOURCE.strip_prefix("catalog:user:") != Some(slug) {
                return Err(e);
            }
            slug.to_string()
        }
    };
    let dir = host_catalog_packs_dir().join(&slug);
    if !dir.is_dir() {
        return Err(ContextError::NotFound(format!(
            "catalog pack not found: {slug}"
        )));
    }
    fs::remove_dir_all(&dir).map_err(|e| {
        ContextError::Db(format!(
            "cannot delete catalog pack {}: {e}",
            dir.display()
        ))
    })
}

fn scan_user_packs() -> Vec<ContextCatalogEntry> {
    let root = host_catalog_packs_dir();
    let entries = match fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let slug = match dir.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        match user_pack_entry_from_dir(&dir, &slug) {
            Ok(entry) => out.push(entry),
            Err(e) => {
                crate::log_debug!(
                    "[context-catalog] skip pack '{}': {}",
                    slug,
                    e.hint()
                );
            }
        }
    }
    out
}

fn user_pack_entry_from_dir(dir: &Path, slug: &str) -> Result<ContextCatalogEntry, ContextError> {
    if let Err(e) = validate_user_pack_identity(&user_pack_id(slug), slug) {
        return Err(e);
    }
    let toml_path = dir.join("pack.toml");
    if !toml_path.is_file() {
        return Err(ContextError::BadUsage(format!(
            "missing pack.toml in {}",
            dir.display()
        )));
    }
    let raw = fs::read_to_string(&toml_path).map_err(|e| {
        ContextError::Db(format!("cannot read {}: {e}", toml_path.display()))
    })?;
    let parsed = parse_user_pack_toml(&raw)?;
    if let Some(ref toml_id) = parsed.id {
        let trimmed = toml_id.trim();
        if !trimmed.is_empty() && trimmed != user_pack_id(slug) {
            if let Err(e) = validate_user_pack_identity(trimmed, slug) {
                return Err(e);
            }
            return Err(ContextError::BadUsage(format!(
                "pack.toml id '{trimmed}' does not match directory slug '{slug}'"
            )));
        }
    }

    let layer_path = dir.join("layer.md");
    if !layer_path.is_file() {
        return Err(ContextError::BadUsage(format!(
            "missing layer.md in {}",
            dir.display()
        )));
    }
    let layer_bytes = fs::metadata(&layer_path)
        .map(|m| m.len())
        .unwrap_or(0);
    check_layer_size(layer_bytes, slug)?;

    let label = parsed
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| title_case_slug(slug));
    let description = parsed
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Ok(ContextCatalogEntry {
        id: user_pack_id(slug),
        path: user_pack_workspace_path(slug),
        label,
        source: user_pack_source(slug),
        kind: parsed.kind,
        recommended: false,
        description,
        version: parsed.version,
        author: parsed.author.filter(|a| !a.trim().is_empty()),
        tags: parsed.tags,
        dir: Some(dir.to_string_lossy().into_owned()),
    })
}

fn copy_host_user_pack_layer(
    project_path: &str,
    slug: &str,
    rel_path: &str,
) -> Result<(), ContextError> {
    let dest = Path::new(project_path).join(rel_path);
    if dest.is_file() {
        return Ok(());
    }
    let src = host_catalog_packs_dir().join(slug).join("layer.md");
    if !src.is_file() {
        return Err(ContextError::NotFound(format!(
            "host catalog layer missing: {}",
            src.display()
        )));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            ContextError::Db(format!(
                "cannot create context catalog directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    fs::copy(&src, &dest).map_err(|e| {
        ContextError::Db(format!(
            "cannot materialize {} from {}: {e}",
            dest.display(),
            src.display()
        ))
    })?;
    Ok(())
}

fn live_kind_for_source(source: &str) -> Option<LiveKind> {
    match source {
        CONNECTIONS_ROSTER_SOURCE => Some(LiveKind::Connections),
        HEARTBEATS_ROSTER_SOURCE => Some(LiveKind::Heartbeats),
        SKILLS_ROSTER_SOURCE => Some(LiveKind::Skills),
        USERS_ROSTER_SOURCE => Some(LiveKind::Users),
        SKIN_ROSTER_SOURCE => Some(LiveKind::Skin),
        APPS_MAINTAINED_SOURCE => Some(LiveKind::AppsMaintained),
        _ => None,
    }
}

fn live_kind_for_path(path: &str) -> Option<LiveKind> {
    if path == CONNECTIONS_ROSTER_PATH || path.ends_with("/context/catalog/connections-roster.md") {
        Some(LiveKind::Connections)
    } else if path == HEARTBEATS_ROSTER_PATH
        || path.ends_with("/context/catalog/heartbeats-roster.md")
    {
        Some(LiveKind::Heartbeats)
    } else if path == SKILLS_ROSTER_PATH || path.ends_with("/context/catalog/skills-roster.md") {
        Some(LiveKind::Skills)
    } else if path == USERS_ROSTER_PATH || path.ends_with("users-roster.md") {
        Some(LiveKind::Users)
    } else if path == SKIN_ROSTER_PATH || path.ends_with("skin-roster.md") {
        Some(LiveKind::Skin)
    } else if path == APPS_MAINTAINED_PATH || path.ends_with("apps-maintained.md") {
        Some(LiveKind::AppsMaintained)
    } else {
        None
    }
}

fn live_kind_for_layer(layer: &ContextLayer) -> Option<LiveKind> {
    live_kind_for_source(&layer.source).or_else(|| live_kind_for_path(&layer.path))
}

/// True when this layer is live-generated (not a static user/wiki file).
pub fn is_live_generated_layer(layer: &ContextLayer) -> bool {
    live_kind_for_layer(layer).is_some()
}

/// Markdown body for a live layer (no leading H1 — compose adds `## label`).
pub fn render_live_layer_body(project_path: &str, layer: &ContextLayer) -> Option<String> {
    match live_kind_for_layer(layer)? {
        LiveKind::Connections => Some(render_connections_roster_body(project_path)),
        LiveKind::Heartbeats => Some(render_heartbeats_roster_body(project_path)),
        LiveKind::Skills => Some(render_skills_roster_body(project_path)),
        LiveKind::Users => Some(render_users_roster_body(project_path)),
        LiveKind::Skin => Some(render_skin_roster_body(project_path)),
        LiveKind::AppsMaintained => Some(render_apps_maintained_body(project_path)),
    }
}

/// "This workspace" (prd-workspace-sessions-cli-v1 WS-D2): a pointer to
/// `k2 sessions` plus the workspace's **named** sidecars, which are
/// durable. Live state never goes in AGENTS.md (it is read once, at
/// start): `k2 sessions` and `k2 whoami` carry it.
pub fn render_this_workspace_paragraph(project_path: &str) -> String {
    let mut out = String::from("### This workspace\n\n");
    out.push_str(
        concat!("Other sessions may be working in this workspace (the main agent, sidecars, \
         chat tabs). `", cli_name!(), " sessions` lists them with addresses you can `", cli_name!(), " msg`; \
         `", cli_name!(), " sessions --past` finds older conversations to reopen."),
    );
    let named: Vec<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        match crate::workspace::agent_identity::resolve_project_id(&conn, project_path) {
            Some(pid) => {
                let ws = crate::workspace_session_handles::workspace_address_name(&conn, &pid)
                    .unwrap_or_default();
                let mut v: Vec<String> = crate::sidecar::list_rows(&conn, &pid)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.name.as_deref().is_some_and(|n| !n.trim().is_empty()))
                    .filter_map(|r| r.handle)
                    .filter(|h| !h.chars().all(|c| c.is_ascii_digit()))
                    .map(|h| crate::workspace_session_handles::format_address(&ws, Some(&h)))
                    .collect();
                v.sort();
                v.dedup();
                v
            }
            None => Vec::new(),
        }
    };
    if !named.is_empty() {
        let list: Vec<String> = named.iter().map(|a| format!("`{a}`")).collect();
        out.push_str(&format!(" Named sidecars: {}.", list.join(", ")));
    }
    out.push_str("\n\n");
    out
}

/// Markdown body for the connections roster (no leading H1 — compose adds `## label`).
pub fn render_connections_roster_body(project_path: &str) -> String {
    let mut out = String::new();
    out.push_str(
        concat!("Live roster of **connected workspace-agents** (peers, not sub-agents). \
         These are **workspace-agents** you can `", cli_name!(), " msg`. Humans who can sign \
         into this K2 box are **not** listed here — they write to you from the \
         app as `[from <name>]`. To see them: `", cli_name!(), " connections list --users`, \
         or stack **User roster** (`", cli_name!(), " agent context add users:roster`) for \
         always-on context. Do **not** `", cli_name!(), " msg` those names. Regenerated whenever \
         K2 rewrites AGENTS.md.\n\n"),
    );
    out.push_str("Message / peek:\n\n");
    out.push_str(concat!("    ", cli_name!(), " msg <workspace-name> \"short live knock\"\n"));
    out.push_str(concat!("    ", cli_name!(), " msg <workspace-name> --inbox-wake <path> [path…]\n"));
    out.push_str(concat!("    ", cli_name!(), " read <workspace-name>\n"));
    out.push_str(concat!("    ", cli_name!(), " connections list\n\n"));
    out.push_str(&render_this_workspace_paragraph(project_path));

    let local = crate::connections::list_peers(project_path).unwrap_or_default();
    let remotes = list_remote_connection_summaries(project_path);

    if local.is_empty() && remotes.is_empty() {
        out.push_str("### No connected agents yet\n\n");
        out.push_str("Wire a peer with:\n\n");
        out.push_str(concat!("    ", cli_name!(), " connections add <other-workspace-path>\n"));
        out.push_str(concat!("    ", cli_name!(), " connections add agent::host.k2.dev   # remote form\n"));
        return out;
    }

    if !local.is_empty() {
        out.push_str("### Local connections\n\n");
        for peer in &local {
            let status = if peer.reachable {
                "reachable"
            } else {
                "unreachable"
            };
            let rel = if peer.relation_types.is_empty() {
                String::new()
            } else {
                format!(" · {}", peer.relation_types.join(", "))
            };
            out.push_str(&format!("- **{}** — `{status}`{rel}\n", peer.project_name));
            if !peer.path.is_empty() {
                out.push_str(&format!("  - path: `{}`\n", peer.path));
            }
        }
        out.push('\n');
    }

    if !remotes.is_empty() {
        out.push_str("### Remote connections\n\n");
        for r in &remotes {
            let pair = if r.paired { "paired" } else { "unbound" };
            out.push_str(&format!(
                "- **{}** @ `{}` — remote · {pair}\n",
                r.agent, r.host
            ));
            out.push_str(&format!("  - address: `{}`\n", r.remote_addr));
        }
        out.push('\n');
    }

    out.push_str(
        concat!("To update: `", cli_name!(), " connections add|remove …` — this layer rewrites on the next AGENTS.md regen \
         and when connections change.\n"),
    );
    out
}

/// Live heartbeats catalog (names, schedules, WAKEUP paths) — not WAKEUP bodies.
pub fn render_heartbeats_roster_body(project_path: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "Live catalog of this workspace’s **heartbeats** (scheduled wakes). \
         Regenerated whenever K2 rewrites AGENTS.md.\n\n",
    );
    out.push_str("Manage:\n\n");
    out.push_str(concat!("    ", cli_name!(), " heartbeat list\n"));
    out.push_str(concat!("    ", cli_name!(), " heartbeat signal wakeup <name>\n"));
    out.push_str(concat!("    ", cli_name!(), " heartbeat enable|disable <name>\n\n"));
    out.push_str(
        "WAKEUP.md is the **user message** for that schedule — do not paste full wake \
         bodies into AGENTS.md; edit the path listed below.\n\n",
    );

    let rows = crate::heartbeats::k2_heartbeat_list(project_path.to_string()).unwrap_or_default();
    if rows.is_empty() {
        out.push_str("### No heartbeats yet\n\n");
        out.push_str("Create one in Settings → Workspaces / Agents → <agent> → Heartbeats, or:\n\n");
        out.push_str(concat!("    ", cli_name!(), " heartbeat create <name> --every 1h\n"));
        return out;
    }

    out.push_str("### Schedules\n\n");
    for hb in &rows {
        let state = if hb.enabled { "on" } else { "off" };
        let last = hb
            .last_fired
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("never");
        out.push_str(&format!(
            "- **{}** — `{state}` · `{}`\n",
            hb.name, hb.frequency
        ));
        out.push_str(&format!("  - wakeup: `{}`\n", hb.wakeup_path));
        out.push_str(&format!("  - last fired: {last}\n"));
        if let Some(ref err) = hb.schedule_error {
            if !err.is_empty() {
                out.push_str(&format!("  - schedule error: {err}\n"));
            }
        }
        // An enabled row's reason is a usage-limit pause, which ends by
        // itself: AGENTS.md (regenerated rarely) would keep a stale line.
        if let Some(ref reason) = hb.disabled_reason {
            if !reason.is_empty() && !hb.enabled {
                out.push_str(&format!("  - disabled reason: {reason}\n"));
            }
        }
    }
    out.push('\n');
    out.push_str(
        "Archived heartbeats are omitted. This list updates on the next AGENTS.md regen \
         after create/edit/archive.\n",
    );
    out
}

/// Live skills catalog under `.k2/skills/` — load on demand, don’t dump bodies.
pub fn render_skills_roster_body(project_path: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "Live catalog of **loadable skills** (documentation profiles under `.k2/skills/`). \
         Regenerated whenever K2 rewrites AGENTS.md.\n\n",
    );
    out.push_str(
        "Skills are **on-demand** — load the matching `SKILL.md` when you need depth. \
         Do not stack full skill bodies into always-on context.\n\n",
    );

    let skills = crate::skills::crud::list(project_path).unwrap_or_default();
    if skills.is_empty() {
        out.push_str("### No skills yet\n\n");
        out.push_str("Common built-ins appear after first use (e.g. `k2-cli`, role packs).\n");
        out.push_str("Create or install skills under `.k2/skills/<name>/SKILL.md`.\n");
        return out;
    }

    out.push_str("### Available skills\n\n");
    for s in &skills {
        let title = s
            .title
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(s.name.as_str());
        out.push_str(&format!("- **{}**", s.name));
        if title != s.name {
            out.push_str(&format!(" — {title}"));
        }
        out.push('\n');
        out.push_str(&format!("  - path: `.k2/skills/{}/SKILL.md`\n", s.name));
    }
    out.push('\n');
    out.push_str("This list updates on the next AGENTS.md regen after skills are added/removed.\n");
    out
}

/// Live list of humans on this box (username / role / disabled).
///
/// Same rows as `connect_users::list_people_for_agents` — host owner plus
/// every stored connect-user, including member, viewer, and disabled.
/// The owner is the linked account's username (none linked → `"owner"`);
/// real names follow usernames after ` — `.
pub fn render_users_roster_body(_project_path: &str) -> String {
    // 0.46.0: the owner row is the linked account's username (identity =
    // username), else "owner" — the old ownerDisplayName is not read.
    let owner = crate::person_name::owner_login_now().unwrap_or_default();
    let rows = crate::connect_users::list_people_for_agents(owner.trim()).unwrap_or_default();
    // 0.46.0: real names (display only). The map is keyed by login, plus
    // the owner's display name — the synthesized owner row's username.
    let names = crate::person_name::load_map();
    let label = |username: &str| {
        let key = if username.eq_ignore_ascii_case(crate::notify_prefs::OWNER) { crate::notify_prefs::OWNER } else { username };
        crate::person_name::label(username, names.get(key))
    };
    let any_named = rows.iter().any(|r| label(&r.username) != r.username);

    let mut out = String::new();
    out.push_str(
        concat!("These are **humans** who can sign into this K2 box — not workspace-agents.\n\
         They write to you from the app as `[from <name>]`; someone signed in to an App is `[guest <login> on App <app>]`, never one of them. Do **not** `", cli_name!(), " msg` these names.\n\
         CLI lookup: `", cli_name!(), " connections list --users`. Regenerated whenever K2 rewrites AGENTS.md.\n\n"),
    );
    if any_named {
        out.push_str(
            "A real name, when set, follows the username after ` — `. It is for context only: \
             `[from …]`, assignees and every command use the username.\n\n",
        );
    }
    out.push_str("| USERNAME | ROLE | DISABLED |\n");
    out.push_str("| --- | --- | --- |\n");
    for r in rows {
        let disabled = if r.disabled { "yes" } else { "no" };
        out.push_str(&format!(
            "| {} | {} | {disabled} |\n",
            label(&r.username),
            r.role.as_wire()
        ));
    }
    out
}

/// Live list of app guests (not Connect / Server Access).
///
/// Username + role + has_password + default rooms. Never platform-token
/// counts, never the raw `k2skn_` secret. Same people as `k2 app user list`.
pub fn render_skin_roster_body(_project_path: &str) -> String {
    let users = crate::skin::list_principals().unwrap_or_default();

    let mut out = String::new();
    out.push_str(
        concat!("These are **app** guests (K2-login sessions for custom UIs) — \
         not Connect / Server Access operators, and not workspace-agents.\n\
         Do **not** `", cli_name!(), " msg` these names. CLI lookup: `", cli_name!(), " app user list` \
         (leftover: `", cli_name!(), " skin user list`). \
         Keys for other sites: `", cli_name!(), " app token create --name <label> --agent sales` \
         (leftover: `", cli_name!(), " skin-token create --name`). \
         Regenerated whenever K2 rewrites AGENTS.md.\n\n"),
    );
    if users.is_empty() {
        out.push_str("### No app users yet\n\n");
        out.push_str("Add a guest in Settings → Cores → Apps, or:\n\n");
        out.push_str(concat!("    ", cli_name!(), " app user add <username>\n"));
        out.push_str(concat!("    ", cli_name!(), " app token create --name vercel --agent sales\n"));
        return out;
    }
    out.push_str("| USERNAME | ROLE | PASSWORD | DEFAULT ROOMS |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for u in users {
        let role = u
            .role_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("—");
        let pw = if u.has_password { "yes" } else { "no" };
        let rooms = if u.default_room_handles.is_empty() {
            if u.default_rooms.is_empty() {
                "—".to_string()
            } else {
                u.default_rooms.join(", ")
            }
        } else {
            u.default_room_handles.join(", ")
        };
        out.push_str(&format!("| {} | {role} | {pw} | {rooms} |\n", u.username));
    }
    out
}

/// "Apps you maintain" (§4.11): the Apps whose `owner_workspace` is this
/// workspace, and for 72 hours after a handover who handed it over and
/// where its brief is. Names and states only: never a guest, a secret or a
/// path.
pub fn render_apps_maintained_body(project_path: &str) -> String {
    let mut out = String::new();
    out.push_str(
        concat!("The K2 Apps **this agent maintains** (`", cli_name!(), " app inspect <app>`). Another agent's Apps aren't listed. \
         Regenerated whenever K2 rewrites AGENTS.md.\n\n"),
    );
    let Ok(ws) = resolve_project_id(project_path) else {
        out.push_str("_This folder isn't a registered workspace._\n");
        return out;
    };
    let db = crate::db::shared();
    let conn = db.lock();
    let apps = crate::deploy::store::list_apps(&conn, Some(&ws)).unwrap_or_default();
    if apps.is_empty() {
        out.push_str(concat!("_No Apps yet._ `", cli_name!(), " app install <id>` makes one; `", cli_name!(), " study apps` explains Apps.\n"));
        return out;
    }
    let now = crate::deploy::now();
    out.push_str("| APP | STATE | HOSTNAMES | LIVE SEED |\n| --- | --- | --- | --- |\n");
    let mut notes = Vec::new();
    for a in &apps {
        let cfg = crate::app_grace::config::load(&conn, &a.app_id).unwrap_or_default();
        let mut labels = vec![a.app_id.clone()];
        labels.extend(cfg.labels.iter().map(|l| l.label.clone()));
        labels.extend(cfg.subdomains.iter().map(|l| l.label.clone()));
        let seed = a.live_seed.as_deref().map(|s| s.chars().take(12).collect::<String>()).unwrap_or_else(|| "—".into());
        out.push_str(&format!("| {} | {} | {} | {seed} |\n", a.app_id, a.state, labels.join(", ")));
        if let Ok(Some(h)) = crate::deploy::handover::latest_for_app(&conn, &a.app_id) {
            let recent = h.to_workspace == ws && h.approved_at.is_some_and(|t| now - t < crate::deploy::handover::WINDOW_SECS);
            if recent && matches!(h.state.as_str(), "applied" | "final") {
                let from = crate::workspace_session_handles::workspace_address_name(&conn, &h.from_workspace).unwrap_or_else(|_| h.from_workspace.clone());
                notes.push(format!(
                    "- **{}** was handed over to you from {} ({} ago){}: `{CLI_NAME} app handover brief {}`. Their Thread history with guests stays with them.",
                    a.app_id,
                    if h.from_workspace.is_empty() { "the owner (parked)".to_string() } else { from },
                    human_age(now - h.approved_at.unwrap_or(now)),
                    if h.brief.is_some() { ", with a brief" } else { "" },
                    a.app_id
                ));
            }
        }
    }
    if !notes.is_empty() {
        out.push_str("\n### Recent handovers\n\n");
        out.push_str(&notes.join("\n"));
        out.push('\n');
    }
    out
}

fn human_age(secs: i64) -> String {
    if secs < 3600 {
        format!("{} min", (secs / 60).max(1))
    } else {
        format!("{} h", secs / 3600)
    }
}

struct RemoteConnSummary {
    agent: String,
    host: String,
    remote_addr: String,
    paired: bool,
}

fn list_remote_connection_summaries(project_path: &str) -> Vec<RemoteConnSummary> {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) =
        crate::workspace::agent_identity::resolve_project_id(&conn, project_path)
    else {
        return Vec::new();
    };
    let Ok(rows) =
        crate::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, &project_id)
    else {
        return Vec::new();
    };
    let mut out: Vec<RemoteConnSummary> = rows
        .into_iter()
        .map(|r| {
            let paired = r
                .peer_fingerprint
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_some();
            RemoteConnSummary {
                agent: r.agent,
                host: r.host,
                remote_addr: r.remote_addr,
                paired,
            }
        })
        .collect();
    out.sort_by(|a, b| a.agent.cmp(&b.agent).then_with(|| a.host.cmp(&b.host)));
    out
}

fn live_kind_meta(kind: LiveKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        LiveKind::Connections => (
            CONNECTIONS_ROSTER_PATH,
            "Connected agents roster",
            CONNECTIONS_ROSTER_SOURCE,
        ),
        LiveKind::Heartbeats => (
            HEARTBEATS_ROSTER_PATH,
            "Heartbeats roster",
            HEARTBEATS_ROSTER_SOURCE,
        ),
        LiveKind::Skills => (SKILLS_ROSTER_PATH, "Skills roster", SKILLS_ROSTER_SOURCE),
        LiveKind::Users => (USERS_ROSTER_PATH, "User roster", USERS_ROSTER_SOURCE),
        LiveKind::Skin => (SKIN_ROSTER_PATH, "App guest roster", SKIN_ROSTER_SOURCE),
        LiveKind::AppsMaintained => (APPS_MAINTAINED_PATH, "Apps you maintain", APPS_MAINTAINED_SOURCE),
    }
}

fn render_live_file(project_path: &str, kind: LiveKind) -> String {
    let (_, title, _) = live_kind_meta(kind);
    let body = match kind {
        LiveKind::Connections => render_connections_roster_body(project_path),
        LiveKind::Heartbeats => render_heartbeats_roster_body(project_path),
        LiveKind::Skills => render_skills_roster_body(project_path),
        LiveKind::Users => render_users_roster_body(project_path),
        LiveKind::Skin => render_skin_roster_body(project_path),
        LiveKind::AppsMaintained => render_apps_maintained_body(project_path),
    };
    format!("# {title}\n\n{body}")
}

/// Write/overwrite one live roster file from current workspace state.
fn sync_live_kind_file(project_path: &str, kind: LiveKind) -> Result<(), ContextError> {
    let (path, _, _) = live_kind_meta(kind);
    let rel = normalize_catalog_path(project_path, path)?;
    let abs = Path::new(project_path).join(&rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            ContextError::Db(format!(
                "cannot create context catalog directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    let body = render_live_file(project_path, kind);
    if fs::read(&abs).ok().as_deref() == Some(body.as_bytes()) {
        return Ok(());
    }
    fs::write(&abs, body).map_err(|e| {
        ContextError::Db(format!(
            "cannot write live context pack {}: {e}",
            abs.display()
        ))
    })?;
    Ok(())
}

/// Write/overwrite the connections roster file (compat helper).
pub fn sync_connections_roster_file(project_path: &str) -> Result<(), ContextError> {
    sync_live_kind_file(project_path, LiveKind::Connections)
}

/// If this project has any live roster layers stacked, rewrite their files.
/// Called from AGENTS.md publish so FileViewer matches compose.
pub fn sync_live_generated_layers(project_path: &str) {
    let Ok(layers) = list_layers(project_path) else {
        return;
    };
    let mut seen = [false; 6];
    for layer in &layers {
        if let Some(kind) = live_kind_for_layer(layer) {
            let idx = match kind {
                LiveKind::Connections => 0,
                LiveKind::Heartbeats => 1,
                LiveKind::Skills => 2,
                LiveKind::Users => 3,
                LiveKind::Skin => 4,
                LiveKind::AppsMaintained => 5,
            };
            if !seen[idx] {
                seen[idx] = true;
                let _ = sync_live_kind_file(project_path, kind);
            }
        }
    }
}

/// After connection graph changes: if either side stacks the connections roster,
/// rewrite AGENTS.md so the always-on roster reflects the new peers.
pub fn refresh_roster_after_connection_change(project_paths: &[&str]) {
    refresh_roster_after_live_kind_change(project_paths, LiveKind::Connections);
}

/// After a live-layer source mutates (connections / heartbeats / skills / users):
/// rewrite AGENTS.md only when that live kind is stacked.
pub fn refresh_roster_after_live_kind_change(project_paths: &[&str], kind: LiveKind) {
    for path in project_paths {
        if path.trim().is_empty() {
            continue;
        }
        let Ok(layers) = list_layers(path) else {
            continue;
        };
        if layers.iter().any(|l| live_kind_for_layer(l) == Some(kind)) {
            crate::workspace::skill_regen::write_workspace_skill_file(path);
        }
    }
}

/// After connect-user mutations: rewrite AGENTS.md only on workspaces
/// that stack the User roster. Host-wide people, stacked workspaces only.
/// Best-effort — never fail the user mutation.
pub fn refresh_users_roster_after_people_change() {
    let paths = match list_project_paths_with_users_roster() {
        Ok(p) => p,
        Err(e) => {
            crate::log_debug!("[context] users roster refresh skipped: {e}");
            return;
        }
    };
    if paths.is_empty() {
        return;
    }
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    refresh_roster_after_live_kind_change(&refs, LiveKind::Users);
}

fn list_project_paths_with_users_roster() -> Result<Vec<String>, ContextError> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT p.path \
             FROM project_context_layers l \
             JOIN projects p ON p.id = l.project_id \
             WHERE l.source = ?1 OR l.path LIKE '%users-roster.md'",
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![USERS_ROSTER_SOURCE], |r| r.get::<_, String>(0))
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(p) if !p.trim().is_empty() => out.push(p),
            Ok(_) => {}
            Err(e) => crate::log_debug!("[context] users roster path row skipped: {e}"),
        }
    }
    Ok(out)
}

/// After skin-user / pass mutations: rewrite AGENTS.md only on workspaces
/// that stack the Skin user roster. Host-wide guests, stacked workspaces only.
/// Best-effort — never fail the skin mutation.
pub fn refresh_skin_roster_after_people_change() {
    let paths = match list_project_paths_with_skin_roster() {
        Ok(p) => p,
        Err(e) => {
            crate::log_debug!("[context] skin roster refresh skipped: {e}");
            return;
        }
    };
    if paths.is_empty() {
        return;
    }
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    refresh_roster_after_live_kind_change(&refs, LiveKind::Skin);
}

fn list_project_paths_with_skin_roster() -> Result<Vec<String>, ContextError> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT p.path \
             FROM project_context_layers l \
             JOIN projects p ON p.id = l.project_id \
             WHERE l.source = ?1 OR l.path LIKE '%skin-roster.md'",
        )
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![SKIN_ROSTER_SOURCE], |r| r.get::<_, String>(0))
        .map_err(|e| ContextError::Db(e.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        match row {
            Ok(p) if !p.trim().is_empty() => out.push(p),
            Ok(_) => {}
            Err(e) => crate::log_debug!("[context] skin roster path row skipped: {e}"),
        }
    }
    Ok(out)
}

/// Lean always-on Manager layer — triage/delegate orientation, not the full skill.
const MANAGER_PACK_MD: &str = concat!(r#"# Workspace Manager (always-on)

Short standing orders for coordinating peers. Full playbook:
`.k2/skills/workspace-manager/SKILL.md` (load on demand).

## Role

You coordinate work across **connected workspaces** (peers, not sub-agents).
Your harness owns sub-agent/worktree spawn; K2 wires messaging and the tray.

## Every wake

1. `"#, cli_name!(), r#" inbox` and `"#, cli_name!(), r#" organization bulletin`: new work, then today's bulletin.
2. Triage live msgs + `"#, cli_name!(), r#" inbox` (act / file / reply / archive).
3. Prefer `"#, cli_name!(), r#" msg <ws> --inbox-wake <path>` for real work packages; live `msg` for short knocks.
4. Peek with `"#, cli_name!(), r#" read <ws>` before injecting into a peer.
5. `"#, cli_name!(), r#" organization update "…"` then `"#, cli_name!(), r#" done` (or `--blocked "…"`).

## Do not

- Dump full skill bodies into always-on context (toggle this layer off if unused).
- Treat peers as subordinates; connections are bidirectional.
- Use `"#, cli_name!(), r#" mail` for agent-to-agent work (that's email).
"#);

/// Lean always-on K2 Agent (planner) layer — planning orientation, not the full skill.
const K2_AGENT_PACK_MD: &str = concat!(r#"# K2 Agent / planner (always-on)

Short standing orders for planning. Full playbook:
`.k2/skills/k2-agent/SKILL.md` (load on demand).

## Role

You turn requests into **PRDs, milestones, and specs** — not implementation.
Write durable plans under `.k2/prds/` / `.k2/milestones/`; keep PROJECT.md current.

## Every wake

1. `"#, cli_name!(), r#" inbox` and `"#, cli_name!(), r#" organization bulletin`: new work, then today's bulletin.
2. Capture work with `"#, cli_name!(), r#" inbox compose` / read tray packages.
3. Plan in docs; register ship-ready items via inbox when useful.
4. Coordinate with `"#, cli_name!(), r#" msg` / `--inbox-wake`; keep live msgs short.
5. `"#, cli_name!(), r#" organization update "…"` then `"#, cli_name!(), r#" done`.

## Do not

- Implement large changes yourself when the plan belongs elsewhere.
- Bloat always-on context with full skill or PRD dumps — link paths instead.
- Skip writing decisions back to PROJECT.md / PRDs.
"#);

/// Lean always-on Kessel layer (0.46.12): what Kessel is for and how it asks.
const KESSEL_PACK_MD: &str = concat!(r#"# Kessel: K2's own agent (always-on)

You are Kessel, the agent that runs K2 for your person: setup, settings and
the agents on this server. The app is K2; you are its helper.

## How you work

- Learn a K2 command before you use it: `"#, cli_name!(), r#" study` lists the pages, and
  every verb takes `--help`. Don't guess flags.
- Make agents with `"#, cli_name!(), r#" agent hire --template <id>` (`"#, cli_name!(), r#" agent template list`
  shows the types). Ask your person before you hire.
- When K2 refuses you, ask on the card it prints. Never work around a
  refusal, and never ask for more than the task needs.
- Report what you changed on the Thread, and how you checked it.

## Do not

- Handle secrets in chat or the wiki: K2's vault and secret cards hold them.
- Change another agent's files, or speak for another agent.
- Paste long docs into always-on context; link the path instead.
"#);

/// Lean standing order: prefer subagent worktrees for heavy work.
const SUBAGENTS_PACK_MD: &str = r#"# Always use subagents

Always use subagents in worktrees to do your heavy work to protect your context window. Review their work and cherry pick their changes onto main when they are done.
"#;

/// Browser and computer use (0.46.11): the text of the user pack it replaces,
/// byte for byte (same H2 title, so a migrated workspace's file is unchanged).
const BROWSER_PACK_MD: &str = concat!(r##"## Browser and computer use

You can drive a web browser and this machine's desktop apps, not only a terminal. Two open-source CLIs, run through your shell, do it. Nothing needs to be "loaded". Remember this after every context compaction.

**Get them if they're missing** (`command -v agent-browser agent-desktop`):
- **agent-browser** (web, over Chrome's DevTools Protocol): https://github.com/vercel-labs/agent-browser, docs https://agent-browser.dev. Install with `npm install -g agent-browser` (or `brew install agent-browser`).
- **agent-desktop** (desktop apps, through the accessibility tree): https://github.com/vercel-labs/agent-desktop, docs https://agent-browser.dev/desktop. Install with `npm install -g agent-desktop` (or `brew install agent-desktop`).
- Installing software on a machine you don't own is the owner's call: ask with a permission card or your person rather than working around it.

**How to start.** Each CLI ships its own version-matched guide; read it before the first command:
- **Browser:**
  - Read `agent-browser skills get core --full` first.
  - Then the loop: `open <url>`, `read` / `snapshot` (gives @refs), `click`, `fill`.
  - Use your own session: `export AGENT_BROWSER_SESSION=<task-name>`.
  - **Signed-in work:** attach to the person's Chrome with `--auto-connect` or `--cdp <port>`. That needs Chrome running with `--remote-debugging-port`, so ask the person to start it. `--profile <name>` copies a Chrome profile, but logins often don't carry over.
- **Desktop:**
  - Read `agent-desktop skills get desktop` and `agent-desktop skills get platform` first.
  - Then the loop: `snapshot` (@refs), `find`, `click`, `type`, `screenshot`.
- **On a Linux server with no screen:** get your own display with `eval "$("##, cli_name!(), r##" desktop env)"` (it starts or wakes a desktop of your own and sets `DISPLAY`, `XAUTHORITY` and `K2_DESKTOP_PROFILE`), then run `agent-browser --headed …` or any GUI program in that shell; people watch it in a Desktop tab. Run it again within 30 minutes to keep it, `"##, cli_name!(), r##" desktop env --release` when done.
- It needs your workspace's "Allow a virtual desktop" switch (refused: run the `Ask with:` line it prints). Point a browser's profile at `$K2_DESKTOP_PROFILE` so its logins stay on your desktop. More: `"##, cli_name!(), r##" study desktop`.

**Rules:**
- **Focus:** if you ever have to take focus (bring a window or app to the front, or click with a physical, headed pointer), give it back to the window that had it before, in the same step. Never leave the person's focus somewhere else.
- **Forms:** read web forms by field names and whether they're filled, never their values. A credential field isn't always `type=password`.
- **Passwords:** never type them. Secrets come from a secret card or the vault, read inside your script and never echoed.
- **Irreversible clicks:** ask your person before any irreversible or outward-facing click: submit, pay, publish, delete, send.
- **Clean up:** close the tabs, sessions (`agent-browser close`) and windows you open.
"##);

/// Lean wiki hygiene — how to keep `.k2/wiki/` healthy without bloating AGENTS.md.
const WIKI_HYGIENE_PACK_MD: &str = r#"# Wiki hygiene (always-on)

Standing orders for this workspace’s **knowledge vault** at `.k2/wiki/`.
This is user-owned brain matter — not host control docs, not a dump into AGENTS.md.

## Principles

1. **Durable facts → wiki notes.** Chat and AGENT.md are for persona / standing orders; lasting knowledge goes under `.k2/wiki/`.
2. **Link, don’t paste.** Prefer `[[wikilinks]]` and short pointers. Do not paste long vault bodies into AGENTS.md, PROJECT.md, or persona.
3. **Index is the map.** Keep `.k2/wiki/_Index.md` (and `Home.md` if you use it) current when you add or rename notes.
4. **No orphans.** New notes should be reachable from Index/Home or a parent note. Fix broken `[[links]]` when you notice them.
5. **Titles + aliases.** Use clear H1 titles; frontmatter `aliases` / `tags` when useful for discovery.
6. **Seed if empty.** If the vault is missing, seed Index/Home (Settings → Workspaces / Agents → <agent> → Context → Seed wiki, or create the files), then grow from there.

## Every meaningful wiki edit

1. Write/update the note under `.k2/wiki/`.
2. Link it from `_Index.md` (and any parent topic note).
3. Fix renames: update inbound links; leave a stub only if something external still points at the old path.
4. Prefer many small linked notes over one giant page.

## Do not

- Treat AGENTS.md as a second wiki (it regenerates; the vault is the SSOT for notes).
- Stack entire skill bodies or full PRD dumps as “wiki” substitutes.
- Invent a parallel notes tree outside `.k2/wiki/` for agent-shared knowledge.
"#;

fn known_catalog_ids() -> String {
    list_catalog()
        .into_iter()
        .map(|p| p.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve a catalog id or alias to its entry (agent templates check their
/// `[context] catalog` ids with it; prd-agent-templates-v1 §2.3).
pub fn resolve_catalog_entry(catalog_id: &str) -> Result<ContextCatalogEntry, ContextError> {
    resolve_catalog_id(catalog_id)
}

fn resolve_catalog_id(catalog_id: &str) -> Result<ContextCatalogEntry, ContextError> {
    let id = catalog_id.trim();
    // Accept short aliases (product language + skill dir names).
    let canonical = match id {
        "manager" | "workspace-manager" | "catalog:manager" => "manager:pack",
        // AT9: every old K2 Agent spelling is Kessel's pack now.
        "k2:pack" | "k2-agent" | "k2" | "k2so-agent" | "catalog:k2-agent" | "kessel" | "catalog:kessel" => KESSEL_PACK_ID,
        "connections"
        | "roster"
        | "connections-roster"
        | "connected-agents"
        | "catalog:connections-roster" => CONNECTIONS_ROSTER_ID,
        "heartbeats" | "heartbeats-roster" | "catalog:heartbeats-roster" => HEARTBEATS_ROSTER_ID,
        "skills" | "skills-roster" | "skills-index" | "catalog:skills-roster" => SKILLS_ROSTER_ID,
        "users:roster"
        | "users"
        | "users-roster"
        | "people"
        | "people-roster"
        | "user-roster"
        | "catalog:users-roster" => USERS_ROSTER_ID,
        "apps:roster"
        | "apps-roster"
        | "apps"
        | "skin:roster"
        | "skin-roster"
        | "skins-roster"
        | "skin-users"
        | "catalog:skin-roster" => APPS_ROSTER_ID,
        "apps:maintained" | "apps-maintained" | "catalog:apps-maintained" => APPS_MAINTAINED_ID,
        "wiki-hygiene" | "hygiene" | "catalog:wiki-hygiene" => WIKI_HYGIENE_ID,
        "subagents"
        | "subagents:pack"
        | "always-use-subagents"
        | "use-subagents"
        | "catalog:subagents" => SUBAGENTS_PACK_ID,
        // The old user pack's ids name the built-in now (0.46.11).
        "browser"
        | "browser:pack"
        | "browser-and-computer-use"
        | "catalog:browser"
        | "user:browser-and-computer-use"
        | OLD_BROWSER_USER_PACK_SOURCE => BROWSER_PACK_ID,
        other => other,
    };
    let catalog = list_catalog();
    catalog
        .iter()
        .find(|p| p.id == canonical || p.source == canonical)
        .cloned()
        .or_else(|| {
            // Host user packs: `user:<slug>` or `catalog:user:<slug>`.
            let slug = canonical
                .strip_prefix("catalog:user:")
                .or_else(|| canonical.strip_prefix("user:"));
            slug.and_then(|s| {
                catalog.iter().find(|p| p.id == user_pack_id(s)).cloned()
            })
        })
        .ok_or_else(|| {
            ContextError::CatalogUnknown(format!(
                "unknown catalog id '{catalog_id}'; known: {}",
                known_catalog_ids()
            ))
        })
}

/// The text K2 writes for a static built-in pack (the manager pack,
/// Kessel's, wiki hygiene, subagents, browser); `None` for live, path and
/// user packs. Agent templates keep these up to date through the lock's
/// checksums (prd-agent-templates-v1 AT10).
pub fn static_pack_body(source: &str) -> Option<&'static str> {
    Some(match source {
        "catalog:manager" => MANAGER_PACK_MD,
        KESSEL_PACK_SOURCE => KESSEL_PACK_MD,
        WIKI_HYGIENE_SOURCE => WIKI_HYGIENE_PACK_MD,
        SUBAGENTS_PACK_SOURCE => SUBAGENTS_PACK_MD,
        BROWSER_PACK_SOURCE => BROWSER_PACK_MD,
        _ => return None,
    })
}

fn is_materializing_pack(entry: &ContextCatalogEntry) -> bool {
    if slug_from_user_source(&entry.source).is_some() {
        return true;
    }
    matches!(
        entry.source.as_str(),
        "catalog:manager"
            | "catalog:k2-agent"
            | KESSEL_PACK_SOURCE
            | WIKI_HYGIENE_SOURCE
            | SUBAGENTS_PACK_SOURCE
            | BROWSER_PACK_SOURCE
            | CONNECTIONS_ROSTER_SOURCE
            | HEARTBEATS_ROSTER_SOURCE
            | SKILLS_ROSTER_SOURCE
            | USERS_ROSTER_SOURCE
            | APPS_MAINTAINED_SOURCE
    )
}

/// Write lean always-on pack + ensure the matching loadable skill exists.
/// Manager/K2/wiki-hygiene: idempotent (does not overwrite existing user-edited packs).
/// Live rosters: always rewritten from current registry / disk state.
fn materialize_pack_if_needed(
    project_path: &str,
    entry: &ContextCatalogEntry,
    rel_path: &str,
) -> Result<(), ContextError> {
    if !is_materializing_pack(entry) {
        return Ok(());
    }

    if let Some(kind) = live_kind_for_source(&entry.source) {
        let _ = rel_path;
        return sync_live_kind_file(project_path, kind);
    }

    if let Some(slug) = slug_from_user_source(&entry.source) {
        return copy_host_user_pack_layer(project_path, slug, rel_path);
    }

    let abs = Path::new(project_path).join(rel_path);
    if !abs.is_file() {
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                ContextError::Db(format!(
                    "cannot create context catalog directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let body = match entry.source.as_str() {
            "catalog:manager" => MANAGER_PACK_MD,
            // An old row whose file went missing gets the old body back.
            "catalog:k2-agent" => K2_AGENT_PACK_MD,
            KESSEL_PACK_SOURCE => KESSEL_PACK_MD,
            WIKI_HYGIENE_SOURCE => WIKI_HYGIENE_PACK_MD,
            SUBAGENTS_PACK_SOURCE => SUBAGENTS_PACK_MD,
            BROWSER_PACK_SOURCE => BROWSER_PACK_MD,
            _ => return Ok(()),
        };
        fs::write(&abs, body).map_err(|e| {
            ContextError::Db(format!("cannot write context pack {}: {e}", abs.display()))
        })?;
    }

    // Full generators remain the loadable skills (depth on demand), not always-on.
    let opt_in = match entry.source.as_str() {
        "catalog:manager" => Some(crate::skills::content::OptInSkill::WorkspaceManager),
        "catalog:k2-agent" => Some(crate::skills::content::OptInSkill::K2Agent),
        _ => None,
    };
    if let Some(skill) = opt_in {
        let _ = crate::skills::content::write_opt_in_skill(project_path, skill);
    }
    Ok(())
}

// ── Project resolution ────────────────────────────────────────────────

/// Resolve a registered project's `id` from its absolute path.
/// Fails if the workspace is not registered in `projects`.
pub fn resolve_project_id(project_path: &str) -> Result<String, ContextError> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::workspace::agent_identity::resolve_project_id(&conn, project_path)
        .ok_or_else(|| ContextError::NotFound(format!("workspace not registered: {project_path}")))
}

// ── Path rules ────────────────────────────────────────────────────────

/// Normalize a path for storage: workspace-relative with `/` separators.
/// Rejects escape outside the workspace root.
pub fn normalize_layer_path(project_path: &str, raw: &str) -> Result<String, ContextError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ContextError::BadUsage("path must not be empty".into()));
    }

    let root = Path::new(project_path);
    let root_canon = dunce::canonicalize(&root)
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
        dunce::canonicalize(&cleaned)
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
        let base = dunce::canonicalize(&existing)
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

    // Role persona
    let agent_rel = if let Some(primary) = find_primary_agent(project_path) {
        let abs =
            crate::workspace::agent_identity::persona_md_in(agent_dir(project_path, &primary));
        let root = Path::new(project_path);
        abs.strip_prefix(root)
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_else(|_| ".k2/agent/ROLE.md".into())
    } else {
        // Prefer actual dot-dir name when present; helper prefers ROLE.md.
        let agent_md = crate::workspace::agent_identity::persona_md_in(
            crate::workspace_dot_dir(project_path).join("agent"),
        );
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
                .unwrap_or_else(|_| ".k2/agent/ROLE.md".into())
        } else {
            ".k2/agent/ROLE.md".into()
        }
    };
    let (exists, bytes) = disk_meta(project_path, &agent_rel);
    out.push(PinnedLayer {
        id: "pinned:agent".into(),
        path: agent_rel,
        label: "Role (persona)".into(),
        exists,
        bytes,
        generated: false,
        enabled: inc_agent,
        editable: true,
        preview: None,
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
        preview: None,
    });

    // Tooling footer (generated k2-cli pointer)
    out.push(PinnedLayer {
        id: "pinned:tooling".into(),
        path: String::new(),
        label: "Tooling (k2-cli pointer)".into(),
        exists: true,
        bytes: crate::workspace::skill_regen::AGENTS_MD_TOOLING_SECTION.len() as u64,
        generated: true,
        enabled: inc_tooling,
        editable: false,
        preview: Some(
            crate::workspace::skill_regen::AGENTS_MD_TOOLING_SECTION
                .trim_end()
                .to_string(),
        ),
    });

    out
}

/// True when a layer id is a system (pinned) toggle: `pinned:agent|project|tooling`.
pub fn is_system_layer_id(id: &str) -> bool {
    matches!(
        id,
        "pinned:agent" | "pinned:project" | "pinned:tooling" | "agent" | "project" | "tooling"
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

/// 0.46.11: "Browser and computer use" was a user pack before it shipped
/// built in, under the same slug. Its user pack is now skipped (reserved
/// slug), so a workspace that stacked `user:browser-and-computer-use` gets
/// its row moved to the built-in source. The path and the file on disk are
/// the same, so nothing is rewritten; the row just points at a pack that
/// exists. Idempotent, and a no-op once no old rows are left.
fn migrate_old_browser_user_pack_rows(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<(), ContextError> {
    conn.execute(
        "UPDATE project_context_layers SET source = ?1 \
         WHERE project_id = ?2 AND source = ?3",
        params![BROWSER_PACK_SOURCE, project_id, OLD_BROWSER_USER_PACK_SOURCE],
    )
    .map_err(|e| ContextError::Db(e.to_string()))?;
    Ok(())
}

/// List optional layers for a registered project (all, ordered by position).
pub fn list_layers(project_path: &str) -> Result<Vec<ContextLayer>, ContextError> {
    let project_id = resolve_project_id(project_path)?;
    let db = crate::db::shared();
    let conn = db.lock();
    migrate_old_browser_user_pack_rows(&conn, &project_id)?;
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
    // Keep on-disk roster fresh when the Settings UI polls the stack.
    sync_live_generated_layers(project_path);
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
        if is_live_generated_layer(l) {
            total += 800; // live roster ballpark
        } else if l.exists {
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

/// Add a layer by path or catalog id. Regenerates AGENTS.md on success.
pub fn add_layer(
    project_path: &str,
    path: Option<&str>,
    catalog: Option<&str>,
    label: Option<&str>,
) -> Result<ContextLayer, ContextError> {
    let has_path = path.map(|p| !p.trim().is_empty()).unwrap_or(false);
    let has_catalog = catalog.map(|p| !p.trim().is_empty()).unwrap_or(false);

    if has_path == has_catalog {
        return Err(ContextError::BadUsage(
            "provide exactly one of path or catalog".into(),
        ));
    }

    let (rel_path, source, default_label) = if has_catalog {
        let p = resolve_catalog_id(catalog.unwrap().trim())?;
        // Catalog paths are fixed relative strings; still normalize to confirm
        // they land under the workspace (and rewrite `.k2/` vs `.k2so/` if needed).
        let rel = normalize_catalog_path(project_path, &p.path)?;
        // Manager / K2 packs: materialize generated role markdown on first add.
        materialize_pack_if_needed(project_path, &p, &rel)?;
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
fn normalize_catalog_path(project_path: &str, catalog_path: &str) -> Result<String, ContextError> {
    let rel = catalog_path.trim_start_matches("./");
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
            return Err(ContextError::BadUsage("position must be >= 0".into()));
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
/// Live-generated layers rebuild from registry/disk state — they never depend
/// on a stale on-disk body for AGENTS.md correctness.
pub fn read_layer_body(project_path: &str, layer: &ContextLayer) -> Option<String> {
    if is_live_generated_layer(layer) {
        let body = render_live_layer_body(project_path, layer)?;
        let body = body.trim();
        if body.is_empty() {
            None
        } else {
            Some(body.to_string())
        }
    } else {
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
#[path = "context_layers_tests.rs"]
mod tests;
