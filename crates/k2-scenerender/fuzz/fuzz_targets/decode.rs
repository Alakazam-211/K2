//! The K2SCENE1 decoder the worker runs on every input, and the listing the
//! daemon runs on the same bytes: the prefix, the header (capped lists, every
//! count against the limits), the mesh data, and the digests. A typed error
//! is fine; a panic, a fault, a hang or an allocation past libFuzzer's limit
//! is a finding. The limits are small, so a header that passes them can't
//! ask for much.
#![no_main]

use k2_scenerender::digest;
use k2_scenerender::scene::{decode, InputLimits};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let lim = InputLimits {
        max_input_bytes: 1 << 20,
        max_header_bytes: 256 << 10,
        max_meshes: 64,
        max_objects: 256,
        max_cameras: 16,
        max_vertices: 100_000,
        max_triangles: 100_000,
        max_drawn_vertices: 400_000,
        max_drawn_triangles: 400_000,
    };
    let decoded = decode(data, &lim);
    let listed = digest::list(data, &lim);
    // The listing reads only the header: whatever it refuses, the decoder refuses too.
    if listed.is_err() {
        assert!(decoded.is_err(), "the listing refused what the decoder took");
    }
});
