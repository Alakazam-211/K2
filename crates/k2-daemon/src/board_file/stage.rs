//! Blobs waiting for a save: `~/.k2/board-stage/<sha256(caller)>/<hex>`. A save
//! may name only blobs its own caller staged, or parts of that file, so an agent
//! with write on file B can't copy a blob out of file A by its hash. Bounded and
//! swept, so `blob/put` can't fill a disk.

use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use k2_mpc_board::Sha256;
use sha2::{Digest, Sha256 as ShaHasher};

/// The most one image may be (the Garden's cap).
pub const MAX_BLOB_BYTES: u64 = 16 * 1024 * 1024;
/// The most one caller may have staged.
pub const MAX_CALLER_BYTES: u64 = 512 * 1024 * 1024;
/// Staged blobs nobody saved go after this.
const MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// Why a chunk wasn't taken.
#[derive(Debug)]
pub enum StageError {
    /// Status, code, hint.
    Refused(&'static str, &'static str, String),
    /// The disk.
    Io(std::io::Error),
}

impl From<std::io::Error> for StageError {
    fn from(e: std::io::Error) -> StageError {
        StageError::Io(e)
    }
}

/// The folder name for a caller: their pid can hold anything a login name can.
pub fn owner_of(pid: &str) -> String {
    ShaHasher::digest(pid.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn root() -> PathBuf {
    k2_core::paths::home_dir().unwrap_or_else(std::env::temp_dir).join(".k2").join("board-stage")
}

fn folder(owner: &str) -> PathBuf {
    root().join(owner)
}

/// `sha256-<hex>`, parsed before it got here, so the name holds only hex.
fn base(owner: &str, hash: &Sha256) -> PathBuf {
    let text = hash.to_string();
    folder(owner).join(text.trim_start_matches("sha256-"))
}

fn sibling(p: &Path, ext: &str) -> PathBuf {
    p.with_extension(ext)
}

fn used_by(owner: &str) -> u64 {
    let Ok(rd) = fs::read_dir(folder(owner)) else { return 0 };
    rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// Removes what's past its age, for every caller.
pub fn sweep() {
    let Ok(owners) = fs::read_dir(root()) else { return };
    for o in owners.flatten() {
        let Ok(files) = fs::read_dir(o.path()) else { continue };
        for f in files.flatten() {
            let old = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|a| a > MAX_AGE);
            if old {
                let _ = fs::remove_file(f.path());
            }
        }
    }
}

/// One chunk, in order. Answers `(bytes received, done)`. The last chunk checks
/// the whole blob against its name; a wrong one is removed and refused.
pub fn put(owner: &str, hash: &Sha256, offset: u64, total: u64, mime: &str, bytes: &[u8]) -> Result<(u64, bool), StageError> {
    if total == 0 || total > MAX_BLOB_BYTES {
        return Err(StageError::Refused("413 Payload Too Large", "blob_too_large", format!("an image here is at most {} MiB", MAX_BLOB_BYTES / (1024 * 1024))));
    }
    if offset + bytes.len() as u64 > total {
        return Err(StageError::Refused("400 Bad Request", "bad_request", "more bytes than total".to_string()));
    }
    if mime.len() > 127 || !mime.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(StageError::Refused("400 Bad Request", "bad_request", "mime isn't a media type".to_string()));
    }
    let done_path = base(owner, hash);
    if done_path.exists() {
        return Ok((total, true));
    }
    sweep();
    if used_by(owner) + bytes.len() as u64 > MAX_CALLER_BYTES {
        return Err(StageError::Refused("413 Payload Too Large", "stage_full", "too many images are waiting to be saved".to_string()));
    }
    k2_core::fs_atomic::create_private_dir_all(&folder(owner))?;
    let part = sibling(&done_path, "part");
    let have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if offset != have {
        return Err(StageError::Refused("409 Conflict", "bad_offset", format!("the next byte is {have}")));
    }
    let mut f = fs::OpenOptions::new().create(true).read(true).append(true).open(&part)?;
    f.write_all(bytes)?;
    let received = offset + bytes.len() as u64;
    if received < total {
        return Ok((received, false));
    }
    let mut all = Vec::with_capacity(total as usize);
    f.seek(SeekFrom::Start(0))?;
    f.read_to_end(&mut all)?;
    if Sha256::of(&all) != *hash {
        let _ = fs::remove_file(&part);
        return Err(StageError::Refused("400 Bad Request", "hash_mismatch", "these bytes don't hash to that name".to_string()));
    }
    k2_core::fs_atomic::atomic_write_private(&sibling(&done_path, "mime"), mime.to_ascii_lowercase().as_bytes())?;
    fs::rename(&part, &done_path)?;
    Ok((received, true))
}

/// A blob this caller staged: `(media type, bytes)`.
pub fn get(owner: &str, hash: &Sha256) -> Option<(String, Vec<u8>)> {
    let p = base(owner, hash);
    let bytes = fs::read(&p).ok()?;
    let mime = fs::read_to_string(sibling(&p, "mime")).unwrap_or_else(|_| "application/octet-stream".to_string());
    Some((mime, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(b: &[u8]) -> Sha256 {
        Sha256::of(b)
    }

    #[test]
    fn a_blob_comes_in_chunks_is_checked_at_the_end_and_is_only_its_callers() {
        crate::test_support::with_temp_home(|| {
            let bytes = b"\x89PNG some image bytes".to_vec();
            let h = hash(&bytes);
            let a = owner_of("person:owner");
            let b = owner_of("agent:someone");
            assert_ne!(a, b);
            assert_eq!(put(&a, &h, 0, bytes.len() as u64, "image/png", &bytes[..5]).expect("first"), (5, false));
            assert!(get(&a, &h).is_none(), "not until it's whole");
            let (r, done) = put(&a, &h, 5, bytes.len() as u64, "IMAGE/PNG", &bytes[5..]).expect("last");
            assert!(done);
            assert_eq!(r, bytes.len() as u64);
            assert_eq!(get(&a, &h), Some(("image/png".to_string(), bytes.clone())));
            assert_eq!(get(&b, &h), None, "another caller can't name it");
        });
    }

    #[test]
    fn wrong_bytes_are_refused_and_gone() {
        crate::test_support::with_temp_home(|| {
            let h = hash(b"the real bytes");
            let a = owner_of("x");
            match put(&a, &h, 0, 5, "image/png", b"wrong") {
                Err(StageError::Refused(_, code, _)) => assert_eq!(code, "hash_mismatch"),
                other => panic!("{other:?}"),
            }
            assert!(get(&a, &h).is_none());
            assert!(!sibling(&base(&a, &h), "part").exists());
        });
    }

    #[test]
    fn the_limits_refuse_typed() {
        crate::test_support::with_temp_home(|| {
            let h = hash(b"x");
            let a = owner_of("x");
            match put(&a, &h, 0, MAX_BLOB_BYTES + 1, "image/png", b"x") {
                Err(StageError::Refused(_, code, _)) => assert_eq!(code, "blob_too_large"),
                other => panic!("{other:?}"),
            }
            match put(&a, &h, 5, 10, "image/png", b"x") {
                Err(StageError::Refused(_, code, _)) => assert_eq!(code, "bad_offset"),
                other => panic!("{other:?}"),
            }
            match put(&a, &h, 0, 1, "bad mime", b"x") {
                Err(StageError::Refused(_, code, _)) => assert_eq!(code, "bad_request"),
                other => panic!("{other:?}"),
            }
        });
    }

    #[test]
    fn what_nobody_saved_is_swept_after_a_day() {
        crate::test_support::with_temp_home(|| {
            let bytes = b"old".to_vec();
            let h = hash(&bytes);
            let a = owner_of("x");
            put(&a, &h, 0, 3, "image/png", &bytes).expect("put");
            let f = fs::File::options().write(true).open(base(&a, &h)).expect("open");
            f.set_modified(SystemTime::now() - Duration::from_secs(25 * 3600)).expect("age");
            sweep();
            assert!(get(&a, &h).is_none());
        });
    }
}
