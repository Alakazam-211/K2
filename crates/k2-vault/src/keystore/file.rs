//! File KeyStore: `~/.k2/vault/key/kek`, 0600 in a 0700 dir with a
//! `CACHEDIR.TAG` (restic/borg `--exclude-caches` skip it), excluded from
//! every K2-made backup and clone. The Linux default in 0.45.2 (KV23/KV26).
//!
//! Honest limit: any process running as this user can read it, and
//! full-disk copies include it. No anchor (rollback guard off).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use zeroize::Zeroize;

use super::{Anchor, KeyHome, KeyStore};
use crate::error::{Result, VaultError};
use crate::secret::Key32;

pub const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This directory holds the K2 vault's box key. Backup tools: skip it.\n";

pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    /// `dir` is the key directory (normally `~/.k2/vault/key`).
    pub fn new(dir: impl Into<PathBuf>) -> FileKeyStore {
        FileKeyStore { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn kek_path(&self) -> PathBuf {
        self.dir.join("kek")
    }

    fn next_path(&self) -> PathBuf {
        self.dir.join("kek.next")
    }

    fn ensure_dir(&self) -> Result<()> {
        crate::fsutil::ensure_private_dir(&self.dir)?;
        let tag = self.dir.join("CACHEDIR.TAG");
        if !tag.exists() {
            crate::fsutil::write_private_atomic(&tag, CACHEDIR_TAG.as_bytes())?;
        }
        Ok(())
    }

    fn read_key(path: &Path) -> Result<Option<Key32>> {
        let mut bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if bytes.len() != 32 {
            bytes.zeroize();
            return Err(VaultError::tamper("box key file has the wrong length"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        bytes.zeroize();
        Ok(Some(Key32::from_array(arr)))
    }

    fn write_key(path: &Path, k: &Key32) -> Result<()> {
        crate::fsutil::write_private_atomic(path, k.as_bytes())
    }

    /// Overwrite then unlink (best effort; on copy-on-write filesystems and
    /// SSDs the old blocks may survive, so status says "removed", VS9).
    fn wipe(path: &Path) -> Result<()> {
        if let Ok(mut f) = fs::OpenOptions::new().write(true).open(path) {
            let _ = f.write_all(&[0u8; 32]);
            let _ = f.sync_all();
        }
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

impl KeyStore for FileKeyStore {
    fn kind(&self) -> KeyHome {
        KeyHome::File
    }

    fn load(&self) -> Result<Option<Key32>> {
        Self::read_key(&self.kek_path())
    }

    fn init(&self) -> Result<Key32> {
        self.ensure_dir()?;
        if self.kek_path().exists() {
            return Err(VaultError::conflict("kek_exists", "a box key already exists"));
        }
        let k = Key32::random();
        Self::write_key(&self.kek_path(), &k)?;
        Ok(k)
    }

    fn supports_anchor(&self) -> bool {
        false
    }

    fn anchor(&self) -> Result<Option<Anchor>> {
        Ok(None)
    }

    fn set_anchor(&self, _a: &Anchor) -> Result<()> {
        Ok(())
    }

    fn stage_next(&self) -> Result<Key32> {
        self.ensure_dir()?;
        let k = Key32::random();
        Self::write_key(&self.next_path(), &k)?;
        Ok(k)
    }

    fn commit_next(&self) -> Result<()> {
        if !self.next_path().exists() {
            return Err(VaultError::conflict("no_staged_kek", "nothing staged"));
        }
        fs::rename(self.next_path(), self.kek_path())?;
        crate::fsutil::fsync_dir(&self.dir);
        Ok(())
    }

    fn abort_next(&self) -> Result<()> {
        Self::wipe(&self.next_path())
    }

    fn remove(&self) -> Result<()> {
        Self::wipe(&self.kek_path())?;
        Self::wipe(&self.next_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn creates_private_key_with_cachedir_tag() {
        let t = crate::fsutil::TestDir::new("filekey");
        let ks = FileKeyStore::new(t.path().join("vault/key"));
        assert!(ks.load().expect("load").is_none());
        let k = ks.init().expect("init");
        let again = ks.load().expect("load").expect("present");
        assert!(k == again);
        let mode = fs::metadata(t.path().join("vault/key/kek")).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dmode = fs::metadata(t.path().join("vault/key")).expect("meta").permissions().mode() & 0o777;
        assert_eq!(dmode, 0o700);
        let tag = fs::read_to_string(t.path().join("vault/key/CACHEDIR.TAG")).expect("tag");
        assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55"));
        assert_eq!(ks.init().err().expect("exists").code, "kek_exists");
    }

    #[test]
    fn rotation_and_remove() {
        let t = crate::fsutil::TestDir::new("filekey-rot");
        let ks = FileKeyStore::new(t.path().join("key"));
        let a = ks.init().expect("init");
        let b = ks.stage_next().expect("stage");
        assert!(ks.load().expect("load").expect("cur") == a);
        ks.commit_next().expect("commit");
        assert!(ks.load().expect("load").expect("cur") == b);
        ks.remove().expect("remove");
        assert!(ks.load().expect("load").is_none());
        assert!(!t.path().join("key/kek").exists());
    }

    #[test]
    fn wrong_length_fails_closed() {
        let t = crate::fsutil::TestDir::new("filekey-bad");
        let ks = FileKeyStore::new(t.path().join("key"));
        crate::fsutil::ensure_private_dir(ks.dir()).expect("dir");
        fs::write(t.path().join("key/kek"), b"short").expect("write");
        assert_eq!(ks.load().err().expect("bad").code, "vault_tamper");
    }
}
