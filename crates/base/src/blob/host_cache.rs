//! Host cache keys: a key derived from a space's key, for a host to encrypt its own caches of
//! the space's content at rest (rendered previews, for one), so that destroying the space key
//! ([`BlobStore::purge_space_key`]) leaves those caches unreadable too.
//!
//! - **The derivation.** HKDF-SHA256 (RFC 5869, on the kit's own SHA-256): extract the space
//!   key under a public salt, then expand one block with the space's name and the host's
//!   purpose label, each length-prefixed. The space key is the extract's **message**, never an
//!   HMAC key: `HMAC(space key, bytes)` is a blob's id, which any writer gets back for bytes
//!   of their choosing.
//! - **Generations.** A space key has a generation number, never reused
//!   ([`HostCacheKey::generation`]). A purge ends it; the next key the space gets (a blob put,
//!   or [`BlobStore::host_cache_key`]) is a new generation. The host records the generation
//!   beside what it seals (it isn't secret) and opens with [`BlobStore::host_cache_key_at`],
//!   which refuses a destroyed generation (`MPC-E603`) and never makes a key.
//! - **Never stored.** The kit derives the key on each call and keeps no copy; its `Debug`
//!   prints the generation only.
//! - **No cipher here.** The kit has no AEAD of its own; the host seals with its own, under
//!   this key.

use std::fmt;

use super::{BlobError, BlobStore};
use crate::hash::hkdf32;

/// The extract's public salt: names this use of the space key, apart from every other.
const HOST_CACHE_DOMAIN: &[u8] = b"k2-mpc-base/host-cache/v1";

/// A key for a host's cache of one space, for one purpose ([`BlobStore::host_cache_key`]).
/// Derived on each call, never stored by the kit. Its `Debug` prints the generation only.
pub struct HostCacheKey {
    generation: u64,
    key: [u8; 32],
}

impl HostCacheKey {
    /// The generation of the space key it was derived from. Not secret: the host records it
    /// beside what it seals, and opens with [`BlobStore::host_cache_key_at`].
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The key's 32 bytes, for the host's cipher. Don't log, print or store them.
    pub fn secret_bytes(&self) -> &[u8; 32] {
        &self.key
    }

    #[cfg(test)]
    pub(crate) fn for_tests(generation: u64, key: [u8; 32]) -> Self {
        Self { generation, key }
    }
}

impl fmt::Debug for HostCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "HostCacheKey {{ generation: {}, key: <secret> }}",
            self.generation
        )
    }
}

/// `HKDF-SHA256(salt = the domain, ikm = space key, info = len ‖ space ‖ len ‖ label)`.
fn derive(space_key: &[u8; 32], space: &str, label: &[u8]) -> [u8; 32] {
    let mut info = Vec::with_capacity(16 + space.len() + label.len());
    for part in [space.as_bytes(), label] {
        info.extend_from_slice(&(part.len() as u64).to_be_bytes());
        info.extend_from_slice(part);
    }
    hkdf32(HOST_CACHE_DOMAIN, space_key, &info)
}

impl BlobStore {
    /// A key for sealing new entries of a host cache of `space`, for the purpose `label`
    /// (a host's own name for it and its version, e.g. `b"host-cache/previews/v1"`). Derived
    /// from the space's current key, which is made if the space has none yet (as a blob put
    /// makes it). The same space, label and generation give the same key, across restarts;
    /// another label or space gives an unrelated one. Record its
    /// [`generation`](HostCacheKey::generation) beside what it seals.
    pub fn host_cache_key(&self, space: &str, label: &[u8]) -> Result<HostCacheKey, BlobError> {
        let k = self.key(space)?;
        Ok(HostCacheKey {
            generation: k.no,
            key: derive(&k.key, space, label),
        })
    }

    /// The key for opening entries of a host cache of `space` sealed under `generation`, for
    /// the purpose `label`. Never makes a key: when that generation's space key was destroyed
    /// ([`BlobStore::purge_space_key`]), or was never this space's, it refuses with
    /// [`BlobError::CacheKeyDestroyed`] (`MPC-E603`), even after the space has a new key. The
    /// host then drops every entry sealed under that generation.
    pub fn host_cache_key_at(
        &self,
        space: &str,
        label: &[u8],
        generation: u64,
    ) -> Result<HostCacheKey, BlobError> {
        #[cfg(test)]
        if planted() == Some(Planted::KeySurvivesErasure) {
            // the planted bug: the generation goes unchecked, so a destroyed one gets a key
            let k = self.key(space)?;
            return Ok(HostCacheKey {
                generation,
                key: derive(&k.key, space, label),
            });
        }
        match self.docs.blob_space(space)? {
            Some(k) if k.no == generation => Ok(HostCacheKey {
                generation,
                key: derive(&k.key, space, label),
            }),
            _ => Err(BlobError::CacheKeyDestroyed { generation }),
        }
    }
}

/// A bug planted in this module, so its rows can show they catch it (unit tests only).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Planted {
    /// A derived key survives erasure: `host_cache_key_at` doesn't check the generation.
    KeySurvivesErasure,
}

#[cfg(test)]
thread_local! {
    static PLANTED: std::cell::Cell<Option<Planted>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn planted() -> Option<Planted> {
    PLANTED.with(|p| p.get())
}

/// Run `f` with `bug` planted on this thread.
#[cfg(test)]
pub(crate) fn with_planted<R>(bug: Planted, f: impl FnOnce() -> R) -> R {
    struct Back(Option<Planted>);
    impl Drop for Back {
        fn drop(&mut self) {
            let back = self.0;
            PLANTED.with(|p| p.set(back));
        }
    }
    let _back = Back(PLANTED.with(|p| p.replace(Some(bug))));
    f()
}

/// The rows of the host cache key (each a check that names itself when it fails, so the
/// planted bug's row can show which row caught it).
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::blob::BlobConfig;
    use crate::errors::Refusal;
    use crate::store::DocStore;

    const PREVIEWS: &[u8] = b"host-cache/previews/v1";

    struct Fx {
        dir: PathBuf,
        blobs: Option<BlobStore>,
    }

    impl Fx {
        fn new(tag: &str) -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "k2mb-cachekey-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut f = Self { dir, blobs: None };
            f.open();
            f
        }

        fn open(&mut self) {
            let docs = Arc::new(DocStore::open(self.dir.join("docs.redb")).unwrap());
            self.blobs = Some(
                BlobStore::open(
                    docs,
                    self.dir.join("blobs"),
                    BlobConfig::default(),
                    Arc::new(|| 0),
                )
                .unwrap(),
            );
        }

        /// Close the store and open it again from its files (a host restart).
        fn restart(&mut self) {
            self.blobs = None;
            self.open();
        }

        fn b(&self) -> &BlobStore {
            self.blobs.as_ref().unwrap()
        }

        /// The sealing key of space `ws` for previews.
        fn previews(&self) -> Result<HostCacheKey, String> {
            self.b()
                .host_cache_key("ws", PREVIEWS)
                .map_err(|e| e.to_string())
        }

        fn space_key(&self, space: &str) -> Option<[u8; 32]> {
            self.b().docs.blob_space(space).unwrap().map(|k| k.key)
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            self.blobs = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fail(row: &str, what: impl fmt::Display) -> Result<(), String> {
        Err(format!("{row}: {what}"))
    }

    /// Row 1: the same space and label give the same key across a restart, by either call,
    /// and the kit stores no copy of it (the store file holds none of its bytes).
    fn row1(tag: &str) -> Result<(), String> {
        const R: &str = "row 1";
        let mut f = Fx::new(tag);
        let first = f.previews()?;
        let g = first.generation();
        let bytes = *first.secret_bytes();
        f.b().put_bytes("ws", b"another write").unwrap();
        let found = f.b().docs.scan_for(&[&bytes]).unwrap();
        if !found.is_empty() {
            return fail(R, "the derived key's bytes are in the store file");
        }
        f.restart();
        let again = f.previews()?;
        if again.generation() != g || again.secret_bytes() != &bytes {
            return fail(R, "another key after a restart");
        }
        let at = f.b().host_cache_key_at("ws", PREVIEWS, g);
        match at {
            Ok(k) if k.secret_bytes() == &bytes && k.generation() == g => {}
            other => return fail(R, format!("opening at generation {g} gave {other:?}")),
        }
        let space_key = f.space_key("ws").unwrap();
        if bytes == space_key {
            return fail(R, "the derived key is the space key itself");
        }
        Ok(())
    }

    /// Row 2: another label, another space, or the label moved into the space's name gives
    /// another key; and none is a blob id a writer can get back.
    fn row2(tag: &str) -> Result<(), String> {
        const R: &str = "row 2";
        let f = Fx::new(tag);
        let key =
            |space: &str, label: &[u8]| *f.b().host_cache_key(space, label).unwrap().secret_bytes();
        let base = key("ws", PREVIEWS);
        let others = [
            ("another label", key("ws", b"host-cache/previews/v2")),
            ("an empty label", key("ws", b"")),
            ("another space", key("ws2", PREVIEWS)),
            // the length prefixes keep (space, label) pairs apart
            ("the split moved", key("wsh", b"ost-cache/previews/v1")),
        ];
        for (what, k) in &others {
            if *k == base {
                return fail(R, format!("{what} gave the same key"));
            }
        }
        for (i, (a, ka)) in others.iter().enumerate() {
            for (b, kb) in &others[i + 1..] {
                if ka == kb {
                    return fail(R, format!("{a} and {b} gave the same key"));
                }
            }
        }
        // a writer who stores the label's bytes gets their blob id: never the key
        let id = f.b().put_bytes("ws", PREVIEWS).unwrap().id;
        if id.as_bytes() == &base {
            return fail(R, "the key is the blob id of the label's bytes");
        }
        Ok(())
    }

    /// Row 3: once the space key is destroyed, opening at its generation refuses with
    /// `MPC-E603`: right after the purge, after the space gets a new key, and after a
    /// restart; the new generation's key is unrelated, and the old key's bytes left the
    /// store file.
    fn row3(tag: &str) -> Result<(), String> {
        const R: &str = "row 3";
        let mut f = Fx::new(tag);
        let old = f.previews()?;
        let g = old.generation();
        let old_space_key = f.space_key("ws").unwrap();
        let rep = f.b().purge_space_key("ws").unwrap();
        if !rep.key_destroyed {
            return fail(R, "the purge destroyed no key");
        }
        let refused = |f: &Fx, when: &str| -> Result<(), String> {
            let e = match f.b().host_cache_key_at("ws", PREVIEWS, g) {
                Err(e) => e,
                Ok(k) => {
                    return fail(
                        R,
                        format!("{when}: generation {g} gave {k:?}, not MPC-E603"),
                    )
                }
            };
            let r = e.report();
            if e != (BlobError::CacheKeyDestroyed { generation: g })
                || r.id() != "MPC-E603"
                || r.exit() != 4
            {
                let (id, exit) = (r.id(), r.exit());
                return fail(R, format!("{when}: refused {e:?} as {id} exit {exit}"));
            }
            Ok(())
        };
        refused(&f, "right after the purge")?;
        if !rep.store_rewritten || !f.b().docs.scan_for(&[&old_space_key]).unwrap().is_empty() {
            return fail(R, "the destroyed key's bytes are in the store file");
        }
        // the space gets a new key: a new generation, an unrelated key; the old one stays gone
        f.b().put_bytes("ws", b"stored after the purge").unwrap();
        refused(&f, "after the space got a new key")?;
        let new = f.previews()?;
        if new.generation() == g || new.secret_bytes() == old.secret_bytes() {
            return fail(R, "the new key is the destroyed generation's");
        }
        f.restart();
        refused(&f, "after a restart")?;
        Ok(())
    }

    #[test]
    fn row1_the_same_space_and_label_give_the_same_key_across_a_restart() {
        row1("row1").unwrap();
    }

    #[test]
    fn row2_another_label_or_space_gives_another_key() {
        row2("row2").unwrap();
    }

    #[test]
    fn row3_after_the_space_key_is_destroyed_the_call_refuses() {
        row3("row3").unwrap();
    }

    /// Row 4: the planted bug "a derived key survives erasure" is caught, by row 3, by name.
    #[test]
    fn row4_planted_key_survives_erasure_is_caught_by_row_3() {
        let got = with_planted(Planted::KeySurvivesErasure, || row3("row4"));
        let err = got.expect_err("the planted bug went uncaught");
        assert!(err.starts_with("row 3: "), "caught by another check: {err}");
        assert!(err.contains("not MPC-E603"), "another reason: {err}");
        // and the rows that don't test erasure pass with it planted (the plant is that one bug)
        with_planted(Planted::KeySurvivesErasure, || {
            row1("row4-r1").unwrap();
            row2("row4-r2").unwrap();
        });
    }
}
