//! The outside-edit path, stateful: a file-backed session driven by a sequence of
//! participant edits, changes that move no byte, flushes, ticks, restarts and outside edits.
//! See `k2_mpc_base_fuzz::outside` for the properties.
#![no_main]

use k2_mpc_base_fuzz::outside::{regressions, run, Case};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|case: Case| {
    // the fixed cases, once per process
    static REGRESSIONS: std::sync::Once = std::sync::Once::new();
    REGRESSIONS.call_once(regressions);
    run(&case);
});
