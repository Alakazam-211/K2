//! Filesystem primitives migrated from `src-tauri/src/commands/filesystem.rs`
//! during Phase 2 Unit 6 (daemon-headless migration).
//!
//! Each function takes plain strings / Vec<String> so it can be invoked
//! either by:
//!
//! - The legacy Tauri `#[tauri::command]` thin wrappers (during the
//!   transition window — already deleted in this Unit), or
//! - The daemon's `/cli/fs/*` route handlers, which is the post-Phase-2
//!   home of the file tree, chat history sidebar, theme manager, etc.
//!
//! Operations are deliberately daemon-side because the **daemon owns the
//! filesystem** in the K2SO Connect architecture. When the Tauri client
//! lives on Machine A and the daemon on Machine B, the file tree the user
//! browses is Machine B's tree. (`open-in-finder` and `open-external` are
//! the lone exceptions — they open the local file on the daemon's machine,
//! which is the right behavior in Bundled mode and harmless-but-unusable
//! in K2SO Connect mode. Phase 3.1 will route those through the renderer.)
//!
//! Path safety: `validate_path` rejects empty paths and resolves symlinks
//! / `..` segments via `canonicalize` for reads. Writes accept paths whose
//! parent already exists, so renderer code can build paths without
//! checking existence first.

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 10MB max for text reads — matches the legacy Tauri command's cap.
/// Above this the renderer should switch to binary read or stream the
/// file in chunks.
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// 50MB max for binary reads (images, PDFs, small media). Anything
/// larger should be streamed via a separate WS endpoint — Phase 3 work.
pub const MAX_BINARY_SIZE: u64 = 50 * 1024 * 1024;

// ── Path validation ─────────────────────────────────────────────────────

/// Validate a path before any FS op. Rejects empty paths and ensures the
/// path resolves to a real location for reads. For writes (where the
/// destination doesn't exist yet) it accepts the path if its parent
/// exists.
pub fn validate_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("Path is empty".to_string());
    }
    let p = Path::new(path);
    if p.exists() {
        p.canonicalize().map_err(|e| format!("Invalid path: {e}"))
    } else {
        let parent = p
            .parent()
            .ok_or_else(|| "Invalid path: no parent".to_string())?;
        if parent.exists() {
            Ok(p.to_path_buf())
        } else {
            Err(format!(
                "Parent directory does not exist: {}",
                parent.display()
            ))
        }
    }
}

// ── Exclusive rename (macOS atomic) ────────────────────────────────────

/// Rename a file/directory without overwriting the destination. On
/// macOS uses `renamex_np` with `RENAME_EXCL` for atomic exclusivity.
/// On other platforms falls back to an exists-check + rename (race
/// window, but catches the common case).
fn rename_exclusive(src: &Path, dst: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        let src_c = CString::new(src.to_string_lossy().as_bytes())
            .map_err(|_| "Invalid source path".to_string())?;
        let dst_c = CString::new(dst.to_string_lossy().as_bytes())
            .map_err(|_| "Invalid destination path".to_string())?;
        // RENAME_EXCL = 0x00000004 — fail if destination exists
        let ret =
            unsafe { libc::renamex_np(src_c.as_ptr(), dst_c.as_ptr(), 0x00000004) };
        if ret == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EEXIST) {
            return Err(format!("Already exists: {}", dst.display()));
        }
        if err.raw_os_error() == Some(libc::ENOTSUP) {
            // Filesystem doesn't support renamex_np (e.g. FUSE). Fall
            // back to the same logic the non-macOS path uses.
            if dst.exists() {
                return Err(format!("Already exists: {}", dst.display()));
            }
            fs::rename(src, dst).map_err(|e| format!("Rename failed: {e}"))?;
            return Ok(());
        }
        Err(format!("Rename failed: {err}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if dst.exists() {
            return Err(format!("Already exists: {}", dst.display()));
        }
        fs::rename(src, dst).map_err(|e| format!("Rename failed: {e}"))
    }
}

// ── read-dir / search-tree ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified_at: f64,
}

pub fn read_dir(path: &str, show_hidden: bool) -> Result<Vec<DirEntryInfo>, String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let entries = fs::read_dir(&path).map_err(|e| e.to_string())?;
    let mut items: Vec<DirEntryInfo> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            let full_path = Path::new(&path).join(&name);
            let full_path_str = full_path.to_string_lossy().to_string();
            let is_directory = entry
                .file_type()
                .ok()
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
            let (size, modified_at) = match fs::metadata(&full_path) {
                Ok(meta) => {
                    let size = meta.len();
                    let modified_at = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                    (size, modified_at)
                }
                Err(_) => (0, 0.0),
            };
            Some(DirEntryInfo {
                name,
                path: full_path_str,
                is_directory,
                size,
                modified_at,
            })
        })
        .collect();
    items.sort_by(|a, b| {
        if a.is_directory != b.is_directory {
            return if a.is_directory {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });
    Ok(items)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    pub name: String,
    pub is_directory: bool,
}

pub fn search_tree(
    root: &str,
    query: &str,
    show_hidden: bool,
    max_results: usize,
) -> Result<Vec<SearchMatch>, String> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return Ok(Vec::new());
    }
    let root_path = validate_path(root)?;
    let mut results: Vec<SearchMatch> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root_path];
    const SKIP_DIRS: &[&str] = &[
        "node_modules",
        ".git",
        "target",
        "dist",
        "build",
        ".next",
        ".turbo",
        ".cache",
        "Pods",
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        "DerivedData",
    ];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let full = dir.join(&name);
            let is_dir = entry
                .file_type()
                .ok()
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
            if name.to_lowercase().contains(&query_lower) {
                results.push(SearchMatch {
                    path: full.to_string_lossy().to_string(),
                    name: name.clone(),
                    is_directory: is_dir,
                });
                if results.len() >= max_results {
                    return Ok(results);
                }
            }
            if is_dir && !SKIP_DIRS.contains(&name.as_str()) {
                stack.push(full);
            }
        }
    }
    Ok(results)
}

// ── read-file / read-binary / write-file ──────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub content: String,
    pub path: String,
    pub name: String,
}

pub fn read_file(path: &str) -> Result<FileContent, String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let p = Path::new(&path);
    if !p.exists() {
        return Err("File not found".to_string());
    }
    let meta = fs::metadata(&path).map_err(map_io_err)?;
    if meta.len() > MAX_FILE_SIZE {
        return Err("File too large (>10MB)".to_string());
    }
    let buffer = fs::read(&path).map_err(map_io_err)?;
    // Reject binary files (null byte in first 8KB).
    let check_len = std::cmp::min(buffer.len(), 8192);
    for byte in &buffer[..check_len] {
        if *byte == 0 {
            return Err("Cannot read binary file".to_string());
        }
    }
    let content = String::from_utf8(buffer)
        .map_err(|_| "Cannot read binary file".to_string())?;
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(FileContent {
        content,
        path,
        name,
    })
}

/// Read a file as raw bytes (caller's responsibility to interpret as
/// PDF, image, etc.). HTTP transport will base64-encode this on the way
/// out — see `daemon::fs_routes` for the wrapper.
pub fn read_binary_file(path: &str) -> Result<Vec<u8>, String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let p = Path::new(&path);
    if !p.exists() {
        return Err("File not found".to_string());
    }
    let meta = fs::metadata(&path).map_err(map_io_err)?;
    if meta.len() > MAX_BINARY_SIZE {
        return Err("File too large (>50MB)".to_string());
    }
    fs::read(&path).map_err(map_io_err)
}

/// Per-request ceiling for a ranged read (`read_file_range`) — bounds the
/// daemon's per-response memory exactly like `MAX_UPLOAD_CHUNK_SIZE` does
/// for the upload direction. The renderer asks for 8 MB; headroom
/// tolerates a larger client.
pub const MAX_READ_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

/// Read up to `len` bytes of a file starting at `offset` — the download
/// counterpart of `write_upload_chunk` (0.40.22). Unlike `read_binary_file`
/// there is NO whole-file size cap: memory is bounded by the per-chunk
/// ceiling, so a multi-GB download streams in requests. Returns the bytes
/// (short at EOF; empty past EOF) plus the file's total size, so a client
/// can size its progress bar from the first response and detect EOF via
/// `offset + returned.len() >= size`.
pub fn read_file_range(path: &str, offset: u64, len: u64) -> Result<(Vec<u8>, u64), String> {
    use std::io::{Read, Seek, SeekFrom};
    if len == 0 || len > MAX_READ_CHUNK_SIZE {
        return Err(format!(
            "range len {len} out of bounds (1..={MAX_READ_CHUNK_SIZE})"
        ));
    }
    let path = validate_path(path)?;
    let meta = fs::metadata(&path).map_err(map_io_err)?;
    if !meta.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    let size = meta.len();
    let mut f = fs::File::open(&path).map_err(map_io_err)?;
    f.seek(SeekFrom::Start(offset)).map_err(map_io_err)?;
    // Loop so a short OS read mid-file still fills the buffer; stop only
    // at genuine EOF (mirrors the Tauri-side read_local_file_range).
    let mut buf = vec![0u8; len as usize];
    let mut filled = 0usize;
    while filled < buf.len() {
        match f.read(&mut buf[filled..]) {
            Ok(0) => break, // EOF
            Ok(n) => filled += n,
            Err(e) => return Err(map_io_err(e)),
        }
    }
    buf.truncate(filled);
    Ok((buf, size))
}

pub fn write_file(path: &str, content: &str) -> Result<(), String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    fs::write(&path, content).map_err(map_io_err)
}

fn map_io_err(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "Permission denied".to_string()
    } else {
        e.to_string()
    }
}

// ── upload (binary drop onto the daemon's disk) ───────────────────────

/// 100MB cap for an uploaded file's decoded bytes — the server-side
/// limit for `/cli/fs/upload-binary`. The renderer enforces a parallel
/// cap before it base64-encodes (Tauri `read_local_file_base64`), but
/// this is the authoritative gate: a remote client could call the route
/// directly. Larger transfers belong on the chunked route
/// (`write_upload_chunk`, capped at [`MAX_TRANSFER_SIZE`] total), not a
/// single base64 JSON body — the renderer picks the path by file size.
pub const MAX_UPLOAD_SIZE: u64 = 100 * 1024 * 1024;

/// Ensure `<workspace_path>/.k2so/downloads/` exists and return its path.
/// Mirrors the `.k2so/` creation pattern used by `inbox` /
/// `workspace::onboarding` — `create_dir_all` is idempotent, so repeated
/// calls are cheap and safe.
pub fn ensure_downloads_dir(workspace_path: &str) -> Result<PathBuf, String> {
    if workspace_path.is_empty() {
        return Err("Workspace path is empty".to_string());
    }
    let dir = crate::workspace_dot_dir(workspace_path).join("downloads");
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create downloads dir: {e}"))?;
    Ok(dir)
}

/// Reduce an arbitrary client-supplied `filename` to a safe basename so
/// it can never escape its destination directory. Strips any path
/// components (`../`, `/`, `\\`, drive prefixes) and NUL bytes, keeping
/// only the final path segment. Returns a fallback when the result would
/// be empty or a relative marker (`.` / `..`).
fn sanitize_filename(filename: &str) -> String {
    // Split on BOTH separators so a Windows-style `..\\x` path supplied
    // to a unix daemon is still reduced to its last segment.
    let last = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename);
    let cleaned: String = last.chars().filter(|c| *c != '\0').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "upload".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Build a non-colliding target path inside `dir` for `filename`. If
/// `dir/filename` is free it's used as-is; otherwise appends
/// ` (1)`, ` (2)`, … before the extension: `name (1).ext`.
fn collision_free_path(dir: &Path, filename: &str) -> PathBuf {
    let initial = dir.join(filename);
    if !initial.exists() {
        return initial;
    }
    let name_path = Path::new(filename);
    let stem = name_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = name_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10_000 {
        let candidate = dir.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Pathological fallback — 10k collisions. Return the initial path;
    // the caller's write will overwrite, but this is effectively
    // unreachable.
    initial
}

/// Write `bytes` into `dir` under a sanitized, collision-free name.
/// `dir` is created (recursively) when absent — Phase 3 terminal drops
/// target `<workspace>/.k2so/downloads/`, which may not exist yet, and
/// folder-drops target an existing tree; `create_dir_all` is idempotent
/// for the latter. `filename` is reduced to a basename so it can't escape
/// `dir`. On a name collision the file is written as `name (1).ext`,
/// `name (2).ext`, … (non-destructive). Returns the final absolute path
/// written.
///
/// Sandbox/auth note: this function does NOT itself enforce who may
/// write where — the destination `dir` is the caller's choice and the
/// HTTP route gates the call. Keep size/sanitize/collision logic here so
/// it's unit-testable without HTTP.
pub fn write_upload(dir: &str, filename: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.len() as u64 > MAX_UPLOAD_SIZE {
        return Err("File too large (>100MB)".to_string());
    }
    if dir.is_empty() {
        return Err("Destination directory is empty".to_string());
    }
    // Auto-create the destination so the terminal-drop convenience path
    // (`<ws>/.k2so/downloads/`) needn't pre-exist. `create_dir_all` is a
    // no-op when the tree already exists (folder-drop case). We create
    // BEFORE validate_path so its canonicalize() (which requires the path
    // to exist) succeeds.
    fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create destination dir: {e}"))?;
    let dir_path = validate_path(dir)?;
    if !dir_path.is_dir() {
        return Err(format!("Destination is not a directory: {dir}"));
    }
    let safe_name = sanitize_filename(filename);
    let target = collision_free_path(&dir_path, &safe_name);
    fs::write(&target, bytes).map_err(map_io_err)?;
    Ok(target)
}

// ── chunked / streaming upload (large clone bundles, GH #3) ────────────

/// Per-chunk decoded-byte ceiling for the streaming upload route. This bounds
/// the daemon's per-request memory (the HTTP layer buffers each request body
/// whole before the handler runs) INDEPENDENT of the total transfer size —
/// which is exactly what lets a multi-GB clone bundle through without the
/// single-shot [`MAX_UPLOAD_SIZE`] cap. The renderer sends well under this
/// (8 MB chunks); the headroom tolerates a larger client chunk.
pub const MAX_UPLOAD_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

/// Total-transfer ceiling for a chunked upload (0.40.22). The chunked route
/// exists precisely to dodge the 100 MB single-shot cap, but "no cap" is not
/// a policy: a runaway or malicious client must not be able to fill the
/// host's disk one 16 MB chunk at a time. 10 GiB covers every sanctioned use
/// (workspace bundles, media, model weights) with room to spare; raise the
/// constant deliberately if a real transfer ever hits it.
pub const MAX_TRANSFER_SIZE: u64 = 10 * 1024 * 1024 * 1024;

/// Free bytes available to unprivileged writes on the filesystem containing
/// `path` (`statvfs` `f_bavail * f_frsize`). `None` when the probe fails or
/// the platform has no statvfs — callers MUST treat unknown as "don't
/// block" (a probe failure is not evidence of a full disk).
pub fn free_disk_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Reduce an `upload_id` to filename-safe characters so it can never inject
/// a path separator into — or escape — the `.part` temp name. Keeps ASCII
/// alphanumerics, `-`, and `_`; drops everything else. Falls back to `anon`
/// when the result is empty.
fn sanitize_upload_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cleaned.is_empty() {
        "anon".to_string()
    } else {
        cleaned
    }
}

/// Streaming counterpart to [`write_upload`]: append ONE ordered chunk of a
/// larger transfer to a temp `.part` file in `dir`, finalizing it — an atomic
/// rename to a sanitized, collision-free name — on the `is_last` call. Returns
/// `Some(final_path)` on that finalizing call and `None` for intermediate
/// chunks.
///
/// This is the substrate for large "Clone to" bundles (GH #3): the single-shot
/// [`write_upload`] buffers the whole payload in memory and is capped at
/// [`MAX_UPLOAD_SIZE`] (100 MB); chunking streams each piece straight to disk,
/// so memory stays flat at one chunk and the total transfer is bounded only by
/// free disk.
///
/// Ordering is enforced LOUDLY (per `feedback_test_discipline` — no silent
/// partial writes): `offset` MUST equal the current length of the in-progress
/// `.part` file. `offset == 0` starts (or restarts) the transfer by truncating
/// any stale part; any other gap, overlap, or a chunk for an `upload_id` whose
/// part is the wrong length is an error — so a dropped/duplicated/reordered
/// chunk surfaces instead of corrupting the assembled bundle.
/// `total_bytes` (0.40.22, optional for wire-compat with pre-0.40.22
/// clients): the transfer's full expected size, sent with the offset-0
/// chunk. It lets the daemon reject a doomed transfer UP FRONT — over the
/// [`MAX_TRANSFER_SIZE`] ceiling or larger than the destination volume's
/// free space — instead of after gigabytes of chunks. The running
/// `offset + len` ceiling check below is the authoritative backstop, so an
/// omitted (or dishonest) `total_bytes` still can't exceed the cap.
pub fn write_upload_chunk(
    dir: &str,
    filename: &str,
    upload_id: &str,
    offset: u64,
    bytes: &[u8],
    is_last: bool,
    total_bytes: Option<u64>,
) -> Result<Option<PathBuf>, String> {
    use std::io::{Seek, SeekFrom, Write};
    if bytes.len() as u64 > MAX_UPLOAD_CHUNK_SIZE {
        return Err(format!(
            "Upload chunk too large ({} bytes > {MAX_UPLOAD_CHUNK_SIZE} max)",
            bytes.len()
        ));
    }
    // Running ceiling — authoritative regardless of what (if anything) the
    // client declared in `total_bytes`.
    if offset.saturating_add(bytes.len() as u64) > MAX_TRANSFER_SIZE {
        return Err(format!(
            "Transfer exceeds the {} GiB ceiling",
            MAX_TRANSFER_SIZE / (1024 * 1024 * 1024)
        ));
    }
    if dir.is_empty() {
        return Err("Destination directory is empty".to_string());
    }
    // Create the destination BEFORE validate_path — its canonicalize() requires
    // the path to exist (mirrors write_upload).
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create destination dir: {e}"))?;
    let dir_path = validate_path(dir)?;
    if !dir_path.is_dir() {
        return Err(format!("Destination is not a directory: {dir}"));
    }
    // Up-front viability checks on the FIRST chunk of a declared-size
    // transfer: ceiling, then free disk. Both fail the transfer before any
    // meaningful bytes move.
    if offset == 0 {
        if let Some(total) = total_bytes {
            if total > MAX_TRANSFER_SIZE {
                return Err(format!(
                    "Transfer of {total} bytes exceeds the {} GiB ceiling",
                    MAX_TRANSFER_SIZE / (1024 * 1024 * 1024)
                ));
            }
            if let Some(free) = free_disk_space(&dir_path) {
                if total > free {
                    return Err(format!(
                        "Not enough free disk space: transfer needs {total} bytes, volume has {free}"
                    ));
                }
            }
        }
    }

    let part_path = dir_path.join(format!(".k2-upload-{}.part", sanitize_upload_id(upload_id)));

    // Enforce ordered, gap-free appends. A missing part counts as length 0.
    let current_len = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
    if offset != 0 && current_len != offset {
        return Err(format!(
            "Upload chunk out of order for id {upload_id}: expected offset {current_len}, got {offset}"
        ));
    }

    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part_path)
        .map_err(map_io_err)?;
    // offset==0 is a fresh (or restarted) transfer — clear any stale bytes.
    if offset == 0 {
        f.set_len(0).map_err(map_io_err)?;
    }
    f.seek(SeekFrom::Start(offset)).map_err(map_io_err)?;
    f.write_all(bytes).map_err(map_io_err)?;

    if is_last {
        // fsync BEFORE the rename: the atomic rename must only ever publish
        // fully-durable bytes (a crash between rename and a lazy flush would
        // leave a complete-looking but truncated file at the final name).
        f.sync_all().map_err(map_io_err)?;
        drop(f);
        let safe_name = sanitize_filename(filename);
        let target = collision_free_path(&dir_path, &safe_name);
        fs::rename(&part_path, &target).map_err(map_io_err)?;
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

// ── move / copy / delete / rename / create / duplicate ────────────────

fn disambiguate_path(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = target.parent().unwrap_or(Path::new("/"));
    let copy_name = parent.join(format!("{stem} copy{ext}"));
    if !copy_name.exists() {
        return copy_name;
    }
    for i in 2..100 {
        let numbered = parent.join(format!("{stem} copy {i}{ext}"));
        if !numbered.exists() {
            return numbered;
        }
    }
    target.to_path_buf()
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let mut visited = HashSet::new();
    copy_dir_inner(src, dst, &mut visited)
}

fn copy_dir_inner(
    src: &Path,
    dst: &Path,
    visited: &mut HashSet<u64>,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = fs::metadata(src) {
            if !visited.insert(meta.ino()) {
                return Ok(()); // symlink cycle: skip
            }
        }
    }
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create directory: {e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read directory: {e}"))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_inner(&src_path, &dst_path, visited)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {}: {e}", src_path.display()))?;
        }
    }
    Ok(())
}

fn resolve_destination(source: &Path, destination: &Path) -> PathBuf {
    if destination.is_dir() {
        if let Some(name) = source.file_name() {
            return destination.join(name);
        }
    }
    destination.to_path_buf()
}

pub fn move_files(sources: &[String], destination: &str) -> Result<(), String> {
    let dest = Path::new(destination);
    if !dest.exists() {
        return Err(format!("Destination does not exist: {destination}"));
    }
    for source in sources {
        let src = Path::new(source);
        if !src.exists() {
            return Err(format!("Source does not exist: {source}"));
        }
        let target = resolve_destination(src, dest);
        if let (Ok(s), Some(parent)) = (src.canonicalize(), target.parent()) {
            if let Ok(dp) = parent.canonicalize() {
                if dp.starts_with(&s) {
                    return Err("Cannot move a folder into itself".to_string());
                }
            }
        }
        let target = disambiguate_path(&target);
        if fs::rename(src, &target).is_err() {
            if src.is_dir() {
                copy_dir_recursive(src, &target)?;
                fs::remove_dir_all(src)
                    .map_err(|e| format!("Moved but failed to remove source: {e}"))?;
            } else {
                fs::copy(src, &target).map_err(|e| format!("Failed to copy: {e}"))?;
                fs::remove_file(src)
                    .map_err(|e| format!("Copied but failed to remove source: {e}"))?;
            }
        }
    }
    Ok(())
}

pub fn copy_files(sources: &[String], destination: &str) -> Result<(), String> {
    let dest = Path::new(destination);
    if !dest.exists() {
        return Err(format!("Destination does not exist: {destination}"));
    }
    for source in sources {
        let src = Path::new(source);
        if !src.exists() {
            return Err(format!("Source does not exist: {source}"));
        }
        let target = resolve_destination(src, dest);
        let target = disambiguate_path(&target);
        if src.is_dir() {
            copy_dir_recursive(src, &target)?;
        } else {
            fs::copy(src, &target).map_err(|e| format!("Failed to copy: {e}"))?;
        }
    }
    Ok(())
}

pub fn delete(paths: &[String], permanent: bool) -> Result<(), String> {
    for path_str in paths {
        validate_path(path_str)?;
        let p = Path::new(path_str);
        if !p.exists() {
            return Err(format!("Does not exist: {path_str}"));
        }
        if permanent {
            if p.is_dir() {
                fs::remove_dir_all(p)
                    .map_err(|e| format!("Failed to delete directory: {e}"))?;
            } else {
                fs::remove_file(p).map_err(|e| format!("Failed to delete file: {e}"))?;
            }
        } else {
            trash::delete(p).map_err(|e| format!("Failed to trash {path_str}: {e}"))?;
        }
    }
    Ok(())
}

pub fn rename(old_path: &str, new_name: &str) -> Result<String, String> {
    let old = Path::new(old_path);
    if !old.exists() {
        return Err(format!("Does not exist: {old_path}"));
    }
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\0') {
        return Err("Invalid file name".to_string());
    }
    let parent = old.parent().ok_or("Cannot determine parent directory")?;
    let new_path = parent.join(new_name);
    let is_case_rename = old
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase() == new_name.to_lowercase())
        .unwrap_or(false);
    if is_case_rename {
        fs::rename(old, &new_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Rename failed: {e}")
            }
        })?;
    } else {
        rename_exclusive(old, &new_path)?;
    }
    Ok(new_path.to_string_lossy().to_string())
}

pub fn create_entry(path: &str, is_directory: bool) -> Result<(), String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let p = Path::new(&path);
    if p.exists() {
        return Err(format!("Already exists: {path}"));
    }
    if is_directory {
        fs::create_dir_all(p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Failed to create directory: {e}")
            }
        })?;
    } else {
        if let Some(parent) = p.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent directory: {e}"))?;
            }
        }
        fs::write(p, "").map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Failed to create file: {e}")
            }
        })?;
    }
    Ok(())
}

pub fn duplicate(path: &str) -> Result<String, String> {
    let src = Path::new(path);
    if !src.exists() {
        return Err(format!("Does not exist: {path}"));
    }
    let parent = src.parent().unwrap_or(Path::new("/"));
    let target = disambiguate_path(&resolve_destination(src, parent));
    if src.is_dir() {
        copy_dir_recursive(src, &target)?;
    } else {
        fs::copy(src, &target).map_err(|e| format!("Failed to duplicate: {e}"))?;
    }
    Ok(target.to_string_lossy().to_string())
}

// ── open-finder / open-external ───────────────────────────────────────

/// Reveal the path in the user's Finder window. Daemon-side caveat:
/// when the daemon runs on a different machine (K2SO Connect), this
/// opens Finder on the **daemon's** machine, not the user's. Phase 3.1
/// will route this through the renderer; for now Bundled mode is the
/// only use case and the behavior matches the pre-Phase-2 Tauri
/// implementation.
pub fn open_in_finder(path: &str) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map_err(|e| format!("Failed to open Finder: {e}"))?;
    Ok(())
}

/// Open a URL in the system default browser. Same K2SO Connect caveat
/// as `open_in_finder` — opens on the daemon's machine.
pub fn open_external(url: &str) -> Result<String, String> {
    let clean: String = url
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();
    let clean = clean.trim().to_string();
    if clean.is_empty() {
        return Err("URL is empty after cleaning".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/usr/bin/open")
            .arg(&clean)
            .output()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
        if output.status.success() {
            return Ok(format!("Opened: {clean}"));
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("open failed: {stderr}"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _output = Command::new("xdg-open")
            .arg(&clean)
            .output()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
        return Ok(format!("Opened: {clean}"));
    }
    #[allow(unreachable_code)]
    Err("Unsupported platform".to_string())
}

// ── clipboard-paths (macOS Finder-copy bridge) ────────────────────────

/// Read file paths from the macOS general pasteboard. Finder's CMD+C
/// writes `NSFilenamesPboardType`, which WKWebView does not expose via
/// the web Clipboard API. The terminal paste handler calls this to
/// match the drag-drop behavior for Finder-copied files.
///
/// K2SO Connect caveat: returns paths from the **daemon's** pasteboard.
/// In Bundled mode (daemon + Tauri on the same machine) this is the
/// user's pasteboard — same behavior as the pre-Phase-2 Tauri command.
/// In Connect mode the renderer should fall back to a renderer-side
/// path (Phase 3.1) since the user's clipboard lives on their own
/// machine.
#[cfg(target_os = "macos")]
pub fn clipboard_read_file_paths() -> Result<Vec<String>, String> {
    use cocoa::appkit::{NSFilenamesPboardType, NSPasteboard};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSArray, NSString};
    use objc::{msg_send, sel, sel_impl};
    use std::ffi::CStr;

    unsafe {
        let pb: id = NSPasteboard::generalPasteboard(nil);
        let list: id = msg_send![pb, propertyListForType: NSFilenamesPboardType];
        if list == nil {
            return Ok(Vec::new());
        }
        let count = NSArray::count(list);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let ns_str: id = NSArray::objectAtIndex(list, i);
            let c_str = NSString::UTF8String(ns_str);
            if !c_str.is_null() {
                paths.push(CStr::from_ptr(c_str).to_string_lossy().into_owned());
            }
        }
        Ok(paths)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn clipboard_read_file_paths() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    //! Tests for the Phase 2 Unit 6 filesystem surface.
    //!
    //! All filesystem mutation happens under a per-test tempdir so the
    //! developer's real workspace is never touched. We intentionally
    //! avoid trash/recycle-bin (`delete(.., permanent=false)`) tests —
    //! on macOS those go through AppleScript and trigger Finder Touch ID
    //! prompts that hang `cargo test`. The permanent-delete branch is
    //! still exercised.
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Tiny in-crate tempdir helper, modeled after the one in
    /// `app_settings::tests`. Each test gets a unique directory under
    /// `std::env::temp_dir()` that auto-deletes on drop.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("k2so-fs_commands-{label}-{pid}-{nanos}"));
            fs::create_dir_all(&path).expect("create tempdir");
            Self { path }
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn s(&self) -> String {
            self.path.to_string_lossy().to_string()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // ── validate_path ───────────────────────────────────────────────

    #[test]
    fn validate_path_rejects_empty() {
        assert_eq!(validate_path(""), Err("Path is empty".to_string()));
    }

    #[test]
    fn validate_path_accepts_existing_dir_and_canonicalizes() {
        let tmp = TempDir::new("validate");
        let p = validate_path(&tmp.s()).expect("existing dir validates");
        // canonicalize resolves symlinks; on macOS /tmp is a symlink to
        // /private/tmp, so we just assert the file_name is preserved.
        assert_eq!(p.file_name(), tmp.path().file_name());
    }

    #[test]
    fn validate_path_accepts_nonexistent_path_with_existing_parent() {
        let tmp = TempDir::new("validate-parent");
        let candidate = tmp.path().join("not-yet-written.txt");
        let p = validate_path(candidate.to_str().unwrap()).expect("parent exists, path itself need not");
        assert_eq!(p, candidate);
    }

    #[test]
    fn validate_path_rejects_when_parent_does_not_exist() {
        let tmp = TempDir::new("validate-noparent");
        let nested = tmp.path().join("a/b/c/never.txt");
        let err = validate_path(nested.to_str().unwrap()).expect_err("nonexistent parent must fail");
        assert!(err.starts_with("Parent directory does not exist"), "got: {err}");
    }

    // ── read_dir ───────────────────────────────────────────────────

    #[test]
    fn read_dir_lists_entries_with_directory_first_sort() {
        let tmp = TempDir::new("readdir");
        fs::create_dir(tmp.path().join("alpha")).unwrap();
        fs::write(tmp.path().join("beta.txt"), b"x").unwrap();
        fs::write(tmp.path().join("zeta.md"), b"y").unwrap();
        let entries = read_dir(&tmp.s(), false).expect("read_dir");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // Directories sort before files; within each group alphabetical.
        assert_eq!(names, vec!["alpha", "beta.txt", "zeta.md"]);
        assert!(entries[0].is_directory);
        assert!(!entries[1].is_directory);
        assert_eq!(entries[1].size, 1);
    }

    #[test]
    fn read_dir_filters_hidden_when_show_hidden_false() {
        let tmp = TempDir::new("readdir-hidden");
        fs::write(tmp.path().join(".secret"), b"").unwrap();
        fs::write(tmp.path().join("visible.txt"), b"").unwrap();
        let hidden = read_dir(&tmp.s(), false).expect("read_dir hidden=false");
        assert_eq!(hidden.len(), 1, "hidden file must be filtered: {hidden:?}");
        let with_hidden = read_dir(&tmp.s(), true).expect("read_dir hidden=true");
        assert_eq!(with_hidden.len(), 2);
    }

    #[test]
    fn read_dir_handles_unicode_and_spaces_in_names() {
        let tmp = TempDir::new("readdir-unicode");
        fs::write(tmp.path().join("hello world.txt"), b"a").unwrap();
        fs::write(tmp.path().join("café.md"), b"b").unwrap();
        fs::write(tmp.path().join("日本語.txt"), b"c").unwrap();
        let entries = read_dir(&tmp.s(), false).expect("read_dir");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"hello world.txt"));
        assert!(names.contains(&"café.md"));
        assert!(names.contains(&"日本語.txt"));
    }

    // ── search_tree ────────────────────────────────────────────────

    #[test]
    fn search_tree_empty_query_returns_empty_results() {
        let tmp = TempDir::new("search-empty");
        fs::write(tmp.path().join("foo.txt"), b"").unwrap();
        let r = search_tree(&tmp.s(), "   ", false, 100).expect("search_tree empty");
        assert!(r.is_empty(), "whitespace query must short-circuit");
    }

    #[test]
    fn search_tree_finds_matches_and_skips_blacklisted_dirs() {
        let tmp = TempDir::new("search-deep");
        fs::create_dir_all(tmp.path().join("src/inner")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::write(tmp.path().join("src/inner/target.rs"), b"x").unwrap();
        fs::write(tmp.path().join("node_modules/should_not_find.rs"), b"x").unwrap();
        let r = search_tree(&tmp.s(), "should_not_find", false, 10).expect("search");
        assert!(r.is_empty(), "node_modules must be skipped: {r:?}");
        let r2 = search_tree(&tmp.s(), "target", false, 10).expect("search hit");
        assert!(r2.iter().any(|m| m.name == "target.rs"));
    }

    #[test]
    fn search_tree_respects_max_results() {
        let tmp = TempDir::new("search-cap");
        for i in 0..10 {
            fs::write(tmp.path().join(format!("match-{i}.txt")), b"x").unwrap();
        }
        let r = search_tree(&tmp.s(), "match", false, 3).expect("search");
        assert_eq!(r.len(), 3, "max_results must cap output");
    }

    // ── read_file / read_binary_file ───────────────────────────────

    #[test]
    fn read_file_returns_content_for_text() {
        let tmp = TempDir::new("readfile-text");
        let p = tmp.path().join("greeting.txt");
        fs::write(&p, "hi there\n").unwrap();
        let r = read_file(p.to_str().unwrap()).expect("read text");
        assert_eq!(r.content, "hi there\n");
        assert_eq!(r.name, "greeting.txt");
    }

    #[test]
    fn read_file_rejects_binary_via_nul_byte_detection() {
        let tmp = TempDir::new("readfile-binary");
        let p = tmp.path().join("blob.bin");
        fs::write(&p, [0xffu8, 0x00, 0x01, 0x02]).unwrap();
        let err = read_file(p.to_str().unwrap()).expect_err("nul byte must reject");
        assert!(err.contains("binary"), "got: {err}");
    }

    #[test]
    fn read_file_enforces_max_file_size() {
        let tmp = TempDir::new("readfile-large");
        let p = tmp.path().join("huge.txt");
        let buf = vec![b'a'; (MAX_FILE_SIZE + 1) as usize];
        fs::write(&p, &buf).unwrap();
        let err = read_file(p.to_str().unwrap()).expect_err("over 10MB must reject");
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn read_binary_file_enforces_50mb_cap() {
        // Sentinel: we don't actually allocate a 50MB file in tests
        // (slow + disk pressure). Instead assert the cap constant
        // itself is the documented limit + that read_binary_file
        // surfaces the size limit error message format for an
        // *under-cap* file (positive path) and that we can read it
        // intact.
        assert_eq!(MAX_BINARY_SIZE, 50 * 1024 * 1024);
        let tmp = TempDir::new("readbin-small");
        let p = tmp.path().join("small.bin");
        let payload: Vec<u8> = (0u8..=255).collect();
        fs::write(&p, &payload).unwrap();
        let got = read_binary_file(p.to_str().unwrap()).expect("under-cap read");
        assert_eq!(got, payload);
    }

    #[test]
    fn read_binary_file_missing_file_returns_parent_exists_error() {
        let tmp = TempDir::new("readbin-missing");
        let p = tmp.path().join("nope.bin");
        // parent exists -> validate_path returns Ok; then existence
        // check inside read_binary_file fires.
        let err = read_binary_file(p.to_str().unwrap()).expect_err("must fail");
        assert!(err.contains("not found"), "got: {err}");
    }

    // ── write_file ─────────────────────────────────────────────────

    #[test]
    fn write_file_creates_and_overwrites() {
        let tmp = TempDir::new("write");
        let p = tmp.path().join("out.txt");
        write_file(p.to_str().unwrap(), "first").expect("write 1");
        assert_eq!(fs::read_to_string(&p).unwrap(), "first");
        write_file(p.to_str().unwrap(), "second").expect("write 2");
        assert_eq!(fs::read_to_string(&p).unwrap(), "second");
    }

    #[test]
    fn write_file_fails_when_parent_directory_missing() {
        let tmp = TempDir::new("write-noparent");
        let nested = tmp.path().join("missing/intermediate/file.txt");
        let err = write_file(nested.to_str().unwrap(), "x").expect_err("must fail");
        assert!(err.contains("Parent directory does not exist"), "got: {err}");
    }

    // ── create_entry / rename / duplicate ──────────────────────────

    #[test]
    fn create_entry_makes_file_or_dir_and_rejects_existing() {
        let tmp = TempDir::new("create");
        let f = tmp.path().join("new.txt");
        create_entry(f.to_str().unwrap(), false).expect("create file");
        assert!(f.exists() && f.is_file());
        let d = tmp.path().join("new_dir");
        create_entry(d.to_str().unwrap(), true).expect("create dir");
        assert!(d.exists() && d.is_dir());
        let err = create_entry(f.to_str().unwrap(), false).expect_err("already exists");
        assert!(err.contains("Already exists"), "got: {err}");
    }

    #[test]
    fn rename_rejects_invalid_names() {
        let tmp = TempDir::new("rename-invalid");
        let f = tmp.path().join("a.txt");
        fs::write(&f, "").unwrap();
        assert!(rename(f.to_str().unwrap(), "").is_err());
        assert!(rename(f.to_str().unwrap(), "../escape").is_err());
        // NUL byte
        assert!(rename(f.to_str().unwrap(), "bad\0name").is_err());
    }

    #[test]
    fn rename_succeeds_for_simple_case_and_returns_new_path() {
        let tmp = TempDir::new("rename-ok");
        let f = tmp.path().join("old.txt");
        fs::write(&f, "data").unwrap();
        let new_path = rename(f.to_str().unwrap(), "renamed.txt").expect("rename");
        assert!(new_path.ends_with("renamed.txt"));
        assert!(!f.exists());
        assert!(tmp.path().join("renamed.txt").exists());
    }

    #[test]
    fn duplicate_creates_copy_with_disambiguated_name() {
        let tmp = TempDir::new("dup");
        let f = tmp.path().join("doc.txt");
        fs::write(&f, "hello").unwrap();
        let dup1 = duplicate(f.to_str().unwrap()).expect("dup1");
        let dup2 = duplicate(f.to_str().unwrap()).expect("dup2");
        assert!(dup1.ends_with("doc copy.txt"), "got: {dup1}");
        // Second duplicate of the *original* should see "doc copy.txt"
        // already exists and pick "doc copy 2.txt".
        assert!(dup2.ends_with("doc copy 2.txt"), "got: {dup2}");
        // Content preserved.
        assert_eq!(fs::read_to_string(&dup1).unwrap(), "hello");
    }

    // ── copy / move / delete (permanent only) ──────────────────────

    #[test]
    fn copy_files_into_destination_directory() {
        let tmp = TempDir::new("copy");
        let src = tmp.path().join("source.txt");
        fs::write(&src, "payload").unwrap();
        let dest = tmp.path().join("destdir");
        fs::create_dir(&dest).unwrap();
        copy_files(&[src.to_string_lossy().to_string()], dest.to_str().unwrap())
            .expect("copy_files");
        assert!(dest.join("source.txt").exists());
        // Source still present (it's a copy).
        assert!(src.exists());
    }

    #[test]
    fn copy_files_rejects_missing_destination() {
        let tmp = TempDir::new("copy-no-dest");
        let src = tmp.path().join("file.txt");
        fs::write(&src, "").unwrap();
        let err = copy_files(
            &[src.to_string_lossy().to_string()],
            tmp.path().join("nope").to_str().unwrap(),
        )
        .expect_err("must fail");
        assert!(err.contains("Destination does not exist"), "got: {err}");
    }

    #[test]
    fn move_files_into_directory_removes_source() {
        let tmp = TempDir::new("move");
        let src = tmp.path().join("m.txt");
        fs::write(&src, "x").unwrap();
        let dest = tmp.path().join("d");
        fs::create_dir(&dest).unwrap();
        move_files(&[src.to_string_lossy().to_string()], dest.to_str().unwrap()).expect("move");
        assert!(!src.exists(), "source should be gone after move");
        assert!(dest.join("m.txt").exists());
    }

    #[test]
    fn delete_permanent_removes_file_and_directory() {
        let tmp = TempDir::new("delete");
        let f = tmp.path().join("doomed.txt");
        fs::write(&f, "").unwrap();
        let d = tmp.path().join("doomed_dir");
        fs::create_dir_all(d.join("nested")).unwrap();
        fs::write(d.join("nested/inner.txt"), "").unwrap();
        delete(
            &[
                f.to_string_lossy().to_string(),
                d.to_string_lossy().to_string(),
            ],
            true,
        )
        .expect("permanent delete");
        assert!(!f.exists());
        assert!(!d.exists());
    }

    #[test]
    fn delete_returns_error_for_missing_path() {
        let tmp = TempDir::new("delete-missing");
        let nonexist = tmp.path().join("ghost.txt");
        let err = delete(&[nonexist.to_string_lossy().to_string()], true)
            .expect_err("missing must fail");
        assert!(err.contains("Does not exist"), "got: {err}");
    }

    // ── open_external (input sanitization only) ────────────────────

    #[test]
    fn open_external_rejects_url_that_becomes_empty_after_cleaning() {
        // Pure control bytes / NUL → cleaned to "" → rejected before
        // any shell-out happens. We don't actually invoke `/usr/bin/open`
        // because that would surface a real Finder/browser tab; the
        // rejection path is what we're pinning down.
        let err = open_external("\0\0\n\r\t").expect_err("must reject empty");
        assert!(err.contains("empty"), "got: {err}");
    }

    // ── ensure_downloads_dir / write_upload (Phase 2 upload substrate) ──

    #[test]
    fn ensure_downloads_dir_creates_nested_path() {
        let tmp = TempDir::new("downloads");
        let dir = ensure_downloads_dir(&tmp.s()).expect("ensure downloads");
        assert!(dir.exists() && dir.is_dir());
        assert!(dir.ends_with(".k2/downloads"), "got: {}", dir.display());
        // Idempotent — second call must not error.
        let dir2 = ensure_downloads_dir(&tmp.s()).expect("ensure downloads idempotent");
        assert_eq!(dir, dir2);
    }

    #[test]
    fn ensure_downloads_dir_rejects_empty_workspace() {
        assert!(ensure_downloads_dir("").is_err());
    }

    #[test]
    fn write_upload_writes_bytes_and_returns_path() {
        let tmp = TempDir::new("upload-basic");
        // write_upload canonicalizes the dir (via validate_path); compare
        // against the canonical form so the /tmp→/private/tmp symlink on
        // macOS doesn't trip the equality.
        let canon = tmp.path().canonicalize().unwrap();
        let path = write_upload(&tmp.s(), "hello.txt", b"hi there").expect("write_upload");
        assert_eq!(path, canon.join("hello.txt"));
        assert_eq!(fs::read(&path).unwrap(), b"hi there");
    }

    #[test]
    fn write_upload_collision_produces_numbered_suffix() {
        let tmp = TempDir::new("upload-collide");
        let p1 = write_upload(&tmp.s(), "doc.pdf", b"a").expect("first");
        let p2 = write_upload(&tmp.s(), "doc.pdf", b"b").expect("second");
        let p3 = write_upload(&tmp.s(), "doc.pdf", b"c").expect("third");
        assert!(p1.ends_with("doc.pdf"), "got: {}", p1.display());
        assert!(p2.ends_with("doc (1).pdf"), "got: {}", p2.display());
        assert!(p3.ends_with("doc (2).pdf"), "got: {}", p3.display());
        // Each write is non-destructive — all three coexist with their bytes.
        assert_eq!(fs::read(&p1).unwrap(), b"a");
        assert_eq!(fs::read(&p2).unwrap(), b"b");
        assert_eq!(fs::read(&p3).unwrap(), b"c");
    }

    #[test]
    fn write_upload_strips_path_traversal_to_basename() {
        let tmp = TempDir::new("upload-traversal");
        let dest = tmp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let dest_canon = dest.canonicalize().unwrap();
        // A filename trying to escape via `../` must be reduced to its
        // basename and land INSIDE `dest`, never in the parent.
        let path = write_upload(
            dest.to_str().unwrap(),
            "../../../etc/evil.conf",
            b"x",
        )
        .expect("write_upload traversal");
        assert_eq!(path, dest_canon.join("evil.conf"));
        assert!(path.starts_with(&dest_canon), "must stay inside dest: {}", path.display());
        // The escaped sibling location must NOT have been written.
        assert!(!tmp.path().join("evil.conf").exists());
    }

    #[test]
    fn write_upload_strips_bare_separators_from_filename() {
        let tmp = TempDir::new("upload-sep");
        let canon = tmp.path().canonicalize().unwrap();
        // Both unix and windows-style separators reduce to the last segment.
        let p1 = write_upload(&tmp.s(), "sub/dir/file.txt", b"u").expect("unix sep");
        assert_eq!(p1, canon.join("file.txt"));
        let p2 = write_upload(&tmp.s(), "win\\dir\\other.txt", b"w").expect("win sep");
        assert_eq!(p2, canon.join("other.txt"));
    }

    #[test]
    fn write_upload_rejects_oversize() {
        let tmp = TempDir::new("upload-oversize");
        let big = vec![0u8; (MAX_UPLOAD_SIZE + 1) as usize];
        let err = write_upload(&tmp.s(), "huge.bin", &big).expect_err("over 100MB must reject");
        assert!(err.contains("too large"), "got: {err}");
        // Nothing was written.
        assert!(!tmp.path().join("huge.bin").exists());
    }

    #[test]
    fn write_upload_creates_missing_dir() {
        // Phase 3: the terminal-drop path `<ws>/.k2so/downloads/` may not
        // exist. write_upload must create it (recursively) and land the
        // file inside, not error.
        let tmp = TempDir::new("upload-nodir");
        let missing = tmp.path().join("a").join("b").join("downloads");
        assert!(!missing.exists());
        let path = write_upload(missing.to_str().unwrap(), "f.txt", b"x")
            .expect("missing dir must be auto-created");
        assert!(missing.exists() && missing.is_dir(), "dir was created");
        assert_eq!(fs::read(&path).unwrap(), b"x");
        assert!(path.ends_with("f.txt"), "got: {}", path.display());
    }

    #[test]
    fn write_upload_rejects_when_dir_is_a_file() {
        let tmp = TempDir::new("upload-isfile");
        let f = tmp.path().join("not-a-dir.txt");
        fs::write(&f, "x").unwrap();
        let err = write_upload(f.to_str().unwrap(), "g.txt", b"y")
            .expect_err("dir-is-file must fail");
        // With auto-create, `create_dir_all` on a path that's already a
        // regular file fails first (the OS won't make a dir over a file),
        // so the error surfaces from there rather than the is_dir() guard.
        // Either way it must reject loudly and write nothing.
        assert!(
            err.contains("create destination dir") || err.contains("not a directory"),
            "got: {err}",
        );
        // The intended upload file was never written.
        assert!(!tmp.path().join("g.txt").exists());
    }

    // ── write_upload_chunk (streaming upload substrate, GH #3) ──────────

    #[test]
    fn upload_chunk_assembles_ordered_chunks_into_one_file() {
        let tmp = TempDir::new("chunk-assemble");
        let canon = tmp.path().canonicalize().unwrap();
        let id = "xfer-1";
        // Three ordered chunks; only the last finalizes (returns the path).
        assert_eq!(
            write_upload_chunk(&tmp.s(), "bundle.tar.gz", id, 0, b"AAAA", false, None).expect("c0"),
            None
        );
        assert_eq!(
            write_upload_chunk(&tmp.s(), "bundle.tar.gz", id, 4, b"BBBB", false, None).expect("c1"),
            None
        );
        let final_path =
            write_upload_chunk(&tmp.s(), "bundle.tar.gz", id, 8, b"CC", true, None).expect("c2 final");
        let final_path = final_path.expect("is_last returns the assembled path");
        assert_eq!(final_path, canon.join("bundle.tar.gz"));
        assert_eq!(fs::read(&final_path).unwrap(), b"AAAABBBBCC");
        // The temp .part is consumed by the rename — no residue left behind.
        assert!(!canon.join(".k2-upload-xfer-1.part").exists());
    }

    #[test]
    fn upload_chunk_single_chunk_finalizes() {
        let tmp = TempDir::new("chunk-single");
        let canon = tmp.path().canonicalize().unwrap();
        let path = write_upload_chunk(&tmp.s(), "one.bin", "solo", 0, b"only", true, None)
            .expect("single chunk")
            .expect("is_last path");
        assert_eq!(path, canon.join("one.bin"));
        assert_eq!(fs::read(&path).unwrap(), b"only");
    }

    #[test]
    fn upload_chunk_rejects_out_of_order_offset() {
        let tmp = TempDir::new("chunk-gap");
        write_upload_chunk(&tmp.s(), "b.bin", "g", 0, b"AAAA", false, None).expect("c0");
        // A gap (expected offset 4, got 7) must reject LOUDLY, not silently
        // pad or overwrite — a dropped chunk can't corrupt the bundle.
        let err = write_upload_chunk(&tmp.s(), "b.bin", "g", 7, b"X", false, None)
            .expect_err("gap must reject");
        assert!(err.contains("out of order"), "got: {err}");
        assert!(err.contains("expected offset 4"), "got: {err}");
    }

    #[test]
    fn upload_chunk_rejects_oversize_chunk() {
        let tmp = TempDir::new("chunk-oversize");
        let big = vec![0u8; (MAX_UPLOAD_CHUNK_SIZE + 1) as usize];
        let err = write_upload_chunk(&tmp.s(), "b.bin", "o", 0, &big, false, None)
            .expect_err("over per-chunk cap must reject");
        assert!(err.contains("Upload chunk too large"), "got: {err}");
        // Nothing was written — no stray .part.
        assert!(!tmp.path().join(".k2-upload-o.part").exists());
    }

    #[test]
    fn read_file_range_returns_slices_and_total_size() {
        let tmp = TempDir::new("read-range");
        let p = tmp.path().join("data.bin");
        fs::write(&p, b"0123456789").unwrap();
        let s = p.to_string_lossy().to_string();

        let (bytes, size) = read_file_range(&s, 0, 4).expect("first slice");
        assert_eq!((bytes.as_slice(), size), (&b"0123"[..], 10));
        let (bytes, _) = read_file_range(&s, 4, 4).expect("middle slice");
        assert_eq!(bytes, b"4567");
        // Short read at EOF returns the remainder…
        let (bytes, _) = read_file_range(&s, 8, 4).expect("tail slice");
        assert_eq!(bytes, b"89");
        // …and past-EOF returns empty (not an error) so a racing truncation
        // terminates the client loop instead of wedging it.
        let (bytes, _) = read_file_range(&s, 20, 4).expect("past EOF");
        assert!(bytes.is_empty());
    }

    #[test]
    fn read_file_range_rejects_dirs_and_bad_len() {
        let tmp = TempDir::new("read-range-guards");
        let err = read_file_range(&tmp.s(), 0, 4).expect_err("dir must reject");
        assert!(err.contains("Not a file"), "got: {err}");

        let p = tmp.path().join("f");
        fs::write(&p, b"x").unwrap();
        let s = p.to_string_lossy().to_string();
        let err = read_file_range(&s, 0, 0).expect_err("len 0 must reject");
        assert!(err.contains("out of bounds"), "got: {err}");
        let err = read_file_range(&s, 0, MAX_READ_CHUNK_SIZE + 1)
            .expect_err("over-cap len must reject");
        assert!(err.contains("out of bounds"), "got: {err}");
    }

    #[test]
    fn upload_chunk_rejects_declared_total_over_ceiling() {
        let tmp = TempDir::new("chunk-total-ceiling");
        // A declared total above MAX_TRANSFER_SIZE fails on the FIRST chunk,
        // before any bytes accumulate.
        let err = write_upload_chunk(
            &tmp.s(),
            "b.bin",
            "t",
            0,
            b"AAAA",
            false,
            Some(MAX_TRANSFER_SIZE + 1),
        )
        .expect_err("over-ceiling declared total must reject");
        assert!(err.contains("ceiling"), "got: {err}");
        assert!(!tmp.path().join(".k2-upload-t.part").exists());
    }

    #[test]
    fn upload_chunk_rejects_running_length_over_ceiling() {
        let tmp = TempDir::new("chunk-run-ceiling");
        // The running offset+len cap is authoritative even with NO declared
        // total — a client that lies (or a pre-0.40.22 client) still can't
        // write past MAX_TRANSFER_SIZE. The check fires before the
        // ordered-append comparison, so a bare over-cap offset probes it.
        let err = write_upload_chunk(&tmp.s(), "b.bin", "r", MAX_TRANSFER_SIZE, b"X", false, None)
            .expect_err("running length past the ceiling must reject");
        assert!(err.contains("ceiling"), "got: {err}");
    }

    #[test]
    fn upload_chunk_accepts_declared_total_that_fits() {
        let tmp = TempDir::new("chunk-total-ok");
        // An honest, in-ceiling total passes the up-front viability check
        // (incl. the free-disk probe against the real volume).
        let path = write_upload_chunk(&tmp.s(), "b.bin", "ok", 0, b"DATA", true, Some(4))
            .expect("declared-total transfer")
            .expect("finalized path");
        assert_eq!(fs::read(&path).unwrap(), b"DATA");
    }

    #[test]
    fn free_disk_space_reports_nonzero_on_a_real_volume() {
        // The probe must work on the platform the daemon ships on (unix) —
        // a silent None there would disable the disk-viability gate.
        let free = free_disk_space(Path::new("/")).expect("statvfs on / must succeed on unix");
        assert!(free > 0, "free space on / reported as 0");
    }

    #[test]
    fn upload_chunk_offset_zero_restarts_stale_part() {
        let tmp = TempDir::new("chunk-restart");
        // A prior, abandoned transfer left a longer part under the same id.
        write_upload_chunk(&tmp.s(), "b.bin", "r", 0, b"STALEXXXXXXXX", false, None).expect("stale");
        // Restarting at offset 0 truncates it, so the final bytes are ONLY
        // the new transfer — no leftover tail from the abandoned one.
        write_upload_chunk(&tmp.s(), "b.bin", "r", 0, b"NEW", false, None).expect("restart c0");
        let path = write_upload_chunk(&tmp.s(), "b.bin", "r", 3, b"!", true, None)
            .expect("restart final")
            .expect("path");
        assert_eq!(fs::read(&path).unwrap(), b"NEW!");
    }

    #[test]
    fn upload_chunk_finalize_is_collision_free() {
        let tmp = TempDir::new("chunk-collide");
        // An existing file with the target name must not be clobbered — the
        // finalize uses the same numbered-suffix scheme as write_upload.
        write_upload_chunk(&tmp.s(), "dup.bin", "a", 0, b"first", true, None).expect("first");
        let p2 = write_upload_chunk(&tmp.s(), "dup.bin", "b", 0, b"second", true, None)
            .expect("second")
            .expect("path");
        assert!(p2.ends_with("dup (1).bin"), "got: {}", p2.display());
        assert_eq!(fs::read(tmp.path().join("dup.bin")).unwrap(), b"first");
        assert_eq!(fs::read(&p2).unwrap(), b"second");
    }

    #[test]
    fn upload_chunk_sanitizes_id_so_part_stays_in_dir() {
        let tmp = TempDir::new("chunk-idsanitize");
        // A malicious upload_id with separators must not escape `dir`; it's
        // reduced to safe chars, so the .part lands inside the tempdir.
        write_upload_chunk(&tmp.s(), "b.bin", "../../evil", 0, b"x", false, None).expect("sanitized id");
        // No traversal happened: the parent dir got no stray .part file.
        let parent = tmp.path().parent().unwrap();
        let strays: Vec<_> = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".k2-upload-"))
            .filter(|e| e.path().parent() == Some(parent))
            .collect();
        assert!(strays.is_empty(), "an .part escaped into the parent dir");
    }
}
