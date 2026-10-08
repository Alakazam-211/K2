//! Keeper KeyStore: the KEK held by the root helper (`k2-root-helper keep
//! …`, protocol ≥ 4; root-helper PRD §7.5). The daemon side ships in
//! 0.45.2 and stays dormant until a helper answers `keep status` with
//! protocol 4 (KV23). Values never pass through the helper; it holds only
//! the KEK and a generation counter.
//!
//! Verbs: `keep status`, `keep kek init|get|stage|commit|abort`,
//! `keep gen get|bump`. The transport is a trait so tests use a fake
//! helper and the daemon uses `sudo -n <helper>`.

use serde::Deserialize;
use zeroize::Zeroize;

use super::{Anchor, KeyHome, KeyStore};
use crate::error::{Result, VaultError};
use crate::secret::Key32;

pub const KEEP_MIN_PROTOCOL: u32 = 4;

/// Runs one helper verb. `args` like `["keep", "kek", "get"]`. Returns
/// stdout bytes; a non-zero exit is an `Err`.
pub trait KeeperTransport: Send + Sync {
    fn call(&self, args: &[&str]) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct KeepStatus {
    pub protocol: u32,
    pub kek: String,
    #[serde(default)]
    pub next: Option<String>,
    pub gen: u64,
    #[serde(default)]
    pub sealed: Option<String>,
}

impl KeepStatus {
    pub fn kek_present(&self) -> bool {
        self.kek == "present"
    }
}

/// Ask the helper for `keep status`; `None` when there's no helper, the
/// call fails, or the protocol is too old.
pub fn probe(t: &dyn KeeperTransport) -> Option<KeepStatus> {
    let out = t.call(&["keep", "status"]).ok()?;
    let s: KeepStatus = serde_json::from_slice(&out).ok()?;
    if s.protocol >= KEEP_MIN_PROTOCOL {
        Some(s)
    } else {
        None
    }
}

pub struct KeeperKeyStore<T: KeeperTransport> {
    t: T,
}

impl<T: KeeperTransport> KeeperKeyStore<T> {
    pub fn new(t: T) -> Self {
        KeeperKeyStore { t }
    }

    fn status(&self) -> Result<KeepStatus> {
        let out = self.t.call(&["keep", "status"])?;
        serde_json::from_slice(&out).map_err(|_| VaultError::internal("keeper: bad status reply"))
    }

    fn key_from(mut out: Vec<u8>) -> Result<Key32> {
        if out.len() != 32 {
            out.zeroize();
            return Err(VaultError::internal("keeper: KEK has the wrong length"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&out);
        out.zeroize();
        Ok(Key32::from_array(arr))
    }

    fn parse_gen(out: &[u8]) -> Result<u64> {
        std::str::from_utf8(out)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| VaultError::internal("keeper: bad gen reply"))
    }
}

impl<T: KeeperTransport> KeyStore for KeeperKeyStore<T> {
    fn kind(&self) -> KeyHome {
        match self.status().ok().and_then(|s| s.sealed) {
            Some(s) if s != "none" => KeyHome::KeeperTpm,
            _ => KeyHome::Keeper,
        }
    }

    fn load(&self) -> Result<Option<Key32>> {
        if !self.status()?.kek_present() {
            return Ok(None);
        }
        Self::key_from(self.t.call(&["keep", "kek", "get"])?).map(Some)
    }

    fn init(&self) -> Result<Key32> {
        if self.status()?.kek_present() {
            return Err(VaultError::conflict("kek_exists", "the root helper already holds a box key"));
        }
        self.t.call(&["keep", "kek", "init"])?;
        Self::key_from(self.t.call(&["keep", "kek", "get"])?)
    }

    fn supports_anchor(&self) -> bool {
        true
    }

    fn anchor(&self) -> Result<Option<Anchor>> {
        let g = Self::parse_gen(&self.t.call(&["keep", "gen", "get"])?)?;
        Ok(Some(Anchor { vault_id: None, gen: g, audit_seq: None, audit_mac: None }))
    }

    fn set_anchor(&self, a: &Anchor) -> Result<()> {
        let mut g = Self::parse_gen(&self.t.call(&["keep", "gen", "get"])?)?;
        // The helper only counts up.
        let mut guard = 0;
        while g < a.gen {
            g = Self::parse_gen(&self.t.call(&["keep", "gen", "bump"])?)?;
            guard += 1;
            if guard > 1_000_000 {
                return Err(VaultError::internal("keeper: gen bump does not advance"));
            }
        }
        Ok(())
    }

    fn stage_next(&self) -> Result<Key32> {
        Self::key_from(self.t.call(&["keep", "kek", "stage"])?)
    }

    fn commit_next(&self) -> Result<()> {
        self.t.call(&["keep", "kek", "commit"]).map(|_| ())
    }

    fn abort_next(&self) -> Result<()> {
        self.t.call(&["keep", "kek", "abort"]).map(|_| ())
    }

    fn remove(&self) -> Result<()> {
        Err(VaultError::forbidden(
            "keeper_remove_refused",
            "the vault never moves its key out of the root helper",
        ))
    }
}

/// A fake helper for tests: holds the KEK and counter in memory.
#[cfg(any(test, feature = "test-util"))]
pub mod fake {
    use parking_lot::Mutex;

    use super::*;

    pub struct FakeHelper {
        pub protocol: u32,
        pub kek: Mutex<Option<[u8; 32]>>,
        pub next: Mutex<Option<[u8; 32]>>,
        pub gen: Mutex<u64>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeHelper {
        pub fn new(protocol: u32) -> FakeHelper {
            FakeHelper {
                protocol,
                kek: Mutex::new(None),
                next: Mutex::new(None),
                gen: Mutex::new(0),
                calls: Mutex::new(vec![]),
            }
        }
    }

    impl KeeperTransport for &FakeHelper {
        fn call(&self, args: &[&str]) -> Result<Vec<u8>> {
            self.calls.lock().push(args.join(" "));
            let present = |o: &Option<[u8; 32]>| if o.is_some() { "present" } else { "absent" };
            match args {
                ["keep", "status"] => Ok(format!(
                    r#"{{"protocol":{},"kek":"{}","next":"{}","gen":{},"sealed":"none"}}"#,
                    self.protocol,
                    present(&self.kek.lock()),
                    present(&self.next.lock()),
                    self.gen.lock()
                )
                .into_bytes()),
                _ if self.protocol < KEEP_MIN_PROTOCOL => Err(VaultError::internal("unknown verb")),
                ["keep", "kek", "init"] => {
                    let mut g = self.kek.lock();
                    if g.is_some() {
                        return Err(VaultError::internal("exists"));
                    }
                    *g = Some(crate::crypto::random_bytes::<32>());
                    Ok(vec![])
                }
                ["keep", "kek", "get"] => self.kek.lock().map(|k| k.to_vec()).ok_or_else(|| VaultError::internal("absent")),
                ["keep", "kek", "stage"] => {
                    let k = crate::crypto::random_bytes::<32>();
                    *self.next.lock() = Some(k);
                    Ok(k.to_vec())
                }
                ["keep", "kek", "commit"] => {
                    let n = self.next.lock().take().ok_or_else(|| VaultError::internal("none staged"))?;
                    *self.kek.lock() = Some(n);
                    Ok(vec![])
                }
                ["keep", "kek", "abort"] => {
                    *self.next.lock() = None;
                    Ok(vec![])
                }
                ["keep", "gen", "get"] => Ok(self.gen.lock().to_string().into_bytes()),
                ["keep", "gen", "bump"] => {
                    let mut g = self.gen.lock();
                    *g += 1;
                    Ok(g.to_string().into_bytes())
                }
                _ => Err(VaultError::internal("unknown verb")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeHelper;
    use super::*;

    #[test]
    fn probe_needs_protocol_4() {
        let old = FakeHelper::new(2);
        assert!(probe(&&old).is_none());
        let new = FakeHelper::new(4);
        assert_eq!(probe(&&new).expect("p4").protocol, 4);
    }

    #[test]
    fn keeper_round_trip_and_gen() {
        let h = FakeHelper::new(4);
        let ks = KeeperKeyStore::new(&h);
        assert!(ks.load().expect("load").is_none());
        let k = ks.init().expect("init");
        assert!(ks.load().expect("load").expect("present") == k);
        ks.set_anchor(&Anchor { gen: 3, ..Default::default() }).expect("bump");
        assert_eq!(ks.anchor().expect("a").expect("some").gen, 3);
        // Never counts down.
        ks.set_anchor(&Anchor { gen: 1, ..Default::default() }).expect("noop");
        assert_eq!(ks.anchor().expect("a").expect("some").gen, 3);
        let n = ks.stage_next().expect("stage");
        ks.commit_next().expect("commit");
        assert!(ks.load().expect("load").expect("cur") == n);
        assert_eq!(ks.remove().err().expect("refused").code, "keeper_remove_refused");
    }
}
