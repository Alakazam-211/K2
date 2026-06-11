//! K2 Connect "Clone to" — the pure bundle engine.
//!
//! An agent's full state lives in **three** filesystem locations, and a
//! one-click migration to a remote host has to gather all three, scrub
//! the secrets, drop the bulk, and package the result so the remote can
//! reconstruct a resumable agent at recomputed paths. This module is the
//! shared CORE that does the gathering + scrubbing + packaging. It has
//! NO HTTP, NO Tauri, NO daemon — it is a pure, unit-testable library
//! consumed by both the high-bar "Clone to" push (P2) and the
//! save-locally README fallback (P3).
//!
//! ## The three locations (see PRD `k2-connect-clone-to.md`)
//! For a project at `PROJECT` with
//! `SLUG = chat_history::claude_project_hash(PROJECT)`:
//! 1. **Workspace dir** — the `PROJECT` tree (`.k2so/`, project-local
//!    `.claude/`, config/persona/tools/docs), minus excludes + scrubbed
//!    secrets.
//! 2. **Durable memory** — the entire `<home>/.claude/projects/<SLUG>/
//!    memory/` directory (`MEMORY.md` + every `*.md`).
//! 3. **Live session(s)** — `<session-id>.jsonl` directly under
//!    `<home>/.claude/projects/<SLUG>/`. Default: the newest-mtime one
//!    (the conversation you're in). Opt-in: all of them. Worktree
//!    variants live under `<SLUG>-<branch>/`.
//!
//! ## Pipeline
//! `inventory()` resolves PROJECT + SLUG, walks the three locations
//! (honoring `.gitignore`/`.k2soignore` + the bulk skip-list via the
//! `ignore` crate), runs the secret classifier, and returns a structured
//! [`CloneInventory`]. `CloneInventory::manifest()` produces the
//! serializable [`CloneManifest`] (entry list + re-supply report + source
//! slug + metadata). `build_bundle()` tar+gz's the included files under
//! per-class subdirs (`workspace/`, `memory/`, `sessions/`) alongside
//! `manifest.json`.

mod bundle;
mod inventory;
mod repair;
mod scrub;
mod settings;
mod unpack;

#[cfg(test)]
mod tests;

pub use bundle::{build_bundle, read_manifest_from_bundle};
pub use inventory::inventory;
pub use repair::{
    migrate_legacy_slug_dirs, repair_cloned_session_paths, ProjectRepair, RepairReport,
    SlugMigration, SlugMigrationReport,
};
pub use settings::{capture_settings, WorkspaceSettings};
pub use unpack::{unpack_bundle, UnpackResult};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Which of the three state locations a bundled file belongs to. Drives
/// where the remote unpack route places the file: `Workspace` files land
/// at `DEST_PATH`; `Memory` + `Session` land under the remote-recomputed
/// `<home>/.claude/projects/<remote-slug>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DestinationClass {
    /// A file from the PROJECT workspace tree.
    Workspace,
    /// A file from `~/.claude/projects/<slug>/memory/`.
    Memory,
    /// A `<session-id>.jsonl` under `~/.claude/projects/<slug>/` (or a
    /// `<slug>-<branch>/` worktree variant).
    Session,
}

/// Options controlling what `inventory()` gathers.
#[derive(Debug, Clone)]
pub struct CloneOptions {
    /// Include EVERY `<session-id>.jsonl` under the slug dir (and worktree
    /// `<slug>-<branch>/` variants), not just the newest-mtime live one.
    /// Default `true` — Clone-to is a true migration tool, so ALL chat
    /// history travels by default (GitHub #21). Set `false` to slim the
    /// bundle down to just the newest-mtime live session.
    pub include_all_history: bool,

    /// Carry secrets over the (encrypted) link instead of scrubbing them.
    /// Default `false` — secrets are excluded from the bundle and recorded
    /// in the re-supply list. `~/.claude/.credentials.json` is excluded
    /// regardless (it is never even enumerated).
    pub carry_secrets: bool,

    /// Home directory override for hermetic resolution of the
    /// `~/.claude/projects/<slug>/` memory + session locations. `None`
    /// uses `dirs::home_dir()`. Tests set this to a temp dir so the
    /// real home is never touched.
    pub home_override: Option<PathBuf>,
}

impl Default for CloneOptions {
    fn default() -> Self {
        Self {
            include_all_history: true,
            carry_secrets: false,
            home_override: None,
        }
    }
}

/// One file selected for the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryEntry {
    /// Absolute path on the source machine. Not serialized into the
    /// manifest (the bundle is portable) — see [`ManifestEntry`].
    #[serde(skip)]
    pub abs_path: PathBuf,
    /// Path relative to the location root, used as the destination key
    /// inside the per-class subdir. For `Workspace` this is relative to
    /// `PROJECT`; for `Memory` relative to `.../<slug>/memory/`; for
    /// `Session` relative to `.../<slug>/` (so worktree variants keep
    /// their `<slug>-<branch>/<id>.jsonl` shape).
    pub rel_path: String,
    /// Destination class.
    pub class: DestinationClass,
}

/// The structured result of an `inventory()` pass.
#[derive(Debug, Clone)]
pub struct CloneInventory {
    /// Absolute, resolved PROJECT path.
    pub project_path: String,
    /// `claude_project_hash(project_path)` — the SOURCE slug. The remote
    /// recomputes its own slug from the dest path; this is never copied.
    pub slug: String,
    /// Every file selected for the bundle, across all three classes.
    pub entries: Vec<InventoryEntry>,
    /// Relative paths of files scrubbed as secrets (empty when
    /// `carry_secrets` is set). Surfaced verbatim in the re-supply report.
    pub scrubbed_secrets: Vec<String>,
}

/// A manifest entry — the portable (relative path + class) pair written
/// into `manifest.json`. Absolute paths are intentionally dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub rel_path: String,
    pub class: DestinationClass,
}

/// The external re-auth checklist shipped in every bundle. These are the
/// things a bundle physically CANNOT carry (or must not), so the remote
/// operator re-supplies them by hand. Static today; structured so the UI
/// can render it and future work can make items workspace-specific.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReauthChecklist {
    /// Scrubbed (or carried) secret files, by relative path.
    pub secret_paths: Vec<String>,
    /// Free-form re-auth items (MCP creds, OS-permission-gated tooling,
    /// "the remote needs its own Claude Code auth", …).
    pub items: Vec<String>,
}

/// The serializable bundle manifest — written as `manifest.json` at the
/// archive root and read back by the remote unpack route (P2) or the
/// human following the README (P3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneManifest {
    /// Manifest format version (bump on breaking layout changes).
    pub version: u32,
    /// The SOURCE slug. Informational — the remote recomputes its own.
    pub source_slug: String,
    /// Original (source) PROJECT path. Informational.
    pub source_project_path: String,
    /// RFC3339 creation timestamp (caller-supplied for determinism).
    pub created_at: String,
    /// Whether secrets were carried (`true`) or scrubbed (`false`).
    pub carry_secrets: bool,
    /// Whether all session history was included (`true`) or just the live
    /// session (`false`).
    pub include_all_history: bool,
    /// Every file in the bundle, with its destination class.
    pub entries: Vec<ManifestEntry>,
    /// The re-supply / re-auth checklist.
    pub reauth: ReauthChecklist,
    /// The source workspace's USER-meaningful K2 settings, read from the
    /// source projects DB row. The remote unpack route applies these to
    /// the freshly registered project so it appears configured + ready to
    /// resume. `None` when the source path isn't a registered project (we
    /// still bundle the files; the remote registers with defaults).
    /// Machine-specific fields (local `id`, absolute `path`,
    /// `focus_group_id`) are intentionally NOT carried — the remote gets a
    /// fresh id + its own path.
    #[serde(default)]
    pub settings: Option<WorkspaceSettings>,
}

impl CloneInventory {
    /// Build the serializable [`CloneManifest`].
    ///
    /// `created_at` is supplied by the caller (RFC3339) so the manifest is
    /// deterministic and the engine stays free of ambient clock calls —
    /// e.g. `chrono::Utc::now().to_rfc3339()`.
    ///
    /// `settings` is the source workspace's captured K2 settings (via
    /// [`capture_settings`]) or `None` when the source path isn't a
    /// registered project.
    pub fn manifest(
        &self,
        opts: &CloneOptions,
        created_at: String,
        settings: Option<WorkspaceSettings>,
    ) -> CloneManifest {
        let entries = self
            .entries
            .iter()
            .map(|e| ManifestEntry {
                rel_path: e.rel_path.clone(),
                class: e.class,
            })
            .collect();

        CloneManifest {
            version: 1,
            source_slug: self.slug.clone(),
            source_project_path: self.project_path.clone(),
            created_at,
            carry_secrets: opts.carry_secrets,
            include_all_history: opts.include_all_history,
            entries,
            reauth: ReauthChecklist {
                secret_paths: self.scrubbed_secrets.clone(),
                items: default_reauth_items(),
            },
            settings,
        }
    }
}

/// The standing external re-auth checklist (PRD: MCP creds; OS-permission
/// -gated tooling; the remote's own Claude Code auth). The bundle can't
/// carry these, so they're always listed for the operator.
fn default_reauth_items() -> Vec<String> {
    vec![
        "MCP server credentials/config — re-supply on the remote (the bundle \
         does not carry MCP auth tokens)."
            .to_string(),
        "OS-permission-gated tooling — re-grant on the remote (e.g. macOS Full \
         Disk Access + Automation permissions)."
            .to_string(),
        "Claude Code auth — the remote authenticates itself; \
         ~/.claude/.credentials.json is never migrated."
            .to_string(),
    ]
}
