//! Any bytes through `k2_mpc_board_fuzz::raw_ops`: see it for the properties.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    k2_mpc_board_fuzz::raw_ops(data);
});
