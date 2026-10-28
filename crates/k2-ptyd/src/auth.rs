//! Socket auth (PRD §8 S-1, P70): ptyd's uid (peer check) AND the key
//! (HMAC challenge). The key is 32 CSPRNG bytes in `<run>/ptyd/key`, 0600,
//! created `O_EXCL` by each fresh ptyd instance; never logged.

use std::io;
#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const KEY_LEN: usize = 32;

pub fn random_bytes<const N: usize>() -> io::Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(b)
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// sha256 hex of arbitrary bytes (endpoint hashing, embedded-binary hash).
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Replace the key file with a fresh one: unlink, then `O_CREAT|O_EXCL`
/// 0600. A new ptyd instance never reuses an older instance's key.
#[cfg(unix)]
pub fn create_key(path: &Path) -> io::Result<[u8; KEY_LEN]> {
    use std::os::unix::fs::OpenOptionsExt;
    let key = random_bytes::<KEY_LEN>()?;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(&key)?;
    f.sync_all()?;
    Ok(key)
}

/// Read the key; refuses a file that is not exactly 32 bytes, not 0600, or
/// not owned by us.
#[cfg(unix)]
pub fn read_key(path: &Path) -> io::Result<[u8; KEY_LEN]> {
    use std::os::unix::fs::MetadataExt;
    let mut f = std::fs::File::open(path)?;
    let md = f.metadata()?;
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if md.uid() != euid || md.mode() & 0o077 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "ptyd key has wrong owner or mode"));
    }
    let mut key = [0u8; KEY_LEN];
    f.read_exact(&mut key)?;
    let mut extra = [0u8; 1];
    if f.read(&mut extra)? != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ptyd key is not 32 bytes"));
    }
    Ok(key)
}

/// hex(HMAC-SHA256(key, nonce ‖ client_version)).
pub fn mac(key: &[u8; KEY_LEN], nonce: &[u8], client_version: &str) -> String {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    m.update(nonce);
    m.update(client_version.as_bytes());
    hex(&m.finalize().into_bytes())
}

/// Constant-time check of a client's mac.
pub fn verify(key: &[u8; KEY_LEN], nonce: &[u8], client_version: &str, mac_hex: &str) -> bool {
    let Some(given) = unhex(mac_hex) else { return false };
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    m.update(nonce);
    m.update(client_version.as_bytes());
    m.verify_slice(&given).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_round_trips_and_rejects_tampering() {
        let key = [7u8; KEY_LEN];
        let nonce = [1u8; 32];
        let m = mac(&key, &nonce, "0.46.2");
        assert!(verify(&key, &nonce, "0.46.2", &m));
        assert!(!verify(&key, &nonce, "0.46.3", &m), "version is bound into the mac");
        assert!(!verify(&[8u8; KEY_LEN], &nonce, "0.46.2", &m), "wrong key");
        assert!(!verify(&key, &nonce, "0.46.2", "zz"), "non-hex");
        assert!(!verify(&key, &nonce, "0.46.2", ""), "empty");
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_0600_32_bytes_and_replaced_per_instance() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("k2ptyd-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("key");
        let a = create_key(&p).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_key(&p).unwrap(), a);
        let b = create_key(&p).unwrap();
        assert_ne!(a, b, "a fresh instance mints a fresh key");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_key(&p).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
