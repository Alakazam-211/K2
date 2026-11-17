//! The tokenizer, the model, the index and the read verbs on any bytes: a typed refusal, or a
//! result that keeps its properties.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_dxf::fuzzing::read(data));
