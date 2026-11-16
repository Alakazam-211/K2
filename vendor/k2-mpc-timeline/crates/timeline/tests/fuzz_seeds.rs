//! The fuzz targets' properties (`k2_mpc_timeline::fuzzing`) on seed files and on every
//! truncation and a set of one-byte changes of each: a deterministic slice of what the fuzz
//! targets in `fuzz/` explore.

use k2_mpc_timeline::fuzzing;

/// Every prefix of `seed`, and each byte replaced by a few others.
fn mutations(seed: &[u8], f: fn(&[u8])) {
    f(seed);
    for n in 0..seed.len() {
        f(&seed[..n]);
    }
    let swaps: &[u8] = b"0-9\"{}[],:.e";
    for i in 0..seed.len() {
        for &b in swaps {
            if seed[i] == b {
                continue;
            }
            let mut m = seed.to_vec();
            m[i] = b;
            f(&m);
        }
    }
}

const OTIO_SEED: &str = include_str!("data/seed.otio");

const FCPXML_SEED: &str = include_str!("data/seed.fcpxml");

const PRPROJ_SEED: &str = include_str!("data/seed-prproj.xml");

#[test]
fn fcpxml_seed_and_its_mutations_keep_the_properties() {
    let doc =
        k2_mpc_timeline::fcpxml::read_fcpxml(FCPXML_SEED.as_bytes(), &fuzzing::limits()).unwrap();
    assert_eq!(doc.timelines.len(), 2);
    mutations(FCPXML_SEED.as_bytes(), fuzzing::fcpxml);
}

#[test]
fn prproj_seed_and_its_mutations_keep_the_properties() {
    let doc =
        k2_mpc_timeline::prproj::read_prproj(PRPROJ_SEED.as_bytes(), &fuzzing::limits()).unwrap();
    assert_eq!(doc.timelines[0].tracks[0].items.len(), 2);
    mutations(PRPROJ_SEED.as_bytes(), fuzzing::prproj);
}

#[test]
fn otio_seed_and_its_mutations_keep_the_properties() {
    // the seed itself reads
    let doc = k2_mpc_timeline::otio::read_otio(OTIO_SEED.as_bytes(), &fuzzing::limits()).unwrap();
    assert_eq!(doc.timelines[0].tracks[0].items.len(), 4);
    mutations(OTIO_SEED.as_bytes(), fuzzing::otio);
}
