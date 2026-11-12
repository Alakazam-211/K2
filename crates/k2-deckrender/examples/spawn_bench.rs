//! How long a render takes to start from a large host: spawn to the worker's
//! first response byte, from a parent of a few MB and from one holding ~1 GB
//! of touched memory (the K2 daemon's size), through `host::render` exactly
//! as the daemon calls it. The input is a few bytes that aren't a `.pptx`, so
//! the worker answers `invalid_input` as soon as it has read them: the time is
//! the spawn, the worker's startup (its limits, its fonts) and the reply.
//!
//! ```text
//! cargo build -p k2-deckrender --bin k2-deckrender --example spawn_bench
//! target/debug/examples/spawn_bench target/debug/k2-deckrender [ballast MiB = 1024] [runs = 40]
//! ```
//!
//! One JSON line per parent size: the parent's resident size (RSS on Linux,
//! the physical footprint on macOS), then the median, p95 (nearest rank),
//! min and max in ms. Only the public host API is used, so the same file
//! builds against the fork + pre-exec host (f4a3d0b86) for the before numbers.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_deckrender::host::{self, Limits, Request};
use k2_deckrender::proto::{FailureCode, SlideRef};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(bin) = args.first() else {
        eprintln!("usage: spawn_bench <k2-deckrender> [ballast MiB] [runs]");
        std::process::exit(2);
    };
    let bin = std::fs::canonicalize(bin).expect("the worker binary");
    let ballast_mib: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);

    // Warm the binary into the page cache, and the host's code paths.
    for _ in 0..3 {
        let _ = once(&bin);
    }
    report("small", 0, &bin, runs);

    let mut ballast = vec![0u8; ballast_mib << 20];
    for off in (0..ballast.len()).step_by(4096) {
        // SAFETY: in bounds; volatile so the pages are really touched.
        unsafe { ballast.as_mut_ptr().add(off).write_volatile(1) };
    }
    std::hint::black_box(&ballast);
    report("ballast", ballast_mib, &bin, runs);
    std::hint::black_box(&ballast);
}

fn once(bin: &Path) -> Duration {
    let req = Request::new(b"not a pptx", SlideRef::Index(0), 64);
    let started = Instant::now();
    let r = host::render(bin, &req, &Limits::default());
    let took = started.elapsed();
    match r {
        Err(f) if f.code == FailureCode::InvalidInput => took,
        Err(f) => panic!("expected invalid_input, got {}: {} | {}", f.code, f.detail, f.stderr),
        Ok(_) => panic!("garbage rendered"),
    }
}

fn report(label: &str, ballast_mib: usize, bin: &Path, runs: usize) {
    let parent = host::resident(std::process::id()).unwrap_or(0);
    let mut ms: Vec<f64> = (0..runs).map(|_| once(bin).as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let n = ms.len();
    let p95 = ms[(n * 95).div_ceil(100).max(1) - 1];
    let median = if n % 2 == 1 { ms[n / 2] } else { (ms[n / 2 - 1] + ms[n / 2]) / 2.0 };
    println!(
        "{{\"parent\":\"{label}\",\"ballast_mib\":{ballast_mib},\"parent_resident_mib\":{:.1},\"runs\":{n},\"median_ms\":{median:.2},\"p95_ms\":{p95:.2},\"min_ms\":{:.2},\"max_ms\":{:.2}}}",
        parent as f64 / (1u64 << 20) as f64,
        ms[0],
        ms[n - 1],
    );
}
