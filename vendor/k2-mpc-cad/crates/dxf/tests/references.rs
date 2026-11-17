//! Block references and leaders as a reader resolves them: a dynamic block's reference by the
//! dynamic block's own name (its anonymous block beside it), an external reference's path, a
//! block reference's geometry as placed (scaled and rotated), and a leader's points (its arrow
//! first) with its text. Known answers on drawings made for them.

mod support;

use k2_mpc_dxf::testgen::{Built, Ent, Spec, Ver};
use k2_mpc_dxf::Limits;
use support::*;

fn doc(b: &Built) -> k2_mpc_cad_model::Doc {
    k2_mpc_dxf::read_doc(&b.bytes, &Limits::default()).expect("our drawing reads")
}

fn entity<'d>(
    d: &'d k2_mpc_cad_model::Doc,
    b: &Built,
    label: &str,
) -> (usize, &'d k2_mpc_cad_model::Entity) {
    let i = d
        .entity_by_handle(b.handle(label))
        .unwrap_or_else(|| panic!("no entity {label}"));
    (i, &d.entities[i])
}

/// A dynamic block `CAMERA` placed twice: once by its own name, once through the anonymous
/// block `*U5` that draws it as set; and `*U9`, an anonymous block that stands for no other.
fn dynamic() -> Built {
    Spec::new(Ver::R2018)
        .block("CAMERA", [0.0, 0.0], vec![Ent::circle([0.0, 0.0], 0.5)])
        .dynamic(
            "*U5",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 0.75)],
            "CAMERA",
        )
        .dynamic("*U9", [0.0, 0.0], vec![Ent::point([0.0, 0.0])], "NO-SUCH")
        .xref("BACKDROP", "../refs/backdrop.dxf")
        .model(
            Ent::insert("CAMERA", [0.0, 0.0])
                .attrib("DEVICE", "CAM-01")
                .label("own"),
        )
        .model(
            Ent::insert("*U5", [10.0, 0.0])
                .attrib("DEVICE", "CAM-02")
                .label("rep"),
        )
        .model(
            Ent::insert("*U9", [20.0, 0.0])
                .attrib("NOTE", "N1")
                .label("other"),
        )
        .model(Ent::insert("BACKDROP", [0.0, 0.0]).label("xref"))
        .build()
}

#[test]
pub fn dynamic_blocks_by_real_name() {
    let b = dynamic();
    let o = run(&b.bytes, "attrib find", &["--tag", "DEVICE"]);
    let rows = list(&o.data, &["rows"]);
    let got: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|r| {
            (
                string(r, &["insert"]),
                string(r, &["block"]),
                string(r, &["anon_block"]),
            )
        })
        .collect();
    let (own, rep) = (b.addr("own"), b.addr("rep"));
    assert_eq!(
        got,
        [
            (own.as_str(), "CAMERA", ""),
            (rep.as_str(), "CAMERA", "*U5")
        ],
        "a dynamic block's reference goes by its real name: {}",
        o.data.to_json()
    );
    let by = |name: &str| run(&b.bytes, "attrib find", &["--block", name]).data;
    assert_eq!(
        number(&by("CAMERA"), &["total"]),
        2.0,
        "--block, the real name"
    );
    assert_eq!(
        number(&by("*U5"), &["total"]),
        1.0,
        "--block, the anonymous one"
    );
    let other = run(&b.bytes, "block show", &[b.addr("other").as_str()]).data;
    assert_eq!(
        (string(&other, &["block"]), at(&other, &["anon_block"])),
        ("*U9", &k2_mpc_base::errors::Value::Null),
        "an anonymous block that names no dynamic block keeps its own name"
    );
    let shown = run(&b.bytes, "block show", &[rep.as_str()]).data;
    assert_eq!(
        (string(&shown, &["block"]), string(&shown, &["anon_block"])),
        ("CAMERA", "*U5")
    );
    let blocks = run(&b.bytes, "block list", &["--anon"]).data;
    let rows = list(&blocks, &["blocks"]);
    assert_eq!(string(row(rows, "name", "*U5"), &["dynamic_of"]), "CAMERA");
    assert_eq!(
        number(row(rows, "name", "CAMERA"), &["inserts", "dynamic"]),
        1.0
    );
    println!("dynamic_blocks_by_real_name: {} attribute rows", got.len());
}

#[test]
pub fn xref_paths() {
    let b = dynamic();
    let d = doc(&b);
    assert_eq!(d.xrefs, ["../refs/backdrop.dxf"]);
    let blocks = run(&b.bytes, "block list", &[]).data;
    let x = row(list(&blocks, &["blocks"]), "name", "BACKDROP");
    assert_eq!(
        (at(x, &["xref"]), string(x, &["path"])),
        (
            &k2_mpc_base::errors::Value::Bool(true),
            "../refs/backdrop.dxf"
        )
    );
}

/// A straight run drawn as a block: `RUN` is two parallel lines one unit long, placed scaled
/// and rotated.
fn runs() -> Built {
    Spec::new(Ver::R2018)
        .block(
            "RUN",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, -0.1], [1.0, -0.1]),
                Ent::line([0.0, 0.1], [1.0, 0.1]),
            ],
        )
        .model(
            Ent::insert("RUN", [10.0, 10.0])
                .placed(20.0, 1.0, 90.0)
                .attrib("SIZE", "1")
                .label("up"),
        )
        .model(
            Ent::insert("RUN", [0.0, 0.0])
                .placed(5.0, 2.0, 0.0)
                .label("flat"),
        )
        .model(Ent::insert("RUN", [50.0, 0.0]).label("plain"))
        .build()
}

/// An attribute definition: its tag and default text at `at` (a block's template).
fn attdef(at: [f64; 2], tag: &str) -> Ent {
    let (x, y) = (
        k2_mpc_dxf::testgen::num(at[0]),
        k2_mpc_dxf::testgen::num(at[1]),
    );
    Ent::raw(
        "ATTDEF",
        &[
            (100, "AcDbText"),
            (10, x.as_str()),
            (20, y.as_str()),
            (30, "0.0"),
            (40, "0.25"),
            (1, ""),
            (100, "AcDbAttributeDefinition"),
            (3, tag),
            (2, tag),
            (70, "0"),
        ],
    )
}

#[test]
pub fn placed_axis() {
    let b = runs();
    let d = doc(&b);
    let near = |a: [[f64; 2]; 2], w: [[f64; 2]; 2], what: &str| {
        let off = (0..2)
            .flat_map(|k| (0..2).map(move |c| (k, c)))
            .map(|(k, c)| (a[k][c] - w[k][c]).abs())
            .fold(0.0f64, f64::max);
        assert!(off < 1e-9, "{what}: {a:?}, not {w:?}");
    };
    let (up, e) = entity(&d, &b, "up");
    assert_eq!(
        e.extra.as_ref().and_then(|x| x.xform),
        Some([20.0, 1.0, 90.0])
    );
    near(
        d.placed_axis(up).expect("an axis"),
        [[10.0, 10.0], [10.0, 30.0]],
        "scaled 20 along x, turned a quarter",
    );
    let (flat, _) = entity(&d, &b, "flat");
    near(
        d.placed_axis(flat).expect("an axis"),
        [[0.0, 0.0], [5.0, 0.0]],
        "scaled 5",
    );
    let (plain, e) = entity(&d, &b, "plain");
    assert!(
        e.extra.is_none(),
        "no scale or rotation stated: nothing kept"
    );
    near(
        d.placed_axis(plain).expect("an axis"),
        [[50.0, 0.0], [51.0, 0.0]],
        "as drawn",
    );
}

/// A leader to a multiline text (by handle), and a multileader with its own text.
fn leaders() -> Built {
    Spec::new(Ver::R2018)
        .model(
            Ent::mtext([16.0, 16.0], 0.5, "C-301")
                .handle(0x500)
                .label("note"),
        )
        .model(Ent::leader(&[[5.0, 5.0], [15.0, 15.0]], Some(0x500)).label("leader"))
        .model(
            Ent::mleader(
                &[[10.0, 5.0], [20.0, 20.0]],
                [28.0, 28.0],
                [30.0, 30.0],
                "C-201",
            )
            .label("ml"),
        )
        .build()
}

#[test]
pub fn leaders_arrow_first() {
    let b = leaders();
    let d = doc(&b);
    let (_, l) = entity(&d, &b, "leader");
    let x = l.extra.as_ref().expect("a leader's points");
    assert_eq!(
        (x.verts.as_slice(), x.annotation),
        (&[[5.0, 5.0], [15.0, 15.0]][..], Some(0x500)),
        "a leader: its points, the arrow first, and its text's handle"
    );
    let (_, m) = entity(&d, &b, "ml");
    let x = m.extra.as_ref().expect("a multileader's points");
    assert_eq!(
        (x.verts.as_slice(), m.text.as_deref(), m.at),
        (
            &[[10.0, 5.0], [20.0, 20.0], [28.0, 28.0]][..],
            Some("C-201"),
            Some([30.0, 30.0])
        ),
        "a multileader: its line from the arrow to the landing, its text and where it sits"
    );
    let o = run(&b.bytes, "text find", &["--contains", "C-201"]);
    assert_eq!(
        string(&list(&o.data, &["rows"])[0], &["address"]),
        b.addr("ml"),
        "text find reads a multileader's text"
    );
}

#[test]
pub fn mleader_typed() {
    // the generated multileader, from its entity
    let b = Spec::new(Ver::R2018)
        .model(
            Ent::mleader(
                &[[10.0, 5.0], [20.0, 20.0]],
                [28.0, 28.0],
                [30.0, 30.0],
                "C-201",
            )
            .label("ml"),
        )
        .build();
    let t = k2_mpc_dxf::token::tokenize(&b.bytes, &Limits::default()).unwrap();
    let s = k2_mpc_dxf::structure::walk(t, &Limits::default()).unwrap();
    let eu = s
        .entities
        .iter()
        .find(|e| &*e.head.kind == "MULTILEADER")
        .expect("a multileader");
    let m = k2_mpc_dxf::mleader::of_entity(&s, eu).expect("read");
    assert_eq!(
        (
            m.text.as_deref(),
            m.at,
            m.attachment,
            m.height,
            m.arrow_size,
            m.arrow_block
        ),
        (
            Some("C-201"),
            Some([30.0, 30.0]),
            Some(1),
            Some(0.5),
            Some(0.25),
            None
        )
    );
    assert_eq!(
        m.leaders,
        [k2_mpc_dxf::mleader::Branch {
            landing: Some([28.0, 28.0]),
            lines: vec![vec![[10.0, 5.0], [20.0, 20.0]]],
        }]
    );
    // two leaders, the second with two lines, and an arrowhead block: pair by pair
    let mut r = k2_mpc_dxf::mleader::Reader::default();
    for (c, v) in [
        (300, "CONTEXT_DATA{"),
        (140, "0.5"),
        (302, "LEADER{"),
        (10, "1.0"),
        (20, "1.0"),
        (304, "LEADER_LINE{"),
        (10, "0.0"),
        (20, "0.0"),
        (305, "}"),
        (303, "}"),
        (302, "LEADER{"),
        (10, "2.0"),
        (20, "2.0"),
        (304, "LEADER_LINE{"),
        (10, "5.0"),
        (20, "0.0"),
        (305, "}"),
        (304, "LEADER_LINE{"),
        (10, "6.0"),
        (20, "1.0"),
        (10, "5.5"),
        (20, "1.5"),
        (305, "}"),
        (303, "}"),
        (301, "}"),
        (342, "1F"),
    ] {
        r.pair(c, v.as_bytes());
    }
    let m = r.finish();
    use k2_mpc_dxf::mleader::Branch;
    assert_eq!(
        m.leaders,
        [
            Branch {
                landing: Some([1.0, 1.0]),
                lines: vec![vec![[0.0, 0.0]]],
            },
            Branch {
                landing: Some([2.0, 2.0]),
                lines: vec![vec![[5.0, 0.0]], vec![[6.0, 1.0], [5.5, 1.5]]],
            },
        ]
    );
    assert_eq!(
        (m.arrow_block, m.arrow_size),
        (Some(0x1F), Some(0.5)),
        "the arrowhead's block, its size from the context when the entity states none"
    );
}

#[test]
pub fn multileader_every_line_and_plain_text() {
    let b = Spec::new(Ver::R2018)
        .model(
            Ent::mleader_branches(
                &[
                    (vec![vec![[0.0, 0.0], [4.0, 4.0]]], [5.0, 5.0]),
                    (
                        vec![vec![[10.0, 0.0]], vec![[12.0, 1.0], [11.0, 3.0]]],
                        [9.0, 5.0],
                    ),
                ],
                [6.0, 6.0],
                "{\\fArial|b1|i0;C-401}\\PSPARE",
            )
            .label("ml"),
        )
        .build();
    let d = doc(&b);
    let (_, m) = entity(&d, &b, "ml");
    let x = m.extra.as_ref().expect("a multileader's lines");
    assert_eq!(
        x.lines,
        [
            vec![[0.0, 0.0], [4.0, 4.0], [5.0, 5.0]],
            vec![[10.0, 0.0], [9.0, 5.0]],
            vec![[12.0, 1.0], [11.0, 3.0], [9.0, 5.0]],
        ],
        "every leader line from its arrow to its leader's landing"
    );
    let raw = m.text.as_deref().unwrap_or("");
    assert_eq!(raw, "{\\fArial|b1|i0;C-401}\\PSPARE", "the text as written");
    assert_eq!(
        k2_mpc_cad_model::leader::plain(raw),
        "C-401\nSPARE",
        "its plain text: inline codes dropped, paragraphs on lines"
    );
}

#[test]
pub fn placed_geometry_leaves_out_attribute_definitions() {
    // a run whose block holds its size and label templates above the run, and a device symbol
    // (a circle of radius 2 around its insertion point) with its identifier's template beside it
    let b = Spec::new(Ver::R2018)
        .block(
            "RUN",
            [0.0, 0.0],
            vec![
                Ent::line([0.0, -0.1], [1.0, -0.1]),
                Ent::line([0.0, 0.1], [1.0, 0.1]),
                attdef([0.5, 3.0], "SIZE"),
                attdef([0.5, 4.0], "LABEL"),
            ],
        )
        .block(
            "DEV",
            [0.0, 0.0],
            vec![Ent::circle([0.0, 0.0], 2.0), attdef([5.0, 0.0], "DEVICE")],
        )
        .model(
            Ent::insert("RUN", [10.0, 10.0])
                .placed(20.0, 1.0, 90.0)
                .label("run"),
        )
        .model(
            Ent::insert("DEV", [40.0, 40.0])
                .placed(2.0, 2.0, 0.0)
                .label("dev"),
        )
        .build();
    let d = doc(&b);
    let (run, _) = entity(&d, &b, "run");
    let [a, z] = d.placed_axis(run).expect("an axis");
    for (got, want) in [(a, [10.0, 10.0]), (z, [10.0, 30.0])] {
        assert!(
            (got[0] - want[0]).abs() < 1e-9 && (got[1] - want[1]).abs() < 1e-9,
            "the run's ends from its lines, not its templates: {a:?} {z:?}"
        );
    }
    let (dev, _) = entity(&d, &b, "dev");
    let bx = d.placed_box(dev).expect("a box");
    same_box(
        bx,
        [36.0, 36.0, 44.0, 44.0],
        "the device's symbol as placed",
    );
}

#[test]
pub fn layout_names() {
    let mut spec = Spec::new(Ver::R2018).paper(Ent::line([0.0, 0.0], [1.0, 0.0]));
    spec.draw.layout = Some("AV-101".into());
    let b = spec.build();
    let d = doc(&b);
    let names: Vec<(&str, Option<&str>)> = d
        .blocks
        .iter()
        .filter(|x| x.kind != k2_mpc_cad_model::BlockKind::Block)
        .map(|x| (x.kind.word(), x.layout.as_deref()))
        .collect();
    assert_eq!(
        names,
        [("model", Some("Model")), ("layout", Some("AV-101"))],
        "each space's layout name, as its layout object names it"
    );
}
