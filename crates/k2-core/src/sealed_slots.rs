//! One sealing module for both LLM login wallets: a server's saved logins
//! (`llm_accounts`, prd-llm-logins-in-vault-v1) and App users' personal
//! wallets (`person_wallet`, 0.46.3).
//!
//! k2-core has no vault dependency. The daemon installs the real vault as
//! a [`SecretSealer`] (system namespace, delivery `none`: no person and no
//! agent ever reads a value; only the daemon resolves it). Tests pass an
//! in-memory fake.
//!
//! The safe order, for every caller:
//! - **seal** = write, then read back and compare byte for byte
//!   ([`seal_verified`]). Nothing that depends on the vault copy happens
//!   before the read-back matches.
//! - **wipe** = overwrite the file, fsync, then unlink
//!   ([`overwrite_then_unlink`]), plus Claude's per-slot keychain item on
//!   macOS ([`wipe_slot_cred`]). A wipe only ever follows a verified seal.

use std::path::Path;

use crate::llm_accounts::{store, Tool, WalletError};

/// What a sealed secret holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealKind {
    /// A subscription's credential file (the tool's own format).
    LoginSlot,
    /// A pasted API key.
    ApiKey,
}

/// Which vault scope a sealed secret lives in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SealScope {
    /// The server's own logins (`llm-login.<tool>.<id>`).
    Server,
    /// One App user's wallet (`wallet-<item>`).
    Person(String),
}

/// Where one sealed secret lives: scope + name (system namespace).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SealAddr {
    pub scope: SealScope,
    pub name: String,
}

/// A sealed secret's status in the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedState {
    Active,
    /// Kept, not usable (a removed login: Trash, never expunge).
    Disabled,
}

/// The vault as both wallets need it. Values cross this trait only
/// between the vault and a slot / a spawn env; never a caller. Every
/// method keeps "the vault is closed" (`Err`) apart from "there is no
/// such item" (`Ok(None)`): the 0.45.7 tunnel lesson.
pub trait SecretSealer: Send + Sync {
    /// Store (or replace) `bytes` at `addr` and make it active. Saving
    /// the same bytes again adds no version.
    fn put_sealed(&self, addr: &SealAddr, kind: SealKind, bytes: &[u8]) -> Result<(), String>;
    /// The active value; `Ok(None)` when there is none (or it is
    /// disabled or destroyed).
    fn get_sealed(&self, addr: &SealAddr) -> Result<Option<Vec<u8>>, String>;
    /// Keep the value but stop it being used. A no-op when missing.
    fn disable_sealed(&self, addr: &SealAddr) -> Result<(), String>;
    /// Destroy the value (its versions go). A no-op when missing.
    fn destroy_sealed(&self, addr: &SealAddr) -> Result<(), String>;
    /// Is there an item at `addr`, and is it active? `Ok(None)` = none.
    fn sealed_state(&self, addr: &SealAddr) -> Result<Option<SealedState>, String>;
    /// Has the owner confirmed the vault's recovery key? `None` when the
    /// vault can't say (closed).
    fn recovery_confirmed(&self) -> Option<bool> {
        None
    }
}

/// Write `bytes`, then read them back and compare byte for byte. `Err`
/// says why without quoting any value; the caller keeps its plaintext.
pub fn seal_verified(s: &dyn SecretSealer, addr: &SealAddr, kind: SealKind, bytes: &[u8]) -> Result<(), String> {
    s.put_sealed(addr, kind, bytes).map_err(|e| format!("vault write failed ({e})"))?;
    verify_holds(|| s.get_sealed(addr), bytes)
}

/// Did the store keep exactly `bytes`? `read` reads it back.
pub fn verify_holds(read: impl FnOnce() -> Result<Option<Vec<u8>>, String>, bytes: &[u8]) -> Result<(), String> {
    match read() {
        Ok(Some(back)) if back == bytes => Ok(()),
        Ok(Some(back)) => Err(format!("read back {} bytes, not the {} written", back.len(), bytes.len())),
        Ok(None) => Err("read back nothing".into()),
        Err(e) => Err(format!("read back failed ({e})")),
    }
}

/// Overwrite a credential file with zeros (its whole length, fsynced),
/// then unlink it. Best effort against recovery: on copy-on-write
/// filesystems and SSDs old blocks may survive (the same honest limit as
/// the vault's own key wipe). A missing file is fine. A symlink is never
/// written through: only the link is removed.
pub fn overwrite_then_unlink(path: &Path) -> Result<(), WalletError> {
    use std::io::Write;
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(WalletError::Io(e.to_string())),
        Ok(m) if m.file_type().is_symlink() => {}
        Ok(m) => {
            let len = m.len() as usize;
            let mut f = std::fs::OpenOptions::new().write(true).open(path).map_err(|e| WalletError::Io(e.to_string()))?;
            f.write_all(&vec![0u8; len.max(1)]).map_err(|e| WalletError::Io(e.to_string()))?;
            f.sync_all().map_err(|e| WalletError::Io(e.to_string()))?;
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WalletError::Io(e.to_string())),
    }
}

/// Remove the plaintext credential from a slot: the file (overwrite, then
/// unlink), and on macOS the per-slot keychain item Claude may have
/// written. Call only after a verified seal.
pub fn wipe_slot_cred(tool: Tool, slot_id: &str) -> Result<(), WalletError> {
    if tool == Tool::Claude && store::keychain_enabled() {
        let svc = store::claude_keychain_service(Some(&store::slot_dir(tool, slot_id)));
        let acct = store::claude_keychain_account();
        if let Ok(true) = store::keychain::exists(&svc, &acct) {
            store::keychain::delete(&svc, &acct).map_err(WalletError::Io)?;
        }
    }
    overwrite_then_unlink(&store::slot_cred_path(tool, slot_id))
}

/// Seal a slot's plaintext and only then wipe it: read the credential
/// (keychain-first on macOS, as Claude reads it), check it is whole,
/// [`seal_verified`], then [`wipe_slot_cred`]. `Ok(false)` when the slot
/// holds nothing. A failed write or a read-back that doesn't match keeps
/// the plaintext and returns `Err(seal_unverified)`. The caller holds the
/// slot lock and has checked no session uses the slot.
pub fn seal_then_wipe(s: &dyn SecretSealer, addr: &SealAddr, tool: Tool, slot_id: &str) -> Result<bool, WalletError> {
    let Some(bytes) = store::read_slot(tool, slot_id)? else {
        return Ok(false);
    };
    store::validate_cred(tool, &bytes).map_err(|why| store::invalid_cred(tool, "sign-in to seal", why))?;
    if let Err(why) = seal_verified(s, addr, SealKind::LoginSlot, &bytes) {
        crate::log_debug!("[sealed-slots] {}: not sealed, plaintext kept: {why}", addr.name);
        return Err(WalletError::Conflict(
            "seal_unverified",
            format!("could not seal the sign-in in the vault ({why}); it stays in its private folder"),
        ));
    }
    wipe_slot_cred(tool, slot_id)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) mod fake {
    //! An in-memory [`SecretSealer`] for tests, with failure modes.

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct FakeSealer {
        pub m: Mutex<HashMap<SealAddr, (SealKind, Vec<u8>, SealedState)>>,
        /// Every call fails (a locked / closed vault).
        pub broken: AtomicBool,
        /// The write "succeeds" but stores a corrupted copy.
        pub corrupts: AtomicBool,
        pub puts: AtomicUsize,
        pub recovery: Mutex<Option<bool>>,
    }

    impl FakeSealer {
        pub fn value(&self, addr: &SealAddr) -> Option<Vec<u8>> {
            self.m.lock().unwrap().get(addr).map(|(_, b, _)| b.clone())
        }
        pub fn state(&self, addr: &SealAddr) -> Option<SealedState> {
            self.m.lock().unwrap().get(addr).map(|(_, _, s)| *s)
        }
        pub fn kind(&self, addr: &SealAddr) -> Option<SealKind> {
            self.m.lock().unwrap().get(addr).map(|(k, _, _)| *k)
        }
        pub fn set_broken(&self, on: bool) {
            self.broken.store(on, Ordering::SeqCst);
        }
        fn check(&self) -> Result<(), String> {
            if self.broken.load(Ordering::SeqCst) {
                return Err("vault: vault_not_open".into());
            }
            Ok(())
        }
    }

    impl SecretSealer for FakeSealer {
        fn put_sealed(&self, addr: &SealAddr, kind: SealKind, bytes: &[u8]) -> Result<(), String> {
            self.check()?;
            self.puts.fetch_add(1, Ordering::SeqCst);
            let mut stored = bytes.to_vec();
            if self.corrupts.load(Ordering::SeqCst) {
                stored.truncate(stored.len() / 2);
            }
            self.m.lock().unwrap().insert(addr.clone(), (kind, stored, SealedState::Active));
            Ok(())
        }
        fn get_sealed(&self, addr: &SealAddr) -> Result<Option<Vec<u8>>, String> {
            self.check()?;
            Ok(self
                .m
                .lock()
                .unwrap()
                .get(addr)
                .filter(|(_, _, s)| *s == SealedState::Active)
                .map(|(_, b, _)| b.clone()))
        }
        fn disable_sealed(&self, addr: &SealAddr) -> Result<(), String> {
            self.check()?;
            if let Some(e) = self.m.lock().unwrap().get_mut(addr) {
                e.2 = SealedState::Disabled;
            }
            Ok(())
        }
        fn destroy_sealed(&self, addr: &SealAddr) -> Result<(), String> {
            self.check()?;
            self.m.lock().unwrap().remove(addr);
            Ok(())
        }
        fn sealed_state(&self, addr: &SealAddr) -> Result<Option<SealedState>, String> {
            self.check()?;
            Ok(self.state(addr))
        }
        fn recovery_confirmed(&self) -> Option<bool> {
            *self.recovery.lock().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSealer;
    use super::*;
    use std::sync::atomic::Ordering;

    const MARKER: &str = "K2TEST_SEALED_SLOT_MARKER";

    fn cred(tag: &str) -> Vec<u8> {
        format!(r#"{{"claudeAiOauth":{{"accessToken":"{MARKER}-at-{tag}","refreshToken":"{MARKER}-rt-{tag}","expiresAt":4102444800000}}}}"#)
            .into_bytes()
    }

    fn addr(n: &str) -> SealAddr {
        SealAddr { scope: SealScope::Server, name: n.into() }
    }

    #[test]
    fn a_corrupted_write_is_never_followed_by_a_wipe() {
        let _h = crate::test_env::TempHome::new();
        let s = FakeSealer::default();
        s.corrupts.store(true, Ordering::SeqCst);
        store::write_private(&store::slot_cred_path(Tool::Claude, "acc_seal1"), &cred("one")).unwrap();
        let err = seal_then_wipe(&s, &addr("llm-login.claude.acc_seal1"), Tool::Claude, "acc_seal1").expect_err("must refuse");
        assert_eq!(err.code(), "seal_unverified");
        assert_eq!(
            std::fs::read(store::slot_cred_path(Tool::Claude, "acc_seal1")).unwrap(),
            cred("one"),
            "the plaintext stays when the read-back doesn't match"
        );
    }

    #[test]
    fn a_closed_vault_keeps_the_plaintext() {
        let _h = crate::test_env::TempHome::new();
        let s = FakeSealer::default();
        s.set_broken(true);
        store::write_private(&store::slot_cred_path(Tool::Claude, "acc_seal2"), &cred("two")).unwrap();
        let err = seal_then_wipe(&s, &addr("llm-login.claude.acc_seal2"), Tool::Claude, "acc_seal2").expect_err("must refuse");
        assert_eq!(err.code(), "seal_unverified");
        assert!(store::slot_cred_path(Tool::Claude, "acc_seal2").exists());
    }

    #[test]
    fn a_verified_seal_wipes_by_overwrite_then_unlink() {
        let _h = crate::test_env::TempHome::new();
        let s = FakeSealer::default();
        let p = store::slot_cred_path(Tool::Claude, "acc_seal3");
        store::write_private(&p, &cred("three")).unwrap();
        assert!(seal_then_wipe(&s, &addr("llm-login.claude.acc_seal3"), Tool::Claude, "acc_seal3").expect("sealed"));
        assert!(!p.exists(), "plaintext gone after the verified seal");
        assert_eq!(s.value(&addr("llm-login.claude.acc_seal3")).expect("in the vault"), cred("three"));
    }

    #[test]
    fn a_broken_file_is_never_sealed() {
        let _h = crate::test_env::TempHome::new();
        let s = FakeSealer::default();
        let p = store::slot_cred_path(Tool::Claude, "acc_seal4");
        store::write_private(&p, b"{\"claudeAiOauth\":{\"accessTok").unwrap();
        let err = seal_then_wipe(&s, &addr("llm-login.claude.acc_seal4"), Tool::Claude, "acc_seal4").expect_err("broken");
        assert_eq!(err.code(), "credential_invalid");
        assert!(p.exists());
        assert_eq!(s.puts.load(Ordering::SeqCst), 0, "nothing written to the vault");
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_never_writes_through_a_symlink() {
        let h = crate::test_env::TempHome::new();
        let target = h.path().join("target.txt");
        std::fs::write(&target, b"keep me").unwrap();
        let link = h.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        overwrite_then_unlink(&link).unwrap();
        assert!(!link.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    }
}
