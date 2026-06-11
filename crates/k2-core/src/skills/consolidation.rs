//! Phase 2.5b first-boot migration: collapse `.k2so/agents/`,
//! `.k2so/agent-templates/`, and bare-md `.k2so/skills/*.md` into a
//! single folder-with-SKILL.md home at `.k2so/skills/<name>/`.
//!
//! Idempotent — a marker file at `.k2so/.skills-consolidation-v1-done`
//! gates re-runs so the daemon's boot sweep can call this on every
//! tick without doing real work after the first successful pass.
//!
//! See [`crate::skills`] module docs for the design narrative and
//! collision rule.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Marker file written after a successful (or no-op) consolidation
/// pass. Subsequent calls short-circuit on this.
fn consolidation_marker(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(workspace).join(".skills-consolidation-v1-done")
}

fn skills_root(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(&workspace).join("skills")
}

fn agents_root(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(&workspace).join("agents")
}

fn agent_templates_root(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(&workspace).join("agent-templates")
}

fn workspace_heartbeats_root(workspace: &Path) -> PathBuf {
    crate::workspace_dot_dir(&workspace).join("heartbeats")
}

/// Outcome of [`consolidate_skills_v1`]. Counts are zero on
/// already-migrated and on no-op fresh workspaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConsolidationOutcome {
    /// Migration was skipped because the marker file already existed.
    pub already_migrated: bool,
    /// Number of bare-md `.k2so/skills/<x>.md` files normalized into
    /// `.k2so/skills/<x>/SKILL.md` folders.
    pub bare_md_normalized: usize,
    /// Number of legacy instance dirs (`.k2so/agents/<x>/`) moved to
    /// `.k2so/skills/<x>/`.
    pub instances_moved: usize,
    /// Number of template dirs (`.k2so/agent-templates/<x>/`) moved
    /// to `.k2so/skills/<x>/` (or `<x>-template0N/` on collision).
    pub templates_moved: usize,
    /// Number of templates that needed a `-templateNN` suffix because
    /// `<x>/` was already populated by a higher-priority source.
    pub templates_suffixed: usize,
    /// Number of legacy `AGENT.md` files renamed to `SKILL.md` during
    /// the instance move (only happens when the source dir had
    /// AGENT.md with no SKILL.md sibling).
    pub agent_md_renamed: usize,
    /// Number of legacy `AGENT.md` files discarded because a sibling
    /// `SKILL.md` already existed in the source dir (SKILL wins per
    /// PRD Open Question #1).
    pub agent_md_discarded: usize,
    /// Number of per-skill heartbeat WAKEUP files moved out of
    /// `.k2so/agents/<x>/heartbeats/` into the workspace-level
    /// `.k2so/heartbeats/` with the skill name prefixed.
    pub heartbeats_migrated: usize,
    /// `.k2so/agents/` was sent to the OS recycle bin.
    pub trashed_agents: bool,
    /// `.k2so/agent-templates/` was sent to the OS recycle bin.
    pub trashed_agent_templates: bool,
    /// Non-fatal per-step errors collected during the run. Best-effort
    /// migration: per-entry failures don't abort the rest.
    pub errors: Vec<String>,
}

/// One-shot first-boot migration. Consolidates the three legacy skill
/// sources into `.k2so/skills/<name>/SKILL.md`. Returns counts +
/// errors so the daemon log can summarize what happened.
///
/// Hard rules (per PRD):
/// - Idempotent via marker file.
/// - `safe_delete::trash` for source-folder retirement (no
///   `fs::remove_dir_all` — sources may contain user-authored content).
/// - Collision priority: instance > template > bare layer.
/// - AGENT.md normalized to SKILL.md; SKILL.md sibling wins.
/// - Per-skill heartbeats moved to workspace-level with name prefix.
pub fn consolidate_skills_v1(workspace: &Path) -> ConsolidationOutcome {
    let marker = consolidation_marker(workspace);
    if marker.exists() {
        return ConsolidationOutcome {
            already_migrated: true,
            ..Default::default()
        };
    }

    let mut outcome = ConsolidationOutcome::default();

    // Ensure .k2so/ exists so we can at least write the marker on a
    // fresh workspace.
    if let Err(e) = fs::create_dir_all(crate::workspace_dot_dir(&workspace)) {
        outcome.errors.push(format!(
            "create .k2so/: {}",
            e
        ));
        return outcome;
    }
    let skills = skills_root(workspace);
    if let Err(e) = fs::create_dir_all(&skills) {
        outcome
            .errors
            .push(format!("create {}: {}", skills.display(), e));
        return outcome;
    }

    // Step 1: normalize bare-md `.k2so/skills/<x>.md` → `.k2so/skills/<x>/SKILL.md`.
    // Done first so instance/template moves treat the normalized
    // folder as a potential collision target.
    if let Ok(entries) = fs::read_dir(&skills) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_md_file = path.is_file()
                && path.extension().map_or(false, |e| e == "md");
            if !is_md_file {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    outcome
                        .errors
                        .push(format!("bare-md missing stem: {}", path.display()));
                    continue;
                }
            };
            let target_dir = skills.join(&stem);
            if target_dir.exists() {
                outcome.errors.push(format!(
                    "bare-md collision: {} vs existing {}; leaving bare file in place",
                    path.display(),
                    target_dir.display()
                ));
                continue;
            }
            if let Err(e) = fs::create_dir_all(&target_dir) {
                outcome
                    .errors
                    .push(format!("create {}: {}", target_dir.display(), e));
                continue;
            }
            let dest = target_dir.join("SKILL.md");
            match fs::rename(&path, &dest) {
                Ok(()) => outcome.bare_md_normalized += 1,
                Err(e) => outcome.errors.push(format!(
                    "rename {} → {}: {}",
                    path.display(),
                    dest.display(),
                    e
                )),
            }
        }
    }

    // Step 2: move instances (`.k2so/agents/<x>/` — highest priority).
    let agents = agents_root(workspace);
    if agents.exists() {
        if let Ok(entries) = fs::read_dir(&agents) {
            for entry in entries.flatten() {
                let src = entry.path();
                if !src.is_dir() {
                    continue;
                }
                let name_os = match src.file_name() {
                    Some(n) => n.to_owned(),
                    None => continue,
                };
                let name_str = name_os.to_string_lossy().to_string();
                // Preserve `.archive/` (and any other dotfile) inside
                // the source so trash() picks it up as one unit — we
                // don't promote retired-agent backups into the
                // skill catalog.
                if name_str.starts_with('.') {
                    continue;
                }
                let dest = skills.join(&name_os);
                migrate_instance_dir(&src, &dest, &mut outcome);
            }
        }
    }

    // Step 3: move templates (`.k2so/agent-templates/<x>/` — lower
    // priority, suffix on collision).
    let templates = agent_templates_root(workspace);
    if templates.exists() {
        if let Ok(entries) = fs::read_dir(&templates) {
            for entry in entries.flatten() {
                let src = entry.path();
                if !src.is_dir() {
                    continue;
                }
                let name_os = match src.file_name() {
                    Some(n) => n.to_owned(),
                    None => continue,
                };
                let name_str = name_os.to_string_lossy().to_string();
                if name_str.starts_with('.') {
                    continue;
                }
                let (dest, suffixed) = resolve_template_dest(&skills, &name_str);
                if suffixed {
                    outcome.templates_suffixed += 1;
                }
                migrate_template_dir(&src, &dest, &mut outcome);
            }
        }
    }

    // Step 4: send the now-empty (or .archive-only) source roots to
    // the OS recycle bin. Recoverable for ~30 days if the user wants
    // the originals back. safe_delete::trash is the canonical path
    // for any deletion that could touch user content.
    //
    // SAFETY: routes through `scratch_safe_trash` so test scratch
    // paths under std::env::temp_dir() bypass the trash crate (avoids
    // macOS Touch ID prompts during cargo test + bash-CLI sandbox
    // runs — `skills_consolidation_first_boot.sh` previously fired
    // two prompts here). Production workspaces still trash.
    if agents.exists() {
        match crate::safe_delete_scratch::scratch_safe_trash(&agents) {
            Ok(()) => outcome.trashed_agents = true,
            Err(e) => outcome
                .errors
                .push(format!("trash {}: {}", agents.display(), e)),
        }
    }
    if templates.exists() {
        match crate::safe_delete_scratch::scratch_safe_trash(&templates) {
            Ok(()) => outcome.trashed_agent_templates = true,
            Err(e) => outcome
                .errors
                .push(format!("trash {}: {}", templates.display(), e)),
        }
    }

    // Step 5: marker file. Best-effort — a write failure just means
    // we retry on the next boot (and the steps above are all
    // idempotent on re-run because the source roots are gone).
    if let Err(e) = fs::write(&marker, "v1") {
        outcome
            .errors
            .push(format!("write consolidation marker: {e}"));
    }
    outcome
}

/// Pick a destination dir for a template. If `name/` is already taken
/// (because an instance with the same name landed there or the bare
/// layer normalization claimed it), apply a numeric suffix
/// `-template01`, `-template02`, … until we find an unused slot.
fn resolve_template_dest(skills_root: &Path, name: &str) -> (PathBuf, bool) {
    let direct = skills_root.join(name);
    if !direct.exists() {
        return (direct, false);
    }
    let mut suffix: u32 = 1;
    loop {
        let candidate = skills_root.join(format!("{}-template{:02}", name, suffix));
        if !candidate.exists() {
            return (candidate, true);
        }
        suffix += 1;
        // Sanity bound: if the user has 99 templates with the same
        // name we have bigger problems.
        if suffix > 99 {
            return (skills_root.join(format!("{}-template99-overflow", name)), true);
        }
    }
}

/// Move an instance dir from `.k2so/agents/<name>/` to `dest`. Handles:
/// - AGENT.md → SKILL.md normalization (rename when no SKILL.md
///   sibling; discard AGENT.md when SKILL.md already exists).
/// - `heartbeats/<sched>/WAKEUP.md` → `.k2so/heartbeats/<name>-<sched>/WAKEUP.md`.
/// - Atomic-rename when possible (same filesystem).
fn migrate_instance_dir(src: &Path, dest: &Path, outcome: &mut ConsolidationOutcome) {
    // Special-case: dest exists from the bare-md normalization step.
    // Per PRD: instance wins. Move source children into dest,
    // overwriting any normalized SKILL.md.
    if dest.exists() {
        // Merge files from src into dest (instance wins for any
        // collision). Then trash the now-empty src to avoid leaving
        // it for the bulk trash of `.k2so/agents/` (which still
        // happens, but the entries inside should be empty by then).
        merge_dir_source_wins(src, dest, outcome);
        // Don't increment instances_moved if dest was a normalized
        // bare layer — the layer existed, the instance overwrote it,
        // net count of instance dirs migrated stays at +1.
        outcome.instances_moved += 1;
        // Normalize AGENT.md after the merge so the file lands at
        // dest first then gets renamed.
        handle_agent_md_at(dest, outcome);
        handle_heartbeats_subdir(src, dest, outcome);
        return;
    }

    // Fast path: rename the dir wholesale.
    if let Err(e) = fs::rename(src, dest) {
        outcome.errors.push(format!(
            "rename {} → {}: {} (falling back to merge)",
            src.display(),
            dest.display(),
            e
        ));
        // Fall back to merge so we still make progress on
        // cross-filesystem boundaries.
        if let Err(create_err) = fs::create_dir_all(dest) {
            outcome.errors.push(format!(
                "create {}: {}",
                dest.display(),
                create_err
            ));
            return;
        }
        merge_dir_source_wins(src, dest, outcome);
    }
    outcome.instances_moved += 1;
    // Heartbeats subdir handling has to happen on the DEST after the
    // rename — the source dir is gone.
    handle_heartbeats_subdir_at_dest(dest, src, outcome);
    handle_agent_md_at(dest, outcome);
}

/// Move a template dir from `.k2so/agent-templates/<name>/` to `dest`.
/// No special-case merge (templates always go to a fresh dir or a
/// suffixed-fresh dir; never collide with an existing skill).
fn migrate_template_dir(src: &Path, dest: &Path, outcome: &mut ConsolidationOutcome) {
    if let Err(e) = fs::rename(src, dest) {
        outcome.errors.push(format!(
            "rename {} → {}: {} (falling back to merge)",
            src.display(),
            dest.display(),
            e
        ));
        if let Err(create_err) = fs::create_dir_all(dest) {
            outcome.errors.push(format!(
                "create {}: {}",
                dest.display(),
                create_err
            ));
            return;
        }
        merge_dir_source_wins(src, dest, outcome);
    }
    outcome.templates_moved += 1;
    // AGENT.md → SKILL.md normalization for templates too — every
    // post-2.5b skill has the same shape (folder + SKILL.md).
    handle_agent_md_at(dest, outcome);
}

/// Recursive merge of `src/` into `dest/` where source wins on file
/// collision. Used as the fall-back path when a wholesale rename
/// can't happen (cross-FS or dest already populated).
fn merge_dir_source_wins(src: &Path, dest: &Path, outcome: &mut ConsolidationOutcome) {
    if !src.exists() {
        return;
    }
    if let Err(e) = fs::create_dir_all(dest) {
        outcome.errors.push(format!("create {}: {}", dest.display(), e));
        return;
    }
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            outcome
                .errors
                .push(format!("read_dir {}: {}", src.display(), e));
            return;
        }
    };
    for entry in entries.flatten() {
        let child_src = entry.path();
        let child_name = match child_src.file_name() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        let child_dest = dest.join(&child_name);
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            merge_dir_source_wins(&child_src, &child_dest, outcome);
            // After children moved, the src subdir should be empty;
            // best-effort cleanup so the outer trash() has nothing
            // left to recover.
            let _ = fs::remove_dir(&child_src);
        } else {
            // For files, source wins: remove dest if it exists, then
            // rename src into place.
            if child_dest.exists() {
                let _ = fs::remove_file(&child_dest);
            }
            if let Err(e) = fs::rename(&child_src, &child_dest) {
                outcome.errors.push(format!(
                    "rename {} → {}: {}",
                    child_src.display(),
                    child_dest.display(),
                    e
                ));
            }
        }
    }
}

/// AGENT.md → SKILL.md normalization for a skill dir. Per PRD Open
/// Question #1:
/// - If only AGENT.md exists: rename it to SKILL.md.
/// - If both exist: keep SKILL.md, discard AGENT.md (route to trash
///   so the user can recover if needed).
/// - If only SKILL.md exists: no-op.
fn handle_agent_md_at(skill_dir: &Path, outcome: &mut ConsolidationOutcome) {
    let agent_md = skill_dir.join("AGENT.md");
    let skill_md = skill_dir.join("SKILL.md");
    if !agent_md.exists() {
        return;
    }
    if skill_md.exists() {
        // Both exist. SKILL.md wins. Route AGENT.md to trash so the
        // user has a recovery path if their AGENT.md had unique
        // content not captured in SKILL.md.
        //
        // SAFETY: routes through `scratch_safe_trash` so test scratch
        // paths bypass the trash crate (no Touch ID prompts).
        match crate::safe_delete_scratch::scratch_safe_trash(&agent_md) {
            Ok(()) => outcome.agent_md_discarded += 1,
            Err(e) => outcome.errors.push(format!(
                "trash duplicate AGENT.md at {}: {}",
                agent_md.display(),
                e
            )),
        }
        return;
    }
    // Only AGENT.md exists: rename to SKILL.md.
    match fs::rename(&agent_md, &skill_md) {
        Ok(()) => outcome.agent_md_renamed += 1,
        Err(e) => outcome.errors.push(format!(
            "rename {} → {}: {}",
            agent_md.display(),
            skill_md.display(),
            e
        )),
    }
}

/// Handle `<skill_dir>/heartbeats/<sched>/WAKEUP.md` after a wholesale
/// rename. Walks the dest's `heartbeats/` subdir, moves each
/// schedule's WAKEUP.md (and any other contents) up to the
/// workspace-level `.k2so/heartbeats/<skill>-<sched>/`, then trashes
/// the now-orphan `<skill_dir>/heartbeats/`. The legacy_src arg
/// carries the original `.k2so/agents/<name>/` path so we can derive
/// the workspace root for the destination heartbeat dir.
fn handle_heartbeats_subdir_at_dest(
    dest: &Path,
    legacy_src: &Path,
    outcome: &mut ConsolidationOutcome,
) {
    let hb_dir = dest.join("heartbeats");
    if !hb_dir.exists() || !hb_dir.is_dir() {
        return;
    }
    // Workspace root = the legacy src's grandparent (.k2so/agents/foo
    // → workspace). Robust because legacy_src is constructed from
    // workspace.join(".k2so/agents/<name>").
    let workspace = legacy_src
        .parent() // .k2so/agents/
        .and_then(|p| p.parent()) // .k2so/
        .and_then(|p| p.parent()); // workspace root
    let Some(workspace) = workspace else {
        outcome.errors.push(format!(
            "cannot derive workspace root from {} for heartbeat migration",
            legacy_src.display()
        ));
        return;
    };
    let skill_name = match dest.file_name().and_then(|n| n.to_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            outcome.errors.push(format!(
                "skill dir missing name: {}",
                dest.display()
            ));
            return;
        }
    };
    move_heartbeats(workspace, &skill_name, &hb_dir, outcome);
}

/// Same as `handle_heartbeats_subdir_at_dest` but operates on the
/// SOURCE dir (used in the merge-not-rename path where the source
/// still exists).
fn handle_heartbeats_subdir(
    src: &Path,
    dest: &Path,
    outcome: &mut ConsolidationOutcome,
) {
    let hb_dir = src.join("heartbeats");
    if !hb_dir.exists() || !hb_dir.is_dir() {
        return;
    }
    let workspace = src.parent().and_then(|p| p.parent()).and_then(|p| p.parent());
    let Some(workspace) = workspace else {
        outcome.errors.push(format!(
            "cannot derive workspace root from {} for heartbeat migration",
            src.display()
        ));
        return;
    };
    let skill_name = match dest.file_name().and_then(|n| n.to_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return,
    };
    move_heartbeats(workspace, &skill_name, &hb_dir, outcome);
}

/// Walk a per-skill heartbeats dir and move each schedule subfolder
/// up to the workspace-level heartbeats dir with the skill name as a
/// prefix. Skips moves that would clobber an existing workspace-level
/// schedule (those are recorded as errors so the operator can
/// reconcile manually).
fn move_heartbeats(
    workspace: &Path,
    skill_name: &str,
    src_hb_dir: &Path,
    outcome: &mut ConsolidationOutcome,
) {
    let dest_root = workspace_heartbeats_root(workspace);
    if let Err(e) = fs::create_dir_all(&dest_root) {
        outcome
            .errors
            .push(format!("create {}: {}", dest_root.display(), e));
        return;
    }
    let entries = match fs::read_dir(src_hb_dir) {
        Ok(e) => e,
        Err(e) => {
            outcome
                .errors
                .push(format!("read_dir {}: {}", src_hb_dir.display(), e));
            return;
        }
    };
    for entry in entries.flatten() {
        let src_child = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let sched_name = match src_child.file_name().and_then(|n| n.to_str()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        let prefixed = format!("{}-{}", skill_name, sched_name);
        let dest_child = dest_root.join(&prefixed);
        if dest_child.exists() {
            outcome.errors.push(format!(
                "workspace-level heartbeat collision: {} already exists (left {} in place; reconcile manually)",
                dest_child.display(),
                src_child.display()
            ));
            continue;
        }
        match fs::rename(&src_child, &dest_child) {
            Ok(()) => outcome.heartbeats_migrated += 1,
            Err(e) => outcome.errors.push(format!(
                "rename {} → {}: {}",
                src_child.display(),
                dest_child.display(),
                e
            )),
        }
    }
    // Best-effort: remove the now-empty per-skill heartbeats dir.
    // safe_delete::trash would be overkill — this is K2SO-owned
    // scaffolding, no user content (the user's WAKEUP.md files
    // moved out individually above).
    let _ = fs::remove_dir(src_hb_dir);
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Self-cleaning scratch workspace under `std::env::temp_dir()`.
    /// Mirrors `inbox::tests::ScratchWs` so cargo can drop these
    /// without touching `~/.k2so/` or any user data.
    struct ScratchWs {
        path: PathBuf,
    }
    impl ScratchWs {
        fn new() -> Self {
            let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "k2so-skills-consolidation-test-{}-{}-{}",
                pid, nanos, n
            ));
            fs::create_dir_all(dir.join(".k2so")).unwrap();
            Self { path: dir }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for ScratchWs {
        fn drop(&mut self) {
            // Test scratch dir lives under std::env::temp_dir() —
            // safe to permanent-delete (no user-authored content,
            // and tests run in parallel so leaks accumulate without
            // this cleanup).
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn empty_workspace_is_noop_and_writes_marker() {
        let ws = ScratchWs::new();
        let out = consolidate_skills_v1(ws.path());
        assert!(!out.already_migrated, "first run must do work or mark");
        assert_eq!(out.instances_moved, 0);
        assert_eq!(out.templates_moved, 0);
        assert_eq!(out.bare_md_normalized, 0);
        assert!(consolidation_marker(ws.path()).exists());
        assert!(out.errors.is_empty(), "errors on empty ws: {:?}", out.errors);
    }

    #[test]
    fn idempotent_marker_short_circuits_second_call() {
        let ws = ScratchWs::new();
        // Seed an instance so the first call has real work.
        write_file(
            &ws.path().join(".k2so/agents/foo/SKILL.md"),
            "foo body",
        );
        let first = consolidate_skills_v1(ws.path());
        assert_eq!(first.instances_moved, 1);
        // Second call sees the marker.
        let second = consolidate_skills_v1(ws.path());
        assert!(second.already_migrated);
        assert_eq!(second.instances_moved, 0);
    }

    #[test]
    fn migrates_all_three_sources_to_skills() {
        let ws = ScratchWs::new();
        // Bare-md layer.
        write_file(
            &ws.path().join(".k2so/skills/baz.md"),
            "# Baz Layer\nbody\n",
        );
        // Instance.
        write_file(
            &ws.path().join(".k2so/agents/foo/SKILL.md"),
            "foo body",
        );
        // Template.
        write_file(
            &ws.path().join(".k2so/agent-templates/bar/AGENT.md"),
            "bar template",
        );

        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.bare_md_normalized, 1, "{:?}", out);
        assert_eq!(out.instances_moved, 1, "{:?}", out);
        assert_eq!(out.templates_moved, 1, "{:?}", out);
        // Template was AGENT.md → SKILL.md normalized.
        assert_eq!(out.agent_md_renamed, 1, "{:?}", out);

        // Verify the consolidated layout.
        let skills = ws.path().join(".k2so/skills");
        assert!(skills.join("baz/SKILL.md").exists(), "bare-md normalized");
        assert!(skills.join("foo/SKILL.md").exists(), "instance moved");
        assert!(skills.join("bar/SKILL.md").exists(), "template moved + AGENT renamed");
        // Source roots gone (trash on success).
        assert!(out.trashed_agents);
        assert!(out.trashed_agent_templates);
    }

    #[test]
    fn instance_wins_collision_template_suffixed() {
        let ws = ScratchWs::new();
        // Same name on both sides — instance must land at `frontend-eng/`,
        // template must get `frontend-eng-template01/`.
        write_file(
            &ws.path().join(".k2so/agents/frontend-eng/SKILL.md"),
            "INSTANCE",
        );
        write_file(
            &ws.path().join(".k2so/agent-templates/frontend-eng/AGENT.md"),
            "TEMPLATE",
        );

        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.instances_moved, 1);
        assert_eq!(out.templates_moved, 1);
        assert_eq!(out.templates_suffixed, 1);

        let skills = ws.path().join(".k2so/skills");
        assert!(skills.join("frontend-eng/SKILL.md").exists());
        let instance_body =
            fs::read_to_string(skills.join("frontend-eng/SKILL.md")).unwrap();
        assert_eq!(instance_body, "INSTANCE");

        assert!(skills.join("frontend-eng-template01/SKILL.md").exists());
        let template_body =
            fs::read_to_string(skills.join("frontend-eng-template01/SKILL.md")).unwrap();
        assert_eq!(template_body, "TEMPLATE");
    }

    #[test]
    fn bare_md_normalized_before_instance_so_instance_overwrites() {
        // Edge case: a skills/<x>.md bare layer AND an
        // agents/<x>/ instance with the same stem. After
        // normalization the dir exists; the instance must merge
        // into it and the instance's SKILL.md wins.
        let ws = ScratchWs::new();
        write_file(
            &ws.path().join(".k2so/skills/git.md"),
            "BARE-LAYER\n",
        );
        write_file(
            &ws.path().join(".k2so/agents/git/SKILL.md"),
            "INSTANCE-WINS",
        );

        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.bare_md_normalized, 1, "{:?}", out);
        assert_eq!(out.instances_moved, 1, "{:?}", out);

        let body = fs::read_to_string(
            ws.path().join(".k2so/skills/git/SKILL.md"),
        )
        .unwrap();
        assert_eq!(body, "INSTANCE-WINS");
    }

    #[test]
    fn agent_md_only_renamed_to_skill_md() {
        let ws = ScratchWs::new();
        write_file(
            &ws.path().join(".k2so/agents/legacy/AGENT.md"),
            "legacy persona",
        );
        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.agent_md_renamed, 1);
        assert!(ws
            .path()
            .join(".k2so/skills/legacy/SKILL.md")
            .exists());
        assert!(!ws
            .path()
            .join(".k2so/skills/legacy/AGENT.md")
            .exists());
    }

    #[test]
    fn agent_md_plus_skill_md_keeps_skill_discards_agent() {
        let ws = ScratchWs::new();
        write_file(
            &ws.path().join(".k2so/agents/both/AGENT.md"),
            "AGENT BODY",
        );
        write_file(
            &ws.path().join(".k2so/agents/both/SKILL.md"),
            "SKILL BODY",
        );
        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.agent_md_discarded, 1, "{:?}", out);
        assert_eq!(out.agent_md_renamed, 0, "{:?}", out);
        let body = fs::read_to_string(
            ws.path().join(".k2so/skills/both/SKILL.md"),
        )
        .unwrap();
        assert_eq!(body, "SKILL BODY");
        assert!(!ws
            .path()
            .join(".k2so/skills/both/AGENT.md")
            .exists());
    }

    #[test]
    fn heartbeats_migrated_to_workspace_level_with_prefix() {
        let ws = ScratchWs::new();
        write_file(
            &ws.path().join(".k2so/agents/scout/SKILL.md"),
            "scout body",
        );
        write_file(
            &ws.path()
                .join(".k2so/agents/scout/heartbeats/daily/WAKEUP.md"),
            "daily scout wakeup",
        );
        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.heartbeats_migrated, 1, "{:?}", out);
        assert!(ws
            .path()
            .join(".k2so/heartbeats/scout-daily/WAKEUP.md")
            .exists());
    }

    #[test]
    fn skips_dotfile_dirs_in_agents_source() {
        // `.k2so/agents/.archive/` must not get promoted into the
        // skill catalog — that's the trash bin for retired sub-agents.
        let ws = ScratchWs::new();
        write_file(
            &ws.path().join(".k2so/agents/.archive/old-thing/AGENT.md"),
            "do not promote",
        );
        write_file(
            &ws.path().join(".k2so/agents/keeper/SKILL.md"),
            "keeper",
        );
        let out = consolidate_skills_v1(ws.path());
        assert_eq!(out.instances_moved, 1);
        // The dotfile dir doesn't end up as a skill.
        assert!(!ws.path().join(".k2so/skills/.archive").exists());
        assert!(ws.path().join(".k2so/skills/keeper/SKILL.md").exists());
    }

    #[test]
    fn resolve_template_dest_with_no_collision_returns_direct() {
        let ws = ScratchWs::new();
        let skills = skills_root(ws.path());
        fs::create_dir_all(&skills).unwrap();
        let (dest, suffixed) = resolve_template_dest(&skills, "alpha");
        assert!(!suffixed);
        assert_eq!(dest, skills.join("alpha"));
    }

    #[test]
    fn resolve_template_dest_with_collision_walks_suffix_counter() {
        let ws = ScratchWs::new();
        let skills = skills_root(ws.path());
        fs::create_dir_all(&skills).unwrap();
        fs::create_dir_all(skills.join("alpha")).unwrap();
        fs::create_dir_all(skills.join("alpha-template01")).unwrap();
        let (dest, suffixed) = resolve_template_dest(&skills, "alpha");
        assert!(suffixed);
        assert_eq!(dest, skills.join("alpha-template02"));
    }
}
