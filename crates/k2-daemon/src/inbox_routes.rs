//! Daemon-side `/cli/inbox/*` route handlers (Phase 2.1 A22).
//!
//! Read endpoints (list, read, folders, search) are GET — wired into
//! the unified `cli::dispatch` table. Write endpoints (compose, move,
//! archive, delete, respond) are POST — wired into the POST
//! allowlist + `dispatch_inbox_post` in main.rs.
//!
//! Every handler is a thin wrapper around `k2_core::inbox::*`.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::cli_response::CliResponse;

// ── Helpers ────────────────────────────────────────────────────────────

fn need_project_path(params: &HashMap<String, String>) -> Result<PathBuf, CliResponse> {
    for key in &["project_path", "project"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Ok(PathBuf::from(v));
            }
        }
    }
    Err(CliResponse::bad_request(
        "Missing project (or project_path) parameter",
    ))
}

fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

fn opt_param(params: &HashMap<String, String>, key: &str) -> Option<String> {
    params.get(key).cloned().filter(|s| !s.is_empty())
}

// ── GET handlers (query-string) ────────────────────────────────────────

/// GET /cli/inbox/list?project=<path>&folder=<name>
/// Empty/missing folder → top-level inbox.
pub fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let folder = str_param(params, "folder");
    let items = k2_core::inbox::list_folder(&workspace, &folder);
    CliResponse::ok_json(
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
    )
}

/// GET /cli/inbox/read?project=<path>&id=<filename-stem>
pub fn handle_read(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let id = str_param(params, "id");
    if id.is_empty() {
        return CliResponse::bad_request("Missing id");
    }
    match k2_core::inbox::read_by_id(&workspace, &id) {
        Ok(content) => CliResponse::ok_json(
            serde_json::json!({"id": id, "content": content}).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/inbox/folders?project=<path>
pub fn handle_folders(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let folders = k2_core::inbox::list_folders(&workspace);
    CliResponse::ok_json(
        serde_json::to_string(&folders).unwrap_or_else(|_| "[]".to_string()),
    )
}

/// GET /cli/inbox/search?project=<path>&q=<query>
pub fn handle_search(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let q = str_param(params, "q");
    let items = k2_core::inbox::search(&workspace, &q);
    CliResponse::ok_json(
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string()),
    )
}

// ── POST handlers (query-string params, no body) ───────────────────────
//
// All mutating routes are simple query-string POSTs (no JSON body
// parsing needed) — matches the pattern used by /cli/heartbeat/add etc.
// They're routed through `dispatch_inbox_post` registered in main.rs.

pub fn handle_compose_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let title = str_param(params, "title");
    if title.is_empty() {
        return CliResponse::bad_request("Missing title");
    }
    let body = str_param(params, "body");
    let priority = opt_param(params, "priority");
    let source = opt_param(params, "source");
    let from = opt_param(params, "from");
    match k2_core::inbox::compose(
        &workspace,
        &title,
        &body,
        priority.as_deref(),
        source.as_deref(),
        from.as_deref(),
    ) {
        Ok(item) => CliResponse::ok_json(
            serde_json::to_string(&item).unwrap_or_default(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_move_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let id = str_param(params, "id");
    let folder = str_param(params, "folder");
    if id.is_empty() {
        return CliResponse::bad_request("Missing id");
    }
    match k2_core::inbox::move_item(&workspace, &id, &folder) {
        Ok(path) => CliResponse::ok_json(
            serde_json::json!({"success": true, "path": path.display().to_string()})
                .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_archive_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let id = str_param(params, "id");
    if id.is_empty() {
        return CliResponse::bad_request("Missing id");
    }
    match k2_core::inbox::archive(&workspace, &id) {
        Ok(path) => CliResponse::ok_json(
            serde_json::json!({"success": true, "path": path.display().to_string()})
                .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_delete_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let id = str_param(params, "id");
    if id.is_empty() {
        return CliResponse::bad_request("Missing id");
    }
    match k2_core::inbox::delete(&workspace, &id) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true,"trashed":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_respond_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let id = str_param(params, "id");
    let text = str_param(params, "text");
    if id.is_empty() || text.is_empty() {
        return CliResponse::bad_request("Missing id or text");
    }
    match k2_core::inbox::respond(&workspace, &id, &text) {
        Ok(path) => CliResponse::ok_json(
            serde_json::json!({"success": true, "path": path.display().to_string()})
                .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── Migration trigger (POST) ───────────────────────────────────────────

/// POST /cli/inbox/migrate?project=<path>
///
/// Explicit caller-driven migration trigger. Idempotent — the daemon
/// also invokes this per-workspace on first boot (not yet wired —
/// Phase 2.1b), but this endpoint lets the CLI/Tauri host force a
/// migration for testing or a particular workspace.
pub fn handle_migrate_post(params: &HashMap<String, String>) -> CliResponse {
    let workspace = match need_project_path(params) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let report = k2_core::inbox::migrate_work_to_inbox(&workspace);
    CliResponse::ok_json(
        serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string()),
    )
}

// ── Glossary (read-only, no project) ───────────────────────────────────

/// Glossary entry: term + one-line summary + full definition body.
#[derive(serde::Serialize)]
struct GlossaryEntry {
    term: &'static str,
    summary: &'static str,
    definition: &'static str,
}

/// 17 glossary terms from PRD A10/A22/A23. Long-form definitions are
/// the verbatim mock text where applicable.
const GLOSSARY: &[GlossaryEntry] = &[
    GlossaryEntry {
        term: "activity",
        summary: "Append-only audit log of every workspace event",
        definition: "Append-only audit log of every workspace event (agent spawn, message, heartbeat fire, etc.). View with `k2so activity`. Persisted in the `activity_feed` table; survives daemon restart.",
    },
    GlossaryEntry {
        term: "agent",
        summary: "The workspace's primary AI assistant (1:1 with workspace)",
        definition: "The workspace's primary AI assistant. K2SO enforces a 1:1 invariant: agent and workspace are one entity — use `workspace` verbs to manage both.\n\nThe agent reads the workspace's inbox, applies skills to work, fires on heartbeat schedules, and coordinates with other workspaces via `msg`. It's the \"user\" of the workspace from K2SO's perspective.\n\nSee also: `k2so workspace profile` (reads the agent's AGENT.md), `k2so glossary skill` (capability profiles the agent can apply).",
    },
    GlossaryEntry {
        term: "agentic",
        summary: "Global toggle for K2SO's autonomous systems",
        definition: "Global toggle for K2SO's agentic systems (heartbeats, scheduled launches, autonomous wake). When off, K2SO acts as a plain workspace manager with no background activity. Configure via `k2so settings --agentic`.",
    },
    GlossaryEntry {
        term: "companion",
        summary: "Daemon's ngrok-tunneled server (Mobile + K2SO Connect)",
        definition: "The local server K2SO exposes via ngrok for Mobile Companion + K2SO Connect remote access. Daemon-owned (post-Phase-2). Configure via `k2so daemon companion`.",
    },
    GlossaryEntry {
        term: "connections",
        summary: "Cross-workspace links: which workspaces can read each other's status",
        definition: "Cross-workspace links. When workspace A \"connects\" to workspace B, A can read B's `who` / `activity` / inbox presence; B can show up in A's `workspace list`. Used for declaring \"these workspaces work together\" so K2SO surfaces the right context.\n\nManage via: `k2so connections list` / `add <path>` / `remove <path>`. Connections are symmetric (both sides see each other) and persisted per-workspace.\n\nNOT the same as: skill profiles (`k2so skills`), live sessions (`k2so workspace list --running`), or ngrok tunnel state (`k2so daemon companion`).",
    },
    GlossaryEntry {
        term: "harness",
        summary: "IDE integration layer (Claude/Cursor config K2SO writes)",
        definition: "The IDE / agent-runtime layer K2SO integrates with — Claude Code, Cursor, etc. K2SO writes the harness's configuration files (settings.json, hooks.json) so the harness knows about K2SO's workflows, but K2SO does NOT manage the harness's runtime.\n\nSpecifically: the harness owns spawn lifecycle (sub-agents, worktrees, sessions). K2SO owns coordination surface (inbox, heartbeats, skills documentation, workspace metadata). Together they form a complete stack: harness for execution, K2SO for orchestration.\n\nSee also: `k2so help-deprecated delegate` for the Phase 2.1 handoff (K2SO no longer spawns; harness does); `k2so glossary skill` for skill profiles the harness loads on spawn.",
    },
    GlossaryEntry {
        term: "heartbeat",
        summary: "Workspace-scoped scheduled wake (cron-like)",
        definition: "A workspace-scoped scheduled wake (cron-like) that fires the workspace's agent at defined intervals. Used for: periodic triage, scheduled syncs, \"wake me at 9am every weekday and check the inbox\" patterns.\n\nA workspace can have multiple heartbeats with different names + schedules. Manage via `k2so heartbeat schedule add|list|remove|edit|enable|disable`. Fire one immediately via `k2so heartbeat signal fire <name>`.\n\nStorage: `.k2/heartbeats/<name>/` per heartbeat. The daemon owns the launchd plist that fires them (`dev.k2.heartbeat.<workspace>.plist`).",
    },
    GlossaryEntry {
        term: "hooks",
        summary: "Claude Code / Cursor integration hooks (NOT git hooks)",
        definition: "K2SO's CLI-tool integration hooks (Claude Code channels, Cursor file hooks). Not the same as git hooks. `k2so daemon hooks` shows pipeline state.",
    },
    GlossaryEntry {
        term: "inbox",
        summary: "Workspace's email-like communication channel",
        definition: "The workspace's email-like communication channel. Items arrive here from other workspaces (via `k2so msg --inbox`) or are composed by the workspace's own agent (via `k2so inbox compose`).\n\nInbox items are non-urgent, non-aggro — the agent reads and triages on its own schedule. Triage = move items into folders the agent creates. There's no system-imposed folder taxonomy; the agent organizes its inbox the way a person organizes email (Projects, Reference, Issues, FYI, etc.).\n\nStorage: `.k2/inbox/<id>.md` (top-level) and `.k2/inbox/<folder>/<id>.md` (after `inbox move`).\n\nMigration from pre-Phase-2.1 K2SO: the daemon runs a one-shot migration on its first boot after upgrade. Old `.k2so/work/{inbox,active,done}/*.md` files are atomic-renamed into `.k2/inbox/{,active,done}/`, then the empty `.k2so/work/` folder is sent to the macOS Recycle Bin (recoverable if anything was missed). After migration there's no `.k2so/work/`; everything lives under `.k2/inbox/`.\n\nSee also: `k2so inbox --help` for the full verb surface, `k2so msg --help` for sending into someone else's inbox.",
    },
    GlossaryEntry {
        term: "onboarding",
        summary: "First-launch flow for registering a workspace",
        definition: "First-launch flow for registering a new workspace or adopting an existing project. See `k2so onboarding --help`.",
    },
    GlossaryEntry {
        term: "signal",
        summary: "Typed event sent between workspaces (msg, status, presence, etc.)",
        definition: "A typed event K2SO sends between workspaces (msg, status, presence, reservation, task-lifecycle, custom). Sent via `k2so msg --signal <kind>`. The default `k2so msg <ws> \"text\"` is sugar for `--signal msg`.",
    },
    GlossaryEntry {
        term: "skill",
        summary: "Documentation profile for a role/capability",
        definition: "A documentation profile describing a role, persona, and instructions. Skills are *not* spawnable entities — they're markdown files (SKILL.md) that your harness (Claude Code, Cursor) loads when you want to apply that role to specific work.\n\nK2SO manages skill files (list, create, remove, profile, regenerate). Spawning a session pre-loaded with a skill is your harness's job (sub-agent spawning in Claude Code, etc.) — K2SO no longer provides a `delegate` verb for this.\n\nFilesystem: skills live at `.k2/skills/<name>/SKILL.md` (unified home as of Phase 2.5b). The legacy `.k2so/agents/` and `.k2so/agent-templates/` folders are consolidated into here at first daemon boot per upgraded workspace; originals go to the macOS Recycle Bin.\n\nSee also: `k2so skills --help`, `k2so glossary agent`.",
    },
    GlossaryEntry {
        term: "skill-template",
        summary: "Master skill definition that can be instantiated",
        definition: "Post-Phase-2.5b: any existing skill at `.k2/skills/<name>/` can serve as a template for a new one. The former `.k2/agent-templates/<role>/` namespace was consolidated into the unified `.k2/skills/` home at first daemon boot. Create new skills from a seed via `k2so skills create <name> --template <existing-skill>`.",
    },
    GlossaryEntry {
        term: "state",
        summary: "Workspace capability tier (build/managed/maintenance/locked)",
        definition: "A workspace's capability tier (build / managed / maintenance / locked) that gates which actions the agent can take autonomously. Configure via `k2so settings --state <id>`.",
    },
    GlossaryEntry {
        term: "workspace",
        summary: "A project folder K2SO knows about (has exactly one primary agent)",
        definition: "A project folder that K2SO knows about. Has at most one primary agent, plus heartbeats, settings, and inbox. List all with `k2so workspace list`.",
    },
    GlossaryEntry {
        term: "worktree",
        summary: "Git worktree — a working directory linked to a different branch",
        definition: "A git worktree — a working directory linked to a separate branch from your main checkout. K2SO uses worktrees so a sub-agent or pull request can work on a feature branch in isolation, then merge back.\n\nModern harnesses (Claude Code, Cursor) create worktrees natively when you spawn a sub-agent. K2SO tracks the resulting worktrees in `reviews` (pending merges) and the daemon owns cleanup on merge or reject.\n\nSee also: `k2so reviews`, `k2so review approve|reject|feedback`.",
    },
    GlossaryEntry {
        term: "reservation",
        summary: "Short-term file path lock between agents",
        definition: "A short-term lock on a file path so two agents don't edit the same file concurrently. Acquire with `k2so reserve`, release with `k2so release`.",
    },
];

/// GET /cli/glossary/list
/// Returns the full glossary as JSON.
pub fn handle_glossary_list() -> CliResponse {
    let entries: Vec<_> = GLOSSARY
        .iter()
        .map(|e| {
            serde_json::json!({
                "term": e.term,
                "summary": e.summary,
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::to_string(&entries).unwrap_or_default())
}

/// GET /cli/glossary/get?term=<name>
/// Returns the full definition for one term, or 404 if not found.
pub fn handle_glossary_get(params: &HashMap<String, String>) -> CliResponse {
    let term = str_param(params, "term").to_lowercase();
    if term.is_empty() {
        return CliResponse::bad_request("Missing term");
    }
    for entry in GLOSSARY {
        if entry.term == term {
            return CliResponse::ok_json(
                serde_json::json!({
                    "term": entry.term,
                    "summary": entry.summary,
                    "definition": entry.definition,
                })
                .to_string(),
            );
        }
    }
    CliResponse::bad_request(format!("glossary term not found: {term}"))
}

// ── POST dispatcher ────────────────────────────────────────────────────

pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/inbox/compose" => handle_compose_post(params),
        "/cli/inbox/move" => handle_move_post(params),
        "/cli/inbox/archive" => handle_archive_post(params),
        "/cli/inbox/delete" => handle_delete_post(params),
        "/cli/inbox/respond" => handle_respond_post(params),
        "/cli/inbox/migrate" => handle_migrate_post(params),
        _ => CliResponse::not_found(),
    }
}
