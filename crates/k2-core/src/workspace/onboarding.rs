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
        atomic_write_str(&flag, "")
            .map_err(|e| format!("write harness-fanout flag: {e}"))?;
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
    atomic_write_str(&flag, "")
        .map_err(|e| format!("write skip flag: {e}"))?;
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
        let Ok(sym_meta) = fs::symlink_metadata(&abs) else { continue };
        if sym_meta.file_type().is_symlink() {
            continue;
        }
        if !sym_meta.file_type().is_file() {
            continue;
        }
        let Ok(content) = fs::read_to_string(&abs) else { continue };

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
        atomic_write_str(&project_md, &final_body)
            .map_err(|e| format!("write PROJECT.md: {e}"))?;
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

/// Remove the K2SO-managed block (`<!-- K2SO:BEGIN -->` …
/// `<!-- K2SO:END -->`) from a string. Used so files containing
/// *only* K2SO-injected content read as empty during scan, and so
/// adopting a file's body doesn't carry K2SO's own markers into
/// the new PROJECT.md.
pub fn strip_k2so_managed_block(content: &str) -> String {
    const BEGIN: &str = "<!-- K2SO:BEGIN -->";
    const END: &str = "<!-- K2SO:END -->";
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
    let Ok(raw) = fs::read_to_string(project_md) else { return false };
    let stripped = strip_frontmatter(&raw).trim().to_string();
    stripped.lines().any(|line| {
        let t = line.trim();
        !t.is_empty()
            && !t.starts_with('#')
            && !t.starts_with("<!--")
            && !t.starts_with('>')
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
        let dir = std::env::temp_dir().join(format!(
            "k2so-onboarding-test-{}",
            uuid::Uuid::new_v4()
        ));
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
        assert!(harness_fanout_enabled(&path), "sanity: opt-in marker enables fan-out");
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
            "<!-- K2SO:BEGIN -->\nfoo\n<!-- K2SO:END -->\n",
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
}
