//! The common-export drill reading on any bytes: a typed refusal, or a model that keeps its
//! properties and is reported not conforming.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| k2_mpc_fab::fuzzing::drill_common(data));
