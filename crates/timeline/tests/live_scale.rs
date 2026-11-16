//! Measurements of the live pack at size, ignored by default (`scripts/measure.sh` runs them in
//! a release build on a node).
//!
//! The first: one timeline of 10 tracks of 200 clips (2,000 items), 600 markers, 300 media
//! (`SCALE_ITEMS=N` sets the item count). Heap live after the open and its peak, from a counting
//! allocator; the open; `clip move`, `clip trim`, `clip add` and `clip delete` p50 and p99 to
//! the watcher's event; a `clip list` page of 200; the view index first, unchanged and after a
//! move; the carrier's read; the splice of 1, 10 and 50 moved clips. The second: the heap after
//! 10,000 sets of one clip's `geom`.

mod live_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use k2_mpc_base::doc::Edit;
use k2_mpc_base::elem::{MapOp, Value};
use k2_mpc_base::session::Pack;
use k2_mpc_timeline::live::{model, splice, ticks, LiveLimits, TimelineLive, ITEMS, ITEM_PROPS};
use live_support::{args, big, field, fx, run, set_field, stdout};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: forwarded to the system allocator with the caller's layout
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: the pointer came from `alloc` with this layout
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static A: Counting = Counting;

fn mib(b: usize) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(v.len() - 1)]
}

const F: i64 = 10_584_000_000;

#[test]
#[ignore = "a measurement: scripts/measure.sh runs it"]
fn a_long_timeline() {
    // a measurement, not a check: the kit's comparisons of kept state are off on this thread
    k2_mpc_base::differential::set_view_checks(false);
    let items: usize = std::env::var("SCALE_ITEMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_000);
    let per = items / 10;
    let bytes = big(10, per, 300, 600);
    // over the kit's default (25,000), which this shape reaches; not `usize::MAX`, which the
    // kit's limit check reads as -1
    let mut f = fx("scale", |c| c.keyed_max_entries = 1 << 40);
    // past the live tier's default bounds when the shape asks for more
    let limits = LiveLimits {
        max_items: (10 * per).max(2_500),
        max_file_bytes: 1 << 30,
        ..LiveLimits::default()
    };
    f.pack = std::sync::Arc::new(TimelineLive::new(limits.clone()));
    f.write(&bytes);
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let s = f.open();
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let a = f.agent(&s);
    let w = f.person(&s);
    let rx = s.subscribe(w).unwrap();
    let entries: usize = TimelineLive::new(limits.clone())
        .decode(&bytes)
        .unwrap()
        .roots
        .iter()
        .map(|(_, n)| match n {
            k2_mpc_base::doc::Neutral::List(l) => l.len(),
            k2_mpc_base::doc::Neutral::Map(m) => m.len(),
            _ => 0,
        })
        .sum();
    println!(
        "scale shape: {} tracks, {} items, {entries} elements and fields, file {} bytes; open {:.1} ms; heap {:.1} MiB live, {:.1} MiB peak",
        10,
        10 * per,
        bytes.len(),
        open_ms,
        mib(LIVE.load(Ordering::Relaxed).saturating_sub(base)),
        mib(PEAK.load(Ordering::Relaxed).saturating_sub(base))
    );
    let timed = |verb: &[&str]| -> f64 {
        let v = run(&s, a, &["timeline", "info"]).unwrap().version;
        let mut all = args(verb);
        all.push("--base".into());
        all.push(v.to_string());
        let t = Instant::now();
        let o = s.run_tool(a, ITEMS, &all, b"").unwrap();
        assert_eq!(o.exit, 0, "{verb:?}: {}", stdout(&o));
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        t.elapsed().as_secs_f64() * 1000.0
    };
    let mut mv = Vec::new();
    let mut tr = Vec::new();
    for i in 0..200 {
        let item = format!("@item:{}", 1 + (i * 37) % (10 * per));
        mv.push(timed(&["clip", "move", &item, "--to", "+1f"]));
        mv.push(timed(&["clip", "move", &item, "--to", "-1f"]));
        tr.push(timed(&["clip", "trim", &item, "--out", "-1f"]));
        tr.push(timed(&["clip", "trim", &item, "--out", "+1f"]));
    }
    let mut add = Vec::new();
    let mut del = Vec::new();
    let end = per as i64 * 25 + 10;
    // the kit's own scopes, counted over the adds and deletes only
    let _ = k2_mpc_base::differential::profile_take();
    for i in 0..100 {
        let at = format!("{}f", end + 30 * i);
        add.push(timed(&[
            "clip", "add", "--media", "@med:1", "--track", "@trk:1", "--at", &at, "--in", "0f",
            "--dur", "24f",
        ]));
        let k = format!("@item:{}", 10 * per + 1 + i as usize);
        del.push(timed(&["clip", "delete", &k]));
    }
    let mut prof = k2_mpc_base::differential::profile_take();
    prof.sort_by(|a, b| b.2.total_cmp(&a.2));
    let top: Vec<String> = prof
        .iter()
        .take(8)
        .map(|(n, c, secs)| format!("{n} {c}x {:.1} ms", secs * 1000.0))
        .collect();
    println!(
        "scale add and delete, 100 each, the kit's scopes: {}",
        top.join("; ")
    );
    println!(
        "scale clip move p50 {:.3} ms p99 {:.3} ms; clip trim p50 {:.3} ms p99 {:.3} ms; clip add p50 {:.3} ms p99 {:.3} ms; clip delete p50 {:.3} ms p99 {:.3} ms (to the watcher's event)",
        pct(&mut mv, 0.5),
        pct(&mut mv, 0.99),
        pct(&mut tr, 0.5),
        pct(&mut tr, 0.99),
        pct(&mut add, 0.5),
        pct(&mut add, 0.99),
        pct(&mut del, 0.5),
        pct(&mut del, 0.99)
    );
    let mut list = Vec::new();
    for _ in 0..50 {
        let t = Instant::now();
        let o = run(&s, a, &["clip", "list", "@trk:3"]).unwrap();
        list.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(o.exit, 0);
    }
    println!(
        "scale clip list (a page of 200) p50 {:.3} ms p99 {:.3} ms",
        pct(&mut list, 0.5),
        pct(&mut list, 0.99)
    );
    let t = Instant::now();
    s.view_index(w).unwrap().unwrap();
    let first = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    s.view_index(w).unwrap().unwrap();
    let same = t.elapsed().as_secs_f64() * 1000.0;
    timed(&["clip", "move", "@item:2", "--to", "+1f"]);
    let t = Instant::now();
    s.view_index(w).unwrap().unwrap();
    let after = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "scale view index: first {first:.2} ms, unchanged {same:.3} ms, after a move {after:.2} ms"
    );
    // the write-back, at the pack's hooks
    let pack = TimelineLive::new(limits.clone());
    let t = Instant::now();
    model::read(&bytes, &limits).unwrap();
    let read_ms = t.elapsed().as_secs_f64() * 1000.0;
    let d = pack.decode(&bytes).unwrap().roots;
    let pairing = pack.pairing(&d, k2_mpc_timeline::live::CODEC).unwrap();
    let mut line = format!("scale write-back: carrier read {read_ms:.1} ms");
    for n in [1usize, 10, 50] {
        let mut c = d.clone();
        for i in 0..n {
            let k = format!("i{}", 1 + i * 7);
            let g = field(&c, ITEM_PROPS, &k, "geom").unwrap();
            let (st, du) = ticks::parse_pair(&g).unwrap();
            set_field(
                &mut c,
                ITEM_PROPS,
                &k,
                "geom",
                &ticks::pair_text(st + F, du),
            );
        }
        let mut ms = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            splice::encode_from_paired(&bytes, &pairing, &c, &limits).unwrap();
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        line.push_str(&format!(
            "; splice of {n} moved: p50 {:.1} ms",
            pct(&mut ms, 0.5)
        ));
    }
    println!("{line}");
}

#[test]
#[ignore = "a measurement: scripts/measure.sh runs it"]
fn one_clip_nudged_ten_thousand_times() {
    // a measurement, not a check: the kit's comparisons of kept state are off on this thread
    k2_mpc_base::differential::set_view_checks(false);
    let bytes = big(10, 200, 300, 600);
    let f = fx("repeat", |c| c.keyed_max_entries = 1 << 40);
    f.write(&bytes);
    let s = f.open();
    let p = f.person(&s);
    let before = LIVE.load(Ordering::Relaxed);
    let t = Instant::now();
    for i in 0..10_000i64 {
        let start = (i % 2) * F;
        s.edit(
            p,
            vec![Edit::Map {
                root: ITEM_PROPS.into(),
                ops: vec![MapOp::Set {
                    key: "i1\x1fgeom".into(),
                    value: Value::str(ticks::pair_text(start, 24 * F)),
                }],
            }],
            None,
        )
        .unwrap();
    }
    println!(
        "repeat: 10,000 sets of one clip's geom: {:.1} s; heap grew {:.2} MiB",
        t.elapsed().as_secs_f64(),
        mib(LIVE.load(Ordering::Relaxed).saturating_sub(before))
    );
}
