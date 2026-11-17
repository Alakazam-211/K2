//! The fuzz properties on seeds: our drawings, cut and mangled. Each read is a typed refusal or
//! a result that keeps its properties, never a panic.

use k2_mpc_dxf::testgen::{self, Ver};

#[test]
fn seeds_keep_the_properties() {
    let mut runs = 0usize;
    for (_, bytes) in testgen::byte_variants().into_iter().step_by(5) {
        k2_mpc_dxf::fuzzing::read(&bytes);
        runs += 1;
        for cut in (0..bytes.len()).step_by(211) {
            k2_mpc_dxf::fuzzing::read(&bytes[..cut]);
            runs += 1;
        }
        let mut m = bytes.clone();
        for i in (0..m.len()).step_by(211) {
            m[i] = m[i].wrapping_add(17);
        }
        k2_mpc_dxf::fuzzing::read(&m);
        runs += 1;
    }
    let big = testgen::nested_blocks(Ver::R12).build().bytes;
    k2_mpc_dxf::fuzzing::read(&big);
    println!("seeds_keep_the_properties: {runs} reads");
    assert!(runs > 100, "only {runs} reads");
}
