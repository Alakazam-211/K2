//! Workspace SKILL.md regeneration orchestrator.
//!
//! This is the **workspace-level** regen entry point. Renamed from
//! `workspace/skill_writer.rs` → `workspace/skill_regen.rs` in 0.39.0
//! to disambiguate from the lower-level per-harness fanout writer at
//! [`crate::skills::writer`] (which writes individual SKILL.md files
//! to each harness discovery path). This module composes the
//! workspace-level lead-agent body (manager brief from
//! [`crate::skills::content::generate_manager_skill_content`] or AI-
//! planner brief from
//! [`crate::skills::content::generate_k2so_agent_skill_content`]),
//! prepends a per-regen workspace-inbox snapshot, writes the canonical
//! `.k2so/skills/k2so/SKILL.md`, adopts SOURCE-region drift back into
//! `PROJECT.md` / `AGENT.md`, archives pre-existing user-authored
//! files, and stamps content-hash drift baselines.
//!
//! Phase 2.5d: extracted from the monolithic `agents/workspace.rs`.
//! 0.39.0: routed body composition through `skills/content.rs`
//! canonical generators instead of inline templates (fixes A25-verb
//! drift), then renamed for clarity.
//!
//! Sibling [`crate::workspace::harness`] owns the harness file-discovery
//! fan-out (symlink scaffolding, cursor MDC, aider conf merge), and
//! [`crate::workspace::teardown`] owns the disconnect/restore flow.
//! [`crate::workspace::migrations`] hosts the three archive-utility
//! helpers (`archive_claude_md_file`, `inject_first_migration_banner`,
//! `log_adoption_event`) all three modules share.


use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::version::{
    skill_checksum_hex, skill_source_agent_md_begin, skill_source_agent_md_end,
    SKILL_END_MARKER, SKILL_SOURCE_PROJECT_MD_BEGIN, SKILL_SOURCE_PROJECT_MD_END,
    SKILL_VERSION_WORKSPACE,
};
use crate::skills::writer::{
    force_symlink, skill_update_footer, upsert_k2so_section, write_skill_to_all_harnesses,
};
use crate::workspace::agent_identity::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, parse_frontmatter,
};
use crate::workspace::onboarding::harness_fanout_enabled;
use crate::workspace::wake_prompts::strip_frontmatter;
use crate::fs_atomic::{self, atomic_symlink, atomic_write_str, log_if_err};
use crate::log_debug;
use crate::workspace::migrations::{
    archive_claude_md_file, inject_first_migration_banner, log_adoption_event,
};

// ══════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════

/// Sentinel marker users can place in the below-END tail to claim freeform
/// content that should survive regeneration. Anything BETWEEN this marker
/// and EOF is preserved verbatim. Useful for notes the user wants to keep
/// but doesn't want routed into PROJECT.md / AGENT.md.
pub const SKILL_USER_NOTES_SENTINEL: &str = "<!-- K2SO:USER_NOTES -->";

/// The placeholder comment emitted alongside the USER_NOTES sentinel on
/// every regen. Tracked as a constant so `strip_workspace_skill_tail` can
/// discard any existing copies from the preserved freeform — otherwise
/// each regen would stack another placeholder copy onto the tail.
pub const USER_NOTES_PLACEHOLDER: &str =
    "<!-- Content below the K2SO:USER_NOTES sentinel is yours — K2SO preserves it verbatim across regenerations. -->";

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding helpers (private to this module)
// ══════════════════════════════════════════════════════════════════════

/// Generate the workspace-level skill for users working directly with an LLM.
/// Lightweight — just the commands a human user would need when working
/// alongside K2SO agents. All verbs here are pinned to the A25 canonical
/// taxonomy (Phase 2.1); the deprecated-verb snapshot test in
/// `skills/content.rs::tests::DEPRECATED_VERBS` guards against
/// regressions.
fn generate_workspace_skill_content(project_name: &str) -> String {
    format!(
r#"# K2SO Skill

This workspace ({project_name}) is managed by K2SO. You can use these commands to interact with the agent system.

## Send Work to a Workspace

Send a task to a workspace's manager for triage and execution:
```
k2so msg <workspace-name> "live chat — appears in the running session"
k2so msg <workspace-name> --inbox --title "..." --body "..."   # queue (email-style)
```

`msg` (live form) fails loudly when the recipient isn't running — use
`--inbox` to queue a task the recipient reads on their own schedule.

## View Activity Feed

See recent agent activity in this workspace:
```
k2so activity [--limit N] [--workspace <path>]
```

## View Connections

See which workspaces are connected:
```
k2so connections list
```

## Compose a Work Item

Add work to this workspace's inbox for the manager to triage:
```
k2so inbox compose --title "Fix login bug" --body "Users can't log in after password reset"
```

## Heartbeats

The agent in this workspace can have one or more scheduled wakeups. Manage them with:
```
k2so heartbeat                                       # default: list active schedules
k2so heartbeat schedule list [--archived]            # full schedule listing
k2so heartbeat show <name> [--json]                  # schedule + last fire details
k2so heartbeat schedule add --name <n> --daily --time HH:MM
k2so heartbeat signal wakeup <name>                  # print/edit the WAKEUP.md
k2so heartbeat signal fire <name>                    # fire now (skip schedule window)
k2so heartbeat signal wake                           # auto-wake (no name needed)
```

Run `k2so heartbeat --help` for the full surface.
"#,
        project_name = project_name,
    )
}

/// Extract the body between a BEGIN/END marker pair, if both are present.
fn extract_source_region(content: &str, begin: &str, end: &str) -> Option<String> {
    let b_idx = content.find(begin)?;
    let after_begin = b_idx + begin.len();
    let e_rel = content[after_begin..].find(end)?;
    let e_idx = after_begin + e_rel;
    Some(content[after_begin..e_idx].trim().to_string())
}

/// Strip an optional leading heading (`## Something\n\n`) from a SOURCE
/// region body so the comparison / commit targets the raw file content.
fn strip_leading_heading(body: &str) -> String {
    let trimmed = body.trim_start();
    if trimmed.starts_with("## ") {
        if let Some(nl) = trimmed.find('\n') {
            return trimmed[nl + 1..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

/// Return the mtime of a file as seconds since epoch, or 0 if unknown.
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests in
/// `agents/workspace.rs` (which still reference this helper for
/// drift-detection assertions) keep compiling through Tier A.
pub(crate) fn mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Content hash of a file path, suitable for drift detection. Returns an
/// empty string on read failure (callers treat empty == "no stored hash"
/// and fall back to mtime comparison).
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests can call it.
pub(crate) fn content_hash_of(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => skill_checksum_hex(&bytes),
        Err(_) => String::new(),
    }
}

/// Read the `.last-skill-regen` JSON payload, which stores the content
/// hashes of every source file at the time of the last regen. Used by
/// drift adoption to tell "source was edited since last regen" apart
/// from "SKILL.md was edited since last regen" — compared to the old
/// mtime-based heuristic this is immune to clock skew, NTP jumps, and
/// cross-machine rsync mtime quirks.
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests can read the
/// stamp file and assert drift-detection behavior.
pub(crate) fn read_regen_hashes(project_path: &str) -> std::collections::HashMap<String, String> {
    let stamp_path = crate::workspace_dot_dir(project_path).join(".last-skill-regen");
    let Ok(raw) = fs::read_to_string(&stamp_path) else {
        return std::collections::HashMap::new();
    };
    if raw.trim().is_empty() {
        return std::collections::HashMap::new();
    }
    serde_json::from_str::<std::collections::HashMap<String, String>>(&raw).unwrap_or_default()
}

/// Persist the content hashes of every source file that participates in
/// drift detection. Called at the end of a successful regen so the next
/// regen has a baseline for comparison.
fn write_regen_hashes(
    project_path: &str,
    hashes: &std::collections::HashMap<String, String>,
) {
    let stamp_path = crate::workspace_dot_dir(project_path).join(".last-skill-regen");
    let payload = serde_json::to_string(hashes).unwrap_or_else(|_| "{}".to_string());
    log_if_err(
        "write_regen_hashes",
        &stamp_path,
        atomic_write_str(&stamp_path, &payload),
    );
}

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding — public entry points
// ══════════════════════════════════════════════════════════════════════

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding — public entry points
// ══════════════════════════════════════════════════════════════════════

/// Write the workspace-level K2SO skill to all harness locations.
pub fn write_workspace_skill_file(project_path: &str) {
    write_workspace_skill_file_with_body(project_path, None);
}

/// Variant that lets callers pass a pre-composed body so that content
/// lands in the canonical SKILL.md rather than being lost when CLAUDE.md
/// collapsed to a symlink.
pub fn write_workspace_skill_file_with_body(project_path: &str, base_body: Option<&str>) {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let regen_marker = crate::workspace_dot_dir(project_path).join(".regen-in-flight");
    log_if_err(
        "regen-in-flight stamp",
        &regen_marker,
        fs_atomic::atomic_write(&regen_marker, b""),
    );

    // Step 1: Adoption sweep
    adopt_workspace_skill_drift(project_path);

    // Step 2: Clear stale tail
    let preserved_freeform = strip_workspace_skill_tail(project_path);

    // Step 3: Compose K2SO-managed body
    let mut managed_body = match base_body {
        Some(body) => body.to_string(),
        None => generate_workspace_skill_content(&project_name),
    };

    let primary_agent = find_primary_agent(project_path);
    if !managed_body.ends_with('\n') {
        managed_body.push('\n');
    }
    managed_body.push('\n');
    managed_body.push_str(&skill_update_footer(project_path, primary_agent.as_deref()));

    // Step 4: Write managed body
    write_skill_to_all_harnesses(
        project_path,
        "k2so",
        "workspace",
        SKILL_VERSION_WORKSPACE,
        "K2SO workspace context — CLI reference + project context + primary agent persona",
        &managed_body,
        false,
    );

    // Step 5: Append fresh SOURCE regions
    append_workspace_source_regions(project_path, preserved_freeform.as_deref());

    // Steps 6 + 7: fan out — gated OFF BY DEFAULT (canonical-agents
    // feature). The root SKILL/CLAUDE.md symlinks, AGENTS.md /
    // copilot-instructions.md marker injection, and the harness
    // discovery-target fan-out only run when the per-workspace opt-in
    // marker is present. The canonical `.k2so/skills/k2so/SKILL.md`
    // (written in Step 4 above) always generates regardless — only the
    // user-visible fan-out below is gated. Legacy
    // `.skip-harness-management` still forces this off (inside
    // `harness_fanout_enabled`).
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if harness_fanout_enabled(project_path) {
        if let Ok(full) = fs::read_to_string(&canonical) {
            let injection_body = strip_frontmatter(&full).trim().to_string();
            let root = PathBuf::from(project_path);
            upsert_k2so_section(&root.join("AGENTS.md"), &injection_body);
            let github_dir = root.join(".github");
            let _ = fs::create_dir_all(&github_dir);
            upsert_k2so_section(&github_dir.join("copilot-instructions.md"), &injection_body);
        }

        let root_skill = PathBuf::from(project_path).join("SKILL.md");
        force_symlink(&canonical, &root_skill);
        let root_claude = PathBuf::from(project_path).join("CLAUDE.md");
        migrate_and_symlink_root_claude_md(&canonical, &root_claude, project_path);
        crate::workspace::harness::write_workspace_harness_discovery_targets(project_path, &canonical);
    }

    // Step 8: Stamp last-regen hashes
    let mut hashes: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let project_md_path = crate::workspace_dot_dir(project_path).join("PROJECT.md");
    let project_hash = content_hash_of(&project_md_path);
    if !project_hash.is_empty() {
        hashes.insert("project_md".to_string(), project_hash);
    }
    if let Some(primary) = find_primary_agent(project_path) {
        let agent_md_path = agent_dir(project_path, &primary).join("AGENT.md");
        let agent_hash = content_hash_of(&agent_md_path);
        if !agent_hash.is_empty() {
            hashes.insert(format!("agent_md::{}", primary), agent_hash);
        }
    }
    write_regen_hashes(project_path, &hashes);

    log_if_err(
        "regen-in-flight clear",
        &regen_marker,
        fs::remove_file(&regen_marker),
    );
}

/// Walk the existing canonical SKILL.md and adopt any SOURCE-region drift
/// back into its canonical source file (PROJECT.md or the primary agent's
/// AGENT.md).
fn adopt_workspace_skill_drift(project_path: &str) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(skill_content) = fs::read_to_string(&canonical) else {
        return;
    };
    let stamp_path = crate::workspace_dot_dir(project_path).join(".last-skill-regen");
    let last_regen = mtime_secs(&stamp_path);
    let stored_hashes = read_regen_hashes(project_path);

    let source_touched_since_regen = |source_path: &Path, key: &str| -> bool {
        if let Some(stored) = stored_hashes.get(key) {
            let current = content_hash_of(source_path);
            !current.is_empty() && current.as_str() != stored.as_str()
        } else {
            mtime_secs(source_path) > last_regen
        }
    };

    // PROJECT.md adoption
    if let Some(region_body) = extract_source_region(
        &skill_content,
        SKILL_SOURCE_PROJECT_MD_BEGIN,
        SKILL_SOURCE_PROJECT_MD_END,
    ) {
        let project_md = crate::workspace_dot_dir(project_path).join("PROJECT.md");
        let region_stripped = strip_leading_heading(&region_body);
        let file_body = fs::read_to_string(&project_md)
            .map(|raw| strip_frontmatter(&raw).trim().to_string())
            .unwrap_or_default();
        if region_stripped.trim() != file_body.trim() {
            if source_touched_since_regen(&project_md, "project_md") {
                log_adoption_event(
                    project_path,
                    "PROJECT.md: user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                );
            } else if !region_stripped.trim().is_empty() {
                let frontmatter = if let Ok(raw) = fs::read_to_string(&project_md) {
                    if raw.starts_with("---") {
                        if let Some(end) = raw[3..].find("---") {
                            Some(raw[..3 + end + 3].to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let new_contents = match frontmatter {
                    Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                    None => format!("{}\n", region_stripped.trim()),
                };
                match atomic_write_str(&project_md, &new_contents) {
                    Ok(()) => log_adoption_event(
                        project_path,
                        "ADOPTED PROJECT.md: SKILL.md SOURCE region committed back to .k2so/PROJECT.md",
                    ),
                    Err(e) => log_if_err::<(), _>("adopt PROJECT.md", &project_md, Err::<(), _>(e)),
                }
            }
        }
    }

    // Primary agent's AGENT.md adoption
    if let Some(primary_agent) = find_primary_agent(project_path) {
        let agent_type = agent_type_for(project_path, &primary_agent);
        let include_primary = matches!(
            agent_type.as_str(),
            "custom" | "k2so" | "manager" | "coordinator" | "pod-leader"
        );
        if include_primary {
            let begin = skill_source_agent_md_begin(&primary_agent);
            let end = skill_source_agent_md_end(&primary_agent);
            if let Some(region_body) = extract_source_region(&skill_content, &begin, &end) {
                let agent_md = agent_dir(project_path, &primary_agent).join("AGENT.md");
                let region_stripped = strip_leading_heading(&region_body);
                let file_body = fs::read_to_string(&agent_md)
                    .map(|raw| strip_frontmatter(&raw).trim().to_string())
                    .unwrap_or_default();
                if region_stripped.trim() != file_body.trim() {
                    let key = format!("agent_md::{}", primary_agent);
                    if source_touched_since_regen(&agent_md, &key) {
                        log_adoption_event(
                            project_path,
                            &format!(
                                "AGENT.md ({}): user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                                primary_agent
                            ),
                        );
                    } else if !region_stripped.trim().is_empty() {
                        let frontmatter = if let Ok(raw) = fs::read_to_string(&agent_md) {
                            if raw.starts_with("---") {
                                if let Some(end) = raw[3..].find("---") {
                                    Some(raw[..3 + end + 3].to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let new_contents = match frontmatter {
                            Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                            None => format!("{}\n", region_stripped.trim()),
                        };
                        match atomic_write_str(&agent_md, &new_contents) {
                            Ok(()) => log_adoption_event(
                                project_path,
                                &format!(
                                    "ADOPTED AGENT.md ({}): SKILL.md SOURCE region committed back to agent file",
                                    primary_agent
                                ),
                            ),
                            Err(e) => log_if_err::<(), _>(
                                "adopt AGENT.md",
                                &agent_md,
                                Err::<(), _>(e),
                            ),
                        }
                    }
                }
            }
        }
    }
}

/// Remove everything after the MANAGED:END marker in the canonical SKILL.md.
/// Returns any truly user-authored content found after the LAST
/// `<!-- K2SO:USER_NOTES -->` sentinel so it can be re-appended after
/// regeneration.
pub fn strip_workspace_skill_tail(project_path: &str) -> Option<String> {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(content) = fs::read_to_string(&canonical) else { return None };
    let end_idx = content.find(SKILL_END_MARKER)?;
    let after_end_start = end_idx + SKILL_END_MARKER.len();
    let tail = &content[after_end_start..];

    let preserved = tail.rfind(SKILL_USER_NOTES_SENTINEL).map(|idx| {
        let after = idx + SKILL_USER_NOTES_SENTINEL.len();
        tail[after..].to_string()
    });

    let truncated = format!("{}\n", &content[..after_end_start]);
    log_if_err(
        "strip_workspace_skill_tail write",
        &canonical,
        atomic_write_str(&canonical, &truncated),
    );

    let preserved = preserved.map(|raw| {
        raw.lines()
            .filter(|l| l.trim() != USER_NOTES_PLACEHOLDER.trim())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    });
    preserved.filter(|s| !s.is_empty())
}

/// After the managed region has been re-written, append fresh SOURCE
/// sub-regions below the END marker in the canonical file.
pub fn append_workspace_source_regions(project_path: &str, preserved_freeform: Option<&str>) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(mut content) = fs::read_to_string(&canonical) else { return };
    if !content.ends_with('\n') {
        content.push('\n');
    }

    let project_md = crate::workspace_dot_dir(project_path).join("PROJECT.md");
    if let Ok(raw) = fs::read_to_string(&project_md) {
        let stripped = strip_frontmatter(&raw);
        let has_content = stripped.lines().any(|line| {
            let t = line.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("<!--")
        });
        if has_content {
            content.push_str(&format!(
                "\n{begin}\n## Project Context\n\n{body}\n{end}\n",
                begin = SKILL_SOURCE_PROJECT_MD_BEGIN,
                body = stripped.trim(),
                end = SKILL_SOURCE_PROJECT_MD_END,
            ));
        }
    }

    if let Some(primary_agent) = find_primary_agent(project_path) {
        let agent_type = agent_type_for(project_path, &primary_agent);
        let include_primary = matches!(
            agent_type.as_str(),
            "custom" | "k2so" | "manager" | "coordinator" | "pod-leader"
        );
        if include_primary {
            let agent_md = agent_dir(project_path, &primary_agent).join("AGENT.md");
            if let Ok(raw) = fs::read_to_string(&agent_md) {
                let stripped = strip_frontmatter(&raw).trim().to_string();
                if !stripped.is_empty() {
                    content.push_str(&format!(
                        "\n{begin}\n## Primary Agent: {name}\n\n{body}\n{end}\n",
                        begin = skill_source_agent_md_begin(&primary_agent),
                        name = primary_agent,
                        body = stripped,
                        end = skill_source_agent_md_end(&primary_agent),
                    ));
                }
            }
        }
    }

    content.push_str(&format!(
        "\n{sentinel}\n{placeholder}\n",
        sentinel = SKILL_USER_NOTES_SENTINEL,
        placeholder = USER_NOTES_PLACEHOLDER,
    ));
    if let Some(freeform) = preserved_freeform {
        let cleaned = freeform.trim();
        if !cleaned.is_empty() {
            content.push('\n');
            content.push_str(cleaned);
            content.push('\n');
        }
    }

    log_if_err(
        "append_workspace_source_regions",
        &canonical,
        atomic_write_str(&canonical, &content),
    );
}

/// CLAUDE.md migration helper for the 0.32.7 transition.
///
/// Phase 2.5d: `pub(crate)` so the sibling [`crate::workspace::harness`]
/// module (and the agents/workspace.rs shim during Tier A) can call it.
pub(crate) fn migrate_and_symlink_root_claude_md(canonical: &Path, root_claude: &Path, project_path: &str) {
    match fs::symlink_metadata(root_claude) {
        Ok(meta) if meta.file_type().is_symlink() => {
            force_symlink(canonical, root_claude);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(root_claude).unwrap_or_default();
            let is_k2so_generated = content.starts_with("# K2SO ");
            let archived = archive_claude_md_file(project_path, root_claude, "CLAUDE.md");
            let source_label = if is_k2so_generated {
                "pre-0.32.7 K2SO-generated CLAUDE.md"
            } else {
                "pre-existing user-authored CLAUDE.md"
            };
            let archive_display = archived
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(archive unavailable)".to_string());
            if !content.trim().is_empty() {
                import_claude_md_into_user_notes(
                    project_path,
                    &content,
                    source_label,
                    &archive_display,
                );
            }
            log_if_err(
                "migrate_and_symlink_root_claude_md",
                root_claude,
                atomic_symlink(canonical, root_claude),
            );
            if let Some(archive_path) = archived {
                inject_first_migration_banner(project_path, &[archive_path]);
            }
            log_debug!(
                "[workspace-skill] Migrated {}./CLAUDE.md → archived + imported body into SKILL.md USER_NOTES; root CLAUDE.md now symlinks to canonical SKILL.md",
                if is_k2so_generated {
                    "K2SO-generated "
                } else {
                    "user-authored "
                },
            );
        }
        _ => {
            force_symlink(canonical, root_claude);
        }
    }
}

/// Append the body of a pre-existing CLAUDE.md into the canonical
/// SKILL.md's USER_NOTES region so the migrated content stays visible
/// to Claude Code (via the symlink) without requiring the user to hand-
/// merge from `.k2so/migration/`.
///
/// Phase 2.5d: `pub(crate)` so the sibling [`crate::workspace::harness`]
/// module can call it when it scaffolds harness discovery targets.
pub(crate) fn import_claude_md_into_user_notes(
    project_path: &str,
    body: &str,
    source_label: &str,
    archive_display: &str,
) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if !canonical.exists() {
        return;
    }
    let Ok(existing) = fs::read_to_string(&canonical) else { return };

    let import_sentinel = format!(
        "<!-- K2SO:IMPORT:CLAUDE_MD archive={} -->",
        archive_display
    );
    if existing.contains(&import_sentinel) {
        return;
    }

    let Some(sentinel_idx) = existing.find(SKILL_USER_NOTES_SENTINEL) else {
        let import_block = format!(
            "\n\n{sentinel}\n## Imported: {label}\n\n> Archived at `{archive}`. You can prune this section once reviewed; K2SO preserves anything below the `K2SO:USER_NOTES` sentinel verbatim.\n\n{body}\n",
            sentinel = import_sentinel,
            label = source_label,
            archive = archive_display,
            body = body.trim(),
        );
        let mut out = existing;
        out.push_str(&import_block);
        log_if_err(
            "import_claude_md fallback append",
            &canonical,
            atomic_write_str(&canonical, &out),
        );
        return;
    };
    let insertion_anchor = existing[sentinel_idx..]
        .find(USER_NOTES_PLACEHOLDER)
        .map(|rel| sentinel_idx + rel + USER_NOTES_PLACEHOLDER.len())
        .unwrap_or(sentinel_idx + SKILL_USER_NOTES_SENTINEL.len());
    let import_block = format!(
        "\n\n{sentinel}\n## Imported: {label}\n\n> Archived at `{archive}`. You can prune this section once reviewed; K2SO preserves anything below the `K2SO:USER_NOTES` sentinel verbatim across regenerations.\n\n{body}\n",
        sentinel = import_sentinel,
        label = source_label,
        archive = archive_display,
        body = body.trim(),
    );
    let mut out = String::with_capacity(existing.len() + import_block.len());
    out.push_str(&existing[..insertion_anchor]);
    out.push_str(&import_block);
    out.push_str(&existing[insertion_anchor..]);
    log_if_err(
        "import_claude_md_into_user_notes",
        &canonical,
        atomic_write_str(&canonical, &out),
    );
    log_adoption_event(
        project_path,
        &format!(
            "IMPORTED {} body into SKILL.md USER_NOTES (archive: {})",
            source_label, archive_display
        ),
    );
}

/// Universal skill refresh. Walks every agent folder + the workspace
/// skill and re-invokes the regular write_* functions. Because those now
/// route through ensure_skill_up_to_date, this is idempotent.
pub fn ensure_all_skills_up_to_date(project_path: &str) {
    write_workspace_skill_file(project_path);

    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(&agents_root) else { return };
    for entry in entries.flatten() {
        let agent_path = entry.path();
        if !agent_path.is_dir() {
            continue;
        }
        let name_osstr = entry.file_name();
        let agent_name = name_osstr.to_string_lossy();
        if agent_name.starts_with('.') {
            continue;
        }

        let agent_md = agent_path.join("AGENT.md");
        if !agent_md.exists() {
            continue;
        }
        let agent_content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&agent_content);
        let agent_type = fm.get("type").cloned().unwrap_or_else(|| "agent-template".to_string());
        let normalized_type = match agent_type.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            other => other.to_string(),
        };
        crate::skills::writer::write_agent_skill_file(
            project_path,
            &agent_name,
            &normalized_type,
        );
    }
}

// ══════════════════════════════════════════════════════════════════════
// Workspace SKILL.md regen — public entry point
// ══════════════════════════════════════════════════════════════════════

// is unchanged.

// ── Canonical-generator delegation (post-Phase-2.5d cleanup) ────────────
//
// The previous CLI_TOOLS_DOCS + WORKFLOW_DOCS constants + the inline
// manager-mode / AI-planner CLAUDE.md templates that lived here baked
// the pre-A25 verb taxonomy (`k2so delegate`, `k2so work create`,
// `k2so agents create`, `k2so heartbeat wake`, `k2so agent complete`,
// …) into every regenerated SKILL.md. Phase 2.1 retired those verbs
// in favor of the A25 surface (`k2so skills *`, `k2so workspace *`,
// `k2so inbox compose`, `k2so msg --inbox`, `k2so heartbeat signal
// wake`). Phase 2.5d moved the canonical bodies into
// `skills/content.rs` and pinned them with the Tier 2.2
// `assert_no_deprecated_verbs` snapshot tests; the body of
// `regenerate_workspace_skill` below now calls those generators
// directly instead of re-implementing them inline.

/// Regenerate the workspace-root SKILL.md — the lead agent's complete
/// operating manual. Written to `<project-root>/SKILL.md` with a
/// matching `<project-root>/CLAUDE.md` symlink so Claude Code auto-
/// discovers it. The SKILL.md is the canonical source of truth;
/// CLAUDE.md is a harness-specific entry point.
///
/// Also auto-scaffolds the `.k2so/` layout on first call (manager +
/// k2so-agent dirs, inbox/active/done folders, prds/, PROJECT.md).
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_regenerate_workspace_skill`
/// so the daemon can run this directly. The Tauri-side wrapper now
/// forwards here.
///
/// Returns the composed CLAUDE.md body that was injected into the
/// canonical SKILL.md — the renderer's regen UIs use this for preview.
pub fn regenerate_workspace_skill(project_path: String) -> Result<String, String> {
    use crate::skills::writer::{generate_default_agent_body, write_agent_skill_file};

    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    // Scaffold .k2so/ structure if it doesn't exist.
    // Post-Phase-2.1: the workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2_core::inbox::*` primitive). The legacy
    // `.k2so/work/inbox/` was retired and the first-boot daemon hook
    // trashes any straggler.
    let k2so_dir = crate::workspace_dot_dir(&project_path);
    let _ = fs::create_dir_all(k2so_dir.join("inbox"));
    let _ = fs::create_dir_all(k2so_dir.join("prds"));

    // 0.37.0 unification check: if the workspace has been migrated to
    // the single-agent layout (`.k2so/agent/AGENT.md` exists OR the
    // unification sentinel is stamped), the legacy auto-scaffold of
    // `.k2so/agents/manager/` and `.k2so/agents/k2so-agent/` is a
    // regression — it'd repopulate the directory tree the migration
    // just retired and re-create files at paths the runtime no longer
    // reads. Skip the legacy scaffold entirely when migrated.
    //
    // 0.39.0: when we DO scaffold, target the post-2.5b unified
    // layout (`.k2so/skills/<name>/`) instead of the pre-2.5b
    // `.k2so/agents/<name>/` tree. Pre-fix, this function created
    // `.k2so/agents/manager/` + `.k2so/agents/k2so-agent/` on first
    // call for a fresh workspace — a layout the runtime no longer
    // reads after the consolidator runs at first boot.
    let unification_sentinel = k2so_dir.join(".unification-0.37.0-done");
    let unified_agent_dir = k2so_dir.join("agent");
    let post_unification = unification_sentinel.exists() || unified_agent_dir.exists();
    if post_unification {
        // Don't recreate `.k2so/skills/` either — the post-migration
        // layout uses `.k2so/agent/` (singular) and
        // `.k2so/agent-templates/<n>/`. Skip straight to PROJECT.md +
        // workspace SKILL writes below.
    } else {
        let _ = fs::create_dir_all(k2so_dir.join("skills"));
    }

    // Auto-create manager skill if it doesn't exist (pre-unification only).
    // Check for old "pod-leader" and "coordinator" directory names as fallback.
    let manager_dir = k2so_dir.join("skills").join("manager");
    let legacy_coordinator_dir = k2so_dir.join("skills").join("coordinator");
    let legacy_pod_leader_dir = k2so_dir.join("skills").join("pod-leader");
    if !post_unification
        && !manager_dir.exists()
        && !legacy_coordinator_dir.exists()
        && !legacy_pod_leader_dir.exists()
    {
        let _ = fs::create_dir_all(&manager_dir);
        // 0.39.0: work/ subdirs are workspace-level under `.k2so/inbox/`
        // (the unified `k2_core::inbox::*` primitive) post-Phase-2.1.
        // The legacy per-agent `<dir>/work/{inbox,active,done}/` layout
        // was retired in 0.37.0 unification — don't recreate it.
        let manager_role = "Workspace Manager — coordinates with connected workspace-agents, reviews their branches, drives milestones";
        let manager_body =
            generate_default_agent_body("manager", "manager", manager_role, &project_path);
        let manager_md = format!(
            "---\nname: manager\nrole: {}\ntype: manager\nmanager: true\n---\n\n{}\n",
            manager_role, manager_body
        );
        let manager_md_path = manager_dir.join("AGENT.md");
        log_if_err(
            "auto-scaffold manager AGENT.md",
            &manager_md_path,
            atomic_write_str(&manager_md_path, &manager_md),
        );
        write_agent_skill_file(&project_path, "manager", "manager");
    }

    // Auto-create K2SO agent if it doesn't exist (pre-unification only).
    // Post-0.37.0 the workspace agent lives at .k2so/agent/, not
    // .k2so/skills/k2so-agent/.
    let k2so_agent_dir = k2so_dir.join("skills").join("k2so-agent");
    if !post_unification && !k2so_agent_dir.exists() {
        let _ = fs::create_dir_all(&k2so_agent_dir);
        let k2so_role = "K2SO planner — builds PRDs, milestones, and technical plans";
        let k2so_body =
            generate_default_agent_body("k2so", "k2so-agent", k2so_role, &project_path);
        let k2so_md = format!(
            "---\nname: k2so-agent\nrole: {}\ntype: k2so\n---\n\n{}\n",
            k2so_role, k2so_body
        );
        let k2so_md_path = k2so_agent_dir.join("AGENT.md");
        log_if_err(
            "auto-scaffold k2so-agent AGENT.md",
            &k2so_md_path,
            atomic_write_str(&k2so_md_path, &k2so_md),
        );
        write_agent_skill_file(&project_path, "k2so-agent", "k2so");
    }

    // List workspace inbox items for the per-regen "current context"
    // header. The canonical generator (`generate_manager_skill_content`)
    // walks `.k2so/agents/` to build the team roster + emits the full
    // CLI surface; we only need the inbox snapshot here because that's
    // the one piece of per-regen state the generator doesn't bake in.
    //
    // Post-Phase-2.1: workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2_core::inbox::*` primitive). Root-level items
    // (`folder == ""`) are the untriaged arrivals; sub-foldered items
    // have already been organized and aren't surfaced in the manager's
    // skill summary.
    let mut inbox_summary = String::new();
    let ws_inbox_items =
        crate::inbox::list_folder(std::path::Path::new(&project_path), "");
    for item in &ws_inbox_items {
        // `type` dropped in the WorkItem → InboxItem migration; `source`
        // (the WorkItem `source` field) is preserved on InboxItem and
        // does double duty here.
        inbox_summary.push_str(&format!(
            "- **{}** (priority: {}, source: {})\n",
            item.title, item.priority, item.source
        ));
    }

    // Detect mode — read from DB, fall back to filesystem
    let is_manager_mode = {
        // Try reading from DB first — shared process-wide connection.
        let db_mode: Option<String> = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT agent_mode FROM projects WHERE path = ?1",
                rusqlite::params![project_path],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };

        match db_mode.as_deref() {
            Some("manager") | Some("coordinator") | Some("pod") => true,
            Some("agent") => false,
            _ => {
                // Fallback heuristic when the DB doesn't carry an
                // explicit `agent_mode`: presence of skill/role
                // directories suggests this workspace acts as a
                // manager.
                //
                // **Dual-probe is intentional, not legacy cruft.**
                // We check BOTH `.k2so/skills/` (post-Phase-2.5b
                // unified home for skill profiles) AND
                // `.k2so/agents/` (pre-2.5b per-agent directory tree)
                // because workspaces caught mid-migration during the
                // Phase 2.5b consolidation window can have entries in
                // either tree (the migration sweep runs on daemon
                // boot per workspace; a workspace that hasn't been
                // touched since the daemon last booted may still
                // expose only the legacy shape). Once every
                // registered workspace has booted under a post-2.5b
                // daemon, the `agents_dir` arm becomes dead code —
                // but until then, removing it would flip false
                // negatives on unmigrated workspaces. Leave both
                // probes in place during the migration window.
                let has_subagents = |root: &PathBuf| -> bool {
                    root.exists()
                        && fs::read_dir(root)
                            .map(|e| {
                                e.flatten().any(|e| {
                                    let Ok(ft) = e.file_type() else { return false };
                                    if !ft.is_dir() {
                                        return false;
                                    }
                                    let name = e.file_name().to_string_lossy().to_string();
                                    // The workspace's own primary skill (`k2so`)
                                    // isn't a peer agent; its presence shouldn't
                                    // flip manager-mode on.
                                    !name.starts_with('.') && name != "k2so"
                                })
                            })
                            .unwrap_or(false)
                };
                let skills_root = k2so_dir.join("skills");
                has_subagents(&skills_root) || has_subagents(&agents_dir(&project_path))
            }
        }
    };

    // Scaffold PROJECT.md for manager mode — shared context across all agents
    if is_manager_mode {
        let project_md_path = k2so_dir.join("PROJECT.md");
        if !project_md_path.exists() {
            let project_md_content = format!(
r#"# {project_name}

<!--
  PROJECT.md is the "what" half of agent context — the codebase facts
  every agent needs regardless of role. K2SO ships this file as part of
  the agent's system prompt on every launch, via --append-system-prompt
  (injected alongside SKILL.md as a "Project Context (shared)" section).
  You don't need to reference it from wakeup.md — it's always there.

  Pair it with Agent Skills (SKILL.md layers) which cover the "how":
    PROJECT.md = what this project IS (tech stack, conventions)
    SKILL.md   = what the agent DOES (standing orders, procedures)

  Edit this file directly or via Settings → Projects → "Manage Project
  Context". Applies to Workspace Manager and Agent Template agents.
  Custom Agents don't receive PROJECT.md by design — they may not be
  codebase-scoped.

  Delete these comments once you've filled the sections in.
-->

## About This Project

<!-- What does this codebase do? What problem does it solve? -->

## Tech Stack

<!-- Languages, frameworks, databases, infrastructure. Include versions
     where they matter (e.g. "Tauri v2, React 19, TailwindCSS v4"). -->

## Key Directories

<!-- Important paths and what lives in them. Call out where tests live,
     where generated files go, where NOT to edit. -->

## Conventions

<!-- Code style, commit message format, PR process, branch naming.
     Anything an engineer would otherwise have to discover by osmosis. -->

## External Systems

<!-- Links to issue trackers, CI dashboards, staging environments, docs.
     If the project depends on an external service the agent may need to
     know about or call, document it here. -->
"#,
                project_name = project_name,
            );
            let _ = crate::workspace::work_item::atomic_write(&project_md_path, &project_md_content);
        }
    }

    // Build the per-regen inbox snapshot header. This is the only piece
    // of per-call workspace state we prepend to the canonical body —
    // the rest (CLI surface, team roster, standing orders, decision
    // framework, …) lives in the canonical generators in
    // `skills/content.rs`, which are pinned by the Tier 2.2 snapshot
    // tests to use only A25 canonical verbs.
    let inbox_section = if inbox_summary.is_empty() {
        if is_manager_mode {
            "*Workspace inbox is empty. Waiting for tasks from the AI Planner or user.*".to_string()
        } else {
            "No items in the workspace inbox.".to_string()
        }
    } else {
        format!("### Current Inbox\n{}", inbox_summary)
    };

    // Delegate to the canonical tier generator instead of inlining a
    // duplicate template. Phase 2.1 retired the legacy verb taxonomy
    // (`k2so delegate`, `k2so work create`, `k2so agents create`,
    // `k2so heartbeat wake`, `k2so agent complete`, …); the canonical
    // generators in `skills/content.rs` emit only A25 verbs.
    let canonical_body = if is_manager_mode {
        crate::skills::content::generate_manager_skill_content(&project_path, &project_name)
    } else {
        // Non-manager workspace = AI Planner role (the comprehensive
        // top-tier autonomous variant). `generate_k2so_agent_skill_content`
        // emits the planner-focused brief including PRD/milestone
        // workflow, cross-workspace messaging, heartbeat scheduling,
        // and review-queue triage.
        let planner_name = find_primary_agent(&project_path)
            .unwrap_or_else(|| "k2so-agent".to_string());
        crate::skills::content::generate_k2so_agent_skill_content(&project_name, &planner_name)
    };

    // Prepend a per-regen "Workspace Inbox" header so the manager /
    // planner has the live triage queue visible without having to
    // re-run `k2so inbox`. The canonical body follows.
    let md = format!(
        "## Workspace Inbox\n\n{inbox_section}\n\n---\n\n{canonical_body}",
        inbox_section = inbox_section,
        canonical_body = canonical_body,
    );

    // As of 0.32.7: the rich workspace-level content (manager brief or AI
    // planner brief + agent list + inbox summary + CLI tools docs) now
    // flows into the canonical SKILL.md instead of a separate ./CLAUDE.md
    // file. `write_workspace_skill_file_with_body` takes the composed `md`
    // as the base body, appends `.k2so/PROJECT.md` body + primary agent's
    // `agent.md` body, writes the canonical at `.k2so/skills/k2so/SKILL.md`,
    // and fans it out via symlinks to every harness discovery path
    // (`./CLAUDE.md`, `./SKILL.md`, `./GEMINI.md`, `./AGENT.md`,
    // `./.goosehints`, `./.claude/skills/k2so/SKILL.md`, etc.).
    //
    // Existing `./CLAUDE.md` files: migrated to `.k2so/CLAUDE.md.migrated` if
    // K2SO-generated, preserved as-is if user-authored (see
    // migrate_and_symlink_root_claude_md).
    write_workspace_skill_file_with_body(&project_path, Some(&md));

    // Clean up the stale `.k2so/CLAUDE.md.disabled` artifact from the
    // pre-symlink era — the disable flow is now "symlink goes away when the
    // workspace is off", not a file rename.
    let disabled_path = crate::workspace_dot_dir(&project_path).join("CLAUDE.md.disabled");
    if disabled_path.exists() {
        let _ = fs::remove_file(&disabled_path);
    }

    Ok(md)
}

#[cfg(test)]
mod tests {
    //! Phase 2 Tier 2.1 coverage for skill_regen entry points that the
    //! migrations.rs::migration_safety_tests block doesn't already
    //! exercise. The migration tests cover the safe-symlink contract,
    //! the import-into-user-notes flow, and strip_workspace_skill_tail
    //! happy paths; here we add a few gap-fillers:
    //!
    //! - strip_workspace_skill_tail returns None when no SKILL.md exists
    //!   at all (vs the existing tests which assume the file exists)
    //! - ensure_all_skills_up_to_date is a no-op for workspaces with no
    //!   agents dir
    //! - regenerate_workspace_skill scaffolds .k2so/inbox + .k2so/prds
    //!   and returns the composed CLAUDE.md body
    //! - append_workspace_source_regions writes the USER_NOTES sentinel
    //!   + placeholder even when no preserved content is passed
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn scratch_project() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-skill-regen-test-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        fs::create_dir_all(dir.join(".k2so/skills/k2so")).unwrap();
        dir
    }

    #[test]
    fn strip_workspace_skill_tail_returns_none_when_canonical_missing() {
        let proj = scratch_project();
        // Don't write a SKILL.md — the canonical doesn't exist.
        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());
        assert!(
            preserved.is_none(),
            "no canonical SKILL.md → no preserved freeform, got {preserved:?}",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn append_workspace_source_regions_writes_user_notes_sentinel_for_clean_workspace() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");

        // Seed a minimal canonical body (no SOURCE regions yet).
        let seed = format!(
            "---\nk2so_skill: workspace\n---\n\n{}\nManaged\n{}\n",
            crate::skills::version::SKILL_BEGIN_MARKER,
            crate::skills::version::SKILL_END_MARKER,
        );
        fs::write(&canonical, &seed).unwrap();

        append_workspace_source_regions(proj.to_str().unwrap(), None);

        let after = fs::read_to_string(&canonical).unwrap();
        assert!(
            after.contains(SKILL_USER_NOTES_SENTINEL),
            "append must write the USER_NOTES sentinel; got: {after:?}",
        );
        assert!(
            after.contains(USER_NOTES_PLACEHOLDER),
            "append must write the placeholder comment under the sentinel",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn append_workspace_source_regions_preserves_passed_freeform_below_sentinel() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let seed = format!(
            "---\nk2so_skill: workspace\n---\n\n{}\nManaged\n{}\n",
            crate::skills::version::SKILL_BEGIN_MARKER,
            crate::skills::version::SKILL_END_MARKER,
        );
        fs::write(&canonical, &seed).unwrap();

        append_workspace_source_regions(
            proj.to_str().unwrap(),
            Some("# my custom note\n\nThis is a private thought."),
        );

        let after = fs::read_to_string(&canonical).unwrap();
        let sentinel_pos = after.find(SKILL_USER_NOTES_SENTINEL).unwrap();
        let user_pos = after.find("This is a private thought.").unwrap();
        assert!(
            user_pos > sentinel_pos,
            "user freeform must appear BELOW the USER_NOTES sentinel (sentinel at {sentinel_pos}, user at {user_pos})",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn ensure_all_skills_up_to_date_handles_workspace_with_no_agents_dir() {
        let proj = scratch_project();
        // Don't create .k2so/agents/ — the function should write the
        // workspace skill and return without panicking.
        ensure_all_skills_up_to_date(proj.to_str().unwrap());

        // Sanity: canonical SKILL.md exists post-call (write_workspace_skill_file ran).
        assert!(
            proj.join(".k2so/skills/k2so/SKILL.md").exists(),
            "ensure_all_skills_up_to_date should still write the workspace skill",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_workspace_skill_scaffolds_inbox_dir_and_returns_body() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        let body = regenerate_workspace_skill(path).expect("regen ok");

        // Body must be non-empty and look like a project-scoped brief.
        assert!(!body.is_empty(), "regen body should not be empty");

        // Phase 2.1: .k2so/inbox/ (unified primitive) and .k2so/prds/
        // must be scaffolded.
        assert!(proj.join(".k2so/inbox").is_dir(), ".k2so/inbox/ should be scaffolded");
        assert!(proj.join(".k2so/prds").is_dir(), ".k2so/prds/ should be scaffolded");

        // Canonical SKILL.md must have landed.
        assert!(
            proj.join(".k2so/skills/k2so/SKILL.md").exists(),
            "canonical SKILL.md should exist after regen",
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_workspace_skill_auto_scaffold_targets_skills_not_agents() {
        // 0.39.0: pre-fix, this function scaffolded `.k2so/agents/manager/`
        // and `.k2so/agents/k2so-agent/` on first call for a fresh
        // workspace — a layout the runtime no longer reads post-2.5b
        // consolidation. The fix re-targets the unified
        // `.k2so/skills/<name>/` home so the auto-scaffolded files
        // actually feed downstream regen + discovery.
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        let _ = regenerate_workspace_skill(path).expect("regen ok");

        // The manager + k2so-agent auto-scaffold must land under the
        // unified `.k2so/skills/` home.
        assert!(
            proj.join(".k2so/skills/manager/AGENT.md").exists(),
            ".k2so/skills/manager/AGENT.md should be auto-scaffolded",
        );
        assert!(
            proj.join(".k2so/skills/k2so-agent/AGENT.md").exists(),
            ".k2so/skills/k2so-agent/AGENT.md should be auto-scaffolded",
        );

        // And critically: the pre-fix legacy `.k2so/agents/` path
        // must NOT be re-created.
        assert!(
            !proj.join(".k2so/agents/manager").exists(),
            "regen must NOT re-create the retired .k2so/agents/manager/ path",
        );
        assert!(
            !proj.join(".k2so/agents/k2so-agent").exists(),
            "regen must NOT re-create the retired .k2so/agents/k2so-agent/ path",
        );

        fs::remove_dir_all(&proj).ok();
    }

    // ── Tier 2.5d cleanup: no deprecated verbs in regen output ─────
    //
    // The previous CLI_TOOLS_DOCS + inline manager/AI-planner templates
    // baked pre-A25 verbs (`k2so delegate`, `k2so work create`,
    // `k2so agents create`, `k2so heartbeat wake`, `k2so agent complete`,
    // …) into every workspace SKILL.md K2SO emitted. The fix routes
    // through `skills/content.rs` canonical generators — these tests
    // pin the property end-to-end through `regenerate_workspace_skill`
    // so a future inline-template regression breaks the build.
    //
    // Kept in lockstep with `skills::content::tests::DEPRECATED_VERBS`.

    /// Hard-deprecated verbs from Phase 2.1 A25. Mirrors the list in
    /// `skills::content::tests::DEPRECATED_VERBS` — kept here as a
    /// local copy so this test module is self-contained (the upstream
    /// list lives in a `cfg(test)` module and can't be referenced
    /// across compilation units).
    const DEPRECATED_REGEN_VERBS: &[&str] = &[
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

    fn assert_regen_body_has_no_deprecated_verbs(body: &str, context: &str) {
        let mut hits: Vec<&str> = Vec::new();
        for verb in DEPRECATED_REGEN_VERBS {
            if body.contains(verb) {
                hits.push(verb);
            }
        }
        assert!(
            hits.is_empty(),
            "{context}: regen body must NOT contain hard-deprecated verbs; found {hits:?}\n\
             Excerpt (first 400 chars):\n{}",
            &body[..body.len().min(400)],
        );
    }

    #[test]
    fn regenerate_workspace_skill_emits_no_deprecated_verbs() {
        // `regenerate_workspace_skill` auto-scaffolds `.k2so/agents/
        // manager/` and `.k2so/agents/k2so-agent/` on its first call
        // for a fresh workspace, so `is_manager_mode` resolves true
        // via the filesystem fallback and the manager-tier canonical
        // body is selected. This is the path users hit in practice
        // when they enable manager mode for the first time.
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        let body = regenerate_workspace_skill(path).expect("regen ok");
        assert_regen_body_has_no_deprecated_verbs(&body, "manager-mode regen (auto-scaffold)");

        // The manager-tier canonical generator emits the Workspace
        // Manager header — confirm we routed through it (not a leftover
        // inline template).
        assert!(
            body.contains("Workspace Manager"),
            "manager-mode regen must include 'Workspace Manager' from the canonical generator",
        );

        // Sanity: at least one canonical A25 verb should appear so an
        // accidental empty-body regression doesn't trivially pass.
        assert!(
            body.contains("k2so checkin")
                && body.contains("k2so inbox"),
            "regen body must reference canonical A25 verbs (k2so checkin, k2so inbox), got first 400 chars:\n{}",
            &body[..body.len().min(400)],
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regenerate_workspace_skill_emits_no_deprecated_verbs_with_pre_existing_skill_dir() {
        // 0.39.0 (workspace==agent): the manager body's "Team" section
        // is now sourced from `connections::list_peers` — bidirectional
        // connected workspaces — NOT from a `.k2so/agents/` directory
        // walk or `.k2so/skills/<name>/` enumeration.
        //
        // Pre-seed a `.k2so/skills/<name>/` entry (the layout post-
        // Phase-2.5b) and assert the regenerated body:
        //   1. Does NOT introduce deprecated verbs.
        //   2. Reports an empty Team section (no connections seeded)
        //      with the canonical "No connected workspaces yet" hint.
        //   3. Does NOT list the seeded skill as a team member —
        //      skills are documentation profiles, not peers.
        let proj = scratch_project();
        let path = proj.to_str().unwrap().to_string();

        fs::create_dir_all(proj.join(".k2so/skills/backend-eng")).unwrap();
        fs::write(
            proj.join(".k2so/skills/backend-eng/SKILL.md"),
            "---\nname: backend-eng\nrole: Backend engineer\ntype: agent-template\n---\n",
        )
        .unwrap();

        let body = regenerate_workspace_skill(path).expect("regen ok");
        assert_regen_body_has_no_deprecated_verbs(&body, "manager-mode regen (with backend-eng skill)");

        // Team section reports empty — no connections seeded.
        assert!(
            body.contains("No connected workspaces yet"),
            "manager body must report empty Team (no connections seeded); first 400 chars:\n{}",
            &body[..body.len().min(400)],
        );
        // The seeded skill must not appear in the team section. Slice
        // the body to just the team region to avoid false positives
        // from unrelated places where the name might legitimately
        // appear (none expected here, but be precise).
        let team_start = body.find("## Team — Connected Workspaces").expect("team section present");
        let after_team = &body[team_start..];
        let team_end_rel = after_team[2..]
            .find("\n## ")
            .map(|i| i + 2)
            .unwrap_or(after_team.len());
        let team_region = &after_team[..team_end_rel];
        assert!(
            !team_region.contains("backend-eng"),
            "seeded skill must NOT appear as team member; team region:\n{team_region}"
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn workspace_skill_generate_content_emits_no_deprecated_verbs() {
        // `generate_workspace_skill_content` is the lightweight user-
        // facing template invoked when `write_workspace_skill_file` is
        // called without a body override (e.g. from
        // `workspace::agent_launch::launch_agent` when there is no
        // current task). Pin its verb set too.
        let body = generate_workspace_skill_content("TestWorkspace");
        assert_regen_body_has_no_deprecated_verbs(&body, "generate_workspace_skill_content");
        assert!(body.contains("TestWorkspace"), "project name must appear");
    }
}
