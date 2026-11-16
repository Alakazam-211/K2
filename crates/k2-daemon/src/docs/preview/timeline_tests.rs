//! The timeline slot: tiles keyed by (media hash, source time), and a host
//! with no video decoder answering `not_on_this_host` at once, on the route
//! (an image and a listing) and in the service, with nothing queued, pushed
//! or kept.

use std::sync::Arc;
use std::time::Duration;

use k2_mpc_base::blob::BlobStore;
use k2_mpc_base::store::DocKey;
use serde_json::json;

use super::*;
use crate::docs::host_events;
use crate::docs::preview::service::{self, Keyed, Requested, Snapshot};
use crate::docs::preview::{renderers, route, FailCode, Look, PreviewRenderer, Shape, Width};
use crate::test_support::{with_temp_home, Scratch};

fn store() -> (Scratch, Arc<BlobStore>) {
    let s = Scratch::new();
    let docs = Arc::new(k2_mpc_base::store::DocStore::open(&s.path().join("docs.redb")).expect("store"));
    let blobs = BlobStore::open(docs, &s.path().join("blobs"), k2_mpc_base::blob::BlobConfig::default(), Arc::new(|| 1_000)).expect("blobs");
    (s, Arc::new(blobs))
}

fn media(b: u8) -> [u8; 32] {
    [b; 32]
}

fn clip(n: u32, name: &str, m: u8, source_in: i64, source_out: i64) -> Tile {
    Tile {
        clip: format!("@trk:1/clip:{n}"),
        name: name.into(),
        media_hash: media(m),
        media_key: format!("m{m}"),
        source_in,
        source_out,
    }
}

fn snapshot(blobs: &BlobStore, key: &DocKey, clips: &[Tile]) -> Arc<Snapshot> {
    let l = listing(clips);
    let secret = *blobs.host_cache_key(&key.space, service::HASH_LABEL).expect("the cache key").secret_bytes();
    Arc::new(Snapshot {
        key: key.clone(),
        rev: 4,
        version: None,
        renderer: &TIMELINE,
        build: TIMELINE.build(),
        page: l.page,
        containers: l.containers.into_iter().map(|c| Keyed { hash: service::keyed_hash(&secret, &c.digest), c }).collect(),
        bytes: Arc::new(Vec::new()),
    })
}

/// The preview events on the doc's sockets within 300 ms (a turn end, which
/// goes to every socket, isn't one).
fn pushed(sub: &host_events::Subscription) -> usize {
    let mut n = 0;
    while let Ok(ev) = sub.rx.recv_timeout(Duration::from_millis(300)) {
        if matches!(ev, host_events::HostEvent::PreviewRendering { .. } | host_events::HostEvent::PreviewReady { .. }) {
            n += 1;
        }
    }
    n
}

#[test]
fn tiles_are_keyed_by_media_and_source_time_only() {
    let l = listing(&[clip(7, "Interview A", 1, 48_000, 96_000), clip(8, "B-roll", 2, 0, 24_000)]);
    assert!(l.page.is_none(), "a timeline has no page");
    let keys: Vec<(&str, u32, &str)> = l.containers.iter().map(|c| (c.key.as_str(), c.position, c.title.as_str())).collect();
    assert_eq!(
        keys,
        [
            ("@trk:1/clip:7/tile:head", 1, "Interview A"),
            ("@trk:1/clip:7/tile:tail", 2, "Interview A"),
            ("@trk:1/clip:8/tile:head", 3, "B-roll"),
            ("@trk:1/clip:8/tile:tail", 4, "B-roll"),
        ]
    );
    for c in &l.containers {
        k2_mpc_base::addr::parse(&c.key, k2_mpc_base::addr::Forms::none().native()).unwrap_or_else(|e| panic!("{}: {e:?}", c.key));
        assert!(!c.locator.is_empty() && c.extras.is_empty(), "{}", c.key);
    }
    let head = l.containers[0].digest;
    // A move (same source in-point, another clip and place): the same picture.
    let moved = listing(&[clip(9, "Interview A, moved", 1, 48_000, 96_000)]);
    assert_eq!(moved.containers[0].digest, head, "a move needs no new picture");
    // A trim of the start: a new head tile, the tail unchanged.
    let trimmed = listing(&[clip(7, "Interview A", 1, 50_000, 96_000)]);
    assert_ne!(trimmed.containers[0].digest, head, "a trim of the start needs one new head tile");
    assert_eq!(trimmed.containers[1].digest, l.containers[1].digest, "and keeps the tail");
    // Another source at the same time is another picture.
    assert_ne!(listing(&[clip(7, "Interview A", 3, 48_000, 96_000)]).containers[0].digest, head);
    assert_eq!(tile_digest(&media(1), 48_000), head);
}

#[test]
fn the_timeline_renderer_is_registered_and_serves_no_doc_yet() {
    assert!(renderers().iter().any(|r| r.kind() == "timeline"), "registered");
    assert!(!decoder_here(), "no host has K2 Render's tile decoder yet");
    let c = &listing(&[clip(7, "A", 1, 0, 10)]).containers[0];
    assert!(!TIMELINE.decodes_here(c));
    assert_eq!(TIMELINE.list(b"{}").expect_err("no pack").code, FailCode::UnsupportedPart);
    assert_eq!(TIMELINE.default_look(), Look::Opaque);
    assert!(!TIMELINE.draws_ids(c));
    assert!(!TIMELINE.shapes(c), "a tile keeps its own aspect: the shape asked is ignored");
}

/// A host with no video decoder: the image answers `failed` with
/// `not_on_this_host` at once (200, as every failed image), a listing says
/// so per tile, and the service queues nothing, pushes nothing and keeps no
/// failure.
#[test]
fn a_host_without_a_decoder_answers_not_on_this_host_at_once() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = DocKey { space: "timeline-ws".into(), path: "edits/rough-cut.otio".into() };
        let snap = snapshot(&blobs, &key, &[clip(7, "Interview A", 1, 48_000, 96_000)]);
        let sub = host_events::subscribe(&key);

        let r = route::image(&blobs, &snap, "@trk:1/clip:7/tile:head", None, Width::Thumb, Look::Opaque, Shape::Wide, false);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["state"], "failed", "{v}");
        assert_eq!(v["failed"], json!({ "code": "not_on_this_host" }), "{v}");
        assert_eq!(v["kind"], "timeline", "{v}");
        assert_eq!(v["container"], "@trk:1/clip:7/tile:head", "{v}");
        assert!(v.get("base64").is_none() && v.get("stale").is_none(), "no picture: {v}");
        assert!(v.get("shape").is_none(), "a tile isn't shaped: {v}");

        for w in Width::ALL {
            assert_eq!(service::request(&blobs, &snap, "@trk:1/clip:7/tile:tail", w, Look::Opaque, Shape::Wide), Requested::Failed(FailCode::NotOnThisHost));
            let k = snap.container("@trk:1/clip:7/tile:tail").expect("tail");
            assert!(!service::rendering(&key, &k.c.key, &k.hash, w, Look::Opaque, Shape::Wide), "nothing queued");
        }
        // Asked again (tall, which a tile ignores): the same answer, decided again (nothing kept to clear).
        assert_eq!(service::request(&blobs, &snap, "@trk:1/clip:7/tile:head", Width::Full, Look::Opaque, Shape::Tall), Requested::Failed(FailCode::NotOnThisHost));
        assert_eq!(pushed(&sub), 0, "no preview_rendering or preview_ready is pushed");

        // The listing (with want=thumb too): every tile says why, and nothing is queued.
        let r = route::listing(&blobs, &snap, Some(Width::Thumb), Look::Opaque, Shape::Tall);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["kind"], "timeline");
        assert_eq!(v["page"], serde_json::Value::Null);
        assert!(v.get("shape").is_none(), "no tile is shaped, so the listing names no shape: {v}");
        let tiles = v["containers"].as_array().expect("containers");
        assert_eq!(tiles.len(), 2);
        for t in tiles {
            assert_eq!(t["previews"], json!({ "thumb": "failed", "full": "failed" }), "{t}");
            assert_eq!(t["failed"], json!({ "code": "not_on_this_host" }), "{t}");
        }
        assert_eq!(pushed(&sub), 0, "the listing pushed nothing either");
    });
}

/// A renderer that decodes keeps its listing as it was: no `failed` field,
/// and `none` until asked (the deck's and the scene's bytes are unchanged).
#[test]
fn a_renderer_that_decodes_lists_as_before() {
    with_temp_home(|| {
        let (_s, blobs) = store();
        let key = DocKey { space: "timeline-ws".into(), path: "talk.pptx".into() };
        let base = snapshot(&blobs, &key, &[clip(1, "A", 1, 0, 10)]);
        let deck = Snapshot {
            key: key.clone(),
            rev: base.rev,
            version: None,
            renderer: &super::super::deck::DECK,
            build: "deck-test".into(),
            page: None,
            containers: base.containers.clone(),
            bytes: Arc::new(Vec::new()),
        };
        let v = route::container_json(&deck, &deck.containers[0], Look::Opaque, Shape::Wide);
        assert_eq!(v["previews"], json!({ "thumb": "none", "full": "none" }), "{v}");
        assert!(v.get("failed").is_none(), "{v}");
    });
}
