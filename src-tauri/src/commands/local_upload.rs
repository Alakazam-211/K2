//! K2 Connect remote-files Phase 2 — read a LOCAL file's bytes for upload.
//!
//! ## Why a Tauri command, not a daemon route?
//!
//! `feedback_daemon_first.md` puts logic in the daemon, but this is a
//! deliberate HOST-side exception (same class as `worktree.rs` /
//! `connect_hosts.rs`): Tauri's native drag-drop hands the renderer a
//! local file PATH on the USER's machine but no bytes. When the daemon
//! lives on a remote machine (K2 Connect), it cannot read that path —
//! the file is on the client's disk. So the renderer must read the bytes
//! HERE, base64-encode them, and POST them to the remote daemon's
//! `/cli/fs/upload-binary`. This command is the "read local bytes" half.
//!
//! Size cap mirrors the daemon's server-side `MAX_UPLOAD_SIZE` (100MB):
//! we reject oversize BEFORE encoding so a huge drop fails fast on the
//! client instead of allocating a ~133MB base64 string. The daemon
//! re-checks the decoded length as the authoritative gate.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

/// Same 100MB cap the daemon enforces on the decoded payload. Reject here
/// (pre-encode) so an oversize local file fails fast client-side.
const MAX_LOCAL_UPLOAD_SIZE: u64 = 100 * 1024 * 1024;

/// Read a local file's raw bytes and return them base64-encoded. The
/// renderer pairs this with `POST /cli/fs/upload-binary` (host-aware via
/// `daemonCliPost`) to move a dropped file onto the active daemon's disk.
#[tauri::command]
pub async fn read_local_file_base64(path: String) -> Result<String, String> {
    // CRITICAL: this MUST be `async` + run on a blocking thread. A
    // synchronous `#[tauri::command]` runs on the webview's MAIN thread, so
    // reading + base64-encoding a file (up to 100MB) there freezes the UI —
    // a beach-ball / lockup on every remote drag-drop. `spawn_blocking`
    // moves the blocking read + encode off the main thread.
    tauri::async_runtime::spawn_blocking(move || {
        if path.is_empty() {
            return Err("Path is empty".to_string());
        }
        let meta = std::fs::metadata(&path).map_err(|e| format!("Cannot stat file: {e}"))?;
        if !meta.is_file() {
            return Err(format!("Not a file: {path}"));
        }
        if meta.len() > MAX_LOCAL_UPLOAD_SIZE {
            return Err("File too large (>100MB)".to_string());
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("Cannot read file: {e}"))?;
        Ok(B64.encode(&bytes))
    })
    .await
    .map_err(|e| format!("read task failed: {e}"))?
}

/// Per-call ceiling for a range read — the streaming "Clone to" upload (GH #3)
/// reads the bundle in slices so neither the client nor the daemon ever holds
/// the whole multi-GB file. This bounds ONE slice (not the file), keeping the
/// base64 string small; a caller asking for more is a bug, so reject loudly.
const MAX_RANGE_LEN: u64 = 32 * 1024 * 1024;

/// Stat a local file and return its size in bytes. Paired with
/// [`read_local_file_range`] so the renderer can decide between a single-shot
/// upload (small bundles → `/cli/fs/upload-binary`) and a chunked stream
/// (large bundles → `/cli/fs/upload-chunk`) WITHOUT reading the file first.
#[tauri::command]
pub async fn local_file_size(path: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if path.is_empty() {
            return Err("Path is empty".to_string());
        }
        let meta = std::fs::metadata(&path).map_err(|e| format!("Cannot stat file: {e}"))?;
        if !meta.is_file() {
            return Err(format!("Not a file: {path}"));
        }
        Ok(meta.len())
    })
    .await
    .map_err(|e| format!("size task failed: {e}"))?
}

/// Read up to `len` bytes of a local file starting at `offset`, returned
/// base64-encoded. The streaming counterpart to [`read_local_file_base64`]:
/// the renderer loops this over a large clone bundle, POSTing each slice to
/// the remote daemon's `/cli/fs/upload-chunk`, so the whole file never lives
/// in memory on either side. A short read at EOF returns fewer bytes (the
/// caller advances by the DECODED length); `offset` past EOF returns empty.
#[tauri::command]
pub async fn read_local_file_range(
    path: String,
    offset: u64,
    len: u64,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        if path.is_empty() {
            return Err("Path is empty".to_string());
        }
        if len > MAX_RANGE_LEN {
            return Err(format!("range len {len} exceeds {MAX_RANGE_LEN} max"));
        }
        let mut f = std::fs::File::open(&path).map_err(|e| format!("Cannot open file: {e}"))?;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("Cannot seek to {offset}: {e}"))?;
        // Read up to `len`, looping so a short OS read mid-file still fills the
        // buffer; stop only at a genuine EOF (read returns 0).
        let mut buf = vec![0u8; len as usize];
        let mut filled = 0usize;
        while filled < buf.len() {
            match f.read(&mut buf[filled..]) {
                Ok(0) => break, // EOF
                Ok(n) => filled += n,
                Err(e) => return Err(format!("Cannot read range: {e}")),
            }
        }
        buf.truncate(filled);
        Ok(B64.encode(&buf))
    })
    .await
    .map_err(|e| format!("read-range task failed: {e}"))?
}
