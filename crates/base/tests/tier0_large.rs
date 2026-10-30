//! Tier 0 streams: a 50 MB file is imported, read, replaced, written back and copied out
//! for an export (`file_at_to`, `file_now_to`) without ever sitting in memory. Its own test binary, so the counting allocator sees only this test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::session::{
    AnyFile, AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size() as isize, Ordering::SeqCst) + l.size() as isize;
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size() as isize, Ordering::SeqCst);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            let now = LIVE.fetch_add(n as isize - l.size() as isize, Ordering::SeqCst) + n as isize
                - l.size() as isize;
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Reset the peak to the current level; returns that level.
fn mark() -> isize {
    let now = LIVE.load(Ordering::SeqCst);
    PEAK.store(now, Ordering::SeqCst);
    now
}

fn peak_since(base: isize) -> isize {
    PEAK.load(Ordering::SeqCst) - base
}

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

/// `len` deterministic pseudo-random bytes, generated as they are read.
struct Gen {
    left: u64,
    x: u64,
}
impl Read for Gen {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.left as usize);
        for b in &mut buf[..n] {
            self.x ^= self.x << 13;
            self.x ^= self.x >> 7;
            self.x ^= self.x << 17;
            *b = self.x as u8;
        }
        self.left -= n as u64;
        Ok(n)
    }
}

const SIZE: u64 = 50 << 20;
/// Far below the 50 MB: 64 KiB buffers, the store's pages, bookkeeping (measured: under
/// 100 KB on every phase).
const BOUND: isize = 1 << 20;

#[test]
fn a_50_mb_file_streams_in_bounded_memory() {
    let dir = std::env::temp_dir().join(format!("k2mb-t0-large-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("ws")).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let (ws, file) = (dir.join("ws"), dir.join("ws/video.mov"));
    {
        let mut f = File::create(&file).unwrap();
        io::copy(&mut Gen { left: SIZE, x: 1 }, &mut f).unwrap();
        f.flush().unwrap();
    }
    let host = Arc::new(H(AtomicU64::new(1_000)));
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store, host.clone(), Config::default()).with_blob_store(blobs);
    let key = DocKey {
        space: "ws".into(),
        path: "video.mov".into(),
    };

    // import: the file is hashed and copied into the blob store
    let base = mark();
    let s = mgr
        .open_blob_file(Arc::new(AnyFile), &key, &file, &ws)
        .unwrap();
    let p = Participant::new("alice", "Alice", "#000", ParticipantKind::Person, "tab");
    let c = s.join(p, Stay::Held).unwrap().conn;
    eprintln!("import: peak {} bytes above the baseline", peak_since(base));
    assert!(
        peak_since(base) < BOUND,
        "import peaked at {} bytes",
        peak_since(base)
    );

    // read: streamed and verified
    let base = mark();
    let mut r = s.read(c).unwrap();
    assert_eq!(r.info.size, SIZE);
    let n = io::copy(&mut r.reader, &mut io::sink()).unwrap();
    assert_eq!(n, SIZE);
    drop(r);
    eprintln!("read: peak {} bytes above the baseline", peak_since(base));
    assert!(
        peak_since(base) < BOUND,
        "read peaked at {} bytes",
        peak_since(base)
    );

    // write 50 MB of new content from a generator, then the write-back to the file
    let base = mark();
    let v = s.status().unwrap().version;
    let w = s.write(c, &mut Gen { left: SIZE, x: 2 }, v).unwrap();
    assert!(w.changed && w.written);
    assert_eq!(w.size, SIZE);
    eprintln!("write: peak {} bytes above the baseline", peak_since(base));
    assert!(
        peak_since(base) < BOUND,
        "write peaked at {} bytes",
        peak_since(base)
    );

    // the file holds the new content (compared streaming, too)
    let mut on_disk = File::open(&file).unwrap();
    let mut want = Gen { left: SIZE, x: 2 };
    let (mut a, mut b) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = on_disk.read(&mut a).unwrap();
        want.read_exact(&mut b[..n]).unwrap();
        assert_eq!(a[..n], b[..n]);
        if n == 0 {
            break;
        }
    }
    assert_eq!(want.left, 0);

    // exports: each version copied out as it streams, verified at the end, in bounded memory
    for (what, vid, x) in [("file_at_to v1", Some(1), 1), ("file_now_to", None, 2)] {
        let mut cmp = Same::new(Gen { left: SIZE, x });
        let base = mark();
        let copy = match vid {
            Some(v) => s.file_at_to(c, v, &mut cmp).unwrap(),
            None => s.file_now_to(c, &mut cmp).unwrap(),
        };
        eprintln!("{what}: peak {} bytes above the baseline", peak_since(base));
        assert!(
            peak_since(base) < BOUND,
            "{what} peaked at {} bytes",
            peak_since(base)
        );
        assert_eq!(copy.bytes, SIZE, "{what}");
        assert_eq!(cmp.want.left, 0, "{what}: every byte, in order");
        assert!(cmp.largest <= 256 << 10, "{what}: written in chunks");
    }
    drop(s);
    mgr.release().unwrap();
    let _ = fs::remove_dir_all(&dir);
}

/// A writer that checks each byte against a generator (no copy of the file is kept).
struct Same {
    want: Gen,
    buf: Vec<u8>,
    largest: usize,
}

impl Same {
    fn new(want: Gen) -> Self {
        Same {
            want,
            buf: vec![0u8; 1 << 20],
            largest: 0,
        }
    }
}

impl Write for Same {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = b.len().min(self.buf.len());
        self.want.read_exact(&mut self.buf[..n])?;
        assert_eq!(&b[..n], &self.buf[..n], "the copy's bytes");
        self.largest = self.largest.max(n);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
