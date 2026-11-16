//! The reader and the scene model on any bytes: a typed refusal, or a file and a model that
//! keep their properties (`k2_mpc_blend::fuzzing::read`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_blend::fuzzing::read(data));
