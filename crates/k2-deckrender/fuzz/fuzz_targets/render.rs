//! The whole render, small: the first slide at 96 px wide, at most 96 x 960
//! (the worker's own size checks apply). A typed error is fine; a panic
//! escaping `render`, a fault, a hang or an allocation past libFuzzer's limit
//! is a finding.
#![no_main]

use k2_deckrender::proto::SlideRef;
use k2_deckrender::worker::{render, Opts};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let opts = Opts { max_side: 960, max_pixels: 96 * 960, ..Opts::default() };
    let _ = render(data, SlideRef::Index(0), 96, &opts);
});
