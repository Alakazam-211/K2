//! Any bytes through `k2_mpc_board_fuzz::board_json`: see it for the properties.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    k2_mpc_board_fuzz::board_json(data);
});
