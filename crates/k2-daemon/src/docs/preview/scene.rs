//! Live BLEND scenes: 3D stills, the second [`super::PreviewRenderer`].
//!
//! - **The mesh read** (in the daemon, host-only): the BLEND pack's read of a
//!   doc version into K2SCENE1 (K2 Render's neutral contract,
//!   `k2_scenerender::scene`): base meshes triangulated by the pack,
//!   transforms as stored, no animation evaluated, each object's modifier
//!   types and whether they were applied. **Not in K2 yet** ([`mesh_read`]
//!   answers `unsupported_part`, and [`SceneRenderer::serves`] no doc), so
//!   everything after it is wired against K2SCENE1 bytes: [`list_k2scene`]
//!   and [`render_k2scene`].
//! - **Listing:** `@scene:main` (the auto 3/4 view of everything), one container
//!   per camera, one per object and the turntable's eight frames
//!   (`@turntable:0` to `@turntable:7`). Every address is a native address in
//!   the multiplayer kit's grammar, so it is also a presence view address: a
//!   camera or object by its live key from the read (`@cam:N`, `@obj:N`),
//!   which a rename keeps; else by its percent-encoded name
//!   (`@obj:Chair%202`), which a rename changes; else (a duplicated or empty
//!   name) by its index in this read (`@obj-n:4`, `@cam-n:0`), which is bound
//!   to that version (`k2_scenerender::digest`, `k2_scenerender::addr`). Each
//!   comes with the raw digest of what it draws, computed from the mesh read alone: meshes,
//!   transforms, colours, sidedness and cameras, never an object's id
//!   (`k2_scenerender::digest`). So the cache key is (doc, object or camera,
//!   render hash over the meshes it draws, width), and editing one object
//!   re-draws that object, the scene and the cameras, nothing else.
//! - **Drawing** (out of process): one `k2-scenerender` per render
//!   (`k2_scenerender::host`: posix_spawn, the limits on argv, a watchdog),
//!   WebP lossless, thumbnail limits up to 640 px (1 thread, 5 s, 512 MiB)
//!   and still limits above (4 threads, 10 s, 1 GiB). A view drawn from base
//!   meshes says so (`without_modifiers` in the image's metadata: the view
//!   shows "shown without modifiers").
//!
//! - **Transparent by default.** The 3D tab paints its own looks behind the
//!   still, so a scene view is drawn transparent ([`Look::Transparent`]:
//!   RGBA with coverage alpha, no background) unless the request asks for
//!   `look=opaque`; the cache keys the look, never a background colour.
//! - **Clay, and the scene view fitted to its contents.** Every view is
//!   drawn in clay ([`Clay`]: the renderer's two default tones, the shell
//!   darker and the contents lighter, by the same class rule as content
//!   framing), never in the objects' base colours. `@scene:main` is fitted to the
//!   contents ([`Framing::Contents`]: the room's shell is drawn but left out
//!   of the fit); the turntable and the cameras keep the bounds framing (the
//!   worker refuses content framing for them), and an object is framed on
//!   itself. Every view also asks for ambient occlusion with the renderer's
//!   default settings ([`Ao`]), so creases darken and objects sit on the
//!   floor. None of these is in the renderer's digest (they are the
//!   request's, not the scene's), so the host keys them: clay and occlusion
//!   in the renderer's [`build`](PreviewRenderer::build) (every view has
//!   both), the framing in the scene view's digest ([`keyed_digest`]).
//! - **Ids for picking.** The scene, object and camera views also ask for
//!   an id image ([`wants_ids`]: which object is under each pixel, so the
//!   client can pick and hover in a still); the turntable frames don't
//!   (nothing is picked there). The id image is cached beside its image
//!   and the route serves it with `layer=ids`; its table rides with the
//!   image. The build carries `.ids` for every view, turntable frames
//!   included: which views ask follows from the view alone, and the views
//!   that can share a render hash (an object and its renamed or identical
//!   twin) all ask, so unlike the framing it needs no per-view key; the
//!   turntable frames are only drawn once more under the new build.
//! - **Shaped stills.** The scene view and an object's view are drawn in
//!   the shape the request asks for ([`Shape`]: `wide` 16:9 by default, or
//!   `tall` 4:3; 1,920 × 1,080 or 1,920 × 1,440 at full width), so the
//!   stage gets a still of its own shape ([`shapes_view`]). A camera's view
//!   keeps its camera's aspect and a turntable frame the renderer's (4:3):
//!   the shape is ignored for them, and they are keyed without it, so
//!   asking a camera for `tall` finds the image it has. Like the look, the
//!   shape is the request's, not the scene's: the host keys it in the cache
//!   build ([`super::cache_build`]), and the renderer's build carries
//!   `.shape`, so no image drawn before the default became wide (4:3) is
//!   served as one.
//! - **Hidden and driven objects.** An object hidden in the file is listed
//!   (`hidden` on its container, `hidden_by` and its box in the extras) and
//!   its own view draws it; the scene, camera and turntable views leave it
//!   out, and their digests say which objects they left out, so hiding or
//!   showing one redraws them. An object whose placement is driven and not
//!   evaluated is drawn at its base transform: its box is `approx`
//!   (`approx_reason: "driven"`) and a view drawing it says `base_values`.
//!   A scene with neither draws, digests and lists as it did, so the cache
//!   tag stays.
//! - **What rides along** ([`super::extras`]): each object's local box,
//!   each camera's display values, the turntable frames' titles, and with
//!   each image the camera that drew it and its id table.
//!
//! **Which worker** (the k2-deckrender order): `K2_SCENERENDER_BIN` in a
//! debug build only (tests), else `k2-scenerender` beside the daemon (`K2
//! Daemon.app`). With neither, every render fails `renderer_error`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_core::log_debug;
use k2_scenerender::camera::Framing;
use k2_scenerender::digest::{self, Target};
use k2_scenerender::host::{self, Limits, SceneRequest};
use k2_scenerender::proto::{Ao, Clay, FailureCode, ImageFormat, BINARY_NAME, PROTO, VERSION};
use k2_scenerender::scene::InputLimits;
use sha2::{Digest as _, Sha256};

use super::{extras, Container, FailCode, Failed, Image, Listing, Look, PreviewRenderer, Shape, Source, Width};
use crate::docs::target::Target as DocTarget;

/// Debug builds only: the worker binary.
pub const ENV_BIN: &str = "K2_SCENERENDER_BIN";

pub struct SceneRenderer;

pub static SCENE: SceneRenderer = SceneRenderer;

/// The worker binary: the debug override, else beside this executable.
pub fn binary() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        if let Some(p) = std::env::var_os(ENV_BIN).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()) {
            return Some(p);
        }
    }
    let here = std::env::current_exe().ok()?;
    let beside = here.parent()?.join(BINARY_NAME);
    beside.is_file().then_some(beside)
}

/// The BLEND pack's mesh read: a doc version's bytes to K2SCENE1.
///
/// TODO(K2 Cores): call the BLEND pack's host-only mesh read here (its v1:
/// base meshes triangulated by the pack, transforms as stored, `modifiers`
/// and `modifiers_not_applied` per object, and `hidden` and
/// `transform_not_evaluated` where they apply) once the pack is vendored in K2,
/// with its own bounds before it allocates; and give a live BLEND doc its
/// tier, so [`SceneRenderer::serves`] says yes. Until then no doc is served
/// and this answers `unsupported_part`. K2 parses no `.blend` itself.
pub fn mesh_read(_doc: &[u8]) -> Result<Vec<u8>, Failed> {
    Err(Failed::new(FailCode::UnsupportedPart, "the BLEND pack's mesh read isn't in this build"))
}

fn code(c: FailureCode) -> FailCode {
    match c {
        FailureCode::TooLarge => FailCode::TooLarge,
        FailureCode::Timeout => FailCode::Timeout,
        // Bytes the reader refuses, or a view of something that isn't there: the file's.
        FailureCode::InvalidInput => FailCode::UnsupportedPart,
        FailureCode::RendererError => FailCode::RendererError,
    }
}

/// How the 3D tab frames `target`: the scene view on its contents, every
/// other view as the renderer frames it (the turntable and a camera on the
/// bounds, which content framing is refused for; an object on itself).
pub fn framing_for(target: Target) -> Framing {
    match target {
        Target::Scene => Framing::Contents,
        Target::Camera(_) | Target::Object(_) | Target::Turntable(_) => Framing::Bounds,
    }
}

/// Whether the 3D tab asks for `target`'s id image: the scene, an object
/// and a camera are picked in; a turntable frame isn't (it saves the id
/// image's time and memory).
pub fn wants_ids(target: Target) -> bool {
    match target {
        Target::Scene | Target::Camera(_) | Target::Object(_) => true,
        Target::Turntable(_) => false,
    }
}

/// Whether `target`'s still is drawn in the shape asked for ([`Shape`]):
/// the scene view and an object's view. A camera's view keeps its camera's
/// aspect, a turntable frame the renderer's.
pub fn shapes_view(target: Target) -> bool {
    match target {
        Target::Scene | Target::Object(_) => true,
        Target::Camera(_) | Target::Turntable(_) => false,
    }
}

/// The digest a container is keyed by: the renderer's digest of what it
/// draws, and for a view not on the bounds framing, that framing too. The
/// renderer's digest leaves the request's framing out, so without this a
/// scene of one object would give `@scene:main` and that object's view the same
/// render hash, and one could be answered with the other's image (see
/// [`super::reuse`]) though they are framed differently.
pub fn keyed_digest(target: Target, raw: [u8; 32]) -> [u8; 32] {
    match framing_for(target) {
        Framing::Bounds => raw,
        Framing::Contents => {
            let mut h = Sha256::new();
            h.update(b"k2/scene-preview/framing/contents\0");
            h.update(raw);
            h.finalize().into()
        }
    }
}

/// The containers of a K2SCENE1, with their digests (the scene view's with
/// its framing, [`keyed_digest`]). Reads the header only (every bound
/// checked) and hashes each mesh's data once.
pub fn list_k2scene(k2scene: &[u8]) -> Result<Listing, Failed> {
    let items = digest::list(k2scene, &InputLimits::default()).map_err(|e| Failed::new(FailCode::UnsupportedPart, format!("the scene: {e}")))?;
    let containers = items
        .into_iter()
        .enumerate()
        .map(|(i, it)| Container {
            title: extras::scene_title(it.target, &it.title),
            extras: extras::scene_item(&it),
            key: it.key,
            position: i as u32 + 1,
            // An object hidden in the file (listed all the same: only its own view draws it).
            hidden: it.hidden.is_some(),
            digest: keyed_digest(it.target, it.digest),
            locator: it.target.locator(),
        })
        .collect();
    Ok(Listing { page: None, containers })
}

/// The limits for a render at `width`, within the host's `wall`.
pub fn limits_for(width: Width, wall: Duration) -> Limits {
    let mut l = Limits::for_width(width.px());
    l.wall = l.wall.min(wall);
    l.max_output_bytes = l.max_output_bytes.min(width.max_bytes());
    l
}

/// [`limits_for`] for `req`: with an id image, the worker's output cap
/// (which counts it with the still) has room for both, and each is held to
/// the width's cap on its own after the render.
pub fn limits_for_request(req: &SceneRequest<'_>, width: Width, wall: Duration) -> Limits {
    let mut l = limits_for(width, wall);
    if req.ids {
        l.max_output_bytes = Limits::for_width(width.px()).max_output_bytes.min(width.max_bytes().saturating_mul(2));
    }
    l
}

/// The worker's request for one view in `look`: WebP; transparent with no
/// background, or opaque on the renderer's own neutral; in clay with the
/// default tones and with ambient occlusion on its default settings, framed
/// as [`framing_for`] says, with its id image when [`wants_ids`], and in
/// `shape` when [`shapes_view`] (else in its own aspect; `shape` ignored).
pub fn request_for(k2scene: &[u8], target: Target, width: Width, look: Look, shape: Shape) -> SceneRequest<'_> {
    let mut view = target.view(width.px());
    view.framing = framing_for(target);
    if shapes_view(target) {
        view.aspect = Some(shape.aspect());
    }
    let mut req = SceneRequest::new(k2scene, vec![view]);
    req.format = ImageFormat::Webp;
    req.background = None;
    req.transparent = look == Look::Transparent;
    req.clay = Some(Clay::default());
    req.ao = Some(Ao::default());
    req.ids = wants_ids(target);
    req
}

/// Draw `container` of a K2SCENE1 at `width` in `look` (and `shape`, for a
/// view it shapes), out of process, within `wall`.
pub fn render_k2scene(k2scene: &[u8], container: &Container, width: Width, look: Look, shape: Shape, wall: Duration) -> Result<Image, Failed> {
    let Some(bin) = binary() else {
        return Err(Failed::new(FailCode::RendererError, format!("{BINARY_NAME} isn't installed beside the daemon")));
    };
    render_k2scene_with(&bin, k2scene, container, width, look, shape, wall)
}

/// [`render_k2scene`] with the worker at `bin`.
pub fn render_k2scene_with(bin: &Path, k2scene: &[u8], container: &Container, width: Width, look: Look, shape: Shape, wall: Duration) -> Result<Image, Failed> {
    let Some(target) = Target::from_locator(&container.locator) else {
        return Err(Failed::new(FailCode::RendererError, format!("not a scene locator: {:?}", container.locator)));
    };
    let req = request_for(k2scene, target, width, look, shape);
    let started = std::time::Instant::now();
    match host::render(bin, &req, &limits_for_request(&req, width, wall)) {
        Ok(mut r) => {
            let Some(v) = r.views.pop() else {
                return Err(Failed::new(FailCode::RendererError, "the worker drew no view"));
            };
            let cap = width.max_bytes();
            for (what, len) in [("image", Some(v.image.len())), ("id image", v.ids.as_ref().map(Vec::len))] {
                if let Some(len) = len.filter(|&n| n as u64 > cap) {
                    return Err(Failed::new(FailCode::TooLarge, format!("the {what} is {len} bytes, over the {cap} kept at this width")));
                }
            }
            Ok(Image {
                mime: match r.format {
                    ImageFormat::Webp => "image/webp",
                    ImageFormat::Png => "image/png",
                },
                format: match r.format {
                    ImageFormat::Webp => "webp-lossless",
                    ImageFormat::Png => "png",
                },
                width: v.meta.width,
                height: v.meta.height,
                fonts_substituted: Vec::new(),
                without_modifiers: v.meta.without_modifiers,
                render_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                extras: extras::scene_view(&v.meta),
                ids: v.ids,
                bytes: v.image,
            })
        }
        Err(f) => {
            if !f.stderr.is_empty() {
                log_debug!("[docs/preview] k2-scenerender stderr ({}): {}", f.code, f.stderr.chars().take(2_000).collect::<String>());
            }
            Err(Failed::new(code(f.code), f.detail))
        }
    }
}

impl PreviewRenderer for SceneRenderer {
    fn kind(&self) -> &'static str {
        "scene"
    }

    /// The worker's protocol and version, then `.clay.ao2.ids.shape`: every
    /// view is drawn in clay with ambient occlusion (`ao2`: above thumbnail
    /// widths worked out on a half grid), every view that is picked in comes
    /// with its id image ([`wants_ids`]), and the views that are shaped are
    /// drawn in the shape asked for, wide by default ([`shapes_view`]), so no
    /// image drawn in base colours, without occlusion, with the earlier
    /// occlusion, without its id image or at the old 4:3 default is ever
    /// served.
    fn build(&self) -> String {
        format!("scenerender-p{PROTO}-{VERSION}.clay.ao2.ids.shape")
    }

    /// No doc yet: a live BLEND doc needs the pack's mesh read and its tier
    /// (see [`mesh_read`]).
    fn serves(&self, _t: &DocTarget) -> bool {
        false
    }

    /// The 3D tab's stills: transparent, so one image serves every look.
    fn default_look(&self) -> Look {
        Look::Transparent
    }

    fn draws(&self, _look: Look) -> bool {
        true
    }

    fn shapes(&self, container: &Container) -> bool {
        Target::from_locator(&container.locator).is_some_and(shapes_view)
    }

    fn draws_ids(&self, container: &Container) -> bool {
        Target::from_locator(&container.locator).is_some_and(wants_ids)
    }

    fn list(&self, bytes: &[u8]) -> Result<Listing, Failed> {
        list_k2scene(&mesh_read(bytes)?)
    }

    fn render(&self, src: &Source<'_>, container: &Container, width: Width, look: Look, shape: Shape, wall: Duration) -> Result<Image, Failed> {
        render_k2scene(&mesh_read(src.doc)?, container, width, look, shape, wall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_scenerender::scene::encode;
    use k2_scenerender::testgen;

    fn scene_with_camera() -> k2_scenerender::scene::Scene {
        let mut s = testgen::grid_scene(2, true);
        s.cameras = testgen::camera_scene().cameras;
        s
    }

    #[test]
    fn a_scene_lists_itself_its_cameras_and_its_objects_without_a_page() {
        let l = list_k2scene(&encode(&scene_with_camera())).expect("a listing");
        assert!(l.page.is_none(), "a scene has no page");
        let keys: Vec<(&str, u32, &str, &str)> = l.containers.iter().map(|c| (c.key.as_str(), c.position, c.locator.as_str(), c.title.as_str())).collect();
        assert_eq!(
            keys,
            [
                ("@scene:main", 1, "scene", "Scene"),
                ("@cam:Camera", 2, "c0", "Camera"),
                ("@obj:Item.000", 3, "o0", "Item.000"),
                ("@obj:Item.001", 4, "o1", "Item.001"),
                ("@obj:Item.002", 5, "o2", "Item.002"),
                ("@obj:Item.003", 6, "o3", "Item.003"),
                ("@turntable:0", 7, "t0", "Turntable 1"),
                ("@turntable:1", 8, "t1", "Turntable 2"),
                ("@turntable:2", 9, "t2", "Turntable 3"),
                ("@turntable:3", 10, "t3", "Turntable 4"),
                ("@turntable:4", 11, "t4", "Turntable 5"),
                ("@turntable:5", 12, "t5", "Turntable 6"),
                ("@turntable:6", 13, "t6", "Turntable 7"),
                ("@turntable:7", 14, "t7", "Turntable 8"),
            ]
        );
        assert_eq!(list_k2scene(&encode(&scene_with_camera())).expect("again"), l, "the same bytes list the same");
    }

    /// Moving one object changes its own digest, the scene's and the
    /// camera's; the other objects keep theirs (only they re-render). Renaming
    /// changes no digest (ids are not in them).
    #[test]
    fn an_edit_dirties_only_what_draws_it_and_a_rename_nothing() {
        let a = scene_with_camera();
        let mut b = a.clone();
        b.objects[3].transform[12] += 1.0;
        let (la, lb) = (list_k2scene(&encode(&a)).expect("a"), list_k2scene(&encode(&b)).expect("b"));
        let changed: Vec<&str> = la.containers.iter().zip(&lb.containers).filter(|(x, y)| x.digest != y.digest).map(|(x, _)| x.key.as_str()).collect();
        assert_eq!(changed[..3], ["@scene:main", "@cam:Camera", "@obj:Item.003"]);
        assert!(changed[3..].iter().all(|k| k.starts_with("@turntable:")) && changed.len() == 11, "and the turntable's 8 frames: {changed:?}");
        let mut c = a.clone();
        c.objects[0].id = "Renamed".into();
        let lc = list_k2scene(&encode(&c)).expect("c");
        assert!(la.containers.iter().zip(&lc.containers).all(|(x, y)| x.digest == y.digest));
        assert_eq!(lc.containers[2].key, "@obj:Renamed");
    }

    #[test]
    fn junk_is_unsupported_and_failures_map_to_the_contracts_codes() {
        assert_eq!(list_k2scene(b"not a scene").expect_err("junk").code, FailCode::UnsupportedPart);
        assert_eq!(code(FailureCode::InvalidInput), FailCode::UnsupportedPart);
        assert_eq!(code(FailureCode::RendererError), FailCode::RendererError);
        assert_eq!(code(FailureCode::Timeout), FailCode::Timeout);
        assert_eq!(code(FailureCode::TooLarge), FailCode::TooLarge);
        // Until the pack's mesh read is in K2, a doc's bytes list as unsupported.
        assert_eq!(SCENE.list(b"BLENDER-v300").expect_err("no mesh read").code, FailCode::UnsupportedPart);
        assert_eq!(SCENE.kind(), "scene");
        assert!(SCENE.build().starts_with("scenerender-p1-"));
    }

    #[test]
    fn widths_get_thumbnail_or_still_limits_within_the_hosts_wall() {
        let t = limits_for(Width::Thumb, Duration::from_secs(20));
        assert_eq!((t.threads, t.wall, t.memory_bytes, t.max_output_bytes), (1, Duration::from_secs(5), 512 << 20, Width::Thumb.max_bytes()));
        let t2 = limits_for(Width::Thumb2x, Duration::from_secs(20));
        assert_eq!(t2.threads, 1);
        let f = limits_for(Width::Full, Duration::from_secs(3));
        assert_eq!((f.threads, f.wall, f.memory_bytes, f.max_output_bytes), (4, Duration::from_secs(3), 1 << 30, Width::Full.max_bytes()));
        // With an id image the worker's cap has room for both (each is held to the width's cap after).
        let bytes = encode(&scene_with_camera());
        for w in Width::ALL {
            let with = request_for(&bytes, Target::Scene, w, Look::Transparent, Shape::Wide);
            let without = request_for(&bytes, Target::Turntable(0), w, Look::Transparent, Shape::Wide);
            assert_eq!(limits_for_request(&with, w, Duration::from_secs(20)).max_output_bytes, 2 * w.max_bytes(), "{w:?}");
            assert_eq!(limits_for_request(&without, w, Duration::from_secs(20)), limits_for(w, Duration::from_secs(20)), "{w:?}");
        }
    }

    /// The real worker: K2_SCENERENDER_BIN, else built for this test
    /// (cargo build -p k2-scenerender into its own target dir).
    #[test]
    fn a_container_renders_through_the_real_worker() {
        let bin = k2_core::test_env::workspace_bin(ENV_BIN, "k2-scenerender", BINARY_NAME);
        let mut s = scene_with_camera();
        s.objects[1].modifiers = vec!["SUBSURF".into()];
        s.objects[1].modifiers_not_applied = true;
        let bytes = encode(&s);
        let l = list_k2scene(&bytes).expect("listing");
        // Wide by default: the scene and an object 16:9; the camera keeps its own (16:9), a turntable frame 4:3.
        for (c, without, height) in [(&l.containers[0], true, 180), (&l.containers[1], true, 180), (&l.containers[2], false, 180), (&l.containers[3], true, 180), (&l.containers[6], true, 240)] {
            let img = render_k2scene_with(&bin, &bytes, c, Width::Thumb, Look::Opaque, Shape::Wide, Duration::from_secs(60)).unwrap_or_else(|f| panic!("{}: {:?}", c.key, f));
            assert_eq!((img.mime, img.format, img.width, img.height), ("image/webp", "webp-lossless", 320, height), "{}", c.key);
            assert_eq!(&img.bytes[..4], b"RIFF");
            assert_eq!(img.without_modifiers, without, "{}", c.key);
            assert!(img.extras["camera"]["proj"].as_array().is_some_and(|p| p.len() == 16), "{}: the camera that drew it", c.key);
            // The scene, objects and cameras come with an id image at the still's size and its table; turntable frames don't.
            assert_eq!(img.ids.is_some(), SCENE.draws_ids(c), "{}", c.key);
            assert_eq!(img.extras.contains_key("ids"), SCENE.draws_ids(c), "{}", c.key);
            if let Some(ids) = &img.ids {
                assert_eq!(&ids[..8], b"\x89PNG\r\n\x1a\n", "{}", c.key);
                assert_eq!((u32::from_be_bytes(ids[16..20].try_into().expect("4")), u32::from_be_bytes(ids[20..24].try_into().expect("4"))), (320, height), "{}: IHDR", c.key);
                let t = &img.extras["ids"];
                assert_eq!((t["layer"].clone(), t["image_len"].clone()), (serde_json::json!("ids"), serde_json::json!(ids.len())), "{}", c.key);
                assert!(t["count"].as_u64().is_some_and(|n| n >= 1 && n == t["objects"].as_array().map_or(0, Vec::len) as u64), "{}: {t}", c.key);
            }
        }
        // An object's view shows that object alone, named as the file names it.
        let o = render_k2scene_with(&bin, &bytes, &l.containers[2], Width::Thumb, Look::Opaque, Shape::Wide, Duration::from_secs(60)).expect("object");
        assert_eq!(o.extras["ids"]["objects"], serde_json::json!([{ "index": 0, "id": "Item.000" }]));
        // Transparent: a lossless WebP whose header says it has alpha.
        let t = render_k2scene_with(&bin, &bytes, &l.containers[0], Width::Thumb, Look::Transparent, Shape::Wide, Duration::from_secs(60)).expect("transparent");
        assert_eq!(&t.bytes[12..16], b"VP8L");
        assert_ne!(t.bytes[24] & 0x10, 0, "the VP8L header's alpha bit");
        let bad = Container { locator: "x9".into(), ..l.containers[0].clone() };
        assert_eq!(render_k2scene_with(&bin, &bytes, &bad, Width::Thumb, Look::Opaque, Shape::Wide, Duration::from_secs(60)).expect_err("locator").code, FailCode::RendererError);
    }

    /// The scene and an object are drawn in the shape asked for (wide 16:9,
    /// tall 4:3); a camera keeps its own aspect and a turntable frame 4:3,
    /// whatever is asked.
    #[test]
    fn the_scene_and_objects_are_drawn_in_the_shape_asked_and_the_rest_in_their_own() {
        let bin = k2_core::test_env::workspace_bin(ENV_BIN, "k2-scenerender", BINARY_NAME);
        let bytes = encode(&scene_with_camera());
        let l = list_k2scene(&bytes).expect("listing");
        let by = |key: &str| l.containers.iter().find(|c| c.key == key).unwrap_or_else(|| panic!("no {key}"));
        let cases = [
            ("@scene:main", Width::Full, Shape::Wide, (1920, 1080)),
            ("@scene:main", Width::Full, Shape::Tall, (1920, 1440)),
            ("@scene:main", Width::Thumb, Shape::Wide, (320, 180)),
            ("@scene:main", Width::Thumb, Shape::Tall, (320, 240)),
            ("@obj:Item.001", Width::Full, Shape::Wide, (1920, 1080)),
            ("@obj:Item.001", Width::Full, Shape::Tall, (1920, 1440)),
            ("@obj:Item.001", Width::Thumb, Shape::Wide, (320, 180)),
            ("@obj:Item.001", Width::Thumb, Shape::Tall, (320, 240)),
            ("@cam:Camera", Width::Thumb, Shape::Wide, (320, 180)),
            ("@cam:Camera", Width::Thumb, Shape::Tall, (320, 180)),
            ("@turntable:2", Width::Thumb, Shape::Wide, (320, 240)),
            ("@turntable:2", Width::Thumb, Shape::Tall, (320, 240)),
        ];
        for (key, w, shape, size) in cases {
            let c = by(key);
            assert_eq!(SCENE.shapes(c), key == "@scene:main" || key.starts_with("@obj:"), "{key}");
            let img = render_k2scene_with(&bin, &bytes, c, w, Look::Transparent, shape, Duration::from_secs(60)).unwrap_or_else(|f| panic!("{key} {shape:?}: {f:?}"));
            assert_eq!((img.width, img.height), size, "{key} {w:?} {shape:?}");
            assert!(img.bytes.len() as u64 <= w.max_bytes(), "{key} {w:?} {shape:?}: within the width's cap");
            if let Some(ids) = &img.ids {
                assert_eq!((u32::from_be_bytes(ids[16..20].try_into().expect("4")), u32::from_be_bytes(ids[20..24].try_into().expect("4"))), size, "{key} {shape:?}: the id image at the still's size");
            }
        }
    }

    #[test]
    fn only_the_scene_and_object_views_take_the_shape_asked() {
        let bytes = encode(&scene_with_camera());
        let mut targets = vec![(Target::Scene, true), (Target::Object(0), true), (Target::Camera(0), false)];
        targets.extend((0..8).map(|k| (Target::Turntable(k), false)));
        for (t, shaped) in targets {
            assert_eq!(shapes_view(t), shaped, "{t:?}");
            for w in Width::ALL {
                let wide = request_for(&bytes, t, w, Look::Transparent, Shape::Wide);
                let tall = request_for(&bytes, t, w, Look::Transparent, Shape::Tall);
                if shaped {
                    assert_eq!((wide.views[0].aspect, tall.views[0].aspect), (Some(16.0 / 9.0), Some(4.0 / 3.0)), "{t:?}");
                } else {
                    assert_eq!((wide.views[0].aspect, tall.views[0].aspect), (None, None), "{t:?}: its own aspect");
                }
                // Byte caps: the largest still (1,920 x 1,440, transparent, with ids) is today's size, so the caps hold.
                assert_eq!(limits_for_request(&tall, w, Duration::from_secs(20)), limits_for_request(&wide, w, Duration::from_secs(20)), "{t:?} {w:?}");
            }
        }
        let l = list_k2scene(&bytes).expect("listing");
        for c in &l.containers {
            assert_eq!(SCENE.shapes(c), c.key == "@scene:main" || c.key.starts_with("@obj:"), "{}", c.key);
        }
    }
}
