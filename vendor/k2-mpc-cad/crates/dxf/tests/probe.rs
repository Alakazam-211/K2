//! The structure probe: a drawing's layers, blocks (a dynamic block's references under its own
//! name), tags, linetypes, candidates and `check` findings in counts and names, and never a
//! value: an attribute's value or a text's content can't reach its output. Over a set, the tags
//! whose values pair up across it.

use k2_mpc_dxf::testgen::{Ent, Layer, Spec, Ver};
use k2_mpc_dxf::Limits;

fn drawing() -> Vec<u8> {
    Spec::new(Ver::R2018)
        .layer("TEXT", 7)
        .layer_full(Layer {
            name: "A-ROOMS".into(),
            color: 3,
            frozen: false,
            locked: false,
            ltype: "DASHED".into(),
        })
        .block("DEV", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .block("FL", [0.0, 0.0], vec![Ent::line([0.0, 0.0], [1.0, 0.0])])
        .block("CAM", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .dynamic(
            "*U3",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 0.75)],
            "CAM",
        )
        .block(
            "TITLE",
            [0.0, 0.0],
            vec![Ent::line([0.0, 0.0], [10.0, 0.0])],
        )
        .model(
            Ent::lwpoly(
                &[
                    ([0.0, 0.0], 0.0),
                    ([20.0, 0.0], 0.0),
                    ([20.0, 20.0], 0.0),
                    ([0.0, 20.0], 0.0),
                ],
                true,
            )
            .on("A-ROOMS"),
        )
        .model(Ent::text([2.0, 18.0], 1.0, "CONFIDENTIAL WORDS").on("TEXT"))
        .model(
            Ent::insert("DEV", [5.0, 5.0])
                .attrib("DEVICE", "QZX-917")
                .attrib("NOTE", "PRIVATE-NOTE TEXT"),
        )
        .model(Ent::insert("DEV", [9.0, 5.0]).attrib("DEVICE", "ABCD"))
        .model(Ent::insert("FL", [30.0, 5.0]).attrib("FLAG", "F12"))
        .model(Ent::insert("CAM", [40.0, 5.0]).attrib("CAMID", "CAMVAL1"))
        .model(Ent::insert("*U3", [45.0, 5.0]).attrib("CAMID", "CAMVAL2"))
        .model(Ent::insert("NOPE", [50.0, 5.0]))
        .paper(
            Ent::insert("TITLE", [0.0, 0.0])
                .attrib("SHEET_NO", "7")
                .attrib("REV1", "Z"),
        )
        .build()
        .bytes
}

#[test]
pub fn probe_names_and_counts_never_values() {
    let (out, _) = k2_mpc_dxf::probe("sheet-1.dxf", &drawing()).unwrap();
    let head = out.lines().next().unwrap_or("");
    assert!(
        head.starts_with("sheet sheet-1.dxf version AC1032 ")
            && head.ends_with(" drawn 9 layers 3 blocks 5"),
        "{head}"
    );
    for line in [
        "layer A-ROOMS ents 1 ltype DASHED kinds LWPOLYLINE:1",
        "block DEV refs 2 with-attribs 2 tags DEVICE,NOTE",
        "boundary-candidate layer A-ROOMS ltype DASHED closed 1 with-text-inside 1",
        "title-candidate block TITLE attribs 2 tags SHEET_NO,REV1",
        "tag DEVICE values 2 id-shaped 1 shapes 3-3:1",
        "block CAM refs 2 with-attribs 2 tags CAMID dynamic 1",
        "anonymous-blocks 1 dynamic 1 referenced-unlinked 0",
        "check dxf.check.insert_undefined_block 1",
    ] {
        assert!(
            out.lines().any(|l| l == line),
            "no line {line:?} in:\n{out}"
        );
    }
    assert!(
        !out.lines().any(|l| l.starts_with("flag-candidate ")
            || l.starts_with("block *U3 ")),
        "no flag guessed on one sheet; an anonymous block that draws a dynamic one isn't listed:\n{out}"
    );
    for value in [
        "QZX-917",
        "PRIVATE-NOTE",
        "CONFIDENTIAL",
        "F12",
        "ABCD",
        "CAMVAL",
    ] {
        assert!(!out.contains(value), "a value reached the output: {value}");
    }
    println!(
        "probe_names_and_counts_never_values: {} lines",
        out.lines().count()
    );
}

/// Three sheets: one-kind flags paired by value across them (`CONN`), view numbers and a
/// quantity that repeat on every sheet (`VIEW`, `QTY`), a device on every sheet (`DEVICE`).
fn sheet(conn: &[&str]) -> k2_mpc_cad_model::Doc {
    let mut s = Spec::new(Ver::R2018)
        .block("OPC", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .block("VT", [0.0, 0.0], vec![Ent::line([0.0, 0.0], [4.0, 0.0])])
        .block("ITEM", [0.0, 0.0], vec![Ent::point([0.0, 0.0])])
        .block("DEV", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .model(Ent::insert("DEV", [0.0, 0.0]).attrib("DEVICE", "CAM-01"))
        .model(Ent::insert("ITEM", [0.0, 5.0]).attrib("QTY", "2"))
        .paper(Ent::insert("VT", [0.0, 0.0]).attrib("VIEW", "1"))
        .paper(Ent::insert("VT", [0.0, 10.0]).attrib("VIEW", "2"));
    for (k, v) in conn.iter().enumerate() {
        s = s.model(Ent::insert("OPC", [20.0, k as f64]).attrib("CONN", v));
    }
    k2_mpc_dxf::read_doc(&s.build().bytes, &Limits::default()).unwrap()
}

#[test]
pub fn probe_set_pairs_by_structure() {
    let docs = [sheet(&["7", "K"]), sheet(&["7"]), sheet(&["K"])];
    let set: Vec<(&str, &k2_mpc_cad_model::Doc)> = docs.iter().map(|d| ("s", d)).collect();
    let out = k2_mpc_cad_model::probe::probe_set(&set);
    let pairing: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("pairing-candidate "))
        .collect();
    assert_eq!(
        pairing,
        ["pairing-candidate tag CONN blocks OPC values 2 paired 2 single 0 more 0 id-shaped 0"],
        "the tag whose values pair up across the set; not the views' or the quantity's, which \
         repeat on every sheet:\n{out}"
    );
}
