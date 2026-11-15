//! Chunked tier-0 versions (`docs/design/version-storage-r2.md`; `docs/SESSIONS.md`, "Chunked
//! versions"), as a host uses them through `BlobConfig::chunked_versions`, on real temp folders
//! with the host's clock: off by default, three versions of a large file read back byte for
//! byte from small segments, the stamp, restarts, `gc` beside a write, a stop after each write
//! step, the fail-closed commit, reads beside an unlink and over a damaged segment, the equal
//! work of a write equal to an older version, retention (beside a write, too), the structure
//! index, doc and space deletes, a busy doc lock, and the purge: its byte scan, a stop after
//! its commit, duplicate copies and dead ranges compacted away, `gc`, a write and reads beside
//! its compaction, what it reports shared, and its closing sweep. Then the cheap set from the
//! writing connection's rights: equal counted work and replies for a writer without history,
//! a base it can't read, a history writer's restore, and a right dropped. Then `rechunk` of a
//! doc's whole-file versions: converted with their file ids, refused with the setting off, a
//! stop after each step, and beside a write. Then a pack's cut hints, from zips written by hand
//! (stored members, the central directory read for their boundaries): more chunks shared after a
//! one-byte change, 100,000 tiny members, hints merged up to the minimum, bad hints ignored, a
//! pure cut never over the maximum, and a hinted version read through a pack with none.
//!
//! Plants (`MPC_CHUNK_PLANT`), each caught by its row after a passing control step:
//! `gc-ignores-waiting-segment`, `commit-skips-chunk-check`, `reader-skips-chunk-check`,
//! `cheap-path-beyond-current`, `reader-opens-lazily`, `retention-ignores-chunk-marks`,
//! `delete-space-leaves-segments`, `purge-skips-compaction`,
//! `removal-list-not-finished-at-open`, `compaction-skips-dead-ranges`,
//! `compaction-output-not-waiting`, `history-chunk-cheap`, `manifest-reuse-skips-full-write`,
//! `cheap-set-ignores-history-right`, `rechunk-keeps-old-blob`, `hints-required`,
//! `hints-unmerged`. `purge-skips-doc-lock` is planted by its row itself.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

mod index_pack;
use index_pack::IxPack;

use k2_mpc_base::blob::{BlobConfig, BlobId, BlobStore, ChunkReport, RechunkError, RechunkReport};
use k2_mpc_base::differential::{
    take_chunk_work, with_chunk_hook, with_chunk_plant, ChunkPlant, ChunkStep, ChunkWork,
};
use k2_mpc_base::index::IndexStatus;
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobPack, BlobSession, BlobWriteResult, Config, ConnId, Host, Need,
    NotReached, Participant, ParticipantKind, Rights, SessionError, SessionManager, Stay,
};
use k2_mpc_base::store::{
    scan_file, DocId, DocKey, DocStore, StoreError, FEATURE_CHUNKED, STORE_FORMAT_BASE,
};

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    /// Every event the kit sent the host, in order.
    events: Mutex<Vec<AuditEvent>>,
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
    fn on_event(&self, ev: &AuditEvent) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    host: Arc<TestHost>,
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
    bc: BlobConfig,
}

const SPACE: &str = "ws";
const MIB: usize = 1 << 20;

fn config(on: bool) -> BlobConfig {
    let mut bc = BlobConfig::default();
    bc.chunked_versions = on;
    bc
}

fn open_at(dir: &std::path::Path, host: &Arc<TestHost>, bc: &BlobConfig) -> Fx2 {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            bc.clone(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    (store, blobs, mgr)
}

type Fx2 = (Arc<DocStore>, Arc<BlobStore>, SessionManager);

fn fx_with(tag: &str, bc: BlobConfig) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-chunk-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    host.rights
        .lock()
        .unwrap()
        .insert("alice".into(), Rights::full().with_owner(true));
    let (store, blobs, mgr) = open_at(&p, &host, &bc);
    Fx {
        dir: TmpDir(p),
        host,
        store,
        blobs,
        mgr,
        bc,
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, config(true))
}

/// The host restarts: every handle goes, and the store file and blob folder are opened again.
fn restart(f: Fx) -> Fx {
    let Fx {
        dir,
        host,
        store,
        blobs,
        mgr,
        bc,
    } = f;
    mgr.release().unwrap();
    drop(mgr);
    drop(blobs);
    assert_eq!(Arc::strong_count(&store), 1, "nothing else holds the store");
    drop(store);
    let (store, blobs, mgr) = open_at(&dir.0, &host, &bc);
    Fx {
        dir,
        host,
        store,
        blobs,
        mgr,
        bc,
    }
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: SPACE.into(),
        path: p.into(),
    }
}

/// Deterministic bytes (splitmix64 from `seed`).
fn data(seed: u64, n: usize) -> Vec<u8> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            (z >> 56) as u8
        })
        .collect()
}

/// `b` with a small edit at `at`: 64 bytes overwritten and 37 inserted.
fn edited(b: &[u8], at: usize, seed: u8) -> Vec<u8> {
    let mut out = b.to_vec();
    for (i, x) in out[at..at + 64].iter_mut().enumerate() {
        *x = x.wrapping_add(seed).wrapping_add(i as u8) | 1;
    }
    out.splice(at + 64..at + 64, std::iter::repeat(seed).take(37));
    out
}

/// A doc at `path` imported from `bytes` (its first version), with Alice joined.
struct Doc {
    s: Arc<BlobSession>,
    c: ConnId,
    id: DocId,
    key: DocKey,
}

impl Fx {
    fn import(&self, path: &str, bytes: &[u8]) -> Doc {
        self.import_with(Arc::new(AnyFile), path, bytes)
    }

    /// [`Fx::import`] under `pack`.
    fn import_with(&self, pack: Arc<dyn BlobPack>, path: &str, bytes: &[u8]) -> Doc {
        let k = key(path);
        let s = self.mgr.open_blob(pack, &k, Some(&mut &bytes[..])).unwrap();
        let c = s
            .join(
                Participant::new("alice", "Alice", "#123456", ParticipantKind::Person, "cli"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        let id = self.store.find(&k).unwrap().unwrap();
        Doc { s, c, id, key: k }
    }

    /// Reopen the doc at `path` after a restart.
    fn reopen(&self, path: &str) -> Doc {
        self.import(path, b"")
    }

    fn report(&self, d: &Doc) -> ChunkReport {
        self.blobs.chunk_report(SPACE, d.id).unwrap()
    }

    fn id_of(&self, bytes: &[u8]) -> BlobId {
        self.blobs.id_of(SPACE, &mut &bytes[..]).unwrap().0
    }

    /// A connection to `d` for `pid`, whose rights the host gives as `rights` from now on.
    fn join_as(&self, d: &Doc, pid: &str, rights: Rights) -> ConnId {
        self.host.rights.lock().unwrap().insert(pid.into(), rights);
        d.s.join(
            Participant::new(pid, pid, "#654321", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }

    /// The events the host got since the last call, each with doc `d`'s path written as
    /// `<doc>`, so two docs' events compare.
    fn events_of(&self, d: &Doc) -> Vec<String> {
        std::mem::take(&mut *self.host.events.lock().unwrap())
            .iter()
            .map(|e| format!("{e:?}").replace(&d.key.path, "<doc>"))
            .collect()
    }
}

impl Doc {
    fn write(&self, bytes: &[u8]) -> u64 {
        let base = self.s.status().unwrap().version;
        let r = self.s.write_bytes(self.c, bytes, base).unwrap();
        assert!(r.changed);
        r.version
    }

    fn try_write(&self, bytes: &[u8]) -> Result<u64, SessionError> {
        let base = self.s.status().unwrap().version;
        self.s.write_bytes(self.c, bytes, base).map(|r| r.version)
    }

    fn at(&self, vid: u64) -> Vec<u8> {
        let mut out = Vec::new();
        self.s.file_at_to(self.c, vid, &mut out).unwrap();
        out
    }

    fn now(&self) -> Vec<u8> {
        let mut r = self.s.read(self.c).unwrap();
        let mut out = Vec::new();
        r.reader.read_to_end(&mut out).unwrap();
        out
    }

    fn current(&self) -> u64 {
        self.s.status().unwrap().version
    }

    /// A write of `bytes` by connection `c` on the current version, and the work it did.
    fn write_by(
        &self,
        c: ConnId,
        bytes: &[u8],
    ) -> (Result<BlobWriteResult, SessionError>, ChunkWork) {
        let base = self.current();
        take_chunk_work();
        let out = self.s.write_bytes(c, bytes, base);
        (out, take_chunk_work())
    }
}

/// Run `f` with this row's plant installed when `MPC_CHUNK_PLANT` names it (printed either way,
/// since a job's record shows the variable's name, not its value); a name that is no plant is
/// refused, so a misspelt plant can't pass as a clean run.
fn planted<R>(row: ChunkPlant, f: impl FnOnce() -> R) -> R {
    let v = std::env::var("MPC_CHUNK_PLANT").ok();
    println!("MPC_CHUNK_PLANT={}", v.as_deref().unwrap_or("(unset)"));
    let p = v.as_deref().map(|n| {
        ChunkPlant::named(n).unwrap_or_else(|| panic!("MPC_CHUNK_PLANT={n}: no such plant"))
    });
    match p {
        Some(p) if p == row => with_chunk_plant(p, f),
        _ => f(),
    }
}

/// The segment numbers of a report's files (`<n>.seg`), and of its segment rows.
fn file_segments(r: &ChunkReport) -> BTreeSet<u64> {
    r.files
        .iter()
        .map(|(name, _)| {
            name.strip_suffix(".seg")
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("a segment file named {name}"))
        })
        .collect()
}

fn row_segments(r: &ChunkReport) -> BTreeSet<u64> {
    r.segments.iter().map(|(n, _)| *n).collect()
}

fn file_bytes(r: &ChunkReport, seg: u64) -> u64 {
    r.files
        .iter()
        .find(|(n, _)| *n == format!("{seg}.seg"))
        .map(|(_, b)| *b)
        .unwrap_or_else(|| panic!("no file for segment {seg}: {:?}", r.files))
}

fn temp_files(f: &Fx) -> usize {
    fs::read_dir(f.dir.0.join("blobs").join("tmp"))
        .map(|d| d.count())
        .unwrap_or(0)
}

// ---------- a. off by default ----------

/// Off by default: a 2 MiB write is a whole-file blob, no chunk table exists, and the store's
/// format is the base format with no features word.
#[test]
fn off_by_default_a_large_write_is_a_whole_file_blob() {
    let bc = BlobConfig::default();
    assert!(
        !bc.chunked_versions,
        "row: chunked versions are off by default"
    );
    assert_eq!(bc.chunk_min_bytes, 1 << 20, "row: the minimum is 1 MiB");
    let f = fx_with("off", bc);
    let v1 = data(1, 2 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 3);
    let vid = d.write(&v2);
    assert_eq!(d.at(vid), v2);
    let r = f.report(&d);
    assert!(
        r.tables.is_empty(),
        "row: no chunk table exists: {:?}",
        r.tables
    );
    assert!(r.files.is_empty(), "row: no segment file: {:?}", r.files);
    for b in [&v1, &v2] {
        assert!(
            f.blobs.info(SPACE, &f.id_of(b)).unwrap().is_some(),
            "row: each version is a whole-file blob"
        );
    }
    assert_eq!(
        f.store.format().unwrap(),
        STORE_FORMAT_BASE,
        "row: the format is unchanged"
    );
    assert_eq!(
        f.store.required_features().unwrap(),
        0,
        "row: no features word"
    );
}

// ---------- b. on: three versions, small segments, the stamp ----------

/// On: three versions of a 3 MiB file, each a small edit of the last, read back byte for byte
/// through `file_at_to` (and `read`); each version's file id is the id a whole-file put of the
/// same bytes gives; the store is stamped format 8 with the chunked feature; and the second and
/// third versions each wrote one segment far smaller than the file.
#[test]
fn on_three_versions_of_a_large_file_read_back_from_small_segments() {
    let f = fx("three");
    let v1 = data(2, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, 2 * MIB + 5_000, 7);
    let v3 = edited(&v2, MIB / 2, 9);
    let vids = [1, d.write(&v2), d.write(&v3)];
    for (vid, b) in vids.iter().zip([&v1, &v2, &v3]) {
        assert!(
            d.at(*vid) == *b,
            "row: version {vid} reads back byte for byte"
        );
    }
    assert!(d.now() == v3, "row: the current version reads back");
    let versions = d.s.versions(d.c).unwrap();
    for ((vid, v), b) in versions.iter().zip([&v1, &v2, &v3]) {
        assert_eq!(
            v.blob,
            f.id_of(b),
            "row: version {vid}'s file id is the whole file's"
        );
        assert_eq!(v.size, b.len() as u64);
    }
    assert_eq!(f.store.format().unwrap(), 8, "row: the store is format 8");
    assert_eq!(
        f.store.required_features().unwrap(),
        FEATURE_CHUNKED,
        "row: its features word names chunked"
    );
    let r = f.report(&d);
    assert_eq!(
        r.manifests.len(),
        3,
        "row: one manifest per version: {:?}",
        r.manifests
    );
    for b in [&v1, &v2, &v3] {
        assert!(
            f.blobs.info(SPACE, &f.id_of(b)).unwrap().is_none(),
            "row: the version is stored chunked, not as a whole-file blob"
        );
    }
    let segs = file_segments(&r);
    assert_eq!(segs.len(), 3, "row: one segment per version: {:?}", r.files);
    assert_eq!(
        segs,
        row_segments(&r),
        "row: every segment file has its row"
    );
    let sizes: Vec<u64> = segs.iter().map(|s| file_bytes(&r, *s)).collect();
    println!("segment bytes: {sizes:?} for a {}-byte file", v1.len());
    assert!(
        sizes[0] >= v1.len() as u64,
        "row: the first version is written whole"
    );
    for (i, n) in sizes.iter().enumerate().skip(1) {
        assert!(
            n * 3 < v1.len() as u64,
            "row: version {} wrote {n} bytes for a small edit of a {}-byte file",
            i + 1,
            v1.len()
        );
    }
}

// ---------- c. under the minimum ----------

/// A file under `chunk_min_bytes` stays a whole-file blob with the setting on, and the store
/// isn't stamped.
#[test]
fn under_the_minimum_a_file_stays_whole_with_the_setting_on() {
    let f = fx("small");
    let v1 = data(4, 512 << 10);
    let d = f.import("small.bin", &v1);
    let v2 = edited(&v1, 1_000, 5);
    let vid = d.write(&v2);
    assert_eq!(d.at(vid), v2);
    let r = f.report(&d);
    assert!(
        r.manifests.is_empty() && r.files.is_empty(),
        "row: nothing chunked: {r:?}"
    );
    assert!(
        r.tables.is_empty(),
        "row: no chunk table exists: {:?}",
        r.tables
    );
    assert!(
        f.blobs.info(SPACE, &f.id_of(&v2)).unwrap().is_some(),
        "row: a whole-file blob"
    );
    assert_eq!(
        f.store.format().unwrap(),
        STORE_FORMAT_BASE,
        "row: not stamped"
    );
}

// ---------- d. restart ----------

/// Chunked versions read back after a restart (the store and the blob store opened again).
#[test]
fn chunked_reads_survive_a_restart() {
    let f = fx("restart");
    let v1 = data(5, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 1);
    let vid = d.write(&v2);
    assert_eq!(
        f.report(&d).manifests.len(),
        2,
        "row: both versions are chunked"
    );
    drop(d);
    let f = restart(f);
    let d = f.reopen("big.bin");
    assert!(d.at(1) == v1, "row: version 1 reads back after a restart");
    assert!(d.at(vid) == v2, "row: version 2 reads back after a restart");
    assert_eq!(f.report(&d).manifests.len(), 2);
}

// ---------- e. gc during a write ----------

/// `gc` while a chunked write is between its segment's rename and its commit: the segment is
/// waiting, so `gc` leaves it, the write commits, and every chunk reads back.
/// `MPC_CHUNK_PLANT=gc-ignores-waiting-segment` fails at the planted step.
#[test]
fn gc_during_a_chunked_write_leaves_it_whole() {
    let f = fx("gc-write");
    let v1 = data(6, 3 * MIB);
    let d = f.import("big.bin", &v1);
    // control: gc after a write commits leaves every segment
    let v2 = edited(&v1, MIB, 2);
    let vid2 = d.write(&v2);
    f.blobs.gc().unwrap();
    assert!(d.at(vid2) == v2 && d.at(1) == v1, "control: reads after gc");
    println!("control: gc after a chunked write leaves its segments");

    let v3 = edited(&v2, 2 * MIB, 4);
    let blobs = f.blobs.clone();
    let dir = f.blobs.segment_dir(SPACE, d.id).unwrap();
    let seen = Rc::new(RefCell::new(None::<bool>));
    let s2 = seen.clone();
    let vid3 = planted(ChunkPlant::GcIgnoresWaitingSegment, || {
        with_chunk_hook(
            move |step| {
                if step == ChunkStep::Renamed {
                    let before = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
                    blobs.gc().unwrap();
                    let after = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
                    *s2.borrow_mut() = Some(after == before);
                }
                false
            },
            || d.try_write(&v3),
        )
    });
    let kept = *seen.borrow();
    assert!(
        kept.is_some(),
        "row: gc ran between the segment's rename and the commit"
    );
    assert_eq!(kept, Some(true), "row: gc left the waiting segment");
    let vid3 = vid3.expect("row: the write gc ran beside commits");
    assert!(d.at(vid3) == v3, "row: every chunk of the write reads back");
}

// ---------- f. rowless segments ----------

/// A segment file with no row (planted) goes at the next `gc`, and at `BlobStore::open`; the
/// doc's own segments stay and its versions read.
#[test]
fn a_rowless_segment_goes_at_gc_and_at_open() {
    let f = fx("rowless");
    let v1 = data(7, 2 * MIB);
    let d = f.import("big.bin", &v1);
    let dir = f.blobs.segment_dir(SPACE, d.id).unwrap();
    assert!(
        !f.report(&d).files.is_empty(),
        "row: a chunked version has its segment folder"
    );
    let stray = dir.join("99.seg");
    fs::write(&stray, b"no row names this segment").unwrap();
    f.blobs.gc().unwrap();
    assert!(!stray.exists(), "row: gc removes a rowless segment");
    assert!(d.at(1) == v1, "row: the doc's own segment stays");
    fs::write(&stray, b"no row names this segment").unwrap();
    fs::write(
        f.dir.0.join("blobs").join("tmp").join("1-1.seg.part"),
        b"temp",
    )
    .unwrap();
    drop(d);
    let f = restart(f);
    assert!(!stray.exists(), "row: the open removes a rowless segment");
    assert_eq!(temp_files(&f), 0, "row: the open removes temp files");
    let d = f.reopen("big.bin");
    assert!(d.at(1) == v1, "row: the version reads after the open");
}

// ---------- g. a stop after each step ----------

/// A stop after each step of a chunked write (temp written, renamed, folder synced, commit,
/// marks released): after a restart the version is whole or absent (whole once the commit is
/// reached), and no rowless segment or temp file survives the open.
#[test]
fn a_stop_after_each_write_step_leaves_the_version_whole_or_absent() {
    use ChunkStep::*;
    for (i, stop) in [TempWritten, Renamed, FolderSynced, Committed, MarksReleased]
        .into_iter()
        .enumerate()
    {
        let f = fx(&format!("stop{i}"));
        let v1 = data(8, 3 * MIB);
        let d = f.import("big.bin", &v1);
        let v2 = edited(&v1, MIB + 333, 6);
        let reached = Rc::new(RefCell::new(false));
        let r2 = reached.clone();
        let out = with_chunk_hook(
            move |step| {
                if step == stop {
                    *r2.borrow_mut() = true;
                    return true;
                }
                false
            },
            || d.try_write(&v2),
        );
        assert!(*reached.borrow(), "row: the write reached {stop:?}");
        println!("stop after {stop:?}: {:?}", out.as_ref().map(|_| ()));
        drop(d);
        let f = restart(f);
        let d = f.reopen("big.bin");
        let cur = d.current();
        let durable = matches!(stop, Committed | MarksReleased);
        if durable {
            assert_eq!(cur, 2, "row: a stop after {stop:?} keeps the version");
        }
        match cur {
            1 => assert!(d.now() == v1, "row: absent: the doc reads version 1 whole"),
            2 => assert!(d.now() == v2, "row: kept: the version reads whole"),
            n => panic!("row: version {n} after one write"),
        }
        assert!(
            d.at(1) == v1,
            "row: version 1 reads whole after a stop at {stop:?}"
        );
        let r = f.report(&d);
        assert_eq!(
            file_segments(&r),
            row_segments(&r),
            "row: no rowless segment survives the open (stop at {stop:?})"
        );
        assert_eq!(temp_files(&f), 0, "row: no temp file survives the open");
    }
}

// ---------- h. fail closed ----------

/// Fail closed: a chunk row the write reuses, removed just before the commit (as a remover
/// that ignored the write's marks would), refuses the whole write, and nothing is
/// half-referenced: no manifest or version row, every count as it was.
/// `MPC_CHUNK_PLANT=commit-skips-chunk-check` fails at the planted step.
#[test]
fn a_reused_chunk_row_gone_before_the_commit_refuses_the_write() {
    let f = fx("fail-closed");
    let v1 = data(9, 3 * MIB);
    let d = f.import("big.bin", &v1);
    // control: a write reusing the current version's chunks commits
    let v2 = edited(&v1, 2 * MIB + 77, 3);
    let vid2 = d.write(&v2);
    assert!(d.at(vid2) == v2, "control: the write reads back");
    println!("control: a write reusing the current version's chunks commits");

    let before = f.report(&d);
    // the file's first chunk: shared by every version (the edits are far from it)
    let first = before
        .chunks
        .iter()
        .find(|c| c.1 == 1 && c.2 == 0)
        .map(|c| c.0);
    let v3 = edited(&v2, 2 * MIB + 9_000, 5);
    let store = f.store.clone();
    let doc = d.id;
    let ran = Rc::new(RefCell::new(false));
    let r2 = ran.clone();
    let out = planted(ChunkPlant::CommitSkipsChunkCheck, || {
        with_chunk_hook(
            move |step| {
                if step == ChunkStep::BeforeCommit {
                    if let Some(id) = first {
                        store.remove_chunk_row_for_tests(doc, &id).unwrap();
                        *r2.borrow_mut() = true;
                    }
                }
                false
            },
            || d.try_write(&v3),
        )
    });
    assert!(
        *ran.borrow(),
        "row: the reused chunk's row was removed before the commit"
    );
    let e = out.expect_err("row: the write is refused");
    assert!(
        matches!(e, SessionError::Store(StoreError::ChunkGone)),
        "row: refused as a chunk gone: {e:?}"
    );
    assert_eq!(e.report().id(), "MPC-E501");
    assert_eq!(d.current(), vid2, "row: no version was recorded");
    let after = f.report(&d);
    assert_eq!(
        after.manifests, before.manifests,
        "row: no manifest was made or counted"
    );
    let mut kept = before.chunks.clone();
    kept.retain(|c| Some(c.0) != first);
    assert_eq!(
        after.chunks, kept,
        "row: every other chunk count is as it was"
    );
    assert_eq!(
        after.segments, before.segments,
        "row: no segment row was made"
    );
}

// ---------- i. a reader beside an unlink ----------

/// A reader opened before its segment files are unlinked still reads the version whole: every
/// segment is open before the first byte streams. `MPC_CHUNK_PLANT=reader-opens-lazily` fails
/// at the planted step.
#[test]
fn a_reader_opened_before_its_segments_are_unlinked_reads_whole() {
    let f = fx("unlink");
    let v1 = data(10, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, 2 * MIB + 100_000, 8);
    let vid = d.write(&v2);
    let mut r = d.s.read_version(d.c, vid).unwrap().reader;
    let mut out = Vec::new();
    r.read_to_end(&mut out).unwrap();
    assert!(out == v2, "control: the version reads whole");
    println!("control: a reader reads the version whole");

    let dir = f.blobs.segment_dir(SPACE, d.id).unwrap();
    let got = planted(ChunkPlant::ReaderOpensLazily, || {
        let mut r = d.s.read_version(d.c, vid).unwrap().reader;
        let files: Vec<PathBuf> = fs::read_dir(&dir)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(
            files.len() >= 2,
            "row: the version spans segment files: {files:?}"
        );
        for p in &files {
            fs::remove_file(p).unwrap();
        }
        let mut out = Vec::new();
        r.read_to_end(&mut out).map(|_| out)
    });
    let got = got.expect("row: a reader opened before the unlink reads to its end");
    assert!(got == v2, "row: and reads the version whole");
}

// ---------- j. a damaged segment ----------

/// A flipped byte in a segment fails the read, and no wrong byte is handed out before the
/// error: each chunk is checked against its keyed id before its bytes stream.
/// `MPC_CHUNK_PLANT=reader-skips-chunk-check` fails at the planted step.
#[test]
fn a_flipped_byte_in_a_segment_fails_the_read_with_no_wrong_byte() {
    let f = fx("flip");
    let v1 = data(11, 3 * MIB);
    let d = f.import("big.bin", &v1);
    assert!(d.at(1) == v1, "control: the version reads whole");
    println!("control: the undamaged version reads whole");
    let r = f.report(&d);
    assert_eq!(
        file_segments(&r).len(),
        1,
        "row: the version has one segment to damage"
    );
    let p = f.blobs.segment_dir(SPACE, d.id).unwrap().join("1.seg");
    let mut b = fs::read(&p).unwrap();
    let at = v1.len() / 2;
    b[at] ^= 0x40;
    fs::write(&p, &b).unwrap();
    let (got, err) = planted(ChunkPlant::ReaderSkipsChunkCheck, || {
        let mut r = d.s.read_version(d.c, 1).unwrap().reader;
        let mut got = Vec::new();
        let mut buf = vec![0u8; 50_000];
        loop {
            match r.read(&mut buf) {
                Ok(0) => break (got, None),
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) => break (got, Some(e)),
            }
        }
    });
    assert!(
        err.is_some(),
        "row: a flipped byte fails the read ({} bytes read)",
        got.len()
    );
    assert!(
        got.len() <= at && got[..] == v1[..got.len()],
        "row: no wrong byte was handed out before the error ({} bytes, the flip at {at})",
        got.len()
    );
}

// ---------- k. a write equal to an older version ----------

/// A write whose file equals an older (not the current) version's, from a connection without
/// history: its chunks, and its manifest's own, are written again to the write's segment (its
/// cheap path is the current version's only), the existing manifest's count is 2, and the
/// version reads back.
/// `MPC_CHUNK_PLANT=cheap-path-beyond-current` fails at the planted step.
#[test]
fn a_write_equal_to_an_older_version_writes_its_chunks_again() {
    let f = fx("older");
    let a = data(12, 3 * MIB);
    let c = data(13, 3 * MIB);
    let d = f.import("big.bin", &a);
    let vc = d.write(&c);
    assert!(d.at(vc) == c && d.at(1) == a, "control: both versions read");
    println!("control: two unrelated versions read");

    let bob = f.join_as(&d, "bob", no_history());
    let (o, _) = planted(ChunkPlant::CheapPathBeyondCurrent, || d.write_by(bob, &a));
    let va = o.expect("row: the write lands").version;
    let r = f.report(&d);
    let ida = f.id_of(&a);
    assert_eq!(
        r.manifests.iter().find(|m| m.0 == ida).map(|m| m.1),
        Some(2),
        "row: the existing manifest counts both versions: {:?}",
        r.manifests
    );
    let list = |seg: u64| -> BTreeSet<[u8; 32]> {
        r.segments
            .iter()
            .find(|s| s.0 == seg)
            .map(|s| s.1.iter().map(|e| e.0).collect())
            .unwrap_or_default()
    };
    let newest = *row_segments(&r).iter().max().unwrap();
    assert_ne!(newest, 2, "row: the write made a segment of its own");
    assert_eq!(
        list(newest),
        list(1),
        "row: every chunk of the older version, and its manifest's, is written again"
    );
    assert!(d.at(va) == a, "row: the version reads back");
}

// ---------- l. retention ----------

/// Retention dropping a chunked version lowers its manifest's chunks' counts (each by one),
/// drops its manifest, and the other versions still read, after a `gc` too.
#[test]
fn retention_of_a_chunked_version_lowers_counts_and_keeps_the_others() {
    let f = fx("retain");
    let v1 = data(14, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 2);
    let v3 = edited(&v2, 2 * MIB, 3);
    let vid2 = d.write(&v2);
    let vid3 = d.write(&v3);
    let before = f.report(&d);
    assert_eq!(before.manifests.len(), 3, "row: three chunked versions");
    let first_written = before
        .segments
        .iter()
        .find(|s| s.0 == 1)
        .map_or(0, |s| s.1.len() as u64);
    f.store
        .delete_blob_version(d.id, 1, f.host.now_ms())
        .unwrap();
    let after = f.report(&d);
    let id1 = f.id_of(&v1);
    assert!(
        after.manifests.iter().all(|m| m.0 != id1),
        "row: the dropped version's manifest goes"
    );
    assert_eq!(after.manifests.len(), 2);
    let sum = |r: &ChunkReport| r.chunks.iter().map(|c| c.4).sum::<u64>();
    assert_eq!(
        sum(&before) - sum(&after),
        first_written,
        "row: each of the dropped manifest's chunks loses one reference"
    );
    f.blobs.gc().unwrap();
    assert!(
        d.at(vid2) == v2 && d.at(vid3) == v3,
        "row: the other versions still read"
    );
}

// ---------- m. purge: the byte scan ----------

/// Text written in a purged version only.
const MARKER: &[u8] = b"purged-content-marker-5be1c0d7-written-in-one-version-only";

/// `b` with [`MARKER`] written over its bytes at `at`.
fn marked(b: &[u8], at: usize) -> Vec<u8> {
    let mut out = b.to_vec();
    out[at..at + MARKER.len()].copy_from_slice(MARKER);
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every place `needles` are found, whole or by an 8-byte window: in the store file and in every
/// file under the blob folder (segment files, temp files, whole-file blobs).
fn scan(f: &Fx, needles: &[Vec<u8>]) -> Vec<String> {
    let n: Vec<&[u8]> = needles.iter().map(|v| v.as_slice()).collect();
    let mut out: Vec<String> = f
        .store
        .scan_for(&n)
        .unwrap()
        .iter()
        .map(|x| format!("store file: needle {} at {}", x.needle, x.offset))
        .collect();
    let mut dirs = vec![f.dir.0.join("blobs")];
    while let Some(d) = dirs.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
                continue;
            }
            for x in scan_file(&p, &n).unwrap() {
                out.push(format!(
                    "{}: needle {} at {}",
                    p.display(),
                    x.needle,
                    x.offset
                ));
            }
        }
    }
    out
}

/// The chunk ids a report holds rows for.
fn chunk_ids(r: &ChunkReport) -> BTreeSet<[u8; 32]> {
    r.chunks.iter().map(|c| c.0).collect()
}

/// The needles of a purged version: its file id and every chunk id the purge erased, raw and in
/// hex, and [`MARKER`].
fn purged_needles(file: &BlobId, gone: &BTreeSet<[u8; 32]>) -> Vec<Vec<u8>> {
    let mut n = vec![
        MARKER.to_vec(),
        file.as_bytes().to_vec(),
        file.to_hex().into_bytes(),
    ];
    for id in gone {
        n.push(id.to_vec());
        n.push(hex(id).into_bytes());
    }
    n
}

/// How many entries of the segment lists no chunk row points at: dead ranges.
fn dead_ranges(r: &ChunkReport) -> usize {
    let live: BTreeSet<(u64, u64)> = r.chunks.iter().map(|c| (c.1, c.2)).collect();
    r.segments
        .iter()
        .flat_map(|(seg, list)| list.iter().map(move |e| (*seg, e.1)))
        .filter(|k| !live.contains(k))
        .count()
}

/// Three versions: the first holds [`MARKER`] (in a chunk only it holds), the second is the
/// same bytes without it, the third a small edit of the second.
fn marked_three(f: &Fx, path: &str, seed: u64) -> (Doc, [Vec<u8>; 3]) {
    let base = data(seed, 3 * MIB);
    let v1 = marked(&base, 3 * MIB / 2);
    let d = f.import(path, &v1);
    d.write(&base);
    let v3 = edited(&base, MIB / 2, 4);
    d.write(&v3);
    (d, [v1, base, v3])
}

/// A purge of a chunked version leaves no byte, id or manifest of it: its chunks' rows and
/// bytes, its manifest and its ids are gone from the store file and from every segment file,
/// no segment of the doc keeps a dead range, and the kept versions read whole.
/// `MPC_CHUNK_PLANT=purge-skips-compaction` fails at the planted step.
#[test]
fn a_purge_leaves_no_byte_id_or_manifest_of_the_purged_version() {
    let f = fx("purge-scan");
    let (d, [v1, v2, v3]) = marked_three(&f, "big.bin", 15);
    let before = f.report(&d);
    let file = f.id_of(&v1);
    assert!(
        !scan(&f, &[MARKER.to_vec()]).is_empty(),
        "control: the scan finds the marker before the purge"
    );
    println!("control: the scan finds the purged version's marker before the purge");
    let out = planted(ChunkPlant::PurgeSkipsCompaction, || d.s.purge(d.c, Some(1)));
    out.expect("row: the purge runs");
    let after = f.report(&d);
    let gone: BTreeSet<[u8; 32]> = chunk_ids(&before)
        .difference(&chunk_ids(&after))
        .copied()
        .collect();
    assert!(
        !gone.is_empty(),
        "row: the purge erased chunks only it held"
    );
    let found = scan(&f, &purged_needles(&file, &gone));
    assert!(
        found.is_empty(),
        "row: no byte, id or manifest of the purged version is left: {found:?}"
    );
    assert_eq!(
        dead_ranges(&after),
        0,
        "row: no segment of the doc keeps a dead range"
    );
    assert!(
        d.s.versions(d.c).unwrap()[0].1.redacted,
        "row: the version is redacted"
    );
    assert!(
        d.at(2) == v2 && d.at(3) == v3,
        "row: the kept versions read whole"
    );
}

// ---------- n. doc delete ----------

/// A doc delete removes the doc's chunk rows (with its other rows, in one transaction) and,
/// once it commits, its segment folder.
#[test]
fn a_doc_delete_removes_its_chunk_rows_and_segment_folder() {
    let f = fx("delete");
    let v1 = data(16, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 4);
    d.write(&v2);
    let dir = f.blobs.segment_dir(SPACE, d.id).unwrap();
    let before = f.report(&d);
    assert!(
        !before.manifests.is_empty() && !before.chunks.is_empty() && dir.is_dir(),
        "row: chunk rows and a segment folder before the delete"
    );
    f.mgr.delete_doc(&d.key).unwrap();
    let after = f.blobs.chunk_report(SPACE, d.id).unwrap();
    assert!(
        after.manifests.is_empty() && after.chunks.is_empty() && after.segments.is_empty(),
        "row: the doc's chunk rows go: {after:?}"
    );
    assert!(!dir.exists(), "row: its segment folder goes");
}

// ---------- o. restore ----------

/// A restore of a chunked version reads the old bytes back through the chunked reader and
/// records them as a new version: its manifest counts both, and it reads back.
#[test]
fn a_restore_of_a_chunked_version_reads_back() {
    let f = fx("restore");
    let v1 = data(17, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 4);
    d.write(&v2);
    let r = d.s.restore(d.c, 1).unwrap();
    assert!(r.changed);
    assert!(
        d.at(r.version) == v1,
        "row: the restored version reads back"
    );
    let rep = f.report(&d);
    assert_eq!(
        rep.manifests
            .iter()
            .find(|m| m.0 == f.id_of(&v1))
            .map(|m| m.1),
        Some(2),
        "row: the restored bytes' manifest counts both versions"
    );
}

// ---------- p. retention beside a write ----------

/// Retention between a write's reuse of a chunk and its commit leaves the chunk: the write marks
/// every chunk it names waiting before it reuses it, so retention keeps the row (at no
/// references) and the write's commit counts it. The writer holds the history right, so it
/// reuses a chunk only an older version holds, and retention drops that version. The save lands
/// and reads back whole, after a `gc` too. `MPC_CHUNK_PLANT=retention-ignores-chunk-marks`
/// fails at the planted step.
#[test]
fn retention_beside_a_write_keeps_the_chunks_it_reuses() {
    let f = fx("retain-beside");
    let a = data(20, 3 * MIB);
    let b = edited(&a, MIB, 3);
    // control: from a connection with the history right, a write of version 1's bytes edited
    // elsewhere reuses version 1's chunks (it writes only the few it changed)
    let ctl = f.import("ctl.bin", &a);
    ctl.write(&b);
    let a1 = edited(&a, 2 * MIB + 500, 5);
    let v = ctl.write(&a1);
    let r = f.report(&ctl);
    let newest = *row_segments(&r).iter().max().unwrap();
    assert!(
        file_bytes(&r, newest) * 3 < a.len() as u64,
        "control: the write reused the older version's chunks"
    );
    assert!(ctl.at(v) == a1, "control: and reads back");
    println!("control: a history writer's write reuses an older version's chunks");

    // the chunk around 1 MiB is version 1's only; the write reuses it, and retention drops
    // version 1 just before the write's commit
    let d = f.import("main.bin", &a);
    d.write(&b);
    let a2 = edited(&a, 2 * MIB + 900, 7);
    let store = f.store.clone();
    let (doc, now) = (d.id, f.host.now_ms());
    let ran = Rc::new(RefCell::new(false));
    let r2 = ran.clone();
    let out = planted(ChunkPlant::RetentionIgnoresChunkMarks, || {
        with_chunk_hook(
            move |step| {
                if step == ChunkStep::BeforeCommit && !*r2.borrow() {
                    *r2.borrow_mut() = true;
                    store.delete_blob_version(doc, 1, now).unwrap();
                }
                false
            },
            || d.try_write(&a2),
        )
    });
    assert!(
        *ran.borrow(),
        "row: retention ran between the write's reuse and its commit"
    );
    let vid = out.expect("row: the save lands");
    assert!(d.at(vid) == a2, "row: the saved version reads back whole");
    f.blobs.gc().unwrap();
    assert!(d.at(vid) == a2, "row: and after a gc");
    assert!(d.at(2) == b, "row: the other version still reads");
}

// ---------- q. the structure index of a chunked version ----------

/// The tree the index rows' files hold (the stand-in pack's `IDX` lines).
const TREE: &[&str] = &[
    "@n:1|group|Layers|-|1|-|-",
    "@n:2|text|Title|0|1|Hello marker-TEXT|-|marker-NOTE",
    "@n:3|raster|Photo|0|0|-|pixels-of-photo|-",
];

/// An `IDX` file of [`TREE`] padded past the chunking minimum with `#source` lines (the last one
/// names the source), so it is stored chunked; `seed` makes the padding differ.
fn idx_padded(seed: u64) -> Vec<u8> {
    let noise = data(seed, 2 * MIB / 8);
    let mut s = String::from("IDX\n");
    for (i, w) in noise.chunks(8).enumerate() {
        let hex: String = w.iter().map(|b| format!("{b:02x}")).collect();
        s.push_str(&format!("#source pad-{i}-{hex}\n"));
    }
    s.push_str("#source fmt-2\n");
    for l in TREE {
        s.push_str(l);
        s.push('\n');
    }
    s.into_bytes()
}

/// A view's nodes as text: key, kind, name and parent.
fn nodes_of(v: &k2_mpc_base::index::IndexView) -> Vec<String> {
    v.nodes
        .iter()
        .map(|n| format!("{:?}|{}|{}|{:?}", n.key, n.kind, n.name, n.parent))
        .collect()
}

/// A chunked version's structure index builds and reads like a whole-file one's: the manifest
/// row stands in for the blob row, so the job is due, records, and the view reads back with the
/// same nodes as a whole-file version of the same tree. Dropping the version (retention) drops
/// its index row in the same transaction.
#[test]
fn a_chunked_versions_structure_index_builds_reads_and_goes_with_it() {
    let f = fx("index");
    // control: a whole-file version (under the minimum) of the tree indexes and reads
    let small = f.import_with(
        Arc::new(IxPack::default()),
        "small.idx",
        &index_pack::file(TREE),
    );
    let job = small.s.index_job(None).unwrap().expect("control: a job");
    assert!(matches!(job.run(), Ok(IndexStatus::Ready { .. })));
    let whole = nodes_of(&small.s.index_view(None).unwrap());
    assert_eq!(whole.len(), TREE.len(), "control: the whole-file view");
    println!("control: a whole-file version's index builds and reads");

    let v1 = idx_padded(1);
    let d = f.import_with(Arc::new(IxPack::default()), "big.idx", &v1);
    assert_eq!(
        f.report(&d).manifests.len(),
        1,
        "row: the version is stored chunked"
    );
    let job = d.s.index_job(None).unwrap();
    let job = job.expect("row: a job is due for a chunked version");
    assert_eq!(
        job.run().expect("row: the job records"),
        IndexStatus::Ready {
            version: 1,
            nodes: TREE.len() as u32
        },
        "row: the chunked version's index is ready"
    );
    let view = d.s.index_view(None).expect("row: the index reads back");
    assert_eq!(
        nodes_of(&view),
        whole,
        "row: with the whole-file version's nodes"
    );
    assert_eq!(view.source, "fmt-2", "row: and its own source");

    let v2 = idx_padded(2);
    let vid2 = d.write(&v2);
    let job = d.s.index_job(None).unwrap();
    job.expect("row: a job is due for the second chunked version")
        .run()
        .expect("row: the second job records");
    assert_eq!(
        f.store.blob_struct_rows_for_tests().unwrap(),
        3,
        "row: one index row per file"
    );
    f.store
        .delete_blob_version(d.id, 1, f.host.now_ms())
        .unwrap();
    assert_eq!(
        f.store.blob_struct_rows_for_tests().unwrap(),
        2,
        "row: retention dropped the version's index row with its manifest"
    );
    assert!(
        matches!(
            d.s.index_status(Some(vid2)).unwrap(),
            IndexStatus::Ready { .. }
        ),
        "row: the kept version's index stays"
    );
}

// ---------- r. a space delete ----------

/// `SessionManager::delete_space` removes its docs' segment folders in the same step, once its
/// transaction commits: none is left for a later `gc`.
/// `MPC_CHUNK_PLANT=delete-space-leaves-segments` fails at the planted step.
#[test]
fn a_space_delete_removes_its_docs_segment_folders() {
    let f = fx("space-delete");
    // control: a doc delete removes its doc's folder
    let c = f.import("ctl.bin", &data(30, 2 * MIB));
    let cdir = f.blobs.segment_dir(SPACE, c.id).unwrap();
    assert!(
        cdir.is_dir(),
        "control: a chunked doc has its segment folder"
    );
    f.mgr.delete_doc(&c.key).unwrap();
    assert!(!cdir.exists(), "control: a doc delete removes it");
    println!("control: a doc delete removes its segment folder");

    let a = f.import("a.bin", &data(31, 2 * MIB));
    let b = f.import("b.bin", &data(32, 3 * MIB));
    b.write(&edited(&data(32, 3 * MIB), MIB, 2));
    let dirs = [a.id, b.id].map(|id| f.blobs.segment_dir(SPACE, id).unwrap());
    for d in &dirs {
        assert!(
            d.is_dir(),
            "row: {} is there before the delete",
            d.display()
        );
    }
    drop((a, b));
    let gone = planted(ChunkPlant::DeleteSpaceLeavesSegments, || {
        f.mgr.delete_space(SPACE).unwrap()
    });
    assert_eq!(gone.len(), 2, "row: both docs are deleted");
    for d in &dirs {
        assert!(
            !d.exists(),
            "row: the space delete removed {} before any gc",
            d.display()
        );
    }
}

// ---------- s. a busy doc lock ----------

/// A chunked write that finds the doc's internal lock busy stores a whole-file version, and the
/// blob store counts it (`BlobStats::chunked_busy_fallbacks`, in memory since its open).
#[test]
fn a_write_finding_the_doc_lock_busy_is_whole_file_and_counted() {
    let f = fx("busy");
    let v1 = data(40, 3 * MIB);
    let d = f.import("big.bin", &v1);
    assert_eq!(
        f.blobs.stats().unwrap().chunked_busy_fallbacks,
        0,
        "row: nothing is counted before"
    );
    let v2 = edited(&v1, MIB, 2);
    let held = f.blobs.hold_doc_lock_for_tests(d.id);
    let vid = d.write(&v2);
    drop(held);
    assert!(
        f.blobs.info(SPACE, &f.id_of(&v2)).unwrap().is_some(),
        "row: the version is a whole-file blob"
    );
    assert_eq!(
        f.report(&d).manifests.len(),
        1,
        "row: no manifest was made for it"
    );
    assert_eq!(
        f.blobs.stats().unwrap().chunked_busy_fallbacks,
        1,
        "row: the fallback is counted"
    );
    assert!(d.at(vid) == v2, "row: the version reads back");
}

// ---------- t. a stop after a purge's commit ----------

/// A stop between a purge's commit and its removals: the reopened store finishes the removal
/// list at its open, and the byte scan finds no byte, id or manifest of the purged version.
/// `MPC_CHUNK_PLANT=removal-list-not-finished-at-open` fails at the planted step.
#[test]
fn a_stop_after_a_purges_commit_is_finished_at_the_next_open() {
    let f = fx("purge-stop");
    let (d, [v1, v2, v3]) = marked_three(&f, "big.bin", 19);
    let before = f.report(&d);
    let file = f.id_of(&v1);
    let blobs = f.blobs.clone();
    let saw = Rc::new(RefCell::new(None::<usize>));
    let s2 = saw.clone();
    let out = with_chunk_hook(
        move |step| {
            if step == ChunkStep::PurgeCommitted {
                *s2.borrow_mut() = Some(blobs.pending_removals_for_tests().unwrap());
                return true;
            }
            false
        },
        || d.s.purge(d.c, Some(1)),
    );
    let pending = saw
        .borrow_mut()
        .take()
        .expect("row: the purge reached its commit");
    assert!(out.is_err(), "row: the purge stopped after its commit");
    assert!(
        pending > 0,
        "control: the purge's commit wrote its removal list"
    );
    println!("control: the purge's commit wrote its removal list ({pending} entries)");
    let committed = f.report(&d);
    drop(d);
    let f = planted(ChunkPlant::RemovalListNotFinishedAtOpen, || restart(f));
    assert_eq!(
        f.blobs.pending_removals_for_tests().unwrap(),
        0,
        "row: the open finished the removal list"
    );
    let gone: BTreeSet<[u8; 32]> = chunk_ids(&before)
        .difference(&chunk_ids(&committed))
        .copied()
        .collect();
    assert!(
        !gone.is_empty(),
        "row: the commit erased chunks only it held"
    );
    let found = scan(&f, &purged_needles(&file, &gone));
    assert!(
        found.is_empty(),
        "row: no byte, id or manifest of the purged version is left: {found:?}"
    );
    let d = f.reopen("big.bin");
    assert!(
        d.at(2) == v2 && d.at(3) == v3,
        "row: the kept versions read whole"
    );
}

// ---------- u. a duplicate copy ----------

/// A chunk that exists only in history is written again by a write from a connection without
/// history that names it (a duplicate copy no row points at); a purge of every version holding
/// it leaves its bytes and id nowhere.
#[test]
fn a_purge_compacts_away_a_duplicate_copy() {
    let f = fx("purge-dup");
    let base = data(21, 3 * MIB);
    let a = marked(&base, 3 * MIB / 2);
    let d = f.import("big.bin", &a);
    d.write(&base);
    let bob = f.join_as(&d, "bob", no_history());
    d.write_by(bob, &edited(&a, MIB / 2, 6)).0.unwrap();
    let v4 = edited(&base, 2 * MIB, 7);
    d.write(&v4);
    let holding = scan(&f, &[MARKER.to_vec()])
        .iter()
        .filter(|l| l.contains(".seg"))
        .count();
    assert!(
        holding >= 2,
        "control: the history-only chunk was written again ({holding} copies)"
    );
    println!("control: the history-only chunk has a duplicate copy");
    let before = f.report(&d);
    let file = f.id_of(&a);
    d.s.purge(d.c, None).expect("row: the purge runs");
    let after = f.report(&d);
    let gone: BTreeSet<[u8; 32]> = chunk_ids(&before)
        .difference(&chunk_ids(&after))
        .copied()
        .collect();
    let found = scan(&f, &purged_needles(&file, &gone));
    assert!(
        found.is_empty(),
        "row: the duplicate's bytes and id are nowhere: {found:?}"
    );
    assert_eq!(dead_ranges(&after), 0, "row: no dead range is left");
    assert!(d.at(4) == v4, "row: the current version reads whole");
}

// ---------- v. dead ranges ----------

/// Retention drops a version holding a marker and defers compaction (its segment stays, the
/// marker a dead range in it); a purge of another version, whose own chunks are all in a
/// segment of their own (so the marker's segment holds none of them), then compacts every
/// segment with a dead range, and the marker is nowhere: the store file, every segment file,
/// the removal list. Every segment file then holds its kept chunks' bytes only.
/// `MPC_CHUNK_PLANT=compaction-skips-dead-ranges` fails at the planted step.
#[test]
fn a_purge_compacts_away_the_dead_ranges_retention_left() {
    let f = fx("purge-dead");
    let base = data(22, 3 * MIB);
    let d = f.import("big.bin", &marked(&base, 3 * MIB / 2)); // segment 1, the marker in it
    d.write(&base); // segment 2: the marker's region without it
    let v3 = edited(&base, 5 * MIB / 2, 3); // segment 3: its own edit (the version purged)
    d.write(&v3);
    let v4 = edited(&base, 5 * MIB / 2, 9); // segment 4: another edit there (current)
    d.write(&v4);
    f.store
        .delete_blob_version(d.id, 1, f.host.now_ms())
        .unwrap();
    let r = f.report(&d);
    let in_seg = |seg: u64| -> BTreeSet<[u8; 32]> {
        r.segments
            .iter()
            .filter(|s| s.0 == seg)
            .flat_map(|s| s.1.iter().map(|e| e.0))
            .collect()
    };
    assert!(dead_ranges(&r) > 0, "control: retention left a dead range");
    assert!(
        !scan(&f, &[MARKER.to_vec()]).is_empty(),
        "control: the marker is still in a segment file"
    );
    // the chunks only version 3 holds (one reference, and not in version 2's or 4's segment)
    let only3: BTreeSet<[u8; 32]> = in_seg(3)
        .into_iter()
        .filter(|id| r.chunks.iter().any(|c| c.0 == *id && c.4 == 1))
        .collect();
    assert!(
        !only3.is_empty() && in_seg(1).is_disjoint(&only3),
        "control: segment 1 holds no chunk the purge erases"
    );
    println!("control: retention deferred compaction: the marker is a dead range in segment 1");
    let out = planted(ChunkPlant::CompactionSkipsDeadRanges, || {
        d.s.purge(d.c, Some(3))
    });
    out.expect("row: the purge runs");
    let found = scan(&f, &[MARKER.to_vec()]);
    assert!(
        found.is_empty(),
        "row: the marker is nowhere after the purge: {found:?}"
    );
    let after = f.report(&d);
    assert_eq!(dead_ranges(&after), 0, "row: no dead range is left");
    // every segment file holds exactly its kept chunks' bytes
    for (seg, list) in &after.segments {
        let kept: u64 = list.iter().map(|e| u64::from(e.2)).sum();
        assert_eq!(
            file_bytes(&after, *seg),
            kept,
            "row: segment {seg} holds its kept chunks' bytes only"
        );
    }
    assert!(
        d.at(2) == base && d.at(4) == v4,
        "row: the kept versions read whole"
    );
}

// ---------- w. gc during a compaction ----------

/// The names of the files in `dir`.
fn names_in(dir: &std::path::Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// `gc` while a purge's compaction is between its output's rename and its commit: the output is
/// waiting, so `gc` leaves it, the purge commits, and the kept versions read whole.
/// `MPC_CHUNK_PLANT=compaction-output-not-waiting` fails at the planted step.
#[test]
fn gc_during_a_purges_compaction_leaves_its_output() {
    let f = fx("purge-gc");
    let (d, [_, v2, v3]) = marked_three(&f, "big.bin", 23);
    f.blobs.gc().unwrap();
    assert!(
        d.at(2) == v2 && d.at(3) == v3,
        "control: gc before a purge leaves every version"
    );
    println!("control: gc before a purge leaves every version");
    let dir = f.blobs.segment_dir(SPACE, d.id).unwrap();
    let blobs = f.blobs.clone();
    let seen = Rc::new(RefCell::new(None::<(BTreeSet<String>, BTreeSet<String>)>));
    let s2 = seen.clone();
    let out = planted(ChunkPlant::CompactionOutputNotWaiting, || {
        with_chunk_hook(
            move |step| {
                if step == ChunkStep::CompactionRenamed && s2.borrow().is_none() {
                    let before = names_in(&dir);
                    blobs.gc().unwrap();
                    *s2.borrow_mut() = Some((before, names_in(&dir)));
                }
                false
            },
            || d.s.purge(d.c, Some(1)),
        )
    });
    let (before, after) = seen
        .borrow_mut()
        .take()
        .expect("row: gc ran during the compaction");
    assert_eq!(
        after, before,
        "row: gc left the compaction's output segment"
    );
    out.expect("row: the purge commits");
    assert!(
        d.at(2) == v2 && d.at(3) == v3,
        "row: the kept versions read whole"
    );
}

// ---------- x. a purge beside a write ----------

/// What a write raced by a purge from another thread saw ([`race_write_and_purge`]).
struct Race {
    doc: Doc,
    bytes: Vec<u8>,
    write: Result<u64, SessionError>,
    /// The purge ended while the write was between its reuse and its commit.
    purge_during: bool,
    purge: Result<(), String>,
}

/// A doc whose version 1 holds a chunk only it holds; a write (by a connection with the
/// history right) reuses that chunk, and once the write's segment is in place, a purge of
/// version 1 starts on another thread (with `plant` there). The write waits up to 3 s for the
/// purge to end, then commits.
fn race_write_and_purge(f: &Fx, path: &str, plant: Option<ChunkPlant>) -> Race {
    let a = data(24, 3 * MIB);
    let d = f.import(path, &a);
    d.write(&edited(&a, MIB, 3));
    let a2 = edited(&a, 2 * MIB + 900, 7);
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let rx = Rc::new(rx);
    let (rx2, s, c) = (rx.clone(), d.s.clone(), d.c);
    let ended = Rc::new(RefCell::new(None::<Result<(), String>>));
    let e2 = ended.clone();
    let mut tx = Some(tx);
    let write = with_chunk_hook(
        move |step| {
            if step != ChunkStep::FolderSynced {
                return false;
            }
            if let Some(tx) = tx.take() {
                let s = s.clone();
                std::thread::spawn(move || {
                    let run = || {
                        s.purge(c, Some(1))
                            .map(|_| ())
                            .map_err(|e| format!("{e:?}"))
                    };
                    let out = match plant {
                        Some(p) => with_chunk_plant(p, run),
                        None => run(),
                    };
                    let _ = tx.send(out);
                });
                if let Ok(out) = rx2.recv_timeout(std::time::Duration::from_secs(3)) {
                    *e2.borrow_mut() = Some(out);
                }
            }
            false
        },
        || d.try_write(&a2),
    );
    let during = ended.borrow_mut().take();
    let purge_during = during.is_some();
    let purge = match during {
        Some(out) => out,
        None => rx
            .recv_timeout(std::time::Duration::from_secs(120))
            .unwrap_or_else(|e| Err(format!("the purge didn't end: {e:?}"))),
    };
    Race {
        doc: d,
        bytes: a2,
        write,
        purge_during,
        purge,
    }
}

/// A purge of the version a write reuses a chunk of is held off by the doc's internal lock until
/// the write commits: the write lands, then the purge runs, and the written version reads whole.
/// With the lock planted off (`purge-skips-doc-lock`, in the row) the purge runs beside the write
/// and the write fails closed: refused, nothing recorded.
#[test]
fn a_purge_of_the_version_a_write_reuses_waits_for_the_write() {
    let f = fx("purge-race");
    let r = race_write_and_purge(&f, "a.bin", None);
    assert!(
        !r.purge_during,
        "row: the purge waited for the write's commit (the doc's lock)"
    );
    let vid = r.write.expect("row: the write landed");
    r.purge.expect("row: then the purge ran");
    assert!(
        r.doc.at(vid) == r.bytes,
        "row: the written version reads whole after the purge"
    );
    assert!(
        r.doc.s.versions(r.doc.c).unwrap()[0].1.redacted,
        "row: version 1 is purged"
    );

    let p = race_write_and_purge(&f, "b.bin", Some(ChunkPlant::PurgeSkipsDocLock));
    assert!(
        p.purge_during,
        "row: with the lock planted off, the purge ran beside the write"
    );
    p.purge.expect("row: and purged");
    let e = p.write.expect_err("row: the write fails closed");
    assert!(
        matches!(e, SessionError::Store(StoreError::ChunkGone)),
        "row: refused as a chunk gone: {e:?}"
    );
    assert_eq!(p.doc.current(), 2, "row: no version was recorded");
}

// ---------- y. reads beside a compaction ----------

/// A read of a kept version during a compaction of its segments returns it whole: one read while
/// a purge's compaction is between its output's rename and its commit, and one whose rows were
/// read before a compaction moved its chunks and removed their old segment (the read takes the
/// rows again).
#[test]
fn a_read_beside_a_compaction_of_its_segments_returns_the_version_whole() {
    let f = fx("purge-read");
    let (d, [_, _, v3]) = marked_three(&f, "a.bin", 25);
    let id3 = f.id_of(&v3);
    let (blobs, doc, want) = (f.blobs.clone(), d.id, v3.clone());
    let got = Rc::new(RefCell::new(None::<bool>));
    let g2 = got.clone();
    let out = with_chunk_hook(
        move |step| {
            if step == ChunkStep::CompactionRenamed && g2.borrow().is_none() {
                let r = blobs.read_to_vec_of(SPACE, doc, &id3, u64::MAX);
                *g2.borrow_mut() = Some(r.is_ok_and(|b| b == want));
            }
            false
        },
        || d.s.purge(d.c, Some(1)),
    );
    let whole = got
        .borrow_mut()
        .take()
        .expect("row: a read ran during the compaction");
    out.expect("row: the purge runs");
    assert!(
        whole,
        "row: the read during the compaction returned the version whole"
    );

    let (e, [_, _, w3]) = marked_three(&f, "b.bin", 26);
    let idw = f.id_of(&w3);
    let before = f.report(&e);
    let (blobs, doc) = (f.blobs.clone(), e.id);
    let purged = Rc::new(RefCell::new(None::<Result<(), String>>));
    let p2 = purged.clone();
    let read = with_chunk_hook(
        move |step| {
            if step == ChunkStep::ReaderRowsRead && p2.borrow().is_none() {
                let out = blobs.purge_versions_for_tests(SPACE, doc, &[1]);
                *p2.borrow_mut() = Some(out.map_err(|x| x.to_string()));
            }
            false
        },
        || f.blobs.read_to_vec_of(SPACE, e.id, &idw, u64::MAX),
    );
    let p = purged
        .borrow_mut()
        .take()
        .expect("row: a purge ran between the read's rows and its opens");
    p.expect("row: the purge ran");
    let after = f.report(&e);
    assert!(
        file_segments(&before)
            .difference(&file_segments(&after))
            .count()
            > 0,
        "row: the compaction removed a segment the read's rows named"
    );
    let b = read.expect("row: the read returns the kept version");
    assert!(b == w3, "row: whole");
}

// ---------- z. shared, and the closing sweep ----------

/// A purge reports `SharedBlobs` only when another version still holds the whole manifest (the
/// same bytes): purging a version whose manifest only it held reports nothing shared; purging
/// one of two versions with identical bytes reports it shared, and the other still reads.
#[test]
fn a_purge_reports_shared_only_when_the_whole_manifest_is_kept() {
    let f = fx("purge-shared");
    let a = data(27, 3 * MIB);
    let b = edited(&a, MIB, 2);
    let d = f.import("big.bin", &a);
    d.write(&b);
    let restored = d.s.restore(d.c, 1).unwrap().version;
    let v4 = edited(&b, 2 * MIB, 5);
    d.write(&v4);
    let shared = |o: &k2_mpc_base::session::PurgeOutcome| {
        o.not_reached
            .iter()
            .any(|n| matches!(n, NotReached::SharedBlobs(_)))
    };
    let out = d.s.purge(d.c, Some(2)).expect("row: the purge runs");
    assert!(
        !shared(&out),
        "row: a manifest only the purged version held isn't shared: {:?}",
        out.not_reached
    );
    let out = d.s.purge(d.c, Some(1)).expect("row: the second purge runs");
    assert!(
        out.not_reached
            .iter()
            .any(|n| matches!(n, NotReached::SharedBlobs(1))),
        "row: one of two versions with the same bytes is reported shared: {:?}",
        out.not_reached
    );
    assert!(
        d.at(restored) == a,
        "row: the other version with the same bytes still reads"
    );
    assert!(d.at(4) == v4, "row: the current version reads");
}

/// A purge ends with a sweep of the doc's segment folder (a segment no row names and a temp
/// file go), and erasing the doc removes its rows and its segment folder.
#[test]
fn a_purge_ends_with_a_sweep_and_an_erasure_removes_the_docs_folder() {
    let f = fx("purge-sweep");
    let (d, [_, v2, v3]) = marked_three(&f, "big.bin", 28);
    let dir = f.blobs.segment_dir(SPACE, d.id).unwrap();
    let (stray, temp) = (dir.join("99.seg"), dir.join("4-1.seg.part"));
    fs::write(&stray, b"a segment no row names").unwrap();
    fs::write(&temp, b"a temp file").unwrap();
    d.s.purge(d.c, Some(1)).expect("row: the purge runs");
    assert!(
        !stray.exists(),
        "row: the purge's closing sweep removed a rowless segment"
    );
    assert!(!temp.exists(), "row: and a temp file");
    assert!(
        d.at(2) == v2 && d.at(3) == v3,
        "row: the kept versions read whole"
    );
    f.mgr.delete_doc(&d.key).unwrap();
    let after = f.blobs.chunk_report(SPACE, d.id).unwrap();
    assert!(
        after.manifests.is_empty() && after.chunks.is_empty() && after.segments.is_empty(),
        "row: erasing the doc removed its rows: {after:?}"
    );
    assert!(!dir.exists(), "row: and its segment folder");
}

// ---------- aa. equal work for a writer without history ----------

/// Where the edits of the equal-work rows go: the region whose chunk differs, and a second
/// region far from it (so the cuts between them are the same in every variant).
const AT_R: usize = MIB;
const AT_S: usize = 5 * MIB / 2;

/// `base` edited at [`AT_R`] with `r` and at [`AT_S`] with `s` (every variant the same length).
fn two_edits(base: &[u8], r: u8, s: u8) -> Vec<u8> {
    edited(&edited(base, AT_R, r), AT_S, s)
}

/// Rights to read and edit a doc, without its history.
fn no_history() -> Rights {
    Rights::full().with_history(false)
}

/// Two docs whose current version (2) is the same, and whose version 1 differs only at
/// [`AT_R`]: in the first, version 1 holds the chunk `w` has there (a chunk only history
/// holds); in the second, nothing does (brand-new). `w` differs from the current version only
/// there. A connection without history is joined to each.
struct Twins {
    hist: Doc,
    new: Doc,
    hist_c: ConnId,
    new_c: ConnId,
    w: Vec<u8>,
}

fn twins(f: &Fx, tag: &str, seed: u64) -> Twins {
    let base = data(seed, 3 * MIB);
    let current = two_edits(&base, 12, 22);
    let hist = f.import(&format!("{tag}-h.bin"), &two_edits(&base, 11, 21));
    hist.write(&current);
    let new = f.import(&format!("{tag}-n.bin"), &two_edits(&base, 13, 21));
    new.write(&current);
    let hist_c = f.join_as(&hist, "bob", no_history());
    let new_c = f.join_as(&new, "bob", no_history());
    Twins {
        hist,
        new,
        hist_c,
        new_c,
        w: two_edits(&base, 11, 22),
    }
}

/// A connection without history writes a file whose only difference from the current version
/// is the chunk at one place: once with that chunk brand-new, once with it held only by an older
/// version. The counted work (segment bytes, file syncs, folder syncs, rows written, commits) is
/// equal field for field: the history-only chunk is written again, as new content is. Control:
/// the chunks of the current version take the cheap path (the same file written to a doc whose
/// current version doesn't hold them counts many more bytes).
/// `MPC_CHUNK_PLANT=history-chunk-cheap` fails at the planted step.
#[test]
fn a_new_and_a_history_only_chunk_cost_a_writer_without_history_the_same() {
    let f = fx("equal-work");
    let t = twins(&f, "ew", 30);
    // control: a doc whose current version holds every chunk of the file but the one at AT_S,
    // against a doc holding none of them
    let base = data(30, 3 * MIB);
    let near = f.import("ew-c.bin", &two_edits(&base, 11, 21));
    let near_c = f.join_as(&near, "bob", no_history());
    let far = f.import("ew-f.bin", &data(31, t.w.len()));
    let far_c = f.join_as(&far, "bob", no_history());
    let (o, cheap) = near.write_by(near_c, &t.w);
    o.unwrap();
    let (o, full) = far.write_by(far_c, &t.w);
    o.unwrap();
    assert!(
        full.segment_bytes >= t.w.len() as u64 && cheap.segment_bytes * 4 < full.segment_bytes,
        "control: the current version's chunks take the cheap path: {cheap:?} against {full:?}"
    );
    assert_ne!(cheap, full, "control: and the counts differ");
    println!("control: the current version's chunks take the cheap path ({cheap:?}, {full:?})");

    let (hist, new) = planted(ChunkPlant::HistoryChunkCheap, || {
        let (o, hist) = t.hist.write_by(t.hist_c, &t.w);
        o.unwrap();
        let (o, new) = t.new.write_by(t.new_c, &t.w);
        o.unwrap();
        (hist, new)
    });
    assert!(new.segment_bytes > 0, "row: the brand-new chunk is written");
    assert_eq!(
        hist, new,
        "row: a history-only chunk costs what a brand-new one does"
    );
    let r = f.report(&t.hist);
    let newest = *row_segments(&r).iter().max().unwrap();
    assert!(
        copied_again(&r, newest),
        "row: the history-only chunk has a second copy, in the write's segment"
    );
    assert!(
        t.hist.now() == t.w && t.new.now() == t.w,
        "row: both read back"
    );
}

/// Whether segment `seg` of a report lists a chunk an older segment lists too: a chunk a write
/// wrote again.
fn copied_again(r: &ChunkReport, seg: u64) -> bool {
    let ids = |pick: &dyn Fn(u64) -> bool| -> BTreeSet<[u8; 32]> {
        r.segments
            .iter()
            .filter(|s| pick(s.0))
            .flat_map(|s| s.1.iter().map(|e| e.0))
            .collect()
    };
    !ids(&|n| n == seg).is_disjoint(&ids(&|n| n < seg))
}

/// A connection without history writes a whole file equal to an older version's (its manifest
/// stored, every chunk held only by that version): the counted work equals a brand-new file's
/// of the same bytes in another doc, field for field. The chunks and the manifest's own are
/// written again, then the existing manifest's count is raised; the version reads back.
/// Control: the brand-new file's write is a full write.
/// `MPC_CHUNK_PLANT=manifest-reuse-skips-full-write` fails at the planted step.
#[test]
fn a_file_equal_to_an_older_version_costs_what_a_new_file_does() {
    let f = fx("equal-manifest");
    let (u1, u2) = (data(32, 3 * MIB), data(33, 3 * MIB));
    let old = f.import("em-o.bin", &u1);
    old.write(&u2);
    let new = f.import("em-n.bin", &data(34, 3 * MIB));
    new.write(&u2);
    let (old_c, new_c) = (
        f.join_as(&old, "bob", no_history()),
        f.join_as(&new, "bob", no_history()),
    );
    let (o, fresh) = new.write_by(new_c, &u1);
    o.unwrap();
    assert!(
        fresh.segment_bytes >= u1.len() as u64 && fresh.file_syncs == 1,
        "control: a brand-new file's write is a full write: {fresh:?}"
    );
    println!("control: a brand-new file's write is a full write ({fresh:?})");

    let (o, again) = planted(ChunkPlant::ManifestReuseSkipsFullWrite, || {
        old.write_by(old_c, &u1)
    });
    let v = o.expect("row: the write lands").version;
    assert_eq!(
        again, fresh,
        "row: a file equal to an older version costs what a brand-new one does"
    );
    let r = f.report(&old);
    assert_eq!(
        r.manifests
            .iter()
            .find(|m| m.0 == f.id_of(&u1))
            .map(|m| m.1),
        Some(2),
        "row: the existing manifest counts both versions"
    );
    assert!(old.at(v) == u1, "row: the version reads back");
}

/// For a write from a connection without history, every value it gets back, and every event
/// the host gets, is the same whether the chunk it differs by was brand-new or held only by an
/// older version: the reply, the doc's status, and the events (the doc's path aside).
#[test]
fn a_writer_without_history_gets_the_same_reply_for_a_new_or_a_history_only_chunk() {
    let f = fx("equal-reply");
    let t = twins(&f, "er", 35);
    f.events_of(&t.hist);
    let (hist, _) = t.hist.write_by(t.hist_c, &t.w);
    let hist_ev = f.events_of(&t.hist);
    let (new, _) = t.new.write_by(t.new_c, &t.w);
    let new_ev = f.events_of(&t.new);
    let (hist, new) = (hist.unwrap(), new.unwrap());
    assert_eq!(hist, new, "row: the reply is the same");
    assert!(
        hist.changed && hist.blob == f.id_of(&t.w),
        "row: the write recorded the file: {hist:?}"
    );
    assert_eq!(hist_ev, new_ev, "row: the host's events are the same");
    let (hs, ns) = (t.hist.s.status().unwrap(), t.new.s.status().unwrap());
    assert_eq!(
        (hs.version, &hs.current, hs.index),
        (ns.version, &ns.current, ns.index),
        "row: the status is the same"
    );
}

// ---------- ab. the cheap set from a connection's rights ----------

/// A connection without history that names an old version as its write's base is refused
/// before anything is stored (the base must be the current version: a stale base), with no work
/// counted; naming its read base (the current version) it takes the cheap path, writing a few
/// chunks of a large file.
#[test]
fn a_writer_without_history_naming_an_old_base_is_refused_and_its_read_base_is_cheap() {
    let f = fx("base");
    let v1 = data(36, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 3);
    d.write(&v2);
    let c = f.join_as(&d, "bob", no_history());
    let v3 = edited(&v2, 2 * MIB, 4);
    take_chunk_work();
    let refused = d.s.write_bytes(c, &v3, 1);
    let work = take_chunk_work();
    assert!(
        matches!(
            refused,
            Err(SessionError::Stale {
                expected: 1,
                actual: 2
            })
        ),
        "row: an old base is refused: {refused:?}"
    );
    assert_eq!(work, ChunkWork::default(), "row: and nothing was stored");
    let (o, work) = d.write_by(c, &v3);
    let v = o.expect("row: the current base is taken").version;
    assert!(
        work.segment_bytes > 0 && work.segment_bytes * 4 < v3.len() as u64,
        "row: the current version's chunks take the cheap path: {work:?}"
    );
    assert!(d.at(v) == v3, "row: the version reads back");
}

/// A connection with the history right restores a large old version: no chunk byte is written
/// (every chunk and the manifest are the old version's), and it reads back. A connection
/// without the right can't restore (it needs history); writing the same bytes, it writes them
/// all.
#[test]
fn a_history_writer_restores_a_large_version_writing_no_chunk() {
    let f = fx("restore-wide");
    let (big, other) = (data(37, 8 * MIB), data(38, 8 * MIB));
    let a = f.import("a.bin", &big);
    a.write(&other);
    let b = f.import("b.bin", &big);
    b.write(&other);
    take_chunk_work();
    let r = a.s.restore(a.c, 1).expect("row: the restore runs");
    let work = take_chunk_work();
    assert_eq!(
        (work.segment_bytes, work.file_syncs, work.folder_syncs),
        (0, 0, 0),
        "row: a history writer's restore writes no chunk: {work:?}"
    );
    assert!(r.changed && a.at(r.version) == big, "row: it reads back");

    let c = f.join_as(&b, "bob", no_history());
    take_chunk_work();
    let refused = b.s.restore(c, 1);
    assert!(
        matches!(
            refused,
            Err(SessionError::Forbidden {
                need: Need::History
            })
        ),
        "row: a restore needs history: {refused:?}"
    );
    assert_eq!(
        take_chunk_work(),
        ChunkWork::default(),
        "row: nothing stored"
    );
    let (o, work) = b.write_by(c, &big);
    let v = o.expect("row: the write lands").version;
    assert!(
        work.segment_bytes >= big.len() as u64,
        "row: without history, the same bytes are written again: {work:?}"
    );
    assert!(b.at(v) == big, "row: and read back");
}

/// The cheap set is worked out at each write from the rights then in force: a connection with
/// the history right reuses a chunk only an older version holds (no second copy); once the
/// host drops that right (`recheck_rights` reports it), the same connection's next write of a
/// history-only chunk writes it again. `MPC_CHUNK_PLANT=cheap-set-ignores-history-right` fails
/// at the planted step.
#[test]
fn after_its_history_right_is_dropped_a_connections_next_write_is_narrow() {
    let f = fx("rights");
    let base = data(39, 3 * MIB);
    let d = f.import("big.bin", &two_edits(&base, 11, 21));
    d.write(&two_edits(&base, 12, 22));
    let c = f.join_as(&d, "carol", Rights::full());
    let (o, _) = d.write_by(c, &two_edits(&base, 11, 22));
    o.unwrap();
    let r = f.report(&d);
    let newest = *row_segments(&r).iter().max().unwrap();
    assert!(
        !copied_again(&r, newest),
        "row: with the history right, a history-only chunk is reused, not copied"
    );
    println!("control: a connection with the history right reuses a history-only chunk");

    f.host
        .rights
        .lock()
        .unwrap()
        .insert("carol".into(), no_history());
    let changed = d.s.recheck_rights().unwrap();
    assert!(
        changed
            .iter()
            .any(|c| c.pid == "carol" && !c.rights.history),
        "row: the rights change is reported: {changed:?}"
    );
    // version 2's chunk at AT_R is history-only now
    let w = two_edits(&base, 12, 23);
    let (o, _) = planted(ChunkPlant::CheapSetIgnoresHistoryRight, || {
        d.write_by(c, &w)
    });
    let v = o.expect("row: the write lands").version;
    let r = f.report(&d);
    let newest = *row_segments(&r).iter().max().unwrap();
    assert!(
        copied_again(&r, newest),
        "row: without the history right, the history-only chunk is written again"
    );
    assert!(d.at(v) == w, "row: the version reads back");
}

// ---------- the rechunk of whole-file versions ----------

/// The whole-file blob of `b`'s bytes: its references, if the space holds it.
fn whole_refs(f: &Fx, b: &[u8]) -> Option<u64> {
    f.blobs
        .info(SPACE, &f.id_of(b))
        .unwrap()
        .map(|(_, refs)| refs)
}

/// The versions naming doc `d`'s manifest of `b`'s bytes, if it holds one.
fn manifest_refs(f: &Fx, d: &Doc, b: &[u8]) -> Option<u64> {
    let id = f.id_of(b);
    f.report(d)
        .manifests
        .iter()
        .find(|m| m.0 == id)
        .map(|m| m.1)
}

/// Doc `big.bin` with two whole-file versions of 3 MiB (`v1`, then a small edit `v2`), written
/// with the setting off; the store reopened with it on.
fn whole_two(tag: &str, seed: u64) -> (Fx, Doc, Vec<u8>, Vec<u8>) {
    let mut f = fx_with(tag, config(false));
    let v1 = data(seed, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB + 77, 5);
    d.write(&v2);
    drop(d);
    f.bc = config(true);
    let f = restart(f);
    let d = f.reopen("big.bin");
    (f, d, v1, v2)
}

/// `rechunk` converts a doc's whole-file versions at or over the minimum: each reads back byte
/// for byte with its file id unchanged, a manifest counts the versions naming its bytes (a
/// restore included), and the store is stamped format 8 with the chunked feature. A version
/// under the minimum stays whole and a purged one is skipped. A whole-file blob only the
/// converted versions held is gone right after, with no `gc`; one another doc's version holds
/// stays, with that doc's reference. A second rechunk finds nothing to do.
/// `MPC_CHUNK_PLANT=rechunk-keeps-old-blob` fails at the planted step.
#[test]
fn a_rechunk_converts_a_docs_whole_file_versions_and_removes_their_blobs_at_once() {
    let mut f = fx_with("rechunk", config(false));
    let v1 = data(50, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 5);
    d.write(&v2);
    let r = d.s.restore(d.c, 1).unwrap();
    assert_eq!(r.version, 3, "version 3 restores version 1's bytes");
    let v4 = edited(&v2, 2 * MIB, 6);
    d.write(&v4);
    let v5 = data(51, MIB / 2);
    d.write(&v5);
    d.s.purge(d.c, Some(4)).unwrap();
    let other = f.import("other.bin", &v1);
    drop((d, other));
    f.bc = config(true);
    let f = restart(f);
    let d = f.reopen("big.bin");
    let other = f.reopen("other.bin");
    assert_eq!(
        whole_refs(&f, &v1),
        Some(3),
        "control: version 1's bytes are a whole-file blob held by three versions"
    );
    assert_eq!(whole_refs(&f, &v2), Some(1));
    assert!(
        f.report(&d).tables.is_empty(),
        "control: no chunk table yet"
    );
    println!("control: five versions, whole files, the store unstamped");
    let rep = planted(ChunkPlant::RechunkKeepsOldBlob, || {
        f.blobs.rechunk(SPACE, d.id)
    })
    .expect("row: the rechunk runs");
    println!("rechunk: {rep:?}");
    assert_eq!(rep.converted, 3, "row: versions 1, 2 and 3 are converted");
    assert_eq!(rep.under_minimum, 1, "row: version 5 is under the minimum");
    assert_eq!(rep.redacted, 1, "row: the purged version 4 is skipped");
    assert_eq!(
        (rep.busy, rep.missing, rep.already_chunked),
        (0, 0, 0),
        "row: nothing else"
    );
    assert_eq!(
        rep.bytes,
        (v1.len() + v2.len()) as u64,
        "row: the bytes of the two blobs converted"
    );
    assert_eq!(
        whole_refs(&f, &v2),
        None,
        "row: the whole-file blob only these versions held is gone right after, with no gc"
    );
    assert_eq!(
        rep.blobs_removed, 1,
        "row: one whole-file blob is removed at once"
    );
    assert_eq!(
        whole_refs(&f, &v1),
        Some(1),
        "row: the blob another doc's version holds stays, with that doc's reference"
    );
    assert_eq!(
        whole_refs(&f, &v5),
        Some(1),
        "row: the version under the minimum stays a whole-file blob"
    );
    assert_eq!(
        manifest_refs(&f, &d, &v1),
        Some(2),
        "row: version 1's manifest counts versions 1 and 3"
    );
    assert_eq!(
        manifest_refs(&f, &d, &v2),
        Some(1),
        "row: version 2 has its manifest"
    );
    let versions = d.s.versions(d.c).unwrap();
    for (vid, b) in [(1, &v1), (2, &v2), (3, &v1), (5, &v5)] {
        let v = &versions.iter().find(|(n, _)| *n == vid).unwrap().1;
        assert_eq!(
            v.blob,
            f.id_of(b),
            "row: version {vid}'s file id is unchanged"
        );
        assert!(
            d.at(vid) == *b,
            "row: version {vid} reads back byte for byte"
        );
    }
    assert!(d.now() == v5, "row: the current version reads back");
    assert!(
        d.s.file_at_to(d.c, 4, &mut Vec::new()).is_err(),
        "row: the purged version stays unreadable"
    );
    assert!(
        other.now() == v1,
        "row: the other doc's version reads whole"
    );
    assert_eq!(f.store.format().unwrap(), 8, "row: the store is format 8");
    assert_eq!(
        f.store.required_features().unwrap(),
        FEATURE_CHUNKED,
        "row: its features word names chunked"
    );
    let again = f.blobs.rechunk(SPACE, d.id).unwrap();
    assert_eq!(
        (again.converted, again.already_chunked),
        (0, 3),
        "row: a second rechunk finds the versions chunked"
    );
    drop((d, other));
    f.blobs.gc().unwrap();
    let f = restart(f);
    let d = f.reopen("big.bin");
    for (vid, b) in [(1, &v1), (2, &v2), (3, &v1), (5, &v5)] {
        assert!(
            d.at(vid) == *b,
            "row: version {vid} reads back after gc and a restart"
        );
    }
}

/// With `chunked_versions` off, `rechunk` is refused by name and changes nothing: no table,
/// no stamp, every version a whole-file blob with its reference.
#[test]
fn a_rechunk_with_the_setting_off_is_refused_and_changes_nothing() {
    let f = fx_with("rechunk-off", config(false));
    let v1 = data(53, 3 * MIB);
    let d = f.import("big.bin", &v1);
    let v2 = edited(&v1, MIB, 5);
    d.write(&v2);
    let out = f.blobs.rechunk(SPACE, d.id);
    println!("rechunk with the setting off: {out:?}");
    assert!(
        matches!(out, Err(RechunkError::ChunkingOff)),
        "row: the rechunk is refused: {out:?}"
    );
    assert!(
        out.unwrap_err().to_string().contains("chunked_versions"),
        "row: the refusal names the setting"
    );
    assert!(f.report(&d).tables.is_empty(), "row: no chunk table");
    assert_eq!(
        f.store.format().unwrap(),
        STORE_FORMAT_BASE,
        "row: the format is unchanged"
    );
    assert_eq!(
        f.store.required_features().unwrap(),
        0,
        "row: no features word"
    );
    for b in [&v1, &v2] {
        assert_eq!(
            whole_refs(&f, b),
            Some(1),
            "row: each version keeps its whole-file blob"
        );
    }
    assert!(d.at(1) == v1 && d.at(2) == v2, "row: both versions read");
}

/// A stop after each step of a rechunk (the segment's temp file, its rename, its folder's
/// sync, the commit, the removal of the old blob): after a restart each version reads whole,
/// either way: a converted one is held by its manifest only (a whole-file blob left at no
/// references by a stop after the commit), an unconverted one by its whole-file blob, nothing
/// half-referenced, and no rowless segment or temp file survives the open. A second rechunk
/// finishes it: every version chunked and no whole-file blob left.
#[test]
fn a_stop_after_each_rechunk_step_leaves_every_version_whole() {
    use ChunkStep::*;
    for (i, stop) in [
        TempWritten,
        Renamed,
        FolderSynced,
        Committed,
        OldBlobRemoved,
    ]
    .into_iter()
    .enumerate()
    {
        let (f, d, v1, v2) = whole_two(&format!("rstop{i}"), 54);
        let reached = Rc::new(RefCell::new(false));
        let r2 = reached.clone();
        let out = with_chunk_hook(
            move |step| {
                if step == stop {
                    *r2.borrow_mut() = true;
                    return true;
                }
                false
            },
            || f.blobs.rechunk(SPACE, d.id),
        );
        assert!(*reached.borrow(), "row: the rechunk reached {stop:?}");
        assert!(out.is_err(), "row: the rechunk stopped after {stop:?}");
        drop(d);
        let f = restart(f);
        let d = f.reopen("big.bin");
        assert!(
            d.at(1) == v1 && d.at(2) == v2,
            "row: each version reads whole after a stop at {stop:?}"
        );
        for b in [&v1, &v2] {
            match manifest_refs(&f, &d, b) {
                Some(m) => {
                    assert_eq!(m, 1, "row: the manifest counts its version ({stop:?})");
                    assert!(
                        whole_refs(&f, b).map_or(true, |w| w == 0),
                        "row: a converted version holds no whole-file reference ({stop:?})"
                    );
                }
                None => assert_eq!(
                    whole_refs(&f, b),
                    Some(1),
                    "row: an unconverted version keeps its whole-file reference ({stop:?})"
                ),
            }
        }
        if matches!(stop, Committed | OldBlobRemoved) {
            assert_eq!(
                manifest_refs(&f, &d, &v1),
                Some(1),
                "row: a stop after {stop:?} keeps the conversion"
            );
        }
        if stop == OldBlobRemoved {
            assert_eq!(whole_refs(&f, &v1), None, "row: the old blob is gone");
        }
        let r = f.report(&d);
        assert_eq!(
            file_segments(&r),
            row_segments(&r),
            "row: no rowless segment survives the open (stop at {stop:?})"
        );
        assert_eq!(temp_files(&f), 0, "row: no temp file survives the open");
        let rep = f.blobs.rechunk(SPACE, d.id).unwrap();
        println!("after a stop at {stop:?}, a second rechunk: {rep:?}");
        for b in [&v1, &v2] {
            assert_eq!(
                manifest_refs(&f, &d, b),
                Some(1),
                "row: the second rechunk converts what is left ({stop:?})"
            );
            assert_eq!(
                whole_refs(&f, b),
                None,
                "row: the second rechunk leaves no whole-file blob ({stop:?})"
            );
        }
        assert!(d.at(1) == v1 && d.at(2) == v2, "row: both read after it");
    }
}

/// A rechunk while a write holds the doc's internal lock (between its segment's rename and its
/// commit) doesn't wait: it skips the doc's versions and reports them busy, changing nothing.
/// The write commits; a later rechunk converts them.
#[test]
fn a_rechunk_beside_a_write_skips_the_busy_versions_and_reports_them() {
    let (f, d, v1, v2) = whole_two("rbusy", 55);
    let v3 = edited(&v2, 2 * MIB, 7);
    let blobs = f.blobs.clone();
    let id = d.id;
    let seen = Rc::new(RefCell::new(None::<Result<RechunkReport, RechunkError>>));
    let s2 = seen.clone();
    let vid3 = with_chunk_hook(
        move |step| {
            if step == ChunkStep::Renamed && s2.borrow().is_none() {
                *s2.borrow_mut() = Some(blobs.rechunk(SPACE, id));
            }
            false
        },
        || d.try_write(&v3),
    )
    .expect("row: the write beside the rechunk commits");
    let rep = seen
        .borrow_mut()
        .take()
        .expect("row: the rechunk ran while the write held the doc's lock")
        .expect("row: the rechunk returns, refusing nothing");
    println!("control: a rechunk beside a write: {rep:?}");
    assert_eq!(
        rep.busy, 2,
        "row: both whole-file versions are skipped as busy"
    );
    assert_eq!(rep.converted, 0, "row: nothing is converted");
    for b in [&v1, &v2] {
        assert_eq!(
            manifest_refs(&f, &d, b),
            None,
            "row: no manifest is made for a busy version"
        );
        assert_eq!(whole_refs(&f, b), Some(1), "row: its whole-file blob stays");
    }
    assert!(d.at(vid3) == v3, "row: the write's version reads back");
    let rep = f.blobs.rechunk(SPACE, d.id).unwrap();
    assert_eq!(
        (rep.converted, rep.already_chunked, rep.busy),
        (2, 1, 0),
        "row: a later rechunk converts them"
    );
    assert!(
        d.at(1) == v1 && d.at(2) == v2 && d.at(vid3) == v3,
        "row: every version reads"
    );
}

// ---------- cut hints: zip members as allowed cut points ----------

fn p16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn p32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn p64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// CRC-32 (the zip polynomial), bit by bit.
fn crc32(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for &x in b {
        c ^= u32::from(x);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A zip of `members` (name, bytes), each stored uncompressed, written by hand: a local file
/// header before each member, the central directory, and its end record (with the zip64 end
/// record and locator past 65,535 members).
fn zip(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for (name, body) in members {
        let at = u32::try_from(out.len()).unwrap();
        let (crc, n) = (crc32(body), u32::try_from(body.len()).unwrap());
        let nl = u16::try_from(name.len()).unwrap();
        p32(&mut out, 0x0403_4b50);
        for x in [20, 0, 0, 0, 0x21] {
            p16(&mut out, x);
        }
        for x in [crc, n, n] {
            p32(&mut out, x);
        }
        p16(&mut out, nl);
        p16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(body);
        p32(&mut cd, 0x0201_4b50);
        for x in [20, 20, 0, 0, 0, 0x21] {
            p16(&mut cd, x);
        }
        for x in [crc, n, n] {
            p32(&mut cd, x);
        }
        for x in [nl, 0, 0, 0, 0] {
            p16(&mut cd, x);
        }
        p32(&mut cd, 0);
        p32(&mut cd, at);
        cd.extend_from_slice(name.as_bytes());
    }
    let cd_at = out.len() as u64;
    let count = members.len() as u64;
    out.extend_from_slice(&cd);
    let small = u16::try_from(count).ok().filter(|&c| c != 0xffff);
    if small.is_none() {
        let z = out.len() as u64;
        p32(&mut out, 0x0606_4b50);
        p64(&mut out, 44);
        p16(&mut out, 45);
        p16(&mut out, 45);
        p32(&mut out, 0);
        p32(&mut out, 0);
        for x in [count, count, cd.len() as u64, cd_at] {
            p64(&mut out, x);
        }
        p32(&mut out, 0x0706_4b50);
        p32(&mut out, 0);
        p64(&mut out, z);
        p32(&mut out, 1);
    }
    let c16 = small.unwrap_or(0xffff);
    p32(&mut out, 0x0605_4b50);
    for x in [0, 0, c16, c16] {
        p16(&mut out, x);
    }
    p32(&mut out, u32::try_from(cd.len()).unwrap());
    p32(&mut out, u32::try_from(cd_at).unwrap());
    p16(&mut out, 0);
    out
}

/// A zip's member boundaries, from its central directory (the zip64 end record when the end
/// record says so): every local header's offset but the first, and the central directory's,
/// sorted, inside the file. None for bytes that aren't a zip this reads.
fn zip_hints(b: &[u8]) -> Vec<u64> {
    fn read(b: &[u8]) -> Option<Vec<u64>> {
        let g16 = |at: usize| {
            Some(u64::from(u16::from_le_bytes(
                b.get(at..at + 2)?.try_into().ok()?,
            )))
        };
        let g32 = |at: usize| {
            Some(u64::from(u32::from_le_bytes(
                b.get(at..at + 4)?.try_into().ok()?,
            )))
        };
        let g64 = |at: usize| Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?));
        let end = (0..=b.len().checked_sub(22)?)
            .rev()
            .take(65_558)
            .find(|&i| g32(i) == Some(0x0605_4b50))?;
        let (mut n, mut cd) = (g16(end + 10)?, g32(end + 16)?);
        if n == 0xffff || cd == 0xffff_ffff {
            let loc = end.checked_sub(20)?;
            if g32(loc)? != 0x0706_4b50 {
                return None;
            }
            let z = usize::try_from(g64(loc + 8)?).ok()?;
            if g32(z)? != 0x0606_4b50 {
                return None;
            }
            n = g64(z + 32)?;
            cd = g64(z + 48)?;
        }
        let mut at = usize::try_from(cd).ok()?;
        let mut out = Vec::new();
        for _ in 0..n {
            if g32(at)? != 0x0201_4b50 {
                return None;
            }
            out.push(g32(at + 42)?);
            let extra = g16(at + 28)? + g16(at + 30)? + g16(at + 32)?;
            at += 46 + usize::try_from(extra).ok()?;
        }
        out.push(cd);
        out.sort_unstable();
        out.dedup();
        out.retain(|&o| o > 0 && o < b.len() as u64);
        Some(out)
    }
    read(b).unwrap_or_default()
}

/// A zip pack for the hint rows: its versions' member boundaries, for versions up to 64 MiB.
struct ZipPack;

impl BlobPack for ZipPack {
    fn chunk_hints(&self, bytes: &[u8]) -> Vec<u64> {
        zip_hints(bytes)
    }
    fn chunk_hints_up_to(&self) -> u64 {
        64 << 20
    }
}

/// A pack whose hints are fixed, whatever the bytes.
struct FixedHints(Vec<u64>);

impl BlobPack for FixedHints {
    fn chunk_hints(&self, _bytes: &[u8]) -> Vec<u64> {
        self.0.clone()
    }
    fn chunk_hints_up_to(&self) -> u64 {
        64 << 20
    }
}

/// The chunk minimum and maximum of a doc whose first chunked version is under 64 MiB.
const CHUNK_MIN: u64 = 16 << 10;
const CHUNK_MAX: u64 = 256 << 10;

/// The lengths of segment `seg`'s chunks, in file order: for a doc's first version, its cut
/// (each chunk written once, in order), then its manifest's own chunks.
fn seg_lens(r: &ChunkReport, seg: u64) -> Vec<u32> {
    r.segments
        .iter()
        .find(|(n, _)| *n == seg)
        .unwrap_or_else(|| panic!("no segment {seg}: {:?}", r.files))
        .1
        .iter()
        .map(|(_, _, len)| *len)
        .collect()
}

/// Forty members of 30 KiB and, as the 21st, one of 1.5 MiB, each of random bytes; with
/// `insert`, one byte inserted in the middle of the large member.
fn deck(seed: u64, insert: bool) -> Vec<u8> {
    let members: Vec<(String, Vec<u8>)> = (0..41u64)
        .map(|i| {
            if i == 20 {
                let mut b = data(seed + 1000, 3 * MIB / 2);
                if insert {
                    b.insert(3 * MIB / 4, 0x5a);
                }
                ("media/large.bin".to_owned(), b)
            } else {
                (format!("part/{i:03}.xml"), data(seed + i, 30 << 10))
            }
        })
        .collect();
    zip(&members)
}

/// A zip of members with a one-byte change in one of them (a byte inserted in the 1.5 MiB
/// member), written over the first version in a doc whose pack gives the zip's member
/// boundaries and in one whose pack gives none: with the hints the second version shares more
/// of the first's chunks (counted: the chunk rows both versions name). Both docs read both
/// versions back.
#[test]
fn with_zip_hints_a_one_byte_change_in_one_member_shares_more_chunks() {
    let f = fx("hints-share");
    let (a, b) = (deck(60, false), deck(60, true));
    let h = f.import_with(Arc::new(ZipPack), "h.zip", &a);
    let n = f.import("n.zip", &a);
    let (hv, nv) = (h.write(&b), n.write(&b));
    assert!(
        h.at(1) == a && h.at(hv) == b && n.at(1) == a && n.at(nv) == b,
        "control: both docs read both versions back"
    );
    let figures = |d: &Doc| {
        let r = f.report(d);
        let shared = r.chunks.iter().filter(|c| c.4 == 2).count();
        let newest = *row_segments(&r).iter().max().unwrap();
        let new_bytes: u64 = seg_lens(&r, newest).iter().map(|&l| u64::from(l)).sum();
        (shared, r.chunks.len(), new_bytes)
    };
    let (hs, ht, hb) = figures(&h);
    let (ns, nt, nb) = figures(&n);
    println!(
        "control: with hints {hs} of {ht} chunks shared, {hb} bytes new; without {ns} of {nt}, \
         {nb} bytes new ({} hints, a {}-byte zip)",
        zip_hints(&a).len(),
        a.len()
    );
    assert!(
        hs > ns,
        "row: with hints the second version shares more chunks ({hs} against {ns})"
    );
}

/// A zip of 100,000 tiny members (its member boundaries all given as hints, closer than the
/// chunk minimum) is cut at its hints, merged: about its length over the minimum in chunks,
/// nowhere near one per member.
#[test]
fn a_zip_of_100000_tiny_members_makes_about_len_over_min_chunks() {
    let f = fx("hints-tiny");
    let members: Vec<(String, Vec<u8>)> = (0..100_000u32)
        .map(|i| (format!("m{i:06}"), vec![(i % 251) as u8]))
        .collect();
    let z = zip(&members);
    let hints = zip_hints(&z);
    assert_eq!(
        hints.len(),
        100_000,
        "control: one hint per member boundary"
    );
    println!("control: a {}-byte zip, {} hints", z.len(), hints.len());
    let h = f.import_with(Arc::new(ZipPack), "tiny.zip", &z);
    let n = f.import("tiny-n.zip", &z);
    assert!(h.at(1) == z, "row: the version reads back");
    let (hc, nc) = (
        f.report(&h).chunks.len() as u64,
        f.report(&n).chunks.len() as u64,
    );
    let bound = z.len() as u64 / CHUNK_MIN;
    println!("chunks: {hc} with hints, {nc} without; length over minimum {bound}");
    assert!(
        seg_lens(&f.report(&h), 1) != seg_lens(&f.report(&n), 1),
        "row: the hints are taken (the cut differs from a no-hint cut)"
    );
    assert!(
        hc <= bound + 2,
        "row: {hc} chunks for 100,000 members, at most the length over the minimum ({bound})"
    );
}

/// Hints closer than the chunk minimum merge into one cut point (the first of a run kept): a
/// zip of 4 KiB members, every boundary a hint, cuts every fourth one, so its first hundred
/// chunks are each exactly 16 KiB. `MPC_CHUNK_PLANT=hints-unmerged` fails at the planted step.
#[test]
fn zip_hints_closer_than_the_minimum_merge_into_a_cut_every_16_kib() {
    let f = fx("hints-merge");
    let members: Vec<(String, Vec<u8>)> = (0..400u64)
        .map(|i| (format!("m{i:09}"), data(70 + i, 4096 - 40)))
        .collect();
    let z = zip(&members);
    let hints = zip_hints(&z);
    assert!(
        hints.windows(2).all(|w| w[1] - w[0] == 4096) && hints[0] == 4096,
        "control: the hints are 4 KiB apart: {:?}",
        &hints[..4]
    );
    println!(
        "control: {} hints 4 KiB apart in a {}-byte zip",
        hints.len(),
        z.len()
    );
    let d = planted(ChunkPlant::HintsUnmerged, || {
        f.import_with(Arc::new(ZipPack), "merge.zip", &z)
    });
    let lens = seg_lens(&f.report(&d), 1);
    println!("first cuts: {:?}", &lens[..6.min(lens.len())]);
    assert!(
        lens.len() > 100 && lens[..100].iter().all(|&l| u64::from(l) == CHUNK_MIN),
        "row: hints closer than the minimum merge: the first hundred chunks are 16 KiB each: {:?}",
        &lens[..6.min(lens.len())]
    );
    assert!(d.at(1) == z, "row: the version reads back");
}

/// Bad hints (out of order, zero, at or past the end, repeated) are ignored, all of them, and
/// counted: the cut is the cut with no hints. Good hints change it.
#[test]
fn bad_hints_are_ignored_and_cut_as_no_hints_would() {
    let f = fx("hints-bad");
    let z = data(80, 2 * MIB);
    let len = z.len() as u64;
    let plain = f.import("plain.bin", &z);
    let want = seg_lens(&f.report(&plain), 1);
    let good = f.import_with(Arc::new(FixedHints(vec![300_000, 700_000])), "good.bin", &z);
    assert!(
        seg_lens(&f.report(&good), 1) != want,
        "row: good hints change the cut"
    );
    assert_eq!(f.blobs.stats().unwrap().chunk_hints_ignored, 0);
    println!("control: good hints change the cut");
    let bad = [
        vec![300_000, 200_000],
        vec![0, 500_000],
        vec![500_000, len],
        vec![500_000, len + 7],
        vec![500_000, 500_000],
    ];
    for (i, hints) in bad.iter().enumerate() {
        let d = f.import_with(
            Arc::new(FixedHints(hints.clone())),
            &format!("bad{i}.bin"),
            &z,
        );
        assert_eq!(
            f.blobs.stats().unwrap().chunk_hints_ignored,
            i as u64 + 1,
            "row: the ignored hints {hints:?} are counted"
        );
        assert_eq!(
            seg_lens(&f.report(&d), 1),
            want,
            "row: bad hints {hints:?} cut as no hints would"
        );
        assert!(d.at(1) == z, "row: the version reads back");
    }
}

/// The hint cut is pure: the same bytes, through two docs and two pack values, cut the same
/// way; content cuts still happen inside spans, and no chunk is over the maximum (the 1.5 MiB
/// member is cut inside). `MPC_CHUNK_PLANT=hints-required` fails at the planted step.
#[test]
fn the_zip_hint_cut_is_pure_and_never_over_the_maximum() {
    let f = fx("hints-pure");
    let a = deck(90, false);
    let spans = zip_hints(&a).len() + 1;
    let (p1, p2, n) = planted(ChunkPlant::HintsRequired, || {
        (
            f.import_with(Arc::new(ZipPack), "p1.zip", &a),
            f.import_with(Arc::new(ZipPack), "p2.zip", &a),
            f.import("p-n.zip", &a),
        )
    });
    let (r1, r2) = (f.report(&p1), f.report(&p2));
    assert_eq!(
        seg_lens(&r1, 1),
        seg_lens(&r2, 1),
        "row: the same bytes cut the same way"
    );
    assert!(
        seg_lens(&r1, 1) != seg_lens(&f.report(&n), 1),
        "row: the hints are taken (the cut differs from a no-hint cut)"
    );
    println!("control: the same bytes cut the same way, at their hints");
    let lens = seg_lens(&r1, 1);
    let data_chunks = lens.len() - 1;
    let longest = lens.iter().copied().max().unwrap();
    println!("{data_chunks} chunks for {spans} spans; the longest {longest} bytes");
    assert!(
        u64::from(longest) <= CHUNK_MAX,
        "row: no chunk is over the maximum ({longest} bytes)"
    );
    assert!(
        data_chunks > spans,
        "row: content cuts happen inside spans ({data_chunks} chunks for {spans} spans)"
    );
    assert!(p1.at(1) == a && p2.at(1) == a, "row: both read back");
}

/// A version cut at its pack's hints reads back through a session whose pack gives none: the
/// read needs nothing from the pack.
#[test]
fn a_hinted_version_reads_back_through_a_pack_that_gives_no_hints() {
    let f = fx("hints-read");
    let (a, b) = (deck(95, false), deck(95, true));
    let d = f.import_with(Arc::new(ZipPack), "r.zip", &a);
    let v = d.write(&b);
    let n = f.import("r-n.zip", &a);
    assert!(
        seg_lens(&f.report(&d), 1) != seg_lens(&f.report(&n), 1),
        "row: the version was cut at its hints"
    );
    println!("control: the version was cut at its hints");
    drop((d, n));
    let f = restart(f);
    let d = f.reopen("r.zip");
    assert!(
        d.at(1) == a && d.at(v) == b && d.now() == b,
        "row: both versions read back through a pack with no hints"
    );
}
