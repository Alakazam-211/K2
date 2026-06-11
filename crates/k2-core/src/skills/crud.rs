//! Phase 2.5b follow-up: CRUD helpers over `.k2so/skills/<name>/`.
//!
//! The Tauri command surface (`src-tauri/src/commands/skills.rs`)
//! and the CLI verbs (`k2so skills list|create|profile|remove`) each
//! wrap these functions one-for-one. Underlying file shape post-2.5b:
//!
//! ```text
//! .k2so/skills/
//! ├── <name>/SKILL.md
//! └── <name>/SKILL.md
//! ```
//!
//! `SkillSummary` is the row shape both surfaces render. It's
//! intentionally narrow — name + title + mtime — so callers can list
//! cheaply without parsing the full body. Heavier fields (role,
//! inbox/active/done counts) live on `K2soAgentInfo` and remain on
//! the legacy `k2_core::agents::commands::list` surface for the
//! existing K2SO Agents panel.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::workspace::agent_identity::{parse_frontmatter, skill_dir, skills_dir};
use crate::safe_delete;

/// Compact row for the workspace-settings Skills list. Returned by
/// [`list`] and [`create`].
///
/// `title` is the first H1 (`# foo`) in `SKILL.md` if present, else
/// `None` — the UI falls back to `name` when title is absent so empty
/// or freshly-scaffolded skills still render usefully.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    /// Folder basename — also the skill id everywhere (CLI args, URLs,
    /// frontmatter `name:` field). Lowercase, alphanumeric + `-` / `_`.
    pub name: String,
    /// First `# heading` line in `SKILL.md`, with the `# ` prefix stripped
    /// and whitespace trimmed. `None` when the file has no H1.
    pub title: Option<String>,
    /// `SKILL.md` mtime as a Unix timestamp (seconds since epoch). 0 if
    /// the file is missing or the mtime is unreadable — both pathological;
    /// the UI should treat 0 as "unknown" rather than the literal 1970.
    pub last_modified: i64,
}

/// Enumerate every skill directory under `.k2so/skills/`. Alphabetical
/// by name. Hidden / dotfile dirs are skipped — `.archive` and friends
/// are housekeeping artifacts, not user-visible skills.
///
/// Missing `.k2so/skills/` returns `Ok(vec![])` rather than an error
/// because that's the steady state for pre-2.5b workspaces caught
/// before the daemon's boot-time consolidation has run. The CLI /
/// renderer can render an empty list with a "Create Skill" affordance
/// just fine.
pub fn list(project_path: &str) -> Result<Vec<SkillSummary>, String> {
    let dir = skills_dir(project_path);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        // Be tolerant of transitional shapes: a workspace mid-migration
        // may still have AGENT.md. Treat either as the profile file
        // so the UI lists every dir that has *something* in it.
        let profile_md = if skill_md.exists() {
            skill_md
        } else {
            let alt = entry.path().join("AGENT.md");
            if alt.exists() {
                alt
            } else {
                // Empty dir; surface it anyway so the user can see
                // (and clean up) zombie skills, but without a title.
                out.push(SkillSummary {
                    name,
                    title: None,
                    last_modified: 0,
                });
                continue;
            }
        };

        let title = first_h1(&profile_md);
        let last_modified = mtime_secs(&profile_md);
        out.push(SkillSummary {
            name,
            title,
            last_modified,
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Read the raw `SKILL.md` body for a single skill. Returns
/// `Err` if the skill dir doesn't exist or the file is missing —
/// callers should treat both as "the skill isn't here" and not try
/// to differentiate. Transitional `AGENT.md` is read transparently
/// for skills caught mid-migration.
pub fn profile(project_path: &str, name: &str) -> Result<String, String> {
    let dir = skill_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Skill '{}' does not exist", name));
    }
    let skill_md = dir.join("SKILL.md");
    let agent_md = dir.join("AGENT.md");
    let path = if skill_md.exists() {
        skill_md
    } else if agent_md.exists() {
        agent_md
    } else {
        return Err(format!("Skill '{}' has no SKILL.md or AGENT.md", name));
    };
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Create a new skill dir at `.k2so/skills/<name>/` and write a
/// `SKILL.md` seed.
///
/// `from_skill` (optional) is the name of an existing skill in this
/// workspace to copy as the starting point. Per A19's "any skill can
/// seed any other" semantic this is just `cp source/SKILL.md
/// dest/SKILL.md`, with the new skill's `name:` frontmatter rewritten
/// to match the new folder name (so the seed doesn't leak the old
/// identity into the new skill). If `from_skill` is `None`, the new
/// skill gets a minimal scaffold:
///
/// ```text
/// ---
/// name: <name>
/// role:
/// type: skill
/// ---
///
/// # <name>
///
/// _Documentation profile for <name>. Edit me._
/// ```
///
/// Name validation: alphanumeric + `-` / `_` only. Mirrors the
/// validation in `k2_core::agents::commands::create` (moved to
/// `crate::deprecated::*` in Phase 2.5c).
pub fn create(
    project_path: &str,
    name: &str,
    from_skill: Option<&str>,
) -> Result<SkillSummary, String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(
            "Skill name must be alphanumeric (hyphens and underscores allowed)".to_string(),
        );
    }
    let dir = skill_dir(project_path, name);
    if dir.exists() {
        return Err(format!("Skill '{}' already exists", name));
    }

    // Make sure `.k2so/skills/` itself exists — the parent of `dir`.
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!("Failed to create skills root {}: {}", parent.display(), e)
        })?;
    }
    fs::create_dir_all(&dir).map_err(|e| {
        format!("Failed to create skill dir {}: {}", dir.display(), e)
    })?;

    let body = match from_skill {
        Some(seed_name) => {
            let seed = profile(project_path, seed_name).map_err(|e| {
                format!("Failed to read seed skill '{}': {}", seed_name, e)
            })?;
            rewrite_name_frontmatter(&seed, name)
        }
        None => default_scaffold(name),
    };

    let skill_md = dir.join("SKILL.md");
    fs::write(&skill_md, &body)
        .map_err(|e| format!("Failed to write {}: {}", skill_md.display(), e))?;

    Ok(SkillSummary {
        name: name.to_string(),
        title: first_h1(&skill_md),
        last_modified: mtime_secs(&skill_md),
    })
}

/// Trash a skill dir (`.k2so/skills/<name>/`) — recoverable via the
/// OS recycle bin for ~30 days. Refuses if the dir doesn't exist so
/// the UI can show a meaningful error instead of silently no-op'ing.
/// Per `feedback_recycle_bin_tests`: NEVER `fs::remove_dir_all` here.
pub fn remove(project_path: &str, name: &str) -> Result<(), String> {
    let dir = skill_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Skill '{}' does not exist", name));
    }
    safe_delete::trash(&dir).map_err(|e| format!("Failed to trash skill '{}': {}", name, e))
}

// ── Internals ────────────────────────────────────────────────────────

fn first_h1(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    // Skip YAML frontmatter (the same parser shape `parse_frontmatter`
    // uses): if the file opens with `---`, walk past the next `---`.
    let body_start = if content.starts_with("---") {
        match content[3..].find("---") {
            Some(end) => 3 + end + 3,
            None => 0,
        }
    } else {
        0
    };
    for line in content[body_start..].lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn mtime_secs(path: &Path) -> i64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let Ok(mt) = meta.modified() else { return 0 };
    match mt.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

fn default_scaffold(name: &str) -> String {
    format!(
        "---\nname: {}\nrole:\ntype: skill\n---\n\n# {}\n\n_Documentation profile for {}. Edit me._\n",
        name, name, name
    )
}

/// When seeding a new skill from an existing one, rewrite the
/// frontmatter `name:` line to match the destination. Leaves every
/// other field (role, type, manager, etc.) alone — that's the whole
/// point of seeding.
fn rewrite_name_frontmatter(content: &str, new_name: &str) -> String {
    if !content.starts_with("---") {
        return content.to_string();
    }
    let Some(end_idx) = content[3..].find("---") else {
        return content.to_string();
    };
    let frontmatter = &content[3..3 + end_idx];
    let body = &content[3 + end_idx + 3..];

    let mut saw_name = false;
    let updated_fm: String = frontmatter
        .lines()
        .map(|line| {
            if let Some((key, _)) = line.split_once(':') {
                if key.trim() == "name" {
                    saw_name = true;
                    return format!("name: {}", new_name);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    let final_fm = if saw_name {
        updated_fm
    } else {
        format!("name: {}\n{}", new_name, updated_fm.trim_start_matches('\n'))
    };
    format!("---\n{}\n---{}", final_fm.trim(), body)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace() -> std::path::PathBuf {
        let base = env::temp_dir().join(format!(
            "k2so-skills-crud-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn cleanup(ws: &Path) {
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn list_empty_workspace_returns_empty() {
        let ws = temp_workspace();
        let result = list(ws.to_str().unwrap()).unwrap();
        assert!(result.is_empty());
        cleanup(&ws);
    }

    #[test]
    fn create_then_list_round_trips() {
        let ws = temp_workspace();
        let summary = create(ws.to_str().unwrap(), "frontend-eng", None).unwrap();
        assert_eq!(summary.name, "frontend-eng");
        assert_eq!(summary.title.as_deref(), Some("frontend-eng"));

        let listed = list(ws.to_str().unwrap()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "frontend-eng");
        cleanup(&ws);
    }

    #[test]
    fn create_rejects_bad_names() {
        let ws = temp_workspace();
        assert!(create(ws.to_str().unwrap(), "", None).is_err());
        assert!(create(ws.to_str().unwrap(), "has space", None).is_err());
        assert!(create(ws.to_str().unwrap(), "has/slash", None).is_err());
        cleanup(&ws);
    }

    #[test]
    fn create_rejects_duplicate() {
        let ws = temp_workspace();
        create(ws.to_str().unwrap(), "dup", None).unwrap();
        let err = create(ws.to_str().unwrap(), "dup", None).unwrap_err();
        assert!(err.contains("already exists"), "got: {}", err);
        cleanup(&ws);
    }

    #[test]
    fn create_from_seed_copies_body_and_rewrites_name() {
        let ws = temp_workspace();
        // Seed source with custom body.
        let src = ws.join(".k2so").join("skills").join("source");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("SKILL.md"),
            "---\nname: source\nrole: source role\n---\n\n# source\n\nseed body\n",
        )
        .unwrap();

        let summary = create(ws.to_str().unwrap(), "copy", Some("source")).unwrap();
        assert_eq!(summary.name, "copy");

        let body = profile(ws.to_str().unwrap(), "copy").unwrap();
        assert!(body.contains("name: copy"), "frontmatter not rewritten: {}", body);
        assert!(body.contains("role: source role"), "role lost: {}", body);
        assert!(body.contains("seed body"), "body not copied: {}", body);
        cleanup(&ws);
    }

    #[test]
    fn profile_reads_skill_md() {
        let ws = temp_workspace();
        create(ws.to_str().unwrap(), "alpha", None).unwrap();
        let body = profile(ws.to_str().unwrap(), "alpha").unwrap();
        assert!(body.contains("name: alpha"));
        assert!(body.contains("# alpha"));
        cleanup(&ws);
    }

    #[test]
    fn profile_falls_back_to_agent_md_during_migration() {
        let ws = temp_workspace();
        let dir = ws.join(".k2so").join("skills").join("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("AGENT.md"),
            "---\nname: legacy\n---\n\nlegacy body\n",
        )
        .unwrap();
        let body = profile(ws.to_str().unwrap(), "legacy").unwrap();
        assert!(body.contains("legacy body"));
        cleanup(&ws);
    }

    #[test]
    fn profile_missing_skill_errors() {
        let ws = temp_workspace();
        let err = profile(ws.to_str().unwrap(), "nope").unwrap_err();
        assert!(err.contains("does not exist"), "got: {}", err);
        cleanup(&ws);
    }

    #[test]
    fn list_skips_dotfile_dirs() {
        let ws = temp_workspace();
        // Real skill.
        create(ws.to_str().unwrap(), "real", None).unwrap();
        // Synthetic .archive sibling.
        std::fs::create_dir_all(ws.join(".k2so").join("skills").join(".archive")).unwrap();
        let listed = list(ws.to_str().unwrap()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "real");
        cleanup(&ws);
    }

    #[test]
    fn first_h1_skips_frontmatter() {
        let ws = temp_workspace();
        let dir = ws.join(".k2so").join("skills").join("hh");
        std::fs::create_dir_all(&dir).unwrap();
        // Frontmatter contains a `# foo`-shaped comment that must
        // NOT win over the real H1 in the body.
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: hh\n# not a heading\n---\n\n# Real Title\n\nbody\n",
        )
        .unwrap();
        let listed = list(ws.to_str().unwrap()).unwrap();
        assert_eq!(listed[0].title.as_deref(), Some("Real Title"));
        cleanup(&ws);
    }

    #[test]
    fn remove_errors_when_skill_missing() {
        let ws = temp_workspace();
        let err = remove(ws.to_str().unwrap(), "nope").unwrap_err();
        assert!(err.contains("does not exist"), "got: {}", err);
        cleanup(&ws);
    }

    // NOTE: a `remove_round_trips_to_trash` test would shell into the
    // macOS Trash via the `trash` crate, which fires a Finder Touch ID
    // prompt during `cargo test` on macOS. Per memory/feedback_recycle_bin_tests.md
    // we deliberately omit this test from the unit suite; the CLI / Tauri
    // path is exercised end-to-end via the parity shell test instead.
}



// Phase 2.5d: Skill regeneration relocated from
// `agents/commands.rs::regenerate_skills`. The Tauri command +
// daemon CLI both call into this canonical home post-split.
//
// 0.39.0: migrated source-of-truth from `.k2so/agents/<name>/` to
// `.k2so/skills/<name>/`. Pre-fix, this function read from a path that
// no post-2.5b workspace has (consolidation moved it all under
// `.k2so/skills/`), so every CLI invocation silently no-op'd. The
// dedicated migration tool (`crate::skills::consolidation`) still
// reads the legacy layout for upgrade flows — DO NOT change that one.
use crate::skills::writer::write_agent_skill_file;

/// Read `<dir>/SKILL.md` (preferred) or `<dir>/AGENT.md` (transitional
/// legacy shape from the 2.5b consolidator) and derive the `agent_type`
/// argument expected by [`write_agent_skill_file`]: `"manager"`,
/// `"k2so"`, `"custom"`, or `"agent-template"` (the catch-all).
///
/// Prefers the K2SO-managed `k2so_skill:` field when present (set by
/// the writer when fanning skills out), falls back to legacy `type:`
/// + `manager:` / `coordinator:` / `pod_leader:` booleans for
/// pre-consolidator content. Returns `None` when neither file exists
/// (caller should skip the entry — an empty `.k2so/skills/<name>/`
/// dir isn't a regenerateable skill).
fn classify_skill_dir(dir: &Path) -> Option<String> {
    let skill_md = dir.join("SKILL.md");
    let agent_md = dir.join("AGENT.md");
    let source = if skill_md.exists() {
        &skill_md
    } else if agent_md.exists() {
        &agent_md
    } else {
        return None;
    };
    let content = fs::read_to_string(source).unwrap_or_default();
    let fm = parse_frontmatter(&content);

    // `k2so_skill:` is the canonical post-2.5b tag emitted by
    // `wrap_managed_skill`. Map back to the `write_agent_skill_file`
    // dispatch keys.
    if let Some(k2so_skill) = fm.get("k2so_skill") {
        return Some(match k2so_skill.as_str() {
            "manager" => "manager".to_string(),
            "k2so-agent" => "k2so".to_string(),
            "custom-agent" => "custom".to_string(),
            // `workspace` is the workspace-root skill — regenerated by
            // the dedicated `regenerate_workspace_skill` entry point,
            // not this per-agent sweep. Callers must filter the `k2so`
            // dir out BEFORE getting here; if a different dir name
            // somehow carries the workspace tag, treat it as a template
            // so the regen at least writes SOMETHING upgrade-tracked.
            "workspace" => "agent-template".to_string(),
            _ => "agent-template".to_string(),
        });
    }

    // Legacy `type:` field from pre-consolidator AGENT.md files. The
    // consolidator rename-in-place preserves the original frontmatter,
    // so a freshly-migrated workspace's `.k2so/skills/<name>/SKILL.md`
    // may carry these legacy tags instead of `k2so_skill:`.
    let raw_type = fm.get("type").cloned().unwrap_or_default();
    let mapped = match raw_type.as_str() {
        "pod-leader" | "coordinator" | "manager" => "manager",
        "custom" => "custom",
        "k2so" => "k2so",
        "agent-template" => "agent-template",
        _ => {
            let is_mgr = fm.get("manager").map(|v| v == "true").unwrap_or(false)
                || fm.get("coordinator").map(|v| v == "true").unwrap_or(false)
                || fm.get("pod_leader").map(|v| v == "true").unwrap_or(false);
            if is_mgr {
                "manager"
            } else {
                "agent-template"
            }
        }
    };
    Some(mapped.to_string())
}

/// Regenerate SKILL.md files for every agent in a workspace. Called
/// on app startup (migration sweep) and via `k2so skills regenerate`
/// from the CLI.
///
/// Walks `.k2so/skills/<name>/` (post-2.5b unified layout). Missing
/// `.k2so/skills/` is a benign no-op that returns `{ "updated": 0 }`.
///
/// Skips:
/// - `k2so/` — the workspace-root skill, regenerated by the dedicated
///   `crate::workspace::skill_regen::regenerate_workspace_skill`.
/// - `<name>` starting with `.` (hidden / `.archive/` dirs left over
///   from agent retirement).
/// - Harness-fanout MIRRORS — these are the copies emitted by
///   `write_skill_to_all_harnesses` when an agent regen runs (it
///   writes both `.k2so/skills/<agent>/SKILL.md` AND
///   `.k2so/skills/k2so-<agent>/SKILL.md`). A directory is treated
///   as a mirror when its name is `k2so-<X>` AND a sibling source
///   directory `<X>/` also exists at `.k2so/skills/<X>/`. The bare
///   `k2so-<X>` heuristic isn't enough — a legitimate agent named
///   `k2so-agent` (the K2SO planner role) carries the `k2so-`
///   prefix as part of its own name; we must NOT skip it just for
///   that. Iterating the mirrors would double-regenerate (and
///   re-key the underlying agent the second pass).
///
/// We deliberately do NOT fall back to `.k2so/agents/` even when it
/// exists alongside `.k2so/skills/` — leaving partial-migration
/// leftovers to the dedicated `consolidate_skills_v1` flow keeps the
/// regen single-source and idempotent.
///
/// Returns `{ "updated": N }` where N is the number of skill dirs
/// that were processed.
pub fn regenerate_skills(project_path: String) -> Result<serde_json::Value, String> {
    let skills_root = skills_dir(&project_path);
    if !skills_root.exists() {
        return Ok(serde_json::json!({"updated": 0}));
    }

    // First pass: collect every candidate source dir name. We need
    // the full set up front so the second pass can tell mirrors
    // (`k2so-<X>` with a sibling `<X>/`) apart from legitimate
    // `k2so-*`-prefixed agent names (e.g., `k2so-agent` the planner).
    let mut all_dirs: Vec<String> = Vec::new();
    if let Ok(entries) = fs::read_dir(&skills_root) {
        for entry in entries.flatten() {
            if entry
                .file_type()
                .map_or(false, |ft| ft.is_dir())
            {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') && name != "k2so" {
                    all_dirs.push(name);
                }
            }
        }
    }
    let dir_set: std::collections::HashSet<&str> =
        all_dirs.iter().map(|s| s.as_str()).collect();

    let mut updated = 0;
    for name in &all_dirs {
        // Skip harness-fanout mirrors: `k2so-<X>` is a mirror IFF
        // `<X>` also exists in the same skills dir.
        if let Some(stripped) = name.strip_prefix("k2so-") {
            if dir_set.contains(stripped) {
                continue;
            }
        }

        let path = skills_root.join(name);
        let Some(agent_type) = classify_skill_dir(&path) else {
            // Empty dir with no SKILL.md / AGENT.md — nothing
            // to regenerate. (`skills::crud::create` always
            // scaffolds at least a SKILL.md, so this is mostly
            // defensive against hand-edited workspaces.)
            continue;
        };

        // Single dispatch to the canonical writer — handles both
        // the agent-dir SKILL.md (upgrade-tracked) and the harness
        // fanout (.claude/, .opencode/, .pi/, plus the
        // `.k2so/skills/k2so-<name>/SKILL.md` mirror) in one call.
        write_agent_skill_file(&project_path, name, &agent_type);
        updated += 1;
    }

    // Also refresh the workspace-root skill (`.k2so/skills/k2so/SKILL.md`).
    // This is the user-facing CLI command's intent: "regenerate everything
    // in this workspace's `.k2so/skills/`". The dedicated
    // `regenerate_workspace_skill` entry point handles the workspace skill
    // (scaffolding, inbox snapshot, drift adoption, SOURCE region
    // composition) — best-effort because per-agent regens above already
    // shipped, so a workspace-skill failure shouldn't poison the
    // operation.
    //
    // The workspace skill only physically rewrites when
    // `SKILL_VERSION_WORKSPACE` advances OR the on-disk managed-region
    // checksum no longer matches the stamped checksum. Stale content
    // updates on workspaces still pinned to the current version sit on
    // disk until the constant is bumped — that's by design (per the
    // upgrade-protocol module docs).
    let _ = crate::workspace::skill_regen::regenerate_workspace_skill(project_path.clone());

    Ok(serde_json::json!({"updated": updated}))
}

#[cfg(test)]
mod regenerate_skills_tests {
    //! Tier 2 coverage for the 0.39.0 source-of-truth migration. The
    //! pre-fix function read `.k2so/agents/` and silently returned 0
    //! on every post-2.5b workspace — these pin the new behavior:
    //!
    //!   1. workspace with `.k2so/skills/<name>/SKILL.md` entries →
    //!      regenerate returns updated ≥ 1 AND the resulting SKILL.md
    //!      contains the upgrade-tracked frontmatter + uses canonical
    //!      A25 verbs only.
    //!   2. workspace with no `.k2so/skills/` → returns `{ updated: 0 }`
    //!      without panicking.
    //!   3. workspace with both `.k2so/skills/` AND legacy
    //!      `.k2so/agents/` → regenerates only from `.k2so/skills/`
    //!      (the legacy tree is left untouched for the dedicated
    //!      consolidator to migrate).
    use super::*;
    use uuid::Uuid;

    fn scratch_project() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-regen-skills-test-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Hard-deprecated verbs from Phase 2.1 A25. Mirrors the list in
    /// `skills::content::tests::DEPRECATED_VERBS` — kept here as a
    /// local copy so this test module is self-contained (the upstream
    /// list lives in a `cfg(test)` module and can't be referenced
    /// across compilation units).
    const DEPRECATED_VERBS: &[&str] = &[
        "k2so delegate",
        "k2so work create",
        "k2so work send",
        "k2so work move",
        "k2so work inbox",
        "k2so signal",
        "k2so app-update",
        "k2so agents create",
        "k2so agents delete",
        "k2so agents list",
        "k2so agents running",
        "k2so agents work",
    ];

    fn assert_no_deprecated_verbs(content: &str, context: &str) {
        let mut hits: Vec<&str> = Vec::new();
        for verb in DEPRECATED_VERBS {
            if content.contains(verb) {
                hits.push(verb);
            }
        }
        assert!(
            hits.is_empty(),
            "{context}: regenerated SKILL.md must NOT contain hard-deprecated verbs; found {hits:?}\n\
             Excerpt (first 400 chars):\n{}",
            &content[..content.len().min(400)],
        );
    }

    #[test]
    fn regenerate_skills_returns_zero_when_skills_dir_missing() {
        let proj = scratch_project();
        let out = regenerate_skills(proj.to_string_lossy().to_string())
            .expect("regen ok");
        assert_eq!(out, serde_json::json!({"updated": 0}));
        std::fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_skills_processes_skills_dir_entries() {
        let proj = scratch_project();
        // Seed a skill under the new layout. Use a transitional
        // shape (AGENT.md content with legacy `type:` frontmatter)
        // because that's the worst-case input — if the regen handles
        // it, it'll handle the already-upgraded SKILL.md shape too.
        let cli_eng_dir = proj.join(".k2so/skills/cli-eng");
        std::fs::create_dir_all(&cli_eng_dir).unwrap();
        std::fs::write(
            cli_eng_dir.join("SKILL.md"),
            "---\nname: cli-eng\nrole: CLI engineer\ntype: agent-template\n---\n\nOld body.\n",
        )
        .unwrap();

        let out = regenerate_skills(proj.to_string_lossy().to_string())
            .expect("regen ok");
        let updated = out
            .get("updated")
            .and_then(|v| v.as_u64())
            .expect("updated key present");
        assert!(
            updated >= 1,
            "regen should process the seeded cli-eng skill; got updated={updated}, out={out}",
        );

        // The regenerated SKILL.md should now carry the K2SO-managed
        // frontmatter + use canonical A25 verbs (no deprecated ones).
        let regenerated = std::fs::read_to_string(cli_eng_dir.join("SKILL.md"))
            .expect("agent-dir SKILL.md exists post-regen");
        assert!(
            regenerated.contains("k2so_skill:"),
            "agent-dir SKILL.md should be upgraded to the managed shape; got:\n{}",
            &regenerated[..regenerated.len().min(400)],
        );
        assert_no_deprecated_verbs(&regenerated, "cli-eng agent-dir SKILL.md");

        // The harness-fanout mirror at `.k2so/skills/k2so-cli-eng/` should
        // also exist (written by `write_skill_to_all_harnesses` inside
        // `write_agent_skill_file`).
        let mirror = proj.join(".k2so/skills/k2so-cli-eng/SKILL.md");
        assert!(
            mirror.exists(),
            "harness-fanout mirror should be written by the canonical writer",
        );
        let mirror_content = std::fs::read_to_string(&mirror).unwrap();
        assert_no_deprecated_verbs(&mirror_content, "k2so-cli-eng harness mirror");

        std::fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_skills_ignores_legacy_agents_dir_when_skills_present() {
        let proj = scratch_project();

        // Seed the canonical skills/ layout with one entry.
        let cli_eng_dir = proj.join(".k2so/skills/cli-eng");
        std::fs::create_dir_all(&cli_eng_dir).unwrap();
        std::fs::write(
            cli_eng_dir.join("SKILL.md"),
            "---\nname: cli-eng\ntype: agent-template\n---\n\nbody.\n",
        )
        .unwrap();

        // Also seed a leftover legacy agents/ entry. This should be
        // IGNORED by regen (leftover from a partial consolidation
        // run; the dedicated `consolidate_skills_v1` is responsible
        // for finishing the migration).
        let legacy_dir = proj.join(".k2so/agents/leftover");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        std::fs::write(
            legacy_dir.join("AGENT.md"),
            "---\nname: leftover\ntype: agent-template\n---\n",
        )
        .unwrap();

        let out = regenerate_skills(proj.to_string_lossy().to_string())
            .expect("regen ok");
        let updated = out
            .get("updated")
            .and_then(|v| v.as_u64())
            .expect("updated key present");
        assert_eq!(
            updated, 1,
            "regen must iterate ONLY .k2so/skills/ (1 entry), NOT also .k2so/agents/leftover; got {out}",
        );

        // Belt-and-suspenders: the legacy dir wasn't touched (no
        // SKILL.md written into it).
        assert!(
            !legacy_dir.join("SKILL.md").exists(),
            "legacy .k2so/agents/leftover/ must remain untouched by regen",
        );

        std::fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_skills_skips_workspace_skill_and_harness_mirrors() {
        let proj = scratch_project();

        // Seed the workspace skill (`k2so/`) and a harness-fanout mirror
        // (`k2so-cli-eng/`). Both must be SKIPPED — only the source
        // `cli-eng/` should be processed.
        let workspace_skill = proj.join(".k2so/skills/k2so");
        std::fs::create_dir_all(&workspace_skill).unwrap();
        std::fs::write(
            workspace_skill.join("SKILL.md"),
            "---\nk2so_skill: workspace\n---\n",
        )
        .unwrap();

        let mirror = proj.join(".k2so/skills/k2so-cli-eng");
        std::fs::create_dir_all(&mirror).unwrap();
        std::fs::write(
            mirror.join("SKILL.md"),
            "---\nk2so_skill: agent-template\n---\n",
        )
        .unwrap();

        let cli_eng = proj.join(".k2so/skills/cli-eng");
        std::fs::create_dir_all(&cli_eng).unwrap();
        std::fs::write(
            cli_eng.join("SKILL.md"),
            "---\nname: cli-eng\ntype: agent-template\n---\n",
        )
        .unwrap();

        let out = regenerate_skills(proj.to_string_lossy().to_string())
            .expect("regen ok");
        let updated = out
            .get("updated")
            .and_then(|v| v.as_u64())
            .expect("updated key present");
        // cli-eng counts once. Workspace skill + harness mirror skipped.
        // (The mirror DOES get rewritten as a side-effect of processing
        // cli-eng — `write_skill_to_all_harnesses` re-emits it — but
        // it's not counted in `updated` because it wasn't iterated.)
        assert_eq!(
            updated, 1,
            "regen must skip k2so/ + k2so-* dirs; only cli-eng counts. got {out}",
        );

        std::fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_skills_processes_literal_k2so_agent_dir() {
        // Regression guard: `k2so-agent` (the K2SO planner role) is a
        // legitimate agent name that happens to share the `k2so-`
        // prefix with the harness-fanout mirror naming convention.
        // The mirror-detection heuristic must check for a sibling
        // SOURCE dir (here, `agent/`) before skipping — without
        // that check, every workspace's K2SO planner agent would
        // silently no-op on regen.
        let proj = scratch_project();

        let k2so_agent = proj.join(".k2so/skills/k2so-agent");
        std::fs::create_dir_all(&k2so_agent).unwrap();
        std::fs::write(
            k2so_agent.join("SKILL.md"),
            "---\nname: k2so-agent\ntype: k2so\n---\n",
        )
        .unwrap();

        let out = regenerate_skills(proj.to_string_lossy().to_string())
            .expect("regen ok");
        let updated = out
            .get("updated")
            .and_then(|v| v.as_u64())
            .expect("updated key present");
        assert_eq!(
            updated, 1,
            "regen must process k2so-agent/ because no sibling agent/ exists; got {out}",
        );

        // Confirm the upgrade actually happened (managed frontmatter
        // appears in the regenerated file).
        let regenerated = std::fs::read_to_string(k2so_agent.join("SKILL.md"))
            .expect("k2so-agent SKILL.md exists post-regen");
        assert!(
            regenerated.contains("k2so_skill: k2so-agent"),
            "k2so-agent must be upgraded with the managed shape; got:\n{}",
            &regenerated[..regenerated.len().min(400)],
        );

        std::fs::remove_dir_all(&proj).ok();
    }
}
