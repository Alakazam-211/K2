//! Daemon-side `/cli/fs/*` route handlers (Phase 2 Unit 6).
//!
//! Every handler is a thin wrapper around `k2_core::fs_commands::*`.
//! Bodies for POST routes are JSON; query-string for GET routes.
//!
//! Binary reads (`/cli/fs/read-binary`) base64-encode the file content
//! before placing it in the JSON response. This trades ~33% wire size
//! for transport-format simplicity (the existing `/cli/*` dispatcher
//! is JSON-only). Pre-Phase-3, this is acceptable up to the 50 MB
//! `read_binary` cap — the renderer's PDF/DOCX viewers don't open
//! anything larger.

use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::fs_commands as fsc;

// ── GET handlers (query-string) ───────────────────────────────────────

pub fn handle_read_dir(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    let show_hidden = matches!(
        params.get("show_hidden").map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on")
    );
    match fsc::read_dir(&path, show_hidden) {
        Ok(entries) => CliResponse::ok_json(
            serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_read_file(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match fsc::read_file(&path) {
        Ok(content) => CliResponse::ok_json(
            serde_json::to_string(&content).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Read a file as raw bytes and base64-encode for transport. The
/// renderer decodes via `atob(...)` or a Uint8Array constructor; both
/// PDF.js and the DOCX renderer take base64 directly.
pub fn handle_read_binary(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    match fsc::read_binary_file(&path) {
        Ok(bytes) => {
            let b64 = B64.encode(&bytes);
            CliResponse::ok_json(serde_json::json!({ "base64": b64 }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET `/cli/fs/read-range?path=&offset=&len=` — one slice of a file,
/// base64 in JSON (the transport every `/cli/*` route shares). The
/// download counterpart of `fs/upload-chunk` (0.40.22): a client loops
/// ranged reads to stream a file of ANY size — per-request memory is
/// bounded by the 16 MB chunk cap, unlike `fs/read-binary`'s whole-file
/// 50 MB ceiling. Response: `{ base64, len, size, eof }`; offset-addressed
/// reads make a resume after disconnect free. Auth: `token_ok` via the
/// shared `/cli/` GET gate, same as every other fs read.
pub fn handle_read_range(params: &HashMap<String, String>) -> CliResponse {
    let path = match params.get("path") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return CliResponse::bad_request("Missing 'path' parameter"),
    };
    let offset: u64 = match params.get("offset").map(|v| v.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'offset' parameter"),
        None => 0,
    };
    let len: u64 = match params.get("len").map(|v| v.parse()) {
        Some(Ok(v)) => v,
        Some(Err(_)) => return CliResponse::bad_request("invalid 'len' parameter"),
        None => 8 * 1024 * 1024,
    };
    match fsc::read_file_range(&path, offset, len) {
        Ok((bytes, size)) => {
            let eof = offset.saturating_add(bytes.len() as u64) >= size;
            CliResponse::ok_json(
                serde_json::json!({
                    "base64": B64.encode(&bytes),
                    "len": bytes.len() as u64,
                    "size": size,
                    "eof": eof,
                })
                .to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Report the daemon machine's filesystem basics so a remote client can
/// seed a folder browser at the host's home dir (instead of a hardcoded
/// `/`) and render paths with the host's separator. No path input — purely
/// describes the host. Gated like the other `fs/*` reads (`token_ok`).
pub fn handle_info(_params: &HashMap<String, String>) -> CliResponse {
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let separator = std::path::MAIN_SEPARATOR.to_string();
    let os = std::env::consts::OS.to_string();
    CliResponse::ok_json(
        serde_json::json!({
            "home": home,
            "separator": separator,
            "os": os,
        })
        .to_string(),
    )
}

pub fn handle_clipboard_paths(_params: &HashMap<String, String>) -> CliResponse {
    match fsc::clipboard_read_file_paths() {
        Ok(paths) => CliResponse::ok_json(
            serde_json::to_string(&paths).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── POST handlers (JSON bodies) ───────────────────────────────────────

#[derive(Deserialize)]
struct SearchTreeBody {
    root: String,
    query: String,
    #[serde(default)]
    show_hidden: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

pub fn handle_search_tree(body: &[u8]) -> CliResponse {
    let parsed: SearchTreeBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let max = parsed.max_results.unwrap_or(500);
    match fsc::search_tree(&parsed.root, &parsed.query, parsed.show_hidden, max) {
        Ok(matches) => CliResponse::ok_json(
            serde_json::to_string(&matches).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct WriteFileBody {
    path: String,
    content: String,
}

pub fn handle_write_file(body: &[u8]) -> CliResponse {
    let parsed: WriteFileBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::write_file(&parsed.path, &parsed.content) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([parsed.path]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct MoveCopyBody {
    sources: Vec<String>,
    destination: String,
}

pub fn handle_move(body: &[u8]) -> CliResponse {
    let parsed: MoveCopyBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::move_files(&parsed.sources, &parsed.destination) {
        Ok(()) => {
            let mut paths = parsed.sources;
            paths.push(parsed.destination);
            crate::session_events::emit_fs_changed_for_paths(paths);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_copy(body: &[u8]) -> CliResponse {
    let parsed: MoveCopyBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::copy_files(&parsed.sources, &parsed.destination) {
        Ok(()) => {
            // Destination is where new entries appear; sources are unchanged.
            crate::session_events::emit_fs_changed_for_paths([parsed.destination]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct DeleteBody {
    paths: Vec<String>,
    #[serde(default)]
    permanent: bool,
}

pub fn handle_delete(body: &[u8]) -> CliResponse {
    let parsed: DeleteBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::delete(&parsed.paths, parsed.permanent) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths(parsed.paths);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    old_path: String,
    new_name: String,
}

pub fn handle_rename(body: &[u8]) -> CliResponse {
    let parsed: RenameBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::rename(&parsed.old_path, &parsed.new_name) {
        Ok(new_path) => {
            crate::session_events::emit_fs_changed_for_paths([
                parsed.old_path,
                new_path.clone(),
            ]);
            CliResponse::ok_json(serde_json::json!({ "path": new_path }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    path: String,
    #[serde(default)]
    is_directory: bool,
}

pub fn handle_create(body: &[u8]) -> CliResponse {
    let parsed: CreateBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::create_entry(&parsed.path, parsed.is_directory) {
        Ok(()) => {
            crate::session_events::emit_fs_changed_for_paths([parsed.path]);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct UploadBinaryBody {
    dir: String,
    filename: String,
    base64: String,
}

/// Decode a base64 payload and write it into `dir` under a sanitized,
/// collision-free name. The renderer (or any remote client) chooses the
/// destination `dir` — for the terminal-drop case it's
/// `<workspace>/.k2so/downloads`. Mirrors `handle_read_binary`'s base64
/// transport in reverse: there we encode bytes OUT, here we decode bytes IN.
///
/// Size cap (`MAX_UPLOAD_SIZE`, 100MB) and path-traversal / sanitize /
/// collision logic all live in `fsc::write_upload` so they're testable
/// without HTTP. We reject the oversize case on the DECODED length before
/// touching the disk.
pub fn handle_upload_binary(body: &[u8]) -> CliResponse {
    let parsed: UploadBinaryBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let bytes = match B64.decode(parsed.base64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid base64: {e}")),
    };
    match fsc::write_upload(&parsed.dir, &parsed.filename, &bytes) {
        Ok(path) => {
            crate::session_events::emit_fs_changed_for_paths([
                path.to_string_lossy().to_string(),
            ]);
            CliResponse::ok_json(
                serde_json::json!({ "path": path.to_string_lossy() }).to_string(),
            )
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct UploadChunkBody {
    /// Client-generated unique id for this transfer (keys the temp `.part`).
    upload_id: String,
    dir: String,
    filename: String,
    /// Decoded-byte offset this chunk starts at; MUST match the part's current
    /// length (0 starts/restarts). Enforced in `fsc::write_upload_chunk`.
    offset: u64,
    base64: String,
    /// `true` on the final chunk → finalize (atomic rename into place).
    is_last: bool,
    /// Full expected transfer size, sent with the offset-0 chunk (0.40.22).
    /// Lets the daemon reject over-ceiling / won't-fit-on-disk transfers up
    /// front; optional for wire-compat with pre-0.40.22 clients.
    #[serde(default)]
    total_bytes: Option<u64>,
}

/// Streaming counterpart to [`handle_upload_binary`]: decode ONE chunk and
/// append it to the in-progress `.part` file (see `fsc::write_upload_chunk`).
/// Memory stays bounded at a single chunk regardless of the total transfer
/// size, so large "Clone to" bundles (GH #3) move without the 100 MB
/// single-shot cap. The per-chunk size guard + ordered-append enforcement live
/// in core so they're unit-testable without HTTP. Returns `{ path, done:true }`
/// on the finalizing chunk, `{ received, done:false }` for intermediate ones.
pub fn handle_upload_chunk(body: &[u8]) -> CliResponse {
    let parsed: UploadChunkBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let bytes = match B64.decode(parsed.base64.as_bytes()) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid base64: {e}")),
    };
    match fsc::write_upload_chunk(
        &parsed.dir,
        &parsed.filename,
        &parsed.upload_id,
        parsed.offset,
        &bytes,
        parsed.is_last,
        parsed.total_bytes,
    ) {
        Ok(Some(path)) => {
            // Final chunk only — intermediate appends are not user-visible.
            crate::session_events::emit_fs_changed_for_paths([
                path.to_string_lossy().to_string(),
            ]);
            CliResponse::ok_json(
                serde_json::json!({ "path": path.to_string_lossy(), "done": true }).to_string(),
            )
        }
        Ok(None) => CliResponse::ok_json(
            serde_json::json!({ "received": parsed.offset + bytes.len() as u64, "done": false })
                .to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── compress (server-side folder → zip, 0.40.22) ──────────────────────
//
// Zipping a big tree takes minutes, and every `/cli/*` response is a
// single buffered body — so compress is an ASYNC JOB, the same
// start-then-poll shape as the daemon self-update (`update_routes`):
// `POST fs/compress` returns a job_id immediately, a worker thread
// streams the zip, `GET fs/compress-status?job_id=` snapshots progress,
// `POST fs/compress-cancel` raises the worker's cooperative cancel flag.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

// ── extract (server-side zip → folder; inverse of compress) ───────────
// Same async job shape as compress. Defined just below the compress
// block so the two stay visually paired.

/// A compress job's observable state; `status` snapshots it. The cancel
/// flag rides along (skipped in serialization) so cancel needs no second
/// registry.
#[derive(Clone, serde::Serialize)]
pub struct CompressJob {
    pub job_id: String,
    /// `running` → `done` | `failed`. Terminal states stay in the map so a
    /// poll that races completion still sees the outcome (jobs are removed
    /// only when a new job starts — see `insert_compress_job`).
    pub phase: &'static str,
    /// Entries written / entries planned (the denominator is exact — the
    /// worker plans the walk before writing).
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

fn compress_jobs() -> &'static Mutex<HashMap<String, CompressJob>> {
    static JOBS: OnceLock<Mutex<HashMap<String, CompressJob>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Insert a fresh job, evicting finished ones so the map can't grow
/// unbounded across a long daemon lifetime (in-flight jobs are kept).
fn insert_compress_job(job: CompressJob) {
    let mut map = compress_jobs().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, j| j.phase == "running");
    map.insert(job.job_id.clone(), job);
}

fn update_compress_job<F: FnOnce(&mut CompressJob)>(job_id: &str, f: F) {
    if let Some(job) = compress_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(job_id)
    {
        f(job);
    }
}

fn get_compress_job(job_id: &str) -> Option<CompressJob> {
    compress_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(job_id)
        .cloned()
}

/// Short collision-resistant job id (random hex via getrandom — same
/// scheme as `update_routes::new_job_id`).
fn new_compress_job_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("zip-{nanos:x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct CompressBody {
    path: String,
}

/// Start a compress job for the folder (or file) at `path`. Validation
/// that would make the job fail instantly (missing path) happens HERE so
/// the caller gets a 400 instead of a job that flips to `failed`; the
/// deep work streams on a worker thread. Returns `{ job_id }`.
pub fn handle_compress(body: &[u8]) -> CliResponse {
    let parsed: CompressBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if let Err(e) = k2_core::fs_commands::validate_path(&parsed.path) {
        return CliResponse::bad_request(e);
    }

    let job_id = new_compress_job_id();
    let cancel = Arc::new(AtomicBool::new(false));
    insert_compress_job(CompressJob {
        job_id: job_id.clone(),
        phase: "running",
        done: 0,
        total: 0,
        zip_path: None,
        error: None,
        cancel: cancel.clone(),
    });

    let src = parsed.path;
    let worker_job_id = job_id.clone();
    std::thread::spawn(move || {
        let progress_id = worker_job_id.clone();
        let result = k2_core::fs_compress::compress_to_zip(
            &src,
            &move |done, total| {
                update_compress_job(&progress_id, |j| {
                    j.done = done;
                    j.total = total;
                });
            },
            &cancel,
        );
        match result {
            Ok(path) => {
                let zip = path.to_string_lossy().to_string();
                update_compress_job(&worker_job_id, |j| {
                    j.phase = "done";
                    j.zip_path = Some(zip.clone());
                });
                // Zip lands next to the source — refresh both.
                crate::session_events::emit_fs_changed_for_paths([src, zip]);
            }
            Err(e) => {
                k2_core::log_debug!(
                    "[daemon] fs/compress — job {worker_job_id} FAILED: {e}"
                );
                update_compress_job(&worker_job_id, |j| {
                    j.phase = "failed";
                    j.error = Some(e);
                });
            }
        }
    });

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// Snapshot a compress job (GET, `?job_id=`).
pub fn handle_compress_status(params: &HashMap<String, String>) -> CliResponse {
    let job_id = match params.get("job_id") {
        Some(id) if !id.is_empty() => id,
        _ => return CliResponse::bad_request("Missing 'job_id' parameter"),
    };
    match get_compress_job(job_id) {
        Some(job) => CliResponse::ok_json(
            serde_json::to_string(&job).unwrap_or_else(|_| "{}".to_string()),
        ),
        None => CliResponse::bad_request(format!("unknown job_id: {job_id}")),
    }
}

#[derive(Deserialize)]
struct CompressCancelBody {
    job_id: String,
}

/// Raise a running job's cooperative cancel flag. The worker notices
/// between entries, removes its `.part`, and flips the job to `failed`
/// ("Compression cancelled") — the status poll surfaces that terminally.
pub fn handle_compress_cancel(body: &[u8]) -> CliResponse {
    let parsed: CompressCancelBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match get_compress_job(&parsed.job_id) {
        Some(job) => {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        None => CliResponse::bad_request(format!("unknown job_id: {}", parsed.job_id)),
    }
}

// ── extract (server-side zip → folder) ────────────────────────────────
//
// Same start-then-poll job shape as compress: `POST fs/extract` returns
// a job_id, a worker thread streams members into a sibling folder,
// `GET fs/extract-status?job_id=` snapshots progress, `POST
// fs/extract-cancel` raises the cooperative cancel flag.

/// An extract job's observable state; `status` snapshots it.
#[derive(Clone, serde::Serialize)]
pub struct ExtractJob {
    pub job_id: String,
    /// `running` → `done` | `failed`. Terminal states stay in the map so a
    /// poll that races completion still sees the outcome (jobs are removed
    /// only when a new job starts — see `insert_extract_job`).
    pub phase: &'static str,
    /// Entries written / entries in the archive.
    pub done: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

fn extract_jobs() -> &'static Mutex<HashMap<String, ExtractJob>> {
    static JOBS: OnceLock<Mutex<HashMap<String, ExtractJob>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn insert_extract_job(job: ExtractJob) {
    let mut map = extract_jobs().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, j| j.phase == "running");
    map.insert(job.job_id.clone(), job);
}

fn update_extract_job<F: FnOnce(&mut ExtractJob)>(job_id: &str, f: F) {
    if let Some(job) = extract_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(job_id)
    {
        f(job);
    }
}

fn get_extract_job(job_id: &str) -> Option<ExtractJob> {
    extract_jobs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(job_id)
        .cloned()
}

fn new_extract_job_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        return format!("unzip-{nanos:x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct ExtractBody {
    path: String,
}

/// Start an extract job for the zip at `path`. Instant-fail validation
/// (missing path / not a .zip file) happens HERE so the caller gets a
/// 400 instead of a job that flips to `failed`; the deep work streams
/// on a worker thread. Returns `{ job_id }`.
pub fn handle_extract(body: &[u8]) -> CliResponse {
    let parsed: ExtractBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let path = match k2_core::fs_commands::validate_path(&parsed.path) {
        Ok(p) => p,
        Err(e) => return CliResponse::bad_request(e),
    };
    if !path.is_file() {
        return CliResponse::bad_request(format!("Not a regular file: {}", parsed.path));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !k2_core::fs_extract::is_zip_filename(&name) {
        return CliResponse::bad_request(format!("Not a .zip archive: {name}"));
    }

    let job_id = new_extract_job_id();
    let cancel = Arc::new(AtomicBool::new(false));
    insert_extract_job(ExtractJob {
        job_id: job_id.clone(),
        phase: "running",
        done: 0,
        total: 0,
        dest_path: None,
        error: None,
        cancel: cancel.clone(),
    });

    let src = parsed.path;
    let worker_job_id = job_id.clone();
    std::thread::spawn(move || {
        let progress_id = worker_job_id.clone();
        let result = k2_core::fs_extract::extract_from_zip(
            &src,
            &move |done, total| {
                update_extract_job(&progress_id, |j| {
                    j.done = done;
                    j.total = total;
                });
            },
            &cancel,
        );
        match result {
            Ok(path) => update_extract_job(&worker_job_id, |j| {
                j.phase = "done";
                j.dest_path = Some(path.to_string_lossy().to_string());
            }),
            Err(e) => {
                k2_core::log_debug!(
                    "[daemon] fs/extract — job {worker_job_id} FAILED: {e}"
                );
                update_extract_job(&worker_job_id, |j| {
                    j.phase = "failed";
                    j.error = Some(e);
                });
            }
        }
    });

    CliResponse::ok_json(serde_json::json!({ "job_id": job_id }).to_string())
}

/// Snapshot an extract job (GET, `?job_id=`).
pub fn handle_extract_status(params: &HashMap<String, String>) -> CliResponse {
    let job_id = match params.get("job_id") {
        Some(id) if !id.is_empty() => id,
        _ => return CliResponse::bad_request("Missing 'job_id' parameter"),
    };
    match get_extract_job(job_id) {
        Some(job) => CliResponse::ok_json(
            serde_json::to_string(&job).unwrap_or_else(|_| "{}".to_string()),
        ),
        None => CliResponse::bad_request(format!("unknown job_id: {job_id}")),
    }
}

#[derive(Deserialize)]
struct ExtractCancelBody {
    job_id: String,
}

/// Raise a running extract job's cooperative cancel flag. The worker
/// notices between entries, removes its partial dest folder, and flips
/// the job to `failed` ("Extraction cancelled").
pub fn handle_extract_cancel(body: &[u8]) -> CliResponse {
    let parsed: ExtractCancelBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match get_extract_job(&parsed.job_id) {
        Some(job) => {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        None => CliResponse::bad_request(format!("unknown job_id: {}", parsed.job_id)),
    }
}

#[derive(Deserialize)]
struct DuplicateBody {
    path: String,
}

pub fn handle_duplicate(body: &[u8]) -> CliResponse {
    let parsed: DuplicateBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::duplicate(&parsed.path) {
        Ok(new_path) => {
            crate::session_events::emit_fs_changed_for_paths([new_path.clone()]);
            CliResponse::ok_json(serde_json::json!({ "path": new_path }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct OpenBody {
    /// Local file path (open-finder) or URL (open-external).
    target: String,
}

pub fn handle_open_finder(body: &[u8]) -> CliResponse {
    let parsed: OpenBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match fsc::open_in_finder(&parsed.target) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_open_external(body: &[u8]) -> CliResponse {
    let parsed: OpenBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };

    // 0.40.34 — viewer-aware URL opening (closes the "Phase 3.1" caveat
    // in `fs_commands::open_external` for http/https targets).
    //
    // RULE IMPLEMENTED: for an http(s) target we ALWAYS broadcast the
    // app-level `open_url` session event (source "terminal-link") so
    // every connected app — local or across the K2 Connect tunnel — can
    // open it in a browser tab. We then SKIP the daemon-local
    // open/xdg-open shell-out whenever there is AT LEAST ONE live
    // `/cli/sessions/events` subscriber (`receiver_count() > 0` on the
    // broadcast bus — cheap, exact): the viewer's browser is the right
    // one, and on a headless server the local shell-out does nothing
    // anyway. Only with ZERO subscribers (nobody watching to open it)
    // do we fall back to the historical local open. Non-http targets
    // (Finder/file-manager paths) keep the current local behavior
    // unconditionally.
    if crate::browser_routes::validate_open_url(&parsed.target).is_ok() {
        let url = parsed.target.trim().to_string();
        let _ = crate::session_events::emit(crate::session_events::SessionEvent::OpenUrl {
            url,
            source: "terminal-link".to_string(),
        });
        let subscribers = crate::session_events::sender().receiver_count();
        if subscribers > 0 {
            return CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "message": format!(
                        "Routed to {subscribers} connected app subscriber(s)"
                    ),
                })
                .to_string(),
            );
        }
        // else: fall through to the local opener below.
    }

    match fsc::open_external(&parsed.target) {
        Ok(msg) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "message": msg }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.40.34 viewer-aware open-external: with ≥1 live events-bus
    /// subscriber, an http(s) target is BROADCAST (`open_url`, source
    /// "terminal-link") and the daemon-local open/xdg-open shell-out is
    /// SKIPPED — proven by the early-return "Routed to …" message (the
    /// local path returns "Opened: …" / an error instead).
    #[test]
    fn open_external_routes_http_to_connected_subscriber_instead_of_local_open() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let probe = format!(
                "https://probe.example/open-external-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            // A live subscriber = "an app is connected".
            let mut rx = crate::session_events::subscribe();

            let body = serde_json::json!({ "target": probe }).to_string();
            let resp = handle_open_external(body.as_bytes());
            assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
            assert!(
                resp.body.contains("Routed to"),
                "must skip the local opener when a subscriber is connected: {}",
                resp.body
            );

            // The subscriber received the event (drain past contamination
            // from other tests on the global bus).
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                let remaining =
                    deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    panic!("did not receive probe open_url in time");
                }
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Ok(crate::session_events::SessionEvent::OpenUrl {
                        url,
                        source,
                    })) if url == probe => {
                        assert_eq!(source, "terminal-link");
                        break;
                    }
                    Ok(Ok(_)) => continue,
                    Ok(Err(_)) => panic!("receiver closed"),
                    Err(_) => panic!("timed out waiting for probe"),
                }
            }
        });
    }

    #[test]
    fn compress_rejects_missing_path_before_creating_a_job() {
        let resp = handle_compress(br#"{"path":"/nonexistent/definitely-not-here-k2"}"#);
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    }

    #[test]
    fn compress_status_rejects_unknown_job_id() {
        let resp = handle_compress_status(&HashMap::from([(
            "job_id".to_string(),
            "no-such-job".to_string(),
        )]));
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    }

    #[test]
    fn compress_cancel_rejects_unknown_job_id() {
        let resp = handle_compress_cancel(br#"{"job_id":"no-such-job"}"#);
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    }

    #[test]
    fn compress_job_runs_to_done_and_status_reports_zip_path() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let src = std::env::temp_dir().join(format!("k2-fs-routes-compress-{nanos}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("f.txt"), b"data").unwrap();

        let body = serde_json::json!({ "path": src.to_string_lossy() }).to_string();
        let resp = handle_compress(body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let job_id = v["job_id"].as_str().expect("job_id").to_string();

        // Poll to a terminal phase (tiny tree → fast; 5s is generous).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let params = HashMap::from([("job_id".to_string(), job_id)]);
        let final_status = loop {
            let s = handle_compress_status(&params);
            assert_eq!(s.status, "200 OK", "body: {}", s.body);
            let j: serde_json::Value = serde_json::from_str(&s.body).unwrap();
            match j["phase"].as_str() {
                Some("running") => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "compress job never finished"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => break j,
            }
        };
        assert_eq!(final_status["phase"].as_str(), Some("done"), "{final_status}");
        let zip_path = final_status["zip_path"].as_str().expect("zip_path");
        assert!(std::path::Path::new(zip_path).exists(), "zip missing: {zip_path}");
        assert!(zip_path.ends_with(".zip"), "got: {zip_path}");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_file(zip_path);
    }

    #[test]
    fn extract_rejects_missing_path_before_creating_a_job() {
        let resp = handle_extract(br#"{"path":"/nonexistent/definitely-not-here-k2"}"#);
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    }

    #[test]
    fn extract_rejects_non_zip_before_creating_a_job() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("k2-fs-routes-extract-notzip-{nanos}.txt"));
        std::fs::write(&path, b"not a zip").unwrap();
        let body = serde_json::json!({ "path": path.to_string_lossy() }).to_string();
        let resp = handle_extract(body.as_bytes());
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
        assert!(
            resp.body.contains(".zip") || resp.body.contains("zip"),
            "body: {}",
            resp.body
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn extract_status_rejects_unknown_job_id() {
        let resp = handle_extract_status(&HashMap::from([(
            "job_id".to_string(),
            "no-such-job".to_string(),
        )]));
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    }

    #[test]
    fn extract_cancel_rejects_unknown_job_id() {
        let resp = handle_extract_cancel(br#"{"job_id":"no-such-job"}"#);
        assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    }

    #[test]
    fn extract_job_runs_to_done_and_status_reports_dest_path() {
        use std::sync::atomic::AtomicBool;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("k2-fs-routes-extract-{nanos}"));
        let src = dir.join("bundle");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"world").unwrap();
        // Build the fixture via compress (daemon has no direct zip dep).
        let zip_path = k2_core::fs_compress::compress_to_zip(
            src.to_str().unwrap(),
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .expect("compress fixture");

        let body = serde_json::json!({ "path": zip_path.to_string_lossy() }).to_string();
        let resp = handle_extract(body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        let job_id = v["job_id"].as_str().expect("job_id").to_string();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let params = HashMap::from([("job_id".to_string(), job_id)]);
        let final_status = loop {
            let s = handle_extract_status(&params);
            assert_eq!(s.status, "200 OK", "body: {}", s.body);
            let j: serde_json::Value = serde_json::from_str(&s.body).unwrap();
            match j["phase"].as_str() {
                Some("running") => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "extract job never finished"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => break j,
            }
        };
        assert_eq!(final_status["phase"].as_str(), Some("done"), "{final_status}");
        let dest_path = final_status["dest_path"].as_str().expect("dest_path");
        assert!(
            std::path::Path::new(dest_path).is_dir(),
            "dest missing: {dest_path}"
        );
        // compress stores parent-relative entries (`bundle/hello.txt`), so
        // extract reproduces that layout under the sibling dest.
        let content = std::fs::read_to_string(
            std::path::Path::new(dest_path).join("bundle/hello.txt"),
        )
        .expect("bundle/hello.txt");
        assert_eq!(content, "world");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn info_returns_home_separator_and_os() {
        let resp = handle_info(&HashMap::new());
        assert_eq!(resp.status, "200 OK", "fs/info must succeed");

        let v: serde_json::Value =
            serde_json::from_str(&resp.body).expect("fs/info body must be JSON");

        // separator is exactly the host's MAIN_SEPARATOR.
        assert_eq!(
            v["separator"].as_str().expect("separator must be a string"),
            std::path::MAIN_SEPARATOR.to_string(),
        );

        // os is the compile-target OS string.
        assert_eq!(
            v["os"].as_str().expect("os must be a string"),
            std::env::consts::OS,
        );

        // home is present as a string (may be empty if unavailable, but the
        // key must exist and be string-typed — not null/missing).
        assert!(
            v["home"].is_string(),
            "home must be a string, got {:?}",
            v["home"],
        );
    }
}
