//! The FCPXML reader on hand-written documents: lanes, placement from offsets, markers,
//! transitions' cuts, compound clips, keys, our own schema, and every refusal typed.

mod xml_support;

use k2_mpc_timeline::fcpxml::read_fcpxml;
use k2_mpc_timeline::model::{
    place_steps, ItemKind, KeyOrigin, MarkerKind, MediaKind, Step, TrackKind,
};
use k2_mpc_timeline::{Document, Rate, ReadError, ReadLimits, Time, TimeError};

const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.10">
  <resources>
    <format id="r1" name="F" frameDuration="1001/30000s" width="1920" height="1080"/>
    <asset id="r2" name="interview" uid="AAA111" start="0s" duration="600s" hasVideo="1" hasAudio="1" format="r1">
      <media-rep kind="original-media" src="file:///media/interview.mov"/>
    </asset>
    <asset id="r3" name="broll" start="0s" duration="60s" format="r1" src="file:///media/broll.mov"/>
    <effect id="r4" name="Basic Title" uid="titles/basic"/>
    <effect id="r5" name="Cross Dissolve" uid="fx:dissolve"/>
    <media id="r6" name="compound" uid="CMP1">
      <sequence format="r1" duration="10s" tcStart="0s" tcFormat="NDF">
        <spine><asset-clip ref="r3" offset="0s" duration="10s" name="b"/></spine>
      </sequence>
    </media>
  </resources>
  <library>
    <event name="E" uid="EV1">
      <project name="P" uid="PRJ1">
        <sequence format="r1" duration="40s" tcStart="3600s" tcFormat="DF" audioRate="48k">
          <spine>
            <asset-clip ref="r2" offset="3600s" start="10s" duration="20s" name="A">
              <marker start="12s" duration="1001/30000s" value="look" completed="0"/>
              <asset-clip ref="r3" lane="1" offset="12s" start="5s" duration="2s" name="cut-in"/>
              <adjust-volume amount="-6dB"/>
            </asset-clip>
            <transition name="Cross Dissolve" offset="3619s" duration="2s">
              <filter-video ref="r5" name="Cross Dissolve"/>
            </transition>
            <gap name="Gap" offset="3620s" start="3600s" duration="10s">
              <title ref="r4" lane="1" offset="3602s" duration="3s" name="Lower third">
                <text><text-style ref="ts1">Jane Doe</text-style></text>
              </title>
            </gap>
            <ref-clip ref="r6" offset="3630s" duration="10s" name="compound"/>
          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>
"#;

fn read(s: &str) -> Result<Document, ReadError> {
    read_fcpxml(s.as_bytes(), &ReadLimits::default())
}

fn s(n: i64) -> Time {
    Time::seconds(n)
}

#[test]
fn a_project_reads_into_lanes_at_absolute_times() {
    let doc = read(DOC).unwrap();
    assert_eq!(doc.version, "1.10");
    assert_eq!(doc.check(), vec![]);
    assert_eq!(doc.timelines.len(), 2);
    let tl = &doc.timelines[0];
    assert_eq!(
        (tl.key.as_str(), tl.key.origin()),
        ("@project:PRJ1", KeyOrigin::Native)
    );
    assert_eq!(tl.name, "P");
    assert_eq!(tl.rate, Some(Rate::ntsc(30).unwrap()));
    assert_eq!(
        (tl.width, tl.height, tl.audio_rate),
        (Some(1920), Some(1080), Some(48000))
    );
    assert_eq!(
        (tl.tc_start, tl.drop_frame, tl.duration),
        (s(3600), true, Some(s(40)))
    );
    assert!(!tl.compound);

    // the lowest lane first: a later track composites over an earlier one, as in OTIO
    let lanes: Vec<(&str, TrackKind)> = tl
        .tracks
        .iter()
        .map(|t| (t.name.as_str(), t.kind))
        .collect();
    assert_eq!(
        lanes,
        vec![("lane 0", TrackKind::Video), ("lane 1", TrackKind::Video)]
    );
    let primary = &tl.tracks[0];
    let got: Vec<(ItemKind, Time, Time)> = primary
        .items
        .iter()
        .map(|i| (i.kind, i.start, i.duration))
        .collect();
    assert_eq!(
        got,
        vec![
            (ItemKind::Clip, s(0), s(20)),
            (ItemKind::Transition, s(19), s(2)),
            (ItemKind::Gap, s(20), s(10)),
            (ItemKind::Nest, s(30), s(10)),
        ]
    );
    let a = &primary.items[0];
    assert_eq!(
        a.key.as_str(),
        "@project:PRJ1/d-lane:0/d-clip:interview@10s"
    );
    assert_eq!(a.source_in, Some(s(10)));
    assert_eq!(a.media.as_ref().map(|k| k.as_str()), Some("@media:AAA111"));
    assert_eq!(
        a.opaque.iter().map(|o| o.what.as_str()).collect::<Vec<_>>(),
        vec!["fcpxml:adjust-volume"]
    );
    let mk = &a.markers[0];
    assert_eq!(
        (mk.kind, mk.name.as_str(), mk.at, mk.media_at),
        (MarkerKind::Todo, "look", s(2), Some(s(12)))
    );
    assert_eq!(mk.duration, Time::new(1001, 30000).unwrap());
    let tr = &primary.items[1];
    assert_eq!(tr.transition.as_ref().unwrap().in_offset, Some(s(1)));
    assert_eq!(tr.effects, vec!["Cross Dissolve".to_owned()]);
    assert_eq!(tr.key.as_str(), "@project:PRJ1/d-lane:0/d-tr:@19s");
    let nest = &primary.items[3];
    assert_eq!(nest.media.as_ref().map(|k| k.as_str()), Some("@media:CMP1"));

    // connected items: their parent's record start plus offset less the parent's start
    let upper = &tl.tracks[1];
    let cut_in = &upper.items[0];
    assert_eq!(
        (cut_in.start, cut_in.duration, cut_in.source_in),
        (s(2), s(2), Some(s(5)))
    );
    assert_eq!(
        cut_in.key.as_str(),
        "@project:PRJ1/d-lane:1/d-clip:broll@5s"
    );
    let title = &upper.items[1];
    assert_eq!(
        (title.kind, title.start, title.text.as_deref()),
        (ItemKind::Title, s(22), Some("Jane Doe"))
    );
    assert_eq!(title.effects, vec!["Basic Title".to_owned()]);

    // the compound clip is its own timeline, and its media points at it
    let comp = &doc.timelines[1];
    assert!(comp.compound);
    assert_eq!(comp.key.as_str(), "@compound:CMP1");
    let m = doc
        .media
        .iter()
        .find(|m| m.key.as_str() == "@media:CMP1")
        .unwrap();
    assert_eq!(
        (m.kind, m.sequence.as_ref()),
        (MediaKind::Sequence, Some(&comp.key))
    );
    let broll = doc.media.iter().find(|m| m.name == "broll").unwrap();
    assert_eq!(
        (broll.key.as_str(), broll.file_name()),
        ("@d-media:broll", "broll.mov")
    );
    assert_eq!(doc.media.len(), 3, "broll is one entry for both its uses");
    let interview = doc.media.iter().find(|m| m.name == "interview").unwrap();
    assert_eq!(
        interview.path.as_deref(),
        Some("file:///media/interview.mov")
    );
    assert_eq!(interview.rate, Some(Rate::ntsc(30).unwrap()));
}

#[test]
fn the_primary_storyline_round_trips_through_its_sequential_form() {
    let doc = read(DOC).unwrap();
    let primary = &doc.timelines[0].tracks[0];
    let steps = primary.to_steps().unwrap();
    assert_eq!(
        steps,
        vec![
            Step::Item(0),
            Step::Transition {
                index: 1,
                in_offset: s(1)
            },
            Step::Item(2),
            Step::Item(3),
        ]
    );
    for (n, start) in place_steps(&steps, &primary.items).unwrap() {
        assert_eq!(start, primary.items[n].start);
    }
    // the upper lane has holes: gaps in its sequential form
    let upper = &doc.timelines[0].tracks[1];
    assert_eq!(
        upper.to_steps().unwrap(),
        vec![
            Step::Gap(s(2)),
            Step::Item(0),
            Step::Gap(s(18)),
            Step::Item(1)
        ]
    );
}

#[test]
fn reading_is_deterministic() {
    assert_eq!(read(DOC).unwrap(), read(DOC).unwrap());
}

#[test]
fn an_audition_stands_in_for_its_first_pick() {
    let d = DOC.replace(
        r#"<ref-clip ref="r6" offset="3630s" duration="10s" name="compound"/>"#,
        r#"<audition offset="3630s"><asset-clip ref="r3" offset="0s" start="1s" duration="10s" name="pick1"/><asset-clip ref="r2" offset="0s" duration="10s" name="pick2"/></audition>"#,
    );
    let doc = read(&d).unwrap();
    let it = &doc.timelines[0].tracks[0].items[3];
    assert_eq!(
        (it.name.as_str(), it.start, it.source_in),
        ("pick1", s(30), Some(s(1)))
    );
    assert_eq!(
        it.opaque
            .iter()
            .filter(|o| o.what == "fcpxml:audition-pick")
            .count(),
        1
    );
}

#[test]
fn a_secondary_storyline_puts_its_items_on_its_lane() {
    let d = DOC.replace(
        r#"<adjust-volume amount="-6dB"/>"#,
        r#"<spine lane="-1" offset="14s"><asset-clip ref="r3" offset="0s" duration="1s" name="s1"/><asset-clip ref="r3" offset="1s" start="1s" duration="1s" name="s2"/></spine>"#,
    );
    let doc = read(&d).unwrap();
    let lanes: Vec<&str> = doc.timelines[0]
        .tracks
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(lanes, vec!["lane -1", "lane 0", "lane 1"]);
    let low = doc.timelines[0].tracks.first().unwrap();
    assert_eq!((low.name.as_str(), low.kind), ("lane -1", TrackKind::Audio));
    let starts: Vec<Time> = low.items.iter().map(|i| i.start).collect();
    assert_eq!(starts, vec![s(4), s(5)]);
}

fn refused(d: &str) -> ReadError {
    match read(d) {
        Ok(_) => panic!("read"),
        Err(e) => e,
    }
}

#[test]
fn refusals_are_typed() {
    for v in ["1.7", "2.0", ""] {
        let d = DOC.replace(r#"version="1.10""#, &format!(r#"version="{v}""#));
        assert!(
            matches!(refused(&d), ReadError::UnknownVersion { .. }),
            "{v}"
        );
    }
    let d = DOC.replace(r#"<gap name="Gap""#, r#"<widget/><gap name="Gap""#);
    assert!(matches!(
        refused(&d),
        ReadError::Unknown {
            what: "FCPXML element",
            ..
        }
    ));
    let d = DOC.replace(r#"<adjust-volume amount="-6dB"/>"#, "<sparkle/>");
    assert!(matches!(
        refused(&d),
        ReadError::Unknown {
            what: "FCPXML element",
            ..
        }
    ));
    let d = DOC.replace(r#"name="A">"#, r#"name="A" duration="">"#);
    assert!(matches!(
        refused(&d),
        ReadError::Syntax { .. } | ReadError::Time(_)
    ));
    let d = DOC.replace(
        r#"start="10s" duration="20s" name="A""#,
        r#"start="10s" name="A""#,
    );
    assert!(matches!(refused(&d), ReadError::Structure { .. }));
    let d = DOC.replace(r#"<ref-clip ref="r6""#, r#"<ref-clip ref="r99""#);
    assert!(matches!(
        refused(&d),
        ReadError::Structure {
            why: "a reference to no resource"
        }
    ));
    let d = DOC.replace(r#"offset="3619s""#, r#"offset="abc""#);
    assert!(matches!(
        refused(&d),
        ReadError::Time(TimeError::Syntax { .. })
    ));
    let d = DOC.replace(r#"duration="2s">"#, r#"duration="-2s">"#);
    assert!(matches!(
        refused(&d),
        ReadError::Structure {
            why: "a negative duration"
        }
    ));
    let d = DOC.replace(
        "<!DOCTYPE fcpxml>",
        r#"<!DOCTYPE fcpxml [<!ENTITY a "aaaa">]>"#,
    );
    assert!(matches!(
        refused(&d),
        ReadError::Structure {
            why: "a document type with its own declarations"
        }
    ));
    assert!(matches!(
        refused("<fcpxml version=\"1.10\">"),
        ReadError::Syntax { format: "xml", .. }
    ));
    assert!(matches!(
        refused("<other/>"),
        ReadError::Unknown {
            what: "root element",
            ..
        }
    ));
    assert!(matches!(
        read_fcpxml(&[0xff, 0xfe], &ReadLimits::default()),
        Err(ReadError::Syntax {
            format: "utf-8",
            ..
        })
    ));
    assert!(matches!(
        read_fcpxml(DOC.as_bytes(), &ReadLimits::new(100, 128, 1000)),
        Err(ReadError::TooLarge { limit: 100 })
    ));
    assert!(matches!(
        read_fcpxml(DOC.as_bytes(), &ReadLimits::new(1 << 20, 128, 20)),
        Err(ReadError::Limit { .. })
    ));
    let d = DOC.replace(r#"<asset id="r3""#, r#"<asset id="r2""#);
    assert!(matches!(
        refused(&d),
        ReadError::Structure {
            why: "two resources share an id"
        }
    ));
}

#[test]
fn an_attribute_is_read_by_its_plain_name_never_a_namespaced_one() {
    xml_support::row_attributes_by_plain_name_only().unwrap();
}
