//! Workspace one-shot migration helpers (boot-time).
//!
//! Phase 2.5d: extracted from the monolithic `agents/workspace.rs`. Each
//! function here is idempotent and gated by an on-disk sentinel or
//! row-existence check so re-running on every boot is cheap and safe.
//!
//! Also hosts the small archive-utility helpers
//! [`archive_claude_md_file`], [`inject_first_migration_banner`], and
//! [`log_adoption_event`] used by the SKILL writer + harness clusters.
//! They're `pub(crate)` so the sibling modules can re-import them; the
//! semantics are migration-flavored ("archive user-authored content
//! before mutating"), which is why they live here rather than in a
//! standalone utility module.

use std::fs;
use std::path::{Path, PathBuf};

use crate::db::schema::{AgentHeartbeat, WorkspaceSession};
use crate::fs_atomic::{self, atomic_write_str, log_if_err, unique_archive_path};
use crate::heartbeats::control::ensure_agent_wakeup;
use crate::heartbeats::k2so_heartbeat_add;
use crate::workspace::agent_identity::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, resolve_project_id,
    workspace_agent_path, workspace_heartbeats_dir,
};
use crate::workspace::wake_prompts::workspace_wakeup_path;

/// Walk `.k2so/agents/` for top-tier directories (agent_type ∈ custom /
/// `manager`, or `k2so` but that aren't the current primary for this
/// workspace. Moves them to `.k2so/agents/.archive/<name>-<timestamp>/`
/// and removes their DB rows (`agent_sessions`, and any stray
/// `agent_heartbeats` pointing at the orphan's folder). Templates are
/// ALWAYS preserved — the Workspace Manager delegates to them on-demand.
///
/// Idempotent: no-op when there are no orphans. Called at startup
/// (after heartbeat repair) and from projects_update before an
/// agent_mode change takes effect.
pub fn archive_orphan_top_tier_agents(project_path: &str) -> Vec<String> {
    let mut archived = Vec::new();
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return archived;
    }
    let Some(primary) = find_primary_agent(project_path) else {
        // Can't resolve primary — don't risk archiving the wrong thing.
        return archived;
    };

    let Ok(entries) = fs::read_dir(&agents_root) else { return archived };
    let mut orphans: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == primary {
            continue;
        }
        let agent_type = agent_type_for(project_path, &name);
        // Stage A dual-read: builtin `k2` is a synonym of legacy `k2so`.
        if matches!(agent_type.as_str(), "custom" | "manager")
            || crate::workspace::agent_identity::is_builtin_agent_type(&agent_type)
        {
            orphans.push(name);
        }
    }
    if orphans.is_empty() {
        return archived;
    }

    let archive_root = agents_root.join(".archive");
    if fs::create_dir_all(&archive_root).is_err() {
        return archived;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();

    let project_id = {
        let db = crate::db::shared();
        let conn = db.lock();
        resolve_project_id(&conn, project_path)
    };

    for orphan in orphans {
        let src = agents_root.join(&orphan);
        let dst = archive_root.join(format!("{}-{}", orphan, stamp));
        if fs::rename(&src, &dst).is_err() {
            continue;
        }
        if let Some(ref pid) = project_id {
            {
                let db = crate::db::shared();
                let conn = db.lock();
                let _ = WorkspaceSession::delete(&conn, pid);
                let prefix = format!(".k2so/agents/{}/", orphan);
                let _ = conn.execute(
                    "DELETE FROM workspace_heartbeats WHERE project_id = ?1 AND wakeup_path LIKE ?2 || '%'",
                    rusqlite::params![pid, prefix],
                );
            }
        }
        archived.push(orphan.clone());
        log_debug!(
            "[agent-archive] {} → .archive/{}-{} (primary={})",
            orphan,
            orphan,
            stamp,
            primary
        );
    }
    archived
}

/// GH#27 — archive a legacy agent-tree heartbeat folder by renaming it
/// to `<name>.orphaned` in place (same parent dir). WAKEUP.md files are
/// USER DATA — the orphan is never deleted, only renamed aside so the
/// canonical workspace-level tree is the single live source of truth.
///
/// Idempotent: when the `.orphaned` archive name is already taken the
/// orphan is left as-is (logged). When both the canonical WAKEUP.md and
/// the orphan's WAKEUP.md exist with DIFFERENT contents, a loud warning
/// names both paths — the canonical file stays authoritative and the
/// diverging copy is preserved under the `.orphaned` name for the user
/// to reconcile. Returns true when the rename happened.
pub fn archive_agent_tree_orphan(orphan_dir: &Path, canonical_wakeup: &Path) -> bool {
    let Some(name) = orphan_dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let orphan_wakeup = orphan_dir.join("WAKEUP.md");
    if canonical_wakeup.exists() && orphan_wakeup.exists() {
        let canon = fs::read_to_string(canonical_wakeup).unwrap_or_default();
        let orphan = fs::read_to_string(&orphan_wakeup).unwrap_or_default();
        if canon != orphan {
            log_debug!(
                "[heartbeat-repair] WARNING: WAKEUP.md contents DIVERGE — canonical {} kept authoritative; legacy agent-tree copy preserved at {}.orphaned (was {}) — reconcile manually",
                canonical_wakeup.display(),
                name,
                orphan_wakeup.display(),
            );
        }
    }
    let archived = orphan_dir.with_file_name(format!("{name}.orphaned"));
    if archived.exists() {
        log_debug!(
            "[heartbeat-repair] orphan {} left in place — archive target {} already exists",
            orphan_dir.display(),
            archived.display(),
        );
        return false;
    }
    match fs::rename(orphan_dir, &archived) {
        Ok(()) => {
            log_debug!(
                "[heartbeat-repair] archived legacy agent-tree heartbeat dir {} → {}",
                orphan_dir.display(),
                archived.display(),
            );
            true
        }
        Err(e) => {
            log_debug!(
                "[heartbeat-repair] WARN: archive {} → {}: {e}",
                orphan_dir.display(),
                archived.display(),
            );
            false
        }
    }
}

/// GH#27 Theme A — canonical-path repair for heartbeat rows.
///
/// The canonical home for every heartbeat is the WORKSPACE-level
/// `<dot>/heartbeats/<name>/WAKEUP.md` (a workspace IS the agent; `.k2/`
/// is the agent root). The agent tree `<dot>/agent/heartbeats/<name>/`
/// is legacy residue from the 0.37.0 window where the scaffolder
/// anchored on `agent_dir()`.
///
/// The pre-GH#27 version of this repair anchored its "correct" target on
/// `agent_dir(...)/heartbeats/<name>` — which post-unification RESOLVES
/// TO the legacy agent tree — and compared rows against a stale
/// `.k2so/agents/<name>/heartbeats/` prefix that never matches modern
/// rows. Net effect: every boot it INVERTED healthy canonical rows back
/// to `.k2/agent/heartbeats/...` and re-copied content into the dead
/// tree (bug report GH#27, log line `[heartbeat-repair] ... →
/// .k2/agent/heartbeats/... (source=existing path)`).
///
/// Invariants enforced per row (idempotent — a clean second boot does
/// nothing and logs nothing):
///   1. If the canonical WAKEUP.md exists, the row points there; a row
///      is NEVER re-pointed at the agent tree.
///   2. If ONLY the agent tree has the folder, it is MOVED to the
///      canonical path (user data relocated, never deleted).
///   3. If BOTH exist, canonical stays authoritative for the row and
///      the agent-tree folder is preserved as `<name>.orphaned` in
///      place; diverging contents log a loud warning naming both paths.
///   4. If NEITHER exists, content is salvaged from the legacy
///      agent-root WAKEUP.md or the row's current path (pre-0.32.1
///      semantics preserved), else a placeholder is scaffolded — always
///      at the canonical path.
pub fn repair_mismigrated_heartbeats(project_path: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };
    let Ok(rows) = AgentHeartbeat::list_by_project(&conn, &project_id) else { return };
    if rows.is_empty() {
        return;
    }

    let canonical_root = workspace_heartbeats_dir(project_path);
    let agent_root = workspace_agent_path(project_path);
    let agent_tree_root = agent_root.join("heartbeats");
    // Legacy agent-ROOT wakeup (`<dot>/agent/WAKEUP.md`) — pre-0.32.1
    // residue that may still hold the user's real content.
    let legacy_root_wakeup = agent_root.join("WAKEUP.md");

    for hb in rows {
        let canonical_dir = canonical_root.join(&hb.name);
        let canonical_wakeup = canonical_dir.join("WAKEUP.md");
        let orphan_dir = agent_tree_root.join(&hb.name);
        let orphan_wakeup = orphan_dir.join("WAKEUP.md");

        let canonical_rel = canonical_wakeup
            .strip_prefix(project_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| canonical_wakeup.to_string_lossy().to_string());
        let row_is_canonical = hb.wakeup_path == canonical_rel;

        // Idempotency fast path: row already canonical, file present,
        // no legacy agent-tree twin → nothing to do, nothing to log.
        if row_is_canonical && canonical_wakeup.exists() && !orphan_dir.exists() {
            continue;
        }

        if canonical_wakeup.exists() {
            // Invariants 1 + 3: canonical wins. Archive a lingering
            // agent-tree twin aside (contents preserved, never merged
            // over the canonical file, never deleted).
            if orphan_dir.exists() {
                archive_agent_tree_orphan(&orphan_dir, &canonical_wakeup);
            }
        } else if orphan_wakeup.exists() {
            // Invariant 2: ONLY the agent tree has it — move the whole
            // folder to the canonical home.
            if fs::create_dir_all(&canonical_root).is_err() {
                continue;
            }
            let moved = if canonical_dir.exists() {
                // Canonical dir exists but is missing its WAKEUP.md —
                // adopt the orphan's file, then clean up the leftover
                // dir (rename-aside if it still has other content).
                let file_moved = fs::rename(&orphan_wakeup, &canonical_wakeup).is_ok();
                if file_moved && fs::remove_dir(&orphan_dir).is_err() {
                    archive_agent_tree_orphan(&orphan_dir, &canonical_wakeup);
                }
                file_moved
            } else {
                fs::rename(&orphan_dir, &canonical_dir).is_ok()
            };
            if !moved {
                log_debug!(
                    "[heartbeat-repair] WARN: failed to move legacy agent-tree dir {} → {}; row left untouched this boot",
                    orphan_dir.display(),
                    canonical_dir.display(),
                );
                continue; // never point a row at a file we failed to place
            }
            log_debug!(
                "[heartbeat-repair] {} moved legacy agent-tree dir {} → {}",
                hb.name,
                orphan_dir.display(),
                canonical_dir.display(),
            );
        } else {
            // Invariant 4: NEITHER tree has the file — salvage content.
            // Template marker is `<!-- DEFAULT TEMPLATE` (from
            // wakeup_templates/*.md); a template is never a content
            // source — it would shadow the row's real edits.
            let legacy_content = fs::read_to_string(&legacy_root_wakeup).ok();
            let legacy_is_template = legacy_content
                .as_deref()
                .map(|s| s.contains("<!-- DEFAULT TEMPLATE"))
                .unwrap_or(false);
            let legacy_present = legacy_content
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
                && !legacy_is_template;

            if row_is_canonical && !legacy_present {
                // Clean up a stray template scaffold if present — it'll
                // just trick the repair into work on future runs.
                if legacy_is_template {
                    let _ = fs::remove_file(&legacy_root_wakeup);
                }
                continue;
            }
            if fs::create_dir_all(&canonical_dir).is_err() {
                continue;
            }
            // Source priority: legacy agent-root WAKEUP.md (the user's
            // real pre-0.32.1 content) → the row's current path if it
            // has non-empty content → scaffold a placeholder.
            let current_abs = Path::new(project_path).join(&hb.wakeup_path);
            let source = if legacy_present {
                Some(legacy_root_wakeup.clone())
            } else if current_abs.exists()
                && fs::read_to_string(&current_abs)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            {
                Some(current_abs.clone())
            } else {
                None
            };
            if let Some(src) = source {
                if let Ok(content) = fs::read_to_string(&src) {
                    if fs::write(&canonical_wakeup, content).is_ok() {
                        // Clean up the legacy agent-root file if we just
                        // used it — avoids dual-source-of-truth next run.
                        if src == legacy_root_wakeup {
                            let _ = fs::remove_file(&legacy_root_wakeup);
                        }
                    }
                }
            } else if !canonical_wakeup.exists() {
                let template = format!(
                    "---\ndescription: Heartbeat migrated by repair (content was missing pre-repair)\n---\n\n\
                    # Wake procedure: {}\n\n\
                    This heartbeat's wakeup file was missing when the boot repair ran.\n\
                    Edit this file with the instructions this heartbeat should run.\n",
                    hb.name
                );
                log_if_err(
                    "heartbeat-repair synth-wakeup",
                    &canonical_wakeup,
                    atomic_write_str(&canonical_wakeup, &template),
                );
            }
        }

        if !row_is_canonical {
            let _ =
                AgentHeartbeat::update_wakeup_path(&conn, &project_id, &hb.name, &canonical_rel);
            log_debug!(
                "[heartbeat-repair] {} wakeup_path {} → {} (canonical)",
                hb.name,
                hb.wakeup_path,
                canonical_rel,
            );
        }
    }
}

/// One-time promotion of the legacy `projects.heartbeat_schedule` single-slot
/// config into the multi-heartbeat `agent_heartbeats` table. Safe to call
/// repeatedly; no-ops when the project already has any agent_heartbeats
/// row (migration is idempotent). Moves the legacy `wakeup.md` to
/// `heartbeats/default/wakeup.md` so everything lives under a consistent
/// hierarchy post-migration.
pub fn promote_legacy_heartbeat(project_path: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };

    // Idempotency: skip if any heartbeat row exists for this project.
    if let Ok(existing) = AgentHeartbeat::list_by_project(&conn, &project_id) {
        if !existing.is_empty() {
            return;
        }
    }

    // Read legacy slot. If empty or null, nothing to migrate.
    let legacy: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT heartbeat_mode, heartbeat_schedule, heartbeat_last_fire \
             FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    let Some((mode, schedule, last_fire)) = legacy else { return };
    let Some(schedule_json) = schedule else { return };
    if schedule_json.trim().is_empty() {
        return;
    }

    // Parse the legacy JSON to extract frequency and spec params.
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&schedule_json) else { return };
    let frequency = v
        .get("frequency")
        .and_then(|s| s.as_str())
        .unwrap_or(match mode.as_deref() {
            Some("hourly") => "hourly",
            _ => "daily",
        })
        .to_string();

    let Some(agent_name) = find_primary_agent(project_path) else { return };

    // Move legacy wakeup.md into the CANONICAL workspace-level
    // heartbeats/default/ so the rest of the system has a single lookup
    // pattern. (GH#27: pre-fix this anchored on `agent_dir()`, which
    // post-unification resolves to the legacy `.k2/agent/` tree — a
    // boot path must never create rows pointing there.)
    let default_dir = workspace_heartbeats_dir(project_path).join("default");
    if fs::create_dir_all(&default_dir).is_err() {
        return;
    }
    let legacy_wakeup = agent_dir(project_path, &agent_name).join("WAKEUP.md");
    let new_wakeup = default_dir.join("WAKEUP.md");
    if legacy_wakeup.exists() && !new_wakeup.exists() {
        if let Ok(content) = fs::read_to_string(&legacy_wakeup) {
            if atomic_write_str(&new_wakeup, &content).is_ok() {
                log_if_err(
                    "promote_legacy_heartbeat legacy remove",
                    &legacy_wakeup,
                    fs::remove_file(&legacy_wakeup),
                );
            }
        }
    } else if !new_wakeup.exists() {
        let template = format!(
            "---\ndescription: Default heartbeat migrated from legacy single-slot schedule\n---\n\n\
            # Wake procedure: default\n\n\
            This heartbeat was auto-created by the migration from the legacy single-slot\n\
            heartbeat system. Edit this file to define what happens when this agent wakes.\n"
        );
        log_if_err(
            "promote_legacy_heartbeat scaffold",
            &new_wakeup,
            atomic_write_str(&new_wakeup, &template),
        );
    }

    let workspace_relative = new_wakeup
        .strip_prefix(project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| new_wakeup.to_string_lossy().to_string());

    let id = uuid::Uuid::new_v4().to_string();
    if AgentHeartbeat::insert(
        &conn,
        &id,
        &project_id,
        "default",
        &frequency,
        &schedule_json,
        &workspace_relative,
        true,
    )
    .is_ok()
    {
        if let Some(lf) = last_fire {
            if !lf.is_empty() {
                let _ = conn.execute(
                    "UPDATE agent_heartbeats SET last_fired = ?1 \
                     WHERE project_id = ?2 AND name = 'default'",
                    rusqlite::params![lf, project_id],
                );
            }
        }
        log_debug!(
            "[heartbeat-migrate] promoted legacy heartbeat_schedule for {} (agent={}, freq={})",
            project_path,
            agent_name,
            frequency
        );
    }
}

/// Scaffold the wakeup files for a single workspace — one for each
/// existing agent that supports wake-up. Safe to call repeatedly;
/// never overwrites an existing file. Used by the app-launch migration
/// pass.
pub fn ensure_workspace_wakeups(project_path: &str) {
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(&agents_root) else { return };
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let agent_type = agent_type_for(project_path, &name);
        ensure_agent_wakeup(project_path, &name, &agent_type);
    }
}

/// Rename lowercase `agent.md` / `wakeup.md` filenames to UPPERCASE in all
/// known locations within a workspace. Idempotent — skips files that are
/// already uppercase.
///
/// Case-insensitive filesystems (macOS HFS+, default APFS) refuse a direct
/// `fs::rename("agent.md", "AGENT.md")` — it's the same filename to the FS.
/// We two-step through a temporary name so the final result is a real case
/// change recorded in the directory entry.
pub fn migrate_filenames_to_uppercase(project_path: &str) {
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let agent_path = entry.path();

                case_rename(&agent_path.join("agent.md"), &agent_path.join("AGENT.md"));
                case_rename(&agent_path.join("wakeup.md"), &agent_path.join("WAKEUP.md"));

                let heartbeats_dir = agent_path.join("heartbeats");
                if let Ok(hb_entries) = fs::read_dir(&heartbeats_dir) {
                    for hb in hb_entries.flatten() {
                        if !hb.file_type().map_or(false, |ft| ft.is_dir()) {
                            continue;
                        }
                        let sched_path = hb.path();
                        case_rename(
                            &sched_path.join("wakeup.md"),
                            &sched_path.join("WAKEUP.md"),
                        );
                    }
                }
            }
        }
    }

    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, project_path) {
            let _ = conn.execute(
                "UPDATE agent_heartbeats \
                 SET wakeup_path = replace(wakeup_path, 'wakeup.md', 'WAKEUP.md') \
                 WHERE project_id = ?1 AND wakeup_path LIKE '%wakeup.md'",
                rusqlite::params![&project_id],
            );
        }
    }
}

/// Rename `from` → `to` with a temp-name intermediate step to survive
/// case-insensitive filesystems. No-op if `from` doesn't exist OR if
/// `to` already exists with different content (we don't want to clobber).
fn case_rename(from: &std::path::Path, to: &std::path::Path) {
    if !from.exists() {
        return;
    }
    if to.exists() {
        let from_meta = fs::metadata(from).ok();
        let to_meta = fs::metadata(to).ok();
        if let (Some(a), Some(b)) = (from_meta, to_meta) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if a.ino() != b.ino() {
                    log_debug!(
                        "[filename-migrate] both {} and {} exist with different inodes — skipping",
                        from.display(),
                        to.display()
                    );
                    return;
                }
            }
        }
    }
    let tmp = from.with_extension(format!("md.tmp-case-rename-{}", uuid::Uuid::new_v4()));
    if fs::rename(from, &tmp).is_err() {
        return;
    }
    if fs::rename(&tmp, to).is_err() {
        let _ = fs::rename(&tmp, from);
        log_debug!(
            "[filename-migrate] second-step rename failed for {} → {}",
            from.display(),
            to.display()
        );
    }
}

/// Idempotent: bails immediately if the workspace's primary already
/// has any heartbeat row, or if the project isn't in manager mode.
pub fn migrate_or_scaffold_lead_heartbeat(project_path: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };

    let agent_mode: Option<String> = conn
        .query_row(
            "SELECT agent_mode FROM projects WHERE id = ?1",
            rusqlite::params![&project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    if agent_mode.as_deref() != Some("manager") {
        return;
    }

    let has_triage: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_heartbeats \
             WHERE project_id = ?1 AND name = 'triage')",
            rusqlite::params![&project_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if has_triage {
        return;
    }

    let legacy_path = workspace_wakeup_path(project_path);
    let migrated_content: Option<String> = fs::read_to_string(&legacy_path)
        .ok()
        .filter(|s| !s.trim().is_empty());

    let wake_body = if let Some(ref existing) = migrated_content {
        if existing.trim_start().starts_with("---") {
            existing.clone()
        } else {
            format!(
                "---\ndescription: Workspace manager triage (migrated from .k2so/wakeup.md)\n---\n\n{}",
                existing
            )
        }
    } else {
        "---\ndescription: Workspace manager triage — follow your Standing Orders\n---\n\n\
         # Wake procedure: default\n\n\
         Follow your Standing Orders to triage the workspace inbox and review queue. \
         Delegate, approve, or exit — keep the session short.\n"
            .to_string()
    };

    let Some(primary_agent) = find_primary_agent(project_path) else {
        log_debug!(
            "[migrate] {}: no scheduleable agent, skipping heartbeat scaffold",
            project_path
        );
        return;
    };

    let spec = r#"{"frequency":"hourly","every_seconds":3600}"#.to_string();
    match k2so_heartbeat_add(
        project_path.to_string(),
        "triage".to_string(),
        "hourly".to_string(),
        spec,
    ) {
        Ok(_) => {
            // GH#27: write the wake body to the SAME canonical
            // workspace-level path `k2so_heartbeat_add` just scaffolded
            // (and pointed the row at) — pre-fix this wrote into the
            // legacy `.k2/agent/heartbeats/` tree, so the content never
            // reached the file the row fires from.
            let wake_path = workspace_heartbeats_dir(project_path)
                .join("triage")
                .join("WAKEUP.md");
            log_if_err(
                "migrate lead-heartbeat wakeup",
                &wake_path,
                atomic_write_str(&wake_path, &wake_body),
            );

            if migrated_content.is_some() {
                let migrated_to = legacy_path.with_file_name("wakeup.md.migrated");
                let _ = fs::rename(&legacy_path, &migrated_to);
                log_debug!(
                    "[migrate] {}: moved .k2so/wakeup.md → triage heartbeat row for agent '{}'; legacy archived as wakeup.md.migrated",
                    project_path,
                    primary_agent
                );
            } else {
                log_debug!(
                    "[migrate] {}: scaffolded lean triage heartbeat for agent '{}'",
                    project_path,
                    primary_agent
                );
            }
        }
        Err(e) => {
            log_debug!(
                "[migrate] Failed to scaffold triage heartbeat for {}: {}",
                project_path,
                e
            );
        }
    }
}

/// Startup check: warn the user if a previous regen didn't clear its
/// in-flight marker. Doesn't auto-repair — a regen is idempotent, so the
/// next real regen will overwrite any partial state — but surfaces the
/// situation so the user can check `.k2so/migration/` for stale archives
/// if they hit unexpected data loss.
pub fn detect_interrupted_regen(project_path: &str) -> bool {
    let marker = crate::workspace_dot_dir(project_path).join(".regen-in-flight");
    if !marker.exists() {
        return false;
    }
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr(),
        "k2so: previous SKILL.md regeneration at {} did not complete cleanly. \
         The next regen will overwrite any partial state; check .k2so/migration/ \
         if your workspace context looks unexpectedly stale.",
        project_path
    );
    log_if_err("clear stale regen marker", &marker, fs::remove_file(&marker));
    true
}

/// Harvest `.k2so/agents/<name>/CLAUDE.md` files left behind by the
/// pre-0.32.7 per-agent CLAUDE.md generator. Each is archived to
/// `.k2so/migration/agents/<name>/CLAUDE.md-<timestamp>.md` then removed.
///
/// Gated with `.k2so/.harvest-0.32.7-done` so a user who later runs
/// `generate-md` isn't re-harvested on the next boot. First-run only.
pub fn harvest_per_agent_claude_md_files(project_path: &str) {
    let sentinel = crate::workspace_dot_dir(project_path).join(".harvest-0.32.7-done");
    if sentinel.exists() {
        return;
    }

    let agents_root = crate::workspace_dot_dir(project_path).join("agents");
    let mut archived_paths: Vec<PathBuf> = Vec::new();
    let mut any_failure = false;
    if let Ok(read_dir) = fs::read_dir(&agents_root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with('.') {
                continue;
            }
            let claude_md = path.join("CLAUDE.md");
            if !claude_md.is_file() {
                continue;
            }
            match archive_claude_md_file(
                project_path,
                &claude_md,
                &format!("agents/{}/CLAUDE.md", name),
            ) {
                Some(archive_path) => {
                    // SAFETY: routes through `scratch_safe_trash` so
                    // test scratch paths under temp_dir() skip the
                    // trash crate (avoids macOS Touch ID prompts
                    // during cargo test).
                    if let Err(e) =
                        crate::safe_delete_scratch::scratch_safe_trash(&claude_md)
                    {
                        log_if_err::<(), _>(
                            "harvest trash original",
                            &claude_md,
                            Err::<(), _>(format!("{e}")),
                        );
                        any_failure = true;
                    }
                    archived_paths.push(archive_path);
                }
                None => {
                    any_failure = true;
                }
            }
        }
    }
    if !archived_paths.is_empty() {
        inject_first_migration_banner(project_path, &archived_paths);
    }
    if !any_failure {
        log_if_err(
            "harvest sentinel",
            &sentinel,
            fs_atomic::atomic_write(&sentinel, b""),
        );
    } else {
        log_if_err::<(), _>(
            "harvest incomplete — sentinel not stamped",
            &sentinel,
            Err::<(), &str>("retry on next boot"),
        );
    }
}

// ══════════════════════════════════════════════════════════════════════
// Archive utility helpers — shared with skill_regen + harness
// ══════════════════════════════════════════════════════════════════════
//
// These three helpers are migration-flavored ("archive user-authored
// content before mutating, log the event, banner if first time") and
// are used by the SKILL regen cluster (`workspace/skill_regen.rs`) and
// the harness file-discovery cluster (`workspace/harness.rs`) as well as
// the migration helpers above. Kept `pub(crate)` so they only escape the
// `workspace/` module family.

/// Copy a file to `.k2so/migration/<relative>-<timestamp>.<ext>`.
/// Returns the path of the archive on success.
pub(crate) fn archive_claude_md_file(
    project_path: &str,
    source: &Path,
    relative_id: &str,
) -> Option<PathBuf> {
    let content = fs::read_to_string(source).ok()?;
    let (subdir, leaf) = match relative_id.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, relative_id),
    };
    let mut target_dir = crate::workspace_dot_dir(project_path).join("migration");
    if let Some(sub) = subdir {
        target_dir = target_dir.join(sub);
    }
    if let Err(e) = fs::create_dir_all(&target_dir) {
        log_if_err::<(), _>(
            "archive_claude_md_file create_dir",
            &target_dir,
            Err::<(), _>(e),
        );
        return None;
    }
    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.to_string(), String::new()),
    };
    let archive_path = unique_archive_path(&target_dir, &leaf_stem, &leaf_ext);
    if let Err(e) = fs_atomic::atomic_write(&archive_path, content.as_bytes()) {
        log_if_err::<(), _>("archive_claude_md_file write", &archive_path, Err::<(), _>(e));
        return None;
    }
    log_adoption_event(
        project_path,
        &format!(
            "ARCHIVED {} → {}",
            source.display(),
            archive_path.display()
        ),
    );
    Some(archive_path)
}

/// MOVE (rename) a displaced user file into `.k2/migration/<relative>` — one
/// step, no copy and no leftover original to clean up afterwards. The
/// `.k2/migration/` folder is the single, in-workspace backup the user can
/// browse + restore from (we deliberately do NOT use the system recycle bin,
/// which alarms people and implies a 30-day clock). Returns the destination.
///
/// Falls back to copy-then-remove only if the rename crosses a filesystem
/// boundary (`.k2/migration/` is normally on the same FS as the workspace, so
/// the rename fast-path wins). Best-effort: on total failure it logs and
/// returns `None`, leaving the original in place rather than destroying it.
pub(crate) fn move_to_migration(
    project_path: &str,
    source: &Path,
    relative_id: &str,
) -> Option<PathBuf> {
    let (subdir, leaf) = match relative_id.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, relative_id),
    };
    let mut target_dir = crate::workspace_dot_dir(project_path).join("migration");
    if let Some(sub) = subdir {
        target_dir = target_dir.join(sub);
    }
    if let Err(e) = fs::create_dir_all(&target_dir) {
        log_if_err::<(), _>("move_to_migration create_dir", &target_dir, Err::<(), _>(e));
        return None;
    }
    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.to_string(), String::new()),
    };
    let dest = unique_archive_path(&target_dir, &leaf_stem, &leaf_ext);
    if fs::rename(source, &dest).is_err() {
        // Cross-filesystem (or other) rename failure → copy then remove.
        let content = fs::read_to_string(source).ok()?;
        if let Err(e) = fs_atomic::atomic_write(&dest, content.as_bytes()) {
            log_if_err::<(), _>("move_to_migration copy-fallback", &dest, Err::<(), _>(e));
            return None;
        }
        if let Err(e) = fs::remove_file(source) {
            log_if_err::<(), _>("move_to_migration remove-original", source, Err::<(), _>(e));
        }
    }
    log_adoption_event(
        project_path,
        &format!("MOVED {} → {}", source.display(), dest.display()),
    );
    Some(dest)
}

/// On first migration, write a standalone notice at
/// `.k2so/MIGRATION-0.32.7.md` listing the archive paths.
pub(crate) fn inject_first_migration_banner(project_path: &str, archived_paths: &[PathBuf]) {
    if archived_paths.is_empty() {
        return;
    }
    let notice_path = crate::workspace_dot_dir(project_path).join("MIGRATION-0.32.7.md");
    if notice_path.exists() {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&notice_path)
        {
            use std::io::Write;
            for p in archived_paths {
                let _ = writeln!(f, "- `{}`", p.display());
            }
        }
        return;
    }
    let mut archive_list = String::new();
    for p in archived_paths {
        archive_list.push_str(&format!("- `{}`\n", p.display()));
    }
    let body = format!(
        "<!-- K2SO:MIGRATION_BANNER:0.32.7 -->\n# ⚠️  K2SO 0.32.7 Migration Notice\n\nK2SO archived your pre-existing CLAUDE.md file(s) when unifying workspace context into a single canonical `SKILL.md`. Your original content is safe at:\n\n{archives}\nReview those archives and move anything worth keeping into one of:\n\n- `.k2so/PROJECT.md` — workspace-level context shared by every agent\n- `.k2so/agents/<name>/AGENT.md` — per-agent persona + standing orders\n- The `<!-- K2SO:USER_NOTES -->` section at the bottom of `SKILL.md` — freeform workspace notes, preserved across regenerations\n\nOnce you've reviewed, `.k2so/migration/` can be safely deleted — and so can this file.\n",
        archives = archive_list,
    );
    log_if_err(
        "migration banner",
        &notice_path,
        atomic_write_str(&notice_path, &body),
    );
    log_adoption_event(
        project_path,
        &format!(
            "WROTE .k2so/MIGRATION-0.32.7.md ({} archive(s))",
            archived_paths.len()
        ),
    );
}

/// Append a drift / conflict note to `.k2so/logs/adoption-conflicts.log`.
pub(crate) fn log_adoption_event(project_path: &str, line: &str) {
    let log_dir = crate::workspace_dot_dir(project_path).join("logs");
    let _ = fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("adoption-conflicts.log");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = format!("[{}] {}\n", ts, line);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        use std::io::Write;
        let _ = f.write_all(entry.as_bytes());
    }
}

#[cfg(test)]
mod migration_safety_tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use uuid::Uuid;
    use crate::skills::version::{SKILL_BEGIN_MARKER, SKILL_END_MARKER};
    use crate::workspace::harness::{
        safe_symlink_harness_file, scaffold_aider_conf,
        write_workspace_harness_discovery_targets,
    };
    use crate::workspace::skill_regen::{
        content_hash_of, migrate_and_symlink_root_claude_md, mtime_secs, read_regen_hashes,
        reap_old_workspace_skill_shape, write_workspace_skill_file_with_body,
        MIGRATED_NOTES_BEGIN, MIGRATED_NOTES_END, SKILL_USER_NOTES_SENTINEL,
        USER_NOTES_PLACEHOLDER,
    };
    use crate::workspace::teardown::{teardown_workspace_harness_files, TeardownMode};

    /// Make a scratch `.k2so/` scaffold for a migration test.
    fn scratch_project() -> PathBuf {
        let dir = std::env::temp_dir()
            .join("k2so-migration-test")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(dir.join(".k2so/skills")).unwrap();
        fs::create_dir_all(dir.join(".k2so/agents")).unwrap();
        dir
    }

    #[test]
    fn archive_claude_md_never_deletes_source() {
        let proj = scratch_project();
        let root_claude = proj.join("CLAUDE.md");
        let body = "# My K2SO notes\n\nThis is my workspace context.\n";
        fs::write(&root_claude, body).unwrap();

        let archive = archive_claude_md_file(
            proj.to_str().unwrap(),
            &root_claude,
            "CLAUDE.md",
        )
        .expect("archive should succeed");

        assert!(root_claude.exists(), "archive must not delete the source");
        let archived_body = fs::read_to_string(&archive).unwrap();
        assert_eq!(archived_body, body, "archive must preserve content byte-for-byte");
        assert!(
            archive.starts_with(proj.join(".k2so").join("migration")),
            "archive path must land under .k2so/migration/, got {}",
            archive.display(),
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn harvest_per_agent_claude_md_archives_then_removes_source() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        let body = "# backend-eng persona\n\nUser-authored memory.\n";
        fs::write(&agent_claude, body).unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        assert!(!agent_claude.exists(), "per-agent CLAUDE.md should be removed after harvest");
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "expected exactly one archive, got {:?}", entries);
        let archived = fs::read_to_string(entries[0].path()).unwrap();
        assert_eq!(archived, body, "archive must preserve content byte-for-byte");
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "harvest sentinel must be written"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn harvest_is_idempotent_even_if_file_regenerated_later() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "first content").unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        fs::write(&agent_claude, "user-regenerated content").unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        assert!(agent_claude.exists(), "second run must not re-harvest");
        assert_eq!(fs::read_to_string(&agent_claude).unwrap(), "user-regenerated content");
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "idempotent harvest must not double-archive");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reap_preserves_user_freeform_into_agent_md_and_discards_placeholders() {
        let proj = scratch_project();
        // Primary agent so the carried notes have a home.
        fs::create_dir_all(proj.join(".k2so/agent")).unwrap();
        fs::write(
            proj.join(".k2so/agent/AGENT.md"),
            "---\nname: agent\ntype: custom\n---\n\nPersona.\n",
        )
        .unwrap();

        let old_dir = proj.join(".k2so/skills/k2so");
        fs::create_dir_all(&old_dir).unwrap();
        let corrupted = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n\n{sentinel}\n{placeholder}\n\nMy real user note line 1.\nMy real user note line 2.\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(old_dir.join("SKILL.md"), &corrupted).unwrap();

        reap_old_workspace_skill_shape(proj.to_str().unwrap());

        let agent_md = fs::read_to_string(crate::workspace::agent_identity::persona_md_in(
            proj.join(".k2so/agent"),
        ))
        .unwrap();
        assert!(
            agent_md.contains("My real user note line 1.") && agent_md.contains("My real user note line 2."),
            "both user lines must survive into the persona, got:\n{agent_md}"
        );
        assert!(
            !agent_md.contains(USER_NOTES_PLACEHOLDER),
            "placeholder comments must be stripped from the migrated content"
        );
        assert!(
            !proj.join(".k2so/skills/k2so").exists(),
            "old composed skill must be reaped"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reap_carries_no_notes_when_tail_is_placeholder_only() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agent")).unwrap();
        fs::write(
            proj.join(".k2so/agent/AGENT.md"),
            "---\nname: agent\ntype: custom\n---\n\nPersona.\n",
        )
        .unwrap();

        let old_dir = proj.join(".k2so/skills/k2so");
        fs::create_dir_all(&old_dir).unwrap();
        let noise = format!(
            "{begin}\nManaged\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(old_dir.join("SKILL.md"), &noise).unwrap();

        reap_old_workspace_skill_shape(proj.to_str().unwrap());

        let agent_md = fs::read_to_string(crate::workspace::agent_identity::persona_md_in(
            proj.join(".k2so/agent"),
        ))
        .unwrap();
        assert!(
            !agent_md.contains(MIGRATED_NOTES_BEGIN),
            "pure K2 noise must NOT produce a migrated-notes block in the persona"
        );
        assert!(!proj.join(".k2so/skills/k2so").exists(), "old skill still reaped");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn migration_banner_is_idempotent_and_appends_new_archives() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let first_archive = proj.join(".k2so/migration/round-1.md");
        let second_archive = proj.join(".k2so/migration/round-2.md");
        fs::create_dir_all(first_archive.parent().unwrap()).unwrap();
        fs::write(&first_archive, "round 1").unwrap();
        fs::write(&second_archive, "round 2").unwrap();

        inject_first_migration_banner(project_path, &[first_archive.clone()]);

        let notice_path = proj.join(".k2so/MIGRATION-0.32.7.md");
        assert!(notice_path.exists(), "migration notice must be created");
        let after_first = fs::read_to_string(&notice_path).unwrap();
        assert!(after_first.contains("round-1"), "first archive must be referenced");
        let first_len = after_first.len();

        inject_first_migration_banner(project_path, &[second_archive.clone()]);
        let after_second = fs::read_to_string(&notice_path).unwrap();
        assert!(after_second.starts_with(&after_first), "append must preserve existing content");
        assert!(after_second.len() > first_len, "second invocation must grow the file");
        assert!(after_second.contains("round-2"), "second archive must be appended");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_archives_existing_regular_file() {
        let proj = scratch_project();
        // New shape: the canonical is `.k2/AGENTS.md`.
        let canonical = proj.join(".k2so/AGENTS.md");
        let canonical_body = "# proj\n\nGenerated AGENTS.md body.\n";
        fs::write(&canonical, canonical_body).unwrap();
        let target = proj.join("GEMINI.md");
        fs::write(&target, "user authored Gemini instructions").unwrap();

        safe_symlink_harness_file(
            &canonical,
            &target,
            proj.to_str().unwrap(),
            "GEMINI.md",
        );

        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(meta.file_type().is_symlink(), "target must be a symlink after safe-link");
        let linked_body = fs::read_to_string(&target).unwrap();
        assert!(
            linked_body.contains("Generated AGENTS.md body."),
            "the symlink must resolve to the canonical AGENTS.md body"
        );
        // The user's pre-existing file is archived (recoverable), NOT
        // merged into a defunct USER_NOTES region.
        let migration_dir = proj.join(".k2so/migration");
        let entries: Vec<_> = std::fs::read_dir(&migration_dir).unwrap().flatten().collect();
        let has_archive = entries.iter().any(|e| {
            let p = e.path();
            let body = fs::read_to_string(&p).unwrap_or_default();
            body == "user authored Gemini instructions"
        });
        assert!(
            has_archive,
            "pre-existing user file must be archived before symlink replaces it"
        );
        fs::remove_dir_all(&proj).ok();
    }

    // ---- MOVE-not-delete fan-out safety (0.40.6) -----------------------
    // The displaced-file path was converted from copy-then-recycle to a single
    // gated MOVE into `.k2/migration/`. These lock in the invariant the user
    // asked for: a fan-out can relocate a user file but can NEVER permanently
    // delete one — if the relocation can't happen, the file is left in place.

    #[test]
    fn move_to_migration_relocates_source_and_preserves_content() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let source = proj.join("CLAUDE.md");
        let body = "# user CLAUDE.md\n\nIrreplaceable user context.\n";
        fs::write(&source, body).unwrap();

        let dest = move_to_migration(project_path, &source, "CLAUDE.md")
            .expect("move_to_migration must succeed on a writable workspace");

        assert!(!source.exists(), "MOVE must remove the file from its original path");
        assert!(
            dest.starts_with(proj.join(".k2so").join("migration")),
            "dest must land under .k2/migration/, got {}",
            dest.display(),
        );
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            body,
            "moved file must preserve content byte-for-byte"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn move_to_migration_dedups_colliding_leaf_names() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();

        let first = proj.join("CLAUDE.md");
        fs::write(&first, "first").unwrap();
        let dest1 = move_to_migration(project_path, &first, "CLAUDE.md").expect("first move");

        // A second displaced file with the SAME leaf name must NOT clobber the
        // first archive — unique_archive_path gives it a distinct destination.
        let second = proj.join("CLAUDE.md");
        fs::write(&second, "second").unwrap();
        let dest2 = move_to_migration(project_path, &second, "CLAUDE.md").expect("second move");

        assert_ne!(dest1, dest2, "colliding leaf names must get distinct archive paths");
        assert_eq!(fs::read_to_string(&dest1).unwrap(), "first", "first archive intact");
        assert_eq!(fs::read_to_string(&dest2).unwrap(), "second", "second archive intact");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn move_to_migration_preserves_nested_relative_path() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let dir = proj.join(".cursor/rules");
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("k2so.mdc");
        fs::write(&source, "user cursor rule").unwrap();

        let dest = move_to_migration(project_path, &source, "cursor/rules/k2so.mdc")
            .expect("nested move must succeed");

        assert!(
            dest.starts_with(proj.join(".k2so/migration/cursor/rules")),
            "a nested relative_id must nest under .k2/migration/, got {}",
            dest.display(),
        );
        assert!(!source.exists(), "source must be gone after the move");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "user cursor rule");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn fanout_leaves_user_file_untouched_when_migration_cannot_be_written() {
        // The whole point of MOVE-not-delete: if the file can't be relocated,
        // the fan-out must NOT symlink over (and thus never destroy) the user's
        // file. Force the move to fail by occupying `.k2/migration` with a
        // regular file so `create_dir_all()` errors out.
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        // workspace_dot_dir() resolves to `.k2so/` for this scratch project.
        let migration_blocker = proj.join(".k2so/migration");
        fs::write(&migration_blocker, "not a directory").unwrap();

        let canonical = proj.join(".k2so/AGENTS.md");
        fs::write(&canonical, "# generated canon\n").unwrap();
        let target = proj.join("GEMINI.md");
        fs::write(&target, "PRECIOUS user instructions").unwrap();

        safe_symlink_harness_file(&canonical, &target, project_path, "GEMINI.md");

        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "when the move fails the user file must stay a real file, NOT be replaced by a symlink"
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "PRECIOUS user instructions",
            "user content must be byte-for-byte intact when migration is blocked"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reap_migrates_claude_md_import_block_into_agent_md() {
        // The OLD shape imported a pre-existing CLAUDE.md into the composed
        // skill's USER_NOTES via a `K2SO:IMPORT:CLAUDE_MD` block. The reap
        // must carry THAT content into AGENT.md too (it lived only there).
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agent")).unwrap();
        fs::write(
            proj.join(".k2so/agent/AGENT.md"),
            "---\nname: agent\ntype: custom\n---\n\nPersona.\n",
        )
        .unwrap();

        let old_dir = proj.join(".k2so/skills/k2so");
        fs::create_dir_all(&old_dir).unwrap();
        let with_import = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n\n\
             <!-- K2SO:IMPORT:CLAUDE_MD archive=/tmp/fake/archive.md -->\n## Imported: CLAUDE.md\n\nIMPORTED-CLAUDE-MARKER from a pre-existing memory.\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(old_dir.join("SKILL.md"), &with_import).unwrap();

        reap_old_workspace_skill_shape(proj.to_str().unwrap());

        let agent_md = fs::read_to_string(crate::workspace::agent_identity::persona_md_in(
            proj.join(".k2so/agent"),
        ))
        .unwrap();
        assert!(
            agent_md.contains("IMPORTED-CLAUDE-MARKER from a pre-existing memory."),
            "the imported CLAUDE.md content must be carried into the persona, got:\n{agent_md}"
        );
        assert!(
            agent_md.contains(MIGRATED_NOTES_BEGIN) && agent_md.contains(MIGRATED_NOTES_END),
            "carried content must be wrapped in the migrated-notes markers"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn workspace_remove_then_readd_leaves_data_intact() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "backend agent notes").unwrap();

        harvest_per_agent_claude_md_files(project_path);

        let archive_dir = proj.join(".k2so/migration/agents/backend-eng");
        let archive_files: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(archive_files.len(), 1, "first launch should archive once");
        let archived_body = fs::read_to_string(archive_files[0].path()).unwrap();
        assert_eq!(archived_body, "backend agent notes");

        harvest_per_agent_claude_md_files(project_path);

        let archive_files_after: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(
            archive_files_after.len(),
            1,
            "re-add must not duplicate archives (sentinel gates re-harvest)"
        );
        let archived_after = fs::read_to_string(archive_files_after[0].path()).unwrap();
        assert_eq!(archived_after, "backend agent notes", "archive content must survive remove+re-add");
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "sentinel persists across remove+re-add (it's filesystem, not DB)"
        );
        fs::remove_dir_all(&proj).ok();
    }

    /// Build a mock workspace that looks like the user was using every
    /// supported CLI LLM already.
    fn mock_multi_harness_workspace() -> PathBuf {
        let proj = scratch_project();
        fs::write(proj.join("CLAUDE.md"), "# Claude memory\nMy codebase notes from # memory writes.\n").unwrap();
        fs::write(proj.join("GEMINI.md"), "# Gemini instructions\nCustom Gemini behavior for this repo.\n").unwrap();
        fs::write(proj.join("AGENT.md"), "# AGENT.md\nAgent persona customizations.\n").unwrap();
        fs::write(proj.join(".goosehints"), "Goose hints — how to navigate this codebase.\n").unwrap();
        fs::write(
            proj.join(".aider.conf.yml"),
            "# Existing Aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\n",
        )
        .unwrap();
        fs::create_dir_all(proj.join(".opencode/agent")).unwrap();
        fs::write(
            proj.join(".opencode/agent/my-refactor-helper.md"),
            "# My custom OpenCode agent\nSpecialized refactoring persona.\n",
        )
        .unwrap();
        fs::create_dir_all(proj.join(".cursor/rules")).unwrap();
        fs::write(
            proj.join(".cursor/rules/my-codebase.mdc"),
            "---\nalwaysApply: true\n---\nMy project-specific Cursor rule.\n",
        )
        .unwrap();
        fs::write(
            proj.join(".k2so/PROJECT.md"),
            "# K2SO\n\nTauri workspace manager. Rust backend + React 19 frontend.\n",
        )
        .unwrap();
        // Canonical-agents gate: user-visible harness fan-out is OFF by
        // default. These migration-safety tests exercise the LEGACY
        // fan-out / teardown path (kept-and-gated), so opt that
        // workspace IN explicitly — the symlink ingest + teardown they
        // assert on only runs when the marker is present.
        crate::workspace::onboarding::set_harness_fanout_enabled(
            proj.to_str().unwrap(),
            true,
        )
        .unwrap();
        proj
    }

    #[test]
    fn add_workspace_points_harness_mirrors_at_agents_md_and_archives_user_files() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);

        // New shape: the canonical entrypoint is `.k2/AGENTS.md`, composed
        // from PROJECT.md (no primary agent in this scratch workspace).
        let canonical = proj.join(".k2so/AGENTS.md");
        assert!(canonical.exists(), "canonical AGENTS.md must be written");
        let agents_body = fs::read_to_string(&canonical).unwrap();
        assert!(
            agents_body.contains("Tauri workspace manager"),
            "AGENTS.md must merge in the PROJECT.md body"
        );
        // The two loadable skills ship.
        assert!(proj.join(".k2so/skills/k2-cli/SKILL.md").exists(), "k2-cli skill must ship");
        assert!(
            proj.join(".k2so/skills/k2-canonical-agents/SKILL.md").exists(),
            "k2-canonical-agents skill must ship"
        );
        // The OLD composed skill must NOT be written.
        assert!(
            !proj.join(".k2so/skills/k2so/SKILL.md").exists(),
            "old composed skill must never be written by the new shape"
        );

        // Leftover harness mirrors point at the canonical AGENTS.md.
        // Fan-out no longer takes over cwd AGENTS.md (generate owns it;
        // this fixture has no generate marker).
        assert!(
            !proj.join("AGENTS.md").exists(),
            "fan-out without generate must not plant cwd AGENTS.md"
        );
        for name in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints"] {
            let path = proj.join(name);
            let meta = fs::symlink_metadata(&path).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "{} should be a symlink after fan-out, got {:?}",
                name,
                meta.file_type(),
            );
            let resolved = fs::read_to_string(&path).unwrap();
            assert!(
                resolved.contains("Tauri workspace manager"),
                "{} must resolve to the canonical AGENTS.md body",
                name
            );
        }

        // Pre-existing user files were archived (recoverable), not lost.
        let migration_root = proj.join(".k2so/migration");
        let mut found_archives = 0;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                if e.path().is_file() {
                    found_archives += 1;
                }
            }
        }
        assert!(
            found_archives >= 4,
            "expected archives for CLAUDE.md/GEMINI.md/AGENT.md/.goosehints at least, got {}",
            found_archives,
        );
        // The archived bodies preserve the user's original content.
        let archived_bodies: Vec<String> = fs::read_dir(&migration_root)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| fs::read_to_string(e.path()).unwrap_or_default())
            .collect();
        assert!(
            archived_bodies.iter().any(|b| b.contains("My codebase notes from # memory writes")),
            "the original CLAUDE.md body must be archived"
        );
        assert!(
            archived_bodies.iter().any(|b| b.contains("Custom Gemini behavior for this repo")),
            "the original GEMINI.md body must be archived"
        );

        // User's own non-managed harness files are left untouched.
        assert!(
            proj.join(".opencode/agent/my-refactor-helper.md").exists(),
            "user's OpenCode agent files must be preserved untouched"
        );
        assert!(
            proj.join(".cursor/rules/my-codebase.mdc").exists(),
            "user's Cursor rule files must be preserved"
        );
        assert!(
            proj.join(".cursor/rules/k2so.mdc").exists(),
            "K2's Cursor MDC must be added"
        );

        let aider = fs::read_to_string(proj.join(".aider.conf.yml")).unwrap();
        assert!(aider.contains("AGENTS.md"), "AGENTS.md must be injected into Aider read: list");
        assert!(aider.contains("CONVENTIONS.md"), "existing Aider reads must be preserved");
        assert!(aider.contains("ARCHITECTURE.md"), "existing Aider reads must be preserved");
        assert!(aider.contains("model: gpt-4o"), "non-read keys must be preserved");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn add_workspace_is_idempotent_second_launch_changes_nothing() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let first_body = fs::read_to_string(proj.join(".k2so/AGENTS.md")).unwrap();
        let first_archives = fs::read_dir(proj.join(".k2so/migration"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .count();

        write_workspace_skill_file_with_body(project_path, None);
        let second_body = fs::read_to_string(proj.join(".k2so/AGENTS.md")).unwrap();
        let second_archives = fs::read_dir(proj.join(".k2so/migration"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .count();

        assert_eq!(first_body, second_body, "second regen must not change AGENTS.md");
        assert_eq!(
            first_archives, second_archives,
            "second regen must not re-archive (mirrors are already symlinks)"
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_keep_current_freezes_symlinks_into_real_files() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        // New shape: the frozen body is the canonical AGENTS.md.
        let canonical_body = fs::read_to_string(proj.join(".k2so/AGENTS.md")).unwrap();

        let results = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        assert!(!results.is_empty(), "teardown should report at least one action");
        assert!(
            results.iter().all(|r| r.action == "froze"),
            "keep_current should produce only 'froze' actions: {:?}",
            results
        );

        for name in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints"] {
            let path = proj.join(name);
            let meta = fs::symlink_metadata(&path).expect(name);
            assert!(
                !meta.file_type().is_symlink(),
                "{} must no longer be a symlink after teardown(keep_current)",
                name,
            );
            assert!(meta.file_type().is_file(), "{} must be a regular file", name);
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body, canonical_body, "{} must contain the frozen AGENTS.md body", name);
        }

        assert!(proj.join(".k2so/AGENTS.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_restore_original_brings_back_every_archive() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        let pre_claude = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        let pre_gemini = fs::read_to_string(proj.join("GEMINI.md")).unwrap();
        let pre_agent = fs::read_to_string(proj.join("AGENT.md")).unwrap();
        let pre_goose = fs::read_to_string(proj.join(".goosehints")).unwrap();
        let pre_aider = fs::read_to_string(proj.join(".aider.conf.yml")).unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let results = teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        assert!(!results.is_empty(), "teardown should report actions");

        assert_eq!(fs::read_to_string(proj.join("CLAUDE.md")).unwrap(), pre_claude);
        assert_eq!(fs::read_to_string(proj.join("GEMINI.md")).unwrap(), pre_gemini);
        assert_eq!(fs::read_to_string(proj.join("AGENT.md")).unwrap(), pre_agent);
        assert_eq!(fs::read_to_string(proj.join(".goosehints")).unwrap(), pre_goose);
        assert_eq!(fs::read_to_string(proj.join(".aider.conf.yml")).unwrap(), pre_aider);

        // Fan-out no longer plants cwd AGENTS.md; generate did not run
        // on this fixture, so the path stays absent through restore.
        assert!(
            !proj.join("AGENTS.md").exists(),
            "cwd AGENTS.md must stay absent when generate never planted it"
        );

        assert!(proj.join(".k2so/AGENTS.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reconnect_after_restore_original_reingests_cleanly() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        write_workspace_skill_file_with_body(project_path, None);

        assert!(fs::symlink_metadata(proj.join("CLAUDE.md")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(proj.join("GEMINI.md")).unwrap().file_type().is_symlink());

        // The canonical AGENTS.md still merges the PROJECT.md body after a
        // restore + reconnect cycle.
        let agents_body = fs::read_to_string(proj.join(".k2so/AGENTS.md")).unwrap();
        assert!(agents_body.contains("Tauri workspace manager"));
        // And the mirror resolves to it.
        let claude = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        assert!(claude.contains("Tauri workspace manager"));

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_leaves_k2so_dir_fully_intact() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let pre_project_md = fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap();

        let pre_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));
        assert!(!pre_paths.is_empty(), "expected a populated .k2so/ before teardown");

        teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let post_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));

        for p in &pre_paths {
            assert!(
                post_paths.contains(p),
                "{} disappeared from .k2so/ during teardown — invariant violated",
                p.display(),
            );
        }
        assert_eq!(fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap(), pre_project_md);

        fs::remove_dir_all(&proj).ok();
    }

    fn walk_dir(root: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                out.push(p.clone());
                if p.is_dir() && !p.is_symlink() {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn aider_conf_merge_preserves_user_reads_and_archives_original() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let aider_path = proj.join(".aider.conf.yml");
        let original = "# my aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\nauto-lint: true\n";
        fs::write(&aider_path, original).unwrap();

        scaffold_aider_conf(project_path);

        let merged = fs::read_to_string(&aider_path).unwrap();
        assert!(merged.contains("AGENTS.md"), "AGENTS.md must be injected");
        assert!(merged.contains("CONVENTIONS.md"), "original read entries preserved");
        assert!(merged.contains("ARCHITECTURE.md"), "original read entries preserved");
        assert!(merged.contains("model: gpt-4o"), "non-read top-level keys preserved");
        assert!(merged.contains("auto-lint: true"), "non-read top-level keys preserved");

        let migration_root = proj.join(".k2so/migration");
        let mut found = false;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                if let Ok(body) = fs::read_to_string(e.path()) {
                    if body == original {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "original .aider.conf.yml must be archived before mutation");

        scaffold_aider_conf(project_path);
        let second = fs::read_to_string(&aider_path).unwrap();
        assert_eq!(merged, second, "idempotent — second call must not re-inject");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_is_idempotent_when_target_is_already_symlink() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/AGENTS.md");
        fs::write(&canonical, "canonical").unwrap();
        let target = proj.join(".goosehints");

        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");
        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");

        let migration_dir = proj.join(".k2so/migration");
        let entries_count = std::fs::read_dir(&migration_dir)
            .map(|r| r.flatten().count())
            .unwrap_or(0);
        assert_eq!(
            entries_count, 0,
            "symlink-to-symlink re-run must not produce spurious archive entries"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn completed_regen_clears_in_flight_marker() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let marker = proj.join(".k2so/.regen-in-flight");
        assert!(!marker.exists(), "regen marker must be cleared on successful completion");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_flags_stale_marker_once() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let k2so_dir = proj.join(".k2so");
        fs::create_dir_all(&k2so_dir).unwrap();
        let marker = k2so_dir.join(".regen-in-flight");
        fs::write(&marker, b"").unwrap();
        assert!(detect_interrupted_regen(project_path), "must flag the stale marker");
        assert!(!marker.exists(), "must clear the marker after surfacing the warning");
        assert!(!detect_interrupted_regen(project_path), "must not re-fire after the marker is cleared");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_is_silent_when_no_marker() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so")).unwrap();
        assert!(!detect_interrupted_regen(proj.to_str().unwrap()));
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn archive_names_never_collide_under_rapid_fire() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let agents = proj.join(".k2so/agents");
        fs::create_dir_all(&agents).unwrap();
        for i in 0..10 {
            let agent_dir = agents.join(format!("agent-{}", i));
            fs::create_dir_all(&agent_dir).unwrap();
            fs::write(agent_dir.join("CLAUDE.md"), format!("body for agent-{}", i)).unwrap();
        }
        harvest_per_agent_claude_md_files(project_path);

        let mut archive_bodies = std::collections::HashSet::new();
        let migration_root = proj.join(".k2so/migration/agents");
        for i in 0..10 {
            let sub = migration_root.join(format!("agent-{}", i));
            let mut count = 0;
            if let Ok(entries) = fs::read_dir(&sub) {
                for e in entries.flatten() {
                    if let Ok(body) = fs::read_to_string(e.path()) {
                        assert!(archive_bodies.insert(body), "duplicate archive body found");
                        count += 1;
                    }
                }
            }
            assert_eq!(count, 1, "agent-{}: expected 1 archive, got {}", i, count);
        }
        assert_eq!(archive_bodies.len(), 10, "all 10 agents must have distinct archives");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_keep_current_leaves_file_usable_even_on_tight_retries() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let claude = proj.join("CLAUDE.md");
        assert!(claude.exists(), "CLAUDE.md must exist after first keep_current");
        let first_body = fs::read_to_string(&claude).unwrap();
        assert!(!first_body.is_empty());

        for _ in 0..5 {
            let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        }
        let final_body = fs::read_to_string(&claude).unwrap();
        assert_eq!(first_body, final_body, "repeated no-op teardowns must not mutate the frozen body");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regen_stamps_content_hashes_for_drift_detection() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let stamp_path = proj.join(".k2so/.last-skill-regen");
        let body = fs::read_to_string(&stamp_path).expect("stamp must exist");
        assert!(!body.trim().is_empty(), "stamp must no longer be empty (hash JSON required)");
        let parsed: std::collections::HashMap<String, String> =
            serde_json::from_str(&body).expect("stamp must parse as JSON hash map");
        assert!(parsed.contains_key("project_md"), "PROJECT.md hash must be recorded: {:?}", parsed);

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_prefers_content_hash_over_mtime() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let project_md = proj.join(".k2so/PROJECT.md");
        let original = fs::read_to_string(&project_md).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&project_md, &original).unwrap();
        assert!(
            mtime_secs(&project_md) > mtime_secs(&proj.join(".k2so/.last-skill-regen")),
            "test setup: source mtime must be newer than regen stamp"
        );

        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_eq!(stored, current, "hash-based drift detection must ignore identical content");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_detects_real_content_change() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let project_md = proj.join(".k2so/PROJECT.md");
        fs::write(&project_md, "completely different body\n").unwrap();

        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_ne!(stored, current, "hash-based drift detection must flag modified content");

        fs::remove_dir_all(&proj).ok();
    }

    // ──────────────────────────────────────────────────────────────────
    // 0.40.6 data-safety hardening: every displaced USER harness file is
    // archived to `.k2/migration/` (incl. empty files), then recycled —
    // never hard-deleted. We assert the ARCHIVE invariant directly (the
    // recycle-bin move is bypassed for temp-dir scratch by
    // `scratch_safe_trash`, so `cargo test` never hangs on a Touch-ID
    // prompt and we never assert real Trash contents — per
    // `feedback_recycle_bin_tests`).
    // ──────────────────────────────────────────────────────────────────

    /// Count regular files anywhere under `.k2so/migration/`.
    fn migration_archive_count(proj: &Path) -> usize {
        fn walk(dir: &Path, acc: &mut usize) {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, acc);
                } else {
                    *acc += 1;
                }
            }
        }
        let mut n = 0;
        walk(&proj.join(".k2so").join("migration"), &mut n);
        n
    }

    /// Return true if SOME archive anywhere under `.k2/migration/` holds
    /// exactly `needle` as its content.
    fn archive_holds_content(proj: &Path, needle: &str) -> bool {
        fn walk(dir: &Path, needle: &str) -> bool {
            let Ok(rd) = fs::read_dir(dir) else { return false };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if walk(&p, needle) {
                        return true;
                    }
                } else if fs::read_to_string(&p).map(|b| b == needle).unwrap_or(false) {
                    return true;
                }
            }
            false
        }
        walk(&proj.join(".k2so").join("migration"), needle)
    }

    #[test]
    fn fanout_archives_nonempty_claude_md_then_symlinks_to_canon() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/AGENTS.md");
        fs::write(&canonical, "# canon\n\nGenerated AGENTS.md body.\n").unwrap();
        let root_claude = proj.join("CLAUDE.md");
        let original = "# my own claude memory\n\nDo X then Y.\n";
        fs::write(&root_claude, original).unwrap();

        migrate_and_symlink_root_claude_md(&canonical, &root_claude, proj.to_str().unwrap());

        // (a) the original content is archived under .k2/migration/.
        assert!(
            archive_holds_content(&proj, original),
            "user CLAUDE.md must be archived with original content before displacement"
        );
        // (b) the live path is now a symlink resolving to the canon body.
        let meta = fs::symlink_metadata(&root_claude).unwrap();
        assert!(meta.file_type().is_symlink(), "CLAUDE.md must be a symlink after fan-out");
        assert!(
            fs::read_to_string(&root_claude)
                .unwrap()
                .contains("Generated AGENTS.md body."),
            "symlink must resolve to the canonical AGENTS.md body"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn write_path_fanout_does_not_archive_user_agents_md() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        crate::workspace::onboarding::set_harness_fanout_enabled(project_path, true).unwrap();
        crate::workspace::onboarding::set_agents_md_generate_enabled(project_path, true).unwrap();
        let original = "user-authored AGENTS.md content\n";
        fs::write(proj.join("AGENTS.md"), original).unwrap();

        write_workspace_skill_file_with_body(project_path, None);

        let kept = fs::read_to_string(proj.join("AGENTS.md")).expect("user file remains");
        assert_eq!(kept, original, "generate must not archive a user AGENTS.md");
        let meta = fs::symlink_metadata(proj.join("AGENTS.md")).expect("still present");
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "fan-out must not replace cwd AGENTS.md with a symlink"
        );
        assert!(
            !archive_holds_content(&proj, original),
            "no .k2/migration/ entry for user AGENTS.md"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn fanout_archives_user_cursor_mdc_before_overwrite() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/AGENTS.md");
        fs::write(&canonical, "# canon\n\nCanonical cursor body.\n").unwrap();
        // A user-authored cursor rule (NO K2 signature) at the managed path.
        let cursor_dir = proj.join(".cursor").join("rules");
        fs::create_dir_all(&cursor_dir).unwrap();
        let cursor_file = cursor_dir.join("k2so.mdc");
        let original = "---\ndescription: my own rule\n---\n\nUser cursor body.\n";
        fs::write(&cursor_file, original).unwrap();

        write_workspace_harness_discovery_targets(proj.to_str().unwrap(), &canonical);

        assert!(
            archive_holds_content(&proj, original),
            "user-authored cursor mdc must be archived before K2 overwrites it"
        );
        // The live file is now OUR generated output, not the user's.
        let after = fs::read_to_string(&cursor_file).unwrap();
        assert!(
            after.contains("Canonical cursor body."),
            "cursor mdc must now carry the canonical body"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn fanout_archives_empty_user_file_not_silently_dropped() {
        // The pre-0.40.6 code skipped archiving when content.trim() was
        // empty — silently dropping the user's (empty) file on displacement.
        // It must now be archived like any other displaced user file.
        let proj = scratch_project();
        let canonical = proj.join(".k2so/AGENTS.md");
        fs::write(&canonical, "# canon\n\nbody\n").unwrap();
        let target = proj.join("GEMINI.md");
        fs::write(&target, "").unwrap(); // empty user file

        let before = migration_archive_count(&proj);
        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), "GEMINI.md");
        let after = migration_archive_count(&proj);

        assert_eq!(
            after,
            before + 1,
            "an EMPTY displaced user file must still be archived (not silently dropped)"
        );
        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(meta.file_type().is_symlink(), "GEMINI.md must be a symlink after fan-out");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn fanout_is_idempotent_already_symlinked_archives_nothing_new() {
        // R5: re-running fan-out on an already-symlinked workspace must
        // archive (and recycle) NOTHING new — symlinks refresh in place.
        let proj = scratch_project();
        let canonical = proj.join(".k2so/AGENTS.md");
        fs::write(&canonical, "# canon\n\nbody\n").unwrap();

        // First pass: displace a real user file (one archive expected).
        let claude = proj.join("CLAUDE.md");
        fs::write(&claude, "user claude\n").unwrap();
        migrate_and_symlink_root_claude_md(&canonical, &claude, proj.to_str().unwrap());

        // Also seed a discovery target so the second pass exercises both.
        let gemini = proj.join("GEMINI.md");
        fs::write(&gemini, "user gemini\n").unwrap();
        safe_symlink_harness_file(&canonical, &gemini, proj.to_str().unwrap(), "GEMINI.md");

        let count_after_first = migration_archive_count(&proj);
        assert!(count_after_first >= 2, "first pass should have archived the two user files");

        // Second pass over the now-symlinked paths: nothing new.
        migrate_and_symlink_root_claude_md(&canonical, &claude, proj.to_str().unwrap());
        safe_symlink_harness_file(&canonical, &gemini, proj.to_str().unwrap(), "GEMINI.md");

        let count_after_second = migration_archive_count(&proj);
        assert_eq!(
            count_after_second, count_after_first,
            "re-running fan-out on already-symlinked paths must archive nothing new (R5)"
        );
        // Both still symlinks.
        assert!(fs::symlink_metadata(&claude).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(&gemini).unwrap().file_type().is_symlink());
        fs::remove_dir_all(&proj).ok();
    }
}




#[cfg(test)]
mod heartbeat_canonical_repair_tests {
    //! GH#27 Theme A — boot-time repair must converge every heartbeat
    //! row + folder on the CANONICAL `.k2/heartbeats/<name>/WAKEUP.md`
    //! and never resurrect the legacy `.k2/agent/heartbeats/` tree.
    //! WAKEUP.md files are USER DATA: every branch below asserts content
    //! survival, and the whole flow must be a no-op on a second run.

    use super::*;
    use crate::db::schema::AgentHeartbeat;
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Scratch workspace with a modern `.k2/` dot-dir + registered
    /// project row in the shared test DB. Unique per test.
    fn scratch_ws(label: &str) -> (PathBuf, String, String) {
        crate::db::init_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "k2-gh27-repair-{label}-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir_all(dir.join(".k2")).unwrap();
        let path_str = dir.to_string_lossy().into_owned();
        let project_id = Uuid::new_v4().to_string();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'gh27-repair', ?2)",
                rusqlite::params![project_id, path_str],
            )
            .unwrap();
        }
        (dir, path_str, project_id)
    }

    fn insert_hb(project_id: &str, name: &str, wakeup_rel: &str) {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::insert(
            &conn,
            &Uuid::new_v4().to_string(),
            project_id,
            name,
            "daily",
            "{}",
            wakeup_rel,
            true,
        )
        .unwrap();
    }

    fn row_wakeup_path(project_id: &str, name: &str) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        AgentHeartbeat::get_by_name(&conn, project_id, name)
            .unwrap()
            .expect("heartbeat row exists")
            .wakeup_path
    }

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// Rule 2: ONLY the agent tree has the folder → the whole folder is
    /// MOVED to `.k2/heartbeats/<name>/` and the row is re-pointed.
    #[test]
    fn agent_only_tree_is_moved_to_canonical_and_row_repointed() {
        let (dir, path, pid) = scratch_ws("agent-only");
        let orphan_wakeup = dir.join(".k2/agent/heartbeats/hb-a/WAKEUP.md");
        write(&orphan_wakeup, "AGENT-TREE USER CONTENT");
        // A sibling file rides along to prove the MOVE takes the folder.
        write(&dir.join(".k2/agent/heartbeats/hb-a/notes.md"), "sibling");
        insert_hb(&pid, "hb-a", ".k2/agent/heartbeats/hb-a/WAKEUP.md");

        repair_mismigrated_heartbeats(&path);

        let canonical = dir.join(".k2/heartbeats/hb-a/WAKEUP.md");
        assert_eq!(
            fs::read_to_string(&canonical).unwrap(),
            "AGENT-TREE USER CONTENT",
            "content must survive the move byte-for-byte"
        );
        assert_eq!(
            fs::read_to_string(dir.join(".k2/heartbeats/hb-a/notes.md")).unwrap(),
            "sibling",
            "the whole folder moves, not just WAKEUP.md"
        );
        assert!(
            !dir.join(".k2/agent/heartbeats/hb-a").exists(),
            "legacy agent-tree dir must be gone after the move"
        );
        assert_eq!(row_wakeup_path(&pid, "hb-a"), ".k2/heartbeats/hb-a/WAKEUP.md");
        fs::remove_dir_all(&dir).ok();
    }

    /// Rule 3: BOTH trees exist with DIVERGING content → canonical wins
    /// the row, canonical content is untouched, and the diverging
    /// agent-tree copy is preserved (not deleted) as `<name>.orphaned`.
    #[test]
    fn both_exist_divergent_canonical_wins_and_orphan_archived_with_content() {
        let (dir, path, pid) = scratch_ws("divergent");
        write(&dir.join(".k2/heartbeats/hb-b/WAKEUP.md"), "CANONICAL EDITS");
        write(
            &dir.join(".k2/agent/heartbeats/hb-b/WAKEUP.md"),
            "DIVERGED LEGACY EDITS",
        );
        // Row pinned at the DEAD tree — the exact Cortana bug shape.
        insert_hb(&pid, "hb-b", ".k2/agent/heartbeats/hb-b/WAKEUP.md");

        repair_mismigrated_heartbeats(&path);

        assert_eq!(row_wakeup_path(&pid, "hb-b"), ".k2/heartbeats/hb-b/WAKEUP.md");
        assert_eq!(
            fs::read_to_string(dir.join(".k2/heartbeats/hb-b/WAKEUP.md")).unwrap(),
            "CANONICAL EDITS",
            "canonical file stays authoritative — never overwritten by the orphan"
        );
        assert!(
            !dir.join(".k2/agent/heartbeats/hb-b").exists(),
            "live-looking legacy dir must not remain"
        );
        assert_eq!(
            fs::read_to_string(dir.join(".k2/agent/heartbeats/hb-b.orphaned/WAKEUP.md"))
                .unwrap(),
            "DIVERGED LEGACY EDITS",
            "diverging orphan content is USER DATA and must be preserved under .orphaned"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// THE INVERSION REGRESSION (GH#27 smoking gun): a healthy row
    /// already pointing at canonical must stay there — pre-fix the
    /// repair re-pointed it to `.k2/agent/heartbeats/` every boot. A
    /// lingering agent-tree twin is archived aside.
    #[test]
    fn canonical_row_is_never_repointed_to_agent_tree() {
        let (dir, path, pid) = scratch_ws("no-invert");
        write(&dir.join(".k2/heartbeats/hb-c/WAKEUP.md"), "CANON");
        write(&dir.join(".k2/agent/heartbeats/hb-c/WAKEUP.md"), "CANON");
        insert_hb(&pid, "hb-c", ".k2/heartbeats/hb-c/WAKEUP.md");

        repair_mismigrated_heartbeats(&path);

        assert_eq!(
            row_wakeup_path(&pid, "hb-c"),
            ".k2/heartbeats/hb-c/WAKEUP.md",
            "repair must NEVER re-point a canonical row at .k2/agent/heartbeats/"
        );
        assert!(dir.join(".k2/agent/heartbeats/hb-c.orphaned").exists());
        assert!(!dir.join(".k2/agent/heartbeats/hb-c").exists());
        fs::remove_dir_all(&dir).ok();
    }

    /// Already-canonical with no legacy residue: repair touches nothing.
    #[test]
    fn already_canonical_workspace_is_untouched() {
        let (dir, path, pid) = scratch_ws("clean");
        write(&dir.join(".k2/heartbeats/hb-d/WAKEUP.md"), "STABLE");
        insert_hb(&pid, "hb-d", ".k2/heartbeats/hb-d/WAKEUP.md");

        repair_mismigrated_heartbeats(&path);

        assert_eq!(row_wakeup_path(&pid, "hb-d"), ".k2/heartbeats/hb-d/WAKEUP.md");
        assert_eq!(
            fs::read_to_string(dir.join(".k2/heartbeats/hb-d/WAKEUP.md")).unwrap(),
            "STABLE"
        );
        assert!(
            !dir.join(".k2/agent").exists(),
            "repair must not scaffold any .k2/agent/ tree on a clean workspace"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// Rule 4 (idempotency): a second boot after any repair is a
    /// complete no-op — nothing re-pointed, nothing re-archived, no
    /// `.orphaned.orphaned` towers.
    #[test]
    fn second_run_is_a_noop() {
        let (dir, path, pid) = scratch_ws("idempotent");
        // Divergent both-exist case — the busiest branch.
        write(&dir.join(".k2/heartbeats/hb-e/WAKEUP.md"), "CANON");
        write(&dir.join(".k2/agent/heartbeats/hb-e/WAKEUP.md"), "LEGACY");
        insert_hb(&pid, "hb-e", ".k2/agent/heartbeats/hb-e/WAKEUP.md");
        // Agent-only case rides along.
        write(&dir.join(".k2/agent/heartbeats/hb-f/WAKEUP.md"), "MOVE ME");
        insert_hb(&pid, "hb-f", ".k2/agent/heartbeats/hb-f/WAKEUP.md");

        repair_mismigrated_heartbeats(&path);
        let snapshot = |p: &Path| -> Vec<String> {
            let mut all: Vec<String> = walk(p);
            all.sort();
            all
        };
        fn walk(p: &Path) -> Vec<String> {
            let mut out = vec![];
            if let Ok(entries) = fs::read_dir(p) {
                for e in entries.flatten() {
                    let path = e.path();
                    out.push(path.to_string_lossy().into_owned());
                    if path.is_dir() {
                        out.extend(walk(&path));
                    }
                }
            }
            out
        }
        let tree_after_first = snapshot(&dir);
        let rows_after_first = (
            row_wakeup_path(&pid, "hb-e"),
            row_wakeup_path(&pid, "hb-f"),
        );

        repair_mismigrated_heartbeats(&path);

        assert_eq!(snapshot(&dir), tree_after_first, "second run must not touch the tree");
        assert_eq!(
            (row_wakeup_path(&pid, "hb-e"), row_wakeup_path(&pid, "hb-f")),
            rows_after_first,
            "second run must not touch the rows"
        );
        assert_eq!(rows_after_first.0, ".k2/heartbeats/hb-e/WAKEUP.md");
        assert_eq!(rows_after_first.1, ".k2/heartbeats/hb-f/WAKEUP.md");
        assert!(
            !dir.join(".k2/agent/heartbeats/hb-e.orphaned.orphaned").exists(),
            "no archive-of-archive towers"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// `archive_agent_tree_orphan` alone: when the archive name is
    /// already taken, the orphan stays put (idempotent, nothing lost).
    #[test]
    fn archive_orphan_leaves_dir_when_target_taken() {
        let base = std::env::temp_dir().join(format!(
            "k2-gh27-archive-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        let orphan = base.join("hb-x");
        write(&orphan.join("WAKEUP.md"), "SECOND ORPHAN");
        write(&base.join("hb-x.orphaned/WAKEUP.md"), "FIRST ORPHAN");
        let canonical = base.join("canonical-WAKEUP.md");
        write(&canonical, "CANON");

        let archived = archive_agent_tree_orphan(&orphan, &canonical);

        assert!(!archived, "occupied archive target must refuse the rename");
        assert_eq!(
            fs::read_to_string(orphan.join("WAKEUP.md")).unwrap(),
            "SECOND ORPHAN",
            "refused orphan stays in place, content intact"
        );
        assert_eq!(
            fs::read_to_string(base.join("hb-x.orphaned/WAKEUP.md")).unwrap(),
            "FIRST ORPHAN",
            "existing archive is never clobbered"
        );
        fs::remove_dir_all(&base).ok();
    }
}
