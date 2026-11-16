//! Measurements of a live scene's costs at a large shape: one scene, 100 collections, 2,000
//! objects (`SCALE_OBJECTS=N` for N objects in 100 collections), through the kit's session. A
//! measurement, not a row: `scripts/measure.sh` runs it on a node in a release build.
//!
//! - **open**: the file opened live (its decode, the import, the carrier kept), heap after;
//! - **verbs**: `object place` and `object visibility`, p50 and p99 of the call; a placement to
//!   a watcher's event, a figure to watch (read against 50 ms p99; no suite asserts it: a
//!   debug suite on a loaded node times nothing reliably);
//! - **reads**: `collection show` of a 200-object page; `object show`;
//! - **the view index**: the first build, unchanged reads, after a placement;
//! - **write-back**: the kit's flush after one placement; the pack's parts outside the kit: the
//!   carrier's read, and the splice of 1, 10 and 100 placed objects, for the file plain, gzip
//!   and zstd;
//! - **repeats**: heap after 10,000 placements of one object.
//!
//! Heap is counted by this test binary's own allocator (bytes live, and the peak). The kit's
//! test-build checks (its `differential` feature, on for this repo's tests) are switched off
//! first, as a host runs.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

mod live_support;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::session::Pack;
use k2_mpc_blend::live::{model, splice, BlendLive, LiveLimits, OBJS, OBJ_PROPS};
use live_support::{args, big_scene, fx, run};

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

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn mib(b: usize) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// `content` with objects `o1..=on` placed at `x`.
fn placed(content: &[(String, Neutral)], n: usize, x: f32) -> Vec<(String, Neutral)> {
    let mut c = content.to_vec();
    for (r, root) in &mut c {
        if let (true, Neutral::Map(m)) = (r == OBJ_PROPS, root) {
            for i in 1..=n {
                let k = format!("o{i}\x1fxform");
                let old = m.get(&k).and_then(Value::as_str).unwrap_or("").to_owned();
                let mut parts: Vec<String> = old.split(',').map(str::to_owned).collect();
                parts[0] = x.to_string();
                m.insert(k, Value::str(parts.join(",")));
            }
        }
    }
    c
}

#[test]
#[ignore = "a measurement: scripts/measure.sh runs it"]
fn scale_open_verbs_reads_view_write_back() {
    let objects: usize = std::env::var("SCALE_OBJECTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let colls = 100;
    // as a host runs: the kit's test-build checks of the live view and the view index off
    k2_mpc_base::differential::set_view_checks(false);
    let bytes = big_scene(colls, objects / colls, false);
    let pack = BlendLive::default();
    let limits = LiveLimits::default();
    let scene = model::read(&bytes, &limits).expect("the scene reads");
    println!(
        "scale: scene {} collections, {} objects, {} elements and fields, file {} bytes",
        scene.colls.len(),
        scene.objs.len(),
        scene.entries(),
        bytes.len()
    );
    drop(scene);

    // open
    let f = fx("scale", |_| {});
    f.write(&bytes);
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let t = Instant::now();
    let s = f.open();
    let open_ms = ms(t);
    let a = f.agent(&s);
    let w = f.person(&s);
    let heap_open = LIVE.load(Ordering::Relaxed) - base;
    println!(
        "scale open: {open_ms:.1} ms; heap live {:.2} MiB, peak {:.2} MiB (from before the open)",
        mib(heap_open),
        mib(PEAK.load(Ordering::Relaxed) - base)
    );

    // verbs
    let rx = s.subscribe(w).expect("a watch");
    let version = |s: &k2_mpc_base::session::Session| s.status().unwrap().version.to_string();
    let mut place = Vec::new();
    let mut to_event = Vec::new();
    for i in 0..200 {
        let addr = format!("@obj:{}", 1 + (i * 7) % objects);
        let x = format!("{},1,2", i as f32 * 0.5);
        let v = version(&s);
        let argv = args(&["object", "place", &addr, "--loc", &x, "--base", &v]);
        let t = Instant::now();
        let o = s.run_tool(a, OBJS, &argv, b"").unwrap();
        place.push(ms(t));
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the watcher's event");
        to_event.push(ms(t));
        assert_eq!(o.exit, 0);
    }
    let mut vis = Vec::new();
    for i in 0..200 {
        let addr = format!("@obj:{}", 1 + (i * 11) % objects);
        let on = if i % 2 == 0 { "off" } else { "on" };
        let v = version(&s);
        let argv = args(&["object", "visibility", &addr, "--render", on, "--base", &v]);
        let t = Instant::now();
        let o = s.run_tool(a, OBJS, &argv, b"").unwrap();
        vis.push(ms(t));
        assert_eq!(o.exit, 0);
    }
    let _ = rx.try_iter().count();
    println!(
        "scale verbs: object place p50 {:.3} ms p99 {:.3} ms; to the watcher's event p50 {:.3} ms p99 {:.3} ms (a figure to watch, read against 50); object visibility p50 {:.3} ms p99 {:.3} ms",
        pct(&mut place.clone(), 0.5),
        pct(&mut place, 0.99),
        pct(&mut to_event.clone(), 0.5),
        pct(&mut to_event, 0.99),
        pct(&mut vis.clone(), 0.5),
        pct(&mut vis, 0.99),
    );

    // reads
    let mut show = Vec::new();
    let mut one = Vec::new();
    for i in 0..20 {
        let c = format!("@coll:{}", 2 + i % colls);
        let t = Instant::now();
        let o = run(&s, a, &["collection", "show", &c]).unwrap();
        show.push(ms(t));
        assert_eq!(o.exit, 0);
        let t = Instant::now();
        let o = run(&s, a, &["object", "show", &format!("@obj:{}", 1 + i * 13)]).unwrap();
        one.push(ms(t));
        assert_eq!(o.exit, 0);
    }
    println!(
        "scale reads: collection show ({} objects a page) p50 {:.3} ms p99 {:.3} ms; object show p50 {:.3} ms p99 {:.3} ms",
        objects / colls,
        pct(&mut show.clone(), 0.5),
        pct(&mut show, 0.99),
        pct(&mut one.clone(), 0.5),
        pct(&mut one, 0.99),
    );

    // the verbs that read every object, with what they print held fixed: `scene list` of one
    // collection, `collection show` of a 5-object page, `object rename` to a name another
    // object has (refused after its check of every name)
    let mut list1 = Vec::new();
    let mut show5 = Vec::new();
    let mut taken = Vec::new();
    for i in 0..50 {
        let t = Instant::now();
        let o = run(&s, a, &["scene", "list", "--limit", "1"]).unwrap();
        list1.push(ms(t));
        assert_eq!(o.exit, 0);
        let c = format!("@coll:{}", 2 + i % colls);
        let t = Instant::now();
        let o = run(&s, a, &["collection", "show", &c, "--limit", "5"]).unwrap();
        show5.push(ms(t));
        assert_eq!(o.exit, 0);
        let v = version(&s);
        let argv = args(&[
            "object",
            "rename",
            "@obj:1",
            "--name",
            "Obj_0_1",
            "--break-links",
            "--base",
            &v,
        ]);
        let t = Instant::now();
        let o = s.run_tool(a, OBJS, &argv, b"").unwrap();
        taken.push(ms(t));
        assert_eq!(o.exit, 2, "a taken name is refused");
    }
    println!(
        "scale whole-doc verbs: scene list --limit 1 p50 {:.3} ms p99 {:.3} ms; collection show --limit 5 p50 {:.3} ms p99 {:.3} ms; object rename to a taken name p50 {:.3} ms p99 {:.3} ms",
        pct(&mut list1.clone(), 0.5),
        pct(&mut list1, 0.99),
        pct(&mut show5.clone(), 0.5),
        pct(&mut show5, 0.99),
        pct(&mut taken.clone(), 0.5),
        pct(&mut taken, 0.99),
    );

    // the view index
    let t = Instant::now();
    let ix = s.view_index(a).unwrap().expect("a view index");
    let first = ms(t);
    let units = ix.units.len();
    let mut same = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let _ = s.view_index(a).unwrap();
        same.push(ms(t));
    }
    let v = version(&s);
    let argv = args(&["object", "place", "@obj:1", "--loc", "5,5,5", "--base", &v]);
    s.run_tool(a, OBJS, &argv, b"").unwrap();
    let t = Instant::now();
    let _ = s.view_index(a).unwrap();
    let moved = ms(t);
    println!(
        "scale view index: {units} units; first build {first:.2} ms; unchanged p50 {:.3} ms; after a placement {moved:.2} ms",
        pct(&mut same, 0.5)
    );

    // write-back through the kit
    let t = Instant::now();
    let out = s.flush(a).unwrap();
    println!(
        "scale write-back: the kit's flush after {} changes: {:.1} ms (written {})",
        401,
        ms(t),
        out.written
    );

    // the pack's parts, plain, gzip, zstd
    let content = pack.decode(&bytes).unwrap();
    let pairing = pack.pairing(&content.roots, &content.codec).unwrap();
    let mut zstd = Vec::new();
    for chunk in bytes.chunks(k2_mpc_blend::write::ZSTD_FRAME) {
        zstd.extend(ruzstd::encoding::compress_to_vec(
            chunk,
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
    }
    for (how, file) in [
        ("plain", bytes.clone()),
        ("gzip", k2_mpc_blend::testgen::gzip(&bytes)),
        ("zstd", zstd),
    ] {
        let t = Instant::now();
        let _ = model::read(&file, &limits).unwrap();
        let read = ms(t);
        let mut parts = vec![format!("carrier read {read:.1} ms")];
        for n in [1usize, 10, 100] {
            let c = placed(&content.roots, n, 42.5);
            let t = Instant::now();
            let out = splice::encode_from_paired(&file, &pairing, &c, &limits).unwrap();
            parts.push(format!("splice of {n} {:.1} ms", ms(t)));
            assert_ne!(out, file);
        }
        println!(
            "scale write-back parts: {how} ({} bytes): {}",
            file.len(),
            parts.join(", ")
        );
    }

    // repeats: one object nudged 10,000 times
    let before = LIVE.load(Ordering::Relaxed);
    let t = Instant::now();
    for i in 0..10_000 {
        let v = version(&s);
        let x = format!("{},0,0", i as f32 * 0.001);
        let argv = args(&["object", "place", "@obj:2", "--loc", &x, "--base", &v]);
        let o = s.run_tool(a, OBJS, &argv, b"").unwrap();
        assert_eq!(o.exit, 0);
    }
    let took = ms(t);
    let after = LIVE.load(Ordering::Relaxed);
    println!(
        "repeats: 10,000 placements of one object: {:.1} s; heap {:.2} MiB -> {:.2} MiB (+{:.2} MiB); kit estimate {} bytes",
        took / 1000.0,
        mib(before - base),
        mib(after - base),
        mib(after.saturating_sub(before)),
        s.status().unwrap().estimated_bytes
    );
}
