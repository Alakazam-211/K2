//! Leaders and multileaders over hand-made bytes: one object at a time, its bit fields packed in
//! the row (the data, the strings, the handles), in the 2004 and the 2010 layouts. Each row
//! checks what the read gives the drawing model, and that its read ends exactly at its data's
//! end.

mod support;

use k2_mpc_cad_model::error::CadError;
use k2_mpc_cad_model::leader::{Branch, Mleader, LINES_MAX, POINTS_MAX};
use k2_mpc_dwg::classes::Class;
use k2_mpc_dwg::entities::{entity, Body};
use k2_mpc_dwg::frame::Ver;
use k2_mpc_dwg::leaders::{LeaderData, MleaderData};
use support::object::{b, bd, bd3, bl, bs, ent, h, rc, tu, tv, Common, F};
use support::refused;

/// The class number the rows give multileaders.
const ML: u16 = 500;

fn classes() -> Vec<Class> {
    vec![Class {
        number: ML,
        proxy_flags: 0,
        app_name: "ObjectDBX Classes".into(),
        cpp_name: "AcDbMLeader".into(),
        dxf_name: "MULTILEADER".into(),
        zombie: false,
        item_class_id: 0x1F2,
        instances: 1,
    }]
}

/// An object colour (`CMC`): by layer, no names.
fn cmc() -> F {
    [bs(256), bl(0), rc(0)].concat()
}

/// A text field: in the string stream from 2007, else inline.
fn text(v: Ver, s: &str, data: &mut F, strings: &mut F) {
    if v >= Ver::R2010 {
        strings.extend(tu(s));
    } else {
        data.extend(tv(s.as_bytes()));
    }
}

/// One leader root: its connection point, its direction, its lines' points, and how many
/// break pairs it and its first line carry.
struct Root {
    connection: [f64; 3],
    direction: [f64; 3],
    lines: Vec<Vec<[f64; 3]>>,
    breaks: u32,
}

/// The content: text (its label, normal, location, alignment) or a block, or none.
enum Content<'a> {
    Text(&'a str, [f64; 3], [f64; 3], u16),
    Block,
    None,
}

/// A multileader's fields: `roots`, `content`, the text height, the context's arrowhead size
/// and the entity's, and the handles: each line's (linetype, arrowhead) from `0x60` up, the
/// content's (`0x51`), the style (`0x52`), the leader linetype (`0x53`), the arrowhead
/// (`arrow`), the style's text style (`0x55`) and block (`0x56`).
struct Ml<'a> {
    roots: Vec<Root>,
    content: Content<'a>,
    height: f64,
    ctx_arrow: f64,
    arrow: f64,
    arrow_block: u64,
}

impl Ml<'_> {
    fn parts(&self, v: Ver) -> (F, F, F) {
        let (mut d, mut s, mut hs): (F, F, F) = (Vec::new(), Vec::new(), Vec::new());
        if v >= Ver::R2010 {
            d.extend(bs(2));
        }
        let mut lh = 0x60u64;
        d.extend(bl(self.roots.len() as u32));
        for r in &self.roots {
            d.extend([b(true), b(true), bd3(r.connection), bd3(r.direction)].concat());
            d.extend(bl(r.breaks));
            for _ in 0..r.breaks {
                d.extend([bd3([1.0, 1.0, 0.0]), bd3([2.0, 2.0, 0.0])].concat());
            }
            d.extend([bl(0), bd(0.36)].concat());
            d.extend(bl(r.lines.len() as u32));
            for (i, l) in r.lines.iter().enumerate() {
                d.extend(bl(l.len() as u32));
                for p in l {
                    d.extend(bd3(*p));
                }
                // the first line's breaks: one segment with `breaks` pairs
                if i == 0 && r.breaks > 0 {
                    d.extend([bl(1), bl(0), bl(r.breaks)].concat());
                    for _ in 0..r.breaks {
                        d.extend([bd3([3.0, 3.0, 0.0]), bd3([4.0, 4.0, 0.0])].concat());
                    }
                } else {
                    d.extend(bl(0));
                }
                d.extend(bl(i as u32));
                if v >= Ver::R2010 {
                    d.extend([bs(1), cmc(), bl(0), bd(0.18), bl(0)].concat());
                    hs.extend([h(5, lh), h(5, lh + 1)].concat());
                    lh += 2;
                }
            }
            if v >= Ver::R2010 {
                d.extend(bs(0));
            }
        }
        // the context's content
        d.extend(
            [
                bd(1.0),
                bd3([0.0; 3]),
                bd(self.height),
                bd(self.ctx_arrow),
                bd(0.09),
            ]
            .concat(),
        );
        d.extend([bs(1), bs(1), bs(0), bs(0)].concat());
        match self.content {
            Content::Text(label, normal, location, align) => {
                d.extend(b(true));
                text(v, label, &mut d, &mut s);
                d.extend([bd3(normal), bd3(location), bd3([1.0, 0.0, 0.0])].concat());
                d.extend([bd(0.0), bd(10.0), bd(0.0), bd(1.0), bs(1), cmc()].concat());
                d.extend([bs(align), bs(1), cmc(), bd(1.5), bl(0), b(false), b(false)].concat());
                d.extend([bs(0), b(false), bd(0.0), bd(0.0), b(false), bl(0)].concat());
                d.extend([b(true), b(false)].concat());
                hs.extend(h(5, 0x51));
            }
            Content::Block => {
                d.extend([b(false), b(true)].concat());
                d.extend([bd3([0.0, 0.0, 1.0]), bd3([7.0, 8.0, 0.0]), bd3([1.0; 3])].concat());
                d.extend([bd(0.0), cmc()].concat());
                for k in 0..16 {
                    d.extend(bd(if k % 5 == 0 { 1.0 } else { 0.0 }));
                }
                hs.extend(h(5, 0x51));
            }
            Content::None => d.extend([b(false), b(false)].concat()),
        }
        d.extend(
            [
                bd3([0.0; 3]),
                bd3([1.0, 0.0, 0.0]),
                bd3([0.0, 1.0, 0.0]),
                b(false),
            ]
            .concat(),
        );
        if v >= Ver::R2010 {
            d.extend([bs(9), bs(9)].concat());
        }
        // the entity's own
        d.extend(
            [
                bl(0),
                bs(1),
                cmc(),
                bl(0),
                b(true),
                b(true),
                bd(0.36),
                bd(self.arrow),
            ]
            .concat(),
        );
        d.extend([bs(2), bs(1), bs(1), bs(1), bs(0), cmc(), b(false)].concat());
        d.extend([cmc(), bd3([1.0; 3]), bd(0.0), bs(0), b(false)].concat());
        // no arrowheads (up to 2007 only), no block labels
        if v < Ver::R2010 {
            d.extend(bl(0));
        }
        d.extend(bl(0));
        d.extend([b(false), bs(0), bs(1), bd(1.0)].concat());
        if v >= Ver::R2010 {
            d.extend([bs(0), bs(9), bs(9)].concat());
        }
        if v >= Ver::R2013 {
            d.extend(b(false));
        }
        hs.extend(
            [
                h(5, 0x52),
                h(5, 0x53),
                h(5, self.arrow_block),
                h(5, 0x55),
                h(5, 0x56),
            ]
            .concat(),
        );
        (d, s, hs)
    }
}

fn read(v: Ver, m: &Ml<'_>, extra: F) -> Result<MleaderData, CadError> {
    let (mut d, s, hs) = m.parts(v);
    d.extend(extra);
    let obj = ent(v, ML, 0x90, &Common::on(0x10), d, s, hs);
    let e = entity(&obj.0, obj.1, v, &classes())?;
    match e.body {
        Body::Mleader(m) => Ok(*m),
        other => panic!("{v:?}: not a multileader: {other:?}"),
    }
}

fn must(v: Ver, m: &Ml<'_>) -> MleaderData {
    read(v, m, Vec::new()).unwrap_or_else(|e| panic!("{v:?}: refused: {}", e.summary()))
}

fn two_roots() -> Ml<'static> {
    Ml {
        roots: vec![
            Root {
                connection: [10.0, 5.0, 0.0],
                direction: [1.0, 0.0, 0.0],
                lines: vec![
                    vec![[0.0, 0.0, 0.0], [4.0, 3.0, 0.0]],
                    vec![[2.0, -1.0, 0.0]],
                ],
                breaks: 1,
            },
            Root {
                connection: [-10.0, 5.0, 0.0],
                direction: [-1.0, 0.0, 0.0],
                lines: vec![vec![[-3.0, 0.0, 0.0], [-6.0, 2.5, 0.0]]],
                breaks: 0,
            },
        ],
        content: Content::Text("Note\\Pline 2", [0.0, 0.0, 1.0], [11.0, 6.0, 0.0], 1),
        height: 2.5,
        ctx_arrow: 3.0,
        arrow: 0.75,
        arrow_block: 0x77,
    }
}

#[test]
pub fn multileaders_with_text() {
    for v in [Ver::R2004, Ver::R2010, Ver::R2013, Ver::R2018] {
        let m = must(v, &two_roots());
        let want = Mleader {
            text: Some("Note\\Pline 2".into()),
            at: Some([11.0, 6.0]),
            attachment: Some(1),
            height: Some(2.5),
            leaders: vec![
                Branch {
                    landing: Some([10.0, 5.0]),
                    lines: vec![vec![[0.0, 0.0], [4.0, 3.0]], vec![[2.0, -1.0]]],
                },
                Branch {
                    landing: Some([-10.0, 5.0]),
                    lines: vec![vec![[-3.0, 0.0], [-6.0, 2.5]]],
                },
            ],
            dropped: 0,
            arrow_block: Some(0x77),
            arrow_size: Some(0.75),
        };
        assert_eq!(m.content.text, want.text, "{v:?}: its text");
        assert_eq!(
            m.content.at, want.at,
            "{v:?}: where its text sits (its location)"
        );
        assert_eq!(
            m.content.leaders, want.leaders,
            "{v:?}: each leader's landing (its connection point) and lines"
        );
        assert_eq!(
            m.content.arrow_size, want.arrow_size,
            "{v:?}: the entity's own arrowhead size"
        );
        assert_eq!(
            m.content.arrow_block, want.arrow_block,
            "{v:?}: the arrowhead block, after each line's handles"
        );
        assert_eq!(m.content, want, "{v:?}: the content");
        assert_eq!(
            (m.has_text, m.has_block, m.style, m.text_style, m.block),
            (true, false, Some(0x52), Some(0x51), None),
            "{v:?}: text content, its style and text style"
        );
        assert_eq!(m.context_arrow, 3.0, "{v:?}: the context's arrowhead size");
    }
}

#[test]
pub fn multileaders_with_a_block_or_nothing() {
    let v = Ver::R2018;
    let mut ml = two_roots();
    ml.content = Content::Block;
    let m = must(v, &ml);
    assert_eq!(
        (
            m.content.text.as_deref(),
            m.content.at,
            m.content.attachment
        ),
        (None, None, None),
        "a block's multileader has no text"
    );
    assert_eq!(
        (m.has_text, m.has_block, m.block, m.text_style),
        (false, true, Some(0x51), None),
        "its block"
    );
    assert_eq!(m.content.height, Some(2.5), "the context's text height");
    assert_eq!(m.content.arrow_block, Some(0x77), "its arrowhead block");
    ml.content = Content::None;
    ml.roots.truncate(1);
    let m = must(v, &ml);
    assert_eq!(
        (m.has_text, m.has_block, m.content.leaders.len()),
        (false, false, 1),
        "no content, one leader"
    );
    assert_eq!(m.content.arrow_block, Some(0x77), "its arrowhead block");
}

#[test]
pub fn multileader_lines_and_points_bounded() {
    let v = Ver::R2013;
    let mut ml = two_roots();
    ml.roots.truncate(1);
    ml.roots[0].breaks = 0;
    // two lines more than the model keeps, one point each
    ml.roots[0].lines = (0..LINES_MAX + 2)
        .map(|i| vec![[i as f64, 0.0, 0.0]])
        .collect();
    let m = must(v, &ml);
    assert_eq!(
        (m.content.leaders[0].lines.len(), m.content.dropped),
        (LINES_MAX, 2),
        "the lines past the bound counted, not kept"
    );
    assert_eq!(
        m.content.leaders[0].lines[LINES_MAX - 1],
        vec![[(LINES_MAX - 1) as f64, 0.0]],
        "the lines kept are the first ones"
    );
    // four points more than the model keeps, on one line
    ml.roots[0].lines = vec![vec![[0.0; 3]; POINTS_MAX + 4]];
    let m = must(v, &ml);
    assert_eq!(
        (m.content.leaders[0].lines[0].len(), m.content.dropped),
        (POINTS_MAX, 4),
        "the points past the bound counted, not kept"
    );
}

#[test]
pub fn multileader_data_end_checked() {
    let v = Ver::R2018;
    assert!(
        read(v, &two_roots(), Vec::new()).is_ok(),
        "the undamaged multileader reads"
    );
    refused(
        read(v, &two_roots(), b(false)),
        "DWG-E003",
        "a multileader whose data runs a bit past its fields",
    );
    // a leader-root count larger than the data holds: refused before anything is read
    let ml = two_roots();
    let (d, s, hs) = ml.parts(v);
    let mut big = bs(2);
    big.extend(bl(1 << 24));
    big.extend(d[bs(2).len() + bl(2).len()..].to_vec());
    let obj = ent(v, ML, 0x91, &Common::on(0x10), big, s, hs);
    refused(
        entity(&obj.0, obj.1, v, &classes()),
        "DWG-E003",
        "more leader roots than the data holds",
    );
}

#[test]
pub fn leaders_read() {
    for v in [Ver::R2004, Ver::R2010, Ver::R2013, Ver::R2018] {
        let pts = [[0.0, 0.0, 0.0], [5.0, 5.0, 0.0], [9.0, 5.0, 0.0]];
        // the unnamed flag, text, a spline, three points
        let mut d = [b(false), bs(0), bs(1), bl(3)].concat();
        for p in pts {
            d.extend(bd3(p));
        }
        // origin, extrusion, x direction, block offset, end projection
        d.extend(
            [
                bd3([0.0; 3]),
                bd3([0.0, 0.0, 1.0]),
                bd3([1.0, 0.0, 0.0]),
                bd3([0.0; 3]),
                bd3([0.0; 3]),
            ]
            .concat(),
        );
        // box height and width (up to 2007), the hook on x, no arrowhead, the unnamed fields
        if v < Ver::R2010 {
            d.extend([bd(2.5), bd(10.0)].concat());
        }
        d.extend([b(true), b(false), bs(0), b(false), b(false)].concat());
        let own = [h(5, 0x71), h(5, 0x72)].concat();
        let obj = ent(v, 0x2D, 0xA0, &Common::on(0x10), d, Vec::new(), own);
        let e = entity(&obj.0, obj.1, v, &[]).unwrap_or_else(|e| panic!("{v:?}: {}", e.summary()));
        let Body::Leader(l) = e.body else {
            panic!("{v:?}: not a leader: {:?}", e.body)
        };
        assert_eq!(
            l,
            LeaderData {
                points: pts.to_vec(),
                annot_type: 0,
                path_type: 1,
                n: [0.0, 0.0, 1.0],
                hook_x: true,
                arrow: false,
                annotation: Some(0x71),
                dimstyle: Some(0x72),
            },
            "{v:?}: its points, a spline, no arrowhead, what it annotates, its style"
        );
    }
}
