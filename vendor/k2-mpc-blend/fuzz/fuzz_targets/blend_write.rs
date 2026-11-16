//! The writer on any bytes: the same refusal twice, or the same file twice, changed only inside
//! the fields it patched (`k2_mpc_blend::fuzzing::write`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_blend::fuzzing::write(data));
