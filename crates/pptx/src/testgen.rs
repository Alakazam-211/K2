//! Test-only (feature `testgen`): the deck generator our tests, the checker step and the fuzz
//! targets build their fixtures with. Our own generated files are the default fixtures: every
//! name and text in them is synthetic, and the generator's model is the truth the reading
//! tests compare against.
//!
//! A deck has one master with two layouts ("Title Slide", "Title and Content", the second
//! with date, footer and slide-number placeholders), a theme, a notes master when asked, and
//! slides of placeholders, text boxes, shapes, a picture, a table, a group and a connector,
//! with speaker notes, an animation and sections when asked. The package is a stored zip
//! written here ([`stored_zip`]), sharing no code with the package layer.

/// PresentationML.
pub const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
/// DrawingML.
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// Relationships, in a part's XML.
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";
const RT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
const CT: &str = "application/vnd.openxmlformats-officedocument.presentationml.";
const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// The first layout's id; the master's is one less.
pub const LAYOUT_TITLE: u64 = 2_147_483_649;
/// The second layout's id.
pub const LAYOUT_CONTENT: u64 = 2_147_483_650;
/// The master's id.
pub const MASTER_ID: u64 = 2_147_483_648;

/// A 1x1 PNG.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0xE9, 0xFA, 0xDC, 0xD8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];

/// One shape on a generated slide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// A title placeholder (`ctr` for a centred title).
    Title { id: u32, text: String, ctr: bool },
    /// The body placeholder (`idx="1"`), one paragraph per line.
    Body { id: u32, text: String },
    /// A text box at x, y, w, h (EMU).
    TextBox {
        id: u32,
        name: String,
        at: (i64, i64, i64, i64),
        text: String,
    },
    /// A rectangle with no text.
    Rect { id: u32, at: (i64, i64, i64, i64) },
    /// A picture of the deck's image, with alt text.
    Picture {
        id: u32,
        at: (i64, i64, i64, i64),
        descr: String,
    },
    /// A table of rows of cells.
    Table {
        id: u32,
        at: (i64, i64, i64, i64),
        rows: Vec<Vec<String>>,
    },
    /// A group of shapes.
    Group {
        id: u32,
        at: (i64, i64, i64, i64),
        children: Vec<Shape>,
    },
    /// A straight connector from shape `from` to shape `to`.
    Connector {
        id: u32,
        at: (i64, i64, i64, i64),
        from: u32,
        to: u32,
    },
}

impl Shape {
    /// A text box with a generated name.
    pub fn text_box(id: u32, at: (i64, i64, i64, i64), text: &str) -> Shape {
        Shape::TextBox {
            id,
            name: format!("TextBox {}", id.saturating_sub(1)),
            at,
            text: text.to_owned(),
        }
    }

    /// The shape's id.
    pub fn id(&self) -> u32 {
        match self {
            Shape::Title { id, .. }
            | Shape::Body { id, .. }
            | Shape::TextBox { id, .. }
            | Shape::Rect { id, .. }
            | Shape::Picture { id, .. }
            | Shape::Table { id, .. }
            | Shape::Group { id, .. }
            | Shape::Connector { id, .. } => *id,
        }
    }
}

/// One generated slide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slide {
    pub id: u32,
    /// 0: "Title Slide", 1: "Title and Content".
    pub layout: usize,
    pub shapes: Vec<Shape>,
    pub notes: Option<String>,
    pub hidden: bool,
    /// A shape id an entrance animation targets.
    pub animate: Option<u32>,
    pub creation_id: Option<u32>,
}

impl Slide {
    /// A slide on layout 1 with a title and a body.
    pub fn titled(id: u32, title: &str, body: &str) -> Slide {
        Slide {
            id,
            layout: 1,
            shapes: vec![
                Shape::Title {
                    id: 2,
                    text: title.to_owned(),
                    ctr: false,
                },
                Shape::Body {
                    id: 3,
                    text: body.to_owned(),
                },
            ],
            notes: None,
            hidden: false,
            animate: None,
            creation_id: None,
        }
    }
}

/// One generated deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckSpec {
    pub slides: Vec<Slide>,
    pub notes_master: bool,
    /// Section names, each with the ids of its slides.
    pub sections: Vec<(String, Vec<u32>)>,
    /// Put a line break before each shape and slide entry (whitespace between elements).
    pub pretty: bool,
}

impl DeckSpec {
    /// Slides with a notes master, no sections, compact.
    pub fn new(slides: Vec<Slide>) -> DeckSpec {
        DeckSpec {
            slides,
            notes_master: true,
            sections: Vec::new(),
            pretty: false,
        }
    }
}

/// A small varied deck: a title slide, a content slide with notes, a slide of every shape kind
/// with an animation and a connector, and a hidden slide.
pub fn sample() -> DeckSpec {
    let mut title = Slide::titled(256, "Quarterly review", "");
    title.layout = 0;
    title.shapes = vec![
        Shape::Title {
            id: 2,
            text: "Quarterly review".into(),
            ctr: true,
        },
        Shape::text_box(
            3,
            (838200, 4000000, 6000000, 600000),
            "Prepared by the planning group",
        ),
    ];
    title.creation_id = Some(1_234_567);
    let mut content = Slide::titled(257, "Goals", "Grow the pilot\nKeep costs flat");
    content.notes = Some("Mention the pilot sites.\nAsk for questions.".into());
    let mix = Slide {
        id: 258,
        layout: 1,
        shapes: vec![
            Shape::Title {
                id: 2,
                text: "Everything".into(),
                ctr: false,
            },
            Shape::text_box(4, (838200, 1500000, 3000000, 500000), "Left box"),
            Shape::Rect {
                id: 5,
                at: (5000000, 1500000, 2000000, 1000000),
            },
            Shape::Connector {
                id: 6,
                at: (3838200, 1750000, 1161800, 250000),
                from: 4,
                to: 5,
            },
            Shape::Picture {
                id: 7,
                at: (838200, 3000000, 1000000, 1000000),
                descr: "A small square".into(),
            },
            Shape::Table {
                id: 8,
                at: (2500000, 3000000, 4000000, 740000),
                rows: vec![
                    vec!["Site".into(), "Count".into()],
                    vec!["North".into(), "12".into()],
                ],
            },
            Shape::Group {
                id: 9,
                at: (7500000, 3000000, 2000000, 1000000),
                children: vec![
                    Shape::text_box(10, (7500000, 3000000, 2000000, 500000), "In a group"),
                    Shape::Rect {
                        id: 11,
                        at: (7500000, 3500000, 2000000, 500000),
                    },
                ],
            },
        ],
        notes: None,
        hidden: false,
        animate: Some(4),
        creation_id: None,
    };
    let mut hidden = Slide::titled(259, "Backup", "Spare numbers");
    hidden.hidden = true;
    DeckSpec::new(vec![title, content, mix, hidden])
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn paras(text: &str) -> String {
    if text.is_empty() {
        return "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>".into();
    }
    text.split('\n')
        .map(|l| {
            if l.is_empty() {
                "<a:p/>".to_owned()
            } else {
                format!(
                    "<a:p><a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t>{}</a:t></a:r></a:p>",
                    esc(l)
                )
            }
        })
        .collect()
}

fn xfrm(at: (i64, i64, i64, i64)) -> String {
    format!(
        "<a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
        at.0, at.1, at.2, at.3
    )
}

fn ph_sp(id: u32, name: &str, ph: &str, text: &str, sp_pr: &str) -> String {
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr>{ph}</p:nvPr></p:nvSpPr><p:spPr{sp_pr}<p:txBody><a:bodyPr/><a:lstStyle/>{}</p:txBody></p:sp>",
        esc(name),
        paras(text)
    )
}

fn shape_xml(s: &Shape) -> String {
    match s {
        Shape::Title { id, text, ctr } => ph_sp(
            *id,
            &format!("Title {}", id - 1),
            if *ctr {
                "<p:ph type=\"ctrTitle\"/>"
            } else {
                "<p:ph type=\"title\"/>"
            },
            text,
            "/>",
        ),
        Shape::Body { id, text } => ph_sp(
            *id,
            &format!("Content Placeholder {}", id - 1),
            "<p:ph idx=\"1\"/>",
            text,
            "/>",
        ),
        Shape::TextBox { id, name, at, text } => format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"><a:spAutoFit/></a:bodyPr><a:lstStyle/>{}</p:txBody></p:sp>",
            esc(name),
            xfrm(*at),
            paras(text)
        ),
        Shape::Rect { id, at } => format!(
            "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Rectangle {}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:sp>",
            id - 1,
            xfrm(*at)
        ),
        Shape::Picture { id, at, descr } => format!(
            "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"Picture {}\" descr=\"{}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed=\"rId2\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
            id - 1,
            esc(descr),
            xfrm(*at)
        ),
        Shape::Table { id, at, rows } => {
            let cols = rows.first().map_or(1, Vec::len).max(1);
            let w = (at.2 / cols as i64).max(1);
            let grid: String = (0..cols)
                .map(|_| format!("<a:gridCol w=\"{w}\"/>"))
                .collect();
            let body: String = rows
                .iter()
                .map(|r| {
                    let cells: String = (0..cols)
                        .map(|c| {
                            let t = r.get(c).map_or("", String::as_str);
                            format!(
                                "<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc>",
                                paras(t)
                            )
                        })
                        .collect();
                    format!("<a:tr h=\"370840\">{cells}</a:tr>")
                })
                .collect();
            format!(
                "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"Table {}\"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblPr firstRow=\"1\" bandRow=\"1\"/><a:tblGrid>{grid}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>",
                id - 1,
                at.0,
                at.1,
                at.2,
                at.3
            )
        }
        Shape::Group { id, at, children } => {
            let inner: String = children.iter().map(shape_xml).collect();
            format!(
                "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"{id}\" name=\"Group {}\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/><a:chOff x=\"{x}\" y=\"{y}\"/><a:chExt cx=\"{w}\" cy=\"{h}\"/></a:xfrm></p:grpSpPr>{inner}</p:grpSp>",
                id - 1,
                x = at.0,
                y = at.1,
                w = at.2,
                h = at.3
            )
        }
        Shape::Connector { id, at, from, to } => format!(
            "<p:cxnSp><p:nvCxnSpPr><p:cNvPr id=\"{id}\" name=\"Straight Connector {}\"/><p:cNvCxnSpPr><a:stCxn id=\"{from}\" idx=\"3\"/><a:endCxn id=\"{to}\" idx=\"1\"/></p:cNvCxnSpPr><p:nvPr/></p:nvCxnSpPr><p:spPr>{}<a:prstGeom prst=\"straightConnector1\"><a:avLst/></a:prstGeom></p:spPr></p:cxnSp>",
            id - 1,
            xfrm(*at)
        ),
    }
}

const GRP: &str = "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>";

fn ns() -> String {
    format!("xmlns:a=\"{A}\" xmlns:r=\"{R}\" xmlns:p=\"{P}\"")
}

/// An entrance ("appear") animation of shape `spid`.
fn timing(spid: u32) -> String {
    format!(
        "<p:timing><p:tnLst><p:par><p:cTn id=\"1\" dur=\"indefinite\" restart=\"never\" nodeType=\"tmRoot\"><p:childTnLst><p:seq concurrent=\"1\" nextAc=\"seek\"><p:cTn id=\"2\" dur=\"indefinite\" nodeType=\"mainSeq\"><p:childTnLst><p:par><p:cTn id=\"3\" fill=\"hold\"><p:stCondLst><p:cond delay=\"indefinite\"/></p:stCondLst><p:childTnLst><p:par><p:cTn id=\"4\" fill=\"hold\"><p:stCondLst><p:cond delay=\"0\"/></p:stCondLst><p:childTnLst><p:par><p:cTn id=\"5\" presetID=\"1\" presetClass=\"entr\" presetSubtype=\"0\" fill=\"hold\" nodeType=\"clickEffect\"><p:stCondLst><p:cond delay=\"0\"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id=\"6\" dur=\"1\" fill=\"hold\"><p:stCondLst><p:cond delay=\"499\"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid=\"{spid}\"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val=\"visible\"/></p:to></p:set></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn><p:prevCondLst><p:cond evt=\"onPrev\" delay=\"0\"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt=\"onNext\" delay=\"0\"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"
    )
}

/// A slide part's XML.
pub fn slide_xml(s: &Slide, pretty: bool) -> String {
    let sep = if pretty { "\n" } else { "" };
    let shapes: String = s
        .shapes
        .iter()
        .map(|x| format!("{sep}{}", shape_xml(x)))
        .collect();
    let show = if s.hidden { " show=\"0\"" } else { "" };
    let anim = s.animate.map(timing).unwrap_or_default();
    let ext = s.creation_id.map_or(String::new(), |c| {
        format!("<p:extLst><p:ext uri=\"{{BB962C8B-B14F-4D97-AF65-F5344CB8AC3E}}\"><p14:creationId xmlns:p14=\"{P14}\" val=\"{c}\"/></p:ext></p:extLst>")
    });
    format!(
        "{HEAD}<p:sld {}{show}><p:cSld><p:spTree>{GRP}{shapes}{sep}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{anim}{ext}</p:sld>",
        ns()
    )
}

fn notes_xml(text: &str) -> String {
    format!(
        "{HEAD}<p:notes {}><p:cSld><p:spTree>{GRP}<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Slide Image Placeholder 1\"/><p:cNvSpPr><a:spLocks noGrp=\"1\" noRot=\"1\" noChangeAspect=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"sldImg\"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>",
        ns(),
        ph_sp(3, "Notes Placeholder 2", "<p:ph type=\"body\" idx=\"1\"/>", text, "/>")
    )
}

const CLR_MAP: &str = "bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"";

fn master_xml() -> String {
    let title = ph_sp(
        2,
        "Title Placeholder 1",
        "<p:ph type=\"title\"/>",
        "Title",
        &format!(
            ">{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>",
            xfrm((838200, 365125, 10515600, 1325563))
        ),
    );
    let body = ph_sp(
        3,
        "Text Placeholder 2",
        "<p:ph type=\"body\" idx=\"1\"/>",
        "Text",
        &format!(
            ">{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>",
            xfrm((838200, 1825625, 10515600, 4351338))
        ),
    );
    format!(
        "{HEAD}<p:sldMaster {}><p:cSld><p:spTree>{GRP}{title}{body}</p:spTree></p:cSld><p:clrMap {CLR_MAP}/><p:sldLayoutIdLst><p:sldLayoutId id=\"{LAYOUT_TITLE}\" r:id=\"rId1\"/><p:sldLayoutId id=\"{LAYOUT_CONTENT}\" r:id=\"rId2\"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz=\"4400\"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz=\"2800\"/></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz=\"1800\"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>",
        ns()
    )
}

fn layout_xml(content: bool) -> String {
    let (name, ty, shapes) = if content {
        (
            "Title and Content",
            "obj",
            [
                ph_sp(
                    2,
                    "Title 1",
                    "<p:ph type=\"title\"/>",
                    "Click to add a title",
                    "/>",
                ),
                ph_sp(
                    3,
                    "Content Placeholder 2",
                    "<p:ph idx=\"1\"/>",
                    "Click to add text",
                    "/>",
                ),
                ph_sp(
                    4,
                    "Date Placeholder 3",
                    "<p:ph type=\"dt\" sz=\"half\" idx=\"10\"/>",
                    "",
                    "/>",
                ),
                ph_sp(
                    5,
                    "Footer Placeholder 4",
                    "<p:ph type=\"ftr\" sz=\"quarter\" idx=\"11\"/>",
                    "",
                    "/>",
                ),
                ph_sp(
                    6,
                    "Slide Number Placeholder 5",
                    "<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>",
                    "",
                    "/>",
                ),
            ]
            .concat(),
        )
    } else {
        (
            "Title Slide",
            "title",
            [
                ph_sp(
                    2,
                    "Title 1",
                    "<p:ph type=\"ctrTitle\"/>",
                    "Click to add a title",
                    "/>",
                ),
                ph_sp(
                    3,
                    "Subtitle 2",
                    "<p:ph type=\"subTitle\" idx=\"1\"/>",
                    "Click to add a subtitle",
                    "/>",
                ),
            ]
            .concat(),
        )
    };
    format!(
        "{HEAD}<p:sldLayout {} type=\"{ty}\" preserve=\"1\"><p:cSld name=\"{name}\"><p:spTree>{GRP}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>",
        ns()
    )
}

fn notes_master_xml() -> String {
    format!(
        "{HEAD}<p:notesMaster {}><p:cSld><p:spTree>{GRP}<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Slide Image Placeholder 1\"/><p:cNvSpPr><a:spLocks noGrp=\"1\" noRot=\"1\" noChangeAspect=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"sldImg\" idx=\"2\"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp>{}</p:spTree></p:cSld><p:clrMap {CLR_MAP}/></p:notesMaster>",
        ns(),
        ph_sp(3, "Notes Placeholder 2", "<p:ph type=\"body\" sz=\"quarter\" idx=\"3\"/>", "Notes", "/>")
    )
}

fn theme_xml() -> String {
    let c = |n: &str, v: &str| format!("<a:{n}><a:srgbClr val=\"{v}\"/></a:{n}>");
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let ln = "<a:ln w=\"6350\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>";
    let eff = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    let font = "<a:latin typeface=\"Arial\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/>";
    format!(
        "{HEAD}<a:theme xmlns:a=\"{A}\" name=\"Generated\"><a:themeElements><a:clrScheme name=\"Generated\">{}{}{}{}{}{}{}{}{}{}{}{}</a:clrScheme><a:fontScheme name=\"Generated\"><a:majorFont>{font}</a:majorFont><a:minorFont>{font}</a:minorFont></a:fontScheme><a:fmtScheme name=\"Generated\"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst><a:lnStyleLst>{ln}{ln}{ln}</a:lnStyleLst><a:effectStyleLst>{eff}{eff}{eff}</a:effectStyleLst><a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
        c("dk1", "000000"),
        c("lt1", "FFFFFF"),
        c("dk2", "1F2A44"),
        c("lt2", "E7E6E6"),
        c("accent1", "4472C4"),
        c("accent2", "ED7D31"),
        c("accent3", "A5A5A5"),
        c("accent4", "FFC000"),
        c("accent5", "5B9BD5"),
        c("accent6", "70AD47"),
        c("hlink", "0563C1"),
        c("folHlink", "954F72"),
    )
}

fn rels(list: &[(String, String, String)]) -> String {
    let items: String = list
        .iter()
        .map(|(id, ty, t)| format!("<Relationship Id=\"{id}\" Type=\"{RT}{ty}\" Target=\"{t}\"/>"))
        .collect();
    format!("{HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{items}</Relationships>")
}

fn r3(id: &str, ty: &str, t: &str) -> (String, String, String) {
    (id.to_owned(), ty.to_owned(), t.to_owned())
}

/// Every part of a generated deck, in a package's usual order.
pub fn parts(spec: &DeckSpec) -> Vec<(String, Vec<u8>)> {
    let sep = if spec.pretty { "\n" } else { "" };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut types = vec![
        (
            "/ppt/presentation.xml".to_owned(),
            format!("{CT}presentation.main+xml"),
        ),
        (
            "/ppt/slideMasters/slideMaster1.xml".to_owned(),
            format!("{CT}slideMaster+xml"),
        ),
        (
            "/ppt/slideLayouts/slideLayout1.xml".to_owned(),
            format!("{CT}slideLayout+xml"),
        ),
        (
            "/ppt/slideLayouts/slideLayout2.xml".to_owned(),
            format!("{CT}slideLayout+xml"),
        ),
        (
            "/ppt/theme/theme1.xml".to_owned(),
            "application/vnd.openxmlformats-officedocument.theme+xml".to_owned(),
        ),
        (
            "/ppt/presProps.xml".to_owned(),
            format!("{CT}presProps+xml"),
        ),
        (
            "/ppt/viewProps.xml".to_owned(),
            format!("{CT}viewProps+xml"),
        ),
        (
            "/ppt/tableStyles.xml".to_owned(),
            format!("{CT}tableStyles+xml"),
        ),
        (
            "/docProps/core.xml".to_owned(),
            "application/vnd.openxmlformats-package.core-properties+xml".to_owned(),
        ),
        (
            "/docProps/app.xml".to_owned(),
            "application/vnd.openxmlformats-officedocument.extended-properties+xml".to_owned(),
        ),
    ];
    out.push((
        "_rels/.rels".into(),
        format!("{HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{RT}officeDocument\" Target=\"ppt/presentation.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/><Relationship Id=\"rId3\" Type=\"{RT}extended-properties\" Target=\"docProps/app.xml\"/></Relationships>"),
    ));
    // the presentation's relationships
    // fixed ids first, the slides' after (a slide's part and relationship follow its id, so
    // a reorder renames nothing and an added slide takes the next ones, as an app does)
    let mut prels = vec![r3("rId1", "slideMaster", "slideMasters/slideMaster1.xml")];
    for (n, (ty, t)) in [
        ("presProps", "presProps.xml"),
        ("viewProps", "viewProps.xml"),
        ("theme", "theme/theme1.xml"),
        ("tableStyles", "tableStyles.xml"),
    ]
    .into_iter()
    .enumerate()
    {
        prels.push(r3(&format!("rId{}", n + 2), ty, t));
    }
    let mut notes_master_list = String::new();
    if spec.notes_master {
        prels.push(r3("rId6", "notesMaster", "notesMasters/notesMaster1.xml"));
        notes_master_list =
            "<p:notesMasterIdLst><p:notesMasterId r:id=\"rId6\"/></p:notesMasterIdLst>".into();
    }
    let mut sld_ids = String::new();
    for s in &spec.slides {
        let k = part_no(s.id);
        prels.push(r3(
            &format!("rId{}", k + 9),
            "slide",
            &format!("slides/slide{k}.xml"),
        ));
        sld_ids.push_str(&format!(
            "{sep}<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
            s.id,
            k + 9
        ));
    }
    let sections = if spec.sections.is_empty() {
        String::new()
    } else {
        let s: String = spec
            .sections
            .iter()
            .enumerate()
            .map(|(i, (name, ids))| {
                let ids: String = ids
                    .iter()
                    .map(|id| format!("<p14:sldId id=\"{id}\"/>"))
                    .collect();
                format!(
                    "<p14:section name=\"{}\" id=\"{{7D3A1C2B-0000-4000-8000-00000000000{}}}\"><p14:sldIdLst>{ids}</p14:sldIdLst></p14:section>",
                    esc(name),
                    i % 10
                )
            })
            .collect();
        format!("<p:extLst><p:ext uri=\"{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}\"><p14:sectionLst xmlns:p14=\"{P14}\">{s}</p14:sectionLst></p:ext></p:extLst>")
    };
    let list = if spec.slides.is_empty() {
        String::new()
    } else {
        format!("<p:sldIdLst>{sld_ids}{sep}</p:sldIdLst>")
    };
    out.push((
        "[Content_Types].xml".into(),
        String::new(), // filled below
    ));
    out.push((
        "ppt/presentation.xml".into(),
        format!(
            "{HEAD}<p:presentation {} saveSubsetFonts=\"1\"><p:sldMasterIdLst><p:sldMasterId id=\"{MASTER_ID}\" r:id=\"rId1\"/></p:sldMasterIdLst>{notes_master_list}{list}<p:sldSz cx=\"12192000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/><p:defaultTextStyle><a:lvl1pPr><a:defRPr sz=\"1800\"/></a:lvl1pPr></p:defaultTextStyle>{sections}</p:presentation>",
            ns()
        ),
    ));
    out.push(("ppt/_rels/presentation.xml.rels".into(), rels(&prels)));
    let mut any_picture = false;
    for s in &spec.slides {
        let k = part_no(s.id);
        out.push((
            format!("ppt/slides/slide{k}.xml"),
            slide_xml(s, spec.pretty),
        ));
        types.push((
            format!("/ppt/slides/slide{k}.xml"),
            format!("{CT}slide+xml"),
        ));
        let mut srels = vec![r3(
            "rId1",
            "slideLayout",
            &format!("../slideLayouts/slideLayout{}.xml", s.layout.min(1) + 1),
        )];
        if has_picture(&s.shapes) {
            any_picture = true;
            srels.push(r3("rId2", "image", "../media/image1.png"));
        }
        if let (Some(t), true) = (&s.notes, spec.notes_master) {
            let notes_n = k;
            srels.push(r3(
                "rId3",
                "notesSlide",
                &format!("../notesSlides/notesSlide{notes_n}.xml"),
            ));
            out.push((
                format!("ppt/notesSlides/notesSlide{notes_n}.xml"),
                notes_xml(t),
            ));
            out.push((
                format!("ppt/notesSlides/_rels/notesSlide{notes_n}.xml.rels"),
                rels(&[
                    r3("rId1", "notesMaster", "../notesMasters/notesMaster1.xml"),
                    r3("rId2", "slide", &format!("../slides/slide{k}.xml")),
                ]),
            ));
            types.push((
                format!("/ppt/notesSlides/notesSlide{notes_n}.xml"),
                format!("{CT}notesSlide+xml"),
            ));
        }
        out.push((format!("ppt/slides/_rels/slide{k}.xml.rels"), rels(&srels)));
    }
    out.push(("ppt/slideMasters/slideMaster1.xml".into(), master_xml()));
    out.push((
        "ppt/slideMasters/_rels/slideMaster1.xml.rels".into(),
        rels(&[
            r3("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
            r3("rId2", "slideLayout", "../slideLayouts/slideLayout2.xml"),
            r3("rId3", "theme", "../theme/theme1.xml"),
        ]),
    ));
    for k in 1..=2 {
        out.push((
            format!("ppt/slideLayouts/slideLayout{k}.xml"),
            layout_xml(k == 2),
        ));
        out.push((
            format!("ppt/slideLayouts/_rels/slideLayout{k}.xml.rels"),
            rels(&[r3(
                "rId1",
                "slideMaster",
                "../slideMasters/slideMaster1.xml",
            )]),
        ));
    }
    if spec.notes_master {
        out.push((
            "ppt/notesMasters/notesMaster1.xml".into(),
            notes_master_xml(),
        ));
        out.push((
            "ppt/notesMasters/_rels/notesMaster1.xml.rels".into(),
            rels(&[r3("rId1", "theme", "../theme/theme2.xml")]),
        ));
        out.push(("ppt/theme/theme2.xml".into(), theme_xml()));
        types.push((
            "/ppt/notesMasters/notesMaster1.xml".into(),
            format!("{CT}notesMaster+xml"),
        ));
        types.push((
            "/ppt/theme/theme2.xml".into(),
            "application/vnd.openxmlformats-officedocument.theme+xml".into(),
        ));
    }
    out.push(("ppt/theme/theme1.xml".into(), theme_xml()));
    out.push((
        "ppt/presProps.xml".into(),
        format!("{HEAD}<p:presentationPr {}/>", ns()),
    ));
    out.push((
        "ppt/viewProps.xml".into(),
        format!("{HEAD}<p:viewPr {}/>", ns()),
    ));
    out.push((
        "ppt/tableStyles.xml".into(),
        format!("{HEAD}<a:tblStyleLst xmlns:a=\"{A}\" def=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\"/>"),
    ));
    out.push((
        "docProps/core.xml".into(),
        format!("{HEAD}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>Generated deck</dc:title><dc:creator>Generator</dc:creator></cp:coreProperties>"),
    ));
    out.push((
        "docProps/app.xml".into(),
        format!("{HEAD}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>k2-mpc-pptx test generator</Application><Slides>{}</Slides></Properties>", spec.slides.len()),
    ));
    let mut out: Vec<(String, Vec<u8>)> =
        out.into_iter().map(|(n, c)| (n, c.into_bytes())).collect();
    if any_picture {
        out.push(("ppt/media/image1.png".into(), PNG.to_vec()));
    }
    let overrides: String = types
        .iter()
        .map(|(p, t)| format!("<Override PartName=\"{p}\" ContentType=\"{t}\"/>"))
        .collect();
    let ct = format!(
        "{HEAD}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/>{overrides}</Types>"
    );
    let at = out
        .iter()
        .position(|(n, _)| n == "[Content_Types].xml")
        .expect("placed");
    out[at].1 = ct.into_bytes();
    // the content types first, as writers put them
    let ct = out.remove(at);
    out.insert(0, ct);
    out
}

/// The number in a generated slide's part name (`slide1.xml` for slide 256).
pub fn part_no(id: u32) -> u32 {
    id.saturating_sub(255).max(1)
}

fn has_picture(shapes: &[Shape]) -> bool {
    shapes.iter().any(|s| match s {
        Shape::Picture { .. } => true,
        Shape::Group { children, .. } => has_picture(children),
        _ => false,
    })
}

pub mod golden;

/// A deck's bytes: [`parts`] in a stored zip.
pub fn deck(spec: &DeckSpec) -> Vec<u8> {
    stored_zip(&parts(spec))
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A stored zip of `parts`, in order: version 2.0, no flags, no times, no extra fields.
pub fn stored_zip(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let (mut out, mut cd) = (Vec::new(), Vec::new());
    for (n, c) in parts {
        let off = out.len() as u32;
        let crc = crc32(c);
        let head = |v: &mut Vec<u8>| {
            v.extend_from_slice(&0u16.to_le_bytes()); // flags
            v.extend_from_slice(&0u16.to_le_bytes()); // stored
            v.extend_from_slice(&0x0021_0000u32.to_le_bytes()); // time 0, date 1980-01-01
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&(c.len() as u32).to_le_bytes());
            v.extend_from_slice(&(c.len() as u32).to_le_bytes());
            v.extend_from_slice(&(n.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        head(&mut out);
        out.extend_from_slice(n.as_bytes());
        out.extend_from_slice(c);
        cd.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        head(&mut cd);
        cd.extend_from_slice(&[0u8; 10]);
        cd.extend_from_slice(&off.to_le_bytes());
        cd.extend_from_slice(n.as_bytes());
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// `parts` with one part's content replaced (by name), for building variants.
pub fn with_part(
    parts: &[(String, Vec<u8>)],
    name: &str,
    content: Vec<u8>,
) -> Vec<(String, Vec<u8>)> {
    parts
        .iter()
        .map(|(n, c)| {
            if n == name {
                (n.clone(), content.clone())
            } else {
                (n.clone(), c.clone())
            }
        })
        .collect()
}
