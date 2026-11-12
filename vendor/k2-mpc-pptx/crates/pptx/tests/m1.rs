//! M1, a measurement of a live deck's costs: heap, verb p99 and `slide show` time on a live deck of
//! 2,500 shapes (50 slides of 50 text boxes), through the kit's session; then the kit's
//! memory estimate and the write-back's time (the one rendering per sync point, through the
//! carrier hook K1); then one-element list changes, `shape add`, `shape order` and `shape
//! delete`, p50 and p99 of the verb alone (the deck ends with the shapes it started with). A
//! measurement, not a row: run it on a node in a release build,
//! `cargo test --release -p k2-mpc-pptx --test m1 -- --ignored --nocapture`; `M1_SLIDES=N`
//! measures a deck of N slides of 50 text boxes instead. It feeds the keyed limit (25,000
//! elements and fields is the kit's provisional default).
//!
//! Heap is counted by this test binary's own allocator (bytes live, and the peak).
//!
//! **Phases** (`M1 phase:` lines, last): where a call's time goes, by phase timers around the
//! pack's own parts run outside the kit (no kit code changed): a verb's closure on a view of
//! the same content (the rest of the call is the kit's), a full copy of that content into a
//! view (what a view of every root costs), and the write-back's parts (the carrier's decode,
//! and the splice of 1, 10 and 50 changed shapes).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

mod common;

use common::live::{args, fx, run};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::{ConnId, Session, View};
use k2_mpc_base::tools::VerbCtx;
use k2_mpc_pptx::live::model::{self, LiveDeck};
use k2_mpc_pptx::live::{splice, verbs, CODEC, SHAPES};
use k2_mpc_pptx::testgen::{self, DeckSpec, Shape, Slide};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded to the system allocator unchanged; only counters are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let now = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = System.realloc(p, l, new);
        if !q.is_null() {
            if new >= l.size() {
                let now = LIVE.fetch_add(new - l.size(), Ordering::Relaxed) + (new - l.size());
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - new, Ordering::Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const PER_SLIDE: u32 = 50;

/// Slides in the deck: 50, or `M1_SLIDES`.
fn slides() -> u32 {
    std::env::var("M1_SLIDES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
}

fn deck() -> Vec<u8> {
    let slides = (0..slides())
        .map(|i| Slide {
            id: 256 + i,
            layout: 1,
            shapes: (0..PER_SLIDE)
                .map(|j| {
                    Shape::text_box(
                        2 + j,
                        (
                            i64::from(j) * 200_000,
                            i64::from(j) * 100_000,
                            1_000_000,
                            400_000,
                        ),
                        &format!("Shape {j} on slide {i}: some words of text"),
                    )
                })
                .collect(),
            notes: None,
            hidden: false,
            animate: None,
            creation_id: None,
        })
        .collect();
    testgen::deck(&DeckSpec::new(slides))
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

fn mib(b: usize) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

#[test]
#[ignore = "a measurement: run on a node, release build"]
fn m1_heap_verb_p99_and_slide_show() {
    let n_slides = slides();
    let bytes = deck();
    let d = model::read(&bytes, &common::limits()).expect("the deck reads");
    let shapes: usize = d.slides.iter().map(|s| s.shapes.len()).sum();
    let entries = d.entries();
    drop(d);
    println!(
        "M1: deck {} slides, {shapes} shapes, {entries} elements and fields, file {} bytes",
        n_slides,
        bytes.len()
    );
    let f = fx("m1", |c| c.keyed_max_entries = 1_000_000);
    f.write(&bytes);
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let s = f.open();
    let a = f.agent(&s);
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let after_open = LIVE.load(Ordering::Relaxed);
    println!(
        "M1: open live {open_ms:.1} ms, heap after open {:.1} MiB over the baseline (peak {:.1} MiB)",
        mib(after_open.saturating_sub(base)),
        mib(PEAK.load(Ordering::Relaxed).saturating_sub(base))
    );
    let mut verb = Vec::new();
    let mut read = Vec::new();
    for i in 0..500u32 {
        let sid = 256 + (i * 7) % n_slides;
        let fid = 2 + (i * 13) % PER_SLIDE;
        let t = Instant::now();
        let v = run(&s, a, &["slide", "list", "--limit", "1"])
            .expect("a read")
            .version;
        read.push(t.elapsed().as_secs_f64() * 1000.0);
        let x = (1_000 + i * 10).to_string();
        let call = args(&[
            "shape",
            "move",
            &format!("@sld:{sid}/sp:{fid}"),
            "--x",
            &x,
            "--base",
            &v.to_string(),
        ]);
        let t = Instant::now();
        let o = s.run_tool(a, SHAPES, &call, b"").expect("the move");
        verb.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(o.exit, 0, "{}", String::from_utf8_lossy(&o.stdout));
    }
    println!(
        "M1: shape move (the verb alone), 500 calls: p50 {:.3} ms, p99 {:.3} ms, max {:.3} ms",
        pct(&mut verb, 0.5),
        pct(&mut verb, 0.99),
        pct(&mut verb, 1.0)
    );
    println!(
        "M1: the agent's read before each (slide list --limit 1, a keyed read: since the kit's K1 it renders nothing), 500 calls: p50 {:.3} ms, p99 {:.3} ms",
        pct(&mut read, 0.5),
        pct(&mut read, 0.99)
    );
    let mut show = Vec::new();
    for i in 0..100u32 {
        let sid = 256 + (i * 3) % n_slides;
        let t = Instant::now();
        let o = run(&s, a, &["slide", "show", &format!("@sld:{sid}")]).expect("slide show");
        show.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(o.exit, 0);
    }
    println!(
        "M1: slide show (50 shapes), 100 calls: p50 {:.3} ms, p99 {:.3} ms",
        pct(&mut show, 0.5),
        pct(&mut show, 0.99)
    );
    let now = LIVE.load(Ordering::Relaxed);
    println!(
        "M1: heap after 500 moves and 100 shows {:.1} MiB over the baseline (peak {:.1} MiB)",
        mib(now.saturating_sub(base)),
        mib(PEAK.load(Ordering::Relaxed).saturating_sub(base))
    );
    // the kit's own memory estimate (what its pool budgets loaded docs by)
    let est = s.status().expect("status").estimated_bytes as usize;
    println!(
        "M1: the kit's memory estimate for the doc {:.1} MiB ({entries} elements and fields)",
        mib(est)
    );
    // the write-back: one rendering per sync point, the moves since spliced into the carrier
    let t = Instant::now();
    let fl = s.flush(a).expect("the write-back");
    let first_ms = t.elapsed().as_secs_f64() * 1000.0;
    assert!(fl.written, "the 500 moves are written");
    println!("M1: write-back (flush) of the 500 moves: {first_ms:.1} ms");
    for per in [1u32, 10, 50] {
        let mut wb = Vec::new();
        for r in 0..5u32 {
            for k in 0..per {
                let i = r * per + k;
                let sid = 256 + (i * 11) % n_slides;
                let fid = 2 + (i * 17) % PER_SLIDE;
                let x = (2_000 + i * 10 + per * 100_000).to_string();
                let v = run(&s, a, &["slide", "list", "--limit", "1"])
                    .expect("a read")
                    .version;
                let call = args(&[
                    "shape",
                    "move",
                    &format!("@sld:{sid}/sp:{fid}"),
                    "--x",
                    &x,
                    "--base",
                    &v.to_string(),
                ]);
                let o = s.run_tool(a, SHAPES, &call, b"").expect("the move");
                assert_eq!(o.exit, 0, "{}", String::from_utf8_lossy(&o.stdout));
            }
            let t = Instant::now();
            assert!(s.flush(a).expect("the write-back").written);
            wb.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "M1: write-back (flush) after {per} moves, 5 rounds: p50 {:.1} ms, max {:.1} ms",
            pct(&mut wb, 0.5),
            pct(&mut wb, 1.0)
        );
    }
    list_changes(&s, a, n_slides);
    assert!(
        s.flush(a).expect("the write-back").written,
        "the list changes are written"
    );
    let now = LIVE.load(Ordering::Relaxed);
    println!(
        "M1: heap after the write-backs and the list changes {:.1} MiB over the baseline (peak {:.1} MiB)",
        mib(now.saturating_sub(base)),
        mib(PEAK.load(Ordering::Relaxed).saturating_sub(base))
    );
    phases(&s.read(a).expect("the content").content, &f.bytes());
}

/// One edit verb as the agent `a`, with the version of a read just made as its base (the read
/// isn't timed): the verb's time in ms and its output. Exit 0 required.
fn verb_alone(s: &Session, a: ConnId, call: &[&str]) -> (f64, String) {
    let v = run(s, a, &["slide", "list", "--limit", "1"])
        .expect("a read")
        .version;
    let mut all = args(call);
    all.push("--base".into());
    all.push(v.to_string());
    let t = Instant::now();
    let o = s.run_tool(a, SHAPES, &all, b"").expect("the verb");
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let out = String::from_utf8_lossy(&o.stdout).trim().to_owned();
    assert_eq!(o.exit, 0, "{call:?}: {out}");
    (ms, out)
}

/// The one-element list changes, 200 calls each, the verb alone timed: `shape add` (a text box
/// in front of a slide's others), `shape order` (a shape to the front, then the same shape to
/// the back: every call moves it) and `shape delete` (each added box, so the deck ends with
/// the shapes it started with).
fn list_changes(s: &Session, a: ConnId, n_slides: u32) {
    const N: u32 = 200;
    // one bracket per line: the verb, timed alone, and what each call does
    let line = |what: &str, how: &str, v: &mut Vec<f64>| {
        println!(
            "M1: {what} (the verb alone; {how}), {} calls: p50 {:.3} ms, p99 {:.3} ms, max {:.3} ms",
            v.len(),
            pct(v, 0.5),
            pct(v, 0.99),
            pct(v, 1.0)
        );
    };
    let mut add = Vec::new();
    let mut added = Vec::new();
    for i in 0..N {
        let sid = 256 + (i * 7) % n_slides;
        let at = format!(
            "{},{},1000000,400000",
            50_000 + i * 1_000,
            60_000 + i * 1_000
        );
        let (ms, out) = verb_alone(
            s,
            a,
            &[
                "shape",
                "add",
                &format!("@sld:{sid}"),
                "--kind",
                "textbox",
                "--at",
                &at,
                "--text",
                &format!("Added box {i}"),
            ],
        );
        assert!(
            out.starts_with(&format!("@sld:{sid}/sp:")),
            "shape add printed {out:?}"
        );
        add.push(ms);
        added.push(out);
    }
    line(
        "shape add",
        "a text box in front of a slide's others",
        &mut add,
    );
    let mut order = Vec::new();
    for k in 0..N {
        // pairs: a shape to the front, then the same shape to the back. Never the slide's last
        // shape at the start (sp:51), so neither call finds it already there.
        let i = k / 2;
        let sid = 256 + (i * 3) % n_slides;
        let fid = 2 + (i * 13) % (PER_SLIDE - 1);
        let to = if k % 2 == 0 { "--front" } else { "--back" };
        let (ms, out) = verb_alone(
            s,
            a,
            &["shape", "order", &format!("@sld:{sid}/sp:{fid}"), to],
        );
        assert_ne!(out, "already there", "shape order {to} moved nothing");
        order.push(ms);
    }
    line(
        "shape order",
        "to the front, then the same shape to the back",
        &mut order,
    );
    let mut delete = Vec::new();
    for addr in &added {
        let (ms, out) = verb_alone(s, a, &["shape", "delete", addr]);
        assert_eq!(&out, addr, "shape delete printed {out:?}");
        delete.push(ms);
    }
    line("shape delete", "each added box", &mut delete);
}

/// Time `f` `n` times: (p50, p99) in ms.
fn timed(n: usize, mut f: impl FnMut(usize)) -> (f64, f64) {
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let t = Instant::now();
        f(i);
        v.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    (pct(&mut v, 0.5), pct(&mut v, 0.99))
}

/// The phase timers: the pack's parts of a read, a verb and a write-back, run outside the kit
/// on the live `content` and the file the kit last wrote (`carrier`, which holds it).
fn phases(content: &[(String, Neutral)], carrier: &[u8]) {
    let pack = k2_mpc_pptx::PptxLive::default();
    let limits = common::limits();
    let verb = |name: &str| {
        verbs::VERBS
            .iter()
            .find(|v| v.name() == name)
            .copied()
            .expect("the verb")
    };
    let (list, mv) = (verb("slide list"), verb("shape move"));
    let view = View::of_content(content);
    let ctx = VerbCtx::new(&view, b"", &pack, CODEC);
    let list_args = args(&["--limit", "1"]);
    let ok = list.parse_structured(&list_args).map(|go| go(&ctx).is_ok());
    let (p50, p99) = timed(200, |_| {
        let go = list.parse_structured(&list_args).expect("parse");
        std::hint::black_box(go(&ctx).is_ok());
    });
    println!(
        "M1 phase: the pack's closure of slide list --limit 1 on a view of the content (ok: {ok:?}), 200 calls: p50 {p50:.3} ms, p99 {p99:.3} ms"
    );
    let mv_args = |i: usize| args(&["@sld:256/sp:2", "--x", &(7_000 + i).to_string()]);
    let ok = mv.parse_structured(&mv_args(0)).map(|go| go(&ctx).is_ok());
    let (p50, p99) = timed(200, |i| {
        let go = mv.parse_structured(&mv_args(i)).expect("parse");
        std::hint::black_box(go(&ctx).is_ok());
    });
    println!(
        "M1 phase: the pack's closure of shape move (its ops, not applied) on a view of the content (ok: {ok:?}), 200 calls: p50 {p50:.3} ms, p99 {p99:.3} ms"
    );
    let (p50, p99) = timed(50, |_| {
        std::hint::black_box(View::of_content(content));
    });
    println!(
        "M1 phase: a full copy of the content into a view (every root), 50 calls: p50 {p50:.3} ms, p99 {p99:.3} ms"
    );
    let (p50, p99) = timed(10, |_| {
        std::hint::black_box(model::read(carrier, &limits).expect("the carrier reads"));
    });
    println!(
        "M1 phase: the carrier's decode (model::read, once per write-back), 10 calls: p50 {p50:.1} ms, p99 {p99:.1} ms"
    );
    let (p50, _) = timed(5, |_| {
        let out = splice::encode_from(carrier, content, CODEC, &limits).expect("untouched");
        assert_eq!(out.len(), carrier.len());
    });
    println!(
        "M1 phase: the splice, untouched (decode, compare, the carrier back), 5 calls: p50 {p50:.1} ms"
    );
    let live = LiveDeck::from_content(content).expect("the content reads");
    for k in [1usize, 10, 50] {
        let mut to = live.clone();
        let mut n = 0;
        'all: for s in &mut to.slides {
            for x in &mut s.shapes {
                if let Some(g) = x.geom.as_mut() {
                    g.x += 1;
                    n += 1;
                    if n == k {
                        break 'all;
                    }
                }
            }
        }
        let to = to.to_content();
        let (p50, _) = timed(3, |_| {
            splice::encode_from(carrier, &to, CODEC, &limits).expect("the splice");
        });
        println!(
            "M1 phase: the splice of {k} changed shapes (decode, one step each, the read-back guard), 3 calls: p50 {p50:.1} ms"
        );
    }
}
