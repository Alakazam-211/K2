//! K2 Connect "Clone to" daemon routes — the two ends of the
//! bundle → push → unpack pipeline (PRD `k2-connect-clone-to.md`, P2).
//!
//! - `POST /cli/clone/bundle` runs on the SOURCE machine: build a scrubbed
//!   tar.gz of the workspace + memory + live session, capture the source
//!   workspace's K2 settings, write it to a temp path, and return the path
//!   + a summary.
//! - `POST /cli/clone/unpack` runs on the DESTINATION machine: extract the
//!   bundle at `<dest_parent>/<source-name>` (collision-safe), place
//!   memory/sessions under the recomputed remote slug, REGISTER the folder
//!   as a project, and APPLY the manifest's settings so the migrated
//!   workspace appears fully configured.
//!
//! Both are gated by `token_ok` in the dispatcher (the same isolated-gate
//! pattern as `fs/upload-binary`), so these handlers assume the caller is
//! already authenticated.

use crate::cli_response::CliResponse;
use k2_core::clone;
use k2_core::clone::DestinationClass;
use k2_core::db;
use k2_core::db::schema::Project;
use k2_core::log_debug;
use k2_core::projects_ops as pops;
use serde::Deserialize;

#[derive(Deserialize)]
struct BundleBody {
    /// Absolute source workspace path on this machine.
    project_path: String,
    /// Slim the bundle down to the newest-mtime LIVE session only, instead
    /// of carrying EVERY session transcript. Absent (the default) ⇒ `false`
    /// ⇒ all history travels — Clone-to is a true migration tool out of the
    /// box (GitHub #21). The renderer's "Include all chat history" checkbox
    /// maps to `!live_only` (checked = carry all).
    #[serde(default)]
    live_only: bool,
    /// Carry secrets over the (encrypted) link instead of scrubbing them.
    #[serde(default)]
    carry_secrets: bool,
}

/// `POST /cli/clone/bundle` — build the bundle on the SOURCE daemon.
///
/// Inventories the three state locations, captures the source workspace's
/// K2 settings from the projects DB row, tar.gz's everything to
/// `~/.k2so/clone-tmp/<name>-<ts>.tar.gz`, and returns the bundle path + a
/// summary (entry count, scrubbed-secret count, byte size).
pub fn handle_clone_bundle(body: &[u8]) -> CliResponse {
    let b: BundleBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    let opts = clone::CloneOptions {
        include_all_history: !b.live_only,
        carry_secrets: b.carry_secrets,
        home_override: None,
    };

    let inv = match clone::inventory(&b.project_path, opts.clone()) {
        Ok(i) => i,
        Err(e) => return CliResponse::bad_request(format!("inventory failed: {e}")),
    };

    // Capture the source workspace's K2 settings (graceful: None if the
    // path isn't a registered project).
    let settings = {
        let db = db::shared();
        let conn = db.lock();
        match clone::capture_settings(&conn, &inv.project_path) {
            Ok(s) => s,
            Err(e) => return CliResponse::internal_error(format!("settings capture: {e}")),
        }
    };

    // Temp bundle path: ~/.k2so/clone-tmp/<name>-<ts>.tar.gz
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return CliResponse::internal_error("cannot resolve home directory"),
    };
    let tmp_dir = home.join(".k2").join("clone-tmp");
    if let Err(e) = std::fs::create_dir_all(&tmp_dir) {
        return CliResponse::internal_error(format!("create clone-tmp dir: {e}"));
    }

    // Self-heal accumulated clone bundles (task #655): prune any stale
    // `*.tar.gz` in this exact dir before writing the fresh one. This
    // reclaims the SOURCE machine's own previous bundle on the next clone,
    // and any DESTINATION bundle whose immediate post-unpack delete failed.
    // Best-effort: errors are logged, never fatal.
    prune_stale_bundles(&tmp_dir, STALE_BUNDLE_AGE);
    let name = std::path::Path::new(&inv.project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let bundle_path = tmp_dir.join(format!("{name}-{ts}.tar.gz"));
    let created_at = chrono::Utc::now().to_rfc3339();

    let scrubbed_count = inv.scrubbed_secrets.len();
    let entry_count = inv.entries.len();

    // GH#25 observability (send side): make the session count VISIBLE in
    // daemon.stderr.log so a "bundled 0 sessions" outcome is diagnosable.
    // Before the 0.39.44 encoder fix, the bundler enumerated the WRONG slug
    // dir and silently shipped 0 sessions; if it's still 0 after the fix,
    // emit a prominent warning so the operator knows BEFORE the bundle ships.
    let session_count = inv
        .entries
        .iter()
        .filter(|e| e.class == DestinationClass::Session)
        .count();
    if session_count == 0 {
        log_debug!(
            "[daemon/clone] WARN: bundling workspace {} — 0 chat sessions found to migrate \
             (no `.jsonl` under ~/.claude/projects/{}/ or its worktree siblings)",
            inv.project_path,
            inv.slug,
        );
    } else {
        log_debug!(
            "[daemon/clone] bundling workspace {} (slug {}): {} chat session(s), {} total file(s)",
            inv.project_path,
            inv.slug,
            session_count,
            entry_count,
        );
    }

    if let Err(e) = clone::build_bundle(&inv, &opts, created_at, settings, &bundle_path) {
        return CliResponse::internal_error(format!("build bundle: {e}"));
    }

    let size = std::fs::metadata(&bundle_path).map(|m| m.len()).unwrap_or(0);

    CliResponse::ok_json(
        serde_json::json!({
            "bundle_path": bundle_path.to_string_lossy(),
            "manifest_summary": {
                "entry_count": entry_count,
                "scrubbed_secret_count": scrubbed_count,
                "size_bytes": size,
                "include_all_history": !b.live_only,
                "carry_secrets": b.carry_secrets,
            }
        })
        .to_string(),
    )
}

#[derive(Deserialize)]
struct UnpackBody {
    /// Path of the uploaded bundle on THIS (remote) machine.
    bundle_path: String,
    /// Parent folder under which to create `<source-name>/`.
    dest_parent: String,
}

/// `POST /cli/clone/unpack` — extract + register + configure on the
/// DESTINATION daemon. Returns the new project row + the final dest path.
pub fn handle_clone_unpack(body: &[u8]) -> CliResponse {
    let b: UnpackBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return CliResponse::internal_error("cannot resolve home directory"),
    };

    match unpack_and_register(
        std::path::Path::new(&b.bundle_path),
        std::path::Path::new(&b.dest_parent),
        &home,
    ) {
        Ok((project, dest_path)) => CliResponse::ok_json(
            serde_json::json!({
                "project": project,
                "dest_path": dest_path,
            })
            .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Core unpack + DB registration. Split out (and `home`-parameterized) so
/// it's hermetically testable with a temp HOME. Returns the registered
/// project + final dest path.
pub fn unpack_and_register(
    bundle_path: &std::path::Path,
    dest_parent: &std::path::Path,
    home: &std::path::Path,
) -> Result<(Project, String), String> {
    // 1. Extract files at recomputed paths; get the manifest back.
    let (result, manifest) = clone::unpack_bundle(bundle_path, dest_parent, home)?;
    let dest_path = result.dest_path.to_string_lossy().to_string();

    // GH#25 observability (receive side): the prior unpack handler emitted NO
    // log line at all, so a destination had no signal of what (if anything)
    // arrived. Summarize the unpack to daemon.stderr.log: per-class counts +
    // the recomputed dest slug the sessions/memory landed under.
    {
        let mut workspace_files = 0usize;
        let mut memory_files = 0usize;
        let mut session_files = 0usize;
        for e in &manifest.entries {
            match e.class {
                DestinationClass::Workspace => workspace_files += 1,
                DestinationClass::Memory => memory_files += 1,
                DestinationClass::Session => session_files += 1,
            }
        }
        log_debug!(
            "[daemon/clone] unpacked workspace -> {} (dest slug {}): {} workspace file(s), \
             {} memory file(s), {} chat session(s) [from source {}]",
            dest_path,
            result.remote_slug,
            workspace_files,
            memory_files,
            session_files,
            manifest.source_project_path,
        );
        if session_files == 0 {
            log_debug!(
                "[daemon/clone] WARN: unpacked bundle carried 0 chat sessions — \
                 /resume will be empty on this destination"
            );
        }
    }

    // Best-effort: the uploaded bundle has now been fully extracted, so
    // delete it to avoid leaking `~/.k2so/clone-tmp/*.tar.gz` on the
    // DESTINATION machine (task #655). Failures here are non-fatal — the
    // source-side stale-prune in `handle_clone_bundle` is the backstop.
    remove_unpacked_bundle(bundle_path);

    // 2. Register the folder as a project. Prefer the git-aware path
    //    (`add-from-path`); a cloned workspace whose ROOT isn't a git repo
    //    falls back to the git-free registration rather than erroring with
    //    a needs-git-init prompt the remote can't answer.
    let project = match pops::projects_add_from_path(&dest_path) {
        Ok(pops::AddFromPathResult::Project(p)) => p,
        Ok(pops::AddFromPathResult::NeedsGitInit { .. }) => {
            pops::projects_add_without_git(&dest_path)?
        }
        Err(e) => return Err(format!("register project: {e}")),
    };

    // 3. Apply the manifest's K2 settings to the freshly registered row.
    //    Machine-specific fields (id/path/focus_group_id) are intentionally
    //    NOT in `settings`, so the remote keeps its own id + path. If
    //    settings is None (source wasn't a registered project), the project
    //    keeps its registration defaults.
    if let Some(s) = manifest.settings {
        let agent_mode = if s.agent_mode.is_empty() {
            None
        } else {
            Some(s.agent_mode.clone())
        };
        let updated = pops::projects_update(
            &project.id,
            Some(&s.name),
            Some(&s.color),
            None,                          // tab_order — keep registration order
            Some(s.worktree_mode),
            None,                          // pinned
            None,                          // manually_active
            None,                          // icon_url
            Some(if s.agent_enabled { 1 } else { 0 }),
            Some(if s.heartbeat_enabled { 1 } else { 0 }),
            agent_mode,                    // also syncs agent_enabled
            None,                          // state_id
            None,                          // heartbeat_mode
            None,                          // heartbeat_schedule
        )
        .map_err(|e| format!("apply settings: {e}"))?;
        return Ok((updated, dest_path));
    }

    Ok((project, dest_path))
}

/// How old a `clone-tmp/*.tar.gz` must be before the stale-prune deletes it.
/// Conservative: an in-flight clone (bundle → upload → unpack) completes in
/// well under an hour, so anything older is leaked leftovers (task #655).
const STALE_BUNDLE_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Best-effort delete of a fully-extracted clone bundle on the DESTINATION.
/// Errors are logged, never propagated — the source-side stale-prune is the
/// backstop if this fails (e.g. the path is on a read-only mount).
fn remove_unpacked_bundle(bundle_path: &std::path::Path) {
    if let Err(e) = std::fs::remove_file(bundle_path) {
        eprintln!(
            "[daemon/clone] could not remove unpacked bundle {}: {e}",
            bundle_path.display()
        );
    }
}

/// Prune leaked clone bundles: delete every `*.tar.gz` DIRECTLY inside
/// `tmp_dir` whose mtime is older than `max_age`. Self-heals the leak from
/// task #655 on the next clone.
///
/// SAFETY: only the immediate children of `tmp_dir` are considered (no
/// recursion), and only regular files whose name ends in `.tar.gz` are
/// touched — anything else in the dir is left alone. All errors are
/// best-effort/logged so a clone never fails because cleanup couldn't run.
fn prune_stale_bundles(tmp_dir: &std::path::Path, max_age: std::time::Duration) {
    let now = std::time::SystemTime::now();
    let entries = match std::fs::read_dir(tmp_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[daemon/clone] stale-prune skipped, read_dir {} failed: {e}",
                tmp_dir.display()
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Only regular files named `*.tar.gz` directly in this dir.
        let is_bundle = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".tar.gz"))
            .unwrap_or(false);
        if !is_bundle {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let modified = match meta.modified() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let age = match now.duration_since(modified) {
            Ok(a) => a,
            // mtime in the future (clock skew) — treat as fresh, leave it.
            Err(_) => continue,
        };
        if age >= max_age {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!(
                    "[daemon/clone] stale-prune could not remove {}: {e}",
                    path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
