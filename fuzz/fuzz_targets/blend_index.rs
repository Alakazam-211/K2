//! The pack's index entry point on any bytes: the same refusal twice, or the same well-formed
//! draft twice (`k2_mpc_blend::fuzzing::index`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_blend::fuzzing::index(data));
