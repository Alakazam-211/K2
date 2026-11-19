//! The store's cache size ([`StoreSettings`]): every database of a store is opened with it (the
//! store's open, the open with a wait, a rewrite's fresh file), a size out of range is refused
//! with the range, and `capabilities` lists what the store runs with.
//!
//! The size is read through what the store library does with it: its count of evictions (the
//! library's `cache_metrics`, on in test builds only through a dev-dependency). A store holding
//! about three times the smallest cache, read twice, evicts at the smallest cache and never at
//! the largest; each row's control arm shows the count tells the two apart.
//!
//! Plant: `MPC_STORE_PLANT=store-cache-ignored` (the settings not applied: the library's
//! default 1 GiB, as before) fails `the_cache_size_is_applied`,
//! `a_rewrite_keeps_the_cache_size` and `an_open_with_a_wait_applies_the_cache_size`, each at its
//! eviction assert.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::*;

/// The host rows a store is filled with: 48 values of 1 MiB, three times the smallest cache.
const ROWS: usize = 48;
const ROW_BYTES: usize = 1 << 20;

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "k2mb-cache-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }

    fn db(&self) -> PathBuf {
        self.0.join("docs.redb")
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn row(i: usize) -> Vec<u8> {
    (0..ROW_BYTES).map(|j| (i * 31 + j) as u8).collect()
}

/// A store at `t` holding [`ROWS`] host rows of [`ROW_BYTES`], closed.
fn fill(t: &TmpDir) {
    let s = DocStore::open(t.db()).unwrap();
    for i in 0..ROWS {
        s.host_put("cache", &format!("r{i:02}"), &row(i)).unwrap();
    }
}

/// Read every host row twice; each must be what was written.
fn read_all_twice(s: &DocStore) {
    for _ in 0..2 {
        for i in 0..ROWS {
            let v = s.host_get("cache", &format!("r{i:02}")).unwrap();
            assert!(
                v.as_deref() == Some(&row(i)[..]),
                "control: row {i} reads back"
            );
        }
    }
}

/// The store library's evictions since `s`'s database was opened.
fn evictions(s: &DocStore) -> u64 {
    s.guard().cache_stats().evictions()
}

fn least() -> StoreSettings {
    StoreSettings::new(StoreSettings::CACHE_LEAST).unwrap()
}

fn most() -> StoreSettings {
    StoreSettings::new(StoreSettings::CACHE_MOST).unwrap()
}

/// A store opened with the smallest cache evicts while reading more than it holds; opened with
/// the largest, the same reads evict nothing (the control: the count tells the sizes apart).
/// `store-cache-ignored` fails at the first assert.
#[test]
fn the_cache_size_is_applied() {
    let t = TmpDir::new("applied");
    fill(&t);
    let small = DocStore::open_with(t.db(), least()).unwrap();
    assert_eq!(
        small.settings(),
        least(),
        "row: the store keeps its settings"
    );
    read_all_twice(&small);
    let n = evictions(&small);
    assert!(
        n > 0,
        "row: the smallest cache evicts while reading three times its size (evictions {n})"
    );
    drop(small);
    let large = DocStore::open_with(t.db(), most()).unwrap();
    read_all_twice(&large);
    assert_eq!(
        evictions(&large),
        0,
        "control: the largest cache holds the same reads"
    );
    println!("CACHE applied evictions_at_least={n}");
}

/// A rewrite's fresh file, which replaces the store's database, is opened with the store's own
/// settings: read after the rewrite, the smallest cache still evicts.
#[test]
fn a_rewrite_keeps_the_cache_size() {
    let t = TmpDir::new("rewrite");
    fill(&t);
    let s = DocStore::open_with(t.db(), least()).unwrap();
    let rep = s.rewrite_file().unwrap();
    assert!(
        rep.rows >= ROWS as u64,
        "control: the rewrite copied the rows"
    );
    // the copy's own writes may already have evicted: only the reads after it count
    let before = evictions(&s);
    read_all_twice(&s);
    let n = evictions(&s) - before;
    assert!(
        n > 0,
        "row: after a rewrite the store still runs with its cache size (evictions {n})"
    );
    assert_eq!(s.settings(), least(), "row: the settings are kept");
}

/// An open with a wait applies the settings as an open does; a plain open uses the default.
#[test]
fn an_open_with_a_wait_applies_the_cache_size() {
    let t = TmpDir::new("retry");
    fill(&t);
    let s = DocStore::open_with_retry_with(t.db(), Duration::from_secs(5), least()).unwrap();
    read_all_twice(&s);
    let n = evictions(&s);
    assert!(
        n > 0,
        "row: an open with a wait runs with the smallest cache (evictions {n})"
    );
    drop(s);
    let s = DocStore::open(t.db()).unwrap();
    assert_eq!(
        s.settings(),
        StoreSettings::DEFAULT,
        "row: a plain open uses the default"
    );
    assert_eq!(
        StoreSettings::DEFAULT.cache_bytes(),
        64 << 20,
        "row: the default is 64 MiB"
    );
    drop(s);
    let s = DocStore::open_with_retry(t.db(), Duration::from_secs(5)).unwrap();
    assert_eq!(
        s.settings(),
        StoreSettings::DEFAULT,
        "row: an open with a wait and no settings uses the default"
    );
}

/// A size outside the range is refused with the size and the range; the bounds are taken.
#[test]
fn a_cache_size_out_of_range_is_refused() {
    let (least, most) = (StoreSettings::CACHE_LEAST, StoreSettings::CACHE_MOST);
    assert_eq!(least, 16 << 20, "row: the least is 16 MiB");
    assert_eq!(most, 1 << 30, "row: the most is 1 GiB");
    let mut rows = 0;
    for bytes in [0, 1, least - 1, most + 1, u64::MAX] {
        assert_eq!(
            StoreSettings::new(bytes),
            Err(StoreSettingsError::CacheOutOfRange { bytes, least, most }),
            "row: {bytes} is refused with the range"
        );
        rows += 1;
    }
    for bytes in [least, 64 << 20, most] {
        let s = StoreSettings::new(bytes);
        assert_eq!(
            s.map(|s| s.cache_bytes()),
            Ok(bytes),
            "row: {bytes} is taken"
        );
        rows += 1;
    }
    let msg = StoreSettings::new(1).unwrap_err().to_string();
    assert_eq!(
        msg, "the store's cache (1 bytes) must be from 16777216 to 1073741824 bytes",
        "row: the refusal names the range"
    );
    println!("CACHE refusal rows={rows}");
}

/// `capabilities` lists the store's cache size; a manager's config reports what its store was
/// opened with, whatever the config it was given said.
#[test]
fn capabilities_list_the_store_cache_size() {
    use crate::session::{AuditEvent, Config, Host, Participant, Rights, SessionManager};

    struct H;
    impl Host for H {
        fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
            Rights::full()
        }
        fn now_ms(&self) -> u64 {
            0
        }
        fn on_event(&self, _: &AuditEvent) {}
    }

    let listed = Config::default().limits();
    assert!(
        listed.contains(&("store_cache_bytes", 64 << 20)),
        "row: store_cache_bytes listed at its default"
    );
    let c = Config {
        store: StoreSettings::new(32 << 20).unwrap(),
        ..Config::default()
    };
    assert!(
        c.limits().contains(&("store_cache_bytes", 32 << 20)),
        "row: a host's size is listed"
    );
    let caps = crate::errors::capabilities(&[], &c);
    let in_limits = match caps.get("limits") {
        Some(crate::errors::Value::Obj(m)) => m
            .iter()
            .find(|(k, _)| k == "store_cache_bytes")
            .map(|(_, v)| v.clone()),
        _ => None,
    };
    assert_eq!(
        in_limits,
        Some(crate::errors::Value::from(32u64 << 20)),
        "row: capabilities print it under limits"
    );

    let t = TmpDir::new("caps");
    let store = Arc::new(DocStore::open_with(t.db(), least()).unwrap());
    let mgr = SessionManager::new(store.clone(), Arc::new(H), c);
    assert_eq!(
        mgr.config().store,
        least(),
        "row: the manager reports the store's settings, not the config's"
    );
    mgr.release().unwrap();
}
