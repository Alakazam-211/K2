//! The carrier-storage measurement (made before the file cap was set): what the kit keeps per
//! write-back of a live BLEND file. Each version keeps its carrier, and every write-back makes a
//! new whole-file carrier, so storage grows with the file's size. A measurement, not a row:
//! `scripts/measure.sh` runs it on a node in a release build.
//!
//! For each of a plain and a zstd file of about 8 MiB and just under the 32 MiB cap (one object
//! whose mesh carries bytes that don't compress): 100 rounds of one `object place` and a
//! write-back, through the kit's session with its default config; the blob folder's and the
//! store file's growth, the versions cut, and the write-back's time (p50, p99).

mod live_support;

use std::time::Instant;

use live_support::{fx, run_based, sized_scene, stdout};

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn mib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

#[test]
#[ignore = "a measurement: scripts/measure.sh runs it"]
fn carrier_storage_per_write_back() {
    let rounds: usize = std::env::var("STORAGE_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    for (how, target) in [
        ("plain", 8 << 20),
        ("zstd", 8 << 20),
        ("plain", (32 << 20) - (64 << 10)),
        ("zstd", (32 << 20) - (64 << 10)),
    ] {
        let bytes = sized_scene(target, how);
        let f = fx("storage", |_| {});
        f.write(&bytes);
        let t = Instant::now();
        let s = f.open();
        let open_ms = t.elapsed().as_secs_f64() * 1000.0;
        let a = f.agent(&s);
        let (blob0, store0) = (f.blob_bytes(), f.store_bytes());
        let mut wb = Vec::new();
        for i in 0..rounds {
            let x = format!("{},0,0", i + 1);
            let o = run_based(&s, a, &["object", "place", "@obj:1", "--loc", &x]).unwrap();
            assert_eq!(o.exit, 0, "{}", stdout(&o));
            let t = Instant::now();
            let out = s.flush(a).expect("the write-back");
            wb.push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(out.written, "round {i}: nothing written");
        }
        let versions = s.versions(a).expect("versions").len();
        let (blob1, store1) = (f.blob_bytes(), f.store_bytes());
        let grown = (blob1 - blob0) + store1.saturating_sub(store0);
        println!(
            "storage: {how} file {} bytes ({:.2} MiB): open {open_ms:.1} ms; {rounds} write-backs: blob folder {:.2} -> {:.2} MiB, store file {:.2} -> {:.2} MiB, {versions} versions; kept per write-back {:.2} MiB = {:.2}x the file; write-back p50 {:.1} ms p99 {:.1} ms",
            bytes.len(),
            mib(bytes.len() as u64),
            mib(blob0),
            mib(blob1),
            mib(store0),
            mib(store1),
            mib(grown) / rounds as f64,
            grown as f64 / rounds as f64 / bytes.len() as f64,
            pct(&mut wb.clone(), 0.5),
            pct(&mut wb, 0.99),
        );
    }
}
