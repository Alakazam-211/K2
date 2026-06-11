//! Canonical-agents core: per-harness state detection + the
//! deterministic safety net (the ONLY destructive surface).
//!
//! This module backs the **K2 Canonical Agent** skill (PRD
//! `.k2so/prds/k2-canonical-agents.md`, §5 + §6). It does two jobs and
//! no more:
//!
//! 1. **Detect** per-harness canonical state without mutating anything
//!    ([`detect_canonical_state`], §5.2). Per-file probe over the 6
//!    [`HARNESS_WORKSPACE_FILES`] plus the extended
//!    [`HARNESS_PROBE_TARGETS`] list via `fs::symlink_metadata` +
//!    `read_link` + the `k2so_generated:` signature. NO directory-level
//!    canonicalization — scope is per-file.
//!
//! 2. **Safely persist / unwind** agent-authored or generated content
//!    ([`run_canonical_setup`], [`unwind_canonical`],
//!    [`persist_agent_md`], §6). The core's job is narrow and
//!    irreversible-only: back up every touched original to
//!    `.k2so/backups/<ISO-ts>/<relpath>` BEFORE any write, record a
//!    `manifest.json` (original path, backup path, pre-content hash,
//!    action, target harness), write COPIES (not symlinks) stamped with
//!    a `k2so_generated:` signature + an "edit `.k2so/...` instead"
//!    header, all via [`crate::fs_atomic::atomic_write_str`]. Default is
//!    **dry-run**; writes require an explicit `confirm`. A best-effort
//!    preflight refuses when a live agent session is reading the
//!    targets (offers `force`).
//!
//! **Model A (PRD §5.1):** `.k2so/agent/AGENT.md` + `.k2so/PROJECT.md`
//! are the canonical source of truth; per-harness files
//! (`CLAUDE.md`, `GEMINI.md`, …) are generated mirrors. The agent
//! supplies the merged body text; this core never authors content — it
//! only does the backup + atomic-write + manifest around the bytes the
//! agent hands it, so role re-runs and canonical setups are
//! byte-reversible.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs_atomic::atomic_write_str;
use crate::workspace::agent_identity::workspace_agent_md_path;
use crate::workspace::harness::HARNESS_WORKSPACE_FILES;
use crate::workspace::scheduler::is_agent_locked;

/// The signature line stamped into every K2SO-generated copy so the
/// probe can tell a managed mirror apart from a user-authored file. Same
/// token the cursor-mdc writer + the add-workspace preview already use
/// (`harness.rs`), kept identical so detection is uniform across paths.
pub const K2SO_GENERATED_SIGNATURE: &str = "k2so_generated: true";

/// Extended per-file probe targets, mirroring
/// `onboarding::HARNESS_PROBES` (PRD §5.3 / `onboarding.rs:130`). These
/// are the harness discovery paths beyond the 6
/// [`HARNESS_WORKSPACE_FILES`] root list. `(relative_path, label)`.
pub const HARNESS_PROBE_TARGETS: &[(&str, &str)] = &[
    ("AGENTS.md", "Multi-harness AGENTS.md"),
    (".opencode/agent/k2so.md", "OpenCode"),
    (".pi/skills/k2so/SKILL.md", "Pi"),
    (".github/copilot-instructions.md", "GitHub Copilot"),
];

/// Per-harness canonical state for a single probed file (PRD §5.2).
///
/// Per-file, never directory-level. A workspace can have Claude
/// `Unified` and Gemini `Unmanaged` independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessFileState {
    /// File is canonicalized — either a K2SO-generated copy (signature
    /// present) or a symlink into the canonical SKILL.md.
    Unified { form: UnifiedForm },
    /// File exists and has user content but is NOT K2SO-managed (no
    /// signature, not a symlink).
    Unmanaged,
    /// No such file at this path — nothing to do here.
    Skipped,
}

/// How a `Unified` file is materialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnifiedForm {
    /// A real file stamped with the `k2so_generated:` signature (the new
    /// copy-based flow — PRD §6).
    Copy,
    /// A symlink (legacy already-unified workspaces — PRD §6 retains
    /// symlinks as legacy-compat only).
    Symlink,
}

/// One harness file's detected state, returned by
/// [`detect_canonical_state`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessProbe {
    /// Path relative to the workspace root (e.g. `"CLAUDE.md"`).
    pub relative_path: String,
    /// Human-readable harness label.
    pub label: String,
    /// The per-file canonical state.
    pub state: HarnessFileState,
}

/// Detect per-harness canonical state for every probe target WITHOUT
/// mutating anything (PRD §5.2). The K2 Canonical Agent skill runs this
/// first, summarizes it in plain language, then asks intent.
///
/// Probe order: the 6 [`HARNESS_WORKSPACE_FILES`] root list, then the
/// extended [`HARNESS_PROBE_TARGETS`]. Each entry is classified via
/// `fs::symlink_metadata` (symlink? → `Unified{Symlink}`), then the
/// `k2so_generated:` signature (present in a real file →
/// `Unified{Copy}`), else `Unmanaged` (real file, no signature) or
/// `Skipped` (no file).
///
/// This is also the per-harness state source resolving PRD §13.1: the
/// `harness_fanout_enabled` bool is the blanket gate; per-harness
/// enabled state is read here from the on-disk markers/signatures.
pub fn detect_canonical_state(project_path: &str) -> Vec<HarnessProbe> {
    let root = PathBuf::from(project_path);
    let mut probes: Vec<HarnessProbe> = Vec::new();

    // Root list (6 HARNESS_WORKSPACE_FILES). Labels mirror the
    // onboarding probe table where the names overlap.
    let root_labels: &[(&str, &str)] = &[
        ("CLAUDE.md", "Claude Code"),
        ("GEMINI.md", "Gemini"),
        ("AGENT.md", "Agent.md (singular)"),
        (".goosehints", "Goose"),
        ("SKILL.md", "Root SKILL.md"),
        (".cursor/rules/k2so.mdc", "Cursor rule"),
    ];
    debug_assert_eq!(
        root_labels.len(),
        HARNESS_WORKSPACE_FILES.len(),
        "root probe labels must cover every HARNESS_WORKSPACE_FILES entry",
    );

    for (rel, label) in root_labels {
        probes.push(HarnessProbe {
            relative_path: (*rel).to_string(),
            label: (*label).to_string(),
            state: probe_file_state(&root.join(rel)),
        });
    }
    for (rel, label) in HARNESS_PROBE_TARGETS {
        probes.push(HarnessProbe {
            relative_path: (*rel).to_string(),
            label: (*label).to_string(),
            state: probe_file_state(&root.join(rel)),
        });
    }

    probes
}

/// Classify a single path's canonical state (per-file, no directory
/// recursion).
fn probe_file_state(abs: &Path) -> HarnessFileState {
    let Ok(meta) = fs::symlink_metadata(abs) else {
        // No file (or unreadable) at this path.
        return HarnessFileState::Skipped;
    };
    if meta.file_type().is_symlink() {
        return HarnessFileState::Unified {
            form: UnifiedForm::Symlink,
        };
    }
    if !meta.file_type().is_file() {
        // A directory or other non-file at a harness path — treat as
        // Skipped (we don't manage it).
        return HarnessFileState::Skipped;
    }
    match fs::read_to_string(abs) {
        Ok(content) if content.contains(K2SO_GENERATED_SIGNATURE) => HarnessFileState::Unified {
            form: UnifiedForm::Copy,
        },
        Ok(_) => HarnessFileState::Unmanaged,
        // Real file we can't read as UTF-8 — still user-owned, not ours.
        Err(_) => HarnessFileState::Unmanaged,
    }
}

// ══════════════════════════════════════════════════════════════════════
// Safety net (PRD §6) — the ONLY destructive surface
// ══════════════════════════════════════════════════════════════════════

/// One recorded action in a setup `manifest.json`. Makes the write
/// byte-reversible and lets re-runs detect "already done."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Workspace-root-relative path of the file written.
    pub relative_path: String,
    /// Target harness label (e.g. `"Claude Code"`), or `""` for the
    /// canonical AGENT.md itself.
    pub harness: String,
    /// What the safety net did: `"created"` (no prior file → fresh
    /// write) or `"backed_up_then_wrote"` (a prior file was backed up
    /// first).
    pub action: String,
    /// Where the original was backed up to (root-relative), or `None`
    /// when the file was created fresh (nothing to back up).
    pub backup_relative_path: Option<String>,
    /// Content hash of the pre-existing file (hex), or `None` when none
    /// existed. Used to detect "already done" + verify a clean unwind.
    pub pre_content_hash: Option<String>,
}

/// The full manifest written to `.k2so/backups/<ts>/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupManifest {
    /// ISO-8601 UTC timestamp directory name under `.k2so/backups/`.
    pub timestamp: String,
    /// Per-file entries.
    pub entries: Vec<ManifestEntry>,
}

/// Options for [`run_canonical_setup`] (PRD §6).
#[derive(Debug, Clone, Default)]
pub struct CanonicalSetupOpts {
    /// Workspace-root-relative target paths to write, each mapped to the
    /// agent-authored merged body. Per-harness selection lives here —
    /// only listed targets are touched (PRD §5.3 per-LLM selection).
    pub merged_bodies: Vec<(String, String)>,
    /// When true, plan only — no file is touched. Default invocation.
    pub dry_run: bool,
    /// Must be true for any write to happen. `dry_run` wins if both set.
    pub confirm: bool,
    /// Bypass the running-agent preflight refusal (with a loud warning
    /// at the call site). Best-effort safety only.
    pub force: bool,
}

/// Outcome of a setup run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalSetupOutcome {
    /// True when this was a dry-run (nothing written).
    pub dry_run: bool,
    /// The manifest. On dry-run it describes what WOULD happen (with
    /// `backup_relative_path` populated as the intended path) but no
    /// files were touched.
    pub manifest: SetupManifest,
    /// Root-relative path the manifest.json was written to (None on
    /// dry-run).
    pub manifest_path: Option<String>,
}

/// The header prepended to every K2SO-generated copy (kind (a)/mirror
/// files), carrying the signature the probe keys on + the "edit
/// `.k2so/...` instead" guidance.
fn generated_header() -> String {
    format!(
        "<!--\n{sig}\nRegenerated by K2SO — do not edit this file directly.\nEdit the canonical source under `.k2so/` (AGENT.md / PROJECT.md) instead;\nK2SO re-derives this mirror from there.\n-->\n\n",
        sig = K2SO_GENERATED_SIGNATURE,
    )
}

/// Run the deterministic canonical setup (PRD §6). Default is dry-run;
/// a real write requires `opts.confirm == true && opts.dry_run == false`.
///
/// For every `(relative_path, body)` in `opts.merged_bodies`:
///   1. If a prior file exists, back it up to
///      `.k2so/backups/<ts>/<relpath>` BEFORE writing (atomic).
///   2. Write the body as a COPY (real file) stamped with the
///      `k2so_generated:` header + signature (atomic).
///   3. Record a manifest entry.
/// Then write `manifest.json` alongside the backups.
///
/// **Preflight (PRD §6):** unless `opts.force`, a best-effort
/// running-agent check refuses (and names the agent) when a live
/// session is using the workspace.
pub fn run_canonical_setup(
    project_path: &str,
    opts: CanonicalSetupOpts,
) -> Result<CanonicalSetupOutcome, String> {
    let root = PathBuf::from(project_path);
    let will_write = opts.confirm && !opts.dry_run;

    // Preflight running-agent check — only for real writes.
    if will_write && !opts.force {
        if let Some(agent) = running_agent_name(project_path) {
            return Err(format!(
                "refusing to write: a live agent session ({agent}) is active in this workspace. \
                 Stop it, or re-run with force to override (this can clobber files the agent is reading)."
            ));
        }
    }

    let timestamp = iso_timestamp();
    let backups_rel_dir = format!(".k2so/backups/{}", timestamp);
    let backups_dir = root.join(&backups_rel_dir);

    let mut entries: Vec<ManifestEntry> = Vec::new();

    for (rel, body) in &opts.merged_bodies {
        let target = root.join(rel);
        let prior = read_prior(&target);

        let (action, backup_rel, pre_hash) = match &prior {
            Some(bytes) => {
                let backup_rel = format!("{}/{}", backups_rel_dir, rel);
                let hash = crate::skills::version::skill_checksum_hex(bytes);
                ("backed_up_then_wrote".to_string(), Some(backup_rel), Some(hash))
            }
            None => ("created".to_string(), None, None),
        };

        if will_write {
            // 1. Back up the original BEFORE touching the target.
            if let (Some(bytes), Some(backup_rel)) = (&prior, &backup_rel) {
                let backup_path = root.join(backup_rel);
                crate::fs_atomic::atomic_write(&backup_path, bytes)
                    .map_err(|e| format!("backup {rel}: {e}"))?;
            }
            // 2. Write the generated copy (header + signature + body).
            let stamped = format!("{}{}", generated_header(), body);
            atomic_write_str(&target, &stamped)
                .map_err(|e| format!("write {rel}: {e}"))?;
        }

        entries.push(ManifestEntry {
            relative_path: rel.clone(),
            harness: harness_label_for(rel),
            action,
            backup_relative_path: backup_rel,
            pre_content_hash: pre_hash,
        });
    }

    let manifest = SetupManifest { timestamp, entries };

    if !will_write {
        return Ok(CanonicalSetupOutcome {
            dry_run: true,
            manifest,
            manifest_path: None,
        });
    }

    // Write the manifest alongside the backups.
    let manifest_rel = format!("{}/manifest.json", backups_rel_dir);
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize manifest: {e}"))?;
    let _ = fs::create_dir_all(&backups_dir);
    atomic_write_str(&root.join(&manifest_rel), &manifest_json)
        .map_err(|e| format!("write manifest: {e}"))?;

    Ok(CanonicalSetupOutcome {
        dry_run: false,
        manifest,
        manifest_path: Some(manifest_rel),
    })
}

/// Persist the agent's organic AGENT.md merge (PRD §6, kind (b)).
///
/// The role skills (Workspace Manager / K2 Agent) and the canonical
/// merge produce a merged `.k2so/agent/AGENT.md` body; this is the
/// safety-net write around it — backup + atomic + manifest — so the
/// integration is byte-reversible (resolves PRD §13.6). Reuses the full
/// [`run_canonical_setup`] machinery so role re-runs are reversible
/// exactly like canonical.
///
/// Writes the *raw* merged body to `.k2so/agent/AGENT.md` (NOT stamped
/// with the generated-mirror header — AGENT.md is the canonical source
/// of truth, authored by the agent, not a generated mirror). Therefore
/// this does its own backup + atomic-write + single-entry manifest
/// rather than routing through the mirror-stamping path.
pub fn persist_agent_md(
    project_path: &str,
    merged_body: &str,
    opts: PersistOpts,
) -> Result<CanonicalSetupOutcome, String> {
    let root = PathBuf::from(project_path);
    let agent_md = workspace_agent_md_path(project_path);
    let rel = relpath_of(&root, &agent_md);

    let will_write = opts.confirm && !opts.dry_run;
    if will_write && !opts.force {
        if let Some(agent) = running_agent_name(project_path) {
            return Err(format!(
                "refusing to write AGENT.md: a live agent session ({agent}) is active. \
                 Stop it, or re-run with force."
            ));
        }
    }

    let timestamp = iso_timestamp();
    let backups_rel_dir = format!(".k2so/backups/{}", timestamp);
    let prior = read_prior(&agent_md);

    let (action, backup_rel, pre_hash) = match &prior {
        Some(bytes) => {
            let backup_rel = format!("{}/{}", backups_rel_dir, rel);
            let hash = crate::skills::version::skill_checksum_hex(bytes);
            ("backed_up_then_wrote".to_string(), Some(backup_rel), Some(hash))
        }
        None => ("created".to_string(), None, None),
    };

    if will_write {
        if let (Some(bytes), Some(backup_rel)) = (&prior, &backup_rel) {
            crate::fs_atomic::atomic_write(&root.join(backup_rel), bytes)
                .map_err(|e| format!("backup AGENT.md: {e}"))?;
        }
        // AGENT.md is the canonical SOURCE (Model A) — write the merged
        // body verbatim, NO generated-mirror header.
        atomic_write_str(&agent_md, merged_body)
            .map_err(|e| format!("write AGENT.md: {e}"))?;
    }

    let manifest = SetupManifest {
        timestamp,
        entries: vec![ManifestEntry {
            relative_path: rel,
            harness: String::new(),
            action,
            backup_relative_path: backup_rel,
            pre_content_hash: pre_hash,
        }],
    };

    if !will_write {
        return Ok(CanonicalSetupOutcome {
            dry_run: true,
            manifest,
            manifest_path: None,
        });
    }

    let manifest_rel = format!("{}/manifest.json", backups_rel_dir);
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize manifest: {e}"))?;
    let _ = fs::create_dir_all(root.join(&backups_rel_dir));
    atomic_write_str(&root.join(&manifest_rel), &manifest_json)
        .map_err(|e| format!("write manifest: {e}"))?;

    Ok(CanonicalSetupOutcome {
        dry_run: false,
        manifest,
        manifest_path: Some(manifest_rel),
    })
}

/// Options for [`persist_agent_md`]. Mirror of the relevant
/// [`CanonicalSetupOpts`] fields.
#[derive(Debug, Clone, Default)]
pub struct PersistOpts {
    pub dry_run: bool,
    pub confirm: bool,
    pub force: bool,
}

/// Unwind a prior canonical setup from its manifest (PRD §6, generalizes
/// `teardown.rs` RestoreOriginal to the copy+manifest model).
///
/// Per manifest entry:
///   - if `backup_relative_path` is set → restore that backup over the
///     generated copy (atomic).
///   - else (the file was created fresh) → trash the generated file.
/// `.k2so/` (including the backups + manifest) is left intact.
///
/// Honors the same dry-run / confirm / force contract as setup so a
/// "show me the exact undo" preview never mutates anything.
pub fn unwind_canonical(
    project_path: &str,
    manifest: &SetupManifest,
    opts: PersistOpts,
) -> Result<Vec<UnwindAction>, String> {
    let root = PathBuf::from(project_path);
    let will_write = opts.confirm && !opts.dry_run;

    if will_write && !opts.force {
        if let Some(agent) = running_agent_name(project_path) {
            return Err(format!(
                "refusing to unwind: a live agent session ({agent}) is active. \
                 Stop it, or re-run with force."
            ));
        }
    }

    let mut actions: Vec<UnwindAction> = Vec::new();

    for entry in &manifest.entries {
        let target = root.join(&entry.relative_path);
        match &entry.backup_relative_path {
            Some(backup_rel) => {
                let backup_path = root.join(backup_rel);
                let bytes = fs::read(&backup_path)
                    .map_err(|e| format!("read backup {backup_rel}: {e}"))?;
                if will_write {
                    atomic_write_str_or_bytes(&target, &bytes)
                        .map_err(|e| format!("restore {}: {e}", entry.relative_path))?;
                }
                actions.push(UnwindAction {
                    relative_path: entry.relative_path.clone(),
                    action: "restored".to_string(),
                    detail: format!("restored original from {}", backup_rel),
                });
            }
            None => {
                if will_write && target.exists() {
                    crate::safe_delete_scratch::scratch_safe_trash(&target)
                        .map_err(|e| format!("trash {}: {e}", entry.relative_path))?;
                }
                actions.push(UnwindAction {
                    relative_path: entry.relative_path.clone(),
                    action: "removed".to_string(),
                    detail: "no prior original — K2SO created this file fresh; sent to Trash"
                        .to_string(),
                });
            }
        }
    }

    Ok(actions)
}

/// One step the unwind took (or would take, on dry-run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnwindAction {
    pub relative_path: String,
    /// `"restored"` or `"removed"`.
    pub action: String,
    pub detail: String,
}

/// Read the latest manifest for a workspace (highest ISO timestamp
/// directory under `.k2so/backups/`). Used by the K2 Canonical Agent
/// manage/undo mode to find the exact undo.
pub fn latest_manifest(project_path: &str) -> Option<SetupManifest> {
    let backups_root = crate::workspace_dot_dir(project_path).join("backups");
    let entries = fs::read_dir(&backups_root).ok()?;
    let mut best: Option<(String, PathBuf)> = None;
    for entry in entries.flatten() {
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let manifest = entry.path().join("manifest.json");
        if !manifest.is_file() {
            continue;
        }
        match &best {
            Some((best_name, _)) if name <= *best_name => {}
            _ => best = Some((name, manifest)),
        }
    }
    let (_, manifest_path) = best?;
    let raw = fs::read_to_string(&manifest_path).ok()?;
    serde_json::from_str(&raw).ok()
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Best-effort running-agent name for the preflight refusal. Returns the
/// primary agent's name when a live session is detected, else None.
fn running_agent_name(project_path: &str) -> Option<String> {
    let primary =
        crate::workspace::agent_identity::find_primary_agent(project_path)?;
    if is_agent_locked(project_path, &primary) {
        Some(primary)
    } else {
        None
    }
}

/// Read a file's prior bytes if it exists as a real file (not a symlink
/// — a managed symlink has no "original" worth backing up; it points at
/// the canonical SKILL.md). Returns None for symlinks + missing files.
fn read_prior(target: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(target).ok()?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return None;
    }
    fs::read(target).ok()
}

/// Compute a root-relative path string for a path inside the workspace.
fn relpath_of(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| abs.to_string_lossy().to_string())
}

/// Atomic write of raw bytes (the restore path may carry non-UTF-8
/// originals).
fn atomic_write_str_or_bytes(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    crate::fs_atomic::atomic_write(target, bytes)
}

/// Map a known harness root/probe path to its label for the manifest.
fn harness_label_for(rel: &str) -> String {
    let known: &[(&str, &str)] = &[
        ("CLAUDE.md", "Claude Code"),
        ("GEMINI.md", "Gemini"),
        ("AGENT.md", "Agent.md (singular)"),
        (".goosehints", "Goose"),
        ("SKILL.md", "Root SKILL.md"),
        (".cursor/rules/k2so.mdc", "Cursor rule"),
        ("AGENTS.md", "Multi-harness AGENTS.md"),
        (".opencode/agent/k2so.md", "OpenCode"),
        (".pi/skills/k2so/SKILL.md", "Pi"),
        (".github/copilot-instructions.md", "GitHub Copilot"),
    ];
    known
        .iter()
        .find(|(k, _)| *k == rel)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| rel.to_string())
}

/// Filesystem-safe ISO-8601 UTC timestamp (`YYYY-MM-DDTHH-MM-SSZ`) for
/// the backups directory name. Colons are replaced with dashes so the
/// path is portable. Derived from the wall clock; uniqueness across
/// rapid re-runs is the caller's concern (re-runs detect "already done"
/// via the manifest hashes).
fn iso_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil-from-days (Howard Hinnant's algorithm) — no chrono dep.
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as i64;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}Z",
        year, m, d, hh, mm, ss
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn scratch_project() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-canonical-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        fs::create_dir_all(dir.join(".k2so")).unwrap();
        dir
    }

    // ── detect_canonical_state ──────────────────────────────────────

    #[test]
    fn detect_reports_skipped_for_clean_workspace() {
        let proj = scratch_project();
        let probes = detect_canonical_state(proj.to_str().unwrap());
        assert!(!probes.is_empty(), "must probe every harness target");
        for p in &probes {
            assert_eq!(
                p.state,
                HarnessFileState::Skipped,
                "{} should be Skipped on a clean workspace, got {:?}",
                p.relative_path,
                p.state,
            );
        }
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_distinguishes_copy_symlink_unmanaged_per_harness() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();

        // Claude: a K2SO-generated copy (signature present).
        fs::write(
            proj.join("CLAUDE.md"),
            format!("---\n{}\n---\n# managed\n", K2SO_GENERATED_SIGNATURE),
        )
        .unwrap();
        // Gemini: a user-authored file (no signature) → Unmanaged.
        fs::write(proj.join("GEMINI.md"), "# my own gemini notes\n").unwrap();
        // .goosehints: a symlink → Unified(Symlink).
        let real = proj.join(".k2so/skills/k2so/SKILL.md");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "canonical body").unwrap();
        std::os::unix::fs::symlink(&real, proj.join(".goosehints")).unwrap();

        let probes = detect_canonical_state(path);
        let by_path: std::collections::HashMap<_, _> =
            probes.iter().map(|p| (p.relative_path.as_str(), &p.state)).collect();

        assert_eq!(
            by_path["CLAUDE.md"],
            &HarnessFileState::Unified { form: UnifiedForm::Copy },
            "Claude with signature → Unified(Copy)",
        );
        assert_eq!(
            by_path["GEMINI.md"],
            &HarnessFileState::Unmanaged,
            "user-authored Gemini → Unmanaged",
        );
        assert_eq!(
            by_path[".goosehints"],
            &HarnessFileState::Unified { form: UnifiedForm::Symlink },
            "symlinked goosehints → Unified(Symlink)",
        );
        // AGENT.md untouched → Skipped — per-harness independence.
        assert_eq!(
            by_path["AGENT.md"],
            &HarnessFileState::Skipped,
            "untouched AGENT.md stays Skipped (per-harness independence)",
        );
        fs::remove_dir_all(&proj).ok();
    }

    // ── run_canonical_setup ─────────────────────────────────────────

    #[test]
    fn setup_dry_run_writes_nothing() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        let opts = CanonicalSetupOpts {
            merged_bodies: vec![("CLAUDE.md".to_string(), "# new body\n".to_string())],
            dry_run: true,
            confirm: true, // confirm is ignored under dry_run
            force: true,
        };
        let outcome = run_canonical_setup(path, opts).unwrap();
        assert!(outcome.dry_run);
        assert!(outcome.manifest_path.is_none());
        assert!(
            !proj.join("CLAUDE.md").exists(),
            "dry-run must not write the target file",
        );
        assert!(
            !proj.join(".k2so/backups").exists(),
            "dry-run must not create backups",
        );
        // The plan still enumerates the intended action.
        assert_eq!(outcome.manifest.entries.len(), 1);
        assert_eq!(outcome.manifest.entries[0].action, "created");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn setup_backs_up_original_then_writes_copy_with_signature() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        fs::write(proj.join("CLAUDE.md"), "# original user content\n").unwrap();

        let opts = CanonicalSetupOpts {
            merged_bodies: vec![("CLAUDE.md".to_string(), "# merged canonical body\n".to_string())],
            dry_run: false,
            confirm: true,
            force: true,
        };
        let outcome = run_canonical_setup(path, opts).unwrap();
        assert!(!outcome.dry_run);

        // Target now holds the stamped copy.
        let written = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        assert!(written.contains(K2SO_GENERATED_SIGNATURE), "copy must carry the signature");
        assert!(written.contains("# merged canonical body"), "copy must carry the body");
        assert!(written.contains("edit") || written.contains("Edit"), "copy must carry the edit-elsewhere header");

        // Original backed up byte-identical.
        let entry = &outcome.manifest.entries[0];
        assert_eq!(entry.action, "backed_up_then_wrote");
        let backup_rel = entry.backup_relative_path.as_ref().unwrap();
        let backed = fs::read_to_string(proj.join(backup_rel)).unwrap();
        assert_eq!(backed, "# original user content\n", "backup must be byte-identical to the original");

        // manifest.json on disk.
        let manifest_path = outcome.manifest_path.unwrap();
        assert!(proj.join(&manifest_path).exists(), "manifest.json must be written");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn setup_then_unwind_round_trips_to_byte_identical_original() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        let original = "# original CLAUDE.md\n\nUser's hand-written notes.\n";
        fs::write(proj.join("CLAUDE.md"), original).unwrap();

        let outcome = run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("CLAUDE.md".to_string(), "# canonical\n".to_string())],
                dry_run: false,
                confirm: true,
                force: true,
            },
        )
        .unwrap();
        // File is now the generated copy, not the original.
        assert_ne!(fs::read_to_string(proj.join("CLAUDE.md")).unwrap(), original);

        // Unwind from the manifest → byte-identical original restored.
        let actions = unwind_canonical(
            path,
            &outcome.manifest,
            PersistOpts { dry_run: false, confirm: true, force: true },
        )
        .unwrap();
        assert_eq!(actions[0].action, "restored");
        assert_eq!(
            fs::read_to_string(proj.join("CLAUDE.md")).unwrap(),
            original,
            "unwind must restore the byte-identical original",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn unwind_trashes_files_created_fresh() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        // No prior GEMINI.md — setup creates it fresh.
        let outcome = run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("GEMINI.md".to_string(), "# fresh\n".to_string())],
                dry_run: false,
                confirm: true,
                force: true,
            },
        )
        .unwrap();
        assert_eq!(outcome.manifest.entries[0].action, "created");
        assert!(proj.join("GEMINI.md").exists());

        unwind_canonical(
            path,
            &outcome.manifest,
            PersistOpts { dry_run: false, confirm: true, force: true },
        )
        .unwrap();
        assert!(
            !proj.join("GEMINI.md").exists(),
            "a file K2SO created fresh must be removed on unwind",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn setup_idempotent_rerun_no_clobber_of_backup() {
        // Re-running setup over an already-canonicalized file must not
        // lose the TRUE original: the second run's "prior" is the
        // generated copy, but the manifest hash lets a caller detect
        // "already done." Here we assert the first backup still holds the
        // real original after a second run writes a fresh backup of the
        // copy.
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        let original = "# real original\n";
        fs::write(proj.join("CLAUDE.md"), original).unwrap();

        let first = run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("CLAUDE.md".to_string(), "# v1\n".to_string())],
                dry_run: false,
                confirm: true,
                force: true,
            },
        )
        .unwrap();
        let first_backup = first.manifest.entries[0].backup_relative_path.clone().unwrap();
        // The first backup is the real original.
        assert_eq!(fs::read_to_string(proj.join(&first_backup)).unwrap(), original);

        // Sleep enough to roll the ISO-second so the second run lands in
        // a distinct backups dir (timestamps are per-second).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _second = run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("CLAUDE.md".to_string(), "# v2\n".to_string())],
                dry_run: false,
                confirm: true,
                force: true,
            },
        )
        .unwrap();

        // The FIRST backup is untouched — still the true original.
        assert_eq!(
            fs::read_to_string(proj.join(&first_backup)).unwrap(),
            original,
            "re-run must not clobber the first backup (the true original)",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn setup_refuses_when_agent_session_live() {
        // Simulate a live session via the `.lock` file (the headless
        // fallback path of is_agent_locked for unregistered scratch
        // projects). Requires a primary agent to name.
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        // Scaffold the canonical primary agent so find_primary_agent
        // resolves a name.
        fs::create_dir_all(proj.join(".k2so/agent")).unwrap();
        fs::write(
            proj.join(".k2so/agent/AGENT.md"),
            "---\nname: pod-leader\ntype: manager\n---\n\nbody\n",
        )
        .unwrap();
        // Drop the `.lock` file at the agent work dir so is_agent_locked
        // returns true via the filesystem fallback.
        let lock = crate::workspace::scheduler::agent_work_dir(path, "pod-leader", "").join(".lock");
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(&lock, "now").unwrap();

        let err = run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("CLAUDE.md".to_string(), "# body\n".to_string())],
                dry_run: false,
                confirm: true,
                force: false, // no force → must refuse
            },
        )
        .unwrap_err();
        assert!(err.contains("pod-leader"), "refusal must name the live agent: {err}");
        assert!(!proj.join("CLAUDE.md").exists(), "must not write while refusing");

        // With force, the same call proceeds.
        let ok = run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("CLAUDE.md".to_string(), "# body\n".to_string())],
                dry_run: false,
                confirm: true,
                force: true,
            },
        );
        assert!(ok.is_ok(), "force must override the preflight refusal");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn per_harness_independence_only_listed_targets_touched() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        fs::write(proj.join("GEMINI.md"), "# gemini user content\n").unwrap();

        // Only canonicalize Claude; Gemini must be left exactly alone.
        run_canonical_setup(
            path,
            CanonicalSetupOpts {
                merged_bodies: vec![("CLAUDE.md".to_string(), "# claude\n".to_string())],
                dry_run: false,
                confirm: true,
                force: true,
            },
        )
        .unwrap();

        assert!(proj.join("CLAUDE.md").exists(), "Claude canonicalized");
        assert_eq!(
            fs::read_to_string(proj.join("GEMINI.md")).unwrap(),
            "# gemini user content\n",
            "Gemini must be byte-untouched (per-harness independence)",
        );
        fs::remove_dir_all(&proj).ok();
    }

    // ── persist_agent_md ────────────────────────────────────────────

    #[test]
    fn persist_agent_md_round_trips_byte_identical() {
        let proj = scratch_project();
        let path = proj.to_str().unwrap();
        fs::create_dir_all(proj.join(".k2so/agent")).unwrap();
        let original = "---\nname: x\n---\n\n# Existing persona the user wrote.\n";
        fs::write(proj.join(".k2so/agent/AGENT.md"), original).unwrap();

        let outcome = persist_agent_md(
            path,
            "---\nname: x\n---\n\n# Existing persona + organically merged role guidance.\n",
            PersistOpts { dry_run: false, confirm: true, force: true },
        )
        .unwrap();
        // AGENT.md is the canonical SOURCE — written verbatim, NO mirror header.
        let written = fs::read_to_string(proj.join(".k2so/agent/AGENT.md")).unwrap();
        assert!(!written.contains(K2SO_GENERATED_SIGNATURE), "AGENT.md must NOT be stamped as a generated mirror");
        assert!(written.contains("organically merged role guidance"));

        // Unwind restores the exact original.
        unwind_canonical(
            path,
            &outcome.manifest,
            PersistOpts { dry_run: false, confirm: true, force: true },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(proj.join(".k2so/agent/AGENT.md")).unwrap(),
            original,
            "persist_agent_md must be byte-reversible (PRD §13.6)",
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn iso_timestamp_is_path_safe_and_sorts() {
        let ts = iso_timestamp();
        assert!(!ts.contains(':'), "timestamp must be path-safe (no colons): {ts}");
        assert!(ts.ends_with('Z'), "timestamp must be UTC: {ts}");
        // Sorts lexicographically as chronological.
        assert_eq!(ts.len(), "2026-06-03T12-34-56Z".len());
    }
}
