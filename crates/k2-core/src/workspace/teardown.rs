//! Workspace teardown (disconnect) cluster.
//!
//! Phase 2.5d: extracted from the monolithic `agents/workspace.rs`. This
//! module owns the "freeze or restore" disconnect flow that runs when
//! the user removes a workspace from K2SO. Two modes:
//!
//! - `KeepCurrent` — freeze the current canonical SKILL.md body into
//!   each symlinked file as a real file. Every CLI LLM the user had
//!   enabled keeps working, reading the last-known merged context.
//! - `RestoreOriginal` — replace each symlinked file with whatever was
//!   there before K2SO took over (from `.k2so/migration/`). Files K2SO
//!   created fresh (no archive) are sent to the recycle bin.
//!
//! In both modes `.k2so/` is preserved. Nothing is destroyed; the
//! restore is a one-way reconnect-safe.
//!
//! Sibling [`crate::workspace::harness`] owns the symlink scaffolding,
//! [`crate::workspace::skill_regen`] owns the canonical SKILL.md
//! regen, and [`crate::workspace::migrations`] hosts the archive
//! utilities all three call.


use std::fs;
use std::path::PathBuf;

use crate::fs_atomic::{atomic_write_str, log_if_err};
use crate::workspace::harness::HARNESS_WORKSPACE_FILES;

// ══════════════════════════════════════════════════════════════════════
// Public types
// ══════════════════════════════════════════════════════════════════════


/// Summary returned to the UI after a teardown. One entry per file
/// touched, with a human-readable note so the dialog can show what
/// happened.
#[derive(serde::Serialize, Debug)]
pub struct TeardownResult {
    pub action: String,
    pub path: String,
    pub note: String,
}


/// User's choice when they remove/disconnect a workspace.
///
/// - `keep_current`: freeze the current canonical SKILL.md body into each
///   symlinked file as a real file. Every CLI LLM the user had enabled
///   keeps working, reading the last-known merged context. Best when
///   the user is stepping away but wants their tools to still have context.
///
/// - `restore_original`: replace each symlinked file with whatever was
///   there before K2SO took over (from `.k2so/migration/`). Files K2SO
///   created fresh (no archive) are deleted. The workspace looks like
///   it did pre-K2SO except for the `.k2so/` folder, which stays
///   intact as the restore source (and the reconnect-later safety net).
///
/// In both modes `.k2so/` itself is preserved. Nothing is destroyed.
#[derive(Clone, Copy, Debug)]
pub enum TeardownMode {
    KeepCurrent,
    RestoreOriginal,
}


// ══════════════════════════════════════════════════════════════════════
// Workspace teardown (disconnect)
// ══════════════════════════════════════════════════════════════════════

// ══════════════════════════════════════════════════════════════════════
// Workspace teardown (disconnect)
// ══════════════════════════════════════════════════════════════════════

/// Freeze or restore every workspace-root symlink, returning a per-file
/// summary the UI can display.
pub fn teardown_workspace_harness_files(
    project_path: &str,
    mode: TeardownMode,
) -> Vec<TeardownResult> {
    let root = PathBuf::from(project_path);
    let canonical = root.join(".k2so/skills/k2so/SKILL.md");
    let current_body = fs::read_to_string(&canonical).unwrap_or_default();
    let mut results: Vec<TeardownResult> = Vec::new();

    for rel in HARNESS_WORKSPACE_FILES {
        let path = root.join(rel);
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if !meta.file_type().is_symlink() {
            continue;
        }

        match mode {
            TeardownMode::KeepCurrent => match atomic_write_str(&path, &current_body) {
                Ok(()) => results.push(TeardownResult {
                    action: "froze".to_string(),
                    path: rel.to_string(),
                    note: "Replaced symlink with a frozen snapshot of the current SKILL.md. Tool will keep reading this context.".to_string(),
                }),
                Err(e) => results.push(TeardownResult {
                    action: "failed".to_string(),
                    path: rel.to_string(),
                    note: format!(
                        "Could not write frozen snapshot ({}); original symlink left intact.",
                        e
                    ),
                }),
            },
            TeardownMode::RestoreOriginal => match find_latest_archive(project_path, rel) {
                Some(archive_path) => match fs::read_to_string(&archive_path) {
                    Ok(body) => match atomic_write_str(&path, &body) {
                        Ok(()) => results.push(TeardownResult {
                            action: "restored".to_string(),
                            path: rel.to_string(),
                            note: format!("Restored from archive: {}", archive_path.display()),
                        }),
                        Err(e) => results.push(TeardownResult {
                            action: "failed".to_string(),
                            path: rel.to_string(),
                            note: format!(
                                "Found archive {} but write failed: {}; symlink left intact.",
                                archive_path.display(),
                                e
                            ),
                        }),
                    },
                    Err(e) => results.push(TeardownResult {
                        action: "failed".to_string(),
                        path: rel.to_string(),
                        note: format!(
                            "Archive unreadable ({}): {}; symlink left intact.",
                            archive_path.display(),
                            e
                        ),
                    }),
                },
                None => {
                    log_if_err(
                        "restore_original trash symlink",
                        &path,
                        crate::safe_delete::trash_or_remove(&path),
                    );
                    results.push(TeardownResult {
                        action: "removed".to_string(),
                        path: rel.to_string(),
                        note: "No prior archive — K2SO created this file fresh; sent to Trash.".to_string(),
                    });
                }
            },
        }
    }

    if matches!(mode, TeardownMode::RestoreOriginal) {
        let aider_path = root.join(".aider.conf.yml");
        if let Some(archive) = find_latest_archive(project_path, ".aider.conf.yml") {
            match fs::read_to_string(&archive) {
                Ok(body) => match atomic_write_str(&aider_path, &body) {
                    Ok(()) => results.push(TeardownResult {
                        action: "restored".to_string(),
                        path: ".aider.conf.yml".to_string(),
                        note: format!("Restored from archive: {}", archive.display()),
                    }),
                    Err(e) => results.push(TeardownResult {
                        action: "failed".to_string(),
                        path: ".aider.conf.yml".to_string(),
                        note: format!(
                            "Archive {} read ok but restore write failed: {}",
                            archive.display(),
                            e
                        ),
                    }),
                },
                Err(e) => results.push(TeardownResult {
                    action: "failed".to_string(),
                    path: ".aider.conf.yml".to_string(),
                    note: format!("Archive unreadable: {}", e),
                }),
            }
        } else if aider_path.exists() {
            log_if_err(
                "teardown trash aider.conf.yml",
                &aider_path,
                crate::safe_delete::trash_or_remove(&aider_path),
            );
            results.push(TeardownResult {
                action: "removed".to_string(),
                path: ".aider.conf.yml".to_string(),
                note: "No prior archive — K2SO scaffolded this file fresh; sent to Trash.".to_string(),
            });
        }
    }

    results
}

/// Walk `.k2so/migration/` looking for the most-recent archive that

/// matches the relative harness path.
fn find_latest_archive(project_path: &str, rel: &str) -> Option<PathBuf> {
    let migration_root = crate::workspace_dot_dir(project_path).join("migration");
    if !migration_root.is_dir() {
        return None;
    }

    let (subdir, leaf) = match rel.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent.to_string()), leaf.to_string()),
        None => (None, rel.to_string()),
    };
    let search_dir = match &subdir {
        Some(s) => migration_root.join(s),
        None => migration_root.clone(),
    };
    if !search_dir.is_dir() {
        return None;
    }

    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.clone(), String::new()),
    };

    let mut best: Option<(u128, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let prefix = format!("{}-", leaf_stem);
            if !name.starts_with(&prefix) {
                continue;
            }
            if !leaf_ext.is_empty() && !name.ends_with(&leaf_ext) {
                continue;
            }
            let rest = &name[prefix.len()..];
            let rest = if leaf_ext.is_empty() {
                rest
            } else {
                rest.trim_end_matches(&leaf_ext[..])
            };
            let ts_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let Ok(ts) = ts_str.parse::<u128>() else { continue };
            match &best {
                Some((existing_ts, _)) if ts <= *existing_ts => {}
                _ => best = Some((ts, path.clone())),
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Tauri-style entrypoint: parse mode string + run teardown. Returns

/// the per-file result list ready for JSON-encoding back to the UI.
pub fn k2so_agents_teardown_workspace(
    project_path: String,
    mode: String,
) -> Result<Vec<TeardownResult>, String> {
    let m = match mode.as_str() {
        "keep_current" => TeardownMode::KeepCurrent,
        "restore_original" => TeardownMode::RestoreOriginal,
        other => return Err(format!("unknown teardown mode: {}", other)),
    };
    Ok(teardown_workspace_harness_files(&project_path, m))
}
