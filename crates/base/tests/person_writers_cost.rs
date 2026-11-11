//! Person-writer reuse's costs and savings (`Config::reuse_person_writers`;
//! `docs/design/person-writer-reuse-r0.md` §6, the ruling on Q4), measured, each printing
//! `MEASURE` lines. Ignored: run on a compute node, release,
//! `--ignored --nocapture --test-threads 1` (with `--features native` for the native column).
//!
//! - `measure_bytes_per_writer`: R8's real bytes per writer. One history of `E` appends, made
//!   once by `E` writers (a writer per connection) and once by `W` writers taking turns (reuse);
//!   consecutive appends are always by different writers, so both docs hold the same blocks and
//!   differ only in their writers. The heap of each doc loaded from its state (counted by a
//!   global allocator), and its state's bytes; the difference over the writer difference is the
//!   cost of one writer. Yrs, and the native engine with the feature.
//! - `measure_join_leave_latency`: join, first edit and leave of a person's connection, with
//!   reuse off (no device key: as before) and on (a device key, released), p50/p99/max. With
//!   reuse on, a join and a leave open no store transaction; the lease's row rides in its first
//!   change's commit and the release's in the next commit or the close.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use k2_mpc_base::session::{
    AuditEvent, Config, EditOptions, Host, JoinOptions, Pack, Participant, ParticipantKind, Rights,
    SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::Hunk;
use k2_mpc_base::{Doc, Edit};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Ordering::SeqCst);
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
            LIVE.fetch_add(n as isize - l.size() as isize, Ordering::SeqCst);
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

/// The allocator counts the whole process.
static LOCK: Mutex<()> = Mutex::new(());

const MD: TextFilePack = TextFilePack::new("markdown", 1 << 20);
const ROOT: &str = TextFilePack::ROOT;

/// Writer IDs as sessions mint them (random, 53 bits): their varints are as long as in
/// production.
fn writer(i: u64) -> u64 {
    let mut x = i.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5bd1_e995;
    x ^= x >> 29;
    (x & ((1 << 53) - 1)).max(2) | (1 << 40)
}

/// `appends` appends to one root, the `k`-th by writer `writer(k % writers)`, on `doc`.
fn history(doc: &mut Doc, appends: u64, writers: u64) {
    let mut len = 0;
    for k in 0..appends {
        let add = format!("w{k} ");
        let e = vec![Edit::Text {
            root: ROOT.into(),
            hunks: vec![Hunk {
                start: len,
                end: len,
                insert: add.clone(),
            }],
        }];
        doc.edit_as(writer(k % writers), &e).unwrap();
        len += add.len();
    }
}

/// The heap a doc loaded from `state` holds, and the state's bytes.
fn loaded_heap(load: impl Fn(&[u8]) -> Doc, state: &[u8]) -> (isize, usize) {
    let before = LIVE.load(Ordering::SeqCst);
    let d = load(state);
    let held = LIVE.load(Ordering::SeqCst) - before;
    drop(d);
    (held, state.len())
}

fn per_writer(
    engine: &str,
    new: impl Fn() -> Doc,
    load: impl Fn(&[u8]) -> Doc + Copy,
    appends: u64,
    reused: u64,
) {
    let mut a = new();
    history(&mut a, appends, appends);
    let mut b = new();
    history(&mut b, appends, reused);
    assert_eq!(a.text(ROOT).unwrap().len(), b.text(ROOT).unwrap().len());
    let (ha, sa) = loaded_heap(load, &a.encode_state().unwrap());
    let (hb, sb) = loaded_heap(load, &b.encode_state().unwrap());
    let dw = (appends - reused) as f64;
    println!(
        "MEASURE bytes_per_writer engine={engine} appends={appends} writers_off={appends} \
         writers_on={reused} heap_off={ha} heap_on={hb} heap_per_writer={:.1} state_off={sa} \
         state_on={sb} state_per_writer={:.1}",
        (ha - hb) as f64 / dw,
        (sa as f64 - sb as f64) / dw
    );
}

#[test]
#[ignore = "a measurement: run on a compute node, release, --ignored --nocapture"]
fn measure_bytes_per_writer() {
    let _g = LOCK.lock().unwrap();
    for (appends, reused) in [(1_000u64, 10u64), (10_000, 10), (10_000, 100)] {
        per_writer(
            "yrs",
            || Doc::new(MD.schema()).unwrap(),
            |s| Doc::load(MD.schema(), s).unwrap(),
            appends,
            reused,
        );
    }
}

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn pct(v: &mut [Duration], p: f64) -> f64 {
    v.sort();
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i].as_secs_f64() * 1e6
}

fn line(what: &str, mode: &str, v: &mut [Duration]) {
    let max = v.iter().max().unwrap().as_secs_f64() * 1e6;
    println!(
        "MEASURE latency op={what} reuse={mode} n={} p50_us={:.0} p99_us={:.0} max_us={max:.0}",
        v.len(),
        pct(v, 0.5),
        pct(v, 0.99)
    );
}

fn latency(mode: &str, on: bool, n: usize) {
    let dir = std::env::temp_dir().join(format!("k2mpc-pw-lat-{mode}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut c = Config::default();
    c.reuse_person_writers = on;
    c.edit_rate_per_s = 1 << 30;
    c.edit_burst = 1 << 30;
    let store = Arc::new(DocStore::open(dir.join("d.redb")).unwrap());
    let mgr = SessionManager::new(store, Arc::new(H(AtomicU64::new(1))), c);
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr.open(Arc::new(MD), &key, Some(b"# T\n\n")).unwrap();
    let opts = if on {
        JoinOptions::new().device("laptop")
    } else {
        JoinOptions::new()
    };
    let (mut join, mut edit, mut leave) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..n {
        let p = Participant::new("ada", "Ada", "#7c3aed", ParticipantKind::Person, "tab");
        let t = Instant::now();
        let j = s.join_with(p, Stay::Held, &opts).unwrap();
        join.push(t.elapsed());
        let v = s.read(j.conn).unwrap();
        let at = v.text_by_root[0].1.len();
        let t = Instant::now();
        s.edit_at_base(
            j.conn,
            ROOT,
            v.version,
            vec![Hunk {
                start: at,
                end: at,
                insert: format!("{i} "),
            }],
            &EditOptions::new(),
        )
        .unwrap();
        edit.push(t.elapsed());
        let t = Instant::now();
        if on {
            s.release(j.conn).unwrap();
        } else {
            s.leave(j.conn).unwrap();
        }
        leave.push(t.elapsed());
    }
    line("join", mode, &mut join);
    line("first_edit", mode, &mut edit);
    line("leave", mode, &mut leave);
    drop(s);
    mgr.release().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "a measurement: run on a compute node, release, --ignored --nocapture"]
fn measure_join_leave_latency() {
    let _g = LOCK.lock().unwrap();
    let n: usize = std::env::var("MPC_PW_LAT_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    // each twice, alternating, so a warm cache favours neither
    for _ in 0..2 {
        latency("off", false, n);
        latency("on", true, n);
    }
}
