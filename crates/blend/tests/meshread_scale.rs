//! Measurements of the mesh read, not rows: `scripts/measure.sh` runs them by name in a release
//! build on a build node, and nothing asserts a time. Each shape is read in two parts, timed
//! apart: the decode (decompression, the reader, the scene model, the live scene) and the mesh
//! phase (objects, arrays, triangles, the K2SCENE1 written). Heap is counted by this test
//! binary's own allocator: the peak of a whole read from before it, against the output's size.
//!
//! Shapes (each stored plain, gzip, and as zstd frames):
//! - **grid**: one object, one mesh of `MESHREAD_GRID` x `MESHREAD_GRID` quads (900: near the
//!   live tier's 32 MiB cap stored plain);
//! - **shared**: 2,000 objects on 200 meshes of 40 x 40 quads (6.4 million triangles drawn,
//!   under the 8 million cap);
//! - **many**: `MESHREAD_MANY` objects (10,000: about 21 MiB stored plain, under the cap), each
//!   with its own one-quad mesh.
//!
//! The heap at the stored cap (`mesh_read_heap_at_the_stored_cap`): the largest shapes under the
//! live tier's 32 MiB stored cap (one mesh of 1,000 x 1,000 quads plain; 15,000 objects of their
//! own one-quad meshes plain; one mesh of 2,000 x 2,000 quads, the 8 million triangle cap, stored
//! zstd when that fits), each read once: the peak the counting allocator saw, from before the
//! decode to after the read, against the figure the read works out before it allocates its
//! meshes (`MeshReport::heap`, checked against `MeshReadLimits::max_heap_bytes`). It fails when a
//! figure is under its peak.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use k2_mpc_blend::meshread::read::{decode, read_decoded};
use k2_mpc_blend::meshread::MeshReadLimits;
use k2_mpc_blend::testgen::{gzip, zstd_frames, MeshLayouts, MeshSpec, ObjectSpec, SceneBuilder};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged; the counters are plain atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = CURRENT.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CURRENT.fetch_sub(l.size(), Relaxed);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// An `n` x `n` grid of quads in the plane z = 0.
fn grid(n: u32) -> MeshSpec {
    let w = n + 1;
    let positions = (0..w * w)
        .map(|i| [(i % w) as f32, (i / w) as f32, 0.0])
        .collect();
    let mut faces = Vec::with_capacity((n * n) as usize);
    for y in 0..n {
        for x in 0..n {
            let a = y * w + x;
            faces.push(vec![a, a + 1, a + 1 + w, a + w]);
        }
    }
    MeshSpec {
        positions,
        faces,
        ..MeshSpec::default()
    }
}

/// A file of `objects` mesh objects spread over `meshes` meshes, each an `n` x `n` grid.
fn shape(objects: usize, meshes: usize, n: u32) -> Vec<u8> {
    let mut b = SceneBuilder::meshes(403, MeshLayouts::LAYERS);
    b.file_global(Some("S"));
    let names: Vec<String> = (0..objects).map(|i| format!("o{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    b.scene("S", &refs, &[], None);
    for (i, name) in names.iter().enumerate() {
        b.object(
            name,
            &ObjectSpec {
                data: Some(format!("ME:m{}", i % meshes)),
                ty: 1,
                rotmode: 1,
                loc: [i as f32, 0.0, 0.0],
                ..ObjectSpec::default()
            },
        );
    }
    let g = grid(n);
    for m in 0..meshes {
        b.mesh_with(&format!("m{m}"), &g, MeshLayouts::LAYERS);
    }
    b.bytes()
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn mib(b: usize) -> f64 {
    b as f64 / f64::from(1 << 20)
}

#[test]
#[ignore = "a measurement: scripts/measure.sh runs it"]
fn mesh_read_decode_and_mesh_phases() {
    let runs = env("MESHREAD_RUNS", 7);
    let shapes = [
        ("grid", shape(1, 1, env("MESHREAD_GRID", 900) as u32)),
        ("shared", shape(2000, 200, 40)),
        (
            "many",
            shape(
                env("MESHREAD_MANY", 10_000),
                env("MESHREAD_MANY", 10_000),
                1,
            ),
        ),
    ];
    let limits = MeshReadLimits::default();
    let mut measured = 0;
    for (name, plain) in &shapes {
        for (how, bytes) in [
            ("plain", plain.clone()),
            ("gzip", gzip(plain)),
            ("zstd", zstd_frames(plain, 8 << 20)),
        ] {
            let (mut dec, mut mesh, mut peak, mut out) = (Vec::new(), Vec::new(), 0usize, 0usize);
            let mut objects = 0;
            for _ in 0..runs {
                let base = CURRENT.load(Relaxed);
                PEAK.store(base, Relaxed);
                let t = Instant::now();
                let d = decode(&bytes, &limits).expect("decodes");
                dec.push(ms(t));
                let t = Instant::now();
                let r = read_decoded(&d, &limits).expect("reads");
                mesh.push(ms(t));
                drop(d);
                peak = peak.max(PEAK.load(Relaxed) - base);
                out = r.k2scene.len();
                objects = r.report.objects;
            }
            assert!(objects > 0, "{name}: objects drawn");
            dec.sort_by(f64::total_cmp);
            mesh.sort_by(f64::total_cmp);
            println!(
                "meshread perf: {name} {how}: stored {:.2} MiB, {objects} objects, output {:.2} \
                 MiB; decode p50 {:.1} ms max {:.1} ms; mesh phase p50 {:.1} ms max {:.1} ms; \
                 heap peak {:.2} MiB ({:.2}x the output); {runs} runs",
                mib(bytes.len()),
                mib(out),
                dec[runs / 2],
                dec[runs - 1],
                mesh[runs / 2],
                mesh[runs - 1],
                mib(peak),
                peak as f64 / out as f64
            );
            measured += 1;
        }
    }
    println!("meshread perf: done, {measured} shapes and storages");
}

#[test]
#[ignore = "a measurement: scripts/measure.sh runs it"]
fn mesh_read_heap_at_the_stored_cap() {
    let limits = MeshReadLimits::default();
    let cap = limits.live.max_file_bytes as usize;
    let grid = shape(1, 1, 1000);
    let many = shape(15_000, 15_000, 1);
    let big = zstd_frames(&shape(1, 1, 2000), 8 << 20);
    let mut worst = (0usize, "");
    let mut measured = 0;
    for (name, bytes) in [
        ("grid 1000 plain", grid),
        ("many 15000 plain", many),
        ("grid 2000 zstd", big),
    ] {
        if bytes.len() > cap {
            println!(
                "meshread heap: {name}: stored {:.2} MiB, over the cap: not read",
                mib(bytes.len())
            );
            continue;
        }
        let base = CURRENT.load(Relaxed);
        PEAK.store(base, Relaxed);
        let d = decode(&bytes, &limits).expect("decodes");
        let r = read_decoded(&d, &limits).expect("reads");
        drop(d);
        let peak = PEAK.load(Relaxed) - base;
        let figure = r.report.heap as usize;
        println!(
            "meshread heap: {name}: stored {:.2} MiB, {} objects, output {:.2} MiB; peak {:.2} \
             MiB, worked out {:.2} MiB ({:.2}x the peak)",
            mib(bytes.len()),
            r.report.objects,
            mib(r.k2scene.len()),
            mib(peak),
            mib(figure),
            figure as f64 / peak as f64
        );
        assert!(figure >= peak, "{name}: the figure is under the peak");
        if peak > worst.0 {
            worst = (peak, name);
        }
        measured += 1;
    }
    assert!(measured >= 2, "shapes read: {measured}");
    println!(
        "meshread heap: worst peak {:.2} MiB ({}), the limit {:.0} MiB; {measured} shapes",
        mib(worst.0),
        worst.1,
        mib(limits.max_heap_bytes as usize)
    );
}
