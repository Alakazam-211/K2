//! The fuzz targets' properties (`k2_mpc_fab::fuzzing`) on the seed files and on every
//! truncation and a set of one-byte changes of each: a deterministic slice of what the fuzz
//! targets in `fuzz/` explore. Hand-written regressions for fuzz findings go here too.

mod support;

use k2_mpc_fab::fuzzing;
use support::{BOARD, BOARD_XNC, COMMON};

/// Every prefix of `seed`, and each byte replaced by a few others.
fn mutations(seed: &[u8], swaps: &[u8], f: fn(&[u8])) {
    f(seed);
    for n in 0..seed.len() {
        f(&seed[..n]);
    }
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

#[test]
fn photoplot_seed_and_its_mutations_keep_the_properties() {
    let m = k2_mpc_fab::photoplot::read(BOARD.as_bytes(), &fuzzing::limits()).unwrap();
    assert_eq!(m.objects.len(), 14);
    mutations(BOARD.as_bytes(), b"0-*%,XYDG\n", fuzzing::photoplot);
}

#[test]
fn drill_seed_and_its_mutations_keep_the_properties() {
    let m = k2_mpc_fab::drill::read(BOARD_XNC.as_bytes(), &fuzzing::limits()).unwrap();
    assert_eq!(m.hits.len(), 4);
    mutations(BOARD_XNC.as_bytes(), b"0-.;XYTGM\n", fuzzing::drill);
}

#[test]
fn common_drill_seed_and_its_mutations_keep_the_properties() {
    let c = k2_mpc_fab::drill::common::read(COMMON.as_bytes(), &fuzzing::limits()).unwrap();
    assert_eq!(c.drill.hits.len(), 2);
    mutations(COMMON.as_bytes(), b"0-.;%,XYTGMLZ\n", fuzzing::drill_common);
}
