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

pub const WIKI_HYGIENE_PATH: &str = ".k2/context/catalog/wiki-hygiene.md";
pub const WIKI_HYGIENE_SOURCE: &str = "catalog:wiki-hygiene";
pub const WIKI_HYGIENE_ID: &str = "wiki:hygiene";

pub const SUBAGENTS_PACK_PATH: &str = ".k2/context/catalog/always-use-subagents.md";
pub const SUBAGENTS_PACK_SOURCE: &str = "catalog:subagents";
pub const SUBAGENTS_PACK_ID: &str = "subagents:pack";

/// Which live-generated pack a layer is (if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveKind {
    Connections,
    Heartbeats,
    Skills,
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
    }
}

/// Built-in catalog entries: wiki seeds + lean packs + live rosters.
///
/// Live rosters (connections / heartbeats / skills) rebuild on every AGENTS.md
/// compose so always-on context tracks a changing workspace.
pub fn list_catalog() -> Vec<ContextCatalogEntry> {
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
        builtin_catalog_entry(
            "k2:pack",
            ".k2/context/catalog/k2-agent.md",
            "K2 Agent",
            "catalog:k2-agent",
            "static",
            "Lean always-on planner orientation. Full K2 Agent playbook stays a loadable skill.",
            Some("1.0.0"),
            false,
            &["role", "planner"],
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
    ]
}

fn live_kind_for_source(source: &str) -> Option<LiveKind> {
    match source {
        CONNECTIONS_ROSTER_SOURCE => Some(LiveKind::Connections),
        HEARTBEATS_ROSTER_SOURCE => Some(LiveKind::Heartbeats),
        SKILLS_ROSTER_SOURCE => Some(LiveKind::Skills),
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
    }
}

/// Markdown body for the connections roster (no leading H1 — compose adds `## label`).
pub fn render_connections_roster_body(project_path: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "Live roster of **connected workspace-agents** (peers, not sub-agents). \
         Regenerated whenever K2 rewrites AGENTS.md.\n\n",
    );
    out.push_str("Message / peek:\n\n");
    out.push_str("    k2 msg <workspace-name> \"short live knock\"\n");
    out.push_str("    k2 msg <workspace-name> --inbox-wake <path> [path…]\n");
    out.push_str("    k2 read <workspace-name>\n");
    out.push_str("    k2 connections list\n\n");

    let local = crate::connections::list_peers(project_path).unwrap_or_default();
    let remotes = list_remote_connection_summaries(project_path);

    if local.is_empty() && remotes.is_empty() {
        out.push_str("### No connected agents yet\n\n");
        out.push_str("Wire a peer with:\n\n");
        out.push_str("    k2 connections add <other-workspace-path>\n");
        out.push_str("    k2 connections add agent::host.k2.dev   # remote form\n");
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
        "To update: `k2 connections add|remove …` — this layer rewrites on the next AGENTS.md regen \
         and when connections change.\n",
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
    out.push_str("    k2 heartbeat list\n");
    out.push_str("    k2 heartbeat signal wakeup <name>\n");
    out.push_str("    k2 heartbeat enable|disable <name>\n\n");
    out.push_str(
        "WAKEUP.md is the **user message** for that schedule — do not paste full wake \
         bodies into AGENTS.md; edit the path listed below.\n\n",
    );

    let rows = crate::heartbeats::k2so_heartbeat_list(project_path.to_string()).unwrap_or_default();
    if rows.is_empty() {
        out.push_str("### No heartbeats yet\n\n");
        out.push_str("Create one in Settings → Heartbeats, or:\n\n");
        out.push_str("    k2 heartbeat create <name> --every 1h\n");
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
        if let Some(ref reason) = hb.disabled_reason {
            if !reason.is_empty() {
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
    }
}

fn render_live_file(project_path: &str, kind: LiveKind) -> String {
    let (_, title, _) = live_kind_meta(kind);
    let body = match kind {
        LiveKind::Connections => render_connections_roster_body(project_path),
        LiveKind::Heartbeats => render_heartbeats_roster_body(project_path),
        LiveKind::Skills => render_skills_roster_body(project_path),
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
    let mut seen = [false; 3];
    for layer in &layers {
        if let Some(kind) = live_kind_for_layer(layer) {
            let idx = match kind {
                LiveKind::Connections => 0,
                LiveKind::Heartbeats => 1,
                LiveKind::Skills => 2,
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

/// After a live-layer source mutates (connections / heartbeats / skills):
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

/// Lean always-on Manager layer — triage/delegate orientation, not the full skill.
const MANAGER_PACK_MD: &str = r#"# Workspace Manager (always-on)

Short standing orders for coordinating peers. Full playbook:
`.k2/skills/workspace-manager/SKILL.md` (load on demand).

## Role

You coordinate work across **connected workspaces** (peers, not sub-agents).
Your harness owns sub-agent/worktree spawn; K2 wires messaging and the tray.

## Every wake

1. `k2 checkin` — messages, inbox, reviews, activity.
2. Triage live msgs + `k2 inbox` (act / file / reply / archive).
3. Prefer `k2 msg <ws> --inbox-wake <path>` for real work packages; live `msg` for short knocks.
4. Peek with `k2 read <ws>` before injecting into a peer.
5. `k2 checkin --status "…"` then `k2 done` (or `--blocked "…"`).

## Do not

- Dump full skill bodies into always-on context (toggle this layer off if unused).
- Treat peers as subordinates; connections are bidirectional.
- Use `k2 mail` for agent-to-agent work (that's email).
"#;

/// Lean always-on K2 Agent (planner) layer — planning orientation, not the full skill.
const K2_AGENT_PACK_MD: &str = r#"# K2 Agent / planner (always-on)

Short standing orders for planning. Full playbook:
`.k2/skills/k2-agent/SKILL.md` (load on demand).

## Role

You turn requests into **PRDs, milestones, and specs** — not implementation.
Write durable plans under `.k2/prds/` / `.k2/milestones/`; keep PROJECT.md current.

## Every wake

1. `k2 checkin` — inbox, peers, reviews.
2. Capture work with `k2 inbox compose` / read tray packages.
3. Plan in docs; register ship-ready items via inbox when useful.
4. Coordinate with `k2 msg` / `--inbox-wake`; keep live msgs short.
5. `k2 checkin --status "…"` then `k2 done`.

## Do not

- Implement large changes yourself when the plan belongs elsewhere.
- Bloat always-on context with full skill or PRD dumps — link paths instead.
- Skip writing decisions back to PROJECT.md / PRDs.
"#;

/// Lean standing order: prefer subagent worktrees for heavy work.
const SUBAGENTS_PACK_MD: &str = r#"# Always use subagents

Always use subagents in worktrees to do your heavy work to protect your context window. Review their work and cherry pick their changes onto main when they are done.
"#;

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
6. **Seed if empty.** If the vault is missing, seed Index/Home (Settings → Context → Seed wiki, or create the files), then grow from there.

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

fn resolve_catalog_id(catalog_id: &str) -> Result<ContextCatalogEntry, ContextError> {
    let id = catalog_id.trim();
    // Accept short aliases (product language + skill dir names).
    let canonical = match id {
        "manager" | "workspace-manager" | "catalog:manager" => "manager:pack",
        "k2-agent" | "k2" | "k2so-agent" | "catalog:k2-agent" => "k2:pack",
        "connections"
        | "roster"
        | "connections-roster"
        | "connected-agents"
        | "catalog:connections-roster" => CONNECTIONS_ROSTER_ID,
        "heartbeats" | "heartbeats-roster" | "catalog:heartbeats-roster" => HEARTBEATS_ROSTER_ID,
        "skills" | "skills-roster" | "skills-index" | "catalog:skills-roster" => SKILLS_ROSTER_ID,
        "wiki-hygiene" | "hygiene" | "catalog:wiki-hygiene" => WIKI_HYGIENE_ID,
        "subagents"
        | "subagents:pack"
        | "always-use-subagents"
        | "use-subagents"
        | "catalog:subagents" => SUBAGENTS_PACK_ID,
        other => other,
    };
    list_catalog()
        .into_iter()
        .find(|p| p.id == canonical)
        .ok_or_else(|| {
            ContextError::CatalogUnknown(format!(
                "unknown catalog id '{catalog_id}'; known: {}",
                known_catalog_ids()
            ))
        })
}

fn is_materializing_pack(entry: &ContextCatalogEntry) -> bool {
    matches!(
        entry.source.as_str(),
        "catalog:manager"
            | "catalog:k2-agent"
            | WIKI_HYGIENE_SOURCE
            | SUBAGENTS_PACK_SOURCE
            | CONNECTIONS_ROSTER_SOURCE
            | HEARTBEATS_ROSTER_SOURCE
            | SKILLS_ROSTER_SOURCE
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
            "catalog:k2-agent" => K2_AGENT_PACK_MD,
            WIKI_HYGIENE_SOURCE => WIKI_HYGIENE_PACK_MD,
            SUBAGENTS_PACK_SOURCE => SUBAGENTS_PACK_MD,
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
        label: "Role".into(),
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
        assert_eq!(stack.pinned[0].label, "Role");
        assert!(
            stack.pinned[0].path.ends_with("ROLE.md"),
            "default pinned path is ROLE.md, got {}",
            stack.pinned[0].path
        );
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

        let layer = add_layer(path, Some("docs/arch.md"), None, Some("Arch")).expect("add layer");
        assert_eq!(layer.path, "docs/arch.md");
        assert!(layer.enabled);
        assert_eq!(layer.position, 0);
        assert_eq!(layer.source, "user");
        assert_eq!(layer.label.as_deref(), Some("Arch"));
        assert!(layer.exists);
        assert!(layer.bytes > 0);

        let err =
            add_layer(path, Some("docs/arch.md"), None, None).expect_err("duplicate must fail");
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

        let err =
            add_layer(path, Some("../../etc/passwd"), None, None).expect_err("escape must fail");
        assert_eq!(err.code(), "path_escape", "got {err}");

        cleanup_project(path, &pid);
    }

    #[test]
    fn catalog_wiki_index_adds_and_rewrites_source() {
        let root = unique_root("catalog");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        fs::write(
            root.join(".k2/wiki/_Index.md"),
            "# Wiki Index\n\n- note a\n",
        )
        .unwrap();

        let layer = add_layer(path, None, Some("wiki:index"), None).expect("catalog add");
        assert_eq!(layer.source, "catalog:wiki-index");
        assert_eq!(layer.path, ".k2/wiki/_Index.md");
        assert_eq!(layer.label.as_deref(), Some("Wiki index"));
        assert!(layer.exists);

        let err = add_layer(path, None, Some("wiki:nope"), None).expect_err("unknown");
        assert_eq!(err.code(), "catalog_unknown");

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
        let err = list_stack("/tmp/k2-ctx-definitely-not-registered-xyz").expect_err("must fail");
        assert_eq!(err.code(), "not_found");
    }

    #[test]
    fn list_catalog_has_wiki_entries() {
        let catalog = list_catalog();
        assert!(catalog.iter().any(|p| p.id == "wiki:index"));
        assert!(catalog.iter().any(|p| p.id == "wiki:home"));
        assert!(catalog.iter().any(|p| p.id == WIKI_HYGIENE_ID));
        assert!(catalog.iter().any(|p| p.id == SUBAGENTS_PACK_ID));
        assert!(catalog.iter().any(|p| p.id == "manager:pack"));
        assert!(catalog.iter().any(|p| p.id == "k2:pack"));
        assert!(catalog.iter().any(|p| p.id == CONNECTIONS_ROSTER_ID));
        assert!(catalog.iter().any(|p| p.id == HEARTBEATS_ROSTER_ID));
        assert!(catalog.iter().any(|p| p.id == SKILLS_ROSTER_ID));
        for p in &catalog {
            assert!(
                matches!(p.kind.as_str(), "live" | "static" | "path"),
                "catalog entry {} bad kind {}",
                p.id,
                p.kind
            );
            assert!(
                p.description
                    .as_ref()
                    .map(|d| !d.is_empty())
                    .unwrap_or(false),
                "catalog entry {} missing description",
                p.id
            );
            assert_eq!(p.author.as_deref(), Some("K2"));
            if p.kind == "static" || p.kind == "path" {
                assert_eq!(
                    p.version.as_deref(),
                    Some("1.0.0"),
                    "static/path need version"
                );
            }
            if p.kind == "live" {
                assert!(p.version.is_none(), "live packs have no version pin");
            }
            assert!(
                !p.tags.is_empty(),
                "catalog entry {} should have tags",
                p.id
            );
            assert!(
                !p.tags.iter().any(|t| t.eq_ignore_ascii_case("recommended")),
                "recommended must be a boolean field, not a free-form tag ({})",
                p.id
            );
        }
        let recommended: Vec<_> = catalog
            .iter()
            .filter(|p| p.recommended)
            .map(|p| p.id.as_str())
            .collect();
        assert!(recommended.contains(&"wiki:index"));
        assert!(recommended.contains(&"wiki:hygiene"));
        assert!(recommended.contains(&SUBAGENTS_PACK_ID));
        assert!(recommended.contains(&CONNECTIONS_ROSTER_ID));
        assert!(recommended.contains(&HEARTBEATS_ROSTER_ID));
        assert!(!recommended.contains(&"skills:roster"));
        assert!(!recommended.contains(&"manager:pack"));
    }

    #[test]
    fn connections_roster_catalog_materializes_and_composes_live() {
        let root = unique_root("roster");
        let path = root.to_str().unwrap();
        let _pid = register_project(path);

        let layer = add_layer(path, None, Some("connections:roster"), None).expect("roster");
        assert_eq!(layer.source, CONNECTIONS_ROSTER_SOURCE);
        assert_eq!(layer.path, CONNECTIONS_ROSTER_PATH);
        assert!(layer.exists);
        let abs = root.join(".k2/context/catalog/connections-roster.md");
        assert!(abs.is_file(), "roster file must materialize");
        let body = fs::read_to_string(&abs).unwrap();
        assert!(
            body.contains("No connected agents yet") || body.contains("Local connections"),
            "roster should describe connections; first 300:\n{}",
            &body[..body.len().min(300)]
        );

        // Compose must include the roster section even if the file is deleted
        // (live generation path).
        fs::remove_file(&abs).ok();
        let composed = show_composed(path).expect("compose");
        assert!(
            composed.contains("Connected agents roster")
                || composed.contains("No connected agents yet"),
            "composed AGENTS.md must inline live roster; first 500:\n{}",
            &composed[..composed.len().min(500)]
        );

        cleanup_project(path, &_pid);
    }

    #[test]
    fn wiki_hygiene_and_live_rosters_materialize() {
        let root = unique_root("hygiene-rosters");
        let path = root.to_str().unwrap();
        let _pid = register_project(path);

        let hygiene = add_layer(path, None, Some("wiki:hygiene"), None).expect("hygiene");
        assert_eq!(hygiene.source, WIKI_HYGIENE_SOURCE);
        let hygiene_body =
            fs::read_to_string(root.join(".k2/context/catalog/wiki-hygiene.md")).unwrap();
        assert!(
            hygiene_body.contains("Wiki hygiene") && hygiene_body.contains(".k2/wiki/"),
            "hygiene pack body unexpected: {}",
            &hygiene_body[..hygiene_body.len().min(200)]
        );

        let hb = add_layer(path, None, Some("heartbeats:roster"), None).expect("hb roster");
        assert_eq!(hb.source, HEARTBEATS_ROSTER_SOURCE);
        assert!(root
            .join(".k2/context/catalog/heartbeats-roster.md")
            .is_file());

        let sk = add_layer(path, None, Some("skills:roster"), None).expect("skills roster");
        assert_eq!(sk.source, SKILLS_ROSTER_SOURCE);
        assert!(root.join(".k2/context/catalog/skills-roster.md").is_file());

        let composed = show_composed(path).expect("compose");
        assert!(
            composed.contains("Wiki hygiene")
                && composed.contains("Heartbeats roster")
                && composed.contains("Skills roster"),
            "compose missing new packs; first 800:\n{}",
            &composed[..composed.len().min(800)]
        );

        cleanup_project(path, &_pid);
    }

    fn compose_banner_stamp(body: &str) -> &str {
        let marker = "<!-- GENERATED by K2 at ";
        let i = body.find(marker).expect("compose banner");
        let rest = &body[i + marker.len()..];
        let end = rest.find(' ').expect("stamp end");
        &rest[..end]
    }

    #[test]
    fn heartbeat_mutate_refreshes_when_roster_stacked_fire_does_not() {
        crate::db::init_for_tests();
        let root = unique_root("hb-roster");
        let path = root.to_str().unwrap();
        let pid = register_project(path);
        crate::workspace::onboarding::set_agents_md_generate_enabled(path, true).unwrap();
        add_layer(path, None, Some("heartbeats:roster"), None).expect("stack hb roster");
        crate::workspace::skill_regen::write_workspace_skill_file(path);

        crate::heartbeats::k2so_heartbeat_add(
            path.to_string(),
            "roster-hb".into(),
            "daily".into(),
            "{}".into(),
        )
        .expect("add heartbeat");

        let cwd = fs::read_to_string(root.join("AGENTS.md")).expect("cwd AGENTS.md");
        assert!(
            cwd.contains("roster-hb"),
            "heartbeat add with roster stacked must refresh AGENTS.md, got:\n{cwd}"
        );
        let stamp = compose_banner_stamp(&cwd).to_string();

        crate::heartbeats::stamp_heartbeat_fired(path, "roster-hb");
        let after_fire = fs::read_to_string(root.join("AGENTS.md")).expect("after fire");
        assert_eq!(
            compose_banner_stamp(&after_fire),
            stamp.as_str(),
            "heartbeat fire must not rewrite the compose banner"
        );

        cleanup_project(path, &pid);
    }

    #[test]
    fn skill_write_refreshes_only_when_skills_roster_stacked() {
        crate::db::init_for_tests();
        let stacked = unique_root("sk-roster-on");
        let stacked_path = stacked.to_str().unwrap();
        let stacked_pid = register_project(stacked_path);
        crate::workspace::onboarding::set_agents_md_generate_enabled(stacked_path, true).unwrap();
        add_layer(stacked_path, None, Some("skills:roster"), None).expect("stack skills");
        crate::workspace::skill_regen::write_workspace_skill_file(stacked_path);

        crate::skills::crud::create(stacked_path, "roster-skill", None).expect("create skill");
        let stacked_body =
            fs::read_to_string(stacked.join("AGENTS.md")).expect("stacked cwd AGENTS.md");
        assert!(
            stacked_body.contains("roster-skill"),
            "skill write with roster stacked must refresh AGENTS.md, got:\n{stacked_body}"
        );

        let bare = unique_root("sk-roster-off");
        let bare_path = bare.to_str().unwrap();
        let bare_pid = register_project(bare_path);
        crate::workspace::onboarding::set_agents_md_generate_enabled(bare_path, true).unwrap();
        crate::workspace::skill_regen::write_workspace_skill_file(bare_path);
        let before = fs::read_to_string(bare.join("AGENTS.md")).expect("bare cwd");
        let stamp = compose_banner_stamp(&before).to_string();
        crate::skills::crud::create(bare_path, "quiet-skill", None).expect("create without layer");
        let after = fs::read_to_string(bare.join("AGENTS.md")).expect("bare after");
        assert_eq!(
            compose_banner_stamp(&after),
            stamp.as_str(),
            "skill write without the skills roster must not restamp AGENTS.md"
        );

        cleanup_project(stacked_path, &stacked_pid);
        cleanup_project(bare_path, &bare_pid);
    }

    #[test]
    fn pack_catalog_materialize_on_first_add() {
        let root = unique_root("pack");
        let path = root.to_str().unwrap();
        let _pid = register_project(path);

        let manager_abs = root.join(".k2/context/catalog/manager.md");
        assert!(!manager_abs.exists());

        let layer = add_layer(path, None, Some("manager:pack"), None).expect("manager pack");
        assert_eq!(layer.source, "catalog:manager");
        assert_eq!(layer.path, ".k2/context/catalog/manager.md");
        assert!(manager_abs.is_file(), "manager pack must materialize");
        assert!(layer.exists);
        let body = fs::read_to_string(&manager_abs).unwrap();
        assert!(
            body.contains("always-on") || body.contains("standing"),
            "pack should be lean always-on orientation, got len={}",
            body.len()
        );
        assert!(
            body.len() < 4_000,
            "always-on pack must stay small (not full skill dump), len={}",
            body.len()
        );
        // Full skill remains loadable for depth.
        assert!(
            root.join(".k2/skills/workspace-manager/SKILL.md").is_file()
                || root
                    .join(".k2so/skills/workspace-manager/SKILL.md")
                    .is_file(),
            "loadable workspace-manager skill should be ensured on pack add"
        );

        let err = add_layer(path, None, Some("manager:pack"), None).expect_err("dup");
        assert_eq!(err.code(), "duplicate_layer");

        let k2_layer = add_layer(path, None, Some("k2-agent"), None).expect("k2 alias");
        assert_eq!(k2_layer.source, "catalog:k2-agent");
        assert!(root.join(".k2/context/catalog/k2-agent.md").is_file());
        let k2_body = fs::read_to_string(root.join(".k2/context/catalog/k2-agent.md")).unwrap();
        assert!(
            k2_body.len() < 4_000,
            "k2 pack must stay lean, len={}",
            k2_body.len()
        );

        cleanup_project(path, &_pid);
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
    fn bad_usage_both_path_and_catalog() {
        let root = unique_root("both");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let err =
            add_layer(path, Some("a.md"), Some("wiki:index"), None).expect_err("both is bad_usage");
        assert_eq!(err.code(), "bad_usage");

        let err = add_layer(path, None, None, None).expect_err("neither");
        assert_eq!(err.code(), "bad_usage");

        cleanup_project(path, &pid);
    }

    #[test]
    fn pinned_info_persona_old_new_and_both() {
        let root = unique_root("persona-pin");
        let path = root.to_str().unwrap();
        let pid = register_project(path);
        let dir = crate::workspace::agent_identity::workspace_agent_path(path);
        fs::create_dir_all(&dir).unwrap();

        fs::write(dir.join("AGENT.md"), "---\nname: old\n---\nold\n").unwrap();
        let pin = pinned_info(path);
        assert_eq!(pin[0].id, "pinned:agent");
        assert_eq!(pin[0].label, "Role");
        assert!(
            pin[0].path.ends_with("agent/AGENT.md"),
            "got {}",
            pin[0].path
        );
        assert!(pin[0].exists);

        fs::write(dir.join("ROLE.md"), "---\nname: neu\n---\nnew\n").unwrap();
        let pin = pinned_info(path);
        assert!(
            pin[0].path.ends_with("agent/ROLE.md"),
            "ROLE.md wins: {}",
            pin[0].path
        );
        assert!(pin[0].exists);

        fs::remove_file(dir.join("AGENT.md")).unwrap();
        let pin = pinned_info(path);
        assert!(pin[0].path.ends_with("agent/ROLE.md"));
        assert!(pin[0].exists);

        cleanup_project(path, &pid);
    }
}
