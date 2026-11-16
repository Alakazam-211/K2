//! The wall time of a chunked write from a connection without the history right
//! (`docs/design/version-storage-r2.md`, section 6; `docs/SESSIONS.md`, "Chunked versions"):
//! a write whose only difference from the current version is a chunk only an older version
//! holds, against one whose differing chunk is brand-new. **Published, never asserted**
//! (`docs/evidence/chunked-write-timing.md`): per class p10, p50, p90 and p99, and the two
//! samples' Kolmogorov-Smirnov D with its asymptotic p-value. Ignored by default; run in a
//! release build (no debug assertions) on a build node:
//!
//! ```text
//! cargo test --release -p k2-mpc-base --test chunked_timing -- --ignored --nocapture
//! ```
//!
//! `MPC_TIMING_WRITES` sets the writes per class (default 2,000); the two classes run
//! interleaved in a random order (a fixed seed, `MPC_TIMING_SEED` to change it).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::take_chunk_work;
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// splitmix64.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

fn data(seed: u64, n: usize) -> Vec<u8> {
    let mut r = Rng(seed);
    (0..n).map(|_| (r.next() >> 56) as u8).collect()
}

/// `b` with 64 bytes at `at` overwritten by `seed`'s (the length kept).
fn patched(b: &[u8], at: usize, seed: u64) -> Vec<u8> {
    let mut out = b.to_vec();
    out[at..at + 64].copy_from_slice(&data(seed, 64));
    out
}

/// The region whose chunk differs, and a second one far from it.
const AT_R: usize = 512 << 10;
const AT_S: usize = 1200 << 10;
const SIZE: usize = 3 << 19;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    HistoryOnly,
    BrandNew,
}

/// The nearest-rank `q` quantile of sorted `v`.
fn quantile(v: &[Duration], q: f64) -> Duration {
    let i = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[i]
}

/// The two-sample Kolmogorov-Smirnov statistic of sorted `a` and `b`.
fn ks_d(a: &[Duration], b: &[Duration]) -> f64 {
    let (mut i, mut j, mut d) = (0, 0, 0.0f64);
    let (n, m) = (a.len() as f64, b.len() as f64);
    while i < a.len() && j < b.len() {
        let x = a[i].min(b[j]);
        while i < a.len() && a[i] <= x {
            i += 1;
        }
        while j < b.len() && b[j] <= x {
            j += 1;
        }
        d = d.max((i as f64 / n - j as f64 / m).abs());
    }
    d
}

/// The asymptotic p-value of a two-sample D (Numerical Recipes' `probks` with Stephens'
/// correction for the effective size).
fn ks_p(d: f64, n: usize, m: usize) -> f64 {
    let ne = (n * m) as f64 / (n + m) as f64;
    let lam = (ne.sqrt() + 0.12 + 0.11 / ne.sqrt()) * d;
    let a2 = -2.0 * lam * lam;
    let (mut fac, mut sum, mut before) = (2.0f64, 0.0f64, 0.0f64);
    for j in 1..=100 {
        let term = fac * (a2 * (j * j) as f64).exp();
        sum += term;
        if term.abs() <= 0.001 * before || term.abs() <= 1e-8 * sum {
            return sum.clamp(0.0, 1.0);
        }
        fac = -fac;
        before = term.abs();
    }
    1.0
}

fn ms(d: Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1e3)
}

#[test]
#[ignore = "a measurement: run by tools/perf/measure.sh (release, on a node)"]
fn a_history_only_chunk_against_a_brand_new_one_wall_time() {
    let n: usize = std::env::var("MPC_TIMING_WRITES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let seed: u64 = std::env::var("MPC_TIMING_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5eed_c0de);
    let dir = std::env::temp_dir().join(format!("k2mb-chunk-timing-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dir = TmpDir(dir);
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        rights: Mutex::new(HashMap::new()),
    });
    {
        let mut r = host.rights.lock().unwrap();
        r.insert("alice".into(), Rights::full());
        r.insert("bob".into(), Rights::full().with_history(false));
    }
    let store = Arc::new(DocStore::open(dir.0.join("docs.redb")).unwrap());
    let mut bc = BlobConfig::default();
    bc.chunked_versions = true;
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.0.join("blobs"),
            bc,
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    let key = DocKey {
        space: "ws".into(),
        path: "big.bin".into(),
    };
    let base = data(seed, SIZE);
    let s = mgr
        .open_blob(Arc::new(AnyFile), &key, Some(&mut &base[..]))
        .unwrap();
    let join = |pid: &str| {
        s.join(
            Participant::new(pid, pid, "#123456", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let (alice, bob) = (join("alice"), join("bob"));
    let mut rng = Rng(seed ^ 0xa5a5);
    // the history: version k holds chunk X_k at AT_R (and its own edit at AT_S, so no write
    // below equals it)
    let xs: Vec<u64> = (0..n).map(|_| rng.next()).collect();
    for x in &xs {
        let v = patched(&patched(&base, AT_R, *x), AT_S, rng.next());
        let cur = s.status().unwrap().version;
        s.write_bytes(alice, &v, cur).unwrap();
    }
    // the current version: AT_S as the base has it, so each write below differs from the
    // version before it at AT_R only
    let cur = s.status().unwrap().version;
    s.write_bytes(bob, &patched(&base, AT_R, rng.next()), cur)
        .unwrap();

    let mut order: Vec<Class> = (0..n)
        .map(|_| Class::HistoryOnly)
        .chain((0..n).map(|_| Class::BrandNew))
        .collect();
    for i in (1..order.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let mut times: [Vec<Duration>; 2] = [Vec::new(), Vec::new()];
    let mut bytes = [0u64; 2];
    let mut next_x = xs.iter();
    for class in order {
        let r = match class {
            Class::HistoryOnly => *next_x.next().expect("one X per write"),
            Class::BrandNew => rng.next(),
        };
        let w = patched(&base, AT_R, r);
        let cur = s.status().unwrap().version;
        take_chunk_work();
        let t = Instant::now();
        let out = s.write_bytes(bob, &w, cur).unwrap();
        let took = t.elapsed();
        assert!(out.changed, "every write records a version");
        let k = (class == Class::BrandNew) as usize;
        times[k].push(took);
        bytes[k] += take_chunk_work().segment_bytes;
    }
    for t in &mut times {
        t.sort();
    }
    println!(
        "chunked-write timing: {n} writes per class, interleaved (seed {seed:#x}), a {SIZE}-byte \
         file, debug assertions {}",
        if cfg!(debug_assertions) { "on" } else { "off" }
    );
    for (k, name) in [(0, "history-only chunk"), (1, "brand-new chunk")] {
        let t = &times[k];
        println!(
            "class {name}: p10 {}, p50 {}, p90 {}, p99 {}; segment bytes per write {}",
            ms(quantile(t, 0.10)),
            ms(quantile(t, 0.50)),
            ms(quantile(t, 0.90)),
            ms(quantile(t, 0.99)),
            bytes[k] / t.len() as u64
        );
    }
    let d = ks_d(&times[0], &times[1]);
    println!(
        "two-sample Kolmogorov-Smirnov: D {d:.4}, p {:.4}",
        ks_p(d, times[0].len(), times[1].len())
    );
    mgr.release().unwrap();
}

/// The statistics' own checks (these do run): the quantiles, D and its p-value on known
/// samples.
#[test]
fn the_timing_statistics_are_right() {
    let us = |v: &[u64]| -> Vec<Duration> { v.iter().map(|x| Duration::from_micros(*x)).collect() };
    let a = us(&(1..=100).collect::<Vec<_>>());
    assert_eq!(quantile(&a, 0.10), Duration::from_micros(10));
    assert_eq!(quantile(&a, 0.99), Duration::from_micros(99));
    assert_eq!(ks_d(&a, &a), 0.0);
    assert!((ks_p(0.0, 100, 100) - 1.0).abs() < 1e-9);
    // disjoint samples: D is 1 and p is tiny
    let b = us(&(101..=200).collect::<Vec<_>>());
    assert_eq!(ks_d(&a, &b), 1.0);
    assert!(ks_p(1.0, 100, 100) < 1e-10);
    // a shift of 10 of 100: D is 0.1, p about 0.70 (two-sided, asymptotic)
    let c = us(&(11..=110).collect::<Vec<_>>());
    assert!((ks_d(&a, &c) - 0.1).abs() < 1e-12);
    let p = ks_p(0.1, 100, 100);
    assert!((0.6..0.8).contains(&p), "p {p}");
}
