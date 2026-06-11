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
