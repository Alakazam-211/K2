//! Resumable chunked upload for app guests (`k2skn_` passes).
//!
//! The owner/Member route (`fs_commands::write_upload_chunk`) keys its
//! temp `.part` on the client's `upload_id` alone. A guest upload is
//! keyed on WHO is uploading as well: the part name is a hash of the
//! principal, the room, the destination folder, the file name and the
//! client's `upload_id`, so one guest can never append to (or resume)
//! another guest's transfer by reusing an id.
//!
//! The daemon route (`fs_routes`) jails the destination folder inside
//! the room first; this module never decides WHERE a guest may write,
//! only HOW the bytes land:
//!
//! - ordered appends to a dot-named `.part` in the destination folder
//!   (same volume, so publishing is a link/rename, never a copy);
//! - the part is opened with `O_NOFOLLOW` and must be a regular file, so
//!   a symlink planted at the part name can't redirect the write;
//! - the final chunk publishes WITHOUT replacing anything: a hard link
//!   to the first free name (`name`, `name (1)`, …), which fails instead
//!   of overwriting, then the part is removed;
//! - a small ledger under `~/.k2/upload-parts/` records each live guest
//!   part so [`sweep_abandoned`] can delete parts nobody finished.
//!
//! No product size limit beyond the existing chunked-transfer ceiling
//! (`MAX_TRANSFER_SIZE`): each request carries at most one chunk
//! (`MAX_UPLOAD_CHUNK_SIZE`), which bounds memory, not the file.

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::fs_commands::{free_disk_space, sanitize_filename, MAX_TRANSFER_SIZE, MAX_UPLOAD_CHUNK_SIZE};

/// File-name prefix of a guest upload's temp part.
pub const GUEST_PART_PREFIX: &str = ".k2-upload-g-";
/// File-name suffix of a guest upload's temp part.
pub const GUEST_PART_SUFFIX: &str = ".part";

/// Largest JSON request body that can still carry one valid chunk: the
/// base64 of [`MAX_UPLOAD_CHUNK_SIZE`] bytes plus 1 MiB for the other
/// fields (paths, ids) and encoders that escape `/` as `\/`. A body over
/// this can never succeed, so the daemon and the app gateway refuse it
/// with 413 before reading it.
pub const MAX_UPLOAD_CHUNK_BODY: usize =
    (MAX_UPLOAD_CHUNK_SIZE as usize).div_ceil(3) * 4 + 1024 * 1024;

/// Longest client `upload_id` accepted (it is hashed, never used as a name).
pub const MAX_UPLOAD_ID_LEN: usize = 200;

/// How long an unfinished guest part may sit untouched before
/// [`sweep_abandoned`] deletes it. Long enough to resume an upload paused
/// over a weekend; not a size limit.
pub const ABANDONED_PART_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Why a guest chunk was refused. The route maps each to a status.
#[derive(Debug, PartialEq, Eq)]
pub enum GuestChunkError {
    /// `offset` is not the part's current length (and not 0). The client
    /// resumes from `expected`.
    OffsetMismatch { expected: u64 },
    /// One chunk over [`MAX_UPLOAD_CHUNK_SIZE`], or the transfer over
    /// [`MAX_TRANSFER_SIZE`]. 413.
    TooLarge(String),
    /// The declared `total_bytes` won't fit on the destination volume. 507.
    NoSpace(String),
    /// Bad input or the part name is taken by something that is not a
    /// regular file (a symlink, a folder). 400.
    Invalid(String),
    /// Disk error. 500.
    Io(String),
    /// Another request for the same transfer is still writing (a client
    /// retry racing its original). 409; ask `status`, then continue.
    Busy,
}

impl std::fmt::Display for GuestChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OffsetMismatch { expected } => {
                write!(f, "upload offset mismatch: resume from offset {expected}")
            }
            Self::Busy => f.write_str("another chunk of this upload is still being written"),
            Self::TooLarge(m) | Self::NoSpace(m) | Self::Invalid(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

/// Result of one accepted chunk.
#[derive(Debug, PartialEq, Eq)]
pub enum GuestChunkOutcome {
    /// Intermediate chunk: bytes now held in the part.
    Received(u64),
    /// Final chunk: the published file.
    Done(PathBuf),
}

/// The name a client `filename` lands under (its last path segment, no
/// NULs; `upload` when empty). The part name binds THIS value.
pub fn final_name(filename: &str) -> String {
    sanitize_filename(filename)
}

/// The part file name for one guest transfer. `principal` is the guest's
/// stable identity (login principal id, or the platform pass id); `room`
/// the project id; `dir` the CANONICAL destination folder; `filename` the
/// sanitized final name. Two guests with the same `upload_id` get
/// different parts.
pub fn guest_part_name(principal: &str, room: &str, dir: &Path, filename: &str, upload_id: &str) -> String {
    let mut h = Sha256::new();
    for field in [
        principal.as_bytes(),
        room.as_bytes(),
        dir.to_string_lossy().as_bytes(),
        filename.as_bytes(),
        upload_id.as_bytes(),
    ] {
        h.update((field.len() as u64).to_le_bytes());
        h.update(field);
    }
    let digest = h.finalize();
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{GUEST_PART_PREFIX}{hex}{GUEST_PART_SUFFIX}")
}

/// True for a file name shaped like [`guest_part_name`]'s output.
pub fn is_guest_part_name(name: &str) -> bool {
    name.strip_prefix(GUEST_PART_PREFIX)
        .and_then(|rest| rest.strip_suffix(GUEST_PART_SUFFIX))
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Bytes already held for this transfer (0 when no part exists). A part
/// name taken by a non-file is an error, never followed.
pub fn guest_part_len(dir: &Path, part_name: &str) -> Result<u64, GuestChunkError> {
    match fs::symlink_metadata(dir.join(part_name)) {
        Ok(m) if m.file_type().is_file() => Ok(m.len()),
        Ok(_) => Err(GuestChunkError::Invalid(
            "upload part is not a regular file".to_string(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(GuestChunkError::Io(e.to_string())),
    }
}

fn open_part_no_follow(path: &Path) -> std::io::Result<fs::File> {
    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
}

/// `dir/filename`, then `dir/stem (i).ext`.
fn numbered_name(dir: &Path, filename: &str, i: u32) -> PathBuf {
    if i == 0 {
        return dir.join(filename);
    }
    let p = Path::new(filename);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    dir.join(format!("{stem} ({i}){ext}"))
}

/// Publish `part` under the first free name in `dir` without ever
/// replacing an existing entry. A hard link fails with `AlreadyExists`
/// instead of overwriting; volumes without hard links fall back to a
/// rename after an existence check.
fn publish_no_replace(part: &Path, dir: &Path, filename: &str) -> Result<PathBuf, GuestChunkError> {
    for i in 0..10_000u32 {
        let cand = numbered_name(dir, filename, i);
        match fs::hard_link(part, &cand) {
            Ok(()) => {
                if let Err(e) = fs::remove_file(part) {
                    // Published; the ledger sweep removes the leftover part.
                    crate::log_debug!("[fs/upload-chunk] guest part not removed after publish: {e}");
                }
                return Ok(cand);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(GuestChunkError::Invalid(
                    "upload part is gone; restart from offset 0".to_string(),
                ));
            }
            Err(_) => {
                if fs::symlink_metadata(&cand).is_ok() {
                    continue;
                }
                fs::rename(part, &cand).map_err(|e| GuestChunkError::Io(e.to_string()))?;
                return Ok(cand);
            }
        }
    }
    Err(GuestChunkError::Invalid(format!("no free name for {filename}")))
}

/// Parts with a chunk being written right now. One writer per transfer:
/// the offset check and the append must not interleave with a retry of
/// the same chunk.
fn in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<PathBuf>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(Default::default)
}

/// Held while one chunk of `part` is written; released on drop.
pub struct PartWriteGuard(PathBuf);

impl PartWriteGuard {
    /// `None` when another request already holds `part`.
    pub fn try_acquire(part: &Path) -> Option<Self> {
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        if set.insert(part.to_path_buf()) {
            Some(Self(part.to_path_buf()))
        } else {
            None
        }
    }
}

impl Drop for PartWriteGuard {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Append ONE ordered chunk of a guest transfer to `dir/part_name`,
/// publishing it under `filename` (sanitized, never replacing an existing
/// file) on `is_last`.
///
/// `dir` MUST already be the canonical, jailed destination folder — the
/// route checks that. `offset == 0` starts or restarts the transfer;
/// any other offset must equal the bytes already held. A second request
/// for the same transfer while one is writing gets [`GuestChunkError::Busy`].
pub fn write_guest_upload_chunk(
    dir: &Path,
    part_name: &str,
    filename: &str,
    offset: u64,
    bytes: &[u8],
    is_last: bool,
    total_bytes: Option<u64>,
) -> Result<GuestChunkOutcome, GuestChunkError> {
    let Some(_guard) = PartWriteGuard::try_acquire(&dir.join(part_name)) else {
        return Err(GuestChunkError::Busy);
    };
    write_guest_upload_chunk_locked(dir, part_name, filename, offset, bytes, is_last, total_bytes)
}

fn write_guest_upload_chunk_locked(
    dir: &Path,
    part_name: &str,
    filename: &str,
    offset: u64,
    bytes: &[u8],
    is_last: bool,
    total_bytes: Option<u64>,
) -> Result<GuestChunkOutcome, GuestChunkError> {
    if !is_guest_part_name(part_name) {
        return Err(GuestChunkError::Invalid("bad upload part name".to_string()));
    }
    if bytes.len() as u64 > MAX_UPLOAD_CHUNK_SIZE {
        return Err(GuestChunkError::TooLarge(format!(
            "upload chunk too large ({} bytes > {MAX_UPLOAD_CHUNK_SIZE} max)",
            bytes.len()
        )));
    }
    if offset.saturating_add(bytes.len() as u64) > MAX_TRANSFER_SIZE {
        return Err(GuestChunkError::TooLarge(format!(
            "transfer exceeds the {} GiB ceiling",
            MAX_TRANSFER_SIZE / (1024 * 1024 * 1024)
        )));
    }
    let safe_name = sanitize_filename(filename);
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_dir() => {}
        _ => {
            return Err(GuestChunkError::Invalid(
                "destination is not a folder".to_string(),
            ))
        }
    }
    if offset == 0 {
        if let Some(total) = total_bytes {
            if total > MAX_TRANSFER_SIZE {
                return Err(GuestChunkError::TooLarge(format!(
                    "transfer of {total} bytes exceeds the {} GiB ceiling",
                    MAX_TRANSFER_SIZE / (1024 * 1024 * 1024)
                )));
            }
            if let Some(free) = free_disk_space(dir) {
                if total > free {
                    // A guest's message names nobody and no numbers
                    // (prd-app-resources-disk-guard-v1 §6.2).
                    return Err(GuestChunkError::NoSpace(crate::disk_guard::GUEST_NO_SPACE.to_string()));
                }
            }
        }
    }
    // C6 (disk guard, mode on): the declared total on the first chunk,
    // else each chunk, must leave the box's reserve free.
    let need = match (offset, total_bytes) {
        (0, Some(total)) => total,
        _ => bytes.len() as u64,
    };
    if crate::disk_guard::reserve_ok(dir, need).is_err() {
        return Err(GuestChunkError::NoSpace(crate::disk_guard::GUEST_NO_SPACE.to_string()));
    }

    let part = dir.join(part_name);
    let current = guest_part_len(dir, part_name)?;
    if offset != 0 && offset != current {
        return Err(GuestChunkError::OffsetMismatch { expected: current });
    }
    let mut f = open_part_no_follow(&part).map_err(|e| {
        GuestChunkError::Invalid(format!("upload part could not be opened: {e}"))
    })?;
    let meta = f.metadata().map_err(|e| GuestChunkError::Io(e.to_string()))?;
    if !meta.file_type().is_file() {
        return Err(GuestChunkError::Invalid(
            "upload part is not a regular file".to_string(),
        ));
    }
    if offset == 0 {
        f.set_len(0).map_err(|e| GuestChunkError::Io(e.to_string()))?;
    } else if meta.len() != offset {
        // Changed between the check and the open.
        return Err(GuestChunkError::OffsetMismatch { expected: meta.len() });
    }
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| GuestChunkError::Io(e.to_string()))?;
    f.write_all(bytes)
        .map_err(|e| GuestChunkError::Io(e.to_string()))?;
    let held = offset + bytes.len() as u64;
    if !is_last {
        return Ok(GuestChunkOutcome::Received(held));
    }
    // Durable bytes before the name appears (see write_upload_chunk).
    f.sync_all().map_err(|e| GuestChunkError::Io(e.to_string()))?;
    drop(f);
    publish_no_replace(&part, dir, &safe_name).map(GuestChunkOutcome::Done)
}

// ── ledger + abandoned-part sweep ─────────────────────────────────────

/// `~/.k2/upload-parts/` — one marker per live guest part.
pub fn ledger_dir() -> PathBuf {
    crate::paths::k2_home().join("upload-parts")
}

fn marker_name(part: &Path) -> String {
    let digest = Sha256::digest(part.to_string_lossy().as_bytes());
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("{hex}.path")
}

/// Record a live guest part (absolute path) so the sweep can find it.
pub fn ledger_record(ledger: &Path, part: &Path) -> Result<(), String> {
    fs::create_dir_all(ledger).map_err(|e| format!("upload ledger: {e}"))?;
    fs::write(ledger.join(marker_name(part)), part.to_string_lossy().as_bytes())
        .map_err(|e| format!("upload ledger: {e}"))
}

/// Forget a guest part (published or gone). Missing marker is fine.
pub fn ledger_clear(ledger: &Path, part: &Path) -> Result<(), String> {
    match fs::remove_file(ledger.join(marker_name(part))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("upload ledger: {e}")),
    }
}

/// What one [`sweep_abandoned`] pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Parts deleted (untouched for longer than the TTL).
    pub removed: usize,
    /// Parts still within the TTL.
    pub kept: usize,
    /// Markers dropped because their part is already gone (or is not a
    /// guest part file, which is never deleted).
    pub forgotten: usize,
}

/// Delete guest parts in `ledger` whose last write is older than `ttl`.
/// Only ever deletes a regular file whose name is a guest part name.
pub fn sweep_abandoned(ledger: &Path, ttl: Duration, now: SystemTime) -> Result<SweepReport, String> {
    let mut report = SweepReport::default();
    let entries = match fs::read_dir(ledger) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(e) => return Err(format!("upload ledger: {e}")),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("upload ledger: {e}"))?;
        let marker = entry.path();
        if marker.extension().and_then(|e| e.to_str()) != Some("path") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&marker) else {
            continue;
        };
        let part = PathBuf::from(text.trim());
        let name_ok = part
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_guest_part_name);
        let meta = fs::symlink_metadata(&part);
        let live_file = matches!(&meta, Ok(m) if m.file_type().is_file());
        if !name_ok || !part.is_absolute() || !live_file {
            let _ = fs::remove_file(&marker);
            report.forgotten += 1;
            continue;
        }
        let modified = meta
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
        if age > ttl {
            fs::remove_file(&part).map_err(|e| format!("remove {}: {e}", part.display()))?;
            let _ = fs::remove_file(&marker);
            report.removed += 1;
        } else {
            report.kept += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let p = std::env::temp_dir().join(format!(
                "k2-guest-upload-{tag}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&p).expect("mkdir tmp");
            Tmp(p.canonicalize().expect("canon tmp"))
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn part(dir: &Path, who: &str, id: &str) -> String {
        guest_part_name(who, "room-1", dir, "big.bin", id)
    }

    #[test]
    fn part_name_binds_principal_room_dir_file_and_id() {
        let d = Path::new("/r/inbox");
        let a = guest_part_name("alice", "room", d, "f.bin", "u1");
        assert!(is_guest_part_name(&a), "{a}");
        assert_eq!(a, guest_part_name("alice", "room", d, "f.bin", "u1"));
        assert_ne!(a, guest_part_name("bob", "room", d, "f.bin", "u1"));
        assert_ne!(a, guest_part_name("alice", "room2", d, "f.bin", "u1"));
        assert_ne!(a, guest_part_name("alice", "room", Path::new("/r"), "f.bin", "u1"));
        assert_ne!(a, guest_part_name("alice", "room", d, "g.bin", "u1"));
        assert_ne!(a, guest_part_name("alice", "room", d, "f.bin", "u2"));
        // Length-prefixed fields: shifting a byte between fields changes it.
        assert_ne!(
            guest_part_name("ab", "c", d, "f", "u"),
            guest_part_name("a", "bc", d, "f", "u")
        );
        assert!(!is_guest_part_name(".k2-upload-x.part"));
        assert!(!is_guest_part_name("notes.md"));
    }

    #[test]
    fn multi_chunk_resume_and_publish() {
        let t = Tmp::new("multi");
        let p = part(&t.0, "alice", "u1");
        assert_eq!(guest_part_len(&t.0, &p), Ok(0));
        assert_eq!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"AAAA", false, Some(10)),
            Ok(GuestChunkOutcome::Received(4))
        );
        // Resume: ask how much is held, continue there.
        assert_eq!(guest_part_len(&t.0, &p), Ok(4));
        assert_eq!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 4, b"BBBB", false, None),
            Ok(GuestChunkOutcome::Received(8))
        );
        // Not visible under its final name mid-upload.
        assert!(!t.0.join("big.bin").exists());
        let done = write_guest_upload_chunk(&t.0, &p, "big.bin", 8, b"CC", true, None)
            .expect("final chunk");
        let GuestChunkOutcome::Done(path) = done else {
            panic!("final chunk must publish: {done:?}");
        };
        assert_eq!(path, t.0.join("big.bin"));
        assert_eq!(fs::read(&path).expect("published"), b"AAAABBBBCC");
        assert!(!t.0.join(&p).exists(), "part removed after publish");
    }

    #[test]
    fn wrong_offset_reports_expected() {
        let t = Tmp::new("offset");
        let p = part(&t.0, "alice", "u1");
        write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"AAAA", false, None).expect("c0");
        assert_eq!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 9, b"X", false, None),
            Err(GuestChunkError::OffsetMismatch { expected: 4 })
        );
        // Another principal's part for the same id is empty: no append.
        let other = part(&t.0, "bob", "u1");
        assert_eq!(
            write_guest_upload_chunk(&t.0, &other, "big.bin", 4, b"EVIL", true, None),
            Err(GuestChunkError::OffsetMismatch { expected: 0 })
        );
        assert_eq!(fs::read(t.0.join(&p)).expect("part"), b"AAAA");
    }

    #[test]
    fn concurrent_write_to_the_same_part_is_busy() {
        let t = Tmp::new("busy");
        let p = part(&t.0, "alice", "u1");
        let held = PartWriteGuard::try_acquire(&t.0.join(&p)).expect("first writer");
        assert_eq!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"AAAA", false, None),
            Err(GuestChunkError::Busy)
        );
        // Another guest's transfer is not blocked.
        let other = part(&t.0, "bob", "u1");
        assert_eq!(
            write_guest_upload_chunk(&t.0, &other, "big.bin", 0, b"BB", false, None),
            Ok(GuestChunkOutcome::Received(2))
        );
        drop(held);
        assert_eq!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"AAAA", false, None),
            Ok(GuestChunkOutcome::Received(4))
        );
    }

    #[test]
    fn publish_never_replaces_an_existing_file() {
        let t = Tmp::new("noreplace");
        fs::write(t.0.join("big.bin"), b"keep").expect("existing");
        let p = part(&t.0, "alice", "u1");
        let done = write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"new", true, None)
            .expect("publish");
        assert_eq!(done, GuestChunkOutcome::Done(t.0.join("big (1).bin")));
        assert_eq!(fs::read(t.0.join("big.bin")).expect("kept"), b"keep");
        assert_eq!(fs::read(t.0.join("big (1).bin")).expect("new"), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_at_part_name_is_refused_and_target_untouched() {
        let t = Tmp::new("symlink");
        let outside = Tmp::new("symlink-outside");
        let target = outside.0.join("victim.txt");
        fs::write(&target, b"untouched").expect("victim");
        let p = part(&t.0, "alice", "u1");
        std::os::unix::fs::symlink(&target, t.0.join(&p)).expect("plant symlink");
        let err = write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"EVIL", false, None)
            .expect_err("symlinked part must be refused");
        assert!(matches!(err, GuestChunkError::Invalid(_)), "{err:?}");
        assert_eq!(fs::read(&target).expect("victim"), b"untouched");
    }

    #[test]
    fn oversize_chunk_and_ceiling_are_too_large() {
        let t = Tmp::new("big");
        let p = part(&t.0, "alice", "u1");
        let big = vec![0u8; MAX_UPLOAD_CHUNK_SIZE as usize + 1];
        assert!(matches!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 0, &big, false, None),
            Err(GuestChunkError::TooLarge(_))
        ));
        assert!(matches!(
            write_guest_upload_chunk(&t.0, &p, "big.bin", 0, b"x", false, Some(MAX_TRANSFER_SIZE + 1)),
            Err(GuestChunkError::TooLarge(_))
        ));
        assert!(!t.0.join(&p).exists(), "nothing written");
    }

    #[test]
    fn chunk_body_cap_fits_a_full_chunk() {
        let b64_len = (MAX_UPLOAD_CHUNK_SIZE as usize).div_ceil(3) * 4;
        assert!(MAX_UPLOAD_CHUNK_BODY > b64_len + 64 * 1024);
        assert!(MAX_UPLOAD_CHUNK_BODY < b64_len + 2 * 1024 * 1024);
    }

    #[test]
    fn sweep_removes_only_stale_guest_parts() {
        let t = Tmp::new("sweep");
        let ledger = t.0.join("ledger");
        let room = t.0.join("room");
        fs::create_dir_all(&room).expect("room");
        let stale = room.join(part(&room, "alice", "old"));
        let fresh = room.join(part(&room, "alice", "new"));
        let gone = room.join(part(&room, "alice", "gone"));
        fs::write(&stale, b"old").expect("stale");
        fs::write(&fresh, b"new").expect("fresh");
        // A marker pointing at a non-part file must never delete it.
        let precious = room.join("precious.txt");
        fs::write(&precious, b"keep").expect("precious");
        for p in [&stale, &fresh, &gone, &precious] {
            ledger_record(&ledger, p).expect("record");
        }
        // TTL 1 h, `now` = fresh's mtime; stale is backdated 3 h.
        let fresh_m = fs::metadata(&fresh).expect("m").modified().expect("mtime");
        let stale_time = fresh_m - Duration::from_secs(3 * 3600);
        let f = fs::OpenOptions::new().write(true).open(&stale).expect("open stale");
        f.set_modified(stale_time).expect("backdate stale");
        drop(f);
        let report = sweep_abandoned(&ledger, Duration::from_secs(3600), fresh_m)
            .expect("sweep");
        assert_eq!(
            report,
            SweepReport { removed: 1, kept: 1, forgotten: 2 },
            "{report:?}"
        );
        assert!(!stale.exists(), "stale part deleted");
        assert!(fresh.exists(), "fresh part kept");
        assert_eq!(fs::read(&precious).expect("precious"), b"keep");
        // Clearing a marker is idempotent.
        ledger_clear(&ledger, &fresh).expect("clear");
        ledger_clear(&ledger, &fresh).expect("clear again");
        let again = sweep_abandoned(&ledger, Duration::from_secs(3600), fresh_m).expect("sweep 2");
        assert_eq!(again, SweepReport::default());
    }
}
