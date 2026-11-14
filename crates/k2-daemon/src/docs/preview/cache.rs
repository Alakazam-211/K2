//! The preview cache on disk (contract Item 8; K2's host markup §C.1, §C.3).
//!
//! **Where:** `~/.k2/cache/doc-previews/<space>/<doc>/<entry>.k2p`, owner
//! only (as the blob folder). `<space>` is the workspace id (hashed when it
//! isn't a plain name), so removing a workspace deletes one folder; `<doc>`
//! is 32 hex of sha256 of the doc key, and it moves with the doc on a
//! rename; `<entry>` is `<container: 16 hex>-<render hash>-<width>-<renderer
//! build>`. The cache key is the contract's `(doc, container, render_hash,
//! width, renderer build)`. A scene view's id image is an entry of its own
//! beside its image, under the image's build plus `.idimg` ([`ids_build`]):
//! sealed, trimmed and moved the same way, and never taken for an image.
//!
//! **Sealed at rest.** Each entry is ChaCha20-Poly1305 under the space's
//! host cache key (`BlobStore::host_cache_key(space, LABEL)`, kit v0.15.0),
//! with the key's generation written in the clear beside it and the entry's
//! name as associated data (an entry copied under another name won't open).
//! It is opened with `host_cache_key_at(space, LABEL, generation)`. After the
//! space key is purged (`purge_space_key`) that refuses with `MPC-E603`, and
//! the whole space's cache is dropped: destroying the space key covers its
//! previews too.
//!
//! **Bounded:** at most [`KEEP_PER_SLOT`] hashes per (container, width) are
//! kept (the newest; an undo back to a known hash is a hit), and the whole
//! cache is pruned oldest-first past its cap ([`cap_bytes`]; a read touches
//! an entry's time).
//!
//! **The purge hooks' file side:** [`drop_doc`] (a doc's content purged or
//! erased, the doc deleted), [`drop_space`] (a workspace removed; its key
//! purged), [`move_doc`] (a rename), [`prune`] (the cap).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest as _, Sha256};

use k2_core::log_debug;
use k2_mpc_base::blob::{BlobError, BlobStore};
use k2_mpc_base::store::DocKey;

use super::Width;

/// The purpose label of the host cache key (kit `host_cache_key`).
pub const LABEL: &[u8] = b"k2/doc-previews/v1";

/// The cache's cap by default (contract Item 8: about 500 MiB).
pub const DEFAULT_CAP: u64 = 500 << 20;

/// Env override for the cap, in MiB (tests lower it).
pub const ENV_CAP_MIB: &str = "K2_DOCS_PREVIEW_CACHE_MIB";

/// Hashes kept per (container, width).
pub const KEEP_PER_SLOT: usize = 3;

const MAGIC: &[u8; 4] = b"K2PV";
const FORMAT: u8 = 1;
pub(super) const EXT: &str = "k2p";

/// The cache's root.
pub fn root() -> PathBuf {
    k2_core::paths::k2_home().join("cache").join("doc-previews")
}

/// The cap in bytes: `K2_DOCS_PREVIEW_CACHE_MIB`, else [`DEFAULT_CAP`].
pub fn cap_bytes() -> u64 {
    std::env::var(ENV_CAP_MIB)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .map(|m| m.saturating_mul(1 << 20))
        .unwrap_or(DEFAULT_CAP)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_be_bytes());
        h.update(p);
    }
    h.finalize().into()
}

/// A space's folder name: the id itself when it is a plain name, else 32
/// hex of its hash (never a path).
fn space_dir_name(space: &str) -> String {
    let plain = !space.is_empty() && space.len() <= 64 && space.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        space.to_string()
    } else {
        format!("h{}", &hex(&sha(&[space.as_bytes()]))[..32])
    }
}

pub fn space_dir(space: &str) -> PathBuf {
    root().join(space_dir_name(space))
}

/// A doc's folder: 32 hex of sha256 of its key.
pub fn doc_dir(key: &DocKey) -> PathBuf {
    space_dir(&key.space).join(&hex(&sha(&[key.space.as_bytes(), key.path.as_bytes()]))[..32])
}

pub(super) fn container_id(container: &str) -> String {
    hex(&sha(&[container.as_bytes()]))[..16].to_string()
}

pub(super) fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '_' }).collect()
}

/// An entry's file name.
pub fn entry_name(container: &str, hash: &str, width: Width, build: &str) -> String {
    format!("{}-{}-{}-{}.{EXT}", container_id(container), clean(hash), width.as_str(), clean(build))
}

/// The build an image's id image is cached under, beside the image's own
/// entry (`build` is the image's, its look included).
pub fn ids_build(build: &str) -> String {
    format!("{build}.idimg")
}

/// One cached preview: the metadata the route answers with, and the image.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub meta: serde_json::Value,
    pub image: Vec<u8>,
}

fn cipher(secret: &[u8; 32]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(secret))
}

fn restrict(dir: &Path) {
    if let Err(e) = k2_core::private_home::restrict_path(dir) {
        log_debug!("[docs/preview] WARN: {}: {e}", dir.display());
    }
}

/// Seal and write one entry (atomically: a temp file, then a rename), then
/// keep the newest [`KEEP_PER_SLOT`] of its (container, width).
#[allow(clippy::too_many_arguments)]
pub fn put(blobs: &BlobStore, key: &DocKey, container: &str, hash: &str, width: Width, build: &str, meta: &serde_json::Value, image: &[u8]) -> Result<(), String> {
    let k = blobs.host_cache_key(&key.space, LABEL).map_err(|e| format!("the space's cache key: {e}"))?;
    let dir = doc_dir(key);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    restrict(&root());
    let name = entry_name(container, hash, width, build);
    let meta_bytes = serde_json::to_vec(meta).map_err(|e| e.to_string())?;
    let mut plain = Vec::with_capacity(4 + meta_bytes.len() + image.len());
    plain.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
    plain.extend_from_slice(&meta_bytes);
    plain.extend_from_slice(image);
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("no randomness: {e}"))?;
    let sealed = cipher(k.secret_bytes())
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plain, aad: name.as_bytes() })
        .map_err(|_| "sealing failed".to_string())?;
    let mut out = Vec::with_capacity(4 + 1 + 8 + 12 + sealed.len());
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);
    out.extend_from_slice(&k.generation().to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
        fs::rename(&tmp, dir.join(&name))
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(format!("{}: {e}", dir.display()));
    }
    trim_slot(&dir, container, width, build);
    Ok(())
}

/// What a read found.
#[derive(Debug, Clone, PartialEq)]
pub enum Got {
    Hit(Entry),
    Miss,
    /// Sealed under a destroyed key (MPC-E603): the space's cache is dropped.
    KeyDestroyed,
}

/// Open one entry by name in a doc's folder (touching its time).
pub(super) fn open(blobs: &BlobStore, key: &DocKey, name: &str) -> Got {
    let path = doc_dir(key).join(name);
    let raw = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Got::Miss,
    };
    if raw.len() < 4 + 1 + 8 + 12 + 16 || &raw[..4] != MAGIC || raw[4] != FORMAT {
        log_debug!("[docs/preview] dropping a malformed cache entry {}", path.display());
        let _ = fs::remove_file(&path);
        return Got::Miss;
    }
    let generation = u64::from_be_bytes(raw[5..13].try_into().expect("8 bytes"));
    let k = match blobs.host_cache_key_at(&key.space, LABEL, generation) {
        Ok(k) => k,
        Err(BlobError::CacheKeyDestroyed { .. }) => {
            let n = drop_space(&key.space).unwrap_or(0);
            super::service::audit_drop("key_purged", &key.space, None, n);
            return Got::KeyDestroyed;
        }
        Err(e) => {
            log_debug!("[docs/preview] WARN: the cache key of {}: {e}", key.space);
            return Got::Miss;
        }
    };
    let nonce = &raw[13..25];
    let plain = match cipher(k.secret_bytes()).decrypt(Nonce::from_slice(nonce), Payload { msg: &raw[25..], aad: name.as_bytes() }) {
        Ok(p) => p,
        Err(_) => {
            log_debug!("[docs/preview] dropping a cache entry that doesn't open {}", path.display());
            let _ = fs::remove_file(&path);
            return Got::Miss;
        }
    };
    if plain.len() < 4 {
        return Got::Miss;
    }
    let n = u32::from_be_bytes(plain[..4].try_into().expect("4 bytes")) as usize;
    let Some(meta_bytes) = plain.get(4..4 + n) else { return Got::Miss };
    let Ok(meta) = serde_json::from_slice(meta_bytes) else { return Got::Miss };
    if let Ok(f) = fs::File::options().write(true).open(&path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    Got::Hit(Entry { meta, image: plain[4 + n..].to_vec() })
}

/// Whether an entry for exactly this key is on disk (not opened).
pub fn entry_exists(key: &DocKey, container: &str, hash: &str, width: Width, build: &str) -> bool {
    doc_dir(key).join(entry_name(container, hash, width, build)).is_file()
}

/// The entry for exactly this key.
pub fn get(blobs: &BlobStore, key: &DocKey, container: &str, hash: &str, width: Width, build: &str) -> Got {
    open(blobs, key, &entry_name(container, hash, width, build))
}

/// The files of one (container, width) slot, newest first.
fn slot_files(dir: &Path, container: &str, width: Width, build: &str) -> Vec<(PathBuf, std::time::SystemTime)> {
    let prefix = format!("{}-", container_id(container));
    let suffix = format!("-{}-{}.{EXT}", width.as_str(), clean(build));
    let mut v: Vec<(PathBuf, std::time::SystemTime)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.starts_with(&prefix) && name.ends_with(&suffix))
                .then(|| (e.path(), e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn trim_slot(dir: &Path, container: &str, width: Width, build: &str) {
    for (p, _) in slot_files(dir, container, width, build).into_iter().skip(KEEP_PER_SLOT) {
        let _ = fs::remove_file(p);
    }
}

/// The newest entry of (container, width) under another hash: the `stale`
/// image shown while the current one renders.
pub fn newest_other(blobs: &BlobStore, key: &DocKey, container: &str, hash: &str, width: Width, build: &str) -> Option<Entry> {
    let current = entry_name(container, hash, width, build);
    for (p, _) in slot_files(&doc_dir(key), container, width, build) {
        let name = p.file_name()?.to_string_lossy().into_owned();
        if name == current {
            continue;
        }
        match open(blobs, key, &name) {
            Got::Hit(e) => return Some(e),
            Got::KeyDestroyed => return None,
            Got::Miss => {}
        }
    }
    None
}

/// The widths cached for a container (any hash): what is re-rendered when
/// its hash moves.
pub fn widths_cached(key: &DocKey, container: &str, build: &str) -> Vec<Width> {
    let dir = doc_dir(key);
    Width::ALL.into_iter().filter(|w| !slot_files(&dir, container, *w, build).is_empty()).collect()
}

fn count_files(dir: &Path) -> usize {
    walk(dir).len()
}

fn walk(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                out.push((e.path(), m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)));
            }
        }
    }
    out
}

/// Drop every preview of a doc. Returns how many entries went.
pub fn drop_doc(key: &DocKey) -> usize {
    let dir = doc_dir(key);
    let n = count_files(&dir);
    match fs::remove_dir_all(&dir) {
        Ok(()) => n,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => {
            log_debug!("[docs/preview] WARN: dropping {}: {e}", dir.display());
            n.saturating_sub(count_files(&dir))
        }
    }
}

/// Drop a whole space's previews (a removed workspace; its key purged).
/// `Err` when the folder is still there (the caller retries).
pub fn drop_space(space: &str) -> Result<usize, String> {
    let dir = space_dir(space);
    let n = count_files(&dir);
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(n),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// A rename: the doc's previews follow it (a doc moved to another space
/// can't keep them: they are sealed under its old space's key, so they go).
pub fn move_doc(old: &DocKey, new: &DocKey) -> usize {
    let from = doc_dir(old);
    if !from.exists() {
        return 0;
    }
    if old.space != new.space {
        return drop_doc(old);
    }
    let to = doc_dir(new);
    let _ = fs::remove_dir_all(&to);
    match fs::rename(&from, &to) {
        Ok(()) => count_files(&to),
        Err(e) => {
            log_debug!("[docs/preview] WARN: moving previews {} → {}: {e}; dropping them", from.display(), to.display());
            drop_doc(old)
        }
    }
}

/// The cap: oldest entries go first until the cache is under 90% of
/// `cap`. Returns `(entries, bytes)` removed.
pub fn prune(cap: u64) -> (usize, u64) {
    let mut files = walk(&root());
    let total: u64 = files.iter().map(|f| f.1).sum();
    if total <= cap {
        return (0, 0);
    }
    files.sort_by(|a, b| a.2.cmp(&b.2));
    let target = cap / 10 * 9;
    let (mut left, mut n, mut bytes) = (total, 0, 0);
    for (p, len, _) in files {
        if left <= target {
            break;
        }
        if fs::remove_file(&p).is_ok() {
            left -= len;
            n += 1;
            bytes += len;
        }
    }
    (n, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn store() -> (crate::test_support::Scratch, Arc<BlobStore>) {
        let s = crate::test_support::Scratch::new();
        let docs = Arc::new(k2_mpc_base::store::DocStore::open(&s.path().join("docs.redb")).expect("store"));
        let blobs = BlobStore::open(docs, &s.path().join("blobs"), k2_mpc_base::blob::BlobConfig::default(), Arc::new(|| 1_000)).expect("blobs");
        (s, Arc::new(blobs))
    }

    fn key(space: &str, path: &str) -> DocKey {
        DocKey { space: space.into(), path: path.into() }
    }

    fn meta(n: u64) -> serde_json::Value {
        serde_json::json!({ "rev": n })
    }

    #[test]
    fn an_entry_is_sealed_and_opens_by_its_key_only() {
        crate::test_support::with_temp_home(|| {
            let (_s, blobs) = store();
            let k = key("ws1", "decks/talk.pptx");
            put(&blobs, &k, "@sld:256", "aa11", Width::Thumb, "b1", &meta(3), b"IMAGE-BYTES").expect("put");
            assert_eq!(get(&blobs, &k, "@sld:256", "aa11", Width::Thumb, "b1"), Got::Hit(Entry { meta: meta(3), image: b"IMAGE-BYTES".to_vec() }));
            for (c, h, w, b) in [("@sld:257", "aa11", Width::Thumb, "b1"), ("@sld:256", "bb22", Width::Thumb, "b1"), ("@sld:256", "aa11", Width::Full, "b1"), ("@sld:256", "aa11", Width::Thumb, "b2")] {
                assert_eq!(get(&blobs, &k, c, h, w, b), Got::Miss, "{c} {h} {w:?} {b}");
            }
            // At rest it is sealed: no image bytes, no metadata in the clear.
            let path = doc_dir(&k).join(entry_name("@sld:256", "aa11", Width::Thumb, "b1"));
            let raw = fs::read(&path).expect("file");
            assert!(raw.starts_with(MAGIC));
            assert!(!raw.windows(11).any(|w| w == b"IMAGE-BYTES"), "the image is sealed");
            assert!(!raw.windows(5).any(|w| w == b"\"rev\""), "the metadata is sealed");
            // Under another name it doesn't open (the name is bound in).
            let other = doc_dir(&k).join(entry_name("@sld:256", "cc33", Width::Thumb, "b1"));
            fs::copy(&path, &other).expect("copy");
            assert_eq!(get(&blobs, &k, "@sld:256", "cc33", Width::Thumb, "b1"), Got::Miss);
            assert!(!other.exists(), "an entry that doesn't open is dropped");
            // The path names no file, slide or workspace path.
            let p = path.to_string_lossy();
            assert!(!p.contains("talk") && !p.contains("sld") && !p.contains("decks"), "{p}");
            assert!(path.starts_with(root().join("ws1")));
        });
    }

    /// Contract: destroying the space key covers previews. After
    /// `purge_space_key`, opening an old entry meets MPC-E603 and drops the
    /// space's whole cache.
    #[test]
    fn a_purged_space_key_drops_the_spaces_cache() {
        crate::test_support::with_temp_home(|| {
            let (_s, blobs) = store();
            let k = key("ws2", "talk.pptx");
            let k2 = key("ws2", "other.pptx");
            put(&blobs, &k, "@sld:256", "aa11", Width::Full, "b1", &meta(1), b"x").expect("put");
            put(&blobs, &k2, "@sld:256", "aa11", Width::Full, "b1", &meta(1), b"y").expect("put");
            blobs.purge_space_key("ws2").expect("purge");
            assert_eq!(get(&blobs, &k, "@sld:256", "aa11", Width::Full, "b1"), Got::KeyDestroyed);
            assert!(!space_dir("ws2").exists(), "the space's previews are gone");
            assert_eq!(get(&blobs, &k2, "@sld:256", "aa11", Width::Full, "b1"), Got::Miss);
            // A new entry is sealed under the new generation and opens.
            put(&blobs, &k, "@sld:256", "aa11", Width::Full, "b1", &meta(2), b"z").expect("put again");
            assert!(matches!(get(&blobs, &k, "@sld:256", "aa11", Width::Full, "b1"), Got::Hit(_)));
        });
    }

    #[test]
    fn a_slot_keeps_its_newest_hashes_and_a_stale_one_is_found() {
        crate::test_support::with_temp_home(|| {
            let (_s, blobs) = store();
            let k = key("ws3", "talk.pptx");
            for (i, h) in ["h1", "h2", "h3", "h4"].iter().enumerate() {
                put(&blobs, &k, "@sld:256", h, Width::Thumb, "b1", &meta(i as u64), h.as_bytes()).expect("put");
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            assert_eq!(slot_files(&doc_dir(&k), "@sld:256", Width::Thumb, "b1").len(), KEEP_PER_SLOT);
            assert_eq!(get(&blobs, &k, "@sld:256", "h1", Width::Thumb, "b1"), Got::Miss, "the oldest went");
            let stale = newest_other(&blobs, &k, "@sld:256", "h5", Width::Thumb, "b1").expect("a stale image");
            assert_eq!(stale.image, b"h4");
            let stale = newest_other(&blobs, &k, "@sld:256", "h4", Width::Thumb, "b1").expect("not the current one");
            assert_eq!(stale.image, b"h3");
            assert_eq!(widths_cached(&k, "@sld:256", "b1"), vec![Width::Thumb]);
            assert!(widths_cached(&k, "@sld:257", "b1").is_empty());
        });
    }

    /// Four of the five hooks' file side: drop a doc, drop a space, move a
    /// doc on a rename (and drop it across spaces), the cap.
    #[test]
    fn the_hooks_drop_move_and_prune() {
        crate::test_support::with_temp_home(|| {
            let (_s, blobs) = store();
            let a = key("ws4", "a.pptx");
            let b = key("ws4", "b.pptx");
            put(&blobs, &a, "@sld:256", "h", Width::Thumb, "b1", &meta(1), b"a").expect("put");
            put(&blobs, &a, "@sld:257", "h", Width::Thumb, "b1", &meta(1), b"a").expect("put");
            put(&blobs, &b, "@sld:256", "h", Width::Thumb, "b1", &meta(1), b"b").expect("put");
            assert_eq!(drop_doc(&a), 2);
            assert!(matches!(get(&blobs, &b, "@sld:256", "h", Width::Thumb, "b1"), Got::Hit(_)), "only that doc");
            let moved = key("ws4", "renamed.pptx");
            assert_eq!(move_doc(&b, &moved), 1);
            assert_eq!(get(&blobs, &b, "@sld:256", "h", Width::Thumb, "b1"), Got::Miss, "nothing at the old key");
            assert!(matches!(get(&blobs, &moved, "@sld:256", "h", Width::Thumb, "b1"), Got::Hit(_)), "the previews followed");
            assert_eq!(move_doc(&moved, &key("ws5", "x.pptx")), 1, "across spaces they go");
            assert!(!doc_dir(&key("ws5", "x.pptx")).exists());
            put(&blobs, &a, "@sld:256", "h", Width::Thumb, "b1", &meta(1), b"a").expect("put");
            assert_eq!(drop_space("ws4"), Ok(1));
            assert!(!space_dir("ws4").exists());
            assert_eq!(drop_space("ws4"), Ok(0), "again: nothing to do");
            // The cap: oldest first, to under 90%.
            for i in 0..10u8 {
                put(&blobs, &key("ws6", "big.pptx"), &format!("@sld:{i}"), "h", Width::Full, "b1", &meta(1), &vec![i; 10_000]).expect("put");
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            let (n, bytes) = prune(50_000);
            assert!(n >= 5 && bytes > 0, "{n} {bytes}");
            let left: u64 = walk(&root()).iter().map(|f| f.1).sum();
            assert!(left <= 45_000, "{left}");
            assert_eq!(get(&blobs, &key("ws6", "big.pptx"), "@sld:0", "h", Width::Full, "b1"), Got::Miss, "the oldest went first");
            assert!(matches!(get(&blobs, &key("ws6", "big.pptx"), "@sld:9", "h", Width::Full, "b1"), Got::Hit(_)), "the newest stays");
            assert_eq!(prune(u64::MAX), (0, 0));
        });
    }

    #[test]
    fn odd_space_ids_never_become_paths() {
        assert_eq!(space_dir_name("3f2a-uuid_1"), "3f2a-uuid_1");
        for odd in ["../etc", "a/b", "", "x y"] {
            let n = space_dir_name(odd);
            assert!(n.starts_with('h') && n.len() == 33 && !n.contains('/') && !n.contains('.'), "{odd:?} → {n}");
        }
    }
}
