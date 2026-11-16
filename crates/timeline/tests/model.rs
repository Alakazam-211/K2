//! The model's own invariants: keys in the kit's grammar and unique, the sequential form both
//! ways, the checks, and what the fingerprints and payload digests cover.

use k2_mpc_base::index::NodeKey;
use k2_mpc_timeline::model::{
    key_token, place_steps, time_token, Document, Finding, Format, Item, ItemKind, Key, KeyBook,
    KeyOrigin, Marker, MarkerKind, Media, MediaKind, Opaque, Step, Timeline, Track, TrackKind,
    TransitionInfo,
};
use k2_mpc_timeline::Time;

fn t(n: i64) -> Time {
    Time::seconds(n)
}

fn k(s: &str) -> Key {
    Key::from_parts(s, KeyOrigin::Derived)
}

#[test]
fn keys_are_in_the_kits_grammar_and_unique() {
    let mut book = KeyBook::default();
    let seq = book.key(None, "seq", "9c19c828-f5f5-4cf2-a82a-ebe25a1a15e2", true);
    assert_eq!(seq.origin(), KeyOrigin::Native);
    assert_eq!(seq.as_str(), "@seq:9c19c828-f5f5-4cf2-a82a-ebe25a1a15e2");
    let trk = book.key(Some(&seq), "d-trk", "V1", false);
    assert_eq!(
        trk.as_str(),
        "@seq:9c19c828-f5f5-4cf2-a82a-ebe25a1a15e2/d-trk:V1"
    );
    assert_eq!(trk.origin(), KeyOrigin::Derived);
    let id = format!(
        "{}@{}",
        key_token("My clip/take 2"),
        time_token(Time::new(1001, 24000).unwrap())
    );
    let clip = book.key(Some(&trk), "d-clip", &id, false);
    assert!(
        clip.as_str()
            .ends_with("/d-clip:My_clip_take_2@1001_24000s"),
        "{clip}"
    );
    // a native ID the grammar can't hold becomes a derived token
    let odd = book.key(None, "media", "has space/and slash", true);
    assert_eq!(
        (odd.as_str(), odd.origin()),
        ("@media:has_space_and_slash", KeyOrigin::Derived)
    );
    // a repeat is suffixed, and marked so the index leaves it to the kit
    let again = book.key(Some(&trk), "d-clip", &id, false);
    assert_eq!(again.origin(), KeyOrigin::Repeat);
    assert_eq!(again.as_str(), format!("{clip}~2"));
    let third = book.key(Some(&trk), "d-clip", &id, false);
    assert_eq!(third.as_str(), format!("{clip}~3"));
    for key in [&seq, &trk, &clip, &odd, &again, &third] {
        assert!(
            NodeKey::parse(key.as_str()).is_some(),
            "{key} is not a kit node key"
        );
    }
    // a key past the kit's limit is kept, marked long
    let mut deep = trk.clone();
    for _ in 0..8 {
        deep = book.key(Some(&deep), "d-nest", &key_token(&"x".repeat(60)), false);
    }
    assert_eq!(deep.origin(), KeyOrigin::Long);
    // a bad kind is taken as x
    assert!(book
        .key(None, "Bad Kind", "1", false)
        .as_str()
        .starts_with("@x:"));
}

#[test]
fn key_tokens_replace_separators_and_cut_at_a_character_boundary() {
    assert_eq!(key_token("a b\tc/d"), "a_b_c_d");
    assert_eq!(key_token(""), "_");
    let long = "é".repeat(40); // 80 bytes
    let tok = key_token(&long);
    assert_eq!(tok.len(), 48);
    assert!(tok.chars().all(|c| c == 'é'));
    assert_eq!(time_token(Time::new(-5, 2).unwrap()), "-5_2s");
    assert_eq!(time_token(t(3)), "3s");
}

fn clip(key: &str, start: i64, dur: i64) -> Item {
    Item::new(k(key), ItemKind::Clip, key, t(start), t(dur))
}

fn transition(key: &str, start: i64, dur: i64, in_offset: Option<i64>) -> Item {
    let mut i = Item::new(k(key), ItemKind::Transition, key, t(start), t(dur));
    i.transition = Some(TransitionInfo::new("dissolve", in_offset.map(t)));
    i
}

fn track(items: Vec<Item>) -> Track {
    let mut tr = Track::new(k("@d-seq:1/d-trk:V1"), "V1", TrackKind::Video);
    tr.items = items;
    tr
}

#[test]
fn the_sequential_form_round_trips() {
    let gap = Item::new(k("g"), ItemKind::Gap, "", t(0), t(10));
    let tr = track(vec![
        gap,
        clip("a", 10, 50),
        transition("x", 50, 20, Some(10)),
        clip("b", 60, 40),
        clip("c", 120, 10),
    ]);
    let steps = tr.to_steps().unwrap();
    assert_eq!(
        steps,
        vec![
            Step::Item(0),
            Step::Item(1),
            Step::Transition {
                index: 2,
                in_offset: t(10)
            },
            Step::Item(3),
            Step::Gap(t(20)),
            Step::Item(4),
        ]
    );
    let placed = place_steps(&steps, &tr.items).unwrap();
    assert_eq!(placed.len(), tr.items.len());
    for (n, start) in placed {
        assert_eq!(start, tr.items[n].start, "item {n}");
    }
    // a transition on the first cut may start before zero, and round-trips
    let first = track(vec![transition("x", -5, 10, Some(5)), clip("a", 0, 30)]);
    let steps = first.to_steps().unwrap();
    assert_eq!(
        steps,
        vec![
            Step::Transition {
                index: 0,
                in_offset: t(5)
            },
            Step::Item(1)
        ]
    );
    let placed = place_steps(&steps, &first.items).unwrap();
    assert_eq!(placed, vec![(0, t(-5)), (1, t(0))]);
}

#[test]
fn the_sequential_form_refuses_overlaps_and_cutless_transitions() {
    let over = track(vec![clip("a", 0, 50), clip("b", 40, 10)]);
    assert_eq!(
        over.to_steps(),
        Err(Finding::Overlap {
            track: "@d-seq:1/d-trk:V1".into(),
            first: "a".into(),
            second: "b".into(),
        })
    );
    let cutless = track(vec![
        clip("a", 0, 50),
        transition("x", 45, 10, None),
        clip("b", 50, 10),
    ]);
    assert_eq!(cutless.to_steps(), Err(Finding::BadTransition("x".into())));
}

fn doc_with(tracks: Vec<Track>) -> Document {
    let mut d = Document::new(Format::Otio, "Timeline.1");
    let mut tl = Timeline::new(k("@d-seq:1"), "cut");
    tl.tracks = tracks;
    d.timelines.push(tl);
    d
}

#[test]
fn check_finds_each_broken_invariant() {
    let good = doc_with(vec![track(vec![clip("a", 0, 10), clip("b", 10, 10)])]);
    assert_eq!(good.check(), vec![]);

    let dup = doc_with(vec![track(vec![clip("a", 0, 10), clip("a", 10, 10)])]);
    assert_eq!(dup.check(), vec![Finding::DuplicateKey("a".into())]);

    let mut dangling = clip("a", 0, 10);
    dangling.media = Some(k("@d-media:nothing"));
    assert_eq!(
        doc_with(vec![track(vec![dangling])]).check(),
        vec![Finding::DanglingMedia {
            item: "a".into(),
            media: "@d-media:nothing".into()
        }]
    );

    let unsorted = doc_with(vec![track(vec![clip("b", 10, 10), clip("a", 0, 5)])]);
    assert!(unsorted
        .check()
        .contains(&Finding::Unsorted("@d-seq:1/d-trk:V1".into())));

    let overlap = doc_with(vec![track(vec![clip("a", 0, 10), clip("b", 5, 10)])]);
    assert_eq!(
        overlap.check(),
        vec![Finding::Overlap {
            track: "@d-seq:1/d-trk:V1".into(),
            first: "a".into(),
            second: "b".into()
        }]
    );

    let negative = doc_with(vec![track(vec![clip("a", 0, -1)])]);
    assert_eq!(negative.check(), vec![Finding::BadTime("a".into())]);

    let wide = doc_with(vec![track(vec![
        clip("a", 0, 10),
        transition("x", 5, 10, Some(11)),
        clip("b", 10, 10),
    ])]);
    assert_eq!(wide.check(), vec![Finding::BadTransition("x".into())]);

    // an overlap across a contained item is still caught: c starts inside a, past b's end
    let contained = doc_with(vec![track(vec![
        clip("a", 0, 100),
        clip("b", 100, 1),
        clip("c", 50, 10),
    ])]);
    assert!(contained
        .check()
        .iter()
        .any(|f| matches!(f, Finding::Overlap { .. })));

    let mut marked = doc_with(vec![]);
    let mut m = Marker::new(k("m"), MarkerKind::Marker, "x", t(1));
    m.duration = t(-1);
    marked.timelines[0].markers.push(m);
    assert_eq!(marked.check(), vec![Finding::BadTime("m".into())]);
}

#[test]
fn fingerprints_cover_facets_and_payloads_cover_carried_pieces() {
    let a = clip("a", 0, 10);
    let mut renamed = a.clone();
    renamed.name = "other".into();
    assert_ne!(a.fingerprint(), renamed.fingerprint());
    let mut moved = a.clone();
    moved.start = t(5);
    assert_ne!(a.fingerprint(), moved.fingerprint());
    let mut rekeyed = a.clone();
    rekeyed.key = k("z");
    assert_eq!(
        a.fingerprint(),
        rekeyed.fingerprint(),
        "the key is not a facet"
    );
    let mut carried = a.clone();
    carried.opaque.push(Opaque::of("otio:metadata", b"{}"));
    assert_eq!(
        a.fingerprint(),
        carried.fingerprint(),
        "carried pieces are not facets"
    );
    assert_eq!(a.payload(), None);
    assert!(carried.payload().is_some());
    let mut changed = carried.clone();
    changed.opaque[0] = Opaque::of("otio:metadata", b"{\"a\":1}");
    assert_ne!(carried.payload(), changed.payload());
    // a field boundary can't be moved: name "ab" + text None differs from name "a" + text "b"
    let mut x = clip("a", 0, 10);
    x.name = "ab".into();
    let mut y = clip("a", 0, 10);
    y.name = "a".into();
    y.text = Some("b".into());
    assert_ne!(x.fingerprint(), y.fingerprint());
    let m1 = Marker::new(k("m"), MarkerKind::Marker, "x", t(1));
    let mut m2 = m1.clone();
    m2.kind = MarkerKind::Chapter;
    assert_ne!(m1.fingerprint(), m2.fingerprint());
    let tr = Track::new(k("t"), "V1", TrackKind::Video);
    let mut muted = tr.clone();
    muted.muted = true;
    assert_ne!(tr.fingerprint(), muted.fingerprint());
}

#[test]
fn media_show_only_their_file_name() {
    let mut m = Media::new(k("@d-media:a"), MediaKind::File, "shot");
    assert_eq!(m.file_name(), "shot");
    m.path = Some("C:\\media\\day 1/shot.mov".into());
    assert_eq!(m.file_name(), "shot.mov");
    m.path = Some("file:///Volumes/x/clip.mxf".into());
    assert_eq!(m.file_name(), "clip.mxf");
}

#[test]
fn keys_lists_every_node_once() {
    let mut d = doc_with(vec![track(vec![clip("a", 0, 10)])]);
    d.media
        .push(Media::new(k("@d-media:m"), MediaKind::File, "m"));
    let mut nest = Item::new(k("n"), ItemKind::Nest, "n", t(10), t(5));
    nest.tracks
        .push(Track::new(k("n/t"), "V1", TrackKind::Video));
    d.timelines[0].tracks[0].items.push(nest);
    d.timelines[0]
        .markers
        .push(Marker::new(k("m1"), MarkerKind::Marker, "m", t(0)));
    let keys: Vec<&str> = d.keys().iter().map(|k| k.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "@d-media:m",
            "@d-seq:1",
            "@d-seq:1/d-trk:V1",
            "a",
            "n",
            "n/t",
            "m1"
        ]
    );
}
