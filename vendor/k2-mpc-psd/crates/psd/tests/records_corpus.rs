//! The walker on the public test corpus (fetched by reference, `scripts/fetch-corpus.sh`):
//! streamed from a file with its heap measured, then compared with the full reader.
//! Prints counts only. Skips, loudly, when `PSD_PUBLIC_CORPUS` is not set; a file that
//! is listed but missing fails.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use k2_mpc_psd::differential::compare_with_full;
use k2_mpc_psd::{walk_records, LayerSource, WalkOptions};

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

#[test]
fn every_public_corpus_file_matches_the_full_reader() {
    let Some(dir) = std::env::var_os("PSD_PUBLIC_CORPUS") else {
        eprintln!("public corpus: skipped (PSD_PUBLIC_CORPUS is not set)");
        return;
    };
    let dir = Path::new(&dir);
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/manifest.tsv"
    ))
    .expect("check: the manifest is committed");
    let mut files = 0;
    let (mut layers_total, mut bytes_total) = (0usize, 0u64);
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let name = line
            .split('\t')
            .next()
            .expect("check: a manifest row has fields");
        if !name.ends_with(".psd") && !name.ends_with(".psb") {
            continue; // the one companion file (a PNG a linked smart object points at)
        }
        let path = dir.join(name);
        assert!(
            path.is_file(),
            "{name}: listed in the manifest, missing from the corpus directory"
        );
        let meta = std::fs::metadata(&path).expect("check: the corpus file is readable");
        let before = CURRENT.load(Relaxed);
        PEAK.store(before, Relaxed);
        let walked = {
            let mut src = File::open(&path).expect("check: the corpus file opens");
            walk_records(&mut src, &WalkOptions::default(), &mut ())
                .unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        let peak = PEAK.load(Relaxed) - before;
        layers_total += walked.layers.len();
        bytes_total += meta.len();
        files += 1;
        let source = match walked.source {
            LayerSource::None => "none",
            LayerSource::Section => "section",
            LayerSource::GlobalBlock { .. } => "block",
        };
        eprintln!(
            "corpus: {name} bytes={} layers={} source={source} peak_heap_bytes={peak}",
            meta.len(),
            walked.layers.len()
        );
        let bytes = std::fs::read(&path).expect("check: the corpus file reads");
        compare_with_full(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    assert_eq!(files, 18, "every PSD and PSB of the manifest is checked");
    eprintln!("public corpus: {files} files, {bytes_total} bytes, {layers_total} layers");
}
