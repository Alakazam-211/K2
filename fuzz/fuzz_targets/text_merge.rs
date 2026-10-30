//! The kit's text diff and three-way merge (`text::diff`, `text::merge`) on arbitrary
//! base/ours/theirs: see `k2_mpc_base_fuzz::merge` for the properties.
#![no_main]

use k2_mpc_base_fuzz::{merge, Text};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|x: (Text, Text, Text)| {
    merge::check(&x.0.render(), &x.1.render(), &x.2.render());
});
