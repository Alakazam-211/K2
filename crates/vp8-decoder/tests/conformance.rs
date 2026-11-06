//! Conformance against the WebM project's VP8 test vectors: every shown
//! frame's I420 MD5 must match the published `.md5` file.
//!
//! Needs the `conformance` feature and `VP8_TEST_VECTORS` set to a directory
//! filled by `scripts/fetch-test-vectors.sh`. Without the directory the test
//! fails; it never skips.

mod common {
    pub mod md5;
}

use common::md5::Md5;
use std::path::PathBuf;
use vp8_decoder::ivf::IvfReader;
use vp8_decoder::{Decoder, Picture};

fn picture_md5(p: &Picture<'_>) -> String {
    let mut m = Md5::new();
    let (w, h) = (p.width as usize, p.height as usize);
    for r in 0..h {
        m.update(&p.y[r * p.y_stride..r * p.y_stride + w]);
    }
    let (cw, ch) = (p.chroma_width() as usize, p.chroma_height() as usize);
    for plane in [p.u, p.v] {
        for r in 0..ch {
            m.update(&plane[r * p.uv_stride..r * p.uv_stride + cw]);
        }
    }
    m.hex()
}

fn vectors_dir() -> PathBuf {
    let dir = std::env::var_os("VP8_TEST_VECTORS")
        .expect("set VP8_TEST_VECTORS to a directory filled by scripts/fetch-test-vectors.sh");
    PathBuf::from(dir)
}

fn vector_names() -> Vec<String> {
    let manifest = include_str!("vectors.sha256");
    manifest
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|n| n.ends_with(".ivf"))
        .map(|n| n.trim_end_matches(".ivf").to_string())
        .collect()
}

/// Decodes one vector; returns a description of the first mismatch, if any,
/// and the number of frames checked.
fn check(dir: &std::path::Path, name: &str) -> (Option<String>, usize) {
    let ivf = std::fs::read(dir.join(format!("{name}.ivf")))
        .unwrap_or_else(|e| panic!("{name}.ivf: {e}"));
    let md5s = std::fs::read_to_string(dir.join(format!("{name}.ivf.md5")))
        .unwrap_or_else(|e| panic!("{name}.ivf.md5: {e}"));
    let expected: Vec<&str> = md5s
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    let reader = IvfReader::new(&ivf).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut dec = Decoder::new();
    let mut shown = 0usize;
    for (i, frame) in reader.enumerate() {
        let frame = frame.unwrap_or_else(|e| panic!("{name}: {e}"));
        match dec.decode(frame.data) {
            Ok(Some(p)) => {
                let got = picture_md5(&p);
                match expected.get(shown) {
                    Some(want) if *want == got => {}
                    Some(want) => {
                        return (
                            Some(format!("frame {i} (shown {shown}): md5 {got}, want {want}")),
                            shown,
                        );
                    }
                    None => {
                        return (
                            Some(format!(
                                "frame {i}: more shown frames than the .md5 file lists"
                            )),
                            shown,
                        )
                    }
                }
                shown += 1;
            }
            Ok(None) => {}
            Err(e) => return (Some(format!("frame {i}: decode error: {e}")), shown),
        }
    }
    if shown != expected.len() {
        return (
            Some(format!(
                "{shown} shown frames, the .md5 file lists {}",
                expected.len()
            )),
            shown,
        );
    }
    (None, shown)
}

#[test]
fn every_test_vector_matches_its_md5s() {
    let dir = vectors_dir();
    let names = vector_names();
    assert!(
        names.len() >= 18,
        "the manifest lists the comprehensive set at least"
    );
    let mut failures = Vec::new();
    let mut frames = 0;
    for name in &names {
        let (fail, n) = check(&dir, name);
        frames += n;
        match fail {
            None => eprintln!("[conformance] pass {name} ({n} frames)"),
            Some(why) => {
                eprintln!("[conformance] FAIL {name}: {why}");
                failures.push(format!("{name}: {why}"));
            }
        }
    }
    eprintln!(
        "[conformance] {}/{} vectors passed, {frames} frames matched",
        names.len() - failures.len(),
        names.len()
    );
    assert!(
        failures.is_empty(),
        "{} vectors failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Corrupted streams must give errors or pictures, never a panic: every
/// vector, with random bytes flipped and frames cut short.
#[test]
fn corrupted_vectors_never_panic() {
    let dir = vectors_dir();
    let mut seed = 0x9e37_79b9u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut frames = 0;
    let mut errors = 0;
    for name in vector_names() {
        let ivf = std::fs::read(dir.join(format!("{name}.ivf")))
            .unwrap_or_else(|e| panic!("{name}.ivf: {e}"));
        let originals: Vec<Vec<u8>> = IvfReader::new(&ivf)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .map(|f| f.unwrap_or_else(|e| panic!("{name}: {e}")).data.to_vec())
            .collect();
        for round in 0..4 {
            let mut dec = Decoder::new();
            for f in &originals {
                let mut f = f.clone();
                match round {
                    // Flip a few bytes after the uncompressed header.
                    0 | 1 => {
                        for _ in 0..1 + rnd() % 4 {
                            if f.len() > 10 {
                                let i = 10 + rnd() as usize % (f.len() - 10);
                                f[i] ^= 1 << (rnd() % 8);
                            }
                        }
                    }
                    // Cut the frame short.
                    2 => {
                        let keep = rnd() as usize % (f.len() + 1);
                        f.truncate(keep);
                    }
                    // Flip anywhere, the header included.
                    _ => {
                        let i = rnd() as usize % f.len();
                        f[i] ^= 1 << (rnd() % 8);
                    }
                }
                frames += 1;
                if dec.decode(&f).is_err() {
                    errors += 1;
                }
            }
        }
    }
    eprintln!("[robustness] {frames} corrupted frames decoded without a panic ({errors} errors)");
}
