//! The PRPROJ reader on a hand-written project (our own text, in the object layout the format
//! uses): the object table, classes and versions, ticks, tracks, clips, nests, markers,
//! transitions' cuts, native keys, and every refusal typed.

use std::io::Write;

use k2_mpc_timeline::model::{
    place_steps, ItemKind, KeyOrigin, MarkerKind, MediaKind, Step, TrackKind,
};
use k2_mpc_timeline::prproj::{read_prproj, TICKS_PER_SECOND};
use k2_mpc_timeline::{Document, Rate, ReadError, ReadLimits, Time};

/// Ticks for `n` half seconds.
fn h(n: i64) -> String {
    (n * TICKS_PER_SECOND / 2).to_string()
}

fn half(n: i64) -> Time {
    Time::new(n, 2).unwrap()
}

fn project() -> String {
    let marker = |id: &str, name: &str, typ: &str, start: &str| {
        format!(
            r#"{{"DVAMarker":{{"mMarkerID":"{id}","mName":"{name}","mStartTime":{{"ticks":{start}}},"mType":"{typ}","mComment":"note {name}"}}}}"#
        )
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" ?>
<PremiereData Version="3">
  <Project ObjectRef="1"/>
  <Project ObjectID="1" ClassID="62ad66dd-0dcd-42da-a660-6d8fbde94876" Version="45"><Node Version="1"/></Project>
  <Sequence ObjectUID="S1" ClassID="6a15d903-8739-11d5-af2d-9b7855ad8974" Version="12">
    <MarkerOwner Version="1"><Markers ObjectRef="10"/></MarkerOwner>
    <TrackGroups Version="1">
      <TrackGroup Version="1" Index="0"><First>v</First><Second ObjectRef="20"/></TrackGroup>
      <TrackGroup Version="1" Index="1"><First>a</First><Second ObjectRef="21"/></TrackGroup>
      <TrackGroup Version="1" Index="2"><First>d</First><Second ObjectRef="22"/></TrackGroup>
    </TrackGroups>
    <Name>Seq A</Name>
  </Sequence>
  <Sequence ObjectUID="S2" ClassID="6a15d903-8739-11d5-af2d-9b7855ad8974" Version="12">
    <TrackGroups Version="1"/>
    <Name>Seq B</Name>
  </Sequence>
  <Markers ObjectID="10" ClassID="bee50706-b524-416c-9f03-b596ce5f6866" Version="4">
    <Markers Version="1"><Marker Version="1" Index="0"><First>MK1</First><Second ObjectRef="11"/></Marker></Markers>
  </Markers>
  <Marker ObjectID="11" ClassID="a45508e0-3ff7-4d04-90a7-2e0dfff4c910" Version="3"><DVAMarker>{m1}</DVAMarker></Marker>
  <VideoTrackGroup ObjectID="20" ClassID="9e9abf7a-0918-49c2-91ae-991b5dde77bb" Version="13">
    <TrackGroup Version="1"><Tracks Version="1"><Track Index="0" ObjectURef="T1"/></Tracks><FrameRate>8475667200</FrameRate></TrackGroup>
    <FrameRect>0,0,64,36</FrameRect>
  </VideoTrackGroup>
  <AudioTrackGroup ObjectID="21" ClassID="9b9238b9-53a8-4cc3-b03f-b36246d052e6" Version="6">
    <TrackGroup Version="1"><Tracks Version="1"><Track Index="0" ObjectURef="T2"/></Tracks><FrameRate>5292000</FrameRate></TrackGroup>
  </AudioTrackGroup>
  <DataTrackGroup ObjectID="22" ClassID="b714b71d-6838-48dd-9b77-db19088ced7e" Version="1">
    <TrackGroup Version="1"><FrameRate>8475667200</FrameRate></TrackGroup>
  </DataTrackGroup>
  <VideoClipTrack ObjectUID="T1" ClassID="f68dcd81-8805-11d5-af2d-9bfa89d4ddd4" Version="1">
    <ClipTrack Version="2">
      <Track Version="4"><ID>1</ID><Index>0</Index></Track>
      <ClipItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="30"/><TrackItem Index="1" ObjectRef="40"/></TrackItems></ClipItems>
      <TransitionItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="31"/></TrackItems></TransitionItems>
    </ClipTrack>
  </VideoClipTrack>
  <AudioClipTrack ObjectUID="T2" ClassID="097f6203-99ae-11d5-84f2-8cf14bde7040" Version="7">
    <ClipTrack Version="2">
      <Track Version="4"><ID>2</ID><Index>0</Index><IsMuted>true</IsMuted></Track>
      <ClipItems Version="3"><TrackItems Version="1"><TrackItem Index="0" ObjectRef="50"/></TrackItems></ClipItems>
    </ClipTrack>
  </AudioClipTrack>
  <VideoClipTrackItem ObjectID="30" ClassID="368b0406-29e3-4923-9fcd-094fbf9a1089" Version="8">
    <ClipTrackItem Version="8">
      <ComponentOwner Version="1"><Components ObjectRef="32"/></ComponentOwner>
      <TrackItem Version="4"><End>{e1}</End></TrackItem>
      <SubClip ObjectRef="33"/>
    </ClipTrackItem>
  </VideoClipTrackItem>
  <VideoComponentChain ObjectID="32" ClassID="0970e08a-f58f-4108-b29a-1a717b8e12e2" Version="3"><Gain>2</Gain></VideoComponentChain>
  <SubClip ObjectID="33" ClassID="e0c58dc9-dbdd-4166-aef7-5db7e3f22e84" Version="6">
    <Clip ObjectRef="34"/><MasterClip ObjectURef="M1"/><Name>red</Name>
  </SubClip>
  <VideoClip ObjectID="34" ClassID="9308dbef-2440-4acb-9ab2-953b9a4e82ec" Version="11">
    <Clip Version="18">
      <MarkerOwner Version="1"><Markers ObjectRef="35"/></MarkerOwner>
      <Source ObjectRef="36"/>
      <OutPoint>{o1}</OutPoint><InPoint>{i1}</InPoint>
      <ClipID>C1</ClipID>
    </Clip>
  </VideoClip>
  <Markers ObjectID="35" ClassID="bee50706-b524-416c-9f03-b596ce5f6866" Version="4">
    <Markers Version="1"><Marker Version="1" Index="0"><First>MK2</First><Second ObjectRef="37"/></Marker></Markers>
  </Markers>
  <Marker ObjectID="37" ClassID="a45508e0-3ff7-4d04-90a7-2e0dfff4c910" Version="3"><DVAMarker>{m2}</DVAMarker></Marker>
  <VideoMediaSource ObjectID="36" ClassID="e64ddf74-8fac-4682-8aa8-0e0ca2248949" Version="2">
    <MediaSource Version="4"><Media ObjectURef="MD1"/></MediaSource>
  </VideoMediaSource>
  <Media ObjectUID="MD1" ClassID="7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff" Version="30">
    <FilePath>C:\media\red.bmp</FilePath><Title>red.bmp</Title>
  </Media>
  <VideoTransitionTrackItem ObjectID="31" ClassID="3eeaed31-f78e-4144-b8e8-077656517181" Version="6">
    <TransitionTrackItem Version="3">
      <TrackItem Version="4"><End>{t1}</End></TrackItem>
      <DisplayName>Dissolve</DisplayName><MatchName>Basic Dissolve</MatchName>
      <HasOutgoingClip>false</HasOutgoingClip><HasIncomingClip>true</HasIncomingClip>
    </TransitionTrackItem>
  </VideoTransitionTrackItem>
  <VideoClipTrackItem ObjectID="40" ClassID="368b0406-29e3-4923-9fcd-094fbf9a1089" Version="8">
    <ClipTrackItem Version="8">
      <TrackItem Version="4"><Start>{e1}</Start><End>{e2}</End></TrackItem>
      <SubClip ObjectRef="41"/>
    </ClipTrackItem>
  </VideoClipTrackItem>
  <SubClip ObjectID="41" ClassID="e0c58dc9-dbdd-4166-aef7-5db7e3f22e84" Version="6"><Clip ObjectRef="42"/><Name>Seq B</Name></SubClip>
  <VideoClip ObjectID="42" ClassID="9308dbef-2440-4acb-9ab2-953b9a4e82ec" Version="11">
    <Clip Version="18"><Source ObjectRef="43"/><ClipID>C2</ClipID></Clip>
  </VideoClip>
  <VideoSequenceSource ObjectID="43" ClassID="4752dfa9-7a7e-4a3b-a25b-cafde1a8d036" Version="3">
    <SequenceSource Version="4"><Sequence ObjectURef="S2"/></SequenceSource>
  </VideoSequenceSource>
  <AudioClipTrackItem ObjectID="50" ClassID="064ec682-9ba6-11d5-af2d-9ca32c7d6164" Version="11">
    <ClipTrackItem Version="8">
      <TrackItem Version="4"><End>{e1}</End></TrackItem>
      <SubClip ObjectRef="51"/>
      <IsMuted>true</IsMuted>
    </ClipTrackItem>
  </AudioClipTrackItem>
  <SubClip ObjectID="51" ClassID="e0c58dc9-dbdd-4166-aef7-5db7e3f22e84" Version="6"><Clip ObjectRef="52"/><Name>red</Name></SubClip>
  <AudioClip ObjectID="52" ClassID="b8830d03-de02-41ee-84ec-fe566dc70cd9" Version="8">
    <Clip Version="18"><Source ObjectRef="53"/><InPoint>{i1}</InPoint><ClipID>C3</ClipID></Clip>
  </AudioClip>
  <AudioMediaSource ObjectID="53" ClassID="f588da05-fc2a-4fbc-9383-74d653b379e3" Version="2">
    <MediaSource Version="4"><Media ObjectURef="MD1"/></MediaSource>
  </AudioMediaSource>
</PremiereData>
"#,
        m1 = marker("MK1", "chapter", "Chapter", &h(2)),
        m2 = marker("MK2", "item", "Comment", &h(3)),
        e1 = h(4),
        e2 = h(8),
        i1 = h(2),
        o1 = h(6),
        t1 = h(1),
    )
}

fn gz(s: &str) -> Vec<u8> {
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(s.as_bytes()).unwrap();
    z.finish().unwrap()
}

fn read(s: &str) -> Result<Document, ReadError> {
    read_prproj(&gz(s), &ReadLimits::default())
}

#[test]
fn a_project_reads_with_native_keys() {
    let doc = read(&project()).unwrap();
    assert_eq!(doc.version, "3");
    assert_eq!(doc.check(), vec![]);
    let names: Vec<&str> = doc.timelines.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["Seq A", "Seq B"]);
    let tl = &doc.timelines[0];
    assert_eq!(
        (tl.key.as_str(), tl.key.origin()),
        ("@seq:S1", KeyOrigin::Native)
    );
    assert_eq!(tl.rate, Some(Rate::ntsc(30).unwrap()));
    assert_eq!(
        (tl.width, tl.height, tl.audio_rate),
        (Some(64), Some(36), Some(48000))
    );
    let m = &tl.markers[0];
    assert_eq!(
        (m.key.as_str(), m.kind, m.name.as_str(), m.at),
        ("@mk:MK1", MarkerKind::Chapter, "chapter", half(2))
    );
    assert_eq!(m.note.as_deref(), Some("note chapter"));

    let tracks: Vec<(&str, &str, TrackKind, bool)> = tl
        .tracks
        .iter()
        .map(|t| (t.key.as_str(), t.name.as_str(), t.kind, t.muted))
        .collect();
    assert_eq!(
        tracks,
        vec![
            ("@track:T1", "V1", TrackKind::Video, false),
            ("@track:T2", "A1", TrackKind::Audio, true)
        ]
    );
    let v1 = &tl.tracks[0];
    let got: Vec<(&str, ItemKind, Time, Time)> = v1
        .items
        .iter()
        .map(|i| (i.key.as_str(), i.kind, i.start, i.duration))
        .collect();
    assert_eq!(
        got,
        vec![
            ("@clip:C1", ItemKind::Clip, half(0), half(4)),
            ("@track:T1/d-tr:@0s", ItemKind::Transition, half(0), half(1)),
            ("@clip:C2", ItemKind::Nest, half(4), half(4)),
        ]
    );
    let c1 = &v1.items[0];
    assert_eq!(c1.source_in, Some(half(2)));
    assert_eq!(c1.media.as_ref().map(|k| k.as_str()), Some("@media:MD1"));
    assert_eq!(
        c1.opaque
            .iter()
            .map(|o| o.what.as_str())
            .collect::<Vec<_>>(),
        vec!["prproj:components"]
    );
    // the source's marker, in media time, placed by the clip's in-point
    let mk = &c1.markers[0];
    assert_eq!(
        (mk.key.as_str(), mk.at, mk.media_at),
        ("@clip:C1/mk:MK2", half(1), Some(half(3)))
    );
    // a transition with no outgoing clip cuts at its start
    let tr = &v1.items[1];
    let info = tr.transition.as_ref().unwrap();
    assert_eq!(
        (tr.name.as_str(), info.kind.as_str(), info.in_offset),
        ("Dissolve", "Basic Dissolve", Some(Time::ZERO))
    );
    // the nest's media is the other sequence
    let nest_media = doc
        .media
        .iter()
        .find(|m| m.kind == MediaKind::Sequence)
        .unwrap();
    assert_eq!(nest_media.sequence.as_ref(), Some(&doc.timelines[1].key));
    assert_eq!(v1.items[2].media.as_ref(), Some(&nest_media.key));
    // the audio clip shares the media, and is disabled
    let a = &tl.tracks[1].items[0];
    assert_eq!((a.key.as_str(), a.enabled), ("@clip:C3", false));
    assert_eq!(a.media, c1.media);
    assert_eq!(doc.media.len(), 2);
    let file = doc
        .media
        .iter()
        .find(|m| m.kind == MediaKind::File)
        .unwrap();
    assert_eq!(
        (file.name.as_str(), file.file_name()),
        ("red.bmp", "red.bmp")
    );
}

/// A group's tracks read in their own index order, the lowest first, whatever order the group
/// lists them in: a later track composites over an earlier one, as in OTIO.
#[test]
fn tracks_read_lowest_index_first() {
    let p = project()
        .replace(
            r#"<Tracks Version="1"><Track Index="0" ObjectURef="T1"/></Tracks><FrameRate>8475667200</FrameRate>"#,
            r#"<Tracks Version="1"><Track Index="0" ObjectURef="T3"/><Track Index="1" ObjectURef="T1"/></Tracks><FrameRate>8475667200</FrameRate>"#,
        )
        .replace(
            r#"  <AudioClipTrack ObjectUID="T2""#,
            r#"  <VideoClipTrack ObjectUID="T3" ClassID="f68dcd81-8805-11d5-af2d-9bfa89d4ddd4" Version="1"><ClipTrack Version="2"><Track Version="4"><ID>3</ID><Index>1</Index></Track></ClipTrack></VideoClipTrack>
  <AudioClipTrack ObjectUID="T2""#,
        );
    assert!(
        p.contains(r#"ObjectURef="T3"/><Track Index="1" ObjectURef="T1""#)
            && p.contains(r#"<VideoClipTrack ObjectUID="T3""#),
        "control: the group lists V2 before V1"
    );
    let doc = read(&p).unwrap();
    let tracks: Vec<(&str, &str)> = doc.timelines[0]
        .tracks
        .iter()
        .map(|t| (t.key.as_str(), t.name.as_str()))
        .collect();
    assert_eq!(
        tracks,
        vec![
            ("@track:T1", "V1"),
            ("@track:T3", "V2"),
            ("@track:T2", "A1")
        ]
    );
}

#[test]
fn plain_xml_reads_too_and_the_same() {
    let p = project();
    let plain = read_prproj(p.as_bytes(), &ReadLimits::default()).unwrap();
    assert_eq!(plain, read(&p).unwrap());
}

#[test]
fn a_track_round_trips_through_its_sequential_form() {
    let doc = read(&project()).unwrap();
    let v1 = &doc.timelines[0].tracks[0];
    let steps = v1.to_steps().unwrap();
    assert_eq!(
        steps,
        vec![
            Step::Transition {
                index: 1,
                in_offset: Time::ZERO
            },
            Step::Item(0),
            Step::Item(2)
        ]
    );
    for (n, start) in place_steps(&steps, &v1.items).unwrap() {
        assert_eq!(start, v1.items[n].start);
    }
}

#[test]
fn a_component_change_shows_in_the_payload_not_the_fingerprint() {
    let a = read(&project()).unwrap();
    let b = read(&project().replace("<Gain>2</Gain>", "<Gain>3</Gain>")).unwrap();
    let (x, y) = (
        &a.timelines[0].tracks[0].items[0],
        &b.timelines[0].tracks[0].items[0],
    );
    assert_eq!(x.key, y.key);
    assert_eq!(x.fingerprint(), y.fingerprint());
    assert_ne!(x.payload(), y.payload());
}

fn refused(s: &str) -> ReadError {
    match read(s) {
        Ok(_) => panic!("read"),
        Err(e) => e,
    }
}

#[test]
fn refusals_are_typed() {
    let p = project();
    let e = refused(&p.replace(
        r#"<PremiereData Version="3">"#,
        r#"<PremiereData Version="4">"#,
    ));
    assert!(matches!(e, ReadError::UnknownVersion { .. }));
    let e = refused(&p.replacen(r#"Version="12">"#, r#"Version="13">"#, 1));
    assert!(matches!(e, ReadError::UnknownVersion { .. }), "{e}");
    let e = refused(&p.replace(
        "368b0406-29e3-4923-9fcd-094fbf9a1089",
        "00000000-0000-0000-0000-000000000000",
    ));
    assert!(matches!(
        e,
        ReadError::Unknown {
            what: "PRPROJ class ID",
            ..
        }
    ));
    let e = refused(&p.replace(
        r#"<TrackItem Index="1" ObjectRef="40"/>"#,
        r#"<TrackItem Index="1" ObjectRef="99"/>"#,
    ));
    assert!(matches!(
        e,
        ReadError::Structure {
            why: "a reference to no object"
        }
    ));
    let e = refused(&p.replace(
        r#"<TrackItem Index="1" ObjectRef="40"/>"#,
        r#"<TrackItem Index="1" ObjectRef="32"/>"#,
    ));
    assert!(matches!(
        e,
        ReadError::Unknown {
            what: "PRPROJ class",
            ..
        }
    ));
    let e = refused(&p.replace(
        &format!("<Start>{}</Start>", h(4)),
        &format!("<Start>{}</Start>", h(9)),
    ));
    assert!(matches!(
        e,
        ReadError::Structure {
            why: "a track item that ends before it starts"
        }
    ));
    let e = refused(&p.replace(r#"<Second ObjectRef="21"/>"#, ""));
    assert!(matches!(e, ReadError::Structure { .. }));
    let e = refused(&p.replace(r#"ObjectID="53""#, r#"ObjectID="52""#));
    assert!(matches!(
        e,
        ReadError::Structure {
            why: "two objects share an ID"
        }
    ));
    let e = refused(&p.replace("<End>", "<End>x"));
    assert!(matches!(e, ReadError::Structure { .. }));
    let e = refused(&p.replace("PremiereData", "OtherData"));
    assert!(matches!(
        e,
        ReadError::Unknown {
            what: "root element",
            ..
        }
    ));
    // a damaged stream, a truncated one, and one past the size limit
    let mut z = gz(&p);
    let mid = z.len() / 2;
    z[mid] ^= 0xff;
    assert!(read_prproj(&z, &ReadLimits::default()).is_err());
    let z = gz(&p);
    assert!(matches!(
        read_prproj(&z[..z.len() - 4], &ReadLimits::default()),
        Err(ReadError::Syntax { format: "gzip", .. })
    ));
    let big = gz(&format!("{p}{}", " ".repeat(1 << 20)));
    assert!(matches!(
        read_prproj(&big, &ReadLimits::new(1 << 16, 128, 1 << 20)),
        Err(ReadError::TooLarge { limit: 65536 })
    ));
}
