//! DPAPI KeyStore (Windows): the box key sealed with `CryptProtectData`
//! under the signed-in user's DPAPI master key, at
//! `~/.k2/vault/key/kek.dpapi` (owner-only, beside the file store's
//! `CACHEDIR.TAG`). The Windows counterpart of the macOS Keychain store: a
//! copy of the file is useless to another account or machine, and only
//! this user's own processes can unseal it. No anchor (rollback guard):
//! the sealed file sits beside the vault, as with the file store.

use std::fs;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use zeroize::Zeroize;

use super::{Anchor, KeyHome, KeyStore};
use crate::error::{Result, VaultError};
use crate::secret::Key32;

/// Extra entropy bound into every seal: a blob made for another purpose
/// with the same user key doesn't unseal as a K2 box key.
const ENTROPY: &[u8] = b"K2 vault box key v1";

pub struct DpapiKeyStore {
    dir: PathBuf,
}

impl DpapiKeyStore {
    /// `dir` is the file store's dir (`~/.k2/vault/key`).
    pub fn new(dir: impl Into<PathBuf>) -> DpapiKeyStore {
        DpapiKeyStore { dir: dir.into() }
    }

    fn kek_path(&self) -> PathBuf {
        self.dir.join("kek.dpapi")
    }

    fn next_path(&self) -> PathBuf {
        self.dir.join("kek.next.dpapi")
    }

    fn ensure_dir(&self) -> Result<()> {
        crate::fsutil::ensure_private_dir(&self.dir)?;
        let tag = self.dir.join("CACHEDIR.TAG");
        if !tag.exists() {
            crate::fsutil::write_private_atomic(&tag, super::file::CACHEDIR_TAG.as_bytes())?;
        }
        Ok(())
    }

    fn read_key(path: &Path) -> Result<Option<Key32>> {
        let sealed = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut plain = unseal(&sealed)?;
        if plain.len() != 32 {
            plain.zeroize();
            return Err(VaultError::tamper("sealed box key has the wrong length"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&plain);
        plain.zeroize();
        Ok(Some(Key32::from_array(arr)))
    }

    fn write_key(path: &Path, k: &Key32) -> Result<()> {
        let sealed = seal(k.as_bytes())?;
        crate::fsutil::write_private_atomic(path, &sealed)
    }

    fn remove_file(path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: bytes.len() as u32, pbData: bytes.as_ptr() as *mut u8 }
}

/// Copy a DPAPI output blob out and free it (zeroing the plaintext copy
/// Windows allocated when `wipe`).
fn take(out: CRYPT_INTEGER_BLOB, wipe: bool) -> Vec<u8> {
    // SAFETY: `out` was filled by CryptProtectData/CryptUnprotectData; we
    // copy cbData bytes, optionally zero them, then LocalFree the buffer.
    unsafe {
        let s = std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize);
        let v = s.to_vec();
        if wipe {
            s.zeroize();
        }
        LocalFree(out.pbData as *mut core::ffi::c_void);
        v
    }
}

fn seal(plain: &[u8]) -> Result<Vec<u8>> {
    let input = blob(plain);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: valid input/entropy blobs borrowed for the call; no prompt.
    let ok = unsafe {
        CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    if ok == 0 {
        return Err(VaultError::internal(format!("DPAPI seal failed: {}", std::io::Error::last_os_error())));
    }
    Ok(take(out, false))
}

fn unseal(sealed: &[u8]) -> Result<Vec<u8>> {
    let input = blob(sealed);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    // SAFETY: as in seal.
    let ok = unsafe {
        CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    if ok == 0 {
        return Err(VaultError::tamper(format!(
            "the sealed box key can't be opened by this Windows account ({})",
            std::io::Error::last_os_error()
        )));
    }
    Ok(take(out, true))
}

impl KeyStore for DpapiKeyStore {
    fn kind(&self) -> KeyHome {
        KeyHome::Dpapi
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
        Self::remove_file(&self.next_path())
    }

    fn remove(&self) -> Result<()> {
        Self::remove_file(&self.kek_path())?;
        Self::remove_file(&self.next_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_reloads_rotates_and_refuses_a_tampered_blob() {
        let t = crate::fsutil::TestDir::new("dpapi");
        let ks = DpapiKeyStore::new(t.path().join("vault/key"));
        assert!(ks.load().expect("load").is_none());
        let k = ks.init().expect("init");
        assert!(ks.load().expect("load").expect("present") == k);
        // On disk it is sealed, never the raw key.
        let on_disk = fs::read(t.path().join("vault/key/kek.dpapi")).expect("sealed file");
        assert!(on_disk.len() > 32 && !on_disk.windows(32).any(|w| w == k.as_bytes()));
        assert!(k2_winsec::is_owner_only(&t.path().join("vault/key/kek.dpapi")).expect("dacl"));
        assert_eq!(ks.init().err().expect("exists").code, "kek_exists");
        let n = ks.stage_next().expect("stage");
        ks.commit_next().expect("commit");
        assert!(ks.load().expect("load").expect("cur") == n);
        // A flipped byte is refused loudly, never read as a key.
        let mut bad = fs::read(t.path().join("vault/key/kek.dpapi")).unwrap();
        let last = bad.len() - 1;
        bad[last] ^= 0x55;
        fs::write(t.path().join("vault/key/kek.dpapi"), &bad).unwrap();
        assert_eq!(ks.load().err().expect("tampered").code, "vault_tamper");
        ks.remove().expect("remove");
        assert!(ks.load().expect("load").is_none());
    }
}
