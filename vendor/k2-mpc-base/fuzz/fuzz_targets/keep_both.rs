//! The kit's outside merge (v0.9.5, `merge3_keep_both`) as a pure function, against the
//! independent oracles of `k2_mpc_base_fuzz::keepboth`: its aligned sides
//! (`check_aligned`: whole lines of the closed texts, v0.9.7), no merge, clean or not,
//! splices a line from both sides or joins two lines (`no_splice_no_join`), loses a line or
//! brings a deleted one back (`check_lines`), a clean merge is write --base's with the end
//! merged on its own (`clean_outside`, an independent construction), and a conflicting one
//! keeps both sides by line (`check_sides`): all in `keepboth::check_outside`. No session, no store:
//! thousands of cases a second. Needs feature `merge-rule` (a kit with the three-way ingest).
#![no_main]

use k2_mpc_base::text::merge;
use k2_mpc_base_fuzz::{keepboth, Text};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|x: (Text, Text, Text)| {
    let (base, ours, theirs) = (x.0.render(), x.1.render(), x.2.render());
    let (a, b) = merge::outside_sides(&base, &ours, &theirs);
    let got = merge::merge3_keep_both(&base, &ours, &theirs);
    assert_eq!(
        k2_mpc_base::text::diff::apply(&theirs, &got.hunks),
        got.text,
        "the merge's hunks give its text"
    );
    if let Err(e) = keepboth::check_outside(&base, &ours, &theirs, &a, &b, &got.text) {
        panic!(
            "an outside merge: {e}\nbase {base:?}\nours {ours:?}\ntheirs {theirs:?}\ngot {:?}",
            got.text
        );
    }
});
