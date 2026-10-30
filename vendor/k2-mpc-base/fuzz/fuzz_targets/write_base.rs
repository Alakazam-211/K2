//! `write --base`'s merge (`merge::merge3`) on line-shaped inputs that share their start,
//! judged independently (`k2_mpc_base_fuzz::writebase`): a clean merge holds only whole
//! lines of base, ours or theirs and passes the line-LCS rule. A conflict is fine.
#![no_main]

use k2_mpc_base::text::merge::{self, Merge};
use k2_mpc_base_fuzz::writebase::{judge, Case};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|case: Case| {
    let (base, ours, theirs) = case.texts();
    if let Merge::Clean(c) = merge::merge3(&base, &ours, &theirs) {
        if let Err(e) = judge(&base, &ours, &theirs, &c.text) {
            panic!(
                "write --base merged cleanly into a bad text: {e}\nbase {base:?}\nours {ours:?}\n\
                 theirs {theirs:?}\ngot {:?}",
                c.text
            );
        }
    }
});
