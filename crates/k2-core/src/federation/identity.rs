//! This box's federation identity key: `~/.k2/federation/identity.pem`
//! (ECDSA P-256, PKCS#8 PEM, file 0600, directory 0700).
//!
//! Until 0.45.2 the federation identity WAS the tunnel TLS key
//! (`~/.k2/tunnel-key.pem`). Peers pin this key, so anything that replaced
//! or deleted the tunnel key (a subdomain release, a re-provision) silently
//! broke every pinned peer. Federation now has its own key; the tunnel keeps
//! `tunnel-key.pem` for its certificate.
//!
//! ## Migration (one time, at first load)
//!
//! - `identity.pem` present → used as is, never overwritten.
//! - missing, `tunnel-key.pem` present → the tunnel key's bytes are COPIED
//!   (never moved), so the fingerprint is unchanged and no peer re-pairs.
//! - neither present → a fresh ECDSA P-256 key, generated the way the tunnel
//!   key is.
//!
//! The file is created with a write-to-temp then `hard_link` into place.
//! `hard_link` fails when the target already exists, so two loaders racing
//! (two threads, or the daemon and a test harness) can never overwrite each
//! other: the loser reads the winner's key.
//!
//! Every federation call site (fingerprint, `k2 fed pubkey`, pairing, SAS,
//! envelope and roster signing, address notices) reads ONLY [`load_or_create`].

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rcgen::KeyPair;

/// `~/.k2/federation/` (honors `$HOME`, like the rest of `federation`).
pub fn identity_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2")
        .join("federation")
}

/// `~/.k2/federation/identity.pem`.
pub fn identity_path() -> PathBuf {
    identity_dir().join("identity.pem")
}

/// Where the key came from on this call (for the boot log line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    /// `identity.pem` already existed.
    Existing,
    /// Copied from `tunnel-key.pem` (same fingerprint as before 0.45.2).
    MigratedFromTunnelKey,
    /// Neither file existed: a fresh key.
    Generated,
}

/// In-process serialization of the create path. Cross-process safety comes
/// from `hard_link` (create-if-absent), not from this lock.
static CREATE_LOCK: Mutex<()> = Mutex::new(());

/// Load this box's federation identity key, migrating or generating it on
/// first use. See the module doc.
pub fn load_or_create() -> Result<KeyPair, String> {
    load_or_create_with_source().map(|(k, _)| k)
}

/// [`load_or_create`] plus where the key came from.
pub fn load_or_create_with_source() -> Result<(KeyPair, IdentitySource), String> {
    let path = identity_path();
    if path.exists() {
        return read_key(&path).map(|k| (k, IdentitySource::Existing));
    }
    let _guard = CREATE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if path.exists() {
        return read_key(&path).map(|k| (k, IdentitySource::Existing));
    }

    let tunnel_key = crate::tunnel::tls::key_path();
    let (pem, source) = if tunnel_key.exists() {
        let bytes = fs::read(&tunnel_key)
            .map_err(|e| format!("read tunnel key {}: {e}", tunnel_key.display()))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| format!("tunnel key {} is not PEM text: {e}", tunnel_key.display()))?;
        // Refuse to copy a key that does not parse: the identity would be
        // broken from its first use.
        KeyPair::from_pem(text)
            .map_err(|e| format!("parse tunnel key {}: {e}", tunnel_key.display()))?;
        (bytes, IdentitySource::MigratedFromTunnelKey)
    } else {
        let kp = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| format!("generate ECDSA P-256 keypair: {e}"))?;
        (kp.serialize_pem().into_bytes(), IdentitySource::Generated)
    };

    let dir = identity_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    restrict_dir(&dir)?;
    let tmp = dir.join(format!("identity.pem.tmp.{}", uuid::Uuid::new_v4()));
    let created = (|| {
        fs::write(&tmp, &pem).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        restrict_file(&tmp)?;
        match fs::hard_link(&tmp, &path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(format!("link into place {}: {e}", path.display())),
        }
    })();
    let _ = fs::remove_file(&tmp);
    if !created? {
        // Another loader won the race: use its key, never ours.
        return read_key(&path).map(|k| (k, IdentitySource::Existing));
    }
    restrict_file(&path)?;
    let text = String::from_utf8(pem).map_err(|e| format!("identity PEM: {e}"))?;
    let kp = KeyPair::from_pem(&text).map_err(|e| format!("parse new identity key: {e}"))?;
    Ok((kp, source))
}

/// SHA-256(SPKI DER) hex of this box's federation identity.
pub fn fingerprint() -> Result<String, String> {
    let kp = load_or_create()?;
    Ok(super::peers::fingerprint_of_spki_der(&kp.public_key_der()))
}

fn read_key(path: &Path) -> Result<KeyPair, String> {
    let pem = fs::read_to_string(path)
        .map_err(|e| format!("read federation identity {}: {e}", path.display()))?;
    KeyPair::from_pem(&pem).map_err(|e| format!("parse federation identity {}: {e}", path.display()))
}

#[cfg(unix)]
fn restrict_file(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", p.display()))
}

#[cfg(unix)]
fn restrict_dir(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("chmod 0700 {}: {e}", p.display()))
}

#[cfg(not(unix))]
fn restrict_file(_p: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_dir(_p: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    fn fp_of(kp: &KeyPair) -> String {
        super::super::peers::fingerprint_of_spki_der(&kp.public_key_der())
    }

    #[test]
    fn migration_copies_the_tunnel_key_byte_for_byte() {
        with_temp_home(|| {
            let tunnel = crate::tunnel::tls::load_or_generate_keypair().expect("tunnel key");
            let tunnel_bytes = fs::read(crate::tunnel::tls::key_path()).expect("read tunnel key");
            assert!(!identity_path().exists());

            let (kp, source) = load_or_create_with_source().expect("identity");
            assert_eq!(source, IdentitySource::MigratedFromTunnelKey);
            assert_eq!(fp_of(&kp), fp_of(&tunnel), "fingerprint must not change on migration");
            assert_eq!(fs::read(identity_path()).expect("read identity"), tunnel_bytes);
            assert!(crate::tunnel::tls::key_path().exists(), "the tunnel key is copied, never moved");
            assert_eq!(
                fs::read(crate::tunnel::tls::key_path()).expect("tunnel key still there"),
                tunnel_bytes
            );
            let (_, again) = load_or_create_with_source().expect("second load");
            assert_eq!(again, IdentitySource::Existing);
        });
    }

    #[test]
    fn tunnel_key_change_or_deletion_does_not_move_the_federation_fingerprint() {
        with_temp_home(|| {
            crate::tunnel::tls::load_or_generate_keypair().expect("tunnel key");
            let before = fingerprint().expect("fp");

            // Replace the tunnel key with a different one.
            fs::remove_file(crate::tunnel::tls::key_path()).expect("rm tunnel key");
            let new_tunnel = crate::tunnel::tls::load_or_generate_keypair().expect("new tunnel key");
            assert_ne!(fp_of(&new_tunnel), before, "the test needs a different tunnel key");
            assert_eq!(fingerprint().expect("fp after change"), before);

            // Delete it outright.
            fs::remove_file(crate::tunnel::tls::key_path()).expect("rm tunnel key again");
            assert_eq!(fingerprint().expect("fp after delete"), before);
            assert!(!crate::tunnel::tls::key_path().exists(), "federation must not recreate the tunnel key");
        });
    }

    #[test]
    fn fresh_key_when_neither_file_exists() {
        with_temp_home(|| {
            assert!(!crate::tunnel::tls::key_path().exists());
            let (kp, source) = load_or_create_with_source().expect("identity");
            assert_eq!(source, IdentitySource::Generated);
            assert!(identity_path().exists());
            assert!(
                !crate::tunnel::tls::key_path().exists(),
                "a fresh federation key does not create a tunnel key"
            );
            let pem = fs::read_to_string(identity_path()).expect("read");
            assert!(pem.contains("BEGIN PRIVATE KEY"), "{pem}");
            assert_eq!(fp_of(&KeyPair::from_pem(&pem).expect("parse")), fp_of(&kp));
            // A later tunnel key is a different key.
            let tunnel = crate::tunnel::tls::load_or_generate_keypair().expect("tunnel key");
            assert_ne!(fp_of(&tunnel), fp_of(&kp));
            assert_eq!(fingerprint().expect("fp"), fp_of(&kp));
        });
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_is_0600_and_dir_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home(|| {
            load_or_create().expect("identity");
            let fmode = fs::metadata(identity_path()).expect("stat").permissions().mode() & 0o777;
            let dmode = fs::metadata(identity_dir()).expect("stat dir").permissions().mode() & 0o777;
            assert_eq!(fmode, 0o600);
            assert_eq!(dmode, 0o700);
            let leftovers: Vec<_> = fs::read_dir(identity_dir())
                .expect("readdir")
                .map(|e| e.expect("entry").file_name().to_string_lossy().to_string())
                .collect();
            assert_eq!(leftovers, vec!["identity.pem".to_string()], "no temp files left behind");
        });
    }

    #[test]
    fn an_existing_identity_is_never_overwritten() {
        with_temp_home(|| {
            let mine = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("gen");
            fs::create_dir_all(identity_dir()).expect("mkdir");
            fs::write(identity_path(), mine.serialize_pem()).expect("plant identity");
            let planted = fs::read(identity_path()).expect("read");
            // A tunnel key exists too and differs: it must not win.
            crate::tunnel::tls::load_or_generate_keypair().expect("tunnel key");

            let (kp, source) = load_or_create_with_source().expect("load");
            assert_eq!(source, IdentitySource::Existing);
            assert_eq!(fp_of(&kp), fp_of(&mine));
            assert_eq!(fs::read(identity_path()).expect("read again"), planted);
        });
    }

    #[test]
    fn a_malformed_identity_fails_loud_and_is_kept() {
        with_temp_home(|| {
            fs::create_dir_all(identity_dir()).expect("mkdir");
            fs::write(identity_path(), "not a key").expect("plant junk");
            let err = load_or_create().err().expect("junk must fail loud");
            assert!(err.contains("parse federation identity"), "{err}");
            assert_eq!(fs::read_to_string(identity_path()).expect("kept"), "not a key");
        });
    }

    #[test]
    fn concurrent_first_loads_agree_on_one_key() {
        with_temp_home(|| {
            let home = dirs::home_dir().expect("home");
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let home = home.clone();
                    std::thread::spawn(move || {
                        // The env lock is held by this test; worker threads
                        // read $HOME through the same process env.
                        assert_eq!(dirs::home_dir().expect("home"), home);
                        fp_of(&load_or_create().expect("load"))
                    })
                })
                .collect();
            let fps: Vec<String> = handles.into_iter().map(|h| h.join().expect("join")).collect();
            assert!(fps.windows(2).all(|w| w[0] == w[1]), "all loaders must see one key: {fps:?}");
            assert_eq!(fingerprint().expect("fp"), fps[0]);
        });
    }
}
