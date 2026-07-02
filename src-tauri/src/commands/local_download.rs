//! 0.40.22 large-file transfers — write a downloaded file onto the LOCAL
//! disk, chunk by chunk.
//!
//! Same host-side-exception class as `local_upload.rs`: the daemon may be
//! REMOTE (K2 Connect), so "Download" means the renderer loops the
//! daemon's `GET /cli/fs/read-range` and needs somewhere local to land
//! the bytes. These commands are the write half — the byte-for-byte
//! mirror of the daemon's `write_upload_chunk`: ordered appends into a
//! dot-hidden `.part`, then fsync + collision-free rename on the final
//! chunk. Destination is FIXED to a named mode — the user's Downloads
//! directory by default, or the `~/.k2/clone-tmp` staging dir for pulled
//! clone bundles (0.40.22 "Clone to this computer") — the renderer never
//! picks an arbitrary local path.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use std::path::{Path, PathBuf};

/// The user's Downloads dir (the app's established download landing spot —
/// no prior feature wrote downloads, so the OS convention is it).
fn downloads_dir() -> Result<PathBuf, String> {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .ok_or_else(|| "Cannot resolve the Downloads directory".to_string())
}

/// Resolve a NAMED destination mode to its directory. Only two modes
/// exist — visible user downloads, and the clone-bundle staging dir the
/// local daemon's `clone/unpack` already deletes from + stale-prunes
/// (never an arbitrary renderer-chosen path):
///   - `None` / `"downloads"` → `~/Downloads` (the pre-0.40.22 behavior)
///   - `"clone-tmp"`          → `~/.k2/clone-tmp`
fn dest_dir(dest: Option<&str>) -> Result<PathBuf, String> {
    match dest {
        None | Some("downloads") => downloads_dir(),
        Some("clone-tmp") => dirs::home_dir()
            .map(|h| h.join(".k2").join("clone-tmp"))
            .ok_or_else(|| "Cannot resolve the home directory".to_string()),
        Some(other) => Err(format!("Unknown download destination mode: {other}")),
    }
}

/// Reduce a transfer id to filename-safe characters (ASCII alnum + `-` +
/// `_`) so it can never inject a separator into the `.part` name. Mirrors
/// the daemon's `sanitize_upload_id`.
fn sanitize_id(id: &str) -> String {
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

/// Reduce a filename to a safe basename (strip separators + NUL) so a
/// hostile remote path can't escape Downloads. Mirrors the daemon's
/// `sanitize_filename`.
fn sanitize_filename(filename: &str) -> String {
    let last = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let cleaned: String = last.chars().filter(|c| *c != '\0').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "download".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Non-colliding `dir/filename`, appending ` (1)`, ` (2)`, … before the
/// extension — the same scheme uploads use on the daemon side.
fn collision_free_path(dir: &Path, filename: &str) -> PathBuf {
    let initial = dir.join(filename);
    if !initial.exists() {
        return initial;
    }
    let name = Path::new(filename);
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10_000 {
        let candidate = dir.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    initial
}

fn part_path(download_id: &str, dest: Option<&str>) -> Result<PathBuf, String> {
    let dir = dest_dir(dest)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create download destination dir: {e}"))?;
    Ok(dir.join(format!(".k2-download-{}.part", sanitize_id(download_id))))
}

/// Append ONE ordered chunk of a download to `<dest dir>/.k2-download-
/// <id>.part`, finalizing (fsync + atomic collision-free rename to
/// `filename`) on `is_last`. Ordering is enforced LOUDLY exactly like the
/// daemon's upload half: `offset` must equal the part's current length
/// (`0` starts/restarts); a gap or overlap errors instead of corrupting
/// the file. Returns the final path on the last chunk, `None` otherwise.
/// `dest` picks the named destination mode (see `dest_dir`); omitted =
/// `~/Downloads`, byte-identical to the pre-0.40.22 command.
#[tauri::command]
pub async fn local_download_chunk(
    download_id: String,
    filename: String,
    offset: u64,
    base64: String,
    is_last: bool,
    dest: Option<String>,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{Seek, SeekFrom, Write};
        let bytes = B64
            .decode(base64.as_bytes())
            .map_err(|e| format!("invalid base64: {e}"))?;
        let part = part_path(&download_id, dest.as_deref())?;

        let current_len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if offset != 0 && current_len != offset {
            return Err(format!(
                "Download chunk out of order for id {download_id}: expected offset {current_len}, got {offset}"
            ));
        }

        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&part)
            .map_err(|e| format!("Cannot open part file: {e}"))?;
        if offset == 0 {
            f.set_len(0).map_err(|e| format!("Cannot truncate part: {e}"))?;
        }
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("Cannot seek part: {e}"))?;
        f.write_all(&bytes)
            .map_err(|e| format!("Cannot write chunk: {e}"))?;

        if is_last {
            // fsync BEFORE the rename — never publish lazy bytes under the
            // final name.
            f.sync_all().map_err(|e| format!("Cannot fsync: {e}"))?;
            drop(f);
            let dir = dest_dir(dest.as_deref())?;
            let target = collision_free_path(&dir, &sanitize_filename(&filename));
            std::fs::rename(&part, &target).map_err(|e| format!("Cannot finalize: {e}"))?;
            Ok(Some(target.to_string_lossy().to_string()))
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(|e| format!("download task failed: {e}"))?
}

/// Remove an in-progress download's `.part` (cancel / hard failure). A
/// missing part is fine — abort must be safe to call unconditionally.
#[tauri::command]
pub async fn local_download_abort(
    download_id: String,
    dest: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let part = part_path(&download_id, dest.as_deref())?;
        match std::fs::remove_file(&part) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Cannot remove part file: {e}")),
        }
    })
    .await
    .map_err(|e| format!("abort task failed: {e}"))?
}
