//! The structure index each pack makes (tier 0): the families and their units, keys from what
//! the file names (the same on every read of the same bytes), counts and bounds, limits, and
//! refusals that carry a code and a section but no file content.

mod support;

use std::collections::BTreeMap;

use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, NodeKey, NodeRect};
use k2_mpc_base::session::{AgentWrites, BlobPack};
use k2_mpc_fab::{DrillPack, PhotoplotPack};
use support::*;

fn index(pack: &dyn BlobPack, bytes: &[u8], l: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    pack.index(&mut &bytes[..], bytes.len() as u64, l)
}

/// The draft keeps the kit's shape: parents earlier, keys in the grammar, each key once.
fn well_formed(d: &IndexDraft) {
    let mut seen = std::collections::BTreeSet::new();
    for (i, n) in d.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            assert!((p as usize) < i, "node {i}: its parent comes later");
        }
        let k = n
            .key
            .as_deref()
            .unwrap_or_else(|| panic!("node {i} has no key"));
        assert!(
            NodeKey::parse(k).is_some(),
            "node {i}: {k} is not a node key"
        );
        assert!(seen.insert(k.to_owned()), "{k} twice");
    }
}

/// Node by key.
fn by_key(d: &IndexDraft) -> BTreeMap<&str, &k2_mpc_base::index::DraftNode> {
    d.nodes
        .iter()
        .filter_map(|n| n.key.as_deref().map(|k| (k, n)))
        .collect()
}

/// The one node of `kind` whose `number` attribute is `n` (an aperture's or a tool's number).
fn numbered<'a>(d: &'a IndexDraft, kind: &str, n: u32) -> &'a k2_mpc_base::index::DraftNode {
    let n = n.to_string();
    let found: Vec<_> = d
        .nodes
        .iter()
        .filter(|x| x.kind == kind && x.attrs.get("number") == Some(&n))
        .collect();
    assert_eq!(found.len(), 1, "one {kind} numbered {n}");
    found[0]
}

fn r(left: i64, top: i64, right: i64, bottom: i64) -> Option<NodeRect> {
    Some(NodeRect {
        left,
        top,
        right,
        bottom,
    })
}

#[test]
fn a_photoplot_indexes_as_families_of_units() {
    let d = index(&PhotoplotPack, BOARD.as_bytes(), &index_limits()).expect("the board indexes");
    well_formed(&d);
    assert_eq!(d.source, "photoplot-2026.05");
    for (k, v) in [
        ("unit", "mm"),
        ("decimals", "6"),
        ("conformance", "current"),
        ("objects", "14"),
        ("expanded_objects", "22"),
        ("apertures", "8"),
        ("polarity", "positive"),
        ("regions", "1"),
        ("clear", "1"),
        ("y_axis", "up"),
    ] {
        assert_eq!(
            d.attrs.get(k).map(String::as_str),
            Some(v),
            "doc attribute {k}"
        );
    }
    let roots: Vec<&str> = d
        .nodes
        .iter()
        .filter(|n| n.parent.is_none())
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(
        roots,
        [
            "apertures",
            "macros",
            "objects",
            "attributes",
            "step-repeats",
            "file-attributes"
        ]
    );
    let k = by_key(&d);
    // an aperture is keyed by its definition, its number an attribute
    let d14 = k["@ap:macro.RoundRect,1.2,0.8,0.2;SMDPad,CuDef"];
    assert_eq!(d14.kind, "aperture");
    assert_eq!(d14.name, "D14");
    assert_eq!(d14.attrs["number"], "14");
    assert_eq!(d14.attrs["template"], "macro");
    assert_eq!(d14.attrs["macro"], "RoundRect");
    assert_eq!(d14.attrs["params"], "1.2,0.8,0.2");
    assert_eq!(d14.attrs["function"], "SMDPad,CuDef");
    assert_eq!(d14.attrs["objects"], "2");
    assert_eq!(d14.bounds, r(4_000_000, 2_000_000, 6_000_000, 2_000_000));
    let d20 = numbered(&d, "aperture", 20);
    assert_eq!(d20.attrs["block_expanded"], "2");
    let d20_key = d20.key.as_deref().unwrap_or("");
    assert!(
        d20_key.starts_with("@ap:block,"),
        "a block by its contents: {d20_key}"
    );
    assert_eq!(k["@macro:RoundRect"].attrs["apertures"], "1");
    // the objects made with D10: two draws and an arc on the layer, a draw in a block
    let d10 = k["@objects:circle,0.25;Conductor"];
    assert_eq!(d10.attrs["aperture"], "10");
    assert_eq!(
        (
            d10.attrs["draws"].as_str(),
            d10.attrs["arcs"].as_str(),
            d10.attrs["in_blocks"].as_str()
        ),
        ("3", "1", "1")
    );
    assert_eq!(d10.bounds, r(0, 0, 10_000_000, 10_000_000));
    assert!(d10.content.is_some());
    let regions = k["@objects:regions"];
    assert_eq!(regions.attrs["contours"], "1");
    assert_eq!(regions.attrs["segments"], "4");
    assert_eq!(regions.bounds, r(20_000_000, 0, 30_000_000, 10_000_000));
    // objects by attribute value
    let gnd = k["@attrval:o..N=GND"];
    assert_eq!(gnd.attrs["objects"], "3");
    assert_eq!(gnd.text.as_deref(), Some("GND"));
    assert_eq!(k["@attrval:o..N=VCC"].attrs["objects"], "1");
    assert_eq!(k["@attrval:o..P=R1,2"].attrs["objects"], "1");
    assert_eq!(k["@attr:a..AperFunction"].attrs["values"], "2");
    assert_eq!(k["@sr:1"].attrs["x"], "2");
    assert_eq!(
        k["@tf:.FileFunction"].text.as_deref(),
        Some("Copper,L1,Top")
    );
    if let Err(e) = row_index_bounds() {
        panic!("{e}");
    }
}

#[test]
fn keys_are_the_same_on_every_read_of_the_same_bytes() {
    let a = index(&PhotoplotPack, BOARD.as_bytes(), &index_limits()).unwrap();
    let b = index(&PhotoplotPack, BOARD.as_bytes(), &index_limits()).unwrap();
    assert_eq!(a, b);
    // and a re-export that renumbers nothing but moves a pad keeps every key
    let moved = BOARD.replace("X8000000Y8000000D03*", "X8100000Y8000000D03*");
    let c = index(&PhotoplotPack, moved.as_bytes(), &index_limits()).unwrap();
    let keys = |d: &IndexDraft| d.nodes.iter().map(|n| n.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&a), keys(&c));
    assert_ne!(
        by_key(&a)["@objects:obround,1.2,0.8;SMDPad,CuDef"].content,
        by_key(&c)["@objects:obround,1.2,0.8;SMDPad,CuDef"].content
    );
}

#[test]
fn a_drill_file_indexes_as_tools_and_their_holes() {
    let d = index(&DrillPack, BOARD_XNC.as_bytes(), &index_limits()).expect("indexes");
    well_formed(&d);
    assert_eq!(d.source, "drill-xnc-2021.11");
    assert_eq!(d.attrs["hits"], "4");
    assert_eq!(d.attrs["routs"], "1");
    let k = by_key(&d);
    // a tool is keyed by its diameter and function, its number an attribute
    let t1 = k["@tool:0.3;ViaDrill"];
    assert_eq!(
        (t1.name.as_str(), t1.attrs["diameter"].as_str()),
        ("T01", "0.3")
    );
    assert_eq!(t1.attrs["number"], "1");
    assert_eq!(t1.attrs["function"], "ViaDrill");
    assert_eq!(k["@hits:0.3;ViaDrill"].attrs["hits"], "2");
    assert_eq!(
        k["@hits:0.3;ViaDrill"].bounds,
        r(1_000_000, 1_000_000, 2_500_000, 1_000_000)
    );
    let t3 = numbered(&d, "tool", 3);
    let t3_id = &t3.key.as_deref().unwrap_or("")["@tool:".len()..];
    let routs = k[format!("@routs:{t3_id}").as_str()];
    assert_eq!(routs.attrs["segments"], "2");
    // the arc's hull: the circle on its chord, (5, 2.6) to (6, 1.6)
    assert_eq!(routs.bounds, r(4_792_893, 1_392_893, 11_000_000, 2_807_107));
    assert_eq!(k["@attrval:o..N=GND"].attrs["objects"], "1");
    assert_eq!(
        k["@attrval:a..AperFunction=ComponentDrill"].attrs["objects"],
        "3"
    );
}

#[test]
fn refusals_carry_the_code_and_section_only() {
    let src = "%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D03*\nM02*\n";
    match index(&PhotoplotPack, src.as_bytes(), &index_limits()) {
        Err(IndexRefused::Unreadable { why }) => {
            assert_eq!(why, "missing-format (photoplot 4.2.2)")
        }
        other => panic!("{other:?}"),
    }
    match index(&DrillPack, b"M48\nFMAT,2\n", &index_limits()) {
        Err(IndexRefused::Unreadable { why }) => {
            assert_eq!(why, "outside-xnc:keyword (XNC 2.7)")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn too_many_nodes_is_the_kits_refusal() {
    let l = IndexLimits::new(5, 64, 64 << 20, 1024);
    match index(&PhotoplotPack, BOARD.as_bytes(), &l) {
        Err(IndexRefused::TooManyNodes { limit: 5, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn agents_never_write_these_files() {
    assert_eq!(PhotoplotPack.agent_writes(), AgentWrites::Refused);
    assert_eq!(DrillPack.agent_writes(), AgentWrites::Refused);
    let s = PhotoplotPack.index_spec().unwrap();
    assert_eq!((s.pack, s.verbs.list), ("photoplot", "feature list"));
    let s = DrillPack.index_spec().unwrap();
    assert_eq!((s.pack, s.verbs.list), ("drill", "hole list"));
}
