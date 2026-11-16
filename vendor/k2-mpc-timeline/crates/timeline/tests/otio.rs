//! The OTIO reader on hand-built files: placement, media, markers, keys, the sequential form
//! round trip, carried pieces, and every refusal typed.

use k2_mpc_timeline::model::{place_steps, ItemKind, KeyOrigin, MarkerKind, MediaKind, Step};
use k2_mpc_timeline::otio::read_otio;
use k2_mpc_timeline::{Document, Rate, ReadError, ReadLimits, Time, TimeError};

fn rt(v: &str, r: &str) -> String {
    format!(r#"{{"OTIO_SCHEMA": "RationalTime.1", "value": {v}, "rate": {r}}}"#)
}

fn range(s: &str, d: &str, r: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "TimeRange.1", "start_time": {}, "duration": {}}}"#,
        rt(s, r),
        rt(d, r)
    )
}

fn timeline(tracks: &str, extra: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "Timeline.1", "name": "cut", "metadata": {{}}, {extra}
            "tracks": {{"OTIO_SCHEMA": "Stack.1", "name": "tracks", "children": [{tracks}],
                        "markers": [], "effects": [], "metadata": {{}}}}}}"#
    )
}

fn track(kind: &str, name: &str, children: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "Track.1", "kind": "{kind}", "name": "{name}", "children": [{children}],
             "markers": [], "effects": [], "metadata": {{}}, "source_range": null}}"#
    )
}

fn clip(name: &str, media: &str, src: &str, markers: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "Clip.1", "name": "{name}", "media_reference": {media},
             "source_range": {src}, "markers": [{markers}], "effects": [], "metadata": {{}}, "enabled": true}}"#
    )
}

fn external(url: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "ExternalReference.1", "name": "", "target_url": "{url}",
             "available_range": {}, "metadata": {{}}}}"#,
        range("0", "240", "24")
    )
}

fn gap(frames: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "Gap.1", "name": "", "source_range": {}, "markers": [], "effects": [], "metadata": {{}}}}"#,
        range("0", frames, "24")
    )
}

fn transition(inn: &str, out: &str) -> String {
    format!(
        r#"{{"OTIO_SCHEMA": "Transition.1", "name": "x", "transition_type": "SMPTE_Dissolve",
             "in_offset": {}, "out_offset": {}, "metadata": {{}}, "parameters": {{}}}}"#,
        rt(inn, "24"),
        rt(out, "24")
    )
}

fn read(s: &str) -> Result<Document, ReadError> {
    read_otio(s.as_bytes(), &ReadLimits::default())
}

fn f(n: i64) -> Time {
    Time::frames(n, Rate::whole(24).unwrap()).unwrap()
}

fn sample() -> String {
    let marker = format!(
        r#"{{"OTIO_SCHEMA": "Marker.2", "name": "look", "color": "RED", "marked_range": {}, "metadata": {{}}}}"#,
        range("20", "0", "24")
    );
    let nest = format!(
        r#"{{"OTIO_SCHEMA": "Stack.1", "name": "inner", "children": [{}], "markers": [], "effects": [], "metadata": {{}}}}"#,
        track("Video", "", &clip("c", "null", &range("0", "12", "24"), ""))
    );
    let v1 = track(
        "Video",
        "",
        &[
            gap("24"),
            clip(
                "a",
                &external("file:///media/a.mov"),
                &range("10", "48", "24"),
                &marker,
            ),
            transition("6", "6"),
            clip("b", "null", &range("0", "24", "24"), ""),
            nest,
        ]
        .join(","),
    );
    let a1 = track(
        "Audio",
        "Dialog",
        &clip(
            "a",
            &external("file:///media/a.mov"),
            &range("10", "48", "24"),
            "",
        ),
    );
    timeline(
        &[v1, a1].join(","),
        &format!(r#""global_start_time": {},"#, rt("86400", "24")),
    )
}

#[test]
fn a_timeline_reads_into_absolute_time() {
    let doc = read(&sample()).unwrap();
    assert_eq!(doc.version, "Timeline.1");
    assert_eq!(doc.check(), vec![]);
    let tl = &doc.timelines[0];
    assert_eq!(tl.name, "cut");
    assert_eq!(tl.key.as_str(), "@d-seq:1");
    assert_eq!(tl.rate, Some(Rate::whole(24).unwrap()));
    assert_eq!(tl.tc_start, Time::seconds(3600));
    let names: Vec<&str> = tl.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["V1", "Dialog"]);
    assert_eq!(tl.tracks[1].key.as_str(), "@d-seq:1/d-trk:A1");

    let v1 = &tl.tracks[0];
    let kinds: Vec<ItemKind> = v1.items.iter().map(|i| i.kind).collect();
    assert_eq!(
        kinds,
        vec![
            ItemKind::Gap,
            ItemKind::Clip,
            ItemKind::Transition,
            ItemKind::Clip,
            ItemKind::Nest
        ]
    );
    let starts: Vec<Time> = v1.items.iter().map(|i| i.start).collect();
    assert_eq!(starts, vec![f(0), f(24), f(66), f(72), f(96)]);
    let a = &v1.items[1];
    assert_eq!((a.duration, a.source_in), (f(48), Some(f(10))));
    assert_eq!(a.key.as_str(), "@d-seq:1/d-trk:V1/d-clip:a.mov@5_12s");
    assert_eq!(a.key.origin(), KeyOrigin::Derived);
    let tr = &v1.items[2];
    assert_eq!(tr.duration, f(12));
    assert_eq!(tr.transition.as_ref().unwrap().in_offset, Some(f(6)));
    assert_eq!(tr.transition.as_ref().unwrap().kind, "SMPTE_Dissolve");
    // a clip with no media is named for itself
    assert_eq!(v1.items[3].key.as_str(), "@d-seq:1/d-trk:V1/d-clip:b@0s");
    // the nest holds its own track, in its own time; its length is its longest track's
    let nest = &v1.items[4];
    assert_eq!(nest.duration, f(12));
    assert_eq!(nest.tracks.len(), 1);
    assert_eq!(nest.tracks[0].items[0].start, f(0));
    assert_eq!(
        nest.tracks[0].key.as_str(),
        "@d-seq:1/d-trk:V1/d-nest:inner@0s/d-trk:V1"
    );

    // one media entry for two clips of the same file
    assert_eq!(doc.media.len(), 1);
    let m = &doc.media[0];
    assert_eq!(
        (m.kind, m.name.as_str(), m.file_name()),
        (MediaKind::File, "a.mov", "a.mov")
    );
    assert_eq!(m.duration, Some(f(240)));
    assert_eq!(a.media.as_ref(), Some(&m.key));
    assert_eq!(tl.tracks[1].items[0].media.as_ref(), Some(&m.key));

    // a clip's marker sits in media time, placed by the clip's source range
    let mk = &a.markers[0];
    assert_eq!((mk.kind, mk.name.as_str()), (MarkerKind::Marker, "look"));
    assert_eq!(mk.at, f(34));
    assert_eq!(mk.media_at, Some(f(20)));
    assert_eq!(mk.color.as_deref(), Some("RED"));
    assert_eq!(mk.key.as_str(), format!("{}/d-mk:look@5_6s", a.key));
}

#[test]
fn the_files_sequence_round_trips_through_the_model() {
    let doc = read(&sample()).unwrap();
    let v1 = &doc.timelines[0].tracks[0];
    let steps = v1.to_steps().unwrap();
    assert_eq!(
        steps,
        vec![
            Step::Item(0),
            Step::Item(1),
            Step::Transition {
                index: 2,
                in_offset: f(6)
            },
            Step::Item(3),
            Step::Item(4),
        ],
        "the file's own children, in order"
    );
    for (n, start) in place_steps(&steps, &v1.items).unwrap() {
        assert_eq!(start, v1.items[n].start);
    }
}

#[test]
fn reading_is_deterministic() {
    let a = read(&sample()).unwrap();
    let b = read(&sample()).unwrap();
    assert_eq!(a, b);
    let fa: Vec<_> = a.timelines[0].tracks[0]
        .items
        .iter()
        .map(|i| i.fingerprint())
        .collect();
    let fb: Vec<_> = b.timelines[0].tracks[0]
        .items
        .iter()
        .map(|i| i.fingerprint())
        .collect();
    assert_eq!(fa, fb);
}

#[test]
fn a_derived_key_survives_a_move_and_a_rename() {
    let one = read(&timeline(
        &track(
            "Video",
            "",
            &clip(
                "a",
                &external("file:///m/a.mov"),
                &range("10", "48", "24"),
                "",
            ),
        ),
        "",
    ))
    .unwrap();
    let moved = read(&timeline(
        &track(
            "Video",
            "",
            &[
                gap("100"),
                clip(
                    "renamed",
                    &external("file:///m/a.mov"),
                    &range("10", "48", "24"),
                    "",
                ),
            ]
            .join(","),
        ),
        "",
    ))
    .unwrap();
    let a = &one.timelines[0].tracks[0].items[0];
    let b = &moved.timelines[0].tracks[0].items[1];
    assert_eq!(a.key, b.key);
    assert_ne!(a.fingerprint(), b.fingerprint());
}

#[test]
fn transitions_at_a_tracks_start_are_kept_as_written() {
    let doc = read(&timeline(
        &track(
            "Video",
            "",
            &[
                transition("10", "10"),
                clip("a", "null", &range("0", "50", "24"), ""),
            ]
            .join(","),
        ),
        "",
    ))
    .unwrap();
    assert_eq!(doc.check(), vec![]);
    let t = &doc.timelines[0].tracks[0].items[0];
    assert_eq!((t.kind, t.start), (ItemKind::Transition, f(-10)));
}

#[test]
fn clip2_takes_its_active_reference_and_carries_the_others() {
    let c = format!(
        r#"{{"OTIO_SCHEMA": "Clip.2", "name": "a", "active_media_reference_key": "HIGH",
             "media_references": {{"DEFAULT_MEDIA": {}, "HIGH": {}}},
             "source_range": {}, "markers": [], "effects": [], "metadata": {{"x": 1}}}}"#,
        external("file:///proxy/a.mp4"),
        external("file:///full/a.mov"),
        range("0", "24", "24")
    );
    let doc = read(&timeline(&track("Video", "", &c), "")).unwrap();
    assert_eq!(doc.media.len(), 1);
    assert_eq!(doc.media[0].path.as_deref(), Some("file:///full/a.mov"));
    let it = &doc.timelines[0].tracks[0].items[0];
    let what: Vec<&str> = it.opaque.iter().map(|o| o.what.as_str()).collect();
    assert_eq!(what, vec!["otio:inactive-media-reference", "otio:metadata"]);
}

#[test]
fn effects_and_metadata_are_carried_including_non_finite_numbers() {
    let c = format!(
        r#"{{"OTIO_SCHEMA": "Clip.1", "name": "a", "media_reference": null, "source_range": {},
             "markers": [], "metadata": {{"vendor": {{"inf": Inf, "nan": NaN, "big": 170141183460469231731687303715884105728}}}},
             "effects": [{{"OTIO_SCHEMA": "LinearTimeWarp.1", "name": "", "effect_name": "", "time_scalar": 2.0, "metadata": {{}}}},
                         {{"OTIO_SCHEMA": "VendorBlur.4", "name": "soft", "metadata": {{}}}}]}}"#,
        range("0", "24", "24")
    );
    let doc = read(&timeline(&track("Video", "", &c), "")).unwrap();
    let it = &doc.timelines[0].tracks[0].items[0];
    assert_eq!(
        it.effects,
        vec!["LinearTimeWarp".to_owned(), "soft".to_owned()]
    );
    assert_eq!(it.opaque.len(), 3);
    assert!(it.payload().is_some());
}

#[test]
fn a_collection_holds_several_timelines() {
    let t1 = timeline(&track("Video", "", ""), "");
    let t2 = timeline(&track("Audio", "", ""), "");
    let s = format!(
        r#"{{"OTIO_SCHEMA": "SerializableCollection.1", "name": "bin", "metadata": {{}},
             "children": [{t1}, {{"OTIO_SCHEMA": "SerializableCollection.1", "children": [{t2}]}}]}}"#
    );
    let doc = read(&s).unwrap();
    let keys: Vec<&str> = doc.timelines.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, vec!["@d-seq:1", "@d-seq:2"]);
    assert_eq!(doc.version, "SerializableCollection.1");
}

#[test]
fn ntsc_rates_read_exactly() {
    let c = clip("a", "null", &range("0", "1001", "23.976023976023978"), "");
    let doc = read(&timeline(
        &track("Video", "", &c),
        &format!(r#""global_start_time": {},"#, rt("0", "23.976")),
    ))
    .unwrap();
    let tl = &doc.timelines[0];
    assert_eq!(tl.rate, Some(Rate::ntsc(24).unwrap()));
    assert_eq!(
        tl.tracks[0].items[0].duration,
        Time::new(1001 * 1001, 24000).unwrap()
    );
}

#[test]
fn a_stack_holding_clips_reads_them_as_layers() {
    // a nest whose stack holds a track and, directly, a clip: two layers from its zero
    let nest = format!(
        r#"{{"OTIO_SCHEMA": "Stack.1", "name": "layers", "children": [{}, {}]}}"#,
        track(
            "Video",
            "",
            &clip("under", "null", &range("0", "10", "24"), "")
        ),
        clip("over", "null", &range("5", "4", "24"), "")
    );
    let doc = read(&timeline(
        &track("Video", "", &[gap("2"), nest].join(",")),
        "",
    ))
    .unwrap();
    assert_eq!(doc.check(), vec![]);
    let n = &doc.timelines[0].tracks[0].items[1];
    assert_eq!((n.kind, n.start, n.duration), (ItemKind::Nest, f(2), f(10)));
    let names: Vec<&str> = n.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["V1", "O1"]);
    let over = &n.tracks[1].items[0];
    assert_eq!(
        (over.name.as_str(), over.start, over.source_in),
        ("over", f(0), Some(f(5)))
    );
    // the same at the top: a timeline's stack may hold a clip
    let top = read(&timeline(
        &clip("solo", "null", &range("0", "3", "24"), ""),
        "",
    ))
    .unwrap();
    assert_eq!(top.timelines[0].tracks[0].items[0].name, "solo");
}

fn refused(s: &str) -> ReadError {
    match read(s) {
        Ok(_) => panic!("read: {s}"),
        Err(e) => e,
    }
}

#[test]
fn refusals_are_typed() {
    // a root it doesn't place
    assert!(matches!(
        refused(&clip("a", "null", &range("0", "1", "24"), "")),
        ReadError::Unknown {
            what: "OTIO root schema",
            ..
        }
    ));
    // a version it doesn't know
    let t2 = timeline("", "").replace("Timeline.1", "Timeline.2");
    assert!(matches!(refused(&t2), ReadError::UnknownVersion { .. }));
    let c9 = timeline(
        &track("Video", "", &clip("a", "null", &range("0", "1", "24"), "")),
        "",
    )
    .replace("Clip.1", "Clip.9");
    assert!(matches!(refused(&c9), ReadError::UnknownVersion { .. }));
    // a track child it doesn't know
    let foo = timeline(&track("Video", "", r#"{"OTIO_SCHEMA": "Foo.1"}"#), "");
    assert!(matches!(
        refused(&foo),
        ReadError::Unknown {
            what: "OTIO schema",
            ..
        }
    ));
    // structure
    let noschema = timeline(&track("Video", "", r#"{"name": "x"}"#), "");
    assert!(matches!(refused(&noschema), ReadError::Structure { .. }));
    let nodur = timeline(&track("Video", "", &clip("a", "null", "null", "")), "");
    assert!(matches!(
        refused(&nodur),
        ReadError::Structure {
            why: "a clip without a duration"
        }
    ));
    let negative = timeline(
        &track("Video", "", &clip("a", "null", &range("0", "-5", "24"), "")),
        "",
    );
    assert!(matches!(
        refused(&negative),
        ReadError::Structure {
            why: "a negative duration"
        }
    ));
    let nan = timeline(
        &track(
            "Video",
            "",
            &clip("a", "null", &range("NaN", "5", "24"), ""),
        ),
        "",
    );
    assert!(matches!(refused(&nan), ReadError::Structure { .. }));
    // a rate it can't name
    let zero = timeline(
        &track("Video", "", &clip("a", "null", &range("0", "5", "0"), "")),
        "",
    );
    assert!(matches!(
        refused(&zero),
        ReadError::Time(TimeError::BadRate)
    ));
    // a stack may hold clips and gaps as layers, never a transition
    let stacked = timeline(&transition("1", "1"), "");
    assert!(matches!(
        refused(&stacked),
        ReadError::Structure {
            why: "a transition outside a track"
        }
    ));
    // malformed and over limits
    assert!(matches!(
        refused("{\"OTIO_SCHEMA\": "),
        ReadError::Syntax { .. }
    ));
    let small = ReadLimits::new(10, 128, 1000);
    assert!(matches!(
        read_otio(sample().as_bytes(), &small),
        Err(ReadError::TooLarge { limit: 10 })
    ));
    let shallow = ReadLimits::new(1 << 20, 4, 100_000);
    assert!(matches!(
        read_otio(sample().as_bytes(), &shallow),
        Err(ReadError::Limit { .. })
    ));
    let few = ReadLimits::new(1 << 20, 128, 20);
    assert!(matches!(
        read_otio(sample().as_bytes(), &few),
        Err(ReadError::Limit { .. })
    ));
    // a refusal's reason quotes nothing from the file
    let e = refused(&foo);
    assert_eq!(e.reason(), "an object type this reader doesn't know");
}
