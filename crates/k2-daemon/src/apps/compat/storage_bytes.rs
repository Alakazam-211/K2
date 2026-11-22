//! Where an object's bytes live (PRD §10 "Bytes").
//!
//! `~/.k2/apps/<app>/storage/objects/<id[..2]>/<id>/<version>`: made from
//! the object's id and the version of its current write, **never from its
//! name** (a name is caller text). The folder is the daemon's, beside the
//! App's `data/` (which a walled App's own user owns and the daemon can't
//! write), inside the App folder the disk scan measures. The App's own
//! process can't reach it: the only way to the bytes is the door, so a
//! storage policy can't be stepped around from inside the App.
//!
//! Why the id and the version together: `id` is the primary key, so no
//! caller can point their row at another object's bytes (a second row
//! with that id can't exist); `version` changes on every write, so an
//! upsert writes new bytes beside the old and the old are removed only
//! after the row moved on. A caller who rewrites their own row's `version`
//! reaches only that object's own versions.
//!
//! Uploads stream into `tmp/` while their MD5 (Supabase's ETag) and
//! SHA-256 are taken, then one rename places them.

use std::path::{Path, PathBuf};

// md-5 0.11 is on digest 0.11, sha2 0.10 on digest 0.10: two Digest traits.
use md5::{Digest as _, Md5};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::storage_err::{SResult, StorageError};

/// How often a streaming write asks the disk guard again.
const SPACE_EVERY: u64 = 16 << 20;

/// One App's byte store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteStore {
    pub root: PathBuf,
}

/// Bytes written to `tmp/`, not placed yet. Removed on drop unless
/// [`ByteStore::place`] took them.
#[derive(Debug)]
pub struct Staged {
    pub path: PathBuf,
    pub size: u64,
    pub md5_hex: String,
    pub sha256_hex: String,
    placed: bool,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.placed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Staged {
    /// A file already written (the import's finished download), hashed
    /// now; it is placed or removed like any staged upload.
    pub fn adopt(path: PathBuf) -> std::io::Result<Staged> {
        let (size, md5_hex, sha256_hex) = hash_file(&path)?;
        Ok(Staged { path, size, md5_hex, sha256_hex, placed: false })
    }

    /// Supabase's `eTag` for the object: the MD5 in quotes.
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.md5_hex)
    }
}

/// An id or version as a path step: a uuid's characters only.
fn step_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

impl ByteStore {
    pub fn for_app(app_id: &str) -> Self {
        ByteStore { root: k2_core::deploy::app_dir(app_id).join("storage") }
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The file of `(id, version)`; `None` for an id or version that isn't
    /// a uuid's characters (a hand-edited row never names a path outside).
    pub fn object_path(&self, id: &str, version: &str) -> Option<PathBuf> {
        if !step_ok(id) || !step_ok(version) || id.len() < 2 {
            return None;
        }
        Some(self.root.join("objects").join(&id[..2]).join(id).join(version))
    }

    /// Stream `body` into a new file in `tmp/`, at most `limit` bytes
    /// (more is [`StorageError::too_large`], and the file is removed).
    pub async fn stage<R: AsyncRead + Unpin>(&self, body: &mut R, limit: u64) -> SResult<Staged> {
        let dir = self.tmp_dir();
        tokio::fs::create_dir_all(&dir).await.map_err(|e| StorageError::internal(format!("storage folder: {e}")))?;
        self.space_ok(0)?;
        let path = dir.join(format!("{}.part", uuid::Uuid::new_v4()));
        let mut staged = Staged { path: path.clone(), size: 0, md5_hex: String::new(), sha256_hex: String::new(), placed: false };
        let mut f = tokio::fs::File::create(&path).await.map_err(|e| StorageError::internal(format!("storage temp file: {e}")))?;
        let (mut md5, mut sha) = (Md5::new(), Sha256::new());
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = body.read(&mut buf).await.map_err(|e| StorageError::invalid_request(format!("the upload body ended early: {e}")))?;
            if n == 0 {
                break;
            }
            let before = staged.size;
            staged.size += n as u64;
            if staged.size > limit {
                return Err(StorageError::too_large(limit));
            }
            // The disk guard again every SPACE_EVERY bytes: a long upload
            // whose size wasn't declared stops at the reserve too.
            if before / SPACE_EVERY != staged.size / SPACE_EVERY {
                self.space_ok(0)?;
            }
            md5.update(&buf[..n]);
            sha.update(&buf[..n]);
            f.write_all(&buf[..n]).await.map_err(|e| StorageError::internal(format!("storage write: {e}")))?;
        }
        f.flush().await.map_err(|e| StorageError::internal(format!("storage write: {e}")))?;
        f.sync_all().await.map_err(|e| StorageError::internal(format!("storage sync: {e}")))?;
        staged.md5_hex = hex(&md5.finalize());
        staged.sha256_hex = hex(&sha.finalize());
        Ok(staged)
    }

    /// The disk guard's upload rule (`disk_guard::reserve_ok`, PRD C6, as
    /// every K2 upload): refused when writing `need` more bytes would leave
    /// less than the box's reserve. Mode `warn` or `off` never refuses.
    pub fn space_ok(&self, need: u64) -> SResult<()> {
        k2_core::disk_guard::reserve_ok(&self.root, need).map_err(|_| StorageError::no_space())
    }

    /// Every `(id, version)` that has bytes on disk (verify's orphan
    /// check: bytes no row names, as after a database restore to an
    /// earlier moment).
    pub fn all_bytes(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(shards) = std::fs::read_dir(self.root.join("objects")) else { return out };
        for shard in shards.flatten() {
            let Ok(ids) = std::fs::read_dir(shard.path()) else { continue };
            for id in ids.flatten() {
                let Ok(versions) = std::fs::read_dir(id.path()) else { continue };
                for v in versions.flatten() {
                    if let (Some(i), Some(ver)) = (id.file_name().to_str(), v.file_name().to_str()) {
                        if self.object_path(i, ver).is_some() {
                            out.push((i.to_string(), ver.to_string()));
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Move staged bytes to `(id, version)`.
    pub fn place(&self, staged: &mut Staged, id: &str, version: &str) -> SResult<PathBuf> {
        let dest = self.object_path(id, version).ok_or_else(|| StorageError::internal("object id or version is not a uuid"))?;
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| StorageError::internal(format!("storage folder: {e}")))?;
        }
        std::fs::rename(&staged.path, &dest).map_err(|e| StorageError::internal(format!("storage place: {e}")))?;
        staged.placed = true;
        Ok(dest)
    }

    /// Copy `(id, version)`'s bytes to a new staged file (the `copy` verb).
    pub fn stage_copy(&self, id: &str, version: &str) -> SResult<Staged> {
        let src = self.object_path(id, version).ok_or_else(StorageError::not_found)?;
        std::fs::create_dir_all(self.tmp_dir()).map_err(|e| StorageError::internal(format!("storage folder: {e}")))?;
        let path = self.tmp_dir().join(format!("{}.part", uuid::Uuid::new_v4()));
        let staged_path = path.clone();
        let mut staged = Staged { path, size: 0, md5_hex: String::new(), sha256_hex: String::new(), placed: false };
        let (size, md5_hex, sha256_hex) = hash_file(&src).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => StorageError::not_found(),
            _ => StorageError::internal(format!("storage read: {e}")),
        })?;
        std::fs::copy(&src, &staged_path).map_err(|e| StorageError::internal(format!("storage copy: {e}")))?;
        staged.size = size;
        staged.md5_hex = md5_hex;
        staged.sha256_hex = sha256_hex;
        Ok(staged)
    }

    /// Remove one version's bytes (gone already is fine), then the
    /// object's folder when it is empty.
    pub fn remove(&self, id: &str, version: &str) {
        if let Some(p) = self.object_path(id, version) {
            let _ = std::fs::remove_file(&p);
            if let Some(dir) = p.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }

    /// Does `(id, version)` have its bytes?
    pub fn has(&self, id: &str, version: &str) -> bool {
        self.object_path(id, version).is_some_and(|p| p.is_file())
    }
}

/// `(size, md5 hex, sha256 hex)` of a file.
pub fn hash_file(p: &Path) -> std::io::Result<(u64, String, String)> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let (mut md5, mut sha) = (Md5::new(), Sha256::new());
    let mut buf = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        md5.update(&buf[..n]);
        sha.update(&buf[..n]);
    }
    Ok((size, hex(&md5.finalize()), hex(&sha.finalize())))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (crate::test_support::Scratch, ByteStore) {
        let d = crate::test_support::Scratch::new();
        let s = ByteStore { root: d.path().join("storage") };
        (d, s)
    }

    #[test]
    fn paths_come_from_ids_never_names() {
        let (_d, s) = store();
        let id = "6f1c4a52-0b7d-4d0e-9a2c-3b8f1e7d9a10";
        let v = "0b7d4d0e-9a2c-4b8f-8e7d-9a106f1c4a52";
        let p = s.object_path(id, v).expect("path");
        assert!(p.ends_with(format!("objects/6f/{id}/{v}")), "{p:?}");
        assert_eq!(s.object_path("../../etc", v), None);
        assert_eq!(s.object_path(id, "a/b"), None);
        assert_eq!(s.object_path(id, ""), None);
        assert_eq!(s.object_path("org/photo.jpg", v), None);
    }

    #[tokio::test]
    async fn stage_hashes_and_limits() {
        let (_d, s) = store();
        let mut body: &[u8] = b"hello world";
        let st = s.stage(&mut body, 100).await.expect("stage");
        assert_eq!(st.size, 11);
        assert_eq!(st.md5_hex, "5eb63bbbe01eeed093cb22bb8f5acdc3");
        assert_eq!(st.sha256_hex, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        assert_eq!(st.etag(), "\"5eb63bbbe01eeed093cb22bb8f5acdc3\"");
        let tmp = st.path.clone();
        drop(st);
        assert!(!tmp.exists(), "an unplaced stage is removed");

        let mut big: &[u8] = &[7u8; 101];
        let e = s.stage(&mut big, 100).await.expect_err("over the limit");
        assert_eq!(e.code, "EntityTooLarge");
        let left: Vec<_> = std::fs::read_dir(s.tmp_dir()).expect("tmp").collect();
        assert!(left.is_empty(), "a refused upload leaves nothing in tmp: {left:?}");
    }

    #[tokio::test]
    async fn a_full_disk_stops_an_upload_at_the_reserve() {
        use k2_core::disk_guard::{fake, Mode, RawStat};
        let (_d, s) = store();
        std::fs::create_dir_all(&s.root).expect("root");
        // 100 GiB with nothing free: under any reserve.
        let _disk = fake::install(&s.root, "9:77", "/", RawStat::sized(100 << 30, 0), Some(Mode::On));
        let mut body: &[u8] = b"bytes";
        let e = s.stage(&mut body, 100).await.expect_err("a full disk");
        assert_eq!(e.code, "InsufficientStorage");
        assert_eq!(e.body()["statusCode"], "507");
        let left: Vec<_> = std::fs::read_dir(s.tmp_dir()).expect("tmp").collect();
        assert!(left.is_empty(), "nothing is left behind: {left:?}");
        assert_eq!(s.space_ok(1).expect_err("declared size").code, "InsufficientStorage");
        // Mode warn never refuses an upload.
        _disk.set_mode(Some(Mode::Warn));
        let mut body: &[u8] = b"bytes";
        s.stage(&mut body, 100).await.expect("mode warn");
    }

    #[tokio::test]
    async fn place_copy_remove() {
        const V1: &str = "0b7d4d0e-9a2c-4b8f-8e7d-9a106f1c4a52";
        let (_d, s) = store();
        let id = "6f1c4a52-0b7d-4d0e-9a2c-3b8f1e7d9a10";
        let mut body: &[u8] = b"bytes";
        let mut st = s.stage(&mut body, 100).await.expect("stage");
        let p = s.place(&mut st, id, V1).expect("place");
        drop(st);
        assert_eq!(std::fs::read(&p).expect("read"), b"bytes");
        assert!(s.has(id, V1));
        let c = s.stage_copy(id, V1).expect("copy");
        assert_eq!(c.size, 5);
        assert_eq!(std::fs::read(&c.path).expect("read copy"), b"bytes");
        assert_eq!(s.all_bytes(), vec![(id.to_string(), V1.to_string())]);
        s.remove(id, V1);
        assert!(!s.has(id, V1));
        assert!(s.all_bytes().is_empty());
        assert!(!p.parent().expect("dir").exists(), "the empty object folder goes too");
        assert_eq!(s.stage_copy(id, V1).expect_err("gone").code, "NoSuchKey");
    }
}
