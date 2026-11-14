//! Stills of the generated interior scenes as PNG files, to look at a
//! renderer change: each scene's auto view framed on every object's bounds
//! and on its contents, in base colours and in clay, with and without
//! ambient occlusion, and transparent in clay. In process (no sandbox), the
//! bytes the worker draws.
//! `cargo run -p k2-scenerender --example stills -- <dir> [width] [ao strength] [ao radius]`.

use k2_scenerender::camera::{Framing, ViewSpec};
use k2_scenerender::proto::{Ao, Clay, ImageFormat, Request, View};
use k2_scenerender::render::render_bytes;
use k2_scenerender::sandbox::Caps;
use k2_scenerender::scene::{encode, InputLimits};
use k2_scenerender::testgen;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "stills".into());
    let width: u32 = args.next().and_then(|w| w.parse().ok()).unwrap_or(960);
    let mut ao = Ao::default();
    if let Some(s) = args.next().and_then(|s| s.parse().ok()) {
        ao.strength = s;
    }
    if let Some(r) = args.next().and_then(|r| r.parse().ok()) {
        ao.radius = r;
    }
    std::fs::create_dir_all(&dir).expect("the stills folder");
    let caps = Caps { max_width: 1920, max_side: 4096, max_pixels: 16 << 20, max_ssaa: 4, max_threads: 4, max_views: 64, max_output_bytes: 256 << 20, input: InputLimits::default() };
    let scenes = [("interior", testgen::interior_scene()), ("small-object", testgen::small_object_scene()), ("empty-room", testgen::empty_room_scene())];
    // (framing, clay, ambient occlusion, transparent)
    let looks = [
        (Framing::Bounds, false, false, false),
        (Framing::Contents, false, false, false),
        (Framing::Bounds, true, false, false),
        (Framing::Contents, true, false, false),
        (Framing::Contents, false, true, false),
        (Framing::Contents, true, true, false),
        (Framing::Contents, true, false, true),
        (Framing::Contents, true, true, true),
    ];
    for (name, s) in scenes {
        let input = encode(&s);
        for (framing, clay, occlusion, transparent) in looks {
            let mut req = Request::new(vec![View { framing, ..View::new(ViewSpec::Auto, width) }]);
            req.format = ImageFormat::Png;
            req.threads = 4;
            req.clay = clay.then(Clay::default);
            req.ao = occlusion.then_some(ao);
            req.transparent = transparent;
            let (out, _) = render_bytes(&input, &req, &caps).unwrap_or_else(|f| panic!("{name}: {} {}", f.code, f.detail));
            let (meta, png) = &out.views[0];
            let tag = match framing {
                Framing::Bounds => "bounds",
                Framing::Contents => "contents",
            };
            let file = format!("{name}-{tag}{}{}{}.png", if clay { "-clay" } else { "" }, if occlusion { "-ao" } else { "" }, if transparent { "-alpha" } else { "" });
            let path = std::path::Path::new(&dir).join(file);
            std::fs::write(&path, png).expect("write a still");
            println!(
                "{}: {}x{}, ssaa {}, raster {:.1} ms, framing {:?}, camera shift {:?}",
                path.display(),
                meta.width,
                meta.height,
                meta.ssaa,
                meta.raster_ms,
                meta.framing,
                meta.camera.map(|c| (c.shift_x, c.shift_y))
            );
        }
    }
}
