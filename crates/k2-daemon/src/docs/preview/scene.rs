//! Live BLEND scenes: 3D stills, the second [`super::PreviewRenderer`].
//!
//! - **The mesh read** (in the daemon, host-only): the BLEND pack's read of a
//!   doc version into K2SCENE1 (K2 Render's neutral contract,
//!   `k2_scenerender::scene`): base meshes triangulated by the pack,
//!   transforms as stored, no animation evaluated, each object's modifier
//!   types and whether they were applied ([`mesh_read_paired`]). It is given
//!   the live doc's carrier pairing (the keys the live store holds for the
//!   version's objects), so an object's key in K2SCENE1 (`@obj:N`, a camera's
//!   `@cam:N`) is its address in the live view; without one (a cut version),
//!   the keys an import of those bytes mints. Kept per (bytes, pairing, the
//!   pack's `MESH_READ_FORMAT`) for the renders that follow a listing. A
//!   refusal maps to the contract's codes ([`fail_code`]: `too_large`, else
//!   `unsupported_part`). [`SceneRenderer::serves`] a live BLEND doc (a
//!   `.blend` while the blend core is on); everything after the read works
//!   on K2SCENE1 bytes: [`list_k2scene`] and [`render_k2scene`].
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
//!   image, each entry naming its object's live key (`key`) when the read
//!   keys it, from the listing when it is answered (keys are in no digest,
//!   so never from the cache; the cache tag stays). The build carries
//!   `.ids` for every view, turntable frames included: which views ask
//!   follows from the view alone, and the views that can share a render
//!   hash (an object and its renamed or identical twin) all ask, so unlike
//!   the framing it needs no per-view key; the turntable frames are only
//!   drawn once more under the new build.
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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use k2_core::log_debug;
use k2_mpc_base::session::Pairing;
use k2_mpc_blend::meshread::{MeshReadLimits, MESH_READ_FORMAT};
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

/// A mesh read's refusal code (`MeshReadError::code`) as the contract's:
/// over a limit is `too_large`; anything else the file holds that the read
/// won't draw (`not_blend`, `unknown_form`, `unreadable`, `live_version`,
/// `live_ambiguous`, `pairing_mismatch`, `invalid`, a code a later pack
/// adds) is `unsupported_part`.
pub fn fail_code(code: &str) -> FailCode {
    match code {
        "too_large" => FailCode::TooLarge,
        _ => FailCode::UnsupportedPart,
    }
}

/// How many reads are kept: the views of one or two scenes being drawn.
const READS_KEPT: usize = 4;

/// The kept reads, newest last: (key, K2SCENE1).
fn reads() -> &'static Mutex<Vec<([u8; 32], Arc<Vec<u8>>)>> {
    static R: OnceLock<Mutex<Vec<([u8; 32], Arc<Vec<u8>>)>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

/// What a read is kept under: the bytes, the pairing and the read's format
/// (a pack whose output changes bumps it).
fn read_key(doc: &[u8], pairing: Option<&Pairing>) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"k2/scene-preview/mesh-read\0");
    h.update(MESH_READ_FORMAT.to_le_bytes());
    h.update((doc.len() as u64).to_le_bytes());
    h.update(doc);
    match pairing {
        Some(p) => {
            h.update([1u8]);
            h.update(p.to_bytes());
        }
        None => h.update([0u8]),
    }
    h.finalize().into()
}

/// The BLEND pack's mesh read of a doc version's bytes into K2SCENE1, its
/// objects keyed by `pairing` (the version's carrier pairing: the live
/// view's addresses), or as an import mints them without one. Bounded by the
/// pack's own limits (the live tier's file cap, the renderer's input
/// limits, the heap) before it allocates. The report goes to the debug log
/// (names only). K2 parses no `.blend` itself.
pub fn mesh_read_paired(doc: &[u8], pairing: Option<&Pairing>) -> Result<Arc<Vec<u8>>, Failed> {
    let key = read_key(doc, pairing);
    if let Some((_, s)) = reads().lock().unwrap_or_else(|p| p.into_inner()).iter().find(|(k, _)| *k == key) {
        return Ok(s.clone());
    }
    let read = k2_mpc_blend::meshread::mesh_read_paired(doc, pairing, &MeshReadLimits::default())
        .map_err(|e| Failed::new(fail_code(e.code()), format!("the mesh read: {e}")))?;
    for line in read.report.lines() {
        log_debug!("[docs/preview] mesh read: {line}");
    }
    let k2scene = Arc::new(read.k2scene);
    let mut kept = reads().lock().unwrap_or_else(|p| p.into_inner());
    if kept.len() >= READS_KEPT {
        kept.remove(0);
    }
    kept.push((key, k2scene.clone()));
    Ok(k2scene)
}

/// [`mesh_read_paired`] with no pairing: the keys an import mints.
pub fn mesh_read(doc: &[u8]) -> Result<Arc<Vec<u8>>, Failed> {
    mesh_read_paired(doc, None)
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
    let listed = digest::listed(k2scene, &InputLimits::default()).map_err(|e| Failed::new(FailCode::UnsupportedPart, format!("the scene: {e}")))?;
    let containers = listed
        .items
        .into_iter()
        .enumerate()
        .map(|(i, it)| Container {
            title: extras::scene_title(it.target, &it.title),
            extras: extras::scene_item(&it),
            live_key: it.keyed.then(|| it.key.clone()),
            key: it.key,
            position: i as u32 + 1,
            // An object hidden in the file (listed all the same: only its own view draws it).
            hidden: it.hidden.is_some(),
            digest: keyed_digest(it.target, it.digest),
            locator: it.target.locator(),
        })
        .collect();
    Ok(Listing { page: None, containers, extras: extras::scene_listing(&listed.cameras_left_out) })
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

    /// A live BLEND doc: a `.blend` while the blend core is on (its tier is
    /// then [`crate::docs::target::Tier::Scene`]).
    fn serves(&self, t: &DocTarget) -> bool {
        t.tier == crate::docs::target::Tier::Scene
    }

    /// The listing's keys are the live view's: the head's carrier pairing.
    fn wants_pairing(&self) -> bool {
        true
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
        self.list_paired(bytes, None)
    }

    fn list_paired(&self, bytes: &[u8], pairing: Option<&Pairing>) -> Result<Listing, Failed> {
        list_k2scene(&mesh_read_paired(bytes, pairing)?)
    }

    fn render(&self, src: &Source<'_>, container: &Container, width: Width, look: Look, shape: Shape, wall: Duration) -> Result<Image, Failed> {
        render_k2scene(&mesh_read_paired(src.doc, src.pairing)?, container, width, look, shape, wall)
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
        // Bytes the pack's mesh read won't draw list as unsupported.
        assert_eq!(SCENE.list(b"BLENDER-v300").expect_err("not a whole scene").code, FailCode::UnsupportedPart);
        assert_eq!(SCENE.list(b"not a scene file").expect_err("not a scene").code, FailCode::UnsupportedPart);
        assert_eq!(SCENE.kind(), "scene");
        assert!(SCENE.build().starts_with("scenerender-p1-"));
    }

    /// The mesh read's refusals as the contract's codes: a limit is
    /// `too_large`, everything else the file holds `unsupported_part`.
    #[test]
    fn mesh_read_refusals_map_to_the_contracts_codes() {
        assert_eq!(fail_code("too_large"), FailCode::TooLarge);
        for c in ["not_blend", "unknown_form", "unreadable", "live_version", "live_ambiguous", "pairing_mismatch", "invalid", "a_code_from_later"] {
            assert_eq!(fail_code(c), FailCode::UnsupportedPart, "{c}");
        }
    }

    /// A real scene (the pack's generated living room): its listing keys
    /// objects and cameras as an import mints them without a pairing, and by
    /// the pairing's keys with one; a pairing that doesn't fit is refused,
    /// never guessed. The same bytes read once.
    #[test]
    fn a_blend_scene_lists_its_cameras_and_objects_by_live_key() {
        let bytes = k2_mpc_blend::testgen::scenes::living_room();
        let k2scene = mesh_read(&bytes).expect("the living room reads");
        let scene = k2_scenerender::scene::decode(&k2scene, &InputLimits::default()).expect("the renderer's own decoder takes it");
        assert!(!scene.objects.is_empty() && !scene.cameras.is_empty(), "objects and cameras");
        let l = SCENE.list(&bytes).expect("a listing");
        assert_eq!(l.containers[0].key, "@scene:main");
        let objs: Vec<&str> = l.containers.iter().map(|c| c.key.as_str()).filter(|k| k.starts_with("@obj:")).collect();
        let cams: Vec<&str> = l.containers.iter().map(|c| c.key.as_str()).filter(|k| k.starts_with("@cam:")).collect();
        assert_eq!(objs.len(), scene.objects.len(), "{objs:?}");
        assert!(!cams.is_empty(), "{cams:?}");
        for k in objs.iter().chain(&cams) {
            let n = k.split_once(':').map(|(_, n)| n).expect("a key");
            assert!(n.parse::<u64>().is_ok_and(|n| n >= 1), "{k}: a live key's number (minted o1, o2, ...)");
        }
        assert!(std::sync::Arc::ptr_eq(&mesh_read(&bytes).expect("again"), &k2scene), "the same bytes are read once");

        // The live doc's pairing: every object of the decode, in its order.
        let blend = k2_mpc_blend::live::BlendLive::default();
        let content = k2_mpc_base::session::Pack::decode(&blend, &bytes).expect("the live tier reads it").roots;
        let objs_root = k2_mpc_blend::live::OBJS;
        let n = content
            .iter()
            .find(|(r, _)| r == objs_root)
            .and_then(|(_, v)| match v {
                k2_mpc_base::doc::Neutral::List(l) => Some(l.len()),
                _ => None,
            })
            .expect("the objects' list");
        let shifted: Vec<k2_mpc_base::elem::Key> = (0..n).map(|i| k2_mpc_base::elem::Key::new(&format!("o{}", 100 + i)).expect("key")).collect();
        let pairing = Pairing { roots: vec![(objs_root.to_string(), shifted)] };
        let paired = SCENE.list_paired(&bytes, Some(&pairing)).expect("a paired listing");
        let paired_objs: Vec<&str> = paired.containers.iter().map(|c| c.key.as_str()).filter(|k| k.starts_with("@obj:")).collect();
        assert!(!paired_objs.is_empty() && paired_objs.iter().all(|k| k[5..].parse::<u64>().is_ok_and(|n| n >= 100)), "keyed by the pairing: {paired_objs:?}");
        let wrong = Pairing { roots: vec![(objs_root.to_string(), vec![k2_mpc_base::elem::Key::new("o1").expect("key")])] };
        assert_eq!(SCENE.list_paired(&bytes, Some(&wrong)).expect_err("a pairing that doesn't fit").code, FailCode::UnsupportedPart);
    }

    /// K2 Render's decoder takes the pack's hand-written K2SCENE1 golden
    /// (the pack's conformance row, on K2's side).
    #[test]
    fn the_renderer_decodes_the_packs_k2scene_golden() {
        let text = include_str!("../../../../../vendor/k2-mpc-blend/crates/blend/tests/meshread_golden/scene.k2scene.hex");
        let mut bytes = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(t) = line.strip_prefix("text ") {
                bytes.extend_from_slice(t.as_bytes());
                continue;
            }
            for h in line.split_whitespace() {
                bytes.push(u8::from_str_radix(h, 16).unwrap_or_else(|e| panic!("{h:?}: {e}")));
            }
        }
        let s = k2_scenerender::scene::decode(&bytes, &InputLimits::default()).expect("the golden decodes");
        assert_eq!((s.meshes.len(), s.objects.len(), s.cameras.len()), (2, 3, 1), "two meshes, three objects, one camera");
        let l = list_k2scene(&bytes).expect("and lists");
        assert!(l.containers.iter().any(|c| c.key.starts_with("@cam:")), "{:?}", l.containers.iter().map(|c| &c.key).collect::<Vec<_>>());
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
