//! The OTIO reader on any bytes: a typed refusal, or a model that keeps its properties.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_timeline::fuzzing::otio(data));
