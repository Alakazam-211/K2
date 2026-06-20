//! 0.37.0 workspace–agent unification migration.
//!
//! Collapses the multi-agent `.k2so/agents/<name>/...` layout into
//! the single-agent layout the product invariant has long implied:
//!
//! ```text
//! .k2so/
//! ├── agent/           (the workspace's one agent)
//! ├── agent-templates/ (role personas for delegation)
//! ├── work/            (workspace inbox/active/done)
//! ├── heartbeats/      (workspace-level wake schedules)
//! └── migration/legacy/ (everything archived here)
//! ```
//!
//! Idempotent — gated by sentinel `.k2so/.unification-0.37.0-done`.
//! Atomic per workspace: either the whole migration runs and the
//! sentinel lands, or any partial state is recoverable from
//! `.k2so/migration/legacy/` (originals are *moved*, never deleted,
//! except for compiled outputs CLAUDE.md/SKILL.md which regen).
//!
//! Caller responsibility (daemon boot or CLI): pass `agent_mode`
//! from `projects.agent_mode`. The module deliberately takes no DB
//! handle so the migration is testable against ephemeral
//! tempdirs without spinning the SQLite shared singleton.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::workspace::agent_identity::{agent_dir, agent_type_for, agents_dir};
use crate::fs_atomic::atomic_write_str;

/// Sentinel filename. Presence under `.k2so/` means the unification
/// migration has already run for this workspace.
pub const SENTINEL_FILENAME: &str = ".unification-0.37.0-done";

/// User-facing migration notice filename.
pub const MIGRATION_NOTICE_FILENAME: &str = "MIGRATION-0.37.0.md";

/// Outcome summary returned to the caller (logs, daemon status, tests).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnificationOutcome {
    /// `Some(name)` if a primary agent was migrated to `.k2so/agent/`.
    pub primary_migrated: Option<String>,
    /// Templates moved to `.k2so/agent-templates/<name>/`.
    pub templates_migrated: Vec<String>,
    /// Sub-paths archived to `.k2so/migration/legacy/`.
    pub legacy_archived: Vec<String>,
    /// Count of work items merged from primary agent into workspace inbox.
    pub work_items_merged: usize,
    /// Filenames that collided in the merged inbox (kept under
    /// `.k2so/migration/legacy/work-conflicts/`); user resolves later.
    pub conflicts: Vec<String>,
    /// True when the sentinel was already present (no work done).
    pub already_done: bool,
}

/// Path to the sentinel file for a given workspace.
pub fn sentinel_path(project_path: &str) -> PathBuf {
    // Resolve via workspace_dot_dir so the gate finds the sentinel after the
    // 0.40.4 `.k2so/` -> `.k2/` cutover moved it. Hardcoding `.k2so` here was
    // the bug that made this migration re-fire on every post-cutover boot and
    // rebuild a stray `.k2so/` scaffolding tree.
    crate::workspace_dot_dir(project_path).join(SENTINEL_FILENAME)
}

/// Cheap stat — true if the unification migration has already run
/// for this workspace.
pub fn is_unification_done(project_path: &str) -> bool {
    sentinel_path(project_path).exists()
}

/// Run the unification migration for one workspace. Idempotent —
/// returns immediately with `already_done = true` if the sentinel
/// is present.
///
/// `agent_mode` is the value of `projects.agent_mode` for this
/// workspace; one of `"off" | "manager" | "custom" | "k2so" |
/// "agent"`. Anything else is treated as `"off"` (no primary
/// migration; templates + cleanup still run).
pub fn run_unification(
    project_path: &str,
    agent_mode: &str,
) -> Result<UnificationOutcome, String> {
    if is_unification_done(project_path) {
        return Ok(UnificationOutcome {
            already_done: true,
            ..Default::default()
        });
    }

    let mut outcome = UnificationOutcome::default();
    // Anchor on the resolver so a (rare) un-unified workspace that the cutover
    // renamed to `.k2/` scaffolds there, never recreating a stray `.k2so/`.
    let dot_k2so = crate::workspace_dot_dir(project_path);

    // Step 2: ensure the new top-level dirs exist before any moves.
    // create_dir_all is idempotent.
    let agent_dir_new = dot_k2so.join("agent");
    let agent_templates_dir = dot_k2so.join("agent-templates");
    let heartbeats_dir = dot_k2so.join("heartbeats");
    let legacy_dir = dot_k2so.join("migration").join("legacy");
    for d in [&agent_dir_new, &agent_templates_dir, &heartbeats_dir, &legacy_dir] {
        fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }

    // Resolve primary BEFORE we start moving things — otherwise type
    // detection (which reads AGENT.md from agents/<n>/) would race
    // with the moves we're about to make.
    let primary = resolve_primary_for_migration(project_path, agent_mode);

    // Step 3: primary agent → .k2so/agent/, work, heartbeats.
    if let Some(name) = &primary {
        migrate_primary_agent(project_path, name, &mut outcome)?;
        outcome.primary_migrated = Some(name.clone());
    }

    // Step 4: every other dir under agents/ that isn't __lead__ or
    // .archive becomes a template.
    migrate_templates(project_path, primary.as_deref(), &mut outcome)?;

    // Step 5+6: __lead__ and .archive go to legacy.
    archive_special_dirs(project_path, &mut outcome)?;

    // Step 7: agents/ should be empty by now — best-effort rmdir.
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        // If non-empty (data drift), move the residue so we don't
        // leave a half-state. Better to over-archive than to skip.
        let is_empty = fs::read_dir(&agents_root)
            .map(|rd| rd.flatten().next().is_none())
            .unwrap_or(false);
        if is_empty {
            let _ = fs::remove_dir(&agents_root);
        } else {
            let dest = legacy_dir.join("agents-residue");
            if let Err(e) = move_dir(&agents_root, &dest) {
                return Err(format!("archive agents/ residue: {e}"));
            }
            outcome.legacy_archived.push("agents-residue".to_string());
        }
    }

    // Step 8: aged-out root cruft → legacy.
    archive_root_cruft(project_path, &mut outcome)?;

    // Step 9 + 10: stamp sentinel + write user-facing notice.
    write_migration_notice(project_path, &outcome)?;
    write_sentinel(project_path, &outcome)?;

    Ok(outcome)
}

// ── Primary agent migration ────────────────────────────────────────

fn migrate_primary_agent(
    project_path: &str,
    name: &str,
    outcome: &mut UnificationOutcome,
) -> Result<(), String> {
    let src = agent_dir(project_path, name);
    if !src.exists() {
        // Drift: declared mode said primary exists but the dir is
        // gone. Don't fail — outcome.primary_migrated stays None
        // and templates + cleanup still run.
        outcome.primary_migrated = None;
        return Ok(());
    }
    let dst_root = PathBuf::from(project_path).join(".k2so");

    // 3a: AGENT.md → .k2so/agent/AGENT.md
    let agent_md_src = src.join("AGENT.md");
    let agent_md_dst = dst_root.join("agent").join("AGENT.md");
    if agent_md_src.exists() {
        fs::rename(&agent_md_src, &agent_md_dst)
            .map_err(|e| format!("move AGENT.md: {e}"))?;
    }

    // 3b: work/{inbox,active,done}/* → .k2so/work/{inbox,active,done}/
    let work_src = src.join("work");
    let work_dst = dst_root.join("work");
    if work_src.exists() {
        outcome.work_items_merged += merge_work_dirs(&work_src, &work_dst, outcome)?;
        // 0.37.6: trash the now-empty (or post-merge) work dir
        // so any unmerged residue is recoverable.
        //
        // SAFETY: routes through `scratch_safe_trash` so test scratch
        // paths under temp_dir() skip the trash crate (avoids macOS
        // Touch ID prompts during cargo test).
        let _ = crate::safe_delete_scratch::scratch_safe_trash(&work_src);
    }

    // 3c: heartbeats/<sched>/* → .k2so/heartbeats/<sched>/*
    let hb_src = src.join("heartbeats");
    let hb_dst = dst_root.join("heartbeats");
    if hb_src.exists() {
        merge_heartbeats(&hb_src, &hb_dst)?;
        // 0.37.6: trash post-merge — heartbeat content (WAKEUP.md
        // edits) was just copied to dst, but sending the source to
        // Trash gives recoverability if the merge missed something.
        let _ = crate::safe_delete_scratch::scratch_safe_trash(&hb_src);
    }

    // 3d: delete compiled outputs that regen.
    // 0.37.6: trash, not permanent. CLAUDE.md and SKILL.md may
    // contain user edits the regen pipeline might not perfectly
    // preserve — keep them recoverable.
    for f in ["CLAUDE.md", "SKILL.md"] {
        let p = src.join(f);
        if p.exists() {
            let _ = crate::safe_delete_scratch::scratch_safe_trash(&p);
        }
    }

    // Anything else left under <primary>/ that we didn't recognize
    // (e.g. .lock, agent.md.bak, persona-backups/) — move to legacy
    // so it's recoverable rather than silently deleted.
    if src.exists() {
        let leftover = collect_leftover_entries(&src);
        if !leftover.is_empty() {
            let dest = dst_root
                .join("migration")
                .join("legacy")
                .join(format!("primary-{name}-residue"));
            for entry in &leftover {
                let from = src.join(entry);
                let to = dest.join(entry);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).ok();
                }
                if let Err(e) = move_path(&from, &to) {
                    return Err(format!("archive primary residue {entry}: {e}"));
                }
            }
            outcome
                .legacy_archived
                .push(format!("primary-{name}-residue"));
        }
        // Now empty (modulo race) — try to remove the agents/<primary>/ dir.
        let _ = fs::remove_dir(&src);
    }

    Ok(())
}

fn collect_leftover_entries(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

fn merge_work_dirs(
    work_src: &Path,
    work_dst: &Path,
    outcome: &mut UnificationOutcome,
) -> Result<usize, String> {
    let mut merged = 0usize;
    for sub in ["inbox", "active", "done"] {
        let from_dir = work_src.join(sub);
        let to_dir = work_dst.join(sub);
        if !from_dir.exists() {
            continue;
        }
        fs::create_dir_all(&to_dir).map_err(|e| format!("mkdir {}: {e}", to_dir.display()))?;
        let Ok(rd) = fs::read_dir(&from_dir) else { continue };
        for entry in rd.flatten() {
            let name = entry.file_name();
            let from = from_dir.join(&name);
            let to = to_dir.join(&name);
            if to.exists() {
                // Conflict: workspace inbox already has a file with
                // this name. Stash the agent-side copy in legacy
                // so the user can resolve manually.
                let conflict_dir = work_dst
                    .parent()
                    .map(|p| p.join("migration").join("legacy").join("work-conflicts").join(sub))
                    .unwrap_or_else(|| PathBuf::from("work-conflicts").join(sub));
                fs::create_dir_all(&conflict_dir)
                    .map_err(|e| format!("mkdir conflicts: {e}"))?;
                let stash = conflict_dir.join(&name);
                move_path(&from, &stash).map_err(|e| format!("stash conflict: {e}"))?;
                outcome
                    .conflicts
                    .push(format!("{sub}/{}", name.to_string_lossy()));
            } else {
                move_path(&from, &to).map_err(|e| format!("move work item: {e}"))?;
                merged += 1;
            }
        }
    }
    Ok(merged)
}

fn merge_heartbeats(hb_src: &Path, hb_dst: &Path) -> Result<(), String> {
    fs::create_dir_all(hb_dst).map_err(|e| format!("mkdir heartbeats: {e}"))?;
    let Ok(rd) = fs::read_dir(hb_src) else { return Ok(()) };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let from = hb_src.join(&name);
        let to = hb_dst.join(&name);
        if !to.exists() {
            move_path(&from, &to).map_err(|e| format!("move heartbeat: {e}"))?;
        }
        // If destination already exists (workspace already has a
        // heartbeats/ from somewhere) — leave both, user resolves.
    }
    Ok(())
}

// ── Templates ──────────────────────────────────────────────────────

fn migrate_templates(
    project_path: &str,
    primary: Option<&str>,
    outcome: &mut UnificationOutcome,
) -> Result<(), String> {
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return Ok(());
    }
    let templates_dir = PathBuf::from(project_path)
        .join(".k2so")
        .join("agent-templates");

    let Ok(rd) = fs::read_dir(&agents_root) else { return Ok(()) };
    let entries: Vec<_> = rd.flatten().collect();
    for entry in entries {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "__lead__" || name == ".archive" {
            continue;
        }
        if Some(name.as_str()) == primary {
            continue;
        }

        let src = agents_root.join(&name);
        let dst = templates_dir.join(&name);
        fs::create_dir_all(&dst).map_err(|e| format!("mkdir template {name}: {e}"))?;

        // Move AGENT.md (the only file templates need to keep).
        let agent_md = src.join("AGENT.md");
        if agent_md.exists() {
            let to = dst.join("AGENT.md");
            move_path(&agent_md, &to)
                .map_err(|e| format!("move template {name}/AGENT.md: {e}"))?;
        }

        // Templates have no inbox — drop work/, CLAUDE.md, SKILL.md.
        // 0.37.6: trash, not permanent. Same reasoning as the
        // primary-agent migration above.
        //
        // SAFETY: routes through `scratch_safe_trash` so test scratch
        // paths under temp_dir() skip the trash crate (avoids macOS
        // Touch ID prompts during cargo test).
        let _ = crate::safe_delete_scratch::scratch_safe_trash(src.join("work"));
        let _ = crate::safe_delete_scratch::scratch_safe_trash(src.join("CLAUDE.md"));
        let _ = crate::safe_delete_scratch::scratch_safe_trash(src.join("SKILL.md"));

        // Drop heartbeats/ on templates too — they don't run.
        let _ = crate::safe_delete_scratch::scratch_safe_trash(src.join("heartbeats"));

        // Anything else under the template dir that we don't
        // recognize → archive to legacy under the template name.
        let leftover = collect_leftover_entries(&src);
        if !leftover.is_empty() {
            let legacy_target = PathBuf::from(project_path)
                .join(".k2so")
                .join("migration")
                .join("legacy")
                .join(format!("template-{name}-residue"));
            for f in &leftover {
                let from = src.join(f);
                let to = legacy_target.join(f);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).ok();
                }
                if let Err(e) = move_path(&from, &to) {
                    return Err(format!("archive template residue {f}: {e}"));
                }
            }
            outcome
                .legacy_archived
                .push(format!("template-{name}-residue"));
        }

        let _ = fs::remove_dir(&src);
        outcome.templates_migrated.push(name);
    }

    Ok(())
}

// ── Special legacy dirs ────────────────────────────────────────────

fn archive_special_dirs(
    project_path: &str,
    outcome: &mut UnificationOutcome,
) -> Result<(), String> {
    let agents_root = agents_dir(project_path);
    let legacy_dir = PathBuf::from(project_path)
        .join(".k2so")
        .join("migration")
        .join("legacy");

    let lead_src = agents_root.join("__lead__");
    if lead_src.exists() {
        let dest = legacy_dir.join("__lead__");
        move_dir(&lead_src, &dest).map_err(|e| format!("archive __lead__: {e}"))?;
        outcome.legacy_archived.push("__lead__".to_string());
    }

    let archive_src = agents_root.join(".archive");
    if archive_src.exists() {
        let dest = legacy_dir.join("agent-archive");
        move_dir(&archive_src, &dest).map_err(|e| format!("archive .archive: {e}"))?;
        outcome.legacy_archived.push("agent-archive".to_string());
    }
    Ok(())
}

fn archive_root_cruft(
    project_path: &str,
    outcome: &mut UnificationOutcome,
) -> Result<(), String> {
    let dot_k2so = PathBuf::from(project_path).join(".k2so");
    let legacy_dir = dot_k2so.join("migration").join("legacy");

    for f in ["wakeup.md", "MIGRATION-0.32.7.md", ".harvest-0.32.7-done"] {
        let src = dot_k2so.join(f);
        if !src.exists() {
            continue;
        }
        let dest = legacy_dir.join(f);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).ok();
        }
        move_path(&src, &dest).map_err(|e| format!("archive root cruft {f}: {e}"))?;
        outcome.legacy_archived.push(f.to_string());
    }

    Ok(())
}

// ── Sentinel + notice ──────────────────────────────────────────────

fn write_sentinel(project_path: &str, outcome: &UnificationOutcome) -> Result<(), String> {
    let path = sentinel_path(project_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create sentinel parent: {e}"))?;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!(
        "version: 0.37.0\nstamped_at: {ts}\nprimary: {primary}\ntemplates: {tcount}\nlegacy_archived: {acount}\nwork_items_merged: {merged}\nconflicts: {ccount}\n",
        primary = outcome.primary_migrated.as_deref().unwrap_or("(none)"),
        tcount = outcome.templates_migrated.len(),
        acount = outcome.legacy_archived.len(),
        merged = outcome.work_items_merged,
        ccount = outcome.conflicts.len(),
    );
    atomic_write_str(&path, &body).map_err(|e| format!("write sentinel: {e}"))
}

fn write_migration_notice(
    project_path: &str,
    outcome: &UnificationOutcome,
) -> Result<(), String> {
    let path = PathBuf::from(project_path)
        .join(".k2so")
        .join(MIGRATION_NOTICE_FILENAME);
    let mut body = String::new();
    body.push_str("# Workspace–Agent Unification (0.37.0)\n\n");
    body.push_str(
        "K2SO 0.37.0 collapsed your workspace's `.k2so/agents/<name>/...` layout \
         into the single-agent layout. Originals were preserved under \
         `.k2so/migration/legacy/`.\n\n",
    );
    body.push_str("## What moved\n\n");
    if let Some(p) = &outcome.primary_migrated {
        body.push_str(&format!(
            "- **Primary agent (`{p}`)** → `.k2so/agent/AGENT.md`\n"
        ));
    } else {
        body.push_str(
            "- No primary agent migrated (workspace `agent_mode` is `off`, or none on disk).\n",
        );
    }
    if !outcome.templates_migrated.is_empty() {
        body.push_str(&format!(
            "- **Templates** ({}) → `.k2so/agent-templates/<name>/AGENT.md`\n  - {}\n",
            outcome.templates_migrated.len(),
            outcome.templates_migrated.join("\n  - ")
        ));
    }
    if outcome.work_items_merged > 0 {
        body.push_str(&format!(
            "- **Work items merged** into `.k2so/work/`: {}\n",
            outcome.work_items_merged
        ));
    }
    if !outcome.legacy_archived.is_empty() {
        body.push_str(&format!(
            "- **Archived to `.k2so/migration/legacy/`**:\n  - {}\n",
            outcome.legacy_archived.join("\n  - ")
        ));
    }
    if !outcome.conflicts.is_empty() {
        body.push_str(&format!(
            "\n## Conflicts (manual review)\n\n\
             {} work item(s) had a name collision with an existing workspace \
             inbox file. Agent-side copies are at \
             `.k2so/migration/legacy/work-conflicts/`:\n  - {}\n",
            outcome.conflicts.len(),
            outcome.conflicts.join("\n  - ")
        ));
    }
    body.push_str(
        "\n## Recovery\n\n\
         Anything in `.k2so/migration/legacy/` is safe to delete after you've \
         verified your workspace works. Compiled SKILL/CLAUDE files were \
         dropped (they regenerate from the AGENT.md source on next launch).\n\n\
         If something looks wrong, the sentinel `.k2so/.unification-0.37.0-done` \
         can be deleted to force a re-run — but only if you've also restored \
         the original `.k2so/agents/` layout from the legacy archive.\n",
    );
    atomic_write_str(&path, &body).map_err(|e| format!("write migration notice: {e}"))
}

// ── Resolver (no DB; takes agent_mode as parameter) ────────────────

fn resolve_primary_for_migration(project_path: &str, agent_mode: &str) -> Option<String> {
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return None;
    }
    let wanted = match agent_mode {
        "custom" => "custom",
        "manager" => "manager",
        "k2so" | "agent" => "k2so",
        _ => return None,
    };
    let rd = fs::read_dir(&agents_root).ok()?;
    for entry in rd.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "__lead__" || name == ".archive" {
            continue;
        }
        if agent_type_for(project_path, &name) == wanted {
            return Some(name);
        }
    }
    None
}

// ── Filesystem move helpers ────────────────────────────────────────

/// Move a directory tree; falls back to copy+delete when rename
/// fails (cross-device, e.g. when `.k2so/migration/legacy/` is on
/// a different mount).
fn move_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir_all(src, dst)?;
    fs::remove_dir_all(src)
}

fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let meta = fs::metadata(src)?;
    if meta.is_dir() {
        copy_dir_all(src, dst)?;
        fs::remove_dir_all(src)
    } else {
        fs::copy(src, dst)?;
        fs::remove_file(src)
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_symlink() {
            let target = fs::read_link(&from)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &to)?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_file(target, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2so-unification-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(p.join(".k2so/agents")).unwrap();
        p
    }

    fn write_agent(p: &Path, name: &str, agent_type: &str) {
        let dir = p.join(".k2so/agents").join(name);
        fs::create_dir_all(&dir).unwrap();
        let body = format!("---\nname: {name}\ntype: {agent_type}\n---\n# {name}\n");
        fs::write(dir.join("AGENT.md"), body).unwrap();
        // Compiled outputs every agent ships today.
        fs::write(dir.join("CLAUDE.md"), "# claude compiled\n").unwrap();
        fs::write(dir.join("SKILL.md"), "# skill compiled\n").unwrap();
    }

    #[test]
    fn unification_migrates_primary_agent() {
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "pod-leader", "manager");
        write_agent(&p, "rust-eng", "agent-template");
        fs::create_dir_all(p.join(".k2so/agents/__lead__")).unwrap();

        let outcome = run_unification(&path, "manager").unwrap();
        assert_eq!(outcome.primary_migrated.as_deref(), Some("pod-leader"));
        assert!(outcome.templates_migrated.contains(&"rust-eng".to_string()));
        assert!(outcome.legacy_archived.contains(&"__lead__".to_string()));

        // Post-state: new layout exists.
        assert!(p.join(".k2so/agent/AGENT.md").exists());
        assert!(p.join(".k2so/agent-templates/rust-eng/AGENT.md").exists());
        assert!(p.join(".k2so/migration/legacy/__lead__").exists());
        // Compiled files deleted.
        assert!(!p.join(".k2so/agent/CLAUDE.md").exists());
        assert!(!p.join(".k2so/agent/SKILL.md").exists());
        // agents/ dir removed entirely.
        assert!(!p.join(".k2so/agents").exists());
        // Sentinel + notice landed.
        assert!(p.join(".k2so/.unification-0.37.0-done").exists());
        assert!(p.join(".k2so/MIGRATION-0.37.0.md").exists());

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_merges_per_agent_inbox_into_workspace() {
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "pod-leader", "manager");
        let inbox = p.join(".k2so/agents/pod-leader/work/inbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("foo.md"), "task body\n").unwrap();
        fs::write(inbox.join("bar.md"), "another\n").unwrap();

        let outcome = run_unification(&path, "manager").unwrap();
        assert_eq!(outcome.work_items_merged, 2);
        assert!(p.join(".k2so/work/inbox/foo.md").exists());
        assert!(p.join(".k2so/work/inbox/bar.md").exists());
        // Source inbox is gone (post-migration the dir tree under the primary is fully drained).
        assert!(!p.join(".k2so/agents/pod-leader").exists());

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_deletes_template_work_dirs() {
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "pod-leader", "manager");
        write_agent(&p, "rust-eng", "agent-template");
        let inbox = p.join(".k2so/agents/rust-eng/work/inbox");
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("template-task.md"), "should be dropped\n").unwrap();

        run_unification(&path, "manager").unwrap();

        // Template's AGENT.md kept; work tree gone (templates have no inbox).
        assert!(p.join(".k2so/agent-templates/rust-eng/AGENT.md").exists());
        assert!(!p.join(".k2so/agent-templates/rust-eng/work").exists());
        // Workspace inbox does NOT contain the template's task.
        let ws_inbox = p.join(".k2so/work/inbox");
        if ws_inbox.exists() {
            let entries: Vec<_> = fs::read_dir(&ws_inbox)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            assert!(
                !entries.contains(&"template-task.md".to_string()),
                "template work item leaked into workspace inbox: {entries:?}"
            );
        }

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_idempotent() {
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "pod-leader", "manager");

        let first = run_unification(&path, "manager").unwrap();
        assert!(!first.already_done);
        assert_eq!(first.primary_migrated.as_deref(), Some("pod-leader"));

        let second = run_unification(&path, "manager").unwrap();
        assert!(second.already_done, "second run must be a no-op");
        assert!(second.primary_migrated.is_none());
        assert!(second.templates_migrated.is_empty());

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_off_mode_skips_primary_keeps_templates() {
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "rust-eng", "agent-template");
        write_agent(&p, "frontend-eng", "agent-template");

        let outcome = run_unification(&path, "off").unwrap();
        assert!(outcome.primary_migrated.is_none());
        assert_eq!(outcome.templates_migrated.len(), 2);

        // No .k2so/agent/AGENT.md (no primary).
        assert!(!p.join(".k2so/agent/AGENT.md").exists());
        // Templates landed.
        assert!(p.join(".k2so/agent-templates/rust-eng/AGENT.md").exists());
        assert!(p
            .join(".k2so/agent-templates/frontend-eng/AGENT.md")
            .exists());
        // agents/ dir cleaned up.
        assert!(!p.join(".k2so/agents").exists());

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_drift_declared_mode_no_primary_on_disk() {
        // Workspace declares manager mode but no manager-type agent
        // exists on disk (orphan from a mode swap). Migration should
        // still run cleanup + templates without erroring.
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "rust-eng", "agent-template");
        // No manager dir.

        let outcome = run_unification(&path, "manager").unwrap();
        assert!(outcome.primary_migrated.is_none());
        assert_eq!(outcome.templates_migrated.len(), 1);
        assert!(p.join(".k2so/.unification-0.37.0-done").exists());

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_k2so_shaped_workspace() {
        // Mirrors K2SO's own .k2so/ layout as of 0.36.15:
        // pod-leader (manager) + k2so-agent (k2so) + 4 templates +
        // __lead__ + .archive/ + root cruft. Single end-to-end smoke
        // assertion against the shape we'll actually meet in the
        // wild on first 0.37.0 boot in this very repo.
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "pod-leader", "manager");
        write_agent(&p, "k2so-agent", "k2so");
        for tpl in ["rust-eng", "frontend-eng", "qa-eng", "cli-eng"] {
            write_agent(&p, tpl, "agent-template");
        }
        // __lead__ has no AGENT.md in K2SO; just a directory.
        fs::create_dir_all(p.join(".k2so/agents/__lead__")).unwrap();
        fs::write(p.join(".k2so/agents/__lead__/CLAUDE.md"), "lead\n").unwrap();
        // .archive holds dated agent snapshots.
        fs::create_dir_all(p.join(".k2so/agents/.archive/k2so-agent-20260424-191719"))
            .unwrap();
        fs::write(
            p.join(".k2so/agents/.archive/k2so-agent-20260424-191719/AGENT.md"),
            "archived\n",
        )
        .unwrap();
        // Per-agent heartbeats for the primary.
        let hb_dir = p.join(".k2so/agents/pod-leader/heartbeats/morning");
        fs::create_dir_all(&hb_dir).unwrap();
        fs::write(hb_dir.join("wakeup.md"), "morning wake\n").unwrap();
        // Pre-seeded workspace inbox (from current .k2so/work/).
        fs::create_dir_all(p.join(".k2so/work/inbox")).unwrap();
        fs::write(p.join(".k2so/work/inbox/existing.md"), "pre-existing\n").unwrap();
        // Per-agent inbox with two items.
        let agent_inbox = p.join(".k2so/agents/pod-leader/work/inbox");
        fs::create_dir_all(&agent_inbox).unwrap();
        fs::write(agent_inbox.join("alpha.md"), "alpha\n").unwrap();
        fs::write(agent_inbox.join("beta.md"), "beta\n").unwrap();
        // Root cruft.
        fs::write(p.join(".k2so/wakeup.md"), "old root wakeup\n").unwrap();
        fs::write(p.join(".k2so/MIGRATION-0.32.7.md"), "aged-out\n").unwrap();

        let outcome = run_unification(&path, "manager").unwrap();

        // Primary
        assert_eq!(outcome.primary_migrated.as_deref(), Some("pod-leader"));
        assert!(p.join(".k2so/agent/AGENT.md").exists());

        // Templates: 4 templates + k2so-agent (since manager is primary,
        // k2so-agent gets demoted to a template even though it has a
        // first-class type — the workspace can only have one primary).
        assert_eq!(outcome.templates_migrated.len(), 5);
        for tpl in ["rust-eng", "frontend-eng", "qa-eng", "cli-eng", "k2so-agent"] {
            assert!(
                p.join(format!(".k2so/agent-templates/{tpl}/AGENT.md")).exists(),
                "missing template: {tpl}"
            );
        }

        // Legacy archives
        assert!(p.join(".k2so/migration/legacy/__lead__").exists());
        assert!(p
            .join(".k2so/migration/legacy/agent-archive/k2so-agent-20260424-191719/AGENT.md")
            .exists());
        assert!(p.join(".k2so/migration/legacy/wakeup.md").exists());
        assert!(p.join(".k2so/migration/legacy/MIGRATION-0.32.7.md").exists());

        // Heartbeats promoted to workspace level
        assert!(p.join(".k2so/heartbeats/morning/wakeup.md").exists());

        // Work merged: existing.md + alpha.md + beta.md all in workspace inbox
        assert_eq!(outcome.work_items_merged, 2); // 2 NEW items merged
        assert!(p.join(".k2so/work/inbox/existing.md").exists());
        assert!(p.join(".k2so/work/inbox/alpha.md").exists());
        assert!(p.join(".k2so/work/inbox/beta.md").exists());

        // agents/ removed entirely
        assert!(!p.join(".k2so/agents").exists());

        // Sentinel + notice
        assert!(p.join(".k2so/.unification-0.37.0-done").exists());
        let notice = fs::read_to_string(p.join(".k2so/MIGRATION-0.37.0.md")).unwrap();
        assert!(notice.contains("pod-leader"));
        assert!(notice.contains("rust-eng"));

        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn unification_archives_root_cruft() {
        let p = temp_workspace();
        let path = p.to_string_lossy().to_string();
        write_agent(&p, "pod-leader", "manager");
        fs::write(p.join(".k2so/wakeup.md"), "old\n").unwrap();
        fs::write(p.join(".k2so/MIGRATION-0.32.7.md"), "old\n").unwrap();
        fs::write(p.join(".k2so/.harvest-0.32.7-done"), "").unwrap();

        run_unification(&path, "manager").unwrap();

        assert!(!p.join(".k2so/wakeup.md").exists());
        assert!(!p.join(".k2so/MIGRATION-0.32.7.md").exists());
        assert!(!p.join(".k2so/.harvest-0.32.7-done").exists());
        assert!(p.join(".k2so/migration/legacy/wakeup.md").exists());
        assert!(p.join(".k2so/migration/legacy/MIGRATION-0.32.7.md").exists());
        assert!(p.join(".k2so/migration/legacy/.harvest-0.32.7-done").exists());

        fs::remove_dir_all(&p).ok();
    }
}
