//! Agent CRUD commands.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`. This
//! is the business logic behind the bulk of the `k2so agents *` CLI
//! surface — list / create / delete / get_profile / update_profile /
//! update_field — plus the shared helpers `cleanup_agent_backups` and
//! `log_agent_warning` they all rely on. Every function is
//! host-agnostic (no Tauri, no AppHandle) so the daemon and the Tauri
//! app serve identical semantics.
//!
//! Sibling modules:
//! - [`crate::heartbeats::control`] — per-agent heartbeat CRUD +
//!   adaptive backoff (`ensure_agent_wakeup`, `get_heartbeat`,
//!   `set_heartbeat`, `heartbeat_noop`, `heartbeat_action`)
//! - [`crate::workspace::agent_editor`] — AIFileEditor surface for
//!   editing AGENT.md (get_editor_context, preview/regenerate/save)
//! - [`crate::workspace::relations`] — `workspace_sessions` and
//!   `workspace_relations` DB accessors.


use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::skills::writer::{generate_default_agent_body, write_agent_skill_file};
use crate::workspace::agent_identity::{
    agent_dir, agent_type_for, agents_dir, parse_frontmatter, workspace_agent_path,
};
use crate::workspace::session::simple_date;
use crate::workspace::work_item::atomic_write;
use crate::heartbeats::control::ensure_agent_wakeup;
use crate::workspace::scheduler::{agent_work_dir, count_md_files};

/// Summary row the UI agent-list + `k2so agents list` CLI render.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct K2soAgentInfo {
    pub name: String,
    pub role: String,
    pub inbox_count: usize,
    pub active_count: usize,
    pub done_count: usize,
    pub is_manager: bool,
    /// Agent type: "k2so", "custom", "manager", "agent-template"
    pub agent_type: String,
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Prune an agent-backups dir down to the most-recent `keep` files.
/// Sorts by filename (which embeds the date stamp from `simple_date`)
/// so "oldest" means lexicographically smallest.
pub fn cleanup_agent_backups(backup_dir: &Path, keep: usize) {
    if let Ok(entries) = fs::read_dir(backup_dir) {
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |ext| ext == "md"))
            .collect();
        files.sort();
        if files.len() > keep {
            for old in &files[..files.len() - keep] {
                fs::remove_file(old).ok();
            }
        }
    }
}

/// Append a warning line to `<agent-dir>/agent.log`. Silent on any I/O
/// failure — logging a warning shouldn't itself cause a crash.
pub fn log_agent_warning(project_path: &str, agent_name: &str, message: &str) {
    let log_path = agent_dir(project_path, agent_name).join("agent.log");
    let entry = format!("[{}] WARN: {}\n", simple_date(), message);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        use std::io::Write;
        let _ = file.write_all(entry.as_bytes());
    }
}

// ── Agent CRUD ────────────────────────────────────────────────────────

// ── Agent CRUD ────────────────────────────────────────────────────────

/// List every skill directory in the workspace with summary counts
/// (inbox / active / done item counts + manager flag + canonical
/// type). Alphabetical.
///
/// Post-Phase-2.5b: reads from `.k2so/skills/<name>/` (the unified
/// home for documentation profiles). Pre-migration workspaces fall
/// back to enumerating the legacy `.k2so/agents/` tree so the call
/// keeps returning useful data during the boot-cycle window where
/// the daemon hasn't run `consolidate_skills_v1` yet.
pub fn list(project_path: String) -> Result<Vec<K2soAgentInfo>, String> {
    let new_home = crate::workspace::agent_identity::skills_dir(&project_path);
    let dir = if new_home.exists() {
        new_home
    } else {
        // Pre-Phase-2.5b workspaces still answer from the legacy tree.
        // The daemon's first-boot sweep migrates this away; after that
        // `.k2so/skills/` exists and the branch above is taken.
        agents_dir(&project_path)
    };
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut agents = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden / system directories. `.archive` is the trash bin
        // for retired agents (see `archive_orphan_top_tier_agents`); it
        // sits inside `.k2so/agents/` for proximity but must never
        // appear in the workspace's agent list. Pre-0.36.0 it leaked
        // through and the WorkspacePanel rendered ".archive" as the
        // primary agent name. Any other dotfile dir is excluded for
        // the same reason.
        if name.starts_with('.') {
            continue;
        }
        // Post-Phase-2.5b skills carry SKILL.md; pre-migration
        // workspaces carry AGENT.md. Read whichever exists — the
        // frontmatter shape is identical.
        let skill_md = entry.path().join("SKILL.md");
        let agent_md = entry.path().join("AGENT.md");
        let profile_md = if skill_md.exists() { skill_md } else { agent_md.clone() };

        let (role, is_manager, agent_type) = if profile_md.exists() {
            let content = fs::read_to_string(&profile_md).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            let role = fm.get("role").cloned().unwrap_or_default();
            // Support old (pod_leader/coordinator) and new (manager) keys.
            let is_mgr = fm.get("pod_leader").map(|v| v == "true").unwrap_or(false)
                || fm.get("coordinator").map(|v| v == "true").unwrap_or(false)
                || fm.get("manager").map(|v| v == "true").unwrap_or(false);
            let agent_type = fm
                .get("type")
                .cloned()
                .map(|t| match t.as_str() {
                    "pod-leader" | "coordinator" => "manager".to_string(),
                    "pod-member" => "agent-template".to_string(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| {
                    if is_mgr {
                        "manager".to_string()
                    } else {
                        "agent-template".to_string()
                    }
                });
            (role, is_mgr, agent_type)
        } else {
            (String::new(), false, "agent-template".to_string())
        };

        let inbox_count = count_md_files(&agent_work_dir(&project_path, &name, "inbox"));
        let active_count = count_md_files(&agent_work_dir(&project_path, &name, "active"));
        let done_count = count_md_files(&agent_work_dir(&project_path, &name, "done"));

        agents.push(K2soAgentInfo {
            name,
            role,
            inbox_count,
            active_count,
            done_count,
            is_manager,
            agent_type,
        });
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

/// Create a new agent dir (`.k2so/agents/<name>/`) + frontmatter-
/// wrapped `AGENT.md` + scaffold inbox/active/done + write the
/// per-agent SKILL.md + scaffold WAKEUP.md (for types that use it).
///
/// `agent_type` defaults to `"agent-template"`. Name must be
/// alphanumeric (plus `-` / `_`).
pub fn create(
    project_path: String,
    name: String,
    role: String,
    prompt: Option<String>,
    agent_type: Option<String>,
) -> Result<K2soAgentInfo, String> {
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err("Agent name must be alphanumeric (hyphens and underscores allowed)".to_string());
    }

    // 0.37.0 unification awareness: if the workspace has been migrated
    // and the unified primary at .k2so/agent/ already exists, "creating"
    // the workspace's named primary is a no-op success — the persona
    // is already there. The pre-0.37.0 contract was "create the agent's
    // dir + scaffold AGENT.md if missing"; post-migration the dir is
    // always present (the migration created it). Frontend "Manage
    // Persona" buttons that reflexively call `create` to ensure the
    // agent exists used to error here ("Agent already exists"); now
    // they get back the existing K2soAgentInfo and proceed to the
    // edit flow.
    let unified_primary = crate::workspace_dot_dir(&project_path).join("agent");
    if unified_primary.join("AGENT.md").exists() {
        let existing_type = agent_type_for(&project_path, &name);
        return Ok(K2soAgentInfo {
            name,
            role,
            inbox_count: 0,
            active_count: 0,
            done_count: 0,
            is_manager: existing_type == "manager" || existing_type == "coordinator",
            agent_type: existing_type,
        });
    }

    // 0.39.x: scaffold a NEW agent at the CANONICAL `.k2so/agent/`
    // (workspace_agent_path), NOT the legacy `.k2so/agents/<name>/`.
    //
    // Pre-fix this used `agent_dir()` — a *resolver* whose final
    // fallback is `.k2so/agents/<name>` — as the creation TARGET. So a
    // brand-new agent (no canonical AGENT.md yet) got scaffolded into
    // the retired plural folder, and a user's agent docs landed in
    // `.k2so/agents/<name>/AGENT.md` instead of `.k2so/agent/AGENT.md`.
    // The post-0.37.0 model is one agent per workspace at `.k2so/agent/`;
    // the unification migration moves legacy dirs, but `create()` for a
    // fresh agent was never repointed. (We only reach here when the
    // canonical AGENT.md does NOT already exist — the early-return above
    // handled that case, so we won't clobber an existing persona.)
    let dir = workspace_agent_path(&project_path);

    let agent_type = agent_type.unwrap_or_else(|| "agent-template".to_string());
    let is_manager = agent_type == "manager" || agent_type == "coordinator";

    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create agent dir: {}", e))?;
    // Post-Phase-2.1: scaffold the unified workspace inbox at
    // `.k2so/inbox/` (not the retired `.k2so/work/inbox/` nor the legacy
    // per-agent `work/{inbox,active,done}` under `.k2so/agents/`).
    // Best-effort — an existing inbox is fine.
    let _ = fs::create_dir_all(crate::inbox::inbox_root(std::path::Path::new(&project_path)));

    let agent_md = dir.join("AGENT.md");
    let mut frontmatter = format!("name: {}\nrole: {}\ntype: {}", name, role, agent_type);
    if is_manager {
        frontmatter.push_str("\nmanager: true");
    }

    let body = if let Some(ref p) = prompt {
        if !p.is_empty() {
            p.clone()
        } else {
            generate_default_agent_body(&agent_type, &name, &role, &project_path)
        }
    } else {
        generate_default_agent_body(&agent_type, &name, &role, &project_path)
    };

    let content = format!("---\n{}\n---\n\n{}\n", frontmatter, body);
    atomic_write(&agent_md, &content)?;

    write_agent_skill_file(&project_path, &name, &agent_type);
    ensure_agent_wakeup(&project_path, &name, &agent_type);

    Ok(K2soAgentInfo {
        name,
        role,
        inbox_count: 0,
        active_count: 0,
        done_count: 0,
        is_manager,
        agent_type,
    })
}

/// 0.39.x repair sweep: move a stray legacy `.k2so/agents/<name>/`
/// agent to the canonical `.k2so/agent/`.
///
/// A pre-0.39.x `create()` bug (see the comment in `create`) scaffolded
/// brand-new agents into the legacy plural `.k2so/agents/<name>/` folder
/// instead of canonical `.k2so/agent/`. Workspaces that hit it AFTER the
/// one-shot 0.37.0 unification migration already ran won't get fixed by
/// that migration (its sentinel is set), so this runs every boot and
/// self-heals — including a user who put ALL their agent docs in the
/// legacy folder.
///
/// Conservative + idempotent:
/// - No-op if canonical `.k2so/agent/AGENT.md` already exists (never
///   clobber a real persona).
/// - No-op if there's no legacy `.k2so/agents/` directory.
/// - Picks the first non-`.archive` legacy agent dir that has an
///   `AGENT.md`, then moves ALL its entries (AGENT.md + any docs/subdirs
///   the user added) into `.k2so/agent/`. Per-entry: skip if a
///   same-named target already exists (don't clobber). Leaves the now-
///   emptied legacy dir in place — harmless once canonical wins
///   resolution, and NOT trashed to avoid macOS Finder Touch-ID prompts
///   during a headless boot sweep.
///
/// Returns `true` if it moved anything.
pub fn repoint_stray_legacy_agent(project_path: &str) -> bool {
    let canonical = workspace_agent_path(project_path);
    if canonical.join("AGENT.md").exists() {
        return false; // canonical persona already present — nothing to do
    }
    let legacy_root = agents_dir(project_path);
    if !legacy_root.exists() {
        return false;
    }

    // Find the first legacy agent dir (skip dotfiles like `.archive`)
    // that actually holds an AGENT.md.
    let entries = match fs::read_dir(&legacy_root) {
        Ok(e) => e,
        Err(_) => return false,
    };
    let mut legacy_agent: Option<std::path::PathBuf> = None;
    for entry in entries.flatten() {
        let p = entry.path();
        let is_dotdir = entry
            .file_name()
            .to_str()
            .map(|n| n.starts_with('.'))
            .unwrap_or(true);
        if p.is_dir() && !is_dotdir && p.join("AGENT.md").exists() {
            legacy_agent = Some(p);
            break;
        }
    }
    let Some(src_dir) = legacy_agent else {
        return false;
    };

    if let Err(e) = fs::create_dir_all(&canonical) {
        crate::log_debug!(
            "[repoint-legacy-agent] {}: create canonical dir failed: {e}",
            project_path
        );
        return false;
    }

    // Move each top-level entry; a single rename moves whole subtrees.
    let inner = match fs::read_dir(&src_dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    let mut moved = 0usize;
    for entry in inner.flatten() {
        let src = entry.path();
        let Some(fname) = src.file_name() else { continue };
        let dst = canonical.join(fname);
        if dst.exists() {
            continue; // don't clobber an existing canonical file
        }
        match fs::rename(&src, &dst) {
            Ok(()) => moved += 1,
            Err(e) => crate::log_debug!(
                "[repoint-legacy-agent] {}: move {:?} -> {:?} failed: {e}",
                project_path,
                src,
                dst
            ),
        }
    }

    if moved > 0 {
        crate::log_debug!(
            "[repoint-legacy-agent] {}: moved {moved} entr{} from {:?} to canonical .k2so/agent/",
            project_path,
            if moved == 1 { "y" } else { "ies" },
            src_dir,
        );
    }
    moved > 0
}

/// Delete an agent's dir. Refuses for manager agents or agents with
/// active work items unless `force` is true.
pub fn delete_inner(project_path: &str, name: &str, force: bool) -> Result<(), String> {
    let dir = agent_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }

    let agent_md = dir.join("AGENT.md");
    if agent_md.exists() {
        let content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&content);
        if fm
            .get("type")
            .map_or(false, |t| t == "manager" || t == "coordinator" || t == "pod-leader")
            && !force
        {
            return Err("Cannot delete manager agent. Use --force to override.".to_string());
        }
    }

    if !force {
        let active_dir = agent_work_dir(project_path, name, "active");
        if active_dir.exists() {
            let active_count = fs::read_dir(&active_dir)
                .map_err(|e| format!("Cannot check active work for '{}': {}", name, e))?
                .flatten()
                .count();
            if active_count > 0 {
                return Err(format!(
                    "Agent '{}' has {} active work item(s). Use --force to delete anyway.",
                    name, active_count
                ));
            }
        }
    }

    // **0.37.6:** route to recycle bin instead of permanent unlink.
    // Agent dir contains AGENT.md (user-editable persona file) +
    // potentially user-authored work items, heartbeat config,
    // skill content. Recoverable from Trash if the user changes
    // their mind.
    //
    // SAFETY: routes through `scratch_safe_trash` so test scratch
    // paths under temp_dir() skip the trash crate (avoids macOS
    // Touch ID prompts during cargo test).
    crate::safe_delete_scratch::scratch_safe_trash(&dir)
        .map_err(|e| format!("Failed to delete agent: {}", e))?;
    Ok(())
}

/// Non-forced variant — the Tauri command shape.
pub fn delete(project_path: String, name: String) -> Result<(), String> {
    delete_inner(&project_path, &name, false)
}

/// Read an agent's raw `AGENT.md` content.
pub fn get_profile(project_path: String, agent_name: String) -> Result<String, String> {
    let path = agent_dir(&project_path, &agent_name).join("AGENT.md");
    if !path.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Replace an agent's raw `AGENT.md` content. Caller owns validation
/// of the incoming string.
pub fn update_profile(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    let path = dir.join("AGENT.md");
    atomic_write(&path, &content)
}

/// Pure, I/O-free rewrite of an `AGENT.md` content blob with `field`
/// set to `value`. If `field` is a frontmatter key, the value replaces
/// the existing frontmatter line. If `field` is a markdown section
/// (`## heading`), the value replaces everything from the heading to
/// the next `## ` or end-of-body. Unknown sections are appended.
pub fn update_agent_md_field(content: &str, field: &str, value: &str) -> Result<String, String> {
    if !content.starts_with("---") {
        return Err("agent.md missing frontmatter".to_string());
    }
    let end_idx = content[3..]
        .find("---")
        .ok_or_else(|| "Invalid frontmatter in agent.md".to_string())?;
    let frontmatter = &content[3..3 + end_idx];
    let body = &content[3 + end_idx + 3..];

    let fm_keys: Vec<&str> = frontmatter
        .lines()
        .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim()))
        .collect();

    if fm_keys.contains(&field) {
        let updated_fm: String = frontmatter
            .lines()
            .map(|line| {
                if let Some((key, _)) = line.split_once(':') {
                    if key.trim() == field {
                        return format!("{}: {}", field, value);
                    }
                }
                line.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(format!("---\n{}\n---{}", updated_fm.trim(), body));
    }

    let section_header = format!("## {}", field);
    if let Some(start) = body.find(&section_header) {
        let after_header = start + section_header.len();
        let end = body[after_header..]
            .find("\n## ")
            .map(|pos| after_header + pos)
            .unwrap_or(body.len());
        let mut new_body = String::new();
        new_body.push_str(&body[..start]);
        new_body.push_str(&section_header);
        new_body.push_str("\n\n");
        new_body.push_str(value);
        new_body.push_str("\n\n");
        new_body.push_str(body[end..].trim_start());
        Ok(format!("---\n{}\n---{}", frontmatter.trim(), new_body))
    } else {
        let mut new_body = body.to_string();
        if !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        new_body.push_str(&format!("\n## {}\n\n{}\n", field, value));
        Ok(format!("---\n{}\n---{}", frontmatter.trim(), new_body))
    }
}

/// Command-shape `update_field` — reads AGENT.md, rewrites via
/// [`update_agent_md_field`], drops a timestamped backup into
/// `<agent-dir>/agent-backups/` (capped at 20 via
/// [`cleanup_agent_backups`]), then atomic-writes the new content.
pub fn update_field(
    project_path: String,
    name: String,
    field: String,
    value: String,
) -> Result<String, String> {
    let dir = agent_dir(&project_path, &name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }

    let md_path = dir.join("AGENT.md");
    let content = fs::read_to_string(&md_path)
        .map_err(|e| format!("Failed to read agent.md: {}", e))?;

    let updated = update_agent_md_field(&content, &field, &value)?;

    let backup_dir = dir.join("agent-backups");
    let _ = fs::create_dir_all(&backup_dir);
    let backup_name = format!(
        "agent-{}.md",
        simple_date().replace(' ', "_").replace(':', "-")
    );
    let _ = fs::copy(&md_path, backup_dir.join(&backup_name));
    cleanup_agent_backups(&backup_dir, 20);

    atomic_write(&md_path, &updated)?;
    Ok(updated)
}

// Phase 2.1 wrap-up (0.39.0f) — the `work_item_slug` + `body_preview`
// helpers (last used by `workspace_inbox_create`) were removed with
// the function itself. The post-Phase-2.1 inbox primitive has its own
// slug + preview helpers in `k2_core::inbox`.

// Phase 2.1c Item 2 — `work_list`, `work_create`, `work_move`, and
// `workspace_inbox_list` removed (zero remaining callers; the
// renderer migrated to `k2_core::inbox::*` via the new
// `commands::inbox::k2so_inbox_*` Tauri shims). The daemon CLI
// surface for these had already been hard-deprecated in Phase 2.1b.
//
// Phase 2.1 wrap-up (0.39.0f) — `workspace_inbox_create` removed.
// Its sole caller (the daemon's `workspace_msg::deliver_to_inbox`)
// was retired in the same commit. The function wrote to the legacy
// `.k2so/work/inbox/` layout, which the post-Phase-2.1 migration
// hook (in `inbox::migrate_work_to_inbox`) now sends to the macOS
// Recycle Bin. New inbox-delivery callers should use
// `k2_core::inbox::compose` so they land in `.k2so/inbox/` where
// the renderer + CLI actually read from.

#[cfg(test)]
mod tests {
    use super::*;

    // `work_item_slug_*` and `body_preview_*` tests removed alongside
    // their helpers in the Phase 2.1 wrap-up. Equivalent coverage for
    // the `k2_core::inbox` slug/preview helpers lives in that
    // module's own test suite.

    #[test]
    fn update_agent_md_field_replaces_frontmatter_key() {
        let md = "---\nrole: old\ntype: custom\n---\n\nbody";
        let updated = update_agent_md_field(md, "role", "new").unwrap();
        assert!(updated.contains("role: new"));
        assert!(!updated.contains("role: old"));
    }

    #[test]
    fn update_agent_md_field_replaces_section() {
        let md = "---\nrole: x\n---\n\n## Existing\n\nold body\n\n## Other\n\nkeep";
        let updated = update_agent_md_field(md, "Existing", "new body").unwrap();
        assert!(updated.contains("new body"));
        assert!(!updated.contains("old body"));
        assert!(updated.contains("## Other"));
        assert!(updated.contains("keep"));
    }

    #[test]
    fn update_agent_md_field_appends_unknown_section() {
        let md = "---\nrole: x\n---\n\nbody";
        let updated = update_agent_md_field(md, "New Section", "content").unwrap();
        assert!(updated.contains("## New Section"));
        assert!(updated.contains("content"));
    }

    #[test]
    fn update_agent_md_field_rejects_missing_frontmatter() {
        assert!(update_agent_md_field("no fm", "role", "x").is_err());
    }

    // ── 0.39.x: repoint_stray_legacy_agent (filesystem-only) ─────────

    fn tmp_ws() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2so-repoint-test-{}-{}",
            std::process::id(),
            // monotonic-ish uniqueness without Date/rand (forbidden):
            // use an atomic counter.
            {
                use std::sync::atomic::{AtomicU64, Ordering};
                static N: AtomicU64 = AtomicU64::new(0);
                N.fetch_add(1, Ordering::SeqCst)
            }
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn repoint_moves_stray_legacy_persona_to_canonical() {
        let ws = tmp_ws();
        let wss = ws.to_string_lossy().to_string();
        // Simulate the create() bug: persona + a user doc under the
        // legacy plural folder, and NO canonical .k2so/agent/AGENT.md.
        let legacy = ws.join(".k2so/agents/scout");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("AGENT.md"), "---\nname: scout\n---\npersona body").unwrap();
        fs::write(legacy.join("NOTES.md"), "all my agent docs").unwrap();

        let moved = repoint_stray_legacy_agent(&wss);
        assert!(moved, "should report it moved content");

        let canonical = ws.join(".k2so/agent");
        assert_eq!(
            fs::read_to_string(canonical.join("AGENT.md")).unwrap(),
            "---\nname: scout\n---\npersona body",
            "persona must land at canonical .k2so/agent/AGENT.md",
        );
        assert_eq!(
            fs::read_to_string(canonical.join("NOTES.md")).unwrap(),
            "all my agent docs",
            "user docs must move alongside the persona",
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn repoint_noop_when_canonical_already_exists() {
        let ws = tmp_ws();
        let wss = ws.to_string_lossy().to_string();
        // Canonical persona present → must NOT clobber it.
        let canonical = ws.join(".k2so/agent");
        fs::create_dir_all(&canonical).unwrap();
        fs::write(canonical.join("AGENT.md"), "real canonical persona").unwrap();
        // A stray legacy also exists, but should be left alone.
        let legacy = ws.join(".k2so/agents/scout");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("AGENT.md"), "legacy stray").unwrap();

        assert!(!repoint_stray_legacy_agent(&wss), "must no-op when canonical exists");
        assert_eq!(
            fs::read_to_string(canonical.join("AGENT.md")).unwrap(),
            "real canonical persona",
            "canonical persona must be untouched",
        );
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn repoint_noop_when_no_legacy_folder() {
        let ws = tmp_ws();
        let wss = ws.to_string_lossy().to_string();
        assert!(!repoint_stray_legacy_agent(&wss));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn repoint_skips_dot_archive_only_legacy() {
        let ws = tmp_ws();
        let wss = ws.to_string_lossy().to_string();
        // Only an archive dir under legacy — no live agent to repoint.
        let archive = ws.join(".k2so/agents/.archive/old-agent-20260101");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("AGENT.md"), "archived").unwrap();

        assert!(!repoint_stray_legacy_agent(&wss), "must skip .archive entries");
        assert!(
            !ws.join(".k2so/agent/AGENT.md").exists(),
            "must not create canonical from an archive",
        );
        let _ = fs::remove_dir_all(&ws);
    }

    // ── 0.37.0 post-unification idempotency ──────────────────────
    //
    // The AIFileEditor "Manage Persona" button calls `create()` to
    // ensure the agent exists before opening the edit flow. Pre-fix,
    // post-unification workspaces (where `.k2so/agent/AGENT.md`
    // exists) errored "Agent already exists" because agent_dir's
    // layout-aware probe correctly resolved to the unified path —
    // which always exists post-migration. Tests below pin the
    // short-circuit: when the unified primary's AGENT.md is on
    // disk, create() returns Ok with the existing K2soAgentInfo
    // instead of erroring.

    fn temp_workspace_with_unified_agent(persona_type: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-commands-create-test-{}-{}-{}",
            persona_type,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".k2so/agent")).unwrap();
        let body = format!("---\nname: cortana\ntype: {}\n---\n# persona body\n", persona_type);
        std::fs::write(dir.join(".k2so/agent/AGENT.md"), body).unwrap();
        dir
    }

    #[test]
    fn create_short_circuits_when_unified_primary_exists() {
        let dir = temp_workspace_with_unified_agent("custom");
        let path = dir.to_string_lossy().into_owned();

        // Pre-fix this would error "Agent 'cortana' already exists"
        // because agent_dir resolved to .k2so/agent/ which has
        // AGENT.md. Post-fix it should return Ok with the existing
        // info (read from the on-disk frontmatter).
        let result = create(
            path.clone(),
            "cortana".to_string(),
            "test role".to_string(),
            None,
            Some("custom".to_string()),
        );
        assert!(result.is_ok(), "create should short-circuit, got: {result:?}");
        let info = result.unwrap();
        assert_eq!(info.name, "cortana");
        assert_eq!(info.agent_type, "custom",
            "agent_type must come from the on-disk AGENT.md frontmatter");
        assert!(!info.is_manager, "custom-type primary is not a manager");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_short_circuit_recognizes_manager_type() {
        let dir = temp_workspace_with_unified_agent("manager");
        let path = dir.to_string_lossy().into_owned();

        let result = create(
            path,
            "anything".to_string(), // name doesn't matter; workspace primary wins
            "test role".to_string(),
            None,
            None,
        );
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.agent_type, "manager",
            "frontmatter type='manager' must propagate");
        assert!(info.is_manager, "manager type should set is_manager=true");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_writes_canonical_agent_dir_not_legacy() {
        // 0.39.x: a fresh workspace (no `.k2so/agent/AGENT.md` yet) must
        // get its new agent scaffolded at the CANONICAL `.k2so/agent/`,
        // NOT the legacy plural `.k2so/agents/<name>/`. This is the
        // regression fix: pre-0.39.x `create()` used the `agent_dir()`
        // resolver (legacy fallback) as the creation target, so a user's
        // agent docs landed in `.k2so/agents/` instead of
        // `.k2so/agent/AGENT.md`.
        let dir = std::env::temp_dir().join(format!(
            "k2so-commands-create-canonical-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        // Sanity: no canonical path exists pre-call.
        assert!(!dir.join(".k2so/agent/AGENT.md").exists());

        let result = create(
            path.clone(),
            "fresh-agent".to_string(),
            "test role".to_string(),
            None,
            Some("custom".to_string()),
        );
        assert!(result.is_ok(), "fresh create should succeed: {result:?}");

        // Canonical persona is written.
        assert!(
            dir.join(".k2so/agent/AGENT.md").exists(),
            "create() must write the canonical .k2so/agent/AGENT.md",
        );
        // And the legacy plural folder must NOT be created.
        assert!(
            !dir.join(".k2so/agents").exists(),
            "create() must NOT create the legacy .k2so/agents/ folder",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
