//! The 3D tab's host side: what a scene's listing and images carry to the
//! client, transparent stills, the turntable's titles, the preview events
//! on the docs socket, and a rename answered from the cache.
//!
//! The service tests draw with [`Fake`], a renderer that runs no process and
//! logs every render, so "no worker run" is a count.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use k2_mpc_base::blob::BlobStore;
use k2_mpc_base::errors::Value;
use k2_mpc_base::store::DocKey;
use k2_scenerender::camera::{Framing, ViewSpec};
use k2_scenerender::digest::Target as SceneTarget;
use k2_scenerender::host::SceneRequest;
use k2_scenerender::proto::{Ao, CameraMeta, Clay, ImageFormat, Request, ViewMeta, AO_RADIUS, AO_STRENGTH, CLAY_CONTENTS, CLAY_SHELL, PROTO, VERSION};
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

/// Lists one container per line, `<address> <digest byte>`; draws every
/// address ending `:fail` as a timeout, the rest as the same small image.
struct Fake;

static FAKE: Fake = Fake;

fn drawn() -> &'static Mutex<Vec<(String, Look)>> {
    static D: Mutex<Vec<(String, Look)>> = Mutex::new(Vec::new());
    &D
}

fn renders_of(container: &str) -> Vec<Look> {
    drawn().lock().unwrap_or_else(|p| p.into_inner()).iter().filter(|(c, _)| c == container).map(|(_, l)| *l).collect()
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
    fn list(&self, bytes: &[u8]) -> Result<Listing, Failed> {
        let text = std::str::from_utf8(bytes).expect("a fake listing is text");
        let containers = text
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let (key, d) = line.split_once(' ').expect("<address> <digest byte>");
                Container {
                    key: key.into(),
                    position: i as u32 + 1,
                    title: String::new(),
                    hidden: false,
                    digest: [d.parse::<u8>().expect("a digest byte"); 32],
                    locator: key.into(),
                    extras: Default::default(),
                }
            })
            .collect();
        Ok(Listing { page: None, containers })
    }
    fn render(&self, _src: &Source<'_>, c: &Container, width: Width, look: Look, _wall: Duration) -> Result<Image, Failed> {
        drawn().lock().unwrap_or_else(|p| p.into_inner()).push((c.key.clone(), look));
        if c.key.ends_with(":fail") {
            return Err(Failed::new(FailCode::Timeout, "the fake's timeout"));
        }
        let mut extras = serde_json::Map::new();
        extras.insert("camera".into(), json!({ "fx": 1.5 }));
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
    assert!(by("@scene").extras.is_empty(), "the auto view has no extras");
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
        let listed: Vec<serde_json::Value> = snap.containers.iter().map(|k| route::container_json(&snap, k, Look::Transparent)).collect();
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
    let t = scene::request_for(&bytes, SceneTarget::Scene, Width::Full, Look::Transparent);
    assert!(t.transparent, "transparent asked");
    assert_eq!(t.background, None, "never with a background (the worker refuses both)");
    assert_eq!((t.format, t.views.len(), t.views[0].width), (ImageFormat::Webp, 1, 1920));
    let o = scene::request_for(&bytes, SceneTarget::Camera(0), Width::Thumb, Look::Opaque);
    assert!(!o.transparent);
    assert_eq!(o.background, None, "opaque: the renderer's own neutral, as before");
    // An opaque entry keeps the name it always had; a transparent one is
    // keyed by the look alone, no colour.
    assert_eq!(Look::Opaque.cache_build("b1"), "b1");
    assert_eq!(cache::entry_name("@scene", "ab12", Width::Thumb, &Look::Opaque.cache_build("b1")), cache::entry_name("@scene", "ab12", Width::Thumb, "b1"));
    assert_eq!(
        cache::entry_name("@scene", "ab12", Width::Thumb, &Look::Transparent.cache_build("b1")),
        format!("{}-ab12-thumb-b1.rgba.k2p", cache::container_id("@scene"))
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
        assert_eq!(service::request(&blobs, &s, "@obj:Tx", Width::Thumb, Look::Transparent), Requested::Rendering);
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
    String::from_utf8(line.line()).expect("utf-8")
}

/// Ambient occlusion on the renderer's default settings, as the line carries it.
const AO_ON_THE_WIRE: &str = r#""ao":{"strength":1.0,"radius":0.1}"#;

#[test]
fn the_scene_view_asks_for_contents_framing_and_clay_and_occlusion_on_the_defaults() {
    let bytes = encode(&scene_with_display_fields());
    for look in Look::ALL {
        let r = scene::request_for(&bytes, SceneTarget::Scene, Width::Full, look);
        assert_eq!(r.views.len(), 1);
        assert_eq!((r.views[0].view, r.views[0].framing, r.views[0].objects.clone()), (ViewSpec::Auto, Framing::Contents, None), "{look:?}");
        assert_eq!(r.clay, Some(Clay { shell: CLAY_SHELL, contents: CLAY_CONTENTS }), "{look:?}: the renderer's own tones");
        assert_eq!(r.ao, Some(Ao { strength: AO_STRENGTH, radius: AO_RADIUS }), "{look:?}: occlusion on the renderer's own settings");
        assert!(wire(&r).contains(AO_ON_THE_WIRE), "{look:?}: {}", wire(&r));
        assert_eq!(r.transparent, look == Look::Transparent);
        assert_eq!(r.background, None);
    }
    // On the wire: contents framing, clay and occlusion, nothing else changed.
    let r = scene::request_for(&bytes, SceneTarget::Scene, Width::Thumb, Look::Transparent);
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
            let r = scene::request_for(&bytes, t, Width::Thumb, look);
            assert_eq!(r.views, [t.view(Width::Thumb.px())], "{t:?}: the renderer's own view, bounds framed");
            assert_eq!(r.views[0].framing, Framing::Bounds, "{t:?}: never contents");
            assert_eq!(r.clay, Some(Clay::default()), "{t:?} {look:?}");
            assert_eq!(r.ao, Some(Ao::default()), "{t:?} {look:?}: occlusion on the default settings");
            let text = wire(&r);
            assert!(text.contains(AO_ON_THE_WIRE), "{t:?} {look:?}: {text}");
            assert_eq!(Request::parse(text.trim_end().as_bytes()).expect("parse").ao, Some(Ao::default()), "{t:?} {look:?}");
        }
        assert_eq!(scene::framing_for(t), Framing::Bounds);
    }
    let o = scene::request_for(&bytes, SceneTarget::Object(3), Width::Thumb, Look::Opaque);
    assert_eq!((o.views[0].view, o.views[0].objects.clone()), (ViewSpec::Auto, Some(vec![3])), "an object framed on itself alone");
    assert_eq!(scene::framing_for(SceneTarget::Scene), Framing::Contents);
}

/// Clay, occlusion and the framing change the pixels but are not in the
/// renderer's digest, so the host keys them: clay and occlusion in the build
/// (every view), the framing in the scene view's digest.
#[test]
fn clay_occlusion_and_the_scene_views_framing_are_in_the_cache_key() {
    let build = scene::SCENE.build();
    assert_eq!(build, format!("scenerender-p{PROTO}-{VERSION}.clay.ao"));
    let plain = format!("scenerender-p{PROTO}-{VERSION}");
    let clay_only = format!("{plain}.clay");
    for look in Look::ALL {
        let name = |b: &str| cache::entry_name("@scene", "ab12", Width::Thumb, &look.cache_build(b));
        assert_ne!(name(&build), name(&plain), "{look:?}: never an image drawn without clay");
        assert_ne!(name(&build), name(&clay_only), "{look:?}: never an image drawn in clay without occlusion");
    }
    assert_eq!(Look::Opaque.cache_build(&build), format!("{plain}.clay.ao"));
    assert_eq!(Look::Transparent.cache_build(&build), format!("{plain}.clay.ao.rgba"));
    assert_ne!(Look::Transparent.cache_build(&build), Look::Transparent.cache_build(&clay_only));

    // A scene of one object: the renderer's digest of `@scene` and of that
    // object's view is one, but they are framed differently.
    let bytes = encode(&testgen::cube_scene());
    let raw = k2_scenerender::digest::list(&bytes, &k2_scenerender::scene::InputLimits::default()).expect("raw");
    let (rs, ro) = (raw.iter().find(|i| i.key == "@scene").expect("scene"), raw.iter().find(|i| i.key.starts_with("@obj:")).expect("object"));
    assert_eq!(rs.digest, ro.digest, "the renderer's digest leaves the framing out");
    let l = scene::list_k2scene(&bytes).expect("a listing");
    let (s, o) = (l.containers.iter().find(|c| c.key == "@scene").expect("scene"), l.containers.iter().find(|c| c.key.starts_with("@obj:")).expect("object"));
    assert_ne!(s.digest, o.digest, "the scene view's framing is keyed: no image is shared between them");
    assert_eq!(s.digest, scene::keyed_digest(SceneTarget::Scene, rs.digest));
    assert_eq!(o.digest, ro.digest, "a bounds-framed view keeps the renderer's digest");
    // Every other view keeps the renderer's digest.
    let both = scene::list_k2scene(&encode(&scene_with_display_fields())).expect("a listing");
    let raw_both = k2_scenerender::digest::list(&encode(&scene_with_display_fields()), &k2_scenerender::scene::InputLimits::default()).expect("raw");
    for (c, r) in both.containers.iter().zip(&raw_both) {
        assert_eq!(c.digest == r.digest, c.key != "@scene", "{}", c.key);
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
        assert_eq!(service::request(&blobs, &s, "@obj:Ok", Width::Thumb, Look::Opaque), Requested::Rendering);
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
        assert_eq!(service::request(&blobs, &s, "@obj:Bad:fail", Width::Full, Look::Opaque), Requested::Rendering);
        assert!(frame(&next(&sub, "rendering")).starts_with(r#"{"type":"preview_rendering","container":"@obj:Bad:fail""#));
        let f = frame(&next(&sub, "failed"));
        assert!(f.starts_with(r#"{"type":"preview_ready","container":"@obj:Bad:fail""#), "{f}");
        assert!(f.contains(r#""width":"full","state":"failed","failed":{"code":"timeout"},"rev":2"#), "{f}");
        assert!(!f.contains("render_ms"), "a failure has no image to time: {f}");
        // Asked again: the failure, no new render and no new events.
        assert_eq!(service::request(&blobs, &s, "@obj:Bad:fail", Width::Full, Look::Opaque), Requested::Failed(FailCode::Timeout));
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
        let a = snap(&blobs, &key, 1, "@scene 9\n@obj:Old 5");
        let sub = host_events::subscribe(&key);
        assert_eq!(service::request(&blobs, &a, "@obj:Old", Width::Thumb, Look::Opaque), Requested::Rendering);
        next(&sub, "rendering");
        assert!(frame(&next(&sub, "ready")).contains(r#""state":"ready""#));
        assert_eq!(renders_of("@obj:Old").len(), 1);

        // The object renamed: same digest, new address.
        let b = snap(&blobs, &key, 2, "@scene 9\n@obj:New 5");
        let new = b.container("@obj:New").expect("renamed").clone();
        assert_eq!(new.hash, a.container("@obj:Old").expect("old").hash, "a rename keeps the hash");
        assert_eq!(reuse::widths_to_redraw(&a, &new, "fake-1"), [Width::Thumb], "the head's re-draw asks for the old address's widths");
        assert_eq!(service::request(&blobs, &b, "@obj:New", Width::Thumb, Look::Opaque), Requested::Reused { from: "@obj:Old".into() });
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
        assert_eq!(service::request(&blobs, &b, "@obj:New", Width::Thumb, Look::Transparent), Requested::Rendering);
        next(&sub, "rendering");
        next(&sub, "ready");
        assert_eq!(renders_of("@obj:New"), [Look::Transparent]);
        // A real edit (a new digest) is drawn too.
        let c = snap(&blobs, &key, 3, "@scene 9\n@obj:New 6");
        assert_eq!(service::request(&blobs, &c, "@obj:New", Width::Thumb, Look::Opaque), Requested::Rendering);
        assert!(matches!(next(&sub, "rendering"), HostEvent::PreviewRendering { .. }));
        next(&sub, "ready");
        assert_eq!(renders_of("@obj:New").len(), 2);
    });
}
