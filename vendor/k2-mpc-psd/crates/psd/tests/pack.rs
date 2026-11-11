//! The pack's index of hand-built and generated files: kinds, the tree, keys, hashes, attributes,
//! limits and refusals, and every file checked against the full reader's own tree
//! ([`compare_index_with_full`]). The kit side (minted keys, agent writes) is in `pack_session.rs`.

mod support;

use std::io::{self, Read};

use k2_mpc_base::index::{
    index_to_wire, IndexDraft, IndexLimits, IndexOutcome, IndexRefused, NodeRect, INDEX_NAME_MAX,
    INDEX_TEXT_MAX,
};
use k2_mpc_psd::differential::{compare_index_with_full, Stepped};
use k2_mpc_psd::pack::{FORMAT, PACK, VERBS};
use k2_mpc_psd::{index_psd, PsdPack};
use photocraft_psd::descriptor::{Descriptor, UnicodeString, Value, VersionedDescriptor};
use photocraft_psd::{testgen, GroupSpec, LayerSpec, PixelData, PsdBuilder};
use support::*;

fn limits() -> IndexLimits {
    IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX)
}

fn index_with(bytes: &[u8], limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    index_psd(&mut &bytes[..], limits)
}

fn draft(bytes: &[u8]) -> IndexDraft {
    let d = index_with(bytes, &limits()).expect("check: the file indexes");
    compare_index_with_full(bytes).expect("the index agrees with the full reader's tree");
    d
}

/// `TySh` data holding `text`: version, transform, text version, the text descriptor.
fn tysh(text: &str) -> Vec<u8> {
    let mut d = 1u16.to_be_bytes().to_vec();
    d.extend([0u8; 48]);
    d.extend(50u16.to_be_bytes());
    d.extend(
        VersionedDescriptor::new(
            Descriptor::new("TxLr").with("Txt ", Value::Text(UnicodeString::new(text))),
        )
        .to_bytes(),
    );
    d
}

/// `SoLd` data for the placed item `id`.
fn sold(id: &str) -> Vec<u8> {
    let mut d = b"soLD".to_vec();
    d.extend(4u32.to_be_bytes());
    d.extend(
        VersionedDescriptor::new(
            Descriptor::new("null")
                .with("Idnt", Value::Text(UnicodeString::new(id)))
                .with("Trnf", Value::List(vec![Value::Double(0.0); 8])),
        )
        .to_bytes(),
    );
    d
}

/// Pixel data irrelevant, and the flag that says that bit means something.
const PIXELS_IRRELEVANT: u8 = 8 | 16;

fn names(d: &IndexDraft) -> Vec<&str> {
    d.nodes.iter().map(|n| n.name.as_str()).collect()
}

#[test]
fn the_spec_names_the_pack_its_format_and_verbs() {
    use k2_mpc_base::session::{AgentWrites, BlobPack};
    let spec = PsdPack.index_spec().expect("the pack indexes");
    assert_eq!((spec.pack, spec.format), (PACK, FORMAT));
    assert_eq!(spec.verbs, &VERBS);
    assert_eq!(VERBS.list, "layer list");
    assert_eq!(
        VERBS.export, None,
        "no export verb until the raster exporter exists"
    );
    assert_eq!(
        PsdPack.agent_writes(),
        AgentWrites::Refused,
        "agents never write a PSD file"
    );
    assert!(!PsdPack.can_merge(), "no three-way merge of PSD bytes");
}

#[test]
fn kinds_and_the_tree_parent_first_top_to_bottom() {
    let layers = [
        Lay::end(),
        Lay::new("Pixels").id(1),
        Lay::new("Title").id(2).block(b"TySh", &tysh("Hello")),
        Lay::end(),
        Lay::new("Logo").id(3).block(b"SoLd", &sold("uuid-1")),
        Lay::folder("Inner").id(4),
        Lay::new("Levels").id(5).block(b"levl", &[0; 2]),
        Lay::new("Solid").id(6).block(b"SoCo", &[0; 4]),
        {
            let mut l = Lay::new("Box")
                .id(7)
                .block(b"SoCo", &[0; 4])
                .block(b"vmsk", &[0; 8]);
            l.flags = PIXELS_IRRELEVANT;
            l
        },
        {
            let mut l = Lay::new("Path").id(8).block(b"vogk", &[0; 4]);
            l.flags = PIXELS_IRRELEVANT;
            l
        },
        // vector data, but its pixels count: still raster
        Lay::new("Masked").id(9).block(b"vmsk", &[0; 8]),
        Lay::folder("Outer").id(10),
        Lay::end(),
        Lay::folder("Board").id(11).block(b"artb", &[0; 4]),
        Lay::new("Odd").id(12).block(b"lsct", &be32(7)),
    ];
    let d = draft(&psd_file(false, &layers, &[], &[]));
    assert_eq!(
        names(&d),
        [
            "Odd", "Board", "Outer", "Masked", "Path", "Box", "Solid", "Levels", "Inner", "Logo",
            "Title", "Pixels"
        ]
    );
    let kinds: Vec<&str> = d.nodes.iter().map(|n| n.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "other",
            "artboard",
            "group",
            "raster",
            "shape",
            "shape",
            "fill",
            "adjustment",
            "group",
            "smart",
            "text",
            "raster"
        ]
    );
    let parents: Vec<Option<u32>> = d.nodes.iter().map(|n| n.parent).collect();
    assert_eq!(
        parents,
        [
            None,
            None,
            None,
            Some(2),
            Some(2),
            Some(2),
            Some(2),
            Some(2),
            Some(2),
            Some(8),
            Some(2),
            Some(2)
        ]
    );
    let by = |name: &str| d.nodes.iter().find(|n| n.name == name).unwrap();
    assert_eq!(by("Title").text.as_deref(), Some("Hello"));
    assert_eq!(by("Logo").attrs["placed_id"], "uuid-1");
    assert_eq!(
        by("Logo").weak["placed_id"],
        "uuid-1",
        "a weak ID for anchors"
    );
    assert_eq!(by("Levels").attrs["adjustment_key"], "levl");
    assert_eq!(by("Solid").attrs["fill_key"], "SoCo");
    assert_eq!(
        by("Box").attrs["fill_key"],
        "SoCo",
        "a shape keeps its fill"
    );
    assert!(by("Box").flags.contains("vector_mask"));
    assert!(by("Masked").flags.contains("vector_mask"));
    assert!(
        !by("Path").flags.contains("vector_mask"),
        "vogk is not a mask"
    );
    assert!(d.nodes.iter().all(|n| !n.flags.contains("unreadable")));
    for n in &d.nodes {
        if matches!(n.kind.as_str(), "group" | "artboard") {
            assert!(n.content.is_none(), "{}: a group has no content", n.name);
            assert!(n.bounds.is_none(), "{}: nor bounds", n.name);
        }
    }
    assert!(by("Pixels").content.is_some());
}

#[test]
fn a_shape_needs_vector_data_and_pixels_marked_irrelevant() {
    let fill_vector = || {
        Lay::new("x")
            .block(b"SoCo", &[0; 4])
            .block(b"vmsk", &[0; 8])
    };
    let kind_of = |l: Lay| {
        draft(&psd_file(false, &[l], &[], &[])).nodes[0]
            .kind
            .clone()
    };
    assert_eq!(kind_of(fill_vector()), "fill", "its pixels count: a fill");
    let mut l = fill_vector();
    l.flags = PIXELS_IRRELEVANT;
    assert_eq!(kind_of(l), "shape");
    let mut l = Lay::new("x").block(b"SoCo", &[0; 4]);
    l.flags = PIXELS_IRRELEVANT;
    assert_eq!(kind_of(l), "fill", "no vector data: a fill");
    let mut l = Lay::new("x").block(b"vsms", &[0; 8]);
    l.flags = PIXELS_IRRELEVANT;
    assert_eq!(kind_of(l), "shape", "vector data and no fill block");
}

#[test]
fn keys_are_the_files_own_ids_never_invented_never_dropped() {
    let layers = [
        Lay::new("A").id(5),
        Lay::new("B"),
        Lay::new("C").id(5),
        Lay::new("D").id(0),
        // a malformed lyid gives no key, and marks the node
        Lay::new("E").block(b"lyid", &[0, 1]),
    ];
    let d = draft(&psd_file(false, &layers, &[], &[]));
    let keys: Vec<Option<&str>> = d.nodes.iter().map(|n| n.key.as_deref()).collect();
    assert_eq!(
        keys,
        [
            None,
            Some("@lyid:0"),
            Some("@lyid:5"),
            None,
            Some("@lyid:5")
        ],
        "top to bottom: E, D, C, B, A"
    );
    assert!(d.nodes[0].flags.contains("unreadable"));
    assert!(
        !d.nodes[3].flags.contains("unreadable"),
        "no ID is not malformed"
    );
}

#[test]
fn a_group_without_its_marker_is_empty_and_an_unclosed_marker_hoists() {
    let d = draft(&psd_file(
        false,
        &[Lay::new("a"), Lay::folder("F")],
        &[],
        &[],
    ));
    assert_eq!(names(&d), ["F", "a"]);
    assert_eq!(d.nodes[1].parent, None, "a is no child of F");
    let d = draft(&psd_file(
        false,
        &[Lay::end(), Lay::new("b"), Lay::end(), Lay::new("c")],
        &[],
        &[],
    ));
    assert_eq!(names(&d), ["c", "b"]);
    assert!(d.nodes.iter().all(|n| n.parent.is_none()));
}

#[test]
fn record_and_content_digests_change_only_with_what_they_cover() {
    let one = |l: Lay| index_with(&psd_file(false, &[l], &[], &[]), &limits()).unwrap();
    let base = Lay::new("A").id(1).pixels(&[1, 2, 3]);
    let a = one(base.clone());
    let again = one(base.clone());
    assert_eq!(a, again, "the same bytes, the same draft");
    let a = &a.nodes[0];

    let px = one(base.clone().pixels(&[1, 2, 4]));
    assert_eq!(
        px.nodes[0].record, a.record,
        "a pixel change is not a record change"
    );
    assert_ne!(px.nodes[0].content, a.content, "it is a content change");

    let longer = one(base.clone().pixels(&[1, 2, 3, 4]));
    assert_eq!(
        longer.nodes[0].record, a.record,
        "a different stored length alone leaves the record digest"
    );
    assert_ne!(longer.nodes[0].content, a.content);

    let renamed = one(Lay {
        name: "B".into(),
        ..base.clone()
    });
    assert_ne!(
        renamed.nodes[0].record, a.record,
        "a rename changes the record"
    );
    assert_eq!(renamed.nodes[0].content, a.content, "and not the content");

    let moved = one(base.clone().at([0, 1, 1, 2]));
    assert_ne!(moved.nodes[0].record, a.record, "a move changes the record");

    let hidden = one(base.clone().hidden());
    assert_ne!(hidden.nodes[0].record, a.record);
    assert_eq!(hidden.nodes[0].content, a.content);
}

#[test]
fn visibility_bounds_and_attributes() {
    let mut clipped = Lay::new("Clip")
        .at([10, 20, 30, 50])
        .block(b"iOpa", &[200])
        .block(b"lspf", &be32(0b101));
    clipped.opacity = 128;
    clipped.clipping = 1;
    let layers = [
        Lay::end(),
        clipped,
        Lay::new("Flat").at([0, 0, 0, 0]),
        Lay::folder("G").hidden(),
    ];
    let d = draft(&psd_file(false, &layers, &[], &[]));
    assert_eq!(names(&d), ["G", "Flat", "Clip"]);
    let (g, flat, clip) = (&d.nodes[0], &d.nodes[1], &d.nodes[2]);
    assert!(!g.visible, "the group's own flag");
    assert!(
        clip.visible,
        "a child's own flag: the kit derives the tree's"
    );
    assert_eq!(
        clip.bounds,
        Some(NodeRect {
            left: 20,
            top: 10,
            right: 50,
            bottom: 30
        })
    );
    assert_eq!(flat.bounds, None, "an empty rectangle has no bounds");
    assert_eq!(clip.attrs["opacity"], "128");
    assert_eq!(clip.attrs["fill_opacity"], "200");
    assert_eq!(clip.attrs["clipped"], "true");
    assert_eq!(clip.attrs["locks"], "transparent,position");
    assert_eq!(clip.attrs["blend"], "norm");
    assert_eq!(flat.attrs["fill_opacity"], "255");
    assert_eq!(flat.attrs["clipped"], "false");
    assert_eq!(flat.attrs["locks"], "none");

    // a group's blend is its divider's
    let mut folder = Lay::new("P");
    folder.rect = [0, 0, 0, 0];
    folder.channels = vec![(0, Vec::new())];
    let folder = folder.block(b"lsct", b"\0\0\0\x018BIMpass");
    let d = draft(&psd_file(
        false,
        &[Lay::end(), Lay::new("x"), folder],
        &[],
        &[],
    ));
    assert_eq!(d.nodes[0].attrs["blend"], "pass");
    assert_eq!(d.nodes[1].attrs["blend"], "norm");
}

#[test]
fn text_and_names_are_cut_to_the_kits_limits() {
    let long_text = "Hello, world";
    let small = IndexLimits::new(100, 64, 1 << 20, 5);
    let f = psd_file(
        false,
        &[Lay::new("T").block(b"TySh", &tysh(long_text))],
        &[],
        &[],
    );
    let d = index_with(&f, &small).unwrap();
    assert_eq!(d.nodes[0].text.as_deref(), Some("Hello"));
    assert!(d.nodes[0].flags.contains("text_truncated"));
    let d = index_with(&f, &limits()).unwrap();
    assert_eq!(d.nodes[0].text.as_deref(), Some(long_text));
    assert!(!d.nodes[0].flags.contains("text_truncated"));

    let long_name = "é".repeat(3000);
    let d = draft(&psd_file(
        false,
        &[Lay::new("x").block(b"luni", &luni(&long_name))],
        &[],
        &[],
    ));
    assert!(d.nodes[0].name.len() <= INDEX_NAME_MAX);
    assert!(long_name.starts_with(&d.nodes[0].name));
    assert!(d.nodes[0].flags.contains("name_truncated"));

    // a malformed type block: no text, marked
    let d = draft(&psd_file(
        false,
        &[Lay::new("T").block(b"TySh", &[0; 10])],
        &[],
        &[],
    ));
    assert_eq!(d.nodes[0].kind, "text");
    assert_eq!(d.nodes[0].text, None);
    assert!(d.nodes[0].flags.contains("unreadable"));
}

/// One linked-file item: length, type, version, Pascal ID, file name, then some data.
fn link_item(kind: &[u8; 4], id: &str, name: &str) -> Vec<u8> {
    let mut b = kind.to_vec();
    b.extend(7u32.to_be_bytes());
    b.push(id.len() as u8);
    b.extend_from_slice(id.as_bytes());
    let units: Vec<u16> = name.encode_utf16().collect();
    b.extend((units.len() as u32).to_be_bytes());
    for u in units {
        b.extend(u.to_be_bytes());
    }
    b.extend(b"8BPS????");
    b.extend(5u64.to_be_bytes());
    b.extend([1, 2, 3, 4, 5]);
    let mut out = (b.len() as u64).to_be_bytes().to_vec();
    out.extend(b);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    out
}

#[test]
fn a_smart_objects_source_comes_from_the_linked_file_blocks() {
    let mut lnk = link_item(b"liFD", "uuid-1", "inside.psb");
    lnk.extend(link_item(b"liFE", "uuid-2", "C:\\work\\private\\pic.png"));
    let globals = block(b"lnk2", &lnk, &[]);
    let layers = [
        Lay::new("In").block(b"SoLd", &sold("uuid-1")),
        Lay::new("Out").block(b"SoLd", &sold("uuid-2")),
        Lay::new("Gone").block(b"SoLd", &sold("uuid-3")),
    ];
    let d = draft(&psd_file(false, &layers, &globals, &[]));
    assert_eq!(names(&d), ["Gone", "Out", "In"]);
    assert_eq!(d.nodes[2].attrs["smart_source"], "embedded");
    assert_eq!(
        d.nodes[1].attrs["smart_source"], "linked:pic.png",
        "the file name, never its path"
    );
    assert!(!d.nodes[0].attrs.contains_key("smart_source"), "not listed");
    assert_eq!(d.nodes[0].attrs["placed_id"], "uuid-3");
}

#[test]
fn doc_attributes_and_the_format_stamp() {
    let res = resource(1044, b"", &be32(77));
    let d = draft(&psd_file(false, &[Lay::new("a")], &[], &res));
    assert_eq!(d.source, "psd-1");
    assert_eq!(d.attrs["layer_id_seed"], "77");
    assert_eq!(d.attrs["width"], "1");
    assert_eq!(d.attrs["depth"], "8");
    let d = draft(&psd_file(true, &[Lay::new("a")], &[], &[]));
    assert_eq!(d.source, "psb-2");
    assert!(!d.attrs.contains_key("layer_id_seed"));
    // a flattened file: no nodes
    let d = draft(&psd_file(false, &[], &[], &[]));
    assert!(d.nodes.is_empty());
}

#[test]
fn a_psb_reads_to_the_same_tree() {
    let layers = [
        Lay::end(),
        Lay::new("child").id(2).pixels(&[9; 40]),
        Lay::folder("group").id(1),
    ];
    let psd = draft(&psd_file(false, &layers, &[], &[]));
    let psb = draft(&psd_file(true, &layers, &[], &[]));
    assert_eq!(names(&psd), names(&psb));
    for (a, b) in psd.nodes.iter().zip(&psb.nodes) {
        assert_eq!((&a.key, a.parent, &a.kind), (&b.key, b.parent, &b.kind));
        assert_eq!(a.content, b.content, "the same channel bytes");
    }
}

#[test]
fn node_and_depth_caps_refuse_before_anything_is_built() {
    let three = psd_file(
        false,
        &[Lay::new("a"), Lay::new("b"), Lay::new("c")],
        &[],
        &[],
    );
    let cap = IndexLimits::new(2, 64, 1 << 20, 64);
    match index_with(&three, &cap) {
        Err(IndexRefused::TooManyNodes { count: 3, limit: 2 }) => {}
        other => panic!("expected TooManyNodes, got {other:?}"),
    }
    // a group's closing marker is not a node
    let group = psd_file(
        false,
        &[Lay::end(), Lay::new("a"), Lay::folder("G")],
        &[],
        &[],
    );
    assert_eq!(index_with(&group, &cap).unwrap().nodes.len(), 2);

    let nested = psd_file(
        false,
        &[
            Lay::end(),
            Lay::end(),
            Lay::new("deep"),
            Lay::folder("g2"),
            Lay::folder("g1"),
        ],
        &[],
        &[],
    );
    match index_with(&nested, &IndexLimits::new(100, 1, 1 << 20, 64)) {
        Err(IndexRefused::TooDeep { depth: 2, limit: 1 }) => {}
        other => panic!("expected TooDeep, got {other:?}"),
    }
    assert_eq!(
        index_with(&nested, &IndexLimits::new(100, 2, 1 << 20, 64))
            .unwrap()
            .nodes
            .len(),
        3
    );
}

struct Failing;
impl Read for Failing {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("the blob store failed"))
    }
}

#[test]
fn what_is_not_a_psd_and_what_is_broken_are_told_apart() {
    for not in [&b""[..], b"\x89PNG\r\n\x1a\n", b"8BP"] {
        assert!(
            matches!(index_with(not, &limits()), Err(IndexRefused::Unsupported)),
            "{not:?}"
        );
    }
    let f = psd_file(false, &[Lay::new("a").pixels(&[1; 100])], &[], &[]);
    for cut in [30, f.len() / 2, f.len() - 110] {
        match index_with(&f[..cut], &limits()) {
            Err(IndexRefused::Unreadable { why }) => assert!(!why.is_empty()),
            other => panic!("cut at {cut}: expected Unreadable, got {other:?}"),
        }
    }
    match index_psd(&mut Failing, &limits()) {
        Err(IndexRefused::Read(_)) => {}
        other => panic!("a failing stream is a read error, not a bad file: {other:?}"),
    }
    let mut late = (&f[..40]).chain(Failing);
    match index_psd(&mut late, &limits()) {
        Err(IndexRefused::Read(_)) => {}
        other => panic!("a stream failing midway is a read error: {other:?}"),
    }
}

#[test]
fn the_draft_does_not_depend_on_how_the_stream_is_split() {
    let mut lnk = link_item(b"liFE", "uuid-2", "pic.png");
    lnk.extend(link_item(b"liFD", "uuid-1", "x"));
    let f = psd_file(
        false,
        &[
            Lay::end(),
            Lay::new("a").id(1).pixels(&[3; 300]),
            Lay::new("s").block(b"SoLd", &sold("uuid-2")),
            Lay::folder("g").id(2),
        ],
        &block(b"lnk2", &lnk, &[]),
        &resource(1044, b"", &be32(9)),
    );
    let whole = index_with(&f, &limits()).unwrap();
    assert_eq!(names(&whole), ["g", "s", "a"]);
    assert_eq!(whole.nodes[1].attrs["smart_source"], "linked:pic.png");
    for step in [1, 2, 7, 64, 4096] {
        let split = index_psd(&mut Stepped { data: &f, step }, &limits()).unwrap();
        assert_eq!(split, whole, "step {step}");
    }
}

#[test]
fn every_draft_passes_the_kits_wire_unchanged() {
    let f = psd_file(
        false,
        &[
            Lay::end(),
            Lay::new("Title").id(2).block(b"TySh", &tysh("Hello")),
            Lay::new("Logo").id(3).block(b"SoLd", &sold("u")),
            Lay::folder("G").id(1).block(b"artb", &[0; 4]),
        ],
        &[],
        &[],
    );
    let l = limits();
    let direct = index_with(&f, &l).unwrap();
    let wire = index_to_wire(&PsdPack, &mut &f[..], f.len() as u64, &l)
        .expect("the kit accepts the pack's draft");
    match IndexOutcome::from_wire(&wire, &l).unwrap() {
        IndexOutcome::Draft(d) => assert_eq!(d, direct),
        other => panic!("expected a draft, got {other:?}"),
    }
}

#[test]
fn generated_files_agree_with_the_full_reader() {
    let mut checked = 0;
    for case in testgen::all_cases() {
        let bytes = case
            .file
            .to_bytes()
            .expect("check: the generated file writes");
        compare_index_with_full(&bytes).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        checked += 1;
    }
    assert!(checked > 50, "{checked} generated cases");

    // nested groups from the builder
    let mut b = PsdBuilder::new(4, 3);
    b.push_layer(LayerSpec::new(
        "bottom",
        0,
        0,
        2,
        2,
        PixelData::Rgba8(vec![9; 16]),
    ));
    b.begin_group(GroupSpec::new("outer"));
    b.push_layer(LayerSpec::new(
        "in-1",
        1,
        1,
        1,
        1,
        PixelData::Rgba8(vec![1; 4]),
    ));
    b.begin_group(GroupSpec::new("inner"));
    b.push_layer(LayerSpec::new(
        "in-2",
        0,
        0,
        4,
        3,
        PixelData::Rgba8(vec![2; 48]),
    ));
    b.end_group().unwrap();
    b.end_group().unwrap();
    b.composite(PixelData::Rgba8(vec![0; 48]));
    let bytes = b.to_bytes().unwrap();
    let d = draft(&bytes);
    assert_eq!(names(&d), ["outer", "inner", "in-2", "in-1", "bottom"]);
    assert_eq!(d.nodes[2].parent, Some(1));
    assert_eq!(d.nodes[3].parent, Some(0));
}
