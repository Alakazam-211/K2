//! In-memory KeyStore for tests (supports the anchor, so rollback and
//! substitution checks are exercised).

use parking_lot::Mutex;

use super::{Anchor, KeyHome, KeyStore};
use crate::error::{Result, VaultError};
use crate::secret::Key32;

#[derive(Default)]
pub struct FakeKeyStore {
    kek: Mutex<Option<[u8; 32]>>,
    next: Mutex<Option<[u8; 32]>>,
    anchor: Mutex<Option<Anchor>>,
    /// Simulate a crash between the vault.db commit and the anchor write.
    pub fail_set_anchor: Mutex<bool>,
    pub anchor_supported: bool,
}

impl FakeKeyStore {
    pub fn new() -> FakeKeyStore {
        FakeKeyStore { anchor_supported: true, ..Default::default() }
    }

    pub fn without_anchor() -> FakeKeyStore {
        FakeKeyStore { anchor_supported: false, ..Default::default() }
    }

    /// Test hook: overwrite the anchor (e.g. to simulate an older vault.db).
    pub fn force_anchor(&self, a: Option<Anchor>) {
        *self.anchor.lock() = a;
    }

    pub fn has_kek(&self) -> bool {
        self.kek.lock().is_some()
    }
}

impl KeyStore for FakeKeyStore {
    fn kind(&self) -> KeyHome {
        KeyHome::Fake
    }

    fn load(&self) -> Result<Option<Key32>> {
        Ok(self.kek.lock().as_ref().map(Key32::from_bytes))
    }

    fn init(&self) -> Result<Key32> {
        let mut g = self.kek.lock();
        if g.is_some() {
            return Err(VaultError::conflict("kek_exists", "a box key already exists"));
        }
        let k = Key32::random();
        *g = Some(*k.as_bytes());
        Ok(k)
    }

    fn supports_anchor(&self) -> bool {
        self.anchor_supported
    }

    fn anchor(&self) -> Result<Option<Anchor>> {
        if !self.anchor_supported {
            return Ok(None);
        }
        Ok(self.anchor.lock().clone())
    }

    fn set_anchor(&self, a: &Anchor) -> Result<()> {
        if !self.anchor_supported {
            return Ok(());
        }
        if *self.fail_set_anchor.lock() {
            return Err(VaultError::internal("simulated anchor write failure"));
        }
        *self.anchor.lock() = Some(a.clone());
        Ok(())
    }

    fn rotation_pending(&self) -> Result<bool> {
        Ok(self.next.lock().is_some())
    }

    fn stage_next(&self) -> Result<Key32> {
        let k = Key32::random();
        *self.next.lock() = Some(*k.as_bytes());
        Ok(k)
    }

    fn commit_next(&self) -> Result<()> {
        let n = self.next.lock().take().ok_or_else(|| VaultError::conflict("no_staged_kek", "nothing staged"))?;
        *self.kek.lock() = Some(n);
        Ok(())
    }

    fn abort_next(&self) -> Result<()> {
        *self.next.lock() = None;
        Ok(())
    }

    fn remove(&self) -> Result<()> {
        *self.kek.lock() = None;
        Ok(())
    }
}
