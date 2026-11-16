//! A **prototype measurement** for version storage (`docs/design/version-storage-r2.md`, the
//! design of record; this file served its first revision): how much storing blob
//! versions as content-defined chunks would store, against today's whole file per version, and
//! how fast a version reads back. No store change: the chunker, the chunk files and the reads
//! are this test's own. Every test here is ignored; tools/perf/measure.sh runs each on a build
//! node, in release (`vs-dir`, `vs-samples` and `zip-shape`), printing counts and sizes only.
//!
//! - `measure_version_dir`: the versions in `VS_DIR` (every regular file of at least
//!   `VS_MIN_FILE` bytes, default 1 MiB, under it, oldest first by modification time, then
//!   name), as a series of one file's versions
//!   (the IFC pack's carrier-storage run leaves one blob per version);
//! - `measure_edited_samples`: each file in `VS_SAMPLES` (a folder of BLEND and PSD samples)
//!   edited `VS_EDITS` times by small synthetic edits (a few in-place byte runs, sometimes an
//!   insert or a delete of a few KiB), every version kept;
//! - `measure_zip_shape`: a synthetic zip the shape of a deck (deflated members are random
//!   bytes to a chunker), one or two members re-deflated per save: whole-file chunking against
//!   one unit per member.
//!
//! Chunking: FastCDC-style (a gear rolling hash with normalized cut points), minimum, average
//! and maximum sizes from `VS_MIN`, `VS_AVG`, `VS_MAX` (KiB; default 16, 64, 256). A chunk's id
//! is a 128-bit SipHash of its bytes (the design keys a SHA-256 HMAC; the bytes stored don't
//! depend on it). A version's manifest is 36 bytes per chunk (a 32-byte id and a length).

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MANIFEST_ENTRY: u64 = 36;

/// A deterministic generator (splitmix64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn fill(&mut self, out: &mut [u8]) {
        for c in out.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            c.copy_from_slice(&v[..c.len()]);
        }
    }
}

/// The chunker's sizes and gear table.
struct Cdc {
    min: usize,
    avg: usize,
    max: usize,
    gear: [u64; 256],
    mask_s: u64,
    mask_l: u64,
}

fn env_kib(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        * 1024
}

impl Cdc {
    fn from_env() -> Cdc {
        Cdc::new(
            env_kib("VS_MIN", 16),
            env_kib("VS_AVG", 64),
            env_kib("VS_MAX", 256),
        )
    }

    fn new(min: usize, avg: usize, max: usize) -> Cdc {
        let mut r = Rng(0x6b32_6d70_6363_6463);
        let mut gear = [0u64; 256];
        for g in gear.iter_mut() {
            *g = r.next();
        }
        let bits = avg.trailing_zeros().max(2);
        // normalized chunking: a harder mask before the average, an easier one after it
        let mask = |b: u32| ((1u64 << b) - 1) << (64 - b);
        Cdc {
            min,
            avg,
            max,
            gear,
            mask_s: mask(bits + 2),
            mask_l: mask(bits - 2),
        }
    }

    /// The length of the chunk at the start of `b`.
    fn cut(&self, b: &[u8]) -> usize {
        if b.len() <= self.min {
            return b.len();
        }
        let end = b.len().min(self.max);
        let mid = end.min(self.avg);
        let mut h = 0u64;
        let mut i = self.min;
        while i < mid {
            h = (h << 1).wrapping_add(self.gear[b[i] as usize]);
            if h & self.mask_s == 0 {
                return i + 1;
            }
            i += 1;
        }
        while i < end {
            h = (h << 1).wrapping_add(self.gear[b[i] as usize]);
            if h & self.mask_l == 0 {
                return i + 1;
            }
            i += 1;
        }
        end
    }

    /// `b`'s chunks, as (offset, length).
    fn chunks(&self, b: &[u8]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < b.len() {
            let n = self.cut(&b[at..]);
            out.push((at, n));
            at += n;
        }
        out
    }
}

fn chunk_id(b: &[u8]) -> u128 {
    let mut a = std::collections::hash_map::DefaultHasher::new();
    let mut c = std::collections::hash_map::DefaultHasher::new();
    (0u8, b).hash(&mut a);
    (1u8, b).hash(&mut c);
    (u128::from(a.finish()) << 64) | u128::from(c.finish())
}

/// A chunk store in a folder (one file per chunk) and each version's manifest.
struct Store {
    dir: PathBuf,
    seen: HashSet<u128>,
    stored: u64,
    manifests: Vec<Vec<(u128, u32)>>,
}

impl Store {
    fn new(dir: PathBuf) -> Store {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Store {
            dir,
            seen: HashSet::new(),
            stored: 0,
            manifests: Vec::new(),
        }
    }

    fn path(&self, id: u128) -> PathBuf {
        self.dir
            .join(format!("{:02x}/{id:032x}", (id >> 120) as u8))
    }

    /// Store a version cut into `units` (offsets and lengths into `b`); the new bytes it stored.
    fn put(&mut self, b: &[u8], units: &[(usize, usize)]) -> u64 {
        let mut new = 0;
        let mut m = Vec::with_capacity(units.len());
        for &(at, n) in units {
            let c = &b[at..at + n];
            let id = chunk_id(c);
            if self.seen.insert(id) {
                let p = self.path(id);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, c).unwrap();
                new += n as u64;
            }
            m.push((id, n as u32));
        }
        new += MANIFEST_ENTRY * m.len() as u64;
        self.stored += new;
        self.manifests.push(m);
        new
    }

    /// Version `v` read back from its chunks.
    fn read(&self, v: usize) -> Vec<u8> {
        let m = &self.manifests[v];
        let mut out = Vec::with_capacity(m.iter().map(|(_, n)| *n as usize).sum());
        for (id, _) in m {
            std::fs::File::open(self.path(*id))
                .unwrap()
                .read_to_end(&mut out)
                .unwrap();
        }
        out
    }
}

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn mb(b: u64) -> f64 {
    b as f64 / 1_048_576.0
}

/// Store `versions` (read through `get`) both ways and print the result for `label`. Each
/// version is also written whole to `whole_dir` (today's shape) for the read comparison.
type Units<'a> = &'a dyn Fn(usize, &[u8]) -> Vec<(usize, usize)>;

fn measure(label: &str, n: usize, get: &mut dyn FnMut(usize) -> Vec<u8>, units: Units<'_>) {
    let base = std::env::temp_dir().join(format!("k2mb-vs-{}-{label}", std::process::id()));
    let mut store = Store::new(base.join("chunks"));
    let whole_dir = base.join("whole");
    std::fs::create_dir_all(&whole_dir).unwrap();
    let (mut today, mut per, mut chunks) = (0u64, Vec::new(), 0usize);
    let (mut first, mut size) = (0u64, 0u64);
    let mut cut_s = 0.0;
    for v in 0..n {
        let b = get(v);
        today += b.len() as u64;
        size = b.len() as u64;
        std::fs::write(whole_dir.join(format!("{v}")), &b).unwrap();
        let t = Instant::now();
        let u = units(v, &b);
        cut_s += t.elapsed().as_secs_f64();
        chunks += u.len();
        let new = store.put(&b, &u);
        if v == 0 {
            first = new;
        } else {
            per.push(new as f64);
        }
    }
    // reads: a sample of versions, whole file vs chunks (warm, both), checked byte for byte
    let sample: Vec<usize> = [0, n / 4, n / 2, (3 * n) / 4, n - 1]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let (mut whole_ms, mut chunk_ms) = (Vec::new(), Vec::new());
    for &v in &sample {
        for _ in 0..3 {
            let t = Instant::now();
            let w = std::fs::read(whole_dir.join(format!("{v}"))).unwrap();
            whole_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let c = store.read(v);
            chunk_ms.push(t.elapsed().as_secs_f64() * 1e3);
            assert!(w == c, "{label}: version {v} reads back other bytes");
        }
    }
    println!(
        "VS {label}: versions={n} file_mb={:.1} today_mb={:.1} chunked_mb={:.1} ratio={:.4} \
         first_version_mb={:.1} per_version_kib p50={:.1} p90={:.1} max={:.1} \
         chunks_per_version={:.0} cut_mb_per_s={:.0}",
        mb(size),
        mb(today),
        mb(store.stored),
        store.stored as f64 / today.max(1) as f64,
        mb(first),
        pct(per.clone(), 0.5) / 1024.0,
        pct(per.clone(), 0.9) / 1024.0,
        pct(per, 1.0) / 1024.0,
        chunks as f64 / n.max(1) as f64,
        mb(today) / cut_s.max(1e-9),
    );
    println!(
        "VS {label}: file_at ms whole p50={:.2} max={:.2} chunked p50={:.2} max={:.2} (warm \
         page cache, {} reads each, verified equal)",
        pct(whole_ms.clone(), 0.5),
        pct(whole_ms, 1.0),
        pct(chunk_ms.clone(), 0.5),
        pct(chunk_ms, 1.0),
        sample.len() * 3
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The smallest file `measure_version_dir` takes (`VS_MIN_FILE`, bytes; default 1 MiB).
fn min_file() -> u64 {
    std::env::var("VS_MIN_FILE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1 << 20)
}

/// Regular files of at least `VS_MIN_FILE` bytes (default 1 MiB) under `dir`, oldest first, then
/// by name.
fn version_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else if m.len() >= min_file() && e.path().extension().map_or(true, |x| x != "redb") {
                out.push((m.modified().unwrap(), e.path()));
            }
        }
    }
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

#[test]
#[ignore = "a measurement: run by tools/perf/measure.sh (release, on a node)"]
fn measure_version_dir() {
    let dir = PathBuf::from(std::env::var("VS_DIR").expect("VS_DIR: a folder of versions"));
    let files = version_files(&dir);
    // counts and sizes only: never a path, a file's name or its content
    assert!(!files.is_empty(), "no version files under VS_DIR");
    let cdc = Cdc::from_env();
    let label = std::env::var("VS_LABEL").unwrap_or_else(|_| "dir".into());
    measure(
        &label,
        files.len(),
        &mut |v| std::fs::read(&files[v]).unwrap(),
        &|_, b| cdc.chunks(b),
    );
}

/// `b` edited: one to three in-place runs of 64 B to 4 KiB, and one time in four an insert or
/// a delete of 1 to 8 KiB.
fn edit(b: &mut Vec<u8>, r: &mut Rng) {
    for _ in 0..1 + r.below(3) {
        let n = (64 + r.below(4032)) as usize;
        if b.len() > n {
            let at = r.below((b.len() - n) as u64) as usize;
            r.fill(&mut b[at..at + n]);
        }
    }
    if r.below(4) == 0 {
        let n = (1024 + r.below(7 * 1024)) as usize;
        let at = r.below(b.len() as u64) as usize;
        if r.below(2) == 0 {
            let mut ins = vec![0u8; n];
            r.fill(&mut ins);
            b.splice(at..at, ins);
        } else {
            let end = (at + n).min(b.len());
            b.drain(at..end);
        }
    }
}

#[test]
#[ignore = "a measurement: run by tools/perf/measure.sh (release, on a node)"]
fn measure_edited_samples() {
    let dir = PathBuf::from(std::env::var("VS_SAMPLES").expect("VS_SAMPLES: a folder of samples"));
    let edits: usize = std::env::var("VS_EDITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let cdc = Cdc::from_env();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    // counts and sizes only: never a path, a file's name or its content
    for (k, f) in files.iter().enumerate() {
        let mut cur = std::fs::read(f).unwrap();
        let name = format!("sample{}-{}b", k + 1, cur.len());
        let mut r = Rng(0x5eed ^ cur.len() as u64);
        let mut versions = vec![cur.clone()];
        for _ in 0..edits {
            edit(&mut cur, &mut r);
            versions.push(cur.clone());
        }
        measure(
            &name,
            versions.len(),
            &mut |v| versions[v].clone(),
            &|_, b| cdc.chunks(b),
        );
    }
}

/// A zip-shaped file's bytes and each member's (offset, length) in it.
type Zip = (Vec<u8>, Vec<(usize, usize)>);

/// A zip-shaped file: `members` (name, bytes as deflate left them) with local headers and a
/// central directory (no real deflate: a deflated member is random bytes to a chunker), and
/// each member's (offset, length) in it, headers included.
fn zip_of(members: &[(String, Vec<u8>)]) -> Zip {
    let mut out = Vec::new();
    let mut at = Vec::new();
    let mut cd = Vec::new();
    for (name, data) in members {
        let off = out.len();
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&[20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&(chunk_id(data) as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&((data.len() * 3) as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        at.push((off, out.len() - off));
        cd.extend_from_slice(b"PK\x01\x02");
        cd.extend_from_slice(&(off as u32).to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_at = out.len();
    out.extend_from_slice(&cd);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&(cd_at as u32).to_le_bytes());
    at.push((cd_at, out.len() - cd_at));
    (out, at)
}

#[test]
#[ignore = "a measurement: run by tools/perf/measure.sh (release, on a node)"]
fn measure_zip_shape() {
    let saves: usize = std::env::var("VS_EDITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let cdc = Cdc::from_env();
    let mut r = Rng(0x7a19);
    // a deck: 60 slide parts of 2 to 30 KiB, 20 other XML parts, 12 media of 100 KiB to 2 MiB
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    let part = |name: String, lo: u64, hi: u64, r: &mut Rng| {
        let mut b = vec![0u8; (lo + r.below(hi - lo)) as usize];
        r.fill(&mut b);
        (name, b)
    };
    for i in 0..60 {
        members.push(part(
            format!("ppt/slides/slide{i}.xml"),
            2 << 10,
            30 << 10,
            &mut r,
        ));
    }
    for i in 0..20 {
        members.push(part(format!("ppt/other{i}.xml"), 1 << 10, 20 << 10, &mut r));
    }
    for i in 0..12 {
        members.push(part(
            format!("ppt/media/image{i}.png"),
            100 << 10,
            2 << 20,
            &mut r,
        ));
    }
    let mut versions: Vec<Vec<(String, Vec<u8>)>> = vec![members.clone()];
    for _ in 0..saves {
        // a save re-deflates the one or two slides it changed (new bytes, about the same size)
        for _ in 0..1 + r.below(2) {
            let i = r.below(60) as usize;
            let n = members[i].1.len() as u64;
            let n = (n * 9 / 10 + r.below(n / 5 + 1)) as usize;
            let mut b = vec![0u8; n];
            r.fill(&mut b);
            members[i].1 = b;
        }
        versions.push(members.clone());
    }
    let built: Vec<Zip> = versions.iter().map(|m| zip_of(m)).collect();
    drop(versions);
    measure(
        "zip-whole-file-cdc",
        built.len(),
        &mut |v| built[v].0.clone(),
        &|_, b| cdc.chunks(b),
    );
    // one unit per member (a large member cut further), boundaries from the zip's own entries
    let units_of = |v: usize| -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for &(at, n) in &built[v].1 {
            for (o, m) in cdc.chunks(&built[v].0[at..at + n]) {
                out.push((at + o, m));
            }
        }
        out
    };
    measure(
        "zip-per-member",
        built.len(),
        &mut |v| built[v].0.clone(),
        &|v, _| units_of(v),
    );
    let _ = std::io::stdout().flush();
}
