//! Workspace onboarding — the three-option flow for first-time
//! setup when a workspace already has CLI-LLM harness files
//! (CLAUDE.md, GEMINI.md, .cursor/rules/k2so.mdc, etc.) that
//! K2SO's symlink fanout would otherwise silently take over.
//!
//! Used by:
//! - The Tauri WorkspaceOnboardingModal in the renderer (display +
//!   button-click only — no logic lives in the renderer).
//! - The `k2so onboarding` CLI subcommand for headless setups.
//!
//! Three options surfaced to the user:
//!
//! - **Skip** — drop a `.k2so/.skip-harness-management` flag file.
//!   K2SO still writes its internal SKILL.md (so heartbeats and
//!   agent launches keep working), but the harness fanout step in
//!   `skills::writer::write_skill_to_all_harnesses` short-circuits,
//!   leaving CLAUDE.md / GEMINI.md / .cursor/rules / etc. untouched.
//!
//! - **Start Fresh** — the existing default behavior. No-op here;
//!   the caller invokes the normal regen pipeline which archives
//!   pre-existing harness files to `.k2so/migration/` and replaces
//!   them with symlinks. Documented as a method on this module so
//!   the CLI/Tauri layer has one symmetric entry point.
//!
//! - **Adopt** — pick one of the detected harness files; copy its
//!   body into `.k2so/PROJECT.md` as the seed for K2SO's workspace
//!   knowledge. Source file is then archived and removed from its
//!   original location so the subsequent regen doesn't re-import
//!   the same content a second time via the existing migration
//!   helpers. After adoption, caller invokes the normal regen
//!   pipeline.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs_atomic::atomic_write_str;

/// A harness file detected on the workspace root, returned to the
/// renderer (or printed by the CLI) so the user can pick which one
/// to adopt. Pure data — every interesting transform happens here
/// in core, not in the display layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedHarnessFile {
    /// Absolute path on disk.
    pub path: String,
    /// Path relative to the workspace root, suitable for display.
    pub relative_path: String,
    /// Human-readable label for the harness this file belongs to
    /// (e.g. "Claude Code", "Cursor rule"). Comes from the probe
    /// table — not derived from the filename so we control wording.
    pub label: String,
    /// Bytes of user content (post-frontmatter strip and post-
    /// K2SO-marker strip) — gives the picker a sense of how much
    /// real content the file has without rendering the full body.
    pub byte_count: usize,
    /// First ~400 chars of the body, for the picker preview pane.
    pub preview: String,
    /// Last-modified mtime as seconds since the unix epoch. The
    /// renderer uses this to sort newest-first.
    pub mtime_secs: u64,
}

/// Outcome returned by `adopt_harness_as_project_md` so the
/// renderer (or CLI) can surface a confirmation message + the
/// archive path the source was preserved at.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptionOutcome {
    /// Where the source file was archived to.
    pub archive_path: String,
    /// Where the new PROJECT.md was written.
    pub project_md_path: String,
    /// How many bytes of the source body were copied.
    pub adopted_bytes: usize,
}

// ── Skip flag ──────────────────────────────────────────────────────

/// Marker filename — written by `skip_harness_management` and
/// checked by `skills::writer::write_skill_to_all_harnesses` (and
/// the harness-discovery target writer in the workspace regen
/// orchestrator) before doing any harness fanout. Lives under
/// `.k2so/` so it survives anything that touches the workspace
/// root, and is tracked by git only if the user explicitly stages
/// it.
pub const SKIP_HARNESS_FLAG_FILENAME: &str = ".skip-harness-management";

/// Absolute path to the skip-harness-management flag file for a
/// given project root.
pub fn skip_flag_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join(SKIP_HARNESS_FLAG_FILENAME)
}

/// Whether the user has opted out of K2SO touching harness files
/// for this workspace. Cheap fs-stat read on every regen tick.
pub fn is_harness_management_skipped(project_path: &str) -> bool {
    skip_flag_path(project_path).exists()
}

// ── Harness-fanout permission marker (the opt-IN switch) ────────────

/// Marker filename — written by `set_harness_fanout_enabled(true)` and
/// checked by `harness_fanout_enabled` before any user-visible harness
/// fan-out (symlinks into `.claude/` / `.opencode/` / `.pi/`, root
/// CLAUDE.md / GEMINI.md / etc., marker-injection into AGENTS.md /
/// copilot-instructions.md).
///
/// This is the positive opt-IN that replaces the old always-on
/// behavior: as of the canonical-agents feature, harness fan-out is
/// **off by default**. The canonical `.k2so/skills/<name>/SKILL.md`
/// still always generates (heartbeats + agent launches depend on it);
/// only the fan-out into user-visible harness paths is gated.
///
/// Filesystem-first / daemon-first: the marker under `.k2so/` is the
/// authoritative source of truth. The Settings UI may mirror it into
/// the `projects` DB row for fast listing, but the marker wins.
pub const HARNESS_FANOUT_FLAG_FILENAME: &str = ".harness-fanout-enabled";

/// Absolute path to the harness-fanout opt-in marker for a project.
pub fn harness_fanout_flag_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join(HARNESS_FANOUT_FLAG_FILENAME)
}

/// Whether user-visible harness fan-out is enabled for this workspace.
///
/// Default is **false** (off by default). Returns true only when the
/// positive `.k2so/.harness-fanout-enabled` marker is present.
///
/// **Legacy alias:** the presence of `.k2so/.skip-harness-management`
/// always FORCES `false`, regardless of the opt-in marker — a user who
/// explicitly skipped harness management before this feature keeps that
/// posture even if some path later writes the opt-in marker. The skip
/// flag is the harder override.
pub fn harness_fanout_enabled(project_path: &str) -> bool {
    // Legacy skip flag is the harder override — its presence forces off.
    if is_harness_management_skipped(project_path) {
        return false;
    }
    harness_fanout_flag_path(project_path).exists()
}

// ── AGENTS.md generate marker (independent of leftover fan-out) ────────

/// Marker filename — written by `set_agents_md_generate_enabled(true)`.
/// Positive marker only: absence means generate is off. Do **not** invent
/// a skip-marker (absence-means-on would fleet-plant cwd files on existing
/// workspaces).
pub const AGENTS_MD_GENERATE_FLAG_FILENAME: &str = ".agents-md-generate";

/// Compose banner that marks a real cwd `AGENTS.md` as K2-generated.
/// Distinct from [`crate::workspace::canonical::K2_GENERATED_SIGNATURE`]
/// (`k2_generated: true`), which is the cursor-mdc stamp.
pub const AGENTS_MD_COMPOSE_BANNER: &str = "<!-- GENERATED by K2";

/// Body with the `<!-- GENERATED by K2 at … -->` comment removed.
/// Used so a second compose pass can no-op when only the banner timestamp
/// would change.
pub fn strip_agents_md_compose_banner(body: &str) -> String {
    let Some(start) = body.find(AGENTS_MD_COMPOSE_BANNER) else {
        return body.to_string();
    };
    let after = &body[start..];
    let Some(rel_end) = after.find("-->") else {
        return body.to_string();
    };
    let end = start + rel_end + 3;
    let mut out = String::with_capacity(body.len().saturating_sub(end - start));
    out.push_str(&body[..start]);
    out.push_str(&body[end..]);
    out
}

/// Loud-skip reason when a user-authored cwd `AGENTS.md` is present.
pub const PLANT_SKIP_USER_FILE: &str = "user AGENTS.md present (not K2-generated)";

/// Loud-skip reason when cwd `AGENTS.md` is a symlink that does not
/// point at `.k2/AGENTS.md` (or `.k2so/AGENTS.md`).
pub const PLANT_SKIP_UNMANAGED_SYMLINK: &str = "unmanaged AGENTS.md symlink";

/// Outcome of [`plant_root_agents_md`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlantResult {
    /// Missing file → wrote the composed body as a real file.
    Written,
    /// Real GENERATED file → overwritten with the new compose.
    Overwritten,
    /// Legacy our-symlink → `.k2/AGENTS.md` left in place.
    LeftSymlink,
    /// User file or unmanaged symlink — not touched.
    Skipped { reason: String },
}

/// Absolute path to the generate opt-in marker for a project.
pub fn agents_md_generate_flag_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join(AGENTS_MD_GENERATE_FLAG_FILENAME)
}

/// Whether cwd `AGENTS.md` generate is enabled.
///
/// **Only** the positive `.agents-md-generate` marker. Skip-harness does
/// **not** force this off — generate and leftover fan-out are independent.
pub fn agents_md_generate_enabled(project_path: &str) -> bool {
    agents_md_generate_flag_path(project_path).exists()
}

/// Set (or clear) the generate marker. Idempotent. Off does **not**
/// delete cwd `AGENTS.md`.
pub fn set_agents_md_generate_enabled(project_path: &str, enabled: bool) -> Result<(), String> {
    let flag = agents_md_generate_flag_path(project_path);
    if enabled {
        if let Some(parent) = flag.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create .k2/: {e}"))?;
        }
        atomic_write_str(&flag, "").map_err(|e| format!("write agents-md-generate flag: {e}"))?;
    } else if flag.exists() {
        fs::remove_file(&flag).map_err(|e| format!("remove agents-md-generate flag: {e}"))?;
    }
    Ok(())
}

/// True when `path` is a symlink whose target is the workspace's
/// canonical `.k2/AGENTS.md` (or the legacy `.k2so/AGENTS.md` name).
pub fn is_our_agents_md_symlink(project_path: &str, link: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(link) else {
        return false;
    };
    if !meta.file_type().is_symlink() {
        return false;
    }
    let Ok(target) = fs::read_link(link) else {
        return false;
    };
    let resolved = if target.is_absolute() {
        target
    } else {
        link.parent()
            .unwrap_or_else(|| Path::new(project_path))
            .join(target)
    };
    if let (Ok(a), Ok(b)) = (
        fs::canonicalize(&resolved),
        fs::canonicalize(crate::workspace_dot_dir(project_path).join("AGENTS.md")),
    ) {
        return a == b;
    }
    looks_like_canonical_agents_md(&resolved)
}

fn looks_like_canonical_agents_md(path: &Path) -> bool {
    let name_ok = path.file_name().and_then(|n| n.to_str()) == Some("AGENTS.md");
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    name_ok && matches!(parent, Some(".k2") | Some(".k2so"))
}

/// True when cwd `AGENTS.md` is ours: a GENERATED real file, or our
/// symlink to `.k2/AGENTS.md`.
pub fn root_agents_md_is_ours(project_path: &str) -> bool {
    let root = Path::new(project_path).join("AGENTS.md");
    let Ok(meta) = fs::symlink_metadata(&root) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return is_our_agents_md_symlink(project_path, &root);
    }
    if !meta.file_type().is_file() {
        return false;
    }
    fs::read_to_string(&root)
        .map(|s| s.contains(AGENTS_MD_COMPOSE_BANNER))
        .unwrap_or(false)
}

/// If root `AGENTS.md` is already ours and the generate marker is
/// missing, write the marker. Does **not** plant a missing file.
/// Returns true when the marker was healed.
pub fn heal_agents_md_generate_marker(project_path: &str) -> bool {
    if agents_md_generate_enabled(project_path) {
        return false;
    }
    if !root_agents_md_is_ours(project_path) {
        return false;
    }
    set_agents_md_generate_enabled(project_path, true).is_ok()
}

/// Plant workspace-root `AGENTS.md` as a **real file** copy of the
/// composed body. Never archives. Never reuses leftover symlink helpers.
///
/// | On-disk | Action |
/// | Missing | Write composed body as a real file |
/// | Our symlink → `.k2/AGENTS.md` | Leave the symlink |
/// | Real file with `<!-- GENERATED by K2` | Overwrite |
/// | Real file without that banner | Loud skip, no archive |
/// | Symlink elsewhere | Unmanaged — same skip |
pub fn plant_root_agents_md(project_path: &str, composed_body: &str) -> PlantResult {
    let target = Path::new(project_path).join("AGENTS.md");
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if is_our_agents_md_symlink(project_path, &target) {
                PlantResult::LeftSymlink
            } else {
                crate::log_debug!(
                    "[agents-md-generate] skipped: unmanaged AGENTS.md symlink at {}",
                    target.display()
                );
                PlantResult::Skipped {
                    reason: PLANT_SKIP_UNMANAGED_SYMLINK.to_string(),
                }
            }
        }
        Ok(meta) if meta.file_type().is_file() => match fs::read_to_string(&target) {
            Ok(existing) if existing.contains(AGENTS_MD_COMPOSE_BANNER) => {
                if strip_agents_md_compose_banner(&existing)
                    == strip_agents_md_compose_banner(composed_body)
                {
                    return PlantResult::Overwritten;
                }
                if let Err(e) = atomic_write_str(&target, composed_body) {
                    crate::log_debug!(
                        "[agents-md-generate] overwrite failed at {}: {e}",
                        target.display()
                    );
                    return PlantResult::Skipped {
                        reason: format!("write failed: {e}"),
                    };
                }
                PlantResult::Overwritten
            }
            _ => {
                crate::log_debug!(
                    "[agents-md-generate] skipped: user AGENTS.md present (not K2-generated) at {}",
                    target.display()
                );
                PlantResult::Skipped {
                    reason: PLANT_SKIP_USER_FILE.to_string(),
                }
            }
        },
        _ => {
            if let Err(e) = atomic_write_str(&target, composed_body) {
                crate::log_debug!(
                    "[agents-md-generate] write failed at {}: {e}",
                    target.display()
                );
                return PlantResult::Skipped {
                    reason: format!("write failed: {e}"),
                };
            }
            PlantResult::Written
        }
    }
}

/// Classify cwd `AGENTS.md` after a compose that already planted.
/// Does **not** write — used by set-generate so regen is the only plant.
pub fn inspect_root_agents_md_plant(project_path: &str) -> PlantResult {
    let target = Path::new(project_path).join("AGENTS.md");
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if is_our_agents_md_symlink(project_path, &target) {
                PlantResult::LeftSymlink
            } else {
                PlantResult::Skipped {
                    reason: PLANT_SKIP_UNMANAGED_SYMLINK.to_string(),
                }
            }
        }
        Ok(meta) if meta.file_type().is_file() => match fs::read_to_string(&target) {
            Ok(existing) if existing.contains(AGENTS_MD_COMPOSE_BANNER) => PlantResult::Overwritten,
            _ => PlantResult::Skipped {
                reason: PLANT_SKIP_USER_FILE.to_string(),
            },
        },
        _ => PlantResult::Written,
    }
}

/// Set (or clear) the harness-fanout opt-in marker. Idempotent.
///
/// Writing the marker does NOT clear the legacy `.skip-harness-management`
/// flag — callers that want fan-out fully on must also call
/// `unskip_harness_management`. (Keeping them independent lets the
/// legacy skip remain the harder, explicit "never touch my files"
/// override.)
pub fn set_harness_fanout_enabled(project_path: &str, enabled: bool) -> Result<(), String> {
    let flag = harness_fanout_flag_path(project_path);
    if enabled {
        if let Some(parent) = flag.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create .k2so/: {e}"))?;
        }
        atomic_write_str(&flag, "").map_err(|e| format!("write harness-fanout flag: {e}"))?;
    } else if flag.exists() {
        fs::remove_file(&flag).map_err(|e| format!("remove harness-fanout flag: {e}"))?;
    }
    Ok(())
}

/// Drop the skip flag. Idempotent — repeated calls just rewrite
/// the (empty) marker file.
pub fn skip_harness_management(project_path: &str) -> Result<(), String> {
    let flag = skip_flag_path(project_path);
    if let Some(parent) = flag.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create .k2so/: {e}"))?;
    }
    atomic_write_str(&flag, "").map_err(|e| format!("write skip flag: {e}"))?;
    Ok(())
}

/// Remove the skip flag — used when a user changes their mind
/// and wants K2SO to take over harness management on the next
/// regen.
pub fn unskip_harness_management(project_path: &str) -> Result<(), String> {
    let flag = skip_flag_path(project_path);
    if flag.exists() {
        fs::remove_file(&flag).map_err(|e| format!("remove skip flag: {e}"))?;
    }
    Ok(())
}

// ── Scan ───────────────────────────────────────────────────────────

/// Standard list of harness probes. Order is the order shown to
/// the user in the picker (most-common-LLM-tools first).
const HARNESS_PROBES: &[(&str, &str)] = &[
    ("CLAUDE.md", "Claude Code"),
    ("AGENTS.md", "Multi-harness AGENTS.md"),
    ("GEMINI.md", "Gemini"),
    ("AGENT.md", "Agent.md (singular)"),
    (".goosehints", "Goose"),
    (".cursor/rules/k2so.mdc", "Cursor rule"),
    (".opencode/agent/k2so.md", "OpenCode"),
    (".pi/skills/k2so/SKILL.md", "Pi"),
    (".github/copilot-instructions.md", "GitHub Copilot"),
];

/// Scan the workspace root for harness files with substantive
/// user content. Files that are missing, empty, dangling
/// symlinks, K2SO-managed symlinks, or contain only the K2SO
/// marker block (no real user content beyond what K2SO wrote)
/// are excluded — the picker should never present a fresh-from-
/// K2SO file as "your existing context."
///
/// Respects the skip-harness-management flag: if the user has
/// already chosen "Do it later" for this workspace, scan returns
/// empty so callers don't re-prompt them on every workspace open.
pub fn scan_harness_files(project_path: &str) -> Vec<DetectedHarnessFile> {
    if is_harness_management_skipped(project_path) {
        return Vec::new();
    }
    let root = PathBuf::from(project_path);
    let mut found = Vec::new();

    for (rel, label) in HARNESS_PROBES {
        let abs = root.join(rel);

        // Skip K2SO's own symlinks (or any symlink) — those are
        // already managed and not user content.
        let Ok(sym_meta) = fs::symlink_metadata(&abs) else {
            continue;
        };
        if sym_meta.file_type().is_symlink() {
            continue;
        }
        if !sym_meta.file_type().is_file() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&abs) else {
            continue;
        };

        // Strip frontmatter (Cursor MDC and any markdown with
        // YAML frontmatter) and K2SO marker blocks so a file
        // containing *only* K2SO-injected content registers as
        // empty.
        let body = strip_frontmatter(&content);
        let user_body = strip_k2so_managed_block(body);
        let stripped = user_body.trim();
        if stripped.is_empty() {
            continue;
        }

        let preview: String = stripped.chars().take(400).collect();
        let mtime_secs = sym_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        found.push(DetectedHarnessFile {
            path: abs.display().to_string(),
            relative_path: (*rel).to_string(),
            label: (*label).to_string(),
            byte_count: stripped.len(),
            preview,
            mtime_secs,
        });
    }

    found
}

// ── Adopt ──────────────────────────────────────────────────────────

/// Adopt a harness file as the seed for `.k2so/PROJECT.md`. The
/// source body (frontmatter and K2SO markers stripped) is written
/// to PROJECT.md, the original is archived to
/// `.k2so/migration/<leaf>-<ts>.<ext>` (matching the existing
/// CLAUDE.md migration archive convention), and the original is
/// then removed so the subsequent regen pipeline doesn't re-
/// archive + re-import the same content a second time via the
/// pre-existing `migrate_and_symlink_root_claude_md` /
/// `safe_symlink_harness_file` paths.
///
/// Caller (Tauri command or CLI) is expected to invoke the
/// normal workspace-regen pipeline afterward — this function only
/// stages content into PROJECT.md; it does not run regen itself.
/// Decoupling lets the caller batch ops (adopt → regen) without
/// double-firing.
pub fn adopt_harness_as_project_md(
    project_path: &str,
    source_path: &Path,
) -> Result<AdoptionOutcome, String> {
    if !source_path.exists() {
        return Err(format!(
            "source file does not exist: {}",
            source_path.display()
        ));
    }

    let raw = fs::read_to_string(source_path).map_err(|e| format!("read source: {e}"))?;
    let body_no_fm = strip_frontmatter(&raw);
    let body = strip_k2so_managed_block(body_no_fm);
    let body = body.trim();
    let adopted_bytes = body.len();

    // Archive the source first so we have a recovery point if any
    // subsequent step fails.
    let project_root = PathBuf::from(project_path);
    let archive_dir = crate::workspace_dot_dir(&project_root).join("migration");
    fs::create_dir_all(&archive_dir).map_err(|e| format!("create archive dir: {e}"))?;
    let leaf = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("adopted-harness")
        .to_string();
    let archive_path = unique_archive_path(&archive_dir, &leaf);
    atomic_write_str(&archive_path, &raw).map_err(|e| format!("archive source: {e}"))?;

    // Write PROJECT.md. Don't clobber substantive existing content
    // — onboarding is gated on "fresh PROJECT.md" upstream, but we
    // double-check here for defense-in-depth.
    let project_md = crate::workspace_dot_dir(&project_root).join("PROJECT.md");
    if let Some(parent) = project_md.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create .k2so/: {e}"))?;
    }
    if !project_md_has_user_content(&project_md) {
        let final_body = format!("# Project Context\n\n{}\n", body);
        atomic_write_str(&project_md, &final_body).map_err(|e| format!("write PROJECT.md: {e}"))?;
    }

    // Remove the source so the regen pipeline doesn't re-archive
    // + re-import the same body via its existing migration
    // helpers. The archive we just wrote is the single source of
    // truth for the original content.
    //
    // **0.37.6:** route to recycle bin — `source_path` is the
    // user's original CLAUDE.md / GEMINI.md / etc. We've already
    // written an archive copy to .k2so/migration/ but Trash gives
    // the user a second recovery path if something went wrong with
    // the archive write.
    //
    // SAFETY: routes through `scratch_safe_trash` so test scratch
    // paths under temp_dir() skip the trash crate (avoids macOS
    // Touch ID prompts during cargo test).
    crate::safe_delete_scratch::scratch_safe_trash(source_path)
        .map_err(|e| format!("trash adopted source: {e}"))?;

    Ok(AdoptionOutcome {
        archive_path: archive_path.display().to_string(),
        project_md_path: project_md.display().to_string(),
        adopted_bytes,
    })
}

// ── Helpers ────────────────────────────────────────────────────────

/// Strip YAML frontmatter (`---`-delimited block at start of file).
/// Returns a borrowed slice rather than allocating when there's no
/// frontmatter, since most files won't have one.
fn strip_frontmatter(content: &str) -> &str {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let after = &content[3 + end + 3..];
            return after.trim_start_matches('\n');
        }
    }
    content
}

/// Remove the K2-managed block (`<!-- K2:BEGIN -->` … `<!-- K2:END -->`)
/// from a string. Used so files containing *only* K2-injected content read
/// as empty during scan, and so adopting a file's body doesn't carry K2's
/// own markers into the new PROJECT.md. Anchored on the writer's
/// [`crate::skills::writer::K2SO_SECTION_BEGIN`] / `…_END` constants so the
/// strip stays in lockstep with the inject.
pub fn strip_k2so_managed_block(content: &str) -> String {
    use crate::skills::writer::{K2SO_SECTION_BEGIN as BEGIN, K2SO_SECTION_END as END};
    if let (Some(b), Some(e)) = (content.find(BEGIN), content.find(END)) {
        if e > b {
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..b]);
            out.push_str(&content[e + END.len()..]);
            return out;
        }
    }
    content.to_string()
}

/// Whether `.k2so/PROJECT.md` already has substantive content
/// (anything beyond a heading + blockquote prompt scaffold).
/// Adoption skips overwriting in that case.
fn project_md_has_user_content(project_md: &Path) -> bool {
    let Ok(raw) = fs::read_to_string(project_md) else {
        return false;
    };
    let stripped = strip_frontmatter(&raw).trim().to_string();
    stripped.lines().any(|line| {
        let t = line.trim();
        !t.is_empty() && !t.starts_with('#') && !t.starts_with("<!--") && !t.starts_with('>')
    })
}

/// Pick a unique archive path inside `dir` for a given filename,
/// adding a nanosecond suffix to avoid collisions when adoption
/// is re-run quickly (e.g., user retries the picker). Mirrors
/// the convention `archive_claude_md_file` uses in the Tauri
/// command layer for the existing migration flow.
fn unique_archive_path(dir: &Path, leaf: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let (stem, ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.to_string(), String::new()),
    };
    dir.join(format!("{stem}-{nanos}{ext}"))
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("k2so-onboarding-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn skip_flag_round_trip() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        assert!(!is_harness_management_skipped(&path));
        skip_harness_management(&path).unwrap();
        assert!(is_harness_management_skipped(&path));
        unskip_harness_management(&path).unwrap();
        assert!(!is_harness_management_skipped(&path));
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn harness_fanout_disabled_by_default() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        // Off by default — no marker present.
        assert!(
            !harness_fanout_enabled(&path),
            "fan-out must be OFF by default for a fresh workspace",
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn harness_fanout_marker_round_trip() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        assert!(!harness_fanout_enabled(&path));
        set_harness_fanout_enabled(&path, true).unwrap();
        assert!(
            harness_fanout_enabled(&path),
            "fan-out must be ON after setting the opt-in marker",
        );
        set_harness_fanout_enabled(&path, false).unwrap();
        assert!(
            !harness_fanout_enabled(&path),
            "fan-out must be OFF after clearing the opt-in marker",
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn legacy_skip_flag_forces_fanout_off_even_when_enabled() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        // Opt in to fan-out…
        set_harness_fanout_enabled(&path, true).unwrap();
        assert!(
            harness_fanout_enabled(&path),
            "sanity: opt-in marker enables fan-out"
        );
        // …but the legacy skip flag is the harder override.
        skip_harness_management(&path).unwrap();
        assert!(
            !harness_fanout_enabled(&path),
            "legacy .skip-harness-management must FORCE fan-out off even with the opt-in marker present",
        );
        // Removing the skip flag restores the opt-in state.
        unskip_harness_management(&path).unwrap();
        assert!(
            harness_fanout_enabled(&path),
            "clearing the skip flag restores the opt-in fan-out state",
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn scan_finds_user_authored_claude_md() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        fs::write(p.join("CLAUDE.md"), "# My project\n\nUses Rust + Tauri.\n").unwrap();
        let found = scan_harness_files(&path);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].relative_path, "CLAUDE.md");
        assert!(found[0].preview.contains("Uses Rust + Tauri"));
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn scan_skips_files_containing_only_k2so_block() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        fs::write(
            p.join("AGENTS.md"),
            "<!-- K2:BEGIN -->\nfoo\n<!-- K2:END -->\n",
        )
        .unwrap();
        let found = scan_harness_files(&path);
        assert!(found.is_empty(), "expected no detections, got {:?}", found);
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn scan_skips_symlinks() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        // Real file we'll point a symlink at.
        let real = p.join("real-content.md");
        fs::write(&real, "real body\n").unwrap();
        // CLAUDE.md is a symlink — should be ignored.
        let claude = p.join("CLAUDE.md");
        std::os::unix::fs::symlink(&real, &claude).unwrap();
        let found = scan_harness_files(&path);
        assert!(found.is_empty(), "expected no detections, got {:?}", found);
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn adopt_seeds_project_md_and_archives_source() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        fs::create_dir_all(p.join(".k2so")).unwrap();
        let claude = p.join("CLAUDE.md");
        fs::write(&claude, "# K2SO\n\nUses Rust and Tauri.\n").unwrap();

        let outcome = adopt_harness_as_project_md(&path, &claude).unwrap();
        // PROJECT.md got written
        let project_md = p.join(".k2so/PROJECT.md");
        assert!(project_md.exists());
        let body = fs::read_to_string(&project_md).unwrap();
        assert!(body.contains("Uses Rust and Tauri"));
        // Source got removed
        assert!(!claude.exists(), "source should have been removed");
        // Archive got written
        assert!(PathBuf::from(&outcome.archive_path).exists());
        let archived = fs::read_to_string(&outcome.archive_path).unwrap();
        assert!(archived.contains("Uses Rust and Tauri"));
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn adopt_skips_overwrite_when_project_md_has_user_content() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        fs::create_dir_all(p.join(".k2so")).unwrap();
        // Pre-existing PROJECT.md with substantive content
        fs::write(
            p.join(".k2so/PROJECT.md"),
            "# Project Context\n\nExisting content the user wrote.\n",
        )
        .unwrap();
        let claude = p.join("CLAUDE.md");
        fs::write(&claude, "# K2SO\n\nNew content from CLAUDE.md\n").unwrap();
        adopt_harness_as_project_md(&path, &claude).unwrap();
        let body = fs::read_to_string(p.join(".k2so/PROJECT.md")).unwrap();
        assert!(body.contains("Existing content the user wrote"));
        assert!(!body.contains("New content from CLAUDE.md"));
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn agents_md_generate_disabled_by_default() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        assert!(
            !agents_md_generate_enabled(&path),
            "generate must be OFF unless the positive marker is present"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn agents_md_generate_marker_round_trip() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        assert!(!agents_md_generate_enabled(&path));
        set_agents_md_generate_enabled(&path, true).unwrap();
        assert!(
            agents_md_generate_enabled(&path),
            "generate must be ON after writing the marker"
        );
        set_agents_md_generate_enabled(&path, false).unwrap();
        assert!(
            !agents_md_generate_enabled(&path),
            "generate must be OFF after clearing the marker"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn skip_harness_does_not_force_generate_off() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        set_agents_md_generate_enabled(&path, true).unwrap();
        skip_harness_management(&path).unwrap();
        assert!(
            agents_md_generate_enabled(&path),
            "skip-harness must NOT force generate off"
        );
        assert!(
            !harness_fanout_enabled(&path),
            "skip-harness still forces fan-out off"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn plant_writes_missing_root_agents_md_as_real_file() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        let body = format!("{AGENTS_MD_COMPOSE_BANNER} at now -->\n\n# hello\n");
        let result = plant_root_agents_md(&path, &body);
        assert_eq!(result, PlantResult::Written);
        let root = p.join("AGENTS.md");
        let meta = fs::symlink_metadata(&root).expect("root AGENTS.md must exist");
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "generate plants a real file, not a symlink"
        );
        let on_disk = fs::read_to_string(&root).expect("read planted file");
        assert_eq!(on_disk, body, "planted bytes must match compose body");
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn plant_skips_user_authored_root_agents_md_without_archiving() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        let user = "# my own notes\nplease keep me\n";
        fs::write(p.join("AGENTS.md"), user).unwrap();
        let result = plant_root_agents_md(&path, &format!("{AGENTS_MD_COMPOSE_BANNER} -->\nnew\n"));
        match result {
            PlantResult::Skipped { reason } => {
                assert_eq!(reason, PLANT_SKIP_USER_FILE, "loud skip reason");
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
        let on_disk = fs::read_to_string(p.join("AGENTS.md")).expect("user file remains");
        assert_eq!(on_disk, user, "user AGENTS.md bytes must be unchanged");
        assert!(
            !p.join(".k2/migration").exists() && !p.join(".k2so/migration").exists(),
            "generate must not archive a user AGENTS.md"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn plant_overwrites_generated_root_file() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        let old = format!("{AGENTS_MD_COMPOSE_BANNER} at old -->\n\nold compose\n");
        fs::write(p.join("AGENTS.md"), &old).unwrap();
        let new = format!("{AGENTS_MD_COMPOSE_BANNER} at new -->\n\nnew compose\n");
        let result = plant_root_agents_md(&path, &new);
        assert_eq!(result, PlantResult::Overwritten);
        let meta = fs::symlink_metadata(p.join("AGENTS.md")).expect("still present");
        assert!(
            meta.file_type().is_file() && !meta.file_type().is_symlink(),
            "overwrite must stay a real file"
        );
        let on_disk = fs::read_to_string(p.join("AGENTS.md")).expect("read overwrite");
        assert_eq!(on_disk, new);
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn plant_leaves_legacy_our_symlink() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        let canonical = crate::workspace_dot_dir(&p).join("AGENTS.md");
        fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        fs::write(
            &canonical,
            format!("{AGENTS_MD_COMPOSE_BANNER} -->\ncanon\n"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&canonical, p.join("AGENTS.md")).unwrap();
        let result = plant_root_agents_md(&path, "ignored");
        assert_eq!(result, PlantResult::LeftSymlink);
        let meta = fs::symlink_metadata(p.join("AGENTS.md")).expect("symlink remains");
        assert!(
            meta.file_type().is_symlink(),
            "legacy our-symlink must stay a symlink"
        );
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn plant_skips_foreign_symlink() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        let other = p.join("elsewhere.md");
        fs::write(&other, "not ours\n").unwrap();
        std::os::unix::fs::symlink(&other, p.join("AGENTS.md")).unwrap();
        let result = plant_root_agents_md(&path, "new");
        match result {
            PlantResult::Skipped { reason } => {
                assert_eq!(reason, PLANT_SKIP_UNMANAGED_SYMLINK);
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
        let target = fs::read_link(p.join("AGENTS.md")).expect("symlink left in place");
        assert_eq!(target, other);
        fs::remove_dir_all(&p).ok();
    }

    #[test]
    fn strip_compose_banner_drops_only_the_generated_comment() {
        let body = "<!-- GENERATED by K2 at 2026-08-17T00:00:00Z from system sources -->\n\n# Role\n\nhello\n";
        let stripped = strip_agents_md_compose_banner(body);
        assert!(
            !stripped.contains("GENERATED by K2"),
            "banner comment must be gone: {stripped}"
        );
        assert!(stripped.contains("# Role"), "body must remain");
        assert_eq!(
            strip_agents_md_compose_banner(body),
            strip_agents_md_compose_banner(
                "<!-- GENERATED by K2 at 2026-08-17T01:02:03Z from system sources -->\n\n# Role\n\nhello\n"
            ),
            "same body with a different timestamp must hash-skip"
        );
    }

    #[test]
    fn heal_writes_marker_only_when_root_is_ours() {
        let p = temp_project();
        let path = p.to_string_lossy().to_string();
        assert!(
            !heal_agents_md_generate_marker(&path),
            "missing file must not heal (that would fleet-plant)"
        );
        assert!(!agents_md_generate_enabled(&path));

        fs::write(p.join("AGENTS.md"), "# human file\n").unwrap();
        assert!(
            !heal_agents_md_generate_marker(&path),
            "user file must not heal"
        );
        assert!(!agents_md_generate_enabled(&path));

        fs::write(
            p.join("AGENTS.md"),
            format!("{AGENTS_MD_COMPOSE_BANNER} -->\nours\n"),
        )
        .unwrap();
        assert!(
            heal_agents_md_generate_marker(&path),
            "GENERATED root file must heal the marker"
        );
        assert!(agents_md_generate_enabled(&path));
        fs::remove_dir_all(&p).ok();
    }
}
