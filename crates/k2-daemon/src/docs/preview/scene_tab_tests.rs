//! The 3D tab's host side: what a scene's listing and images carry to the
//! client, transparent stills, the turntable's titles, the preview events
//! on the docs socket, a rename answered from the cache, and the id images
//! (asked for, keyed, cached beside their images, served, and named as the
//! doc is now after a rename).
//!
//! The service tests draw with [`Fake`], a renderer that runs no process and
//! logs every render, so "no worker run" is a count. A test can hold one of
//! its renders until it lets it land ([`hold`]).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_mpc_base::blob::BlobStore;
use k2_mpc_base::errors::Value;
use k2_mpc_base::store::DocKey;
use k2_scenerender::camera::{Framing, ViewSpec};
use k2_scenerender::digest::Target as SceneTarget;
use base64::Engine as _;
use k2_scenerender::host::SceneRequest;
use k2_scenerender::proto::{Ao, CameraMeta, Clay, IdEntry, IdsMeta, ImageFormat, Request, ViewMeta, AO_RADIUS, AO_STRENGTH, BINARY_NAME, CLAY_CONTENTS, CLAY_SHELL, PROTO, VERSION};
use k2_scenerender::scene::{encode, SensorFit};
use k2_scenerender::testgen;
use serde_json::json;

use super::cache::{self, Got};
use super::service::{self, Keyed, Requested, Snapshot};
use super::*;
use crate::docs::host_events::{self, HostEvent, Subscription};
use crate::docs::target::Target as DocTarget;
use crate::test_support::{with_temp_home, Scratch};

fn store() -> (Scratch, Arc<BlobStore>) {
    let s = Scratch::new();
    let docs = Arc::new(k2_mpc_base::store::DocStore::open(&s.path().join("docs.redb")).expect("store"));
    let blobs = BlobStore::open(docs, &s.path().join("blobs"), k2_mpc_base::blob::BlobConfig::default(), Arc::new(|| 1_000)).expect("blobs");
    (s, Arc::new(blobs))
}

fn doc(path: &str) -> DocKey {
    DocKey { space: "scene-tab-ws".into(), path: path.into() }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ── A renderer that draws nothing and counts ─────────────────────────

/// Lists one container per line, `<address> <digest byte>`, optionally
/// with a scene locator after them (`scene`, `o<i>`, `t<k>`: then an
/// `@obj:` address's title is its id) and a title after that (an object
/// keyed by its live address, `@obj:7 5 o0 Chair`); draws every address ending `:fail`
/// as a timeout, the rest as the same small image. A container with a
/// locator the scene renderer gives ids to also gets [`FAKE_IDS`] and a
/// table of the listed objects it shows (an object view: itself; any other:
/// every object with an `o<i>` locator).
struct Fake;

/// The fake's id image.
const FAKE_IDS: &[u8] = b"IDPNG";

/// A fake listing's lines: (address, digest byte, locator, title).
fn fake_lines(text: &str) -> Vec<(&str, u8, Option<&str>, Option<&str>)> {
    text.lines()
        .map(|line| {
            let mut w = line.split(' ');
            let key = w.next().expect("an address");
            let d = w.next().expect("<address> <digest byte>").parse::<u8>().expect("a digest byte");
            (key, d, w.next(), w.next())
        })
        .collect()
}

fn fake_title(key: &str, locator: Option<&str>, title: Option<&str>) -> String {
    title.or(locator.and(key.strip_prefix("@obj:"))).unwrap_or_default().to_string()
}

static FAKE: Fake = Fake;

fn drawn() -> &'static Mutex<Vec<(String, Look, Shape)>> {
    static D: Mutex<Vec<(String, Look, Shape)>> = Mutex::new(Vec::new());
    &D
}

fn renders_of(container: &str) -> Vec<Look> {
    drawn().lock().unwrap_or_else(|p| p.into_inner()).iter().filter(|(c, _, _)| c == container).map(|(_, l, _)| *l).collect()
}

fn shapes_drawn(container: &str) -> Vec<Shape> {
    drawn().lock().unwrap_or_else(|p| p.into_inner()).iter().filter(|(c, _, _)| c == container).map(|(_, _, s)| *s).collect()
}

type Held = Vec<(String, String, std::sync::mpsc::Receiver<()>)>;

fn held() -> &'static Mutex<Held> {
    static H: Mutex<Held> = Mutex::new(Vec::new());
    &H
}

/// Hold the fake's next render of `container` from the doc `listing` until
/// the returned sender sends (or is dropped, or 20 s pass, inside the
/// host's cap): the test decides when that draw lands.
fn hold(listing: &str, container: &str) -> std::sync::mpsc::Sender<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    held().lock().unwrap_or_else(|p| p.into_inner()).push((listing.to_string(), container.to_string(), rx));
    tx
}

impl PreviewRenderer for Fake {
    fn kind(&self) -> &'static str {
        "fake"
    }
    fn build(&self) -> String {
        "fake-1".into()
    }
    fn serves(&self, _t: &DocTarget) -> bool {
        false
    }
    fn default_look(&self) -> Look {
        Look::Transparent
    }
    fn draws(&self, _look: Look) -> bool {
        true
    }
    fn shapes(&self, c: &Container) -> bool {
        scene::SCENE.shapes(c)
    }
    fn draws_ids(&self, c: &Container) -> bool {
        scene::SCENE.draws_ids(c)
    }
    fn list(&self, bytes: &[u8]) -> Result<Listing, Failed> {
        let text = std::str::from_utf8(bytes).expect("a fake listing is text");
        let containers = fake_lines(text)
            .into_iter()
            .enumerate()
            .map(|(i, (key, d, locator, title))| Container {
                key: key.into(),
                position: i as u32 + 1,
                title: fake_title(key, locator, title),
                hidden: false,
                digest: [d; 32],
                locator: locator.unwrap_or(key).into(),
                extras: Default::default(),
            })
            .collect();
        Ok(Listing { page: None, containers })
    }
    fn render(&self, src: &Source<'_>, c: &Container, width: Width, look: Look, shape: Shape, _wall: Duration) -> Result<Image, Failed> {
        let gate = {
            let mut h = held().lock().unwrap_or_else(|p| p.into_inner());
            h.iter().position(|(l, k, _)| l.as_bytes() == src.doc && *k == c.key).map(|i| h.remove(i).2)
        };
        if let Some(rx) = gate {
            let _ = rx.recv_timeout(Duration::from_secs(20));
        }
        drawn().lock().unwrap_or_else(|p| p.into_inner()).push((c.key.clone(), look, shape));
        if c.key.ends_with(":fail") {
            return Err(Failed::new(FailCode::Timeout, "the fake's timeout"));
        }
        let mut extras = serde_json::Map::new();
        extras.insert("camera".into(), json!({ "fx": 1.5 }));
        let mut ids = None;
        if self.draws_ids(c) {
            let own = SceneTarget::from_locator(&c.locator);
            let objects: Vec<IdEntry> = fake_lines(std::str::from_utf8(src.doc).expect("text"))
                .into_iter()
                .filter_map(|(key, _, locator, title)| match locator.and_then(SceneTarget::from_locator) {
                    Some(t @ SceneTarget::Object(i)) if !matches!(own, Some(SceneTarget::Object(_))) || own == Some(t) => {
                        Some(IdEntry { index: i, id: fake_title(key, locator, title) })
                    }
                    _ => None,
                })
                .collect();
            let meta = ViewMeta { ids: Some(IdsMeta { image_len: FAKE_IDS.len() as u64, count: objects.len() as u64, objects }), ..Default::default() };
            extras.extend(extras::scene_view(&meta));
            ids = Some(FAKE_IDS.to_vec());
        }
        Ok(Image {
            bytes: b"PIXELS".to_vec(),
            mime: "image/webp",
            format: "webp-lossless",
            width: width.px(),
            height: 10,
            fonts_substituted: Vec::new(),
            without_modifiers: true,
            render_ms: 7,
            extras,
            ids,
        })
    }
}

fn snap(blobs: &BlobStore, key: &DocKey, rev: u64, listing: &str) -> Arc<Snapshot> {
    Arc::new(service::list(blobs, &FAKE, key, rev, None, listing.as_bytes().to_vec()).expect("a fake listing"))
}

fn next(sub: &Subscription, what: &str) -> HostEvent {
    sub.rx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|e| panic!("no {what}: {e}"))
}

fn frame(ev: &HostEvent) -> String {
    ev.frame(|pid: &str| Value::from(pid)).to_json()
}

fn scene_with_display_fields() -> k2_scenerender::scene::Scene {
    let mut s = testgen::grid_scene(2, true);
    s.cameras = testgen::camera_scene().cameras;
    s.cameras[0].active = Some(true);
    s.cameras[0].lens_mm = Some(50.0);
    s.cameras[0].sensor_mm = Some([36.0, 24.0]);
    s.cameras[0].sensor_fit = Some(SensorFit::Horizontal);
    s.objects[1].modifiers = vec!["SUBSURF".into()];
    s.objects[1].modifiers_not_applied = true;
    s
}

// ── 1. Pass-through, keyed per the contract ──────────────────────────

#[test]
fn a_scene_listing_carries_bounds_camera_display_values_and_turntable_titles() {
    let l = scene::list_k2scene(&encode(&scene_with_display_fields())).expect("a listing");
    let by = |key: &str| l.containers.iter().find(|c| c.key == key).unwrap_or_else(|| panic!("no {key}"));
    assert!(by("@scene:main").extras.is_empty(), "the auto view has no extras");
    assert_eq!(
        serde_json::Value::Object(by("@cam:Camera").extras.clone()),
        json!({ "camera": { "active": true, "lens_mm": 50.0, "sensor_mm": [36.0, 24.0], "sensor_fit": "horizontal" } })
    );
    for (key, approx) in [("@obj:Item.000", false), ("@obj:Item.001", true), ("@obj:Item.002", false)] {
        let b = &by(key).extras["bounds"];
        assert_eq!(b["approx"], json!(approx), "{key}: approx when drawn without its modifiers");
        let (min, max) = (b["min"].as_array().expect("min"), b["max"].as_array().expect("max"));
        assert_eq!((min.len(), max.len()), (3, 3), "{key}");
        for i in 0..3 {
            assert!(min[i].as_f64().expect("a number") < max[i].as_f64().expect("a number"), "{key}: {b}");
        }
        assert_eq!(by(key).extras.len(), 1, "{key}: bounds only");
    }
    let titles: Vec<&str> = l.containers.iter().filter(|c| c.key.starts_with("@turntable:")).map(|c| c.title.as_str()).collect();
    assert_eq!(titles, (1..=8).map(|k| format!("Turntable {k}")).collect::<Vec<_>>());
    assert!(l.containers.iter().filter(|c| c.key.starts_with("@turntable:")).all(|c| c.extras.is_empty()));
    // One place names them.
    assert_eq!(extras::scene_title(SceneTarget::Turntable(7), ""), "Turntable 8");
    assert_eq!(extras::scene_title(SceneTarget::Scene, ""), "Scene");
    assert_eq!(extras::scene_title(SceneTarget::Object(2), "Cube"), "Cube");
}

/// What the client gets (the route's listing, the socket's welcome) carries
/// the extras and keyed render hashes: no raw digest anywhere.
#[test]
fn the_route_and_the_welcome_carry_the_extras_with_keyed_hashes_only() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("scenes/shot.blend");
        let l = scene::list_k2scene(&encode(&scene_with_display_fields())).expect("a listing");
        let secret = *blobs.host_cache_key(&key.space, service::HASH_LABEL).expect("the cache key").secret_bytes();
        let snap = Snapshot {
            key: key.clone(),
            rev: 3,
            version: None,
            renderer: &scene::SCENE,
            build: scene::SCENE.build(),
            page: l.page,
            containers: l.containers.iter().cloned().map(|c| Keyed { hash: service::keyed_hash(&secret, &c.digest), c }).collect(),
            bytes: Arc::new(Vec::new()),
        };
        let listed: Vec<serde_json::Value> = snap.containers.iter().map(|k| route::container_json(&snap, k, Look::Transparent, Shape::Wide)).collect();
        let welcome = crate::docs::live_deck::previews_value(&snap).to_json();
        let route_text = serde_json::to_string(&listed).expect("json");
        for (k, v) in snap.containers.iter().zip(&listed) {
            assert_eq!(v["render_hash"], json!(k.hash), "{}", k.c.key);
            assert_ne!(k.hash, hex(&k.c.digest[..16]), "keyed, not the digest");
            for text in [&route_text, &welcome] {
                assert!(!text.contains(&hex(&k.c.digest[..8])), "{}: a raw digest reached the client", k.c.key);
            }
        }
        let cam = listed.iter().find(|v| v["key"] == "@cam:Camera").expect("the camera");
        assert_eq!(cam["camera"]["lens_mm"], json!(50.0));
        let obj = listed.iter().find(|v| v["key"] == "@obj:Item.001").expect("an object");
        assert_eq!(obj["bounds"]["approx"], json!(true));
        assert_eq!(obj["previews"], json!({ "thumb": "none", "full": "none" }));
        assert!(listed.iter().any(|v| v["title"] == "Turntable 1"));
        assert!(welcome.contains(r#""key":"@cam:Camera","position":2,"title":"Camera","hidden":false,"render_hash":""#), "{welcome}");
        assert!(welcome.contains(r#""camera":{"active":true,"lens_mm":50,"sensor_fit":"horizontal","sensor_mm":[36,24]}"#) || welcome.contains(r#""camera":{"active":true,"lens_mm":50,"sensor_mm":[36,24],"sensor_fit":"horizontal"}"#), "{welcome}");
        assert!(welcome.contains(r#""bounds":{"#), "{welcome}");
        assert!(welcome.contains(r#""title":"Turntable 8""#), "{welcome}");
    });
}

#[test]
fn a_scene_view_carries_the_camera_that_drew_it_in_short_numbers() {
    let mut proj = [0f32; 16];
    proj[0] = 1.5;
    let cam = CameraMeta { view: testgen::identity(), proj, fx: 1.5, fy: 2.0, near: 0.1, shift_x: 0.0, shift_y: 0.0 };
    let v = ViewMeta { width: 320, height: 240, camera: Some(cam), ..Default::default() };
    let x = extras::scene_view(&v);
    let c = &x["camera"];
    assert_eq!(c["view"].as_array().map(Vec::len), Some(16));
    assert_eq!(c["proj"][0], json!(1.5));
    assert_eq!(serde_json::to_string(&c["near"]).expect("json"), "0.1", "the f32's shortest form");
    assert_eq!((c["fx"].clone(), c["fy"].clone(), c["shift_x"].clone()), (json!(1.5), json!(2.0), json!(0.0)));
    assert!(extras::scene_view(&ViewMeta { camera: None, ..v.clone() }).is_empty(), "no camera, no field");
    assert!(extras::scene_view(&ViewMeta { camera: Some(CameraMeta { fx: f32::INFINITY, ..cam }), ..v }).is_empty(), "never a number JSON can't hold");
    // On the socket, as frame fields.
    assert_eq!(Value::Obj(extras::wire_fields(&x)).to_json().matches("\"near\":0.1").count(), 1);
}

// ── 2. Transparent stills ────────────────────────────────────────────

#[test]
fn scene_views_ask_for_transparent_stills_and_the_cache_key_has_no_background() {
    assert_eq!(scene::SCENE.default_look(), Look::Transparent, "the 3D tab's default");
    assert!(scene::SCENE.draws(Look::Opaque), "a caller may still ask for opaque");
    assert_eq!(deck::DECK.default_look(), Look::Opaque, "a deck is unchanged");
    assert!(!deck::DECK.draws(Look::Transparent));
    let bytes = encode(&scene_with_display_fields());
    let t = scene::request_for(&bytes, SceneTarget::Scene, Width::Full, Look::Transparent, Shape::Wide);
    assert!(t.transparent, "transparent asked");
    assert_eq!(t.background, None, "never with a background (the worker refuses both)");
    assert_eq!((t.format, t.views.len(), t.views[0].width), (ImageFormat::Webp, 1, 1920));
    let o = scene::request_for(&bytes, SceneTarget::Camera(0), Width::Thumb, Look::Opaque, Shape::Wide);
    assert!(!o.transparent);
    assert_eq!(o.background, None, "opaque: the renderer's own neutral, as before");
    // An opaque entry keeps the name it always had; a transparent one is
    // keyed by the look alone, no colour.
    assert_eq!(Look::Opaque.cache_build("b1"), "b1");
    assert_eq!(cache::entry_name("@scene:main", "ab12", Width::Thumb, &Look::Opaque.cache_build("b1")), cache::entry_name("@scene:main", "ab12", Width::Thumb, "b1"));
    assert_eq!(
        cache::entry_name("@scene:main", "ab12", Width::Thumb, &Look::Transparent.cache_build("b1")),
        format!("{}-ab12-thumb-b1.rgba.k2p", cache::container_id("@scene:main"))
    );
    assert_eq!(Look::parse("transparent"), Some(Look::Transparent));
    assert_eq!(Look::parse("glass"), None);
}

#[test]
fn a_transparent_request_is_drawn_and_cached_as_transparent() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("tx.blend");
        let s = snap(&blobs, &key, 1, "@obj:Tx 3");
        let sub = host_events::subscribe(&key);
        assert_eq!(service::request(&blobs, &s, "@obj:Tx", Width::Thumb, Look::Transparent, Shape::Wide), Requested::Rendering);
        let r = frame(&next(&sub, "rendering"));
        assert!(r.contains(r#""width":"thumb","look":"transparent","rev":1"#), "{r}");
        let ready = frame(&next(&sub, "ready"));
        assert!(ready.contains(r#""state":"ready""#) && ready.contains(r#""look":"transparent""#), "{ready}");
        assert_eq!(renders_of("@obj:Tx"), [Look::Transparent]);
        let hash = &s.containers[0].hash;
        assert!(cache::entry_exists(&key, "@obj:Tx", hash, Width::Thumb, &Look::Transparent.cache_build("fake-1")));
        assert!(!cache::entry_exists(&key, "@obj:Tx", hash, Width::Thumb, "fake-1"), "not under the opaque name");
        let Got::Hit(e) = cache::get(&blobs, &key, "@obj:Tx", hash, Width::Thumb, &Look::Transparent.cache_build("fake-1")) else { panic!("cached") };
        assert_eq!((e.meta["look"].clone(), e.meta["camera"].clone()), (json!("transparent"), json!({ "fx": 1.5 })));
    });
}

// ── 3. Clay, occlusion, and the scene view on its contents ───────────

/// The request line the worker gets for `r` (what the host sends, less the
/// limits it adds).
fn wire(r: &SceneRequest<'_>) -> String {
    let mut line = Request::new(r.views.clone());
    line.transparent = r.transparent;
    line.background = r.background;
    line.clay = r.clay;
    line.ao = r.ao;
    line.ids = r.ids;
    String::from_utf8(line.line()).expect("utf-8")
}

/// Ambient occlusion on the renderer's default settings, as the line carries it.
const AO_ON_THE_WIRE: &str = r#""ao":{"strength":1.0,"radius":0.1}"#;

#[test]
fn the_scene_view_asks_for_contents_framing_and_clay_and_occlusion_on_the_defaults() {
    let bytes = encode(&scene_with_display_fields());
    for look in Look::ALL {
        let r = scene::request_for(&bytes, SceneTarget::Scene, Width::Full, look, Shape::Wide);
        assert_eq!(r.views.len(), 1);
        assert_eq!((r.views[0].view, r.views[0].framing, r.views[0].objects.clone()), (ViewSpec::Auto, Framing::Contents, None), "{look:?}");
        assert_eq!(r.clay, Some(Clay { shell: CLAY_SHELL, contents: CLAY_CONTENTS }), "{look:?}: the renderer's own tones");
        assert_eq!(r.ao, Some(Ao { strength: AO_STRENGTH, radius: AO_RADIUS }), "{look:?}: occlusion on the renderer's own settings");
        assert!(wire(&r).contains(AO_ON_THE_WIRE), "{look:?}: {}", wire(&r));
        assert_eq!(r.transparent, look == Look::Transparent);
        assert_eq!(r.background, None);
    }
    // On the wire: contents framing, clay and occlusion, nothing else changed.
    let r = scene::request_for(&bytes, SceneTarget::Scene, Width::Thumb, Look::Transparent, Shape::Wide);
    let mut line = Request::new(r.views.clone());
    line.clay = r.clay;
    let text = String::from_utf8(line.line()).expect("utf-8");
    assert!(text.contains(r#""framing":"contents""#), "{text}");
    assert!(text.contains(r#""clay":{"shell":[138,134,128],"contents":[206,202,194]}"#), "{text}");
    let text = wire(&r);
    assert!(text.contains(r#""framing":"contents""#) && text.contains(r#""clay":{"shell":[138,134,128],"contents":[206,202,194]}"#), "{text}");
    assert!(text.contains(AO_ON_THE_WIRE), "{text}");
    assert_eq!(Request::parse(text.trim_end().as_bytes()).expect("parse").ao, Some(Ao::default()), "the worker reads the defaults back");
}

#[test]
fn objects_cameras_and_turntable_frames_ask_for_clay_and_occlusion_on_their_own_framing() {
    let bytes = encode(&scene_with_display_fields());
    let mut targets = vec![SceneTarget::Object(0), SceneTarget::Object(3), SceneTarget::Camera(0)];
    targets.extend((0..8).map(SceneTarget::Turntable));
    for t in targets {
        for look in Look::ALL {
            let r = scene::request_for(&bytes, t, Width::Thumb, look, Shape::Wide);
            let mut own = t.view(Width::Thumb.px());
            if scene::shapes_view(t) {
                own.aspect = Some(Shape::Wide.aspect());
            }
            assert_eq!(r.views, [own], "{t:?}: the renderer's own view, bounds framed (an object's in the shape asked)");
            assert_eq!(r.views[0].framing, Framing::Bounds, "{t:?}: never contents");
            assert_eq!(r.clay, Some(Clay::default()), "{t:?} {look:?}");
            assert_eq!(r.ao, Some(Ao::default()), "{t:?} {look:?}: occlusion on the default settings");
            let text = wire(&r);
            assert!(text.contains(AO_ON_THE_WIRE), "{t:?} {look:?}: {text}");
            assert_eq!(Request::parse(text.trim_end().as_bytes()).expect("parse").ao, Some(Ao::default()), "{t:?} {look:?}");
        }
        assert_eq!(scene::framing_for(t), Framing::Bounds);
    }
    let o = scene::request_for(&bytes, SceneTarget::Object(3), Width::Thumb, Look::Opaque, Shape::Wide);
    assert_eq!((o.views[0].view, o.views[0].objects.clone()), (ViewSpec::Auto, Some(vec![3])), "an object framed on itself alone");
    assert_eq!(scene::framing_for(SceneTarget::Scene), Framing::Contents);
}

/// Clay, occlusion and the framing change the pixels but are not in the
/// renderer's digest, so the host keys them: clay and occlusion in the build
/// (every view), the framing in the scene view's digest.
#[test]
fn clay_occlusion_and_the_scene_views_framing_are_in_the_cache_key() {
    let build = scene::SCENE.build();
    assert_eq!(build, format!("scenerender-p{PROTO}-{VERSION}.clay.ao2.ids.shape"));
    let plain = format!("scenerender-p{PROTO}-{VERSION}");
    let clay_only = format!("{plain}.clay");
    let full_size_occlusion = format!("{plain}.clay.ao.ids.shape");
    for look in Look::ALL {
        let name = |b: &str| cache::entry_name("@scene:main", "ab12", Width::Thumb, &look.cache_build(b));
        assert_ne!(name(&build), name(&plain), "{look:?}: never an image drawn without clay");
        assert_ne!(name(&build), name(&clay_only), "{look:?}: never an image drawn in clay without occlusion");
        assert_ne!(name(&build), name(&full_size_occlusion), "{look:?}: never an image drawn with the earlier occlusion");
    }
    assert_eq!(Look::Opaque.cache_build(&build), format!("{plain}.clay.ao2.ids.shape"));
    assert_eq!(Look::Transparent.cache_build(&build), format!("{plain}.clay.ao2.ids.shape.rgba"));
    assert_ne!(Look::Transparent.cache_build(&build), Look::Transparent.cache_build(&clay_only));

    // A scene of one object: the renderer's digest of `@scene:main` and of that
    // object's view is one, but they are framed differently.
    let bytes = encode(&testgen::cube_scene());
    let raw = k2_scenerender::digest::list(&bytes, &k2_scenerender::scene::InputLimits::default()).expect("raw");
    let (rs, ro) = (raw.iter().find(|i| i.key == "@scene:main").expect("scene"), raw.iter().find(|i| i.key.starts_with("@obj:")).expect("object"));
    assert_eq!(rs.digest, ro.digest, "the renderer's digest leaves the framing out");
    let l = scene::list_k2scene(&bytes).expect("a listing");
    let (s, o) = (l.containers.iter().find(|c| c.key == "@scene:main").expect("scene"), l.containers.iter().find(|c| c.key.starts_with("@obj:")).expect("object"));
    assert_ne!(s.digest, o.digest, "the scene view's framing is keyed: no image is shared between them");
    assert_eq!(s.digest, scene::keyed_digest(SceneTarget::Scene, rs.digest));
    assert_eq!(o.digest, ro.digest, "a bounds-framed view keeps the renderer's digest");
    // Every other view keeps the renderer's digest.
    let both = scene::list_k2scene(&encode(&scene_with_display_fields())).expect("a listing");
    let raw_both = k2_scenerender::digest::list(&encode(&scene_with_display_fields()), &k2_scenerender::scene::InputLimits::default()).expect("raw");
    for (c, r) in both.containers.iter().zip(&raw_both) {
        assert_eq!(c.digest == r.digest, c.key != "@scene:main", "{}", c.key);
    }
}

// ── 4. The events, in order ──────────────────────────────────────────

#[test]
fn a_render_pushes_rendering_then_ready_with_its_ms_and_modifiers() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("events-ok.blend");
        let s = snap(&blobs, &key, 4, "@obj:Ok 1");
        let hash = s.containers[0].hash.clone();
        let sub = host_events::subscribe(&key);
        assert_eq!(service::request(&blobs, &s, "@obj:Ok", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Rendering);
        let first = next(&sub, "rendering");
        assert!(matches!(first, HostEvent::PreviewRendering { .. }), "{first:?}");
        assert_eq!(frame(&first), format!(r#"{{"type":"preview_rendering","container":"@obj:Ok","render_hash":"{hash}","width":"thumb","rev":4}}"#));
        let second = next(&sub, "ready");
        let f = frame(&second);
        let HostEvent::PreviewReady { rendered_at_ms, .. } = second else { panic!("{f}") };
        assert_eq!(
            f,
            format!(
                r#"{{"type":"preview_ready","container":"@obj:Ok","render_hash":"{hash}","width":"thumb","state":"ready","failed":null,"rev":4,"rendered_at_ms":{rendered_at_ms},"render_ms":7,"without_modifiers":true,"camera":{{"fx":1.5}}}}"#
            ),
            "the old fields first, in order, then the new ones"
        );
        assert!(sub.rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing more");
    });
}

#[test]
fn a_failed_render_pushes_rendering_then_failed_with_its_code() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("events-fail.blend");
        let s = snap(&blobs, &key, 2, "@obj:Bad:fail 1");
        let sub = host_events::subscribe(&key);
        assert_eq!(service::request(&blobs, &s, "@obj:Bad:fail", Width::Full, Look::Opaque, Shape::Wide), Requested::Rendering);
        assert!(frame(&next(&sub, "rendering")).starts_with(r#"{"type":"preview_rendering","container":"@obj:Bad:fail""#));
        let f = frame(&next(&sub, "failed"));
        assert!(f.starts_with(r#"{"type":"preview_ready","container":"@obj:Bad:fail""#), "{f}");
        assert!(f.contains(r#""width":"full","state":"failed","failed":{"code":"timeout"},"rev":2"#), "{f}");
        assert!(!f.contains("render_ms"), "a failure has no image to time: {f}");
        // Asked again: the failure, no new render and no new events.
        assert_eq!(service::request(&blobs, &s, "@obj:Bad:fail", Width::Full, Look::Opaque, Shape::Wide), Requested::Failed(FailCode::Timeout));
        assert_eq!(renders_of("@obj:Bad:fail").len(), 1);
        assert!(sub.rx.recv_timeout(Duration::from_millis(300)).is_err());
    });
}

// ── 5. A rename is answered from the cache ───────────────────────────

#[test]
fn a_renamed_container_is_ready_at_once_from_the_cache_without_a_render() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("rename.blend");
        let a = snap(&blobs, &key, 1, "@scene:main 9\n@obj:Old 5");
        let sub = host_events::subscribe(&key);
        assert_eq!(service::request(&blobs, &a, "@obj:Old", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Rendering);
        next(&sub, "rendering");
        assert!(frame(&next(&sub, "ready")).contains(r#""state":"ready""#));
        assert_eq!(renders_of("@obj:Old").len(), 1);

        // The object renamed: same digest, new address.
        let b = snap(&blobs, &key, 2, "@scene:main 9\n@obj:New 5");
        let new = b.container("@obj:New").expect("renamed").clone();
        assert_eq!(new.hash, a.container("@obj:Old").expect("old").hash, "a rename keeps the hash");
        assert_eq!(reuse::widths_to_redraw(&a, &new, "fake-1"), [Width::Thumb], "the head's re-draw asks for the old address's widths");
        assert_eq!(service::request(&blobs, &b, "@obj:New", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Reused { from: "@obj:Old".into() });
        let ev = next(&sub, "ready at once");
        let f = frame(&ev);
        assert!(matches!(ev, HostEvent::PreviewReady { state: "ready", .. }), "no rendering first: {f}");
        assert!(f.starts_with(&format!(r#"{{"type":"preview_ready","container":"@obj:New","render_hash":"{}","width":"thumb","state":"ready","failed":null,"rev":2,"#, new.hash)), "{f}");
        assert!(f.contains(r#""render_ms":0,"without_modifiers":true,"reused_from":"@obj:Old","camera":{"fx":1.5}"#), "{f}");
        assert!(renders_of("@obj:New").is_empty(), "no worker ran");
        assert_eq!(renders_of("@obj:Old").len(), 1);
        let Got::Hit(e) = cache::get(&blobs, &key, "@obj:New", &new.hash, Width::Thumb, "fake-1") else { panic!("cached under the new address") };
        assert_eq!(e.image, b"PIXELS");
        assert_eq!((e.meta["container"].clone(), e.meta["reused_from"].clone(), e.meta["rev"].clone()), (json!("@obj:New"), json!("@obj:Old"), json!(2)));

        // Another look or width isn't the same image: it is drawn.
        assert_eq!(service::request(&blobs, &b, "@obj:New", Width::Thumb, Look::Transparent, Shape::Wide), Requested::Rendering);
        next(&sub, "rendering");
        next(&sub, "ready");
        assert_eq!(renders_of("@obj:New"), [Look::Transparent]);
        // A real edit (a new digest) is drawn too.
        let c = snap(&blobs, &key, 3, "@scene:main 9\n@obj:New 6");
        assert_eq!(service::request(&blobs, &c, "@obj:New", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Rendering);
        assert!(matches!(next(&sub, "rendering"), HostEvent::PreviewRendering { .. }));
        next(&sub, "ready");
        assert_eq!(renders_of("@obj:New").len(), 2);
    });
}

// ── 6. Id images ─────────────────────────────────────────────────────

#[test]
fn the_scene_objects_and_cameras_ask_for_ids_and_the_turntable_frames_dont() {
    let bytes = encode(&scene_with_display_fields());
    let mut targets = vec![(SceneTarget::Scene, true), (SceneTarget::Object(0), true), (SceneTarget::Object(3), true), (SceneTarget::Camera(0), true)];
    targets.extend((0..8).map(|k| (SceneTarget::Turntable(k), false)));
    for (t, asks) in targets {
        assert_eq!(scene::wants_ids(t), asks, "{t:?}");
        for look in Look::ALL {
            for w in Width::ALL {
                let r = scene::request_for(&bytes, t, w, look, Shape::Wide);
                assert_eq!(r.ids, asks, "{t:?} {look:?} {w:?}");
                let text = wire(&r);
                assert_eq!(text.contains(r#""ids":true"#), asks, "{t:?} {look:?}: {text}");
                assert_eq!(text.contains(r#""ids""#), asks, "{t:?} {look:?}: off the line when not asked: {text}");
                assert_eq!(Request::parse(text.trim_end().as_bytes()).expect("parse").ids, asks, "{t:?} {look:?}");
                // Nothing else changes with it.
                assert_eq!((r.clay, r.ao, r.transparent), (Some(Clay::default()), Some(Ao::default()), look == Look::Transparent));
            }
        }
    }
    // The listing's containers say the same (the route's `layer=ids` goes by it): all but the 8 frames.
    let l = scene::list_k2scene(&bytes).expect("a listing");
    for c in &l.containers {
        assert_eq!(scene::SCENE.draws_ids(c), !c.key.starts_with("@turntable:"), "{}", c.key);
    }
    assert_eq!(l.containers.iter().filter(|c| !scene::SCENE.draws_ids(c)).count(), 8);
    assert!(l.containers.iter().all(|c| !deck::DECK.draws_ids(c)), "a deck draws none");
}

/// Ids are keyed in the build (`.ids` after `.clay.ao2`): no image cached
/// before the views asked for them is served. The id image is an entry of
/// its own beside the image, never found as an image.
#[test]
fn ids_are_in_the_cache_key_and_the_id_image_has_an_entry_of_its_own() {
    let build = scene::SCENE.build();
    let before = format!("scenerender-p{PROTO}-{VERSION}.clay.ao2");
    assert_eq!(build, format!("{before}.ids.shape"));
    for look in Look::ALL {
        let name = |b: &str| cache::entry_name("@scene:main", "ab12", Width::Thumb, b);
        assert_ne!(name(&look.cache_build(&build)), name(&look.cache_build(&before)), "{look:?}: never an image drawn without its id image");
        let ids = name(&cache::ids_build(&look.cache_build(&build)));
        assert_ne!(ids, name(&look.cache_build(&build)), "{look:?}");
        // Not in the image's slot (trimmed, re-drawn and reused by that suffix).
        assert!(!ids.ends_with(&format!("-thumb-{}.{}", look.cache_build(&build), cache::EXT)), "{look:?}: {ids}");
    }
    assert_eq!(cache::ids_build(&Look::Transparent.cache_build(&build)), format!("{before}.ids.shape.rgba.idimg"));
    assert_eq!(cache::ids_build(&cache_build(&build, Look::Transparent, Shape::Tall)), format!("{before}.ids.shape.rgba.tall.idimg"));
}

#[test]
fn an_id_table_rides_after_the_camera_and_a_push_lists_it_within_its_bound() {
    let shared = "2O2Fr$t4X7Zf8NOew3FLOH";
    let m = IdsMeta {
        image_len: 77,
        count: 4,
        objects: vec![
            IdEntry { index: 0, id: shared.into() },
            IdEntry { index: 3, id: shared.into() },
            IdEntry { index: 4, id: shared.to_lowercase() },
            IdEntry { index: 9, id: " Wall \"A\" ".into() },
        ],
    };
    let cam = CameraMeta { view: testgen::identity(), proj: testgen::identity(), fx: 1.0, fy: 1.0, near: 0.1, shift_x: 0.0, shift_y: 0.0 };
    let x = extras::scene_view(&ViewMeta { camera: Some(cam), ids: Some(m.clone()), ..Default::default() });
    assert_eq!(
        x["ids"],
        json!({ "layer": "ids", "image_len": 77, "count": 4, "objects": [
            { "index": 0, "id": shared }, { "index": 3, "id": shared }, { "index": 4, "id": shared.to_lowercase() }, { "index": 9, "id": " Wall \"A\" " }
        ] }),
        "ids as they are: repeated, case and spaces kept"
    );
    assert!(x.contains_key("camera"));
    assert!(!extras::scene_view(&ViewMeta { ids: None, ..Default::default() }).contains_key("ids"), "no ids asked, no table");
    // On the socket, after the camera.
    let fields = service::ready_fields(&serde_json::Value::Object(x.clone()));
    let names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["render_ms", "without_modifiers", "camera", "ids"]);
    assert_eq!(extras::ids_for_push(&x["ids"]), x["ids"], "a table within the bound goes whole");
    // Past the bound a push sends the count alone (the route has the table).
    let big = IdsMeta { image_len: 9, count: 5_000, objects: (0..5_000).map(|i| IdEntry { index: i, id: "x".repeat(300) }).collect() };
    let full = extras::ids_meta(&big);
    assert!(serde_json::to_vec(&full["objects"]).expect("json").len() > extras::PUSH_IDS_MAX);
    let pushed = extras::ids_for_push(&full);
    assert_eq!(pushed, json!({ "layer": "ids", "image_len": 9, "count": 5_000, "objects": [] }));
}

/// An object hidden in the file is listed with its box: its container is
/// `hidden` and `hidden_by` says how (the route keeps `hidden` a bool). A
/// driven object's box is approximate for that reason, the modifiers'
/// reason winning. Only the views that leave a hidden object out get new
/// digests; a scene with neither field lists as it did.
#[test]
fn a_hidden_object_is_listed_with_its_box_and_a_driven_ones_box_is_approx() {
    use k2_scenerender::scene::Hidden;
    let mut s = scene_with_display_fields();
    s.objects[0].hidden = Some(Hidden::Itself);
    s.objects[1].transform_not_evaluated = true;
    s.objects[2].hidden = Some(Hidden::Collection);
    s.objects[2].transform_not_evaluated = true;
    s.objects[3].transform_not_evaluated = true;
    let l = scene::list_k2scene(&encode(&s)).expect("a listing");
    let plain = scene::list_k2scene(&encode(&scene_with_display_fields())).expect("plain");
    let by = |l: &'_ Listing, key: &str| -> Container { l.containers.iter().find(|c| c.key == key).cloned().unwrap_or_else(|| panic!("no {key}")) };
    // (key, container hidden, hidden_by, approx, approx_reason)
    let want: [(&str, bool, Option<&str>, bool, Option<&str>); 4] = [
        ("@obj:Item.000", true, Some("self"), false, None),
        ("@obj:Item.001", false, None, true, Some("modifiers")),
        ("@obj:Item.002", true, Some("collection"), true, Some("driven")),
        ("@obj:Item.003", false, None, true, Some("driven")),
    ];
    for (key, hidden, by_what, approx, reason) in want {
        let c = by(&l, key);
        assert_eq!(c.hidden, hidden, "{key}");
        assert_eq!(c.extras.get("hidden_by").and_then(|v| v.as_str()), by_what, "{key}");
        let b = &c.extras["bounds"];
        assert_eq!(b["approx"], json!(approx), "{key}");
        assert_eq!(b.get("approx_reason").and_then(|v| v.as_str()), reason, "{key}");
        assert_eq!(b["min"], by(&plain, key).extras["bounds"]["min"], "{key}: the same box, hidden or not");
        // Hidden or not, an object's own view draws it; the flag it carries, base values, is in its digest.
        assert_eq!(c.digest == by(&plain, key).digest, key == "@obj:Item.000", "{key}");
    }
    for key in ["@scene:main", "@cam:Camera", "@turntable:0", "@turntable:7"] {
        assert!(!by(&l, key).hidden && by(&l, key).extras.get("hidden_by").is_none(), "{key}");
        assert_ne!(by(&l, key).digest, by(&plain, key).digest, "{key}: it leaves the hidden ones out");
    }
    // Neither field: nothing hidden, and only the modifiers' reason.
    assert!(plain.containers.iter().all(|c| !c.hidden && !c.extras.contains_key("hidden_by")));
    assert_eq!(by(&plain, "@obj:Item.001").extras["bounds"]["approx_reason"], json!("modifiers"));
    assert!(plain.containers.iter().filter(|c| c.key != "@obj:Item.001").all(|c| c.extras.get("bounds").is_none_or(|b| b.get("approx_reason").is_none())));
    // Through the route and the welcome: `hidden` stays the container's bool.
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("scenes/hidden.blend");
        let secret = *blobs.host_cache_key(&key.space, service::HASH_LABEL).expect("the cache key").secret_bytes();
        let snap = Snapshot {
            key: key.clone(),
            rev: 1,
            version: None,
            renderer: &scene::SCENE,
            build: scene::SCENE.build(),
            page: l.page,
            containers: l.containers.iter().cloned().map(|c| Keyed { hash: service::keyed_hash(&secret, &c.digest), c }).collect(),
            bytes: Arc::new(Vec::new()),
        };
        let listed: Vec<serde_json::Value> = snap.containers.iter().map(|k| route::container_json(&snap, k, Look::Transparent, Shape::Wide)).collect();
        let obj = listed.iter().find(|v| v["key"] == "@obj:Item.000").expect("the hidden object");
        assert_eq!((obj["hidden"].clone(), obj["hidden_by"].clone(), obj["bounds"]["approx"].clone()), (json!(true), json!("self"), json!(false)));
        let welcome = crate::docs::live_deck::previews_value(&snap).to_json();
        assert!(welcome.contains(r#""title":"Item.000","hidden":true,"render_hash":""#), "{welcome}");
        assert!(welcome.contains(r#""hidden_by":"self""#) && welcome.contains(r#""approx_reason":"driven""#), "{welcome}");
    });
}

/// A view drawing an object at its base transform says `base_values`, one
/// that left hidden objects out counts them (`hidden_left_out`), with the
/// image and on the push; neither is said when there is nothing to say.
#[test]
fn base_values_and_hidden_left_out_ride_with_the_image_and_the_push() {
    let x = extras::scene_view(&ViewMeta { base_values: true, hidden_left_out: 2, ..Default::default() });
    assert_eq!(serde_json::Value::Object(x.clone()), json!({ "base_values": true, "hidden_left_out": 2 }));
    assert!(extras::scene_view(&ViewMeta::default()).is_empty(), "nothing to say, nothing said");
    let fields = service::ready_fields(&serde_json::Value::Object(x));
    let names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["render_ms", "without_modifiers", "base_values", "hidden_left_out"]);
    // The real worker.
    use k2_scenerender::scene::Hidden;
    let bin = k2_core::test_env::workspace_bin(scene::ENV_BIN, "k2-scenerender", BINARY_NAME);
    let mut s = scene_with_display_fields();
    s.objects[0].hidden = Some(Hidden::Itself);
    s.objects[2].transform_not_evaluated = true;
    let bytes = encode(&s);
    let l = scene::list_k2scene(&bytes).expect("listing");
    let draw = |key: &str| -> Image {
        let c = l.containers.iter().find(|c| c.key == key).unwrap_or_else(|| panic!("no {key}"));
        scene::render_k2scene_with(&bin, &bytes, c, Width::Thumb, Look::Opaque, Shape::Wide, Duration::from_secs(60)).unwrap_or_else(|f| panic!("{key}: {f:?}"))
    };
    for key in ["@scene:main", "@cam:Camera", "@turntable:3"] {
        let img = draw(key);
        assert_eq!((img.extras.get("base_values"), img.extras.get("hidden_left_out")), (Some(&json!(true)), Some(&json!(1))), "{key}");
        assert!(img.without_modifiers, "{key}");
        if let Some(t) = img.extras.get("ids") {
            assert!(t["objects"].as_array().expect("objects").iter().all(|e| e["index"] != json!(0)), "{key}: the hidden object has no pixels");
        }
    }
    // The hidden object's own view draws it, and says nothing of either.
    let o = draw("@obj:Item.000");
    assert_eq!(o.extras["ids"]["objects"], json!([{ "index": 0, "id": "Item.000" }]));
    assert!(!o.extras.contains_key("base_values") && !o.extras.contains_key("hidden_left_out"));
    assert_eq!(draw("@obj:Item.002").extras.get("base_values"), Some(&json!(true)));
}

fn body(r: crate::cli_response::CliResponse) -> serde_json::Value {
    serde_json::from_str(&r.body).expect("a JSON answer")
}

fn b64(v: &serde_json::Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v["base64"].as_str().expect("base64")).expect("decodes")
}

#[test]
fn a_scene_image_carries_its_id_table_and_its_id_image_is_served_beside_it() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("ids.blend");
        let s = snap(&blobs, &key, 1, "@scene:main 9 scene\n@obj:Chair 5 o0\n@obj:Lamp 6 o1\n@turntable:0 7 t0");
        let sub = host_events::subscribe(&key);
        assert_eq!(service::request(&blobs, &s, "@scene:main", Width::Full, Look::Transparent, Shape::Wide), Requested::Rendering);
        next(&sub, "rendering");
        let ready = frame(&next(&sub, "ready"));
        let table = json!({ "layer": "ids", "image_len": 5, "count": 2, "objects": [{ "index": 0, "id": "Chair" }, { "index": 1, "id": "Lamp" }] });
        let v: serde_json::Value = serde_json::from_str(&ready).expect("json");
        assert_eq!((v["camera"].clone(), v["ids"].clone()), (json!({ "fx": 1.5 }), table.clone()), "{ready}");
        assert!(ready.find(r#""camera""#) < ready.find(r#""ids""#), "after the camera: {ready}");

        // Cached: the table in the image's metadata, the id image in its own entry.
        let hash = s.container("@scene:main").expect("scene").hash.clone();
        let build = Look::Transparent.cache_build("fake-1");
        let Got::Hit(img) = cache::get(&blobs, &key, "@scene:main", &hash, Width::Full, &build) else { panic!("the image") };
        assert_eq!((img.image.as_slice(), img.meta["ids"].clone()), (&b"PIXELS"[..], table.clone()));
        let Got::Hit(ids) = cache::get(&blobs, &key, "@scene:main", &hash, Width::Full, &cache::ids_build(&build)) else { panic!("the id image") };
        assert_eq!(ids.image, FAKE_IDS);
        assert_eq!(
            (ids.meta["layer"].clone(), ids.meta["mime"].clone(), ids.meta["container"].clone(), ids.meta["render_hash"].clone(), ids.meta["look"].clone()),
            (json!("ids"), json!("image/png"), json!("@scene:main"), json!(hash), json!("transparent"))
        );
        assert!(ids.meta.get("ids").is_none() && ids.meta.get("camera").is_none(), "the table stays with the image: {}", ids.meta);

        // The route: the image's answer carries the table; `layer=ids` answers the id image.
        let a = body(route::image(&blobs, &s, "@scene:main", None, Width::Full, Look::Transparent, Shape::Wide, false));
        assert_eq!((a["state"].clone(), a["ids"].clone(), b64(&a)), (json!("ready"), table.clone(), b"PIXELS".to_vec()));
        let i = body(route::image(&blobs, &s, "@scene:main", Some(&hash), Width::Full, Look::Transparent, Shape::Wide, true));
        assert_eq!((i["state"].clone(), i["layer"].clone(), i["format"].clone(), b64(&i)), (json!("ready"), json!("ids"), json!("png"), FAKE_IDS.to_vec()));
        assert_eq!(i["size"], a["size"], "at the still's size");
        // Another look has its own (not drawn yet).
        assert_eq!(route::image(&blobs, &s, "@scene:main", None, Width::Full, Look::Opaque, Shape::Wide, true).status, "202 Accepted");
        next(&sub, "rendering");
        next(&sub, "ready");

        // A turntable frame has neither.
        assert_eq!(service::request(&blobs, &s, "@turntable:0", Width::Thumb, Look::Transparent, Shape::Wide), Requested::Rendering);
        next(&sub, "rendering");
        let t = frame(&next(&sub, "ready"));
        assert!(t.contains(r#""camera":{"fx":1.5}"#) && !t.contains(r#""ids""#), "{t}");
        let r = route::image(&blobs, &s, "@turntable:0", None, Width::Thumb, Look::Transparent, Shape::Wide, true);
        assert_eq!(r.status, "404 Not Found");
        assert!(r.body.contains("no_id_image"), "{}", r.body);
        assert!(body(route::image(&blobs, &s, "@turntable:0", None, Width::Thumb, Look::Transparent, Shape::Wide, false)).get("ids").is_none());

        // A cached image whose id image went (the cap) is drawn again when it is asked for.
        std::fs::remove_file(cache::doc_dir(&key).join(cache::entry_name("@scene:main", &hash, Width::Full, &cache::ids_build(&build)))).expect("remove");
        let r = route::image(&blobs, &s, "@scene:main", None, Width::Full, Look::Transparent, Shape::Wide, true);
        assert_eq!((r.status, body(r)["state"].clone()), ("202 Accepted", json!("rendering")), "never a stale id image");
        assert!(matches!(next(&sub, "rendering"), HostEvent::PreviewRendering { .. }), "drawn again");
        next(&sub, "ready");
        assert_eq!(b64(&body(route::image(&blobs, &s, "@scene:main", None, Width::Full, Look::Transparent, Shape::Wide, true))), FAKE_IDS);
    });
}

/// A rename changes no render hash: the renamed object's image and id image
/// are reused, and every current image's table names the objects as the doc
/// does now (the pixels hold indexes, which stay right).
#[test]
fn a_rename_reuses_the_id_image_and_its_table_names_the_new_id() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("rename-ids.blend");
        let a = snap(&blobs, &key, 1, "@scene:main 9 scene\n@obj:Before 5 o0\n@obj:Other 6 o1");
        let sub = host_events::subscribe(&key);
        for c in ["@obj:Before", "@scene:main"] {
            assert_eq!(service::request(&blobs, &a, c, Width::Thumb, Look::Opaque, Shape::Wide), Requested::Rendering);
            next(&sub, "rendering");
            next(&sub, "ready");
        }

        // Before renamed After: reused, its table names After, its id image came along.
        let b = snap(&blobs, &key, 2, "@scene:main 9 scene\n@obj:After 5 o0\n@obj:Other 6 o1");
        let new = b.container("@obj:After").expect("renamed").clone();
        assert_eq!(service::request(&blobs, &b, "@obj:After", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Reused { from: "@obj:Before".into() });
        let f: serde_json::Value = serde_json::from_str(&frame(&next(&sub, "ready at once"))).expect("json");
        assert_eq!((f["reused_from"].clone(), f["ids"]["objects"].clone()), (json!("@obj:Before"), json!([{ "index": 0, "id": "After" }])), "{f}");
        assert!(renders_of("@obj:After").is_empty(), "no worker ran");
        let Got::Hit(e) = cache::get(&blobs, &key, "@obj:After", &new.hash, Width::Thumb, "fake-1") else { panic!("cached under the new address") };
        assert_eq!(e.meta["ids"]["objects"], json!([{ "index": 0, "id": "After" }]), "stored as it is now");
        let Got::Hit(i) = cache::get(&blobs, &key, "@obj:After", &new.hash, Width::Thumb, &cache::ids_build("fake-1")) else { panic!("the id image came along") };
        assert_eq!((i.image.as_slice(), i.meta["container"].clone(), i.meta["reused_from"].clone()), (FAKE_IDS, json!("@obj:After"), json!("@obj:Before")));
        let r = body(route::image(&blobs, &b, "@obj:After", None, Width::Thumb, Look::Opaque, Shape::Wide, false));
        assert_eq!(r["ids"]["objects"], json!([{ "index": 0, "id": "After" }]));
        assert_eq!(b64(&body(route::image(&blobs, &b, "@obj:After", None, Width::Thumb, Look::Opaque, Shape::Wide, true))), FAKE_IDS);

        // The scene view keeps its address and hash: its cached image is answered named as now, without a render.
        let r = body(route::image(&blobs, &b, "@scene:main", None, Width::Thumb, Look::Opaque, Shape::Wide, false));
        assert_eq!(r["ids"]["objects"], json!([{ "index": 0, "id": "After" }, { "index": 1, "id": "Other" }]));
        assert!(sub.rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing drawn");

        // An object added before it, and it renamed again: its view's hash is the same, so it is
        // reused; the index in the image stays (the pixels hold it) and names the object itself.
        let c = snap(&blobs, &key, 3, "@scene:main 8 scene\n@obj:Added 3 o0\n@obj:Again 5 o1\n@obj:Other 6 o2");
        let Requested::Reused { from } = service::request(&blobs, &c, "@obj:Again", Width::Thumb, Look::Opaque, Shape::Wide) else { panic!("reused") };
        assert!(from == "@obj:After" || from == "@obj:Before", "{from}");
        let f: serde_json::Value = serde_json::from_str(&frame(&next(&sub, "ready at once"))).expect("json");
        assert_eq!(f["ids"]["objects"], json!([{ "index": 0, "id": "Again" }]), "{f}");
        // The scene's image from before (another hash now) keeps the names it was drawn with.
        let old_scene = a.container("@scene:main").expect("scene").hash.clone();
        let r = body(route::image(&blobs, &c, "@scene:main", Some(&old_scene), Width::Thumb, Look::Opaque, Shape::Wide, false));
        assert_eq!(r["ids"]["objects"], json!([{ "index": 0, "id": "Before" }, { "index": 1, "id": "Other" }]));
    });
}

// ── 7. Objects renamed in place: the tables pushed again ─────────────

/// Draw `container` at `w` in `look` and read its `ready` off `sub`.
fn draw_now(blobs: &Arc<BlobStore>, s: &Arc<Snapshot>, sub: &Subscription, container: &str, w: Width, look: Look) -> serde_json::Value {
    assert_eq!(service::request(blobs, s, container, w, look, Shape::Wide), Requested::Rendering, "{container}");
    assert!(matches!(next(sub, "rendering"), HostEvent::PreviewRendering { .. }), "{container}");
    serde_json::from_str(&frame(&next(sub, "ready"))).expect("json")
}

/// Every event `sub` has now, then none within 300 ms (no worker started).
fn pushed_now(sub: &Subscription) -> Vec<HostEvent> {
    let mut v: Vec<HostEvent> = sub.rx.try_iter().collect();
    if let Ok(late) = sub.rx.recv_timeout(Duration::from_millis(300)) {
        v.push(late);
    }
    v
}

const PLACE: &str = "@scene:main 9 scene\n@cam:Main 4 c0\n@obj:Chair 5 o0\n@obj:Lamp 6 o1\n@turntable:0 7 t0";

/// An object renamed in place: the scene and camera views keep their
/// address and render hash, so nothing is drawn; each of their cached
/// images is pushed `ready` once more with the new id in its table, and the
/// renamed object's own view is reused under its new address.
#[test]
fn a_rename_pushes_the_scene_and_camera_tables_again_without_a_render() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("renamed-in-place.blend");
        let a = snap(&blobs, &key, 1, PLACE);
        let sub = host_events::subscribe(&key);
        let views = [
            ("@scene:main", Width::Thumb, Look::Opaque),
            ("@scene:main", Width::Full, Look::Transparent),
            ("@cam:Main", Width::Thumb2x, Look::Transparent),
            ("@obj:Lamp", Width::Thumb, Look::Transparent),
            ("@turntable:0", Width::Thumb, Look::Transparent),
        ];
        let mut at = std::collections::HashMap::new();
        for (c, w, l) in views {
            let r = draw_now(&blobs, &a, &sub, c, w, l);
            at.insert(format!("{c} {}", w.as_str()), r["rendered_at_ms"].clone());
        }

        let b = snap(&blobs, &key, 2, &PLACE.replace("@obj:Lamp", "@obj:Light"));
        service::head_moved(&blobs, &a, &b);
        let evs = pushed_now(&sub);
        assert!(evs.iter().all(|e| matches!(e, HostEvent::PreviewReady { state: "ready", .. })), "no worker ran: {evs:?}");
        let frames: Vec<serde_json::Value> = evs.iter().map(|e| serde_json::from_str(&frame(e)).expect("json")).collect();
        let renamed = json!([{ "index": 0, "id": "Chair" }, { "index": 1, "id": "Light" }]);

        // The object's own view: reused under its new address, as before.
        let obj: Vec<&serde_json::Value> = frames.iter().filter(|f| f["container"] == "@obj:Light").collect();
        assert_eq!(obj.len(), 1, "{frames:?}");
        assert_eq!((obj[0]["reused_from"].clone(), obj[0]["ids"]["objects"].clone()), (json!("@obj:Lamp"), json!([{ "index": 1, "id": "Light" }])));

        // The scene and the camera: once per cached (width, look), same hash, the new name.
        let mut again: Vec<(String, String, Option<String>)> = Vec::new();
        for f in frames.iter().filter(|f| f["container"] != "@obj:Light") {
            let c = f["container"].as_str().expect("a container");
            let hash = &b.container(c).expect("listed").hash;
            assert_eq!(hash, &a.container(c).expect("listed").hash, "{c}: a rename keeps the hash");
            assert_eq!((f["render_hash"].clone(), f["state"].clone(), f["rev"].clone()), (json!(hash), json!("ready"), json!(2)), "{f}");
            assert_eq!(f["ids"]["objects"], renamed, "{f}");
            assert_eq!(f["ids"]["count"], json!(2), "{f}");
            let w = f["width"].as_str().expect("a width");
            assert_eq!(f["rendered_at_ms"], at[&format!("{c} {w}")], "{c} {w}: the image is the one drawn then");
            assert_eq!((f["render_ms"].clone(), f["camera"].clone()), (json!(7), json!({ "fx": 1.5 })), "the image's own fields");
            assert!(f.get("reused_from").is_none(), "{f}");
            again.push((c.to_string(), w.to_string(), f["look"].as_str().map(str::to_string)));
        }
        again.sort();
        assert_eq!(
            again,
            [
                ("@cam:Main".to_string(), "thumb2x".to_string(), Some("transparent".to_string())),
                ("@scene:main".to_string(), "full".to_string(), Some("transparent".to_string())),
                ("@scene:main".to_string(), "thumb".to_string(), None),
            ],
            "the turntable frame has no table; nothing else was cached"
        );
        // A ready as every older client reads it: the old fields first, in order.
        let f = frame(evs.iter().find(|e| matches!(e, HostEvent::PreviewReady { container, width: "thumb", .. } if container == "@scene:main")).expect("the scene's thumb"));
        let hash = &b.container("@scene:main").expect("scene").hash;
        assert!(
            f.starts_with(&format!(r#"{{"type":"preview_ready","container":"@scene:main","render_hash":"{hash}","width":"thumb","state":"ready","failed":null,"rev":2,"rendered_at_ms":{},"render_ms":7,"without_modifiers":true,"shape":"wide","camera":{{"fx":1.5}},"ids":{{"#, at["@scene:main thumb"])),
            "{f}"
        );

        // The cache keeps the ids it was drawn with; the route names them as now.
        let Got::Hit(e) = cache::get(&blobs, &key, "@scene:main", hash, Width::Thumb, "fake-1") else { panic!("cached") };
        assert_eq!(e.meta["ids"]["objects"][1]["id"], json!("Lamp"));
        assert_eq!(body(route::image(&blobs, &b, "@scene:main", None, Width::Thumb, Look::Opaque, Shape::Wide, false))["ids"]["objects"], renamed);
        assert!(pushed_now(&sub).is_empty(), "the route drew nothing");

        // A head move that renames nothing pushes nothing: the same listing,
        // a camera added, an object added that the tables don't list.
        let c = snap(&blobs, &key, 3, &PLACE.replace("@obj:Lamp", "@obj:Light"));
        service::head_moved(&blobs, &b, &c);
        assert!(pushed_now(&sub).is_empty(), "nothing renamed");
        let d = snap(&blobs, &key, 4, &format!("{}\n@cam:Side 3 c1\n@obj:Vase 2 o2", PLACE.replace("@obj:Lamp", "@obj:Light")));
        service::head_moved(&blobs, &c, &d);
        assert!(pushed_now(&sub).is_empty(), "the tables are as they were");

        // Renamed again: pushed again, from the names the last listing gave.
        let e = snap(&blobs, &key, 5, &format!("{}\n@cam:Side 3 c1\n@obj:Vase 2 o2", PLACE.replace("@obj:Lamp", "@obj:Lantern")));
        service::head_moved(&blobs, &d, &e);
        let again: Vec<serde_json::Value> = pushed_now(&sub).iter().map(|e| serde_json::from_str(&frame(e)).expect("json")).collect();
        let views: Vec<&serde_json::Value> = again.iter().filter(|f| f["container"] == "@scene:main" || f["container"] == "@cam:Main").collect();
        assert_eq!(views.len(), 3, "{again:?}");
        assert!(views.iter().all(|f| f["ids"]["objects"][1]["id"] == "Lantern" && f["rev"] == 5), "{again:?}");
        assert_eq!(again.len(), 4, "and the object's own view reused: {again:?}");
    });
}

/// An object the read keys keeps its address when renamed (its live key),
/// and so its render hash: its own view is not drawn or reused but pushed
/// `ready` again with its table naming the new name, as the scene and
/// camera views are.
#[test]
fn a_keyed_rename_keeps_the_address_and_pushes_the_objects_own_table() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("renamed-keyed.blend");
        let listing = "@scene:main 9 scene\n@cam:3 4 c0 Main\n@obj:7 5 o0 Chair\n@obj:8 6 o1 Lamp";
        let a = snap(&blobs, &key, 1, listing);
        let sub = host_events::subscribe(&key);
        for c in ["@obj:8", "@scene:main"] {
            draw_now(&blobs, &a, &sub, c, Width::Thumb, Look::Transparent);
        }
        let b = snap(&blobs, &key, 2, &listing.replace("o1 Lamp", "o1 Light"));
        assert_eq!(b.container("@obj:8").expect("the same address").hash, a.container("@obj:8").expect("keyed").hash);
        assert!(service::dirty(&a, &b).is_empty(), "nothing to draw");
        service::head_moved(&blobs, &a, &b);
        let evs = pushed_now(&sub);
        assert!(evs.iter().all(|e| matches!(e, HostEvent::PreviewReady { state: "ready", .. })), "no worker ran: {evs:?}");
        let frames: Vec<serde_json::Value> = evs.iter().map(|e| serde_json::from_str(&frame(e)).expect("json")).collect();
        let own: Vec<&serde_json::Value> = frames.iter().filter(|f| f["container"] == "@obj:8").collect();
        assert_eq!(own.len(), 1, "{frames:?}");
        assert_eq!((own[0]["ids"]["objects"].clone(), own[0].get("reused_from")), (json!([{ "index": 1, "id": "Light" }]), None));
        let scene: Vec<&serde_json::Value> = frames.iter().filter(|f| f["container"] == "@scene:main").collect();
        assert_eq!(scene.len(), 1, "{frames:?}");
        assert_eq!(scene[0]["ids"]["objects"], json!([{ "index": 0, "id": "Chair" }, { "index": 1, "id": "Light" }]));
        assert_eq!(frames.len(), 2, "{frames:?}");
        assert_eq!(body(route::image(&blobs, &b, "@obj:8", None, Width::Thumb, Look::Transparent, Shape::Wide, false))["ids"]["objects"], json!([{ "index": 1, "id": "Light" }]));
        assert!(pushed_now(&sub).is_empty(), "the route drew nothing");
    });
}

/// Only an image cached at the view's current hash is pushed: none for a
/// view not drawn, or drawn only at a hash it has no longer.
#[test]
fn a_rename_pushes_nothing_for_an_image_not_cached_at_the_current_hash() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("renamed-uncached.blend");
        let a = snap(&blobs, &key, 1, PLACE);
        let sub = host_events::subscribe(&key);
        draw_now(&blobs, &a, &sub, "@scene:main", Width::Thumb, Look::Transparent);
        // The scene moved on (its hash is another now; the camera was never drawn), then an object renamed.
        let b = snap(&blobs, &key, 2, &PLACE.replace("@scene:main 9", "@scene:main 8"));
        let c = snap(&blobs, &key, 3, &PLACE.replace("@scene:main 9", "@scene:main 8").replace("@obj:Chair", "@obj:Seat"));
        assert_ne!(b.container("@scene:main").expect("scene").hash, a.container("@scene:main").expect("scene").hash);
        service::head_moved(&blobs, &b, &c);
        assert!(pushed_now(&sub).is_empty(), "nothing cached at the current hash, nothing drawn");
    });
}

/// A draw that started before an object was renamed in place and lands
/// after the head moved (the view's hash the same): its push names the
/// table as the head lists the objects now. The cache keeps the names it
/// was drawn with, and the route answers the new ones.
#[test]
fn a_draw_landing_after_a_rename_pushes_its_table_named_as_now() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("renamed-mid-draw.blend");
        let _pin = service::pin_head(&key);
        let listing = "@scene:main 9 scene\n@cam:Main 4 c0\n@obj:Desk 5 o0\n@obj:Stool 6 o1";
        let a = snap(&blobs, &key, 1, listing);
        let sub = host_events::subscribe(&key);
        let land = hold(listing, "@scene:main");
        assert_eq!(service::request(&blobs, &a, "@scene:main", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Rendering);
        assert!(matches!(next(&sub, "rendering"), HostEvent::PreviewRendering { .. }));

        // Stool renamed Bench while the scene is drawn: the same hash, nothing cached yet to push.
        let b = snap(&blobs, &key, 2, &listing.replace("@obj:Stool", "@obj:Bench"));
        let hash = b.container("@scene:main").expect("scene").hash.clone();
        assert_eq!(hash, a.container("@scene:main").expect("scene").hash, "a rename keeps the hash");
        service::head_moved(&blobs, &a, &b);
        assert!(pushed_now(&sub).is_empty(), "nothing cached yet, nothing pushed");

        land.send(()).expect("the draw is held");
        let ev = next(&sub, "ready");
        let f: serde_json::Value = serde_json::from_str(&frame(&ev)).expect("json");
        let renamed = json!([{ "index": 0, "id": "Desk" }, { "index": 1, "id": "Bench" }]);
        assert_eq!(
            (f["container"].clone(), f["render_hash"].clone(), f["state"].clone(), f["rev"].clone()),
            (json!("@scene:main"), json!(hash), json!("ready"), json!(1)),
            "the draw's own push, at its rev: {f}"
        );
        assert_eq!((f["ids"]["objects"].clone(), f["ids"]["count"].clone()), (renamed.clone(), json!(2)), "{f}");

        let Got::Hit(e) = cache::get(&blobs, &key, "@scene:main", &hash, Width::Thumb, "fake-1") else { panic!("cached") };
        assert_eq!(e.meta["ids"]["objects"][1]["id"], json!("Stool"), "the cache keeps the drawn name");
        assert_eq!(body(route::image(&blobs, &b, "@scene:main", None, Width::Thumb, Look::Opaque, Shape::Wide, false))["ids"]["objects"], renamed);
        assert!(pushed_now(&sub).is_empty(), "nothing more");
    });
}

/// A draw that lands after its view moved to another hash (and an object
/// was renamed) is no longer the view's current image: its push keeps the
/// names it was drawn with.
#[test]
fn a_draw_landing_after_its_view_moved_on_pushes_the_names_it_drew_with() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("moved-mid-draw.blend");
        let _pin = service::pin_head(&key);
        let listing = "@scene:main 9 scene\n@cam:Main 4 c0\n@obj:Shelf 5 o0\n@obj:Crate 6 o1";
        let a = snap(&blobs, &key, 1, listing);
        let sub = host_events::subscribe(&key);
        let land = hold(listing, "@cam:Main");
        assert_eq!(service::request(&blobs, &a, "@cam:Main", Width::Thumb, Look::Transparent, Shape::Wide), Requested::Rendering);
        assert!(matches!(next(&sub, "rendering"), HostEvent::PreviewRendering { .. }));

        // The camera moved and Crate renamed Box while the old view is drawn.
        let b = snap(&blobs, &key, 2, &listing.replace("@cam:Main 4", "@cam:Main 3").replace("@obj:Crate", "@obj:Box"));
        let drawn_at = a.container("@cam:Main").expect("camera").hash.clone();
        assert_ne!(b.container("@cam:Main").expect("camera").hash, drawn_at);
        service::head_moved(&blobs, &a, &b);
        assert!(pushed_now(&sub).is_empty(), "nothing cached yet, nothing pushed");

        land.send(()).expect("the draw is held");
        let f: serde_json::Value = serde_json::from_str(&frame(&next(&sub, "ready"))).expect("json");
        let as_drawn = json!([{ "index": 0, "id": "Shelf" }, { "index": 1, "id": "Crate" }]);
        assert_eq!((f["container"].clone(), f["render_hash"].clone(), f["rev"].clone()), (json!("@cam:Main"), json!(drawn_at), json!(1)), "{f}");
        assert_eq!(f["ids"]["objects"], as_drawn, "{f}");
        let build = Look::Transparent.cache_build("fake-1");
        let Got::Hit(e) = cache::get(&blobs, &key, "@cam:Main", &drawn_at, Width::Thumb, &build) else { panic!("cached") };
        assert_eq!(e.meta["ids"]["objects"], as_drawn);
        assert!(pushed_now(&sub).is_empty(), "nothing more");
    });
}

/// A draw that lands when the doc has no head snapshot (here purged after
/// an object was renamed) has no listing to name its table from: its push
/// keeps the names it was drawn with.
#[test]
fn a_draw_landing_without_a_head_snapshot_pushes_the_names_it_drew_with() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("headless-mid-draw.blend");
        let listing = "@scene:main 9 scene\n@obj:Bench 5 o0\n@obj:Easel 6 o1";
        let a = snap(&blobs, &key, 1, listing);
        let sub = host_events::subscribe(&key);
        let land = hold(listing, "@scene:main");
        assert_eq!(service::request(&blobs, &a, "@scene:main", Width::Thumb, Look::Opaque, Shape::Wide), Requested::Rendering);
        assert!(matches!(next(&sub, "rendering"), HostEvent::PreviewRendering { .. }));

        let b = snap(&blobs, &key, 2, &listing.replace("@obj:Easel", "@obj:Stand"));
        service::head_moved(&blobs, &a, &b);
        service::doc_purged(&key, "doc_deleted");
        assert!(pushed_now(&sub).is_empty(), "nothing cached yet, nothing pushed");

        land.send(()).expect("the draw is held");
        let f: serde_json::Value = serde_json::from_str(&frame(&next(&sub, "ready"))).expect("json");
        assert_eq!((f["container"].clone(), f["state"].clone(), f["rev"].clone()), (json!("@scene:main"), json!("ready"), json!(1)), "{f}");
        assert_eq!(f["ids"]["objects"], json!([{ "index": 0, "id": "Bench" }, { "index": 1, "id": "Easel" }]), "{f}");
        assert!(pushed_now(&sub).is_empty(), "nothing more");
    });
}

/// A deck's slides have no id tables: a retitled slide pushes nothing.
#[test]
fn a_deck_never_gets_an_id_table_push() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("retitled.pptx");
        let slides = |titles: [&str; 2]| {
            Arc::new(Snapshot {
                key: key.clone(),
                rev: 1,
                version: None,
                renderer: &deck::DECK,
                build: deck::DECK.build(),
                page: None,
                containers: titles
                    .iter()
                    .enumerate()
                    .map(|(i, t)| Keyed {
                        c: Container { key: format!("@sld:{}", 256 + i), position: i as u32 + 1, title: t.to_string(), hidden: false, digest: [i as u8; 32], locator: (i + 1).to_string(), extras: Default::default() },
                        hash: format!("h{i}"),
                    })
                    .collect(),
                bytes: Arc::new(Vec::new()),
            })
        };
        let (a, b) = (slides(["Intro", "Plan"]), slides(["Overview", "Plan"]));
        let meta = json!({ "container": "@sld:256", "render_hash": "h0", "rendered_at_ms": 1, "ids": { "objects": [{ "index": 0, "id": "Intro" }] } });
        cache::put(&blobs, &key, "@sld:256", "h0", Width::Thumb, &deck::DECK.build(), &meta, b"PIXELS").expect("cached");
        let sub = host_events::subscribe(&key);
        service::head_moved(&blobs, &a, &b);
        assert!(pushed_now(&sub).is_empty(), "a deck is untouched");
    });
}

/// The real worker when it is built (cargo build -p k2-scenerender --bins).
fn built_worker() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|d| d.parent()).map(|d| d.join(BINARY_NAME)))
        .filter(|p| p.is_file())
        .or_else(|| std::env::var_os(scene::ENV_BIN).map(std::path::PathBuf::from))
}

#[test]
fn the_workers_id_image_round_trips_through_the_cache_when_it_is_built() {
    let Some(bin) = built_worker() else {
        eprintln!("[docs/preview] k2-scenerender not built; skipping the worker's id image");
        return;
    };
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("worker-ids.blend");
        let bytes = encode(&scene_with_display_fields());
        let l = scene::list_k2scene(&bytes).expect("a listing");
        let secret = *blobs.host_cache_key(&key.space, service::HASH_LABEL).expect("the cache key").secret_bytes();
        let snap = Arc::new(Snapshot {
            key: key.clone(),
            rev: 1,
            version: None,
            renderer: &scene::SCENE,
            build: scene::SCENE.build(),
            page: l.page,
            containers: l.containers.iter().cloned().map(|c| Keyed { hash: service::keyed_hash(&secret, &c.digest), c }).collect(),
            bytes: Arc::new(bytes.clone()),
        });
        let titles: Vec<&str> = l.containers.iter().filter(|c| c.key.starts_with("@obj:")).map(|c| c.title.as_str()).collect();
        for (c, w) in [("@scene:main", Width::Thumb), ("@obj:Item.002", Width::Thumb), ("@cam:Camera", Width::Thumb2x)] {
            let k = snap.container(c).expect("listed");
            let img = scene::render_k2scene_with(&bin, &bytes, &k.c, w, Look::Transparent, Shape::Wide, Duration::from_secs(60)).unwrap_or_else(|f| panic!("{c}: {f:?}"));
            let png = img.ids.clone().expect("an id image");
            service::store(&blobs, &snap, c, &k.hash, w, Look::Transparent, Shape::Wide, &img, 1).expect("cached");
            let a = body(route::image(&blobs, &snap, c, None, w, Look::Transparent, Shape::Wide, false));
            assert_eq!(a["ids"], img.extras["ids"], "{c}: the table as drawn (nothing renamed)");
            let objects = a["ids"]["objects"].as_array().expect("objects");
            assert!(!objects.is_empty() && objects.iter().all(|e| titles.contains(&e["id"].as_str().expect("an id"))), "{c}: {objects:?}");
            let i = body(route::image(&blobs, &snap, c, None, w, Look::Transparent, Shape::Wide, true));
            assert_eq!(b64(&i), png, "{c}: the id image back as drawn");
            assert_eq!(i["size"], a["size"], "{c}");
        }
        let t = snap.container("@turntable:3").expect("a frame");
        let img = scene::render_k2scene_with(&bin, &bytes, &t.c, Width::Thumb, Look::Transparent, Shape::Wide, Duration::from_secs(60)).expect("a frame");
        assert!(img.ids.is_none() && !img.extras.contains_key("ids"), "a turntable frame is drawn without");
    });
}

// ── 8. Shapes ────────────────────────────────────────────────────────

#[test]
fn the_route_takes_wide_or_tall_and_wide_by_default() {
    assert_eq!(route::shape_param(None), Ok(Shape::Wide));
    assert_eq!(route::shape_param(Some("wide")), Ok(Shape::Wide));
    assert_eq!(route::shape_param(Some("tall")), Ok(Shape::Tall));
    assert_eq!(route::shape_param(Some(" tall ")), Ok(Shape::Tall));
    for junk in ["", "square", "Tall", "4:3", "wide,tall"] {
        let e = route::shape_param(Some(junk)).expect_err(junk);
        assert_eq!(e, format!("shape must be wide or tall (got {junk})"));
    }
}

const SHAPED: &str = "@scene:main 9 scene\n@cam:Shaped 4 c0\n@obj:Plinth 5 o0\n@obj:Wobbly:fail 3 o1\n@turntable:0 7 t0";

/// The scene and object views are keyed by shape (tall under its own name,
/// wide under the name every image had); a camera's view and a turntable
/// frame are keyed once, whatever shape is asked. The build tag says the
/// default changed.
#[test]
fn only_the_scene_and_object_views_are_keyed_by_shape() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let s = snap(&blobs, &doc("shape-keys.blend"), 1, SHAPED);
        for look in Look::ALL {
            for c in ["@scene:main", "@obj:Plinth"] {
                assert_eq!(s.drawn_shape(c, Shape::Tall), Shape::Tall, "{c}");
                assert_ne!(s.cache_build(c, look, Shape::Tall), s.cache_build(c, look, Shape::Wide), "{c} {look:?}");
                assert_eq!(s.cache_build(c, look, Shape::Wide), look.cache_build("fake-1"), "{c} {look:?}: wide keeps the look's name");
                assert_eq!(s.shape_field(c, Shape::Wide), Some("wide"), "{c}: named, wide included");
            }
            for c in ["@cam:Shaped", "@turntable:0"] {
                assert_eq!(s.drawn_shape(c, Shape::Tall), Shape::Wide, "{c}");
                assert_eq!(s.cache_build(c, look, Shape::Tall), s.cache_build(c, look, Shape::Wide), "{c} {look:?}: one image whatever the shape");
                assert_eq!(s.shape_field(c, Shape::Tall), None, "{c}: never named");
            }
        }
        let build = scene::SCENE.build();
        assert_eq!(build, format!("scenerender-p{PROTO}-{VERSION}.clay.ao2.ids.shape"), "no 4:3 image from before is served as wide");
        assert_eq!(cache_build(&build, Look::Transparent, Shape::Tall), format!("{build}.rgba.tall"));
        assert!(s.containers.iter().all(|k| !deck::DECK.shapes(&k.c)), "a deck shapes nothing");
    });
}

#[test]
fn a_tall_still_is_drawn_and_cached_apart_and_a_camera_or_frame_ignores_the_shape() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = doc("shapes.blend");
        let s = snap(&blobs, &key, 1, SHAPED);
        let sub = host_events::subscribe(&key);

        // An object: one render per shape, each named in its pushes and metadata.
        for shape in Shape::ALL {
            assert_eq!(service::request(&blobs, &s, "@obj:Plinth", Width::Thumb, Look::Transparent, shape), Requested::Rendering);
            let r = frame(&next(&sub, "rendering"));
            assert!(r.contains(&format!(r#""width":"thumb","look":"transparent","shape":"{}","rev":1"#, shape.as_str())), "{r}");
            let ready: serde_json::Value = serde_json::from_str(&frame(&next(&sub, "ready"))).expect("json");
            assert_eq!((ready["shape"].clone(), ready["look"].clone()), (json!(shape.as_str()), json!("transparent")), "{ready}");
        }
        assert_eq!(shapes_drawn("@obj:Plinth"), Shape::ALL);
        let hash = s.container("@obj:Plinth").expect("listed").hash.clone();
        for shape in Shape::ALL {
            assert!(cache::entry_exists(&key, "@obj:Plinth", &hash, Width::Thumb, &cache_build("fake-1", Look::Transparent, shape)), "{shape:?}");
            let a = body(route::image(&blobs, &s, "@obj:Plinth", None, Width::Thumb, Look::Transparent, shape, false));
            assert_eq!((a["state"].clone(), a["shape"].clone()), (json!("ready"), json!(shape.as_str())), "{shape:?}");
            let i = body(route::image(&blobs, &s, "@obj:Plinth", None, Width::Thumb, Look::Transparent, shape, true));
            assert_eq!((i["layer"].clone(), i["shape"].clone()), (json!("ids"), json!(shape.as_str())), "{shape:?}: the id image names it too");
        }

        // A camera asked tall is drawn once in its own aspect, keyed as it always was, and never names a shape.
        assert_eq!(service::request(&blobs, &s, "@cam:Shaped", Width::Thumb, Look::Transparent, Shape::Tall), Requested::Rendering);
        let r = frame(&next(&sub, "rendering"));
        assert!(!r.contains("shape"), "{r}");
        assert!(!frame(&next(&sub, "ready")).contains("shape"));
        assert_eq!(shapes_drawn("@cam:Shaped"), [Shape::Wide], "drawn in its own aspect");
        let cam = s.container("@cam:Shaped").expect("listed").hash.clone();
        assert!(cache::entry_exists(&key, "@cam:Shaped", &cam, Width::Thumb, &Look::Transparent.cache_build("fake-1")));
        for shape in Shape::ALL {
            let a = body(route::image(&blobs, &s, "@cam:Shaped", None, Width::Thumb, Look::Transparent, shape, false));
            assert_eq!(a["state"], json!("ready"), "{shape:?}: the image it has");
            assert!(a.get("shape").is_none(), "{a}");
        }
        assert!(pushed_now(&sub).is_empty(), "nothing drawn again");
        assert_eq!(shapes_drawn("@cam:Shaped").len(), 1);

        // A turntable frame asked tall: its answer while drawing names no shape, and the wide ask finds it.
        let r = route::image(&blobs, &s, "@turntable:0", None, Width::Full, Look::Transparent, Shape::Tall, false);
        assert_eq!(r.status, "202 Accepted");
        assert!(body(r).get("shape").is_none());
        assert!(!frame(&next(&sub, "rendering")).contains("shape"));
        next(&sub, "ready");
        assert_eq!(body(route::image(&blobs, &s, "@turntable:0", None, Width::Full, Look::Transparent, Shape::Wide, false))["state"], json!("ready"));

        // A failure is per shape and names it.
        assert_eq!(service::request(&blobs, &s, "@obj:Wobbly:fail", Width::Full, Look::Transparent, Shape::Tall), Requested::Rendering);
        next(&sub, "rendering");
        let f = frame(&next(&sub, "failed"));
        assert!(f.contains(r#""state":"failed""#) && f.ends_with(r#""look":"transparent","shape":"tall"}"#), "{f}");
        assert_eq!(service::request(&blobs, &s, "@obj:Wobbly:fail", Width::Full, Look::Transparent, Shape::Tall), Requested::Failed(FailCode::Timeout));
        let failed = body(route::image(&blobs, &s, "@obj:Wobbly:fail", None, Width::Full, Look::Transparent, Shape::Tall, false));
        assert_eq!((failed["state"].clone(), failed["shape"].clone()), (json!("failed"), json!("tall")));

        // The listing: each container's states in the shape asked (one image for the camera), the shape named.
        let listed = |shape: Shape| body(route::listing(&blobs, &s, None, Look::Transparent, shape));
        let state = |v: &serde_json::Value, c: &str| v["containers"].as_array().expect("containers").iter().find(|x| x["key"] == c).expect("listed")["previews"]["thumb"].clone();
        for shape in Shape::ALL {
            let v = listed(shape);
            assert_eq!(v["shape"], json!(shape.as_str()));
            assert_eq!((state(&v, "@obj:Plinth"), state(&v, "@cam:Shaped"), state(&v, "@scene:main")), (json!("ready"), json!("ready"), json!("none")), "{shape:?}");
        }
    });
}

#[test]
fn a_deck_listing_names_no_shape() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let s = Arc::new(Snapshot {
            key: doc("shapeless.pptx"),
            rev: 1,
            version: None,
            renderer: &deck::DECK,
            build: deck::DECK.build(),
            page: None,
            containers: vec![Keyed {
                c: Container { key: "@sld:256".into(), position: 1, title: "Intro".into(), hidden: false, digest: [1; 32], locator: "1".into(), extras: Default::default() },
                hash: "h0".into(),
            }],
            bytes: Arc::new(Vec::new()),
        });
        let v = body(route::listing(&blobs, &s, None, Look::Opaque, Shape::Tall));
        assert!(v.get("shape").is_none(), "{v}");
        assert_eq!(s.cache_build("@sld:256", Look::Opaque, Shape::Tall), deck::DECK.build(), "a slide is keyed as it always was");
    });
}
