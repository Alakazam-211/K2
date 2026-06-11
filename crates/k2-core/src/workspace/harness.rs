//! Workspace harness file-discovery cluster.
//!
//! Phase 2.5d: extracted from the monolithic `agents/workspace.rs`. This
//! module owns the workspace-root harness fan-out — the symlinks and
//! scaffold files K2SO writes so every CLI LLM (Claude Code, Gemini,
//! Goose, Cursor, Aider, etc.) auto-discovers the canonical SKILL.md
//! body without per-tool reconfiguration.
//!
//! Public surface:
//!   - [`HARNESS_WORKSPACE_FILES`] — workspace-root targets list
//!   - [`WorkspacePreviewEntry`] — preview row the UI consumes
//!   - [`k2so_agents_preview_workspace_ingest`] — preview a workspace
//!     add without mutating
//!   - [`k2so_agents_run_workspace_ingest`] — fire the harvest + write
//!     pipeline for a workspace
//!   - [`disable_workspace_claude_md`] — flip the root CLAUDE.md off
//!
//! Sibling [`crate::workspace::skill_regen`] owns the canonical
//! SKILL.md regen, [`crate::workspace::teardown`] owns the disconnect
//! flow, and [`crate::workspace::migrations`] hosts the archive
//! utilities all three call.


use std::fs;
use std::path::{Path, PathBuf};

// Canonical-agents safety net + detection live in the sibling
// `canonical` module; re-export under `harness::` so the PRD-named
// paths (`harness::run_canonical_setup` / `unwind_canonical` /
// `persist_agent_md` / `detect_canonical_state`) resolve.
pub use crate::workspace::canonical::{
    detect_canonical_state, persist_agent_md, run_canonical_setup, unwind_canonical,
    CanonicalSetupOpts, CanonicalSetupOutcome, HarnessFileState, HarnessProbe, PersistOpts,
    SetupManifest, UnifiedForm,
};

use crate::skills::writer::force_symlink;
use crate::workspace::wake_prompts::strip_frontmatter;
use crate::fs_atomic::{atomic_write_str, log_if_err};
use crate::workspace::migrations::{
    archive_claude_md_file, harvest_per_agent_claude_md_files,
    inject_first_migration_banner,
};
use crate::workspace::skill_regen::{import_claude_md_into_user_notes, write_workspace_skill_file};

// ══════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════

/// The six workspace-root files K2SO can take over via symlink / scaffold.
/// On teardown we walk this list and either freeze the current SKILL.md
/// body into each as a real file (keep_current mode), or restore the
/// archive from `.k2so/migration/` (restore_original mode).
pub const HARNESS_WORKSPACE_FILES: &[&str] = &[
    "CLAUDE.md",
    "GEMINI.md",
    "AGENT.md",
    ".goosehints",
    "SKILL.md",
    ".cursor/rules/k2so.mdc",
    // NOT .aider.conf.yml — that's a config file with merged entries,
    // handled separately below.
];

// ══════════════════════════════════════════════════════════════════════
// Public types
// ══════════════════════════════════════════════════════════════════════

/// One entry in the Add-Workspace preview. Mirrors what the CLI's
/// `k2so workspace preview` reports, but structured for the UI.
#[derive(serde::Serialize, Debug)]
pub struct WorkspacePreviewEntry {
    pub path: String,
    pub action: String, // "archive_and_import" | "refresh" | "create" | "marker_injected"
    pub size_bytes: Option<u64>,
    pub note: String,
}

// ══════════════════════════════════════════════════════════════════════
// Extended harness file-discovery coverage
// ══════════════════════════════════════════════════════════════════════

/// Create a symlink for a workspace-root harness file with the contract:
///   1. Archive the original to `.k2so/migration/` (never destroy).
///   2. Import its body into SKILL.md's USER_NOTES so the new symlinked
///      SKILL.md still surfaces the user's accumulated context.
///   3. Replace the target with the symlink.
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests can exercise
/// the safe-link contract through Tier A.
pub(crate) fn safe_symlink_harness_file(
    canonical: &Path,
    target: &Path,
    project_path: &str,
    harness_display: &str,
) {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            force_symlink(canonical, target);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(target).unwrap_or_default();
            let filename = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(harness_display)
                .to_string();
            let archived = archive_claude_md_file(project_path, target, &filename);
            if !content.trim().is_empty() {
                let archive_display = archived
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(archive unavailable)".to_string());
                import_claude_md_into_user_notes(
                    project_path,
                    &content,
                    &format!("pre-existing {}", harness_display),
                    &archive_display,
                );
            }
            force_symlink(canonical, target);
            if let Some(p) = archived {
                inject_first_migration_banner(project_path, &[p]);
            }
        }
        _ => {
            force_symlink(canonical, target);
        }
    }
}

/// Workspace-level harness file-discovery targets.
///
/// Phase 2.5d: `pub(crate)` so the sibling
/// [`crate::workspace::skill_regen`] can invoke it from
/// `write_workspace_skill_file_with_body` while harness cluster still

/// lives in this file (moves out in Tier A.3).
pub(crate) fn write_workspace_harness_discovery_targets(project_path: &str, canonical: &Path) {
    let root = PathBuf::from(project_path);

    safe_symlink_harness_file(canonical, &root.join("GEMINI.md"), project_path, "GEMINI.md");
    safe_symlink_harness_file(canonical, &root.join("AGENT.md"), project_path, "AGENT.md");
    safe_symlink_harness_file(canonical, &root.join(".goosehints"), project_path, ".goosehints");

    write_cursor_rules_mdc(project_path, canonical);
    scaffold_aider_conf(project_path);
}

/// Generate `./.cursor/rules/k2so.mdc` with MDC frontmatter + the

/// canonical SKILL.md body.
fn write_cursor_rules_mdc(project_path: &str, canonical: &Path) {
    let Ok(raw) = fs::read_to_string(canonical) else { return };
    let body = strip_frontmatter(&raw).trim().to_string();
    if body.is_empty() {
        return;
    }

    let dir = PathBuf::from(project_path).join(".cursor").join("rules");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let target = dir.join("k2so.mdc");

    const K2SO_MDC_SIGNATURE: &str = "k2so_generated: true";

    if target.exists() {
        if let Ok(existing) = fs::read_to_string(&target) {
            let is_our_output = existing.contains(K2SO_MDC_SIGNATURE);
            if !is_our_output {
                let existing_body = strip_frontmatter(&existing).trim().to_string();
                if !existing_body.is_empty() {
                    let archived = archive_claude_md_file(
                        project_path,
                        &target,
                        "cursor/rules/k2so.mdc",
                    );
                    let archive_display = archived
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(archive unavailable)".to_string());
                    import_claude_md_into_user_notes(
                        project_path,
                        &existing_body,
                        "pre-existing .cursor/rules/k2so.mdc",
                        &archive_display,
                    );
                }
            }
        }
    }

    let mdc = format!(
        "---\n{signature}\ndescription: K2SO workspace context — CLI reference + project context + primary agent persona\nalwaysApply: true\n---\n\n{body}\n",
        signature = K2SO_MDC_SIGNATURE,
        body = body,
    );
    log_if_err(
        "write_cursor_rules_mdc",
        &target,
        atomic_write_str(&target, &mdc),
    );
}

/// Scaffold `./.aider.conf.yml` with `read: [SKILL.md]` so Aider pulls
/// the workspace context on every session.
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests can verify
/// the aider-conf merge behavior through Tier A.
pub(crate) fn scaffold_aider_conf(project_path: &str) {
    let path = PathBuf::from(project_path).join(".aider.conf.yml");
    if !path.exists() {
        log_if_err(
            "scaffold_aider_conf create",
            &path,
            atomic_write_str(
                &path,
                "# K2SO: ship workspace context to Aider on every session.\nread:\n  - SKILL.md\n",
            ),
        );
        return;
    }
    let Ok(existing) = fs::read_to_string(&path) else { return };
    if existing.contains("SKILL.md") {
        return;
    }

    let _ = archive_claude_md_file(project_path, &path, ".aider.conf.yml");

    let lines: Vec<&str> = existing.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);
    let mut injected = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if !injected && (trimmed == "read:" || trimmed.starts_with("read:")) {
            out.push(line.to_string());
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            out.push(format!("{}  - SKILL.md", indent));
            out.push(format!("{}  # ^ added by K2SO — workspace context", indent));
            injected = true;
            i += 1;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    if !injected {
        if !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("# K2SO: ship workspace context on every session.".to_string());
        out.push("read:".to_string());
        out.push("  - SKILL.md".to_string());
    }
    let mut final_out = out.join("\n");
    if !final_out.ends_with('\n') {
        final_out.push('\n');
    }
    log_if_err(
        "scaffold_aider_conf merge",
        &path,
        atomic_write_str(&path, &final_out),
    );
}


// ══════════════════════════════════════════════════════════════════════
// Public entry points
// ══════════════════════════════════════════════════════════════════════


/// Inspect a workspace path WITHOUT mutating anything. Returns a list
/// of entries describing what K2SO will do on add.
pub fn k2so_agents_preview_workspace_ingest(
    project_path: String,
) -> Result<Vec<WorkspacePreviewEntry>, String> {
    let root = PathBuf::from(&project_path);
    let mut entries: Vec<WorkspacePreviewEntry> = Vec::new();

    let collision_targets: &[(&str, &str)] = &[
        ("CLAUDE.md", "Claude Code memory"),
        ("GEMINI.md", "Gemini CLI instructions"),
        ("AGENT.md", "agent.md spec file"),
        (".goosehints", "Goose hints"),
        (".cursor/rules/k2so.mdc", "Cursor rule"),
    ];
    for (rel, label) in collision_targets {
        let path = root.join(rel);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                entries.push(WorkspacePreviewEntry {
                    path: rel.to_string(),
                    action: "refresh".to_string(),
                    size_bytes: None,
                    note: format!("{} — already symlinked to K2SO canonical (will refresh)", label),
                });
            }
            Ok(meta) if meta.file_type().is_file() => {
                let is_ours = fs::read_to_string(&path)
                    .map(|s| s.contains("k2so_generated: true"))
                    .unwrap_or(false);
                if is_ours {
                    entries.push(WorkspacePreviewEntry {
                        path: rel.to_string(),
                        action: "refresh".to_string(),
                        size_bytes: Some(meta.len()),
                        note: format!("{} — K2SO-generated, will refresh in place", label),
                    });
                } else {
                    entries.push(WorkspacePreviewEntry {
                        path: rel.to_string(),
                        action: "archive_and_import".to_string(),
                        size_bytes: Some(meta.len()),
                        note: format!("{} — archive → import body into SKILL.md USER_NOTES → symlink", label),
                    });
                }
            }
            _ => {
                entries.push(WorkspacePreviewEntry {
                    path: rel.to_string(),
                    action: "create".to_string(),
                    size_bytes: None,
                    note: format!("{} — no prior file, will create symlink", label),
                });
            }
        }
    }

    let aider_path = root.join(".aider.conf.yml");
    if aider_path.is_file() {
        let already = fs::read_to_string(&aider_path)
            .map(|s| s.contains("SKILL.md"))
            .unwrap_or(false);
        let size = fs::metadata(&aider_path).ok().map(|m| m.len());
        if already {
            entries.push(WorkspacePreviewEntry {
                path: ".aider.conf.yml".to_string(),
                action: "refresh".to_string(),
                size_bytes: size,
                note: "Aider config — already references SKILL.md, no change".to_string(),
            });
        } else {
            entries.push(WorkspacePreviewEntry {
                path: ".aider.conf.yml".to_string(),
                action: "archive_and_import".to_string(),
                size_bytes: size,
                note: "Aider config — archive → merge SKILL.md into read: list (preserves other keys)".to_string(),
            });
        }
    } else {
        entries.push(WorkspacePreviewEntry {
            path: ".aider.conf.yml".to_string(),
            action: "create".to_string(),
            size_bytes: None,
            note: "Aider config — scaffold fresh with read: [SKILL.md]".to_string(),
        });
    }

    let marker_targets: &[(&str, &str)] = &[
        ("AGENTS.md", "Codex / OpenCode / Pi"),
        (".github/copilot-instructions.md", "GitHub Copilot"),
    ];
    for (rel, label) in marker_targets {
        let path = root.join(rel);
        let size = fs::metadata(&path).ok().map(|m| m.len());
        let action = if path.exists() { "marker_injected" } else { "create" };
        let note = if path.exists() {
            format!(
                "{} — K2SO block inserted between markers, your content preserved",
                label
            )
        } else {
            format!("{} — will create with K2SO block only", label)
        };
        entries.push(WorkspacePreviewEntry {
            path: rel.to_string(),
            action: action.to_string(),
            size_bytes: size,
            note,
        });
    }

    Ok(entries)
}

/// Trigger the workspace skill write for a single project on demand.

pub fn k2so_agents_run_workspace_ingest(project_path: String) -> Result<(), String> {
    harvest_per_agent_claude_md_files(&project_path);
    write_workspace_skill_file(&project_path);
    Ok(())
}




/// Remove or disable the workspace SKILL.md + CLAUDE.md symlink
/// (when the Agent toggle is turned off).
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_disable_workspace_claude_md`.
pub fn disable_workspace_claude_md(project_path: String) -> Result<(), String> {
    let claude_md = PathBuf::from(&project_path).join("CLAUDE.md");
    let disabled = crate::workspace_dot_dir(&project_path).join("CLAUDE.md.disabled");

    if claude_md.exists() {
        // Move to .k2so/ rather than delete — preserves any user edits
        fs::rename(&claude_md, &disabled)
            .map_err(|e| format!("Failed to disable CLAUDE.md: {}", e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for the workspace harness preview +
    //! disable surface. The ingest function (`run_workspace_ingest`)
    //! triggers the full skill-writer fanout which is exercised by
    //! `workspace/migrations.rs::migration_safety_tests`; this module
    //! covers the read-only preview shape + the disable lifecycle.
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn scratch_project() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-harness-test-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        fs::create_dir_all(dir.join(".k2so")).unwrap();
        dir
    }

    fn entries_by_path(entries: &[WorkspacePreviewEntry]) -> std::collections::HashMap<String, &WorkspacePreviewEntry> {
        entries.iter().map(|e| (e.path.clone(), e)).collect()
    }

    #[test]
    fn preview_workspace_ingest_reports_create_for_clean_workspace() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        let entries = k2so_agents_preview_workspace_ingest(path).expect("preview ok");

        // Every collision target + marker file should be reported.
        let by_path = entries_by_path(&entries);
        for rel in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints", ".cursor/rules/k2so.mdc"] {
            let e = by_path.get(rel).unwrap_or_else(|| {
                panic!("preview should include '{rel}', got {entries:?}")
            });
            assert_eq!(
                e.action, "create",
                "clean workspace should report action=create for {rel}, got {:?}",
                e.action
            );
            assert!(e.size_bytes.is_none(), "no pre-existing file → no size");
        }
        // .aider.conf.yml is reported as create for a fresh workspace.
        let aider = by_path
            .get(".aider.conf.yml")
            .expect("aider entry present");
        assert_eq!(aider.action, "create");
        // AGENTS.md + copilot-instructions are marker-injected when
        // present, "create" when absent (we created none, so create).
        for rel in ["AGENTS.md", ".github/copilot-instructions.md"] {
            let e = by_path.get(rel).unwrap_or_else(|| {
                panic!("preview should include {rel}, got entries: {entries:?}")
            });
            assert_eq!(e.action, "create");
        }

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn preview_workspace_ingest_reports_archive_and_import_for_user_authored_file() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        // Seed a user-authored CLAUDE.md (no `k2so_generated: true` signature).
        fs::write(
            proj.join("CLAUDE.md"),
            "# my own claude memory\n\nDo X then Y.\n",
        )
        .unwrap();

        let entries = k2so_agents_preview_workspace_ingest(path).expect("preview ok");
        let by_path = entries_by_path(&entries);

        let claude = by_path
            .get("CLAUDE.md")
            .expect("CLAUDE.md entry should be present");
        assert_eq!(
            claude.action, "archive_and_import",
            "user-authored CLAUDE.md should be archived + imported, got {:?}",
            claude.action
        );
        assert!(
            claude.size_bytes.is_some(),
            "existing-file branch must report size_bytes"
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn preview_workspace_ingest_reports_refresh_for_k2so_generated_cursor_mdc() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        // Seed a K2SO-authored cursor mdc — the signature `k2so_generated: true`
        // is the discriminator vs user-authored.
        let cursor_dir = proj.join(".cursor").join("rules");
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(
            cursor_dir.join("k2so.mdc"),
            "---\nk2so_generated: true\n---\n\n# managed\n",
        )
        .unwrap();

        let entries = k2so_agents_preview_workspace_ingest(path).expect("preview");
        let by_path = entries_by_path(&entries);

        let mdc = by_path
            .get(".cursor/rules/k2so.mdc")
            .expect("cursor mdc entry present");
        assert_eq!(
            mdc.action, "refresh",
            "k2so-generated file should be refreshed in place, got {:?}",
            mdc.action,
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn preview_workspace_ingest_reports_marker_injected_for_existing_agents_md() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();
        fs::write(proj.join("AGENTS.md"), "user-authored agents.md content").unwrap();

        let entries = k2so_agents_preview_workspace_ingest(path).expect("preview");
        let by_path = entries_by_path(&entries);

        let agents = by_path.get("AGENTS.md").expect("AGENTS.md entry");
        assert_eq!(
            agents.action, "marker_injected",
            "existing AGENTS.md should be treated as marker-injection target, got {:?}",
            agents.action,
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn disable_workspace_claude_md_moves_existing_file_to_disabled_path() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        let claude = proj.join("CLAUDE.md");
        fs::write(&claude, "my body").unwrap();
        let disabled = proj.join(".k2so").join("CLAUDE.md.disabled");
        assert!(!disabled.exists(), "sanity: disabled file should not pre-exist");

        disable_workspace_claude_md(path).expect("disable ok");

        assert!(!claude.exists(), "root CLAUDE.md must be moved away");
        assert!(disabled.exists(), "disabled path must now hold the body");
        let preserved = fs::read_to_string(&disabled).unwrap();
        assert_eq!(preserved, "my body", "content must be preserved byte-for-byte");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn disable_workspace_claude_md_noops_when_root_file_absent() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();
        // No CLAUDE.md at the workspace root.
        assert!(!proj.join("CLAUDE.md").exists());

        // Should succeed silently, not error.
        disable_workspace_claude_md(path).expect("no-op disable should not error");

        // Disabled path also should not be created.
        assert!(!proj.join(".k2so").join("CLAUDE.md.disabled").exists());

        fs::remove_dir_all(&proj).ok();
    }
}
