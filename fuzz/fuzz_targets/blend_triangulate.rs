//! The triangulator on any polygons: exactly n - 2 triangles of each polygon's own corners, in
//! a bounded number of ear tests, the same twice (`k2_mpc_blend::fuzzing::triangulate`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_blend::fuzzing::triangulate(data));
