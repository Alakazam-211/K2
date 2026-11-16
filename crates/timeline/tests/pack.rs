//! The structure index each pack makes: node kinds and tree, keys (and which it leaves to the
//! kit), what nodes hold, bounds, limits, and refusals.

use k2_mpc_base::index::{
    IndexDraft, IndexLimits, IndexRefused, NodeKey, NodeRect, INDEX_TEXT_MAX,
};
use k2_mpc_base::session::{AgentWrites, BlobPack};
use k2_mpc_timeline::pack::{FcpxmlPack, OtioPack, PrprojPack, VERBS};

mod index_support;

const OTIO: &str = include_str!("data/seed.otio");
const FCPXML: &str = include_str!("data/seed.fcpxml");
const PRPROJ: &str = include_str!("data/seed-prproj.xml");

fn limits() -> IndexLimits {
    IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX)
}

fn index(
    pack: &dyn BlobPack,
    bytes: &[u8],
    limits: &IndexLimits,
) -> Result<IndexDraft, IndexRefused> {
    pack.index(&mut &bytes[..], bytes.len() as u64, limits)
}

/// The draft keeps the kit's shape: parents earlier, keys in the grammar.
fn well_formed(d: &IndexDraft) {
    for (i, n) in d.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            assert!((p as usize) < i, "node {i}: its parent comes later");
        }
        if let Some(k) = &n.key {
            assert!(
                NodeKey::parse(k).is_some(),
                "node {i}: {k} is not a node key"
            );
        }
    }
}

/// Every sequence says how its tracks stack: in their order, a later one over an earlier one
/// (`stacking` = `last_on_top`, OTIO's meaning, whatever the format).
fn stacks_last_on_top(d: &IndexDraft) {
    let seqs: Vec<Option<&str>> = d
        .nodes
        .iter()
        .filter(|n| n.kind == "sequence")
        .map(|n| n.attrs.get("stacking").map(String::as_str))
        .collect();
    assert!(!seqs.is_empty(), "control: no sequence");
    assert!(
        seqs.iter().all(|s| *s == Some("last_on_top")),
        "each sequence's stacking: {seqs:?}"
    );
}

#[test]
fn an_otio_file_indexes_as_a_tree_of_timeline_nodes() {
    let d = index(&OtioPack, OTIO.as_bytes(), &limits()).unwrap();
    well_formed(&d);
    assert_eq!(d.source, "otio-Timeline.1");
    assert_eq!(d.attrs["timelines"], "1");
    assert_eq!(d.attrs["media"], "1");
    let kinds: Vec<&str> = d.nodes.iter().map(|n| n.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "sequence",
            "track",
            "clip",
            "marker",
            "transition",
            "nest",
            "track"
        ]
    );
    let parents: Vec<Option<u32>> = d.nodes.iter().map(|n| n.parent).collect();
    assert_eq!(
        parents,
        vec![None, Some(0), Some(1), Some(2), Some(1), Some(1), Some(5)]
    );

    let seq = &d.nodes[0];
    assert_eq!(seq.key.as_deref(), Some("@d-seq:1"));
    assert_eq!(seq.attrs["rate"], "24/1");
    assert_eq!(seq.attrs["bounds_unit"], "frames");
    let clip = &d.nodes[2];
    assert_eq!(
        clip.key.as_deref(),
        Some("@d-seq:1/d-trk:V1/d-clip:a.mov@1_12s")
    );
    assert_eq!(clip.attrs["start"], "5/24s");
    assert_eq!(clip.attrs["duration"], "3/8s");
    assert_eq!(clip.attrs["start_tc"], "00:00:00:05");
    assert_eq!(clip.attrs["source_in"], "1/12s");
    assert_eq!(
        (
            clip.attrs["media"].as_str(),
            clip.attrs["media_file"].as_str()
        ),
        ("a.mov", "a.mov")
    );
    assert_eq!(clip.weak["source_in"], "1/12s");
    assert_eq!(
        clip.bounds,
        Some(NodeRect {
            left: 5,
            top: 0,
            right: 14,
            bottom: 1
        })
    );
    let marker = &d.nodes[3];
    assert_eq!(
        marker.key.as_deref(),
        Some("@d-seq:1/d-trk:V1/d-clip:a.mov@1_12s/d-mk:m@1_8s")
    );
    assert_eq!(marker.attrs["media_at"], "1/8s");
    assert_eq!(d.nodes[4].attrs["cut_offset"], "1/24s");
    stacks_last_on_top(&d);
    // the index is a pure function of the bytes
    assert_eq!(d, index(&OtioPack, OTIO.as_bytes(), &limits()).unwrap());
}

#[test]
fn an_fcpxml_file_indexes_its_projects_and_compound_clips() {
    let d = index(&FcpxmlPack, FCPXML.as_bytes(), &limits()).unwrap();
    well_formed(&d);
    assert_eq!(d.source, "fcpxml-1.9");
    let seqs: Vec<(&str, &str)> = d
        .nodes
        .iter()
        .filter(|n| n.kind == "sequence")
        .map(|n| (n.key.as_deref().unwrap(), n.attrs["compound"].as_str()))
        .collect();
    assert_eq!(
        seqs,
        vec![("@project:P1", "false"), ("@compound:C1", "true")]
    );
    let title = d.nodes.iter().find(|n| n.kind == "title").unwrap();
    assert_eq!(title.text.as_deref(), Some("hi"));
    // a lane's tracks are named for their lanes, the lowest first: a later track composites
    // over an earlier one, as in OTIO
    let lanes: Vec<&str> = d
        .nodes
        .iter()
        .filter(|n| n.kind == "track")
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(lanes, vec!["lane -1", "lane 0", "lane 1", "lane 0"]);
    stacks_last_on_top(&d);
}

/// A drop-frame statement at a rate with no drop-frame form is non-drop
/// (`index_support::row_df_at_25_fps_is_non_drop`).
#[test]
fn df_at_25_fps_is_non_drop() {
    if let Err(e) = index_support::row_df_at_25_fps_is_non_drop() {
        panic!("row failed at its step: {e}");
    }
}

/// A sequence's end is the latest end of any item on any track (`end`, with `end_from` =
/// `items`); a duration the file states is carried as `duration`, never used for the end.
#[test]
fn a_sequence_ends_at_its_latest_item_end() {
    let stated = FCPXML.replace(
        r#"tcStart="10s" duration="9s""#,
        r#"tcStart="10s" duration="30s""#,
    );
    assert_ne!(
        stated, FCPXML,
        "control: the project states another duration"
    );
    let d = index(&FcpxmlPack, stated.as_bytes(), &limits()).unwrap();
    // each sequence's key, end, end_from and duration
    let seqs: Vec<[Option<&str>; 4]> = d
        .nodes
        .iter()
        .filter(|n| n.kind == "sequence")
        .map(|n| {
            let attr = |k: &str| n.attrs.get(k).map(String::as_str);
            [
                n.key.as_deref(),
                attr("end"),
                attr("end_from"),
                attr("duration"),
            ]
        })
        .collect();
    assert_eq!(
        seqs,
        vec![
            [Some("@project:P1"), Some("9s"), Some("items"), Some("30s")],
            [Some("@compound:C1"), Some("2s"), Some("items"), Some("2s")],
        ]
    );
}

#[test]
fn a_prproj_file_indexes_with_its_own_ids() {
    let d = index(&PrprojPack, PRPROJ.as_bytes(), &limits()).unwrap();
    well_formed(&d);
    assert_eq!(d.source, "prproj-3");
    let keys: Vec<(&str, Option<&str>)> = d
        .nodes
        .iter()
        .map(|n| (n.kind.as_str(), n.key.as_deref()))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("sequence", Some("@seq:S1")),
            ("track", Some("@track:T1")),
            ("clip", Some("@clip:C1")),
            ("transition", Some("@track:T1/d-tr:@1_2s")),
            ("marker", Some("@mk:M1")),
        ]
    );
    let clip = &d.nodes[2];
    assert!(clip.content.is_some(), "its component chain is carried");
    assert_eq!(d.nodes[0].attrs["rate"], "25/1");
    stacks_last_on_top(&d);
}

#[test]
fn a_repeated_derived_key_is_left_for_the_kit_to_mint() {
    let doubled = OTIO.replacen(
        r#"{"OTIO_SCHEMA":"Transition.1""#,
        r#"{"OTIO_SCHEMA":"Clip.1","name":"a","media_reference":{"OTIO_SCHEMA":"ExternalReference.1","target_url":"a.mov"},
"source_range":{"OTIO_SCHEMA":"TimeRange.1","start_time":{"OTIO_SCHEMA":"RationalTime.1","value":2,"rate":24},"duration":{"OTIO_SCHEMA":"RationalTime.1","value":9,"rate":24}}},
{"OTIO_SCHEMA":"Transition.1""#,
        1,
    );
    let d = index(&OtioPack, doubled.as_bytes(), &limits()).unwrap();
    well_formed(&d);
    let clips: Vec<Option<&str>> = d
        .nodes
        .iter()
        .filter(|n| n.kind == "clip")
        .map(|n| n.key.as_deref())
        .collect();
    assert_eq!(
        clips,
        vec![Some("@d-seq:1/d-trk:V1/d-clip:a.mov@1_12s"), None]
    );
}

#[test]
fn limits_are_refused_typed() {
    let few = IndexLimits::new(3, 64, 64 << 20, INDEX_TEXT_MAX);
    assert!(matches!(
        index(&OtioPack, OTIO.as_bytes(), &few),
        Err(IndexRefused::TooManyNodes { count: 7, limit: 3 })
    ));
    let shallow = IndexLimits::new(100, 1, 64 << 20, INDEX_TEXT_MAX);
    assert!(matches!(
        index(&OtioPack, OTIO.as_bytes(), &shallow),
        Err(IndexRefused::TooDeep { depth: 2, limit: 1 })
    ));
    let short = IndexLimits::new(100, 64, 64 << 20, 1);
    let d = index(&FcpxmlPack, FCPXML.as_bytes(), &short).unwrap();
    let title = d.nodes.iter().find(|n| n.kind == "title").unwrap();
    assert_eq!(title.text.as_deref(), Some("h"));
    assert!(title.flags.contains("text_truncated"));
}

#[test]
fn a_file_the_reader_refuses_is_refused_to_the_kit_without_its_content() {
    let unknown = OTIO.replace("Timeline.1", "Timeline.7");
    assert!(matches!(
        index(&OtioPack, unknown.as_bytes(), &limits()),
        Err(IndexRefused::Unsupported)
    ));
    match index(&OtioPack, b"{\"secret name\"", &limits()) {
        Err(IndexRefused::Unreadable { why }) => assert!(!why.contains("secret")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        index(&FcpxmlPack, b"<other/>", &limits()),
        Err(IndexRefused::Unsupported)
    ));
    assert!(matches!(
        index(&PrprojPack, b"\x1f\x8b\x08junk", &limits()),
        Err(IndexRefused::Unreadable { .. })
    ));
}

#[test]
fn every_pack_names_its_index_and_refuses_agent_writes() {
    for (p, name) in [
        (&OtioPack as &dyn BlobPack, "otio"),
        (&FcpxmlPack, "fcpxml"),
        (&PrprojPack, "prproj"),
    ] {
        let spec = p.index_spec().unwrap();
        assert_eq!((spec.pack, spec.format, spec.verbs), (name, 1, &VERBS));
        assert!(matches!(p.agent_writes(), AgentWrites::Refused));
    }
    assert_eq!(VERBS.list, "clip list");
    assert_eq!(VERBS.export, None);
}
