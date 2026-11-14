//! The whole request, small: decode, plan and draw two views at 48 px (the
//! auto view and a camera or turntable frame), as the worker does, then the
//! same views in clay with ambient occlusion and their id images. A typed error is fine; a panic escaping the renderer, a fault, a hang or an
//! allocation past libFuzzer's limit is a finding.
#![no_main]

use k2_scenerender::camera::ViewSpec;
use k2_scenerender::proto::{Ao, Clay, Request, View};
use k2_scenerender::render::render_bytes;
use k2_scenerender::sandbox::Caps;
use k2_scenerender::scene::InputLimits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let caps = Caps {
        max_width: 48,
        max_side: 96,
        max_pixels: 48 * 96,
        max_ssaa: 2,
        max_threads: 2,
        max_views: 4,
        max_output_bytes: 1 << 20,
        input: InputLimits {
            max_input_bytes: 1 << 20,
            max_header_bytes: 256 << 10,
            max_meshes: 64,
            max_objects: 256,
            max_cameras: 16,
            max_vertices: 50_000,
            max_triangles: 50_000,
            max_drawn_vertices: 100_000,
            max_drawn_triangles: 100_000,
        },
    };
    let mut req = Request::new(vec![View::new(ViewSpec::Auto, 48), View::new(ViewSpec::Camera(0), 48)]);
    req.threads = 2;
    if render_bytes(data, &req, &caps).is_err() {
        req.views[1] = View::new(ViewSpec::Turntable(3), 32);
        let _ = render_bytes(data, &req, &caps);
    }
    // The same views in clay with ambient occlusion and ids: the depth pass, the kernel, the id
    // image and its table too.
    req.clay = Some(Clay::default());
    req.ao = Some(Ao::default());
    req.ids = true;
    let _ = render_bytes(data, &req, &caps);
});
