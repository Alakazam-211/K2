//! The parse path the worker runs on every file: sniffing, deckcraft-pptx's
//! import, and the package's slide list. A typed error is fine; a panic
//! escaping `open`, a fault, a hang or an allocation past libFuzzer's limit
//! is a finding.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(o) = k2_deckrender::worker::open(data) {
        let _ = k2_deckrender::worker::slide_ids(data, &o.lost);
        for i in 0..o.pres.slides.len().min(4) {
            let _ = k2_deckrender::worker::shape_boxes(&o.pres, i);
        }
    }
});
