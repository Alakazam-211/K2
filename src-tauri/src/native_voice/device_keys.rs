//! Where a call device's keys live: K2's side of Sew's `DeviceKeyStore`
//! (prd-k2-voice-rooms-v1 §14.5, vs-live V46).
//!
//! - **The private key** goes in the OS keychain through the `keyring` crate K2
//!   already uses, service `dev.k2.voice.device`: macOS login Keychain, Windows
//!   Credential Manager, Linux Secret Service. On Linux with no Secret Service
//!   running it falls back to a 0600 file; on the other two a keychain that can't
//!   answer fails the call's key setup instead.
//! - **The public record** (device id, public key, certificate) goes in a 0600
//!   file beside it, on every OS. It isn't secret, and a keychain entry has limits
//!   the record shouldn't have to fit (Windows caps a credential at 2560 bytes).
//!
//! Sew's rule: `load` answers `Ok(None)` for "no device", including a damaged
//! entry, and the engine enrols again. `Err` is only for a store that can't answer.
//! No error here carries a key.

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};
use tauri_plugin_sew_voice::{DeviceError, DeviceKeyId, DeviceKeyRecord, DeviceKeyStore, StoredDevice};
use zeroize::Zeroizing;

/// The keychain service every entry goes under.
pub const KEYCHAIN_SERVICE: &str = "dev.k2.voice.device";

/// Where the private key goes. The real one is the OS keychain.
pub trait KeyVault: Send + Sync + 'static {
    /// `Err` means the keychain can't answer at all; its text carries no key.
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, String>;
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    /// Succeeds when there's nothing to delete.
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// The OS keychain, through the same `keyring` crate as K2's saved logins.
pub struct OsKeychain;

impl KeyVault for OsKeychain {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(s) => Ok(Some(Zeroizing::new(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account).and_then(|e| e.set_password(secret)).map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub struct K2DeviceKeys {
    vault: Box<dyn KeyVault>,
    dir: PathBuf,
    /// Keep the key in a 0600 file when the keychain can't answer (Linux only).
    file_fallback: bool,
}

impl K2DeviceKeys {
    pub fn new(vault: impl KeyVault, dir: PathBuf, file_fallback: bool) -> K2DeviceKeys {
        K2DeviceKeys { vault: Box::new(vault), dir, file_fallback }
    }

    /// The app's store, under `dir`.
    pub fn for_app(dir: PathBuf) -> K2DeviceKeys {
        K2DeviceKeys::new(OsKeychain, dir, cfg!(target_os = "linux"))
    }

    fn name(id: &DeviceKeyId) -> String {
        // `account()` is `<install>:<member>:<scope>`; a colon can't be in a Windows
        // file name. The scope is already `[A-Za-z0-9_-]`.
        id.account().replace(':', "-")
    }

    fn record_path(&self, id: &DeviceKeyId) -> PathBuf {
        self.dir.join(format!("{}.json", Self::name(id)))
    }

    fn key_path(&self, id: &DeviceKeyId) -> PathBuf {
        self.dir.join(format!("{}.key", Self::name(id)))
    }

    fn key_text(&self, id: &DeviceKeyId) -> Result<Option<Zeroizing<String>>, DeviceError> {
        match self.vault.get(&id.account()) {
            Ok(Some(k)) => Ok(Some(k)),
            Ok(None) if self.file_fallback => Ok(read_key_file(&self.key_path(id))),
            Ok(None) => Ok(None),
            Err(e) if self.file_fallback => {
                log_debug!("[native-voice] keychain unavailable, using the key file: {e}");
                Ok(read_key_file(&self.key_path(id)))
            }
            Err(e) => {
                log_debug!("[native-voice] keychain unavailable: {e}");
                Err(DeviceError::Store("the system keychain isn't available".into()))
            }
        }
    }
}

fn read_key_file(path: &Path) -> Option<Zeroizing<String>> {
    std::fs::read_to_string(path).ok().map(Zeroizing::new)
}

fn store_err(what: &str, e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Store(format!("{what}: {e}"))
}

/// Write `bytes` as a private file, atomically.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), DeviceError> {
    let dir = path.parent().ok_or_else(|| DeviceError::Store("the device key folder has no parent".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| store_err("creating the device key folder", e))?;
    let tmp = path.with_extension("tmp");
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
        let _ = std::fs::DirBuilder::new().mode(0o700).create(dir);
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| store_err("writing a device file", e))?;
        f.write_all(bytes).and_then(|()| f.sync_all()).map_err(|e| store_err("writing a device file", e))?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes).map_err(|e| store_err("writing a device file", e))?;
    std::fs::rename(&tmp, path).map_err(|e| store_err("writing a device file", e))
}

fn remove_if_there(path: &Path) -> Result<(), DeviceError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(store_err("removing a device file", e)),
    }
}

fn record_json(d: &StoredDevice) -> Value {
    json!({
        "install_id": d.install_id,
        "member_id": d.member_id.to_string(),
        "device_id": d.device_id.to_string(),
        "pubkey": B64.encode(d.pubkey),
        "cert": B64.encode(&d.cert),
    })
}

fn record_from(v: &Value) -> Option<StoredDevice> {
    Some(StoredDevice {
        install_id: v.get("install_id")?.as_u64()?,
        member_id: v.get("member_id")?.as_str()?.parse().ok()?,
        device_id: v.get("device_id")?.as_str()?.parse().ok()?,
        pubkey: B64.decode(v.get("pubkey")?.as_str()?).ok()?.try_into().ok()?,
        cert: B64.decode(v.get("cert")?.as_str()?).ok()?,
    })
}

fn key_from(text: &str) -> Option<Zeroizing<[u8; 32]>> {
    let bytes = Zeroizing::new(B64.decode(text.trim()).ok()?);
    let key: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some(Zeroizing::new(key))
}

impl DeviceKeyStore for K2DeviceKeys {
    fn load(&self, id: &DeviceKeyId) -> Result<Option<DeviceKeyRecord>, DeviceError> {
        let Some(text) = self.key_text(id)? else { return Ok(None) };
        // A damaged key or record is a lost device: the engine enrols again.
        let Some(key) = key_from(&text) else { return Ok(None) };
        let Ok(raw) = std::fs::read(self.record_path(id)) else { return Ok(None) };
        let Ok(v) = serde_json::from_slice::<Value>(&raw) else { return Ok(None) };
        let Some(device) = record_from(&v) else { return Ok(None) };
        if device.install_id != id.install_id || device.member_id != id.member_id {
            return Ok(None);
        }
        Ok(Some(DeviceKeyRecord { key, device }))
    }

    fn store(&self, id: &DeviceKeyId, record: &DeviceKeyRecord) -> Result<(), DeviceError> {
        let body = serde_json::to_vec(&record_json(&record.device)).map_err(|e| store_err("writing the device record", e))?;
        write_private(&self.record_path(id), &body)?;
        let secret = Zeroizing::new(B64.encode(*record.key));
        match self.vault.set(&id.account(), &secret) {
            // A key file from a time with no keychain is no longer needed.
            Ok(()) => remove_if_there(&self.key_path(id)),
            Err(e) if self.file_fallback => {
                log_debug!("[native-voice] keychain unavailable, keeping the device key in a 0600 file: {e}");
                write_private(&self.key_path(id), secret.as_bytes())
            }
            Err(e) => {
                log_debug!("[native-voice] keychain unavailable: {e}");
                Err(DeviceError::Store("the system keychain isn't available".into()))
            }
        }
    }

    fn clear(&self, id: &DeviceKeyId) -> Result<(), DeviceError> {
        let vault = self.vault.delete(&id.account());
        remove_if_there(&self.record_path(id))?;
        remove_if_there(&self.key_path(id))?;
        match vault {
            Ok(()) => Ok(()),
            Err(e) if self.file_fallback => {
                log_debug!("[native-voice] keychain unavailable while forgetting a device: {e}");
                Ok(())
            }
            Err(e) => {
                log_debug!("[native-voice] keychain unavailable: {e}");
                Err(DeviceError::Store("the system keychain isn't available".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    const KEY: [u8; 32] = [7; 32];

    /// A keychain in memory. `down`: it can't answer. Tests never touch the real one.
    #[derive(Clone, Default)]
    struct Vault {
        items: Arc<Mutex<HashMap<String, String>>>,
        down: Arc<Mutex<bool>>,
    }

    impl KeyVault for Vault {
        fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, String> {
            if *self.down.lock().unwrap() {
                return Err("no keychain".into());
            }
            Ok(self.items.lock().unwrap().get(account).cloned().map(Zeroizing::new))
        }
        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            if *self.down.lock().unwrap() {
                return Err("no keychain".into());
            }
            self.items.lock().unwrap().insert(account.to_string(), secret.to_string());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            if *self.down.lock().unwrap() {
                return Err("no keychain".into());
            }
            self.items.lock().unwrap().remove(account);
            Ok(())
        }
    }

    fn id() -> DeviceKeyId {
        DeviceKeyId { install_id: 9, member_id: "48831765147549699".parse().unwrap(), device_scope: "fedcba9876543210".into() }
    }

    fn record() -> DeviceKeyRecord {
        DeviceKeyRecord {
            key: Zeroizing::new(KEY),
            device: StoredDevice {
                install_id: 9,
                member_id: "48831765147549699".parse().unwrap(),
                device_id: "48831765147549700".parse().unwrap(),
                pubkey: [4; 65],
                cert: vec![1, 2, 3],
            },
        }
    }

    fn file_text_of_dir(dir: &Path) -> String {
        std::fs::read_dir(dir).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect()
    }

    #[test]
    fn a_record_round_trips_with_its_key_only_in_the_keychain() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::default();
        let store = K2DeviceKeys::new(vault.clone(), tmp.path().join("keys"), true);
        assert!(store.load(&id()).unwrap().is_none(), "nothing stored yet");
        store.store(&id(), &record()).unwrap();

        let back = store.load(&id()).unwrap().expect("stored");
        assert_eq!(*back.key, KEY);
        assert_eq!(back.device, record().device);
        assert_eq!(vault.items.lock().unwrap().get(&id().account()), Some(&B64.encode(KEY)));
        let files = file_text_of_dir(&tmp.path().join("keys"));
        assert!(!files.contains(&B64.encode(KEY)), "the key reached a file: {files}");
        assert!(!tmp.path().join("keys").join(format!("{}.key", K2DeviceKeys::name(&id()))).exists());
    }

    #[test]
    fn store_replaces_and_clear_forgets_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::default();
        let store = K2DeviceKeys::new(vault.clone(), tmp.path().join("keys"), true);
        store.store(&id(), &record()).unwrap();
        let mut newer = record();
        newer.device.cert = vec![9, 9];
        store.store(&id(), &newer).unwrap();
        assert_eq!(store.load(&id()).unwrap().expect("stored").device.cert, vec![9, 9]);
        store.clear(&id()).unwrap();
        assert!(store.load(&id()).unwrap().is_none());
        assert!(vault.items.lock().unwrap().is_empty());
        assert_eq!(file_text_of_dir(&tmp.path().join("keys")), "");
        store.clear(&id()).unwrap();
    }

    #[test]
    fn damage_is_a_lost_device_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::default();
        let store = K2DeviceKeys::new(vault.clone(), tmp.path().join("keys"), true);
        store.store(&id(), &record()).unwrap();
        let path = store.record_path(&id());

        std::fs::write(&path, "not json").unwrap();
        assert!(store.load(&id()).unwrap().is_none(), "a damaged record");
        store.store(&id(), &record()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(store.load(&id()).unwrap().is_none(), "a missing record");

        store.store(&id(), &record()).unwrap();
        vault.items.lock().unwrap().insert(id().account(), "short".into());
        assert!(store.load(&id()).unwrap().is_none(), "a damaged key");
    }

    #[test]
    fn a_record_for_someone_else_is_not_this_device() {
        let tmp = tempfile::tempdir().unwrap();
        let store = K2DeviceKeys::new(Vault::default(), tmp.path().join("keys"), true);
        let mut other = record();
        other.device.member_id = "48831765147549701".parse().unwrap();
        store.store(&id(), &other).unwrap();
        assert!(store.load(&id()).unwrap().is_none());
    }

    #[test]
    fn different_ids_stay_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let store = K2DeviceKeys::new(Vault::default(), tmp.path().join("keys"), true);
        store.store(&id(), &record()).unwrap();
        let mut scope = id();
        scope.device_scope = "another".into();
        assert!(store.load(&scope).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn with_no_keychain_on_linux_the_key_is_a_0600_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::default();
        *vault.down.lock().unwrap() = true;
        let store = K2DeviceKeys::new(vault.clone(), tmp.path().join("keys"), true);
        store.store(&id(), &record()).unwrap();
        assert_eq!(*store.load(&id()).unwrap().expect("from the file").key, KEY);
        for p in [store.key_path(&id()), store.record_path(&id())] {
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "{}", p.display());
        }
        assert_eq!(std::fs::metadata(tmp.path().join("keys")).unwrap().permissions().mode() & 0o777, 0o700);

        // The keychain comes back: the key moves in and the file goes.
        *vault.down.lock().unwrap() = false;
        store.store(&id(), &record()).unwrap();
        assert!(!store.key_path(&id()).exists());
        assert_eq!(*store.load(&id()).unwrap().expect("from the keychain").key, KEY);

        *vault.down.lock().unwrap() = true;
        store.clear(&id()).unwrap();
        assert!(!store.record_path(&id()).exists());
    }

    #[test]
    fn with_no_keychain_elsewhere_the_call_fails_rather_than_writing_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::default();
        *vault.down.lock().unwrap() = true;
        let store = K2DeviceKeys::new(vault, tmp.path().join("keys"), false);
        for e in [store.load(&id()).err().expect("load"), store.store(&id(), &record()).err().expect("store"), store.clear(&id()).err().expect("clear")] {
            assert_eq!(e, DeviceError::Store("the system keychain isn't available".into()));
        }
        assert!(!store.key_path(&id()).exists());
    }
}
