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
//! 0.40.22 adds the PULL half ("Clone to this computer"): a renderer that is
//! remotely signed into this daemon asks it to pack a workspace, downloads
//! the bundle over `fs/read-range`, and unpacks on ITS OWN local daemon.
//! Packing a big workspace over a tunneled request can outlive an HTTP
//! round-trip, so the pull pack is an ASYNC JOB (the same start-then-poll
//! shape as `fs/compress`):
//! - `POST /cli/clone/pack` starts the job (project must be a REGISTERED
//!   workspace — a pull may only take what this server actually manages)
//!   and returns `{ job_id }`; a worker thread builds the same bundle the
//!   push route does, then enforces the `MAX_TRANSFER_SIZE` ceiling.
//! - `GET  /cli/clone/pack-status?job_id=` snapshots the job.
//! - `POST /cli/clone/pack-cleanup` deletes a finished job's bundle after
//!   the client has downloaded it (job-id-keyed — the route never takes a
//!   path, so it can only ever delete a bundle THIS daemon packed).
//!
//! All POSTs are gated by `token_ok` in the dispatcher (the same
//! isolated-gate pattern as `fs/upload-binary`), so these handlers assume
//! the caller is already authenticated.

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

/// How a bundle build failed — carried out of [`build_workspace_bundle`]
/// so the synchronous push route can keep its exact 400-vs-500 mapping
/// while the async pack job flattens both into a failed-job message.
enum BundleError {
    /// Caller-supplied input was wrong (bad path, inventory failure).
    BadRequest(String),
    /// The daemon itself couldn't do the work (home/dir/tar failures).
    Internal(String),
}

/// A successfully built bundle on this daemon's disk.
struct BundleOutcome {
    bundle_path: std::path::PathBuf,
    size_bytes: u64,
    entry_count: usize,
    scrubbed_count: usize,
}

/// Build a workspace clone bundle into `<home>/.k2/clone-tmp/` — the shared
/// engine behind the synchronous push route (`handle_clone_bundle`) and the
/// async pull-pack job (`handle_clone_pack`). `home`-parameterized (like
/// `unpack_and_register`) so the pack-job tests run against a temp HOME.
fn build_workspace_bundle(
    project_path: &str,
    opts: &clone::CloneOptions,
    home: &std::path::Path,
) -> Result<BundleOutcome, BundleError> {
    let inv = clone::inventory(project_path, opts.clone())
        .map_err(|e| BundleError::BadRequest(format!("inventory failed: {e}")))?;

    // Capture the source workspace's K2 settings + pinned-chat identity
    // (graceful: None/empty if the path isn't a registered project).
    let (settings, pinned_chat, chat_pins) = {
        let db = db::shared();
        let conn = db.lock();
        let settings = clone::capture_settings(&conn, &inv.project_path)
            .map_err(|e| BundleError::Internal(format!("settings capture: {e}")))?;
        let pinned_chat = clone::capture_pinned_chat(&conn, &inv.project_path)
            .map_err(|e| BundleError::Internal(format!("pinned-chat capture: {e}")))?;
        let session_ids = clone::session_ids_from_entries(
            inv.entries
                .iter()
                .map(|e| (e.class, e.rel_path.clone())),
        );
        let chat_pins = clone::capture_chat_pins(&conn, &session_ids)
            .map_err(|e| BundleError::Internal(format!("chat-pins capture: {e}")))?;
        (settings, pinned_chat, chat_pins)
    };

    // Temp bundle path: ~/.k2so/clone-tmp/<name>-<ts>.tar.gz
    let tmp_dir = home.join(".k2").join("clone-tmp");
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| BundleError::Internal(format!("create clone-tmp dir: {e}")))?;

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
    let claude_session_count = inv
        .entries
        .iter()
        .filter(|e| e.class == DestinationClass::Session)
        .count();
    let provider_file_count = inv
        .entries
        .iter()
        .filter(|e| e.class == DestinationClass::Provider)
        .count();
    // "chat sessions" total for the zero-session warning includes Claude
    // sessions + multi-provider files (Gemini/Pi/Codex/Grok/Cursor/Hermes).
    let session_count = claude_session_count + provider_file_count;
    if session_count == 0 {
        log_debug!(
            "[daemon/clone] WARN: bundling workspace {} — 0 chat sessions found to migrate \
             (no Claude `.jsonl` under ~/.claude/projects/{}/ and no provider sessions)",
            inv.project_path,
            inv.slug,
        );
    } else {
        log_debug!(
            "[daemon/clone] bundling workspace {} (slug {}): {} chat session file(s) \
             ({} Claude + {} provider), {} total file(s)",
            inv.project_path,
            inv.slug,
            session_count,
            claude_session_count,
            provider_file_count,
            entry_count,
        );
    }

    clone::build_bundle(
        &inv,
        opts,
        created_at,
        settings,
        pinned_chat,
        chat_pins,
        &bundle_path,
    )
    .map_err(|e| BundleError::Internal(format!("build bundle: {e}")))?;

    let size = std::fs::metadata(&bundle_path).map(|m| m.len()).unwrap_or(0);

    Ok(BundleOutcome {
        bundle_path,
        size_bytes: size,
        entry_count,
        scrubbed_count,
    })
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

    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return CliResponse::internal_error("cannot resolve home directory"),
    };

    let out = match build_workspace_bundle(&b.project_path, &opts, &home) {
        Ok(o) => o,
        Err(BundleError::BadRequest(e)) => return CliResponse::bad_request(e),
        Err(BundleError::Internal(e)) => return CliResponse::internal_error(e),
    };

    CliResponse::ok_json(
        serde_json::json!({
            "bundle_path": out.bundle_path.to_string_lossy(),
            "manifest_summary": {
                "entry_count": out.entry_count,
                "scrubbed_secret_count": out.scrubbed_count,
                "size_bytes": out.size_bytes,
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
        let mut provider_files = 0usize;
        for e in &manifest.entries {
            match e.class {
                DestinationClass::Workspace => workspace_files += 1,
                DestinationClass::Memory => memory_files += 1,
                DestinationClass::Session => session_files += 1,
                DestinationClass::Provider => provider_files += 1,
            }
        }
        log_debug!(
            "[daemon/clone] unpacked workspace -> {} (dest slug {}): {} workspace file(s), \
             {} memory file(s), {} chat session(s), {} provider file(s) [from source {}]",
            dest_path,
            result.remote_slug,
            workspace_files,
            memory_files,
            session_files,
            provider_files,
            manifest.source_project_path,
        );
        if session_files == 0 && provider_files == 0 {
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
    let project = if let Some(s) = &manifest.settings {
        let agent_mode = if s.agent_mode.is_empty() {
            None
        } else {
            Some(s.agent_mode.clone())
        };
        pops::projects_update(
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
            None,                          // default_agent — the pulled row keeps its stamp-on-register value
        )
        .map_err(|e| format!("apply settings: {e}"))?
    } else {
        project
    };

    // 4. Apply pinned chat + chat pins (best-effort — never fail the
    //    whole unpack if identity re-stamp has a DB/fs hiccup).
    {
        let db = db::shared();
        let conn = db.lock();
        if let Err(e) = clone::apply_clone_identity(
            &conn,
            &project.id,
            &dest_path,
            Some(home),
            &manifest,
        ) {
            log_debug!(
                "[daemon/clone] WARN: apply_clone_identity failed (non-fatal): {e}"
            );
        }
    }

    Ok((project, dest_path))
}

// ── pull pack job (0.40.22 "Clone to this computer") ──────────────────
//
// Packing a workspace can take minutes and the pull client sits on the
// far side of a tunnel, so the pack is an ASYNC JOB — the same
// start-then-poll shape as `fs/compress` (`fs_routes::handle_compress`):
// `POST clone/pack` returns a job_id immediately, a worker thread builds
// the bundle, `GET clone/pack-status?job_id=` snapshots it, and
// `POST clone/pack-cleanup` reclaims the bundle once downloaded.

use std::sync::{Mutex, OnceLock};

/// A pack job's observable state; `pack-status` snapshots it. On `done`
/// it carries the bundle path (for the `fs/read-range` download loop) +
/// the same manifest-summary fields the push bundle route returns.
#[derive(Clone, serde::Serialize)]
pub struct PackJob {
    pub job_id: String,
    /// `running` → `done` | `failed`. Terminal states stay in the map so a
    /// poll that races completion still sees the outcome (jobs are removed
    /// only when a new job starts — see `insert_pack_job`).
    pub phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrubbed_secret_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn pack_jobs() -> &'static Mutex<std::collections::HashMap<String, PackJob>> {
    static JOBS: OnceLock<Mutex<std::collections::HashMap<String, PackJob>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Insert a fresh job, evicting finished ones so the map can't grow
/// unbounded across a long daemon lifetime (in-flight jobs are kept).
fn insert_pack_job(job: PackJob) {
    let mut map = pack_jobs().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, j| j.phase == "running");
    map.insert(job.job_id.clone(), job);
}

fn update_pack_job<F: FnOnce(&mut PackJob)>(job_id: &str, f: F) {
    if let Some(job) = pack_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(job_id)
    {
        f(job);
    }
}

fn get_pack_job(job_id: &str) -> Option<PackJob> {
    pack_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(job_id)
        .cloned()
}

/// Short collision-resistant job id (random hex via getrandom — same
/// scheme as `fs_routes::new_compress_job_id`).
fn new_pack_job_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("pack-{nanos:x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `POST /cli/clone/pack` — start a pull-pack job. Body is the same shape
/// as `clone/bundle` (`project_path` + `live_only` + `carry_secrets`).
/// Validation that would make the job fail instantly happens HERE so the
/// caller gets a 400 instead of a job that flips to `failed`; the deep
/// work runs on a worker thread. Returns `{ job_id }`.
pub fn handle_clone_pack(body: &[u8]) -> CliResponse {
    let b: BundleBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    // Path validation, same chokepoint as the fs routes (empty / traversal
    // relative paths / nonexistent → 400, and the path is canonicalized so
    // the registered-workspace comparison below can't be dodged with `..`).
    let canonical = match k2_core::fs_commands::validate_path(&b.project_path) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };

    // Pull-pack only serves REGISTERED workspaces: a remote viewer may take
    // home what this server manages as a workspace, not arbitrary trees the
    // token could otherwise name (the push `clone/bundle` route stays as-is
    // — its caller is the machine's own renderer bundling its own disk).
    if !is_registered_workspace(&canonical) {
        return CliResponse::bad_request(format!(
            "not a registered workspace on this server: {}",
            canonical.display()
        ));
    }

    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return CliResponse::internal_error("cannot resolve home directory"),
    };

    let opts = clone::CloneOptions {
        include_all_history: !b.live_only,
        carry_secrets: b.carry_secrets,
        home_override: None,
    };

    let job_id = new_pack_job_id();
    insert_pack_job(PackJob {
        job_id: job_id.clone(),
        phase: "running",
        bundle_path: None,
        size_bytes: None,
        entry_count: None,
        scrubbed_secret_count: None,
        error: None,
    });

    let project_path = canonical.to_string_lossy().to_string();
    let worker_job_id = job_id.clone();
    std::thread::spawn(move || {
        run_pack_job(
            &worker_job_id,
            &project_path,
            &opts,
            &home,
            k2_core::fs_commands::MAX_TRANSFER_SIZE,
        );
    });

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// `canonical` equals a registered project's path? Registered paths are
/// canonicalized too (best-effort; a stale row whose dir vanished simply
/// never matches) so symlinked spellings of the same workspace compare
/// equal.
fn is_registered_workspace(canonical: &std::path::Path) -> bool {
    let projects = match pops::projects_list() {
        Ok(p) => p,
        Err(_) => return false,
    };
    projects.iter().any(|p| {
        std::fs::canonicalize(&p.path)
            .map(|reg| reg == canonical)
            .unwrap_or(false)
    })
}

/// Pack-job worker body: build the bundle, then enforce the transfer
/// ceiling. Split out (and `home`/`max_bytes`-parameterized) so it's
/// hermetically testable without spawning the route's thread.
fn run_pack_job(
    job_id: &str,
    project_path: &str,
    opts: &clone::CloneOptions,
    home: &std::path::Path,
    max_bytes: u64,
) {
    match build_workspace_bundle(project_path, opts, home) {
        Ok(out) => {
            // Transfer ceiling: a bundle the download machinery would refuse
            // to move must not linger on disk as a terminal "success".
            if out.size_bytes > max_bytes {
                let _ = std::fs::remove_file(&out.bundle_path);
                k2_core::log_debug!(
                    "[daemon/clone] pack job {job_id} FAILED: bundle {} bytes exceeds \
                     the {} GiB transfer ceiling",
                    out.size_bytes,
                    max_bytes / (1024 * 1024 * 1024),
                );
                update_pack_job(job_id, |j| {
                    j.phase = "failed";
                    j.error = Some(format!(
                        "bundle is {} bytes — over the {} GiB transfer ceiling",
                        out.size_bytes,
                        max_bytes / (1024 * 1024 * 1024)
                    ));
                });
                return;
            }
            update_pack_job(job_id, |j| {
                j.phase = "done";
                j.bundle_path = Some(out.bundle_path.to_string_lossy().to_string());
                j.size_bytes = Some(out.size_bytes);
                j.entry_count = Some(out.entry_count as u64);
                j.scrubbed_secret_count = Some(out.scrubbed_count as u64);
            });
        }
        Err(BundleError::BadRequest(e)) | Err(BundleError::Internal(e)) => {
            k2_core::log_debug!("[daemon/clone] pack job {job_id} FAILED: {e}");
            update_pack_job(job_id, |j| {
                j.phase = "failed";
                j.error = Some(e);
            });
        }
    }
}

/// `GET /cli/clone/pack-status?job_id=` — snapshot a pack job.
pub fn handle_clone_pack_status(
    params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    let job_id = match params.get("job_id") {
        Some(id) if !id.is_empty() => id,
        _ => return CliResponse::bad_request("Missing 'job_id' parameter"),
    };
    match get_pack_job(job_id) {
        Some(job) => CliResponse::ok_json(
            serde_json::to_string(&job).unwrap_or_else(|_| "{}".to_string()),
        ),
        None => CliResponse::bad_request(format!("unknown job_id: {job_id}")),
    }
}

#[derive(Deserialize)]
struct PackCleanupBody {
    job_id: String,
}

/// `POST /cli/clone/pack-cleanup` — delete a finished pack job's bundle
/// once the client has downloaded it. Keyed by job_id (never a path), so
/// it can only delete a bundle THIS daemon packed; an already-gone file is
/// fine (idempotent). A still-running job is a 400 — the client must wait
/// for a terminal phase. The hourly stale-prune in `handle_clone_bundle`
/// remains the backstop if the client dies before calling this.
pub fn handle_clone_pack_cleanup(body: &[u8]) -> CliResponse {
    let parsed: PackCleanupBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let job = match get_pack_job(&parsed.job_id) {
        Some(j) => j,
        None => {
            return CliResponse::bad_request(format!("unknown job_id: {}", parsed.job_id))
        }
    };
    if job.phase == "running" {
        return CliResponse::bad_request("pack job is still running".to_string());
    }
    if let Some(path) = &job.bundle_path {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return CliResponse::internal_error(format!(
                    "could not remove packed bundle: {e}"
                ));
            }
        }
        update_pack_job(&parsed.job_id, |j| j.bundle_path = None);
    }
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
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
