//! The generic reads: block attributes by tag (typed, each with its own handle), one block
//! reference's attributes, the effective linetype, entities inside or crossing a closed
//! polyline, and texts near an entity. Known answers on one drawing made for them.

mod support;

use k2_mpc_dxf::testgen::{Built, Ent, Layer, Spec, Ver};
use k2_mpc_dxf::Limits;
use support::*;

fn drawing() -> Built {
    Spec::new(Ver::R2018)
        .layer("WALLS", 1)
        .layer_full(Layer {
            name: "BOUNDS".into(),
            color: 3,
            frozen: false,
            locked: false,
            ltype: "DASHED".into(),
        })
        .block(
            "DEV",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 0.5).on("WALLS")],
        )
        // a square room and an L-shaped one
        .model(
            Ent::lwpoly(
                &[
                    ([0.0, 0.0], 0.0),
                    ([10.0, 0.0], 0.0),
                    ([10.0, 10.0], 0.0),
                    ([0.0, 10.0], 0.0),
                ],
                true,
            )
            .on("BOUNDS")
            .label("room"),
        )
        .model(
            Ent::lwpoly(
                &[
                    ([20.0, 0.0], 0.0),
                    ([30.0, 0.0], 0.0),
                    ([30.0, 4.0], 0.0),
                    ([24.0, 4.0], 0.0),
                    ([24.0, 10.0], 0.0),
                    ([20.0, 10.0], 0.0),
                ],
                true,
            )
            .on("WALLS")
            .label("ell"),
        )
        .model(Ent::line([2.0, 2.0], [8.0, 8.0]).on("WALLS").label("in"))
        .model(
            Ent::line([2.0, 8.0], [8.0, 2.0])
                .on("WALLS")
                .label("infall"),
        )
        .model(
            Ent::line([5.0, 5.0], [15.0, 5.0])
                .on("WALLS")
                .label("cross"),
        )
        .model(
            Ent::line([10.0, 5.0], [15.0, 5.0])
                .on("WALLS")
                .label("touch"),
        )
        .model(
            Ent::line([40.0, 40.0], [45.0, 45.0])
                .on("WALLS")
                .label("out"),
        )
        // falling, near the square's corner without touching it
        .model(
            Ent::line([9.0, 13.0], [13.0, 9.0])
                .on("WALLS")
                .label("fallnear"),
        )
        // in the L's notch: inside its box, outside the L
        .model(
            Ent::line([26.0, 6.0], [29.0, 9.0])
                .on("WALLS")
                .label("notch"),
        )
        .model(
            Ent::line([100.0, 0.0], [110.0, 0.0])
                .on("WALLS")
                .label("base"),
        )
        .model(Ent::text([101.0, 1.0], 0.5, "T1").on("WALLS").label("t1"))
        .model(Ent::text([101.0, 5.0], 0.5, "T2").on("WALLS").label("t2"))
        .model(
            Ent::line([200.0, 0.0], [201.0, 0.0])
                .on("BOUNDS")
                .label("bylayer"),
        )
        .model(
            Ent::line([200.0, 2.0], [201.0, 2.0])
                .on("WALLS")
                .ltype("CENTER")
                .label("center"),
        )
        .model(
            Ent::line([200.0, 4.0], [201.0, 4.0])
                .on("WALLS")
                .ltype("BYBLOCK")
                .label("byblock"),
        )
        .model(
            Ent::insert("DEV", [300.0, 0.0])
                .on("WALLS")
                .attrib("DEVICE", "CAM-01")
                .attrib("ROOM", "101")
                .label("dev1"),
        )
        .model(
            Ent::insert("DEV", [310.0, 0.0])
                .on("WALLS")
                .attrib("DEVICE", "TX-01")
                .label("dev2"),
        )
        .build()
}

fn found(b: &Built, args: &[&str]) -> Vec<String> {
    let o = run(&b.bytes, "entity find", args);
    let mut v: Vec<String> = list(&o.data, &["rows"])
        .iter()
        .map(|r| string(r, &["address"]).to_owned())
        .collect();
    v.sort();
    v
}

fn addrs(b: &Built, labels: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = labels.iter().map(|l| b.addr(l)).collect();
    v.sort();
    v
}

#[test]
pub fn inside_and_crossing() {
    let b = drawing();
    assert_eq!(
        found(&b, &["--inside", &b.addr("room")]),
        addrs(&b, &["in", "infall"])
    );
    assert_eq!(
        found(&b, &["--crosses", &b.addr("room")]),
        addrs(&b, &["cross", "touch"])
    );
    assert!(
        found(&b, &["--inside", &b.addr("ell")]).is_empty(),
        "the notch is outside the L"
    );
    assert!(found(&b, &["--crosses", &b.addr("ell")]).is_empty());
    let e = refused(&b.bytes, "entity find", &["--inside", &b.addr("in")]);
    assert_eq!(e.code.id, "CAD-E012", "a line isn't a boundary: {e}");
}

#[test]
pub fn attrib_find_typed() {
    let b = drawing();
    let o = run(&b.bytes, "attrib find", &["--tag", "device"]);
    assert_eq!(o.spec, ("cad.attrib-find", 1));
    let rows = list(&o.data, &["rows"]);
    let values: Vec<&str> = rows.iter().map(|r| string(r, &["value"])).collect();
    assert_eq!(values, ["CAM-01", "TX-01"]);
    // each attribute by its own handle: the ATTRIB's, not its block reference's
    let limits = Limits::default();
    let s = k2_mpc_dxf::structure::walk(
        k2_mpc_dxf::token::tokenize(&b.bytes, &limits).unwrap(),
        &limits,
    )
    .unwrap();
    let dev1 = s
        .entities
        .iter()
        .find(|e| {
            (e.head.first..e.head.end).any(|i| {
                s.tok.pairs[i].code == 5
                    && k2_mpc_dxf::handle::parse(s.tok.value(i)) == Some(b.handle("dev1"))
            })
        })
        .unwrap();
    let f = &dev1.followers[0];
    let own = (f.first..f.end)
        .find(|&i| s.tok.pairs[i].code == 5)
        .and_then(|i| k2_mpc_dxf::handle::parse(s.tok.value(i)))
        .unwrap();
    assert_eq!(string(&rows[0], &["attrib"]), k2_mpc_dxf::handle::key(own));
    assert_eq!(string(&rows[0], &["insert"]), b.addr("dev1"));
    assert_eq!(string(&rows[0], &["block"]), "DEV");
    let o = run(&b.bytes, "attrib find", &["--value", "CAM-01"]);
    assert_eq!(list(&o.data, &["rows"]).len(), 1);
    let e = refused(&b.bytes, "attrib find", &[]);
    assert_eq!(e.code.id, "CAD-E004");
}

#[test]
pub fn block_show_attribs() {
    let b = drawing();
    let o = run(&b.bytes, "block show", &[&b.addr("dev1"), "--attribs"]);
    assert_eq!(o.spec, ("cad.block-show", 1));
    let tags: Vec<&str> = list(&o.data, &["attribs"])
        .iter()
        .map(|r| string(r, &["tag"]))
        .collect();
    assert_eq!(tags, ["DEVICE", "ROOM"], "attributes in the file's order");
    assert_eq!(string(&o.data, &["block"]), "DEV");
    let e = refused(&b.bytes, "block show", &[&b.addr("in")]);
    assert_eq!(e.code.id, "CAD-E010", "a line isn't a block reference");
}

#[test]
pub fn ltype_resolved() {
    let b = drawing();
    let o = run(
        &b.bytes,
        "entity props",
        &[
            &b.addr("bylayer"),
            &b.addr("center"),
            &b.addr("byblock"),
            &b.addr("in"),
        ],
    );
    assert_eq!(o.spec, ("cad.entity-props", 1));
    let eff: Vec<&str> = list(&o.data, &["rows"])
        .iter()
        .map(|r| string(r, &["ltype_eff"]))
        .collect();
    assert_eq!(eff, ["DASHED", "CENTER", "CONTINUOUS", "CONTINUOUS"]);
    assert_eq!(
        found(&b, &["--ltype", "dashed"]),
        addrs(&b, &["room", "bylayer"])
    );
}

#[test]
pub fn text_near() {
    let b = drawing();
    let near = |d: &str| {
        let o = run(
            &b.bytes,
            "text find",
            &["--contains", "t", "--near", &b.addr("base"), "--within", d],
        );
        let mut v: Vec<String> = list(&o.data, &["rows"])
            .iter()
            .map(|r| string(r, &["address"]).to_owned())
            .collect();
        v.sort();
        v
    };
    assert_eq!(near("2"), addrs(&b, &["t1"]));
    assert_eq!(near("10"), addrs(&b, &["t1", "t2"]));
    let e = refused(
        &b.bytes,
        "text find",
        &["--contains", "t", "--near", &b.addr("base")],
    );
    assert_eq!(e.code.id, "CAD-E004", "--near without --within");
}
