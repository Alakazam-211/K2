//! The golden sample deck, generated: four slides
//! on one master with three layouts, theme fonts "Liberation Sans" (major) and "Liberation
//! Serif" (minor), a group turned 90°, a turned and flipped arrow that links to slide 4, a
//! table on a table style, a slide-number field. Every name and text in it is synthetic.
//!
//! [`Extra`] appends one of the untested cases as a fifth slide: `D` (`approx_box`),
//! `D2` (one group flip with a turned child) or `D3` (approx inherited by a nested child).

use super::{esc, r3, rels, stored_zip, A, CT, HEAD, P, R};

/// The master's id, and its three layouts'.
pub const MASTER: u64 = 2_147_483_648;
pub const TITLE_SLIDE: u64 = 2_147_483_649;
pub const TITLE_CONTENT: u64 = 2_147_483_650;
pub const TITLE_ONLY: u64 = 2_147_483_651;

/// A fifth slide, from the untested cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    None,
    /// Case `D`: `@sld:261`, a group scaled (2, 0.5) holding a square at 0°, one at 45° and one
    /// at 90°.
    D,
    /// Case `D2`: `@sld:263`, a group turned 90° and flipped across, holding a box turned 30°.
    D2,
    /// Case `D2`'s variant: the same group at 0°.
    D2Rot0,
    /// Case `D3`: `@sld:262`, a group scaled (2, 0.5) holding a group turned 45° holding a box.
    D3,
    /// Case `D3` with the box counter-turned (315°), so its angle at the scaled level is 0°:
    /// only step 8 (approx inherited) flags it. Not one of the hand-made goldens.
    D3Counter,
}

fn rpr(font: Option<&str>) -> String {
    match font {
        Some(f) => format!(
            "<a:rPr lang=\"en-US\" dirty=\"0\"><a:latin typeface=\"{}\"/></a:rPr>",
            esc(f)
        ),
        None => "<a:rPr lang=\"en-US\" dirty=\"0\"/>".into(),
    }
}

fn paras(text: &str, font: Option<&str>) -> String {
    text.split('\n')
        .map(|l| format!("<a:p><a:r>{}<a:t>{}</a:t></a:r></a:p>", rpr(font), esc(l)))
        .collect()
}

fn xfrm(x: i64, y: i64, w: i64, h: i64, extra: &str) -> String {
    format!("<a:xfrm{extra}><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></a:xfrm>")
}

/// A placeholder; `at` its own box (none: its layout's).
fn ph(id: u32, name: &str, ph: &str, at: Option<(i64, i64, i64, i64)>, body: &str) -> String {
    let sp_pr = match at {
        Some((x, y, w, h)) => format!("<p:spPr>{}</p:spPr>", xfrm(x, y, w, h, "")),
        None => "<p:spPr/>".into(),
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr>{ph}</p:nvPr></p:nvSpPr>{sp_pr}<p:txBody><a:bodyPr/><a:lstStyle/>{body}</p:txBody></p:sp>",
        esc(name)
    )
}

/// A shape's style, its text in the theme's minor font.
const STYLE: &str = "<p:style><a:lnRef idx=\"2\"><a:schemeClr val=\"accent1\"/></a:lnRef><a:fillRef idx=\"1\"><a:schemeClr val=\"accent1\"/></a:fillRef><a:effectRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"lt1\"/></a:fontRef></p:style>";

/// A preset shape with no text, with the style.
#[allow(clippy::too_many_arguments)]
fn rect(id: u32, name: &str, x: i64, y: i64, w: i64, h: i64, rot: i64, prst: &str) -> String {
    let r = if rot == 0 {
        String::new()
    } else {
        format!(" rot=\"{rot}\"")
    };
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst=\"{prst}\"><a:avLst/></a:prstGeom></p:spPr>{STYLE}</p:sp>",
        esc(name),
        xfrm(x, y, w, h, &r)
    )
}

/// A group: its transform (with `extra` attributes such as `rot` and `flipH`), child space.
#[allow(clippy::too_many_arguments)]
fn group(
    id: u32,
    name: &str,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    ch: (i64, i64, i64, i64),
    extra: &str,
    kids: &str,
) -> String {
    format!(
        "<p:grpSp><p:nvGrpSpPr><p:cNvPr id=\"{id}\" name=\"{}\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm{extra}><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/><a:chOff x=\"{}\" y=\"{}\"/><a:chExt cx=\"{}\" cy=\"{}\"/></a:xfrm></p:grpSpPr>{kids}</p:grpSp>",
        esc(name),
        ch.0,
        ch.1,
        ch.2,
        ch.3
    )
}

const GRP: &str = "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>";

fn ns() -> String {
    format!("xmlns:a=\"{A}\" xmlns:r=\"{R}\" xmlns:p=\"{P}\"")
}

fn slide(shapes: &str) -> String {
    format!(
        "{HEAD}<p:sld {}><p:cSld><p:spTree>{GRP}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>",
        ns()
    )
}

fn slide1() -> String {
    slide(
        &[
            ph(
                2,
                "Title 1",
                "<p:ph type=\"ctrTitle\"/>",
                None,
                &paras("Quarterly review", None),
            ),
            ph(
                3,
                "Subtitle 2",
                "<p:ph type=\"subTitle\" idx=\"1\"/>",
                None,
                &paras("Team update", None),
            ),
        ]
        .concat(),
    )
}

fn slide2() -> String {
    let callout = format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"6\" name=\"Revenue callout\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"><a:spAutoFit/></a:bodyPr><a:lstStyle/>{}</p:txBody></p:sp>",
        xfrm(2_000_000, 500_000, 2_000_000, 1_000_000, ""),
        paras("Revenue up 12%", Some("Inter"))
    );
    let kids = [
        rect(5, "Rectangle 5", 0, 0, 2_000_000, 2_000_000, 0, "rect"),
        callout,
    ]
    .concat();
    let arrow = format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"7\" name=\"Right Arrow 7\"><a:hlinkClick r:id=\"rId2\" action=\"ppaction://hlinksldjump\"/></p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst=\"rightArrow\"><a:avLst/></a:prstGeom></p:spPr>{STYLE}</p:sp>",
        xfrm(7_000_000, 2_000_000, 3_000_000, 1_000_000, " rot=\"1800000\" flipH=\"1\"")
    );
    slide(
        &[
            ph(
                2,
                "Title 1",
                "<p:ph type=\"title\"/>",
                None,
                &paras("Highlights", None),
            ),
            group(
                4,
                "Group 4",
                1_000_000,
                1_500_000,
                4_000_000,
                2_000_000,
                (0, 0, 4_000_000, 2_000_000),
                " rot=\"5400000\"",
                &kids,
            ),
            arrow,
        ]
        .concat(),
    )
}

fn slide3() -> String {
    let rows = [
        ["Region", "Q3", "Q4"],
        ["North", "40", "44"],
        ["South", "35", "38"],
    ];
    let grid: String = (0..3)
        .map(|_| "<a:gridCol w=\"3505200\"/>".to_owned())
        .collect();
    let body: String = rows
        .iter()
        .map(|r| {
            let cells: String = r
                .iter()
                .map(|t| {
                    format!(
                        "<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr/></a:tc>",
                        paras(t, Some("DejaVu Sans"))
                    )
                })
                .collect();
            format!("<a:tr h=\"1000000\">{cells}</a:tr>")
        })
        .collect();
    let table = format!(
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"4\" name=\"Table 3\"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"838200\" y=\"1825625\"/><a:ext cx=\"10515600\" cy=\"3000000\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblPr firstRow=\"1\" bandRow=\"1\"><a:tableStyleId>{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}</a:tableStyleId></a:tblPr><a:tblGrid>{grid}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"
    );
    slide(
        &[
            ph(
                2,
                "Title 1",
                "<p:ph type=\"title\"/>",
                None,
                &paras("Regional numbers", None),
            ),
            table,
        ]
        .concat(),
    )
}

fn slide4() -> String {
    let num = "<a:p><a:fld id=\"{B6F15528-21DE-4FAA-801E-634DDDAF4B2B}\" type=\"slidenum\"><a:rPr lang=\"en-US\"/><a:t>4</a:t></a:fld><a:endParaRPr lang=\"en-US\"/></a:p>";
    slide(
        &[
            ph(
                2,
                "Title 1",
                "<p:ph type=\"title\"/>",
                None,
                &paras("Next steps", None),
            ),
            ph(
                3,
                "Content Placeholder 2",
                "<p:ph type=\"body\" idx=\"1\"/>",
                None,
                &paras("Hire two engineers\nShip the v2 beta", None),
            ),
            ph(
                4,
                "Slide Number Placeholder 3",
                "<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>",
                None,
                num,
            ),
        ]
        .concat(),
    )
}

/// The fifth slide of a case: its id and part.
fn extra_slide(e: Extra) -> Option<(u32, String)> {
    let uneven = |id: u32, kids: &str| {
        group(
            id,
            &format!("Group {id}"),
            6_000_000,
            4_000_000,
            4_000_000,
            1_000_000,
            (0, 0, 2_000_000, 2_000_000),
            "",
            kids,
        )
    };
    let d2 = |rot: &str| {
        group(
            30,
            "Group 30",
            2_000_000,
            1_000_000,
            2_000_000,
            2_000_000,
            (0, 0, 2_000_000, 2_000_000),
            &format!("{rot} flipH=\"1\""),
            &rect(
                31,
                "Rectangle 31",
                0,
                0,
                1_000_000,
                500_000,
                1_800_000,
                "rect",
            ),
        )
    };
    match e {
        Extra::None => None,
        Extra::D => Some((
            261,
            slide(&uneven(
                10,
                &[
                    rect(11, "Rectangle 11", 0, 0, 1_000_000, 1_000_000, 0, "rect"),
                    rect(
                        12,
                        "Rectangle 12",
                        1_000_000,
                        1_000_000,
                        1_000_000,
                        1_000_000,
                        2_700_000,
                        "rect",
                    ),
                    rect(
                        13,
                        "Rectangle 13",
                        0,
                        1_000_000,
                        1_000_000,
                        1_000_000,
                        5_400_000,
                        "rect",
                    ),
                ]
                .concat(),
            )),
        )),
        Extra::D2 => Some((263, slide(&d2(" rot=\"5400000\"")))),
        Extra::D2Rot0 => Some((263, slide(&d2("")))),
        Extra::D3 | Extra::D3Counter => {
            let own = if e == Extra::D3Counter { 18_900_000 } else { 0 };
            Some((
                262,
                slide(&uneven(
                    20,
                    &group(
                        21,
                        "Group 21",
                        0,
                        0,
                        2_000_000,
                        2_000_000,
                        (0, 0, 2_000_000, 2_000_000),
                        " rot=\"2700000\"",
                        &rect(22, "Rectangle 22", 0, 0, 2_000_000, 1_000_000, own, "rect"),
                    ),
                )),
            ))
        }
    }
}

/// A layout's placeholders, each with its box.
fn layout(name: &str, ty: &str, shapes: &str) -> String {
    format!(
        "{HEAD}<p:sldLayout {} type=\"{ty}\" preserve=\"1\"><p:cSld name=\"{name}\"><p:spTree>{GRP}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>",
        ns()
    )
}

const TITLE_BOX: (i64, i64, i64, i64) = (838_200, 365_125, 10_515_600, 1_325_563);
const BODY_BOX: (i64, i64, i64, i64) = (838_200, 1_825_625, 10_515_600, 4_351_338);

fn layouts() -> [String; 3] {
    let empty = "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>";
    [
        layout(
            "Title Slide",
            "title",
            &[
                ph(
                    2,
                    "Title 1",
                    "<p:ph type=\"ctrTitle\"/>",
                    Some((1_524_000, 1_122_363, 9_144_000, 2_387_600)),
                    empty,
                ),
                ph(
                    3,
                    "Subtitle 2",
                    "<p:ph type=\"subTitle\" idx=\"1\"/>",
                    Some((1_524_000, 3_602_038, 9_144_000, 1_655_762)),
                    empty,
                ),
            ]
            .concat(),
        ),
        layout(
            "Title and Content",
            "obj",
            &[
                ph(
                    2,
                    "Title 1",
                    "<p:ph type=\"title\"/>",
                    Some(TITLE_BOX),
                    empty,
                ),
                ph(
                    3,
                    "Content Placeholder 2",
                    "<p:ph type=\"body\" idx=\"1\"/>",
                    Some(BODY_BOX),
                    empty,
                ),
                ph(
                    4,
                    "Slide Number Placeholder 3",
                    "<p:ph type=\"sldNum\" sz=\"quarter\" idx=\"12\"/>",
                    Some((8_610_600, 6_356_350, 2_743_200, 365_125)),
                    empty,
                ),
            ]
            .concat(),
        ),
        layout(
            "Title Only",
            "titleOnly",
            &ph(
                2,
                "Title 1",
                "<p:ph type=\"title\"/>",
                Some(TITLE_BOX),
                empty,
            ),
        ),
    ]
}

fn master() -> String {
    let empty = "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>";
    let shapes = [
        ph(
            2,
            "Title Placeholder 1",
            "<p:ph type=\"title\"/>",
            Some(TITLE_BOX),
            empty,
        ),
        ph(
            3,
            "Text Placeholder 2",
            "<p:ph type=\"body\" idx=\"1\"/>",
            Some(BODY_BOX),
            empty,
        ),
    ]
    .concat();
    let style = |tf: &str, sz: u32| {
        format!(
            "<a:lvl1pPr><a:defRPr sz=\"{sz}\"><a:latin typeface=\"{tf}\"/></a:defRPr></a:lvl1pPr>"
        )
    };
    format!(
        "{HEAD}<p:sldMaster {}><p:cSld><p:spTree>{GRP}{shapes}</p:spTree></p:cSld><p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/><p:sldLayoutIdLst><p:sldLayoutId id=\"{TITLE_SLIDE}\" r:id=\"rId1\"/><p:sldLayoutId id=\"{TITLE_CONTENT}\" r:id=\"rId2\"/><p:sldLayoutId id=\"{TITLE_ONLY}\" r:id=\"rId3\"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle>{}</p:titleStyle><p:bodyStyle>{}</p:bodyStyle><p:otherStyle>{}</p:otherStyle></p:txStyles></p:sldMaster>",
        ns(),
        style("+mj-lt", 4400),
        style("+mn-lt", 2800),
        style("+mn-lt", 1800)
    )
}

/// The theme, with these major and minor fonts.
pub fn theme(major: &str, minor: &str) -> String {
    let c = |n: &str, v: &str| format!("<a:{n}><a:srgbClr val=\"{v}\"/></a:{n}>");
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let ln = "<a:ln w=\"6350\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>";
    let eff = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    let font = |f: &str| {
        format!(
            "<a:latin typeface=\"{}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/>",
            esc(f)
        )
    };
    format!(
        "{HEAD}<a:theme xmlns:a=\"{A}\" name=\"Golden\"><a:themeElements><a:clrScheme name=\"Golden\">{}{}{}{}{}{}{}{}{}{}{}{}</a:clrScheme><a:fontScheme name=\"Golden\"><a:majorFont>{}</a:majorFont><a:minorFont>{}</a:minorFont></a:fontScheme><a:fmtScheme name=\"Golden\"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst><a:lnStyleLst>{ln}{ln}{ln}</a:lnStyleLst><a:effectStyleLst>{eff}{eff}{eff}</a:effectStyleLst><a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
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
        font(major),
        font(minor),
    )
}

/// The table style the table on slide 3 uses.
pub fn table_styles(fill: &str) -> String {
    format!(
        "{HEAD}<a:tblStyleLst xmlns:a=\"{A}\" def=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\"><a:tblStyle styleId=\"{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}\" styleName=\"Medium Style 2\"><a:wholeTbl><a:tcStyle><a:fill><a:solidFill><a:srgbClr val=\"{fill}\"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl></a:tblStyle></a:tblStyleLst>"
    )
}

/// A slide: its id, its layout number, its part and its relationships.
type SlidePart = (u32, u32, String, Vec<(String, String, String)>);

/// Every part of the golden deck (with `extra` as a fifth slide), in a package's usual order.
pub fn parts(extra: Extra) -> Vec<(String, Vec<u8>)> {
    let mut slides: Vec<SlidePart> = vec![
        (
            256,
            1,
            slide1(),
            vec![r3(
                "rId1",
                "slideLayout",
                "../slideLayouts/slideLayout1.xml",
            )],
        ),
        (
            257,
            3,
            slide2(),
            vec![
                r3("rId1", "slideLayout", "../slideLayouts/slideLayout3.xml"),
                r3("rId2", "slide", "slide4.xml"),
            ],
        ),
        (
            258,
            3,
            slide3(),
            vec![r3(
                "rId1",
                "slideLayout",
                "../slideLayouts/slideLayout3.xml",
            )],
        ),
        (
            259,
            2,
            slide4(),
            vec![r3(
                "rId1",
                "slideLayout",
                "../slideLayouts/slideLayout2.xml",
            )],
        ),
    ];
    if let Some((id, xml)) = extra_slide(extra) {
        slides.push((
            id,
            3,
            xml,
            vec![r3(
                "rId1",
                "slideLayout",
                "../slideLayouts/slideLayout3.xml",
            )],
        ));
    }
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
    for k in 1..=3 {
        types.push((
            format!("/ppt/slideLayouts/slideLayout{k}.xml"),
            format!("{CT}slideLayout+xml"),
        ));
    }
    let mut prels = vec![
        r3("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
        r3("rId2", "presProps", "presProps.xml"),
        r3("rId3", "viewProps", "viewProps.xml"),
        r3("rId4", "theme", "theme/theme1.xml"),
        r3("rId5", "tableStyles", "tableStyles.xml"),
    ];
    let mut ids = String::new();
    let mut out: Vec<(String, String)> = Vec::new();
    out.push(("[Content_Types].xml".into(), String::new()));
    out.push((
        "_rels/.rels".into(),
        format!("{HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/></Relationships>"),
    ));
    for (n, (id, ..)) in slides.iter().enumerate() {
        let rid = format!("rId{}", n + 7);
        prels.push(r3(&rid, "slide", &format!("slides/slide{}.xml", n + 1)));
        ids.push_str(&format!("<p:sldId id=\"{id}\" r:id=\"{rid}\"/>"));
    }
    out.push((
        "ppt/presentation.xml".into(),
        format!(
            "{HEAD}<p:presentation {} saveSubsetFonts=\"1\"><p:sldMasterIdLst><p:sldMasterId id=\"{MASTER}\" r:id=\"rId1\"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx=\"12192000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/><p:defaultTextStyle><a:lvl1pPr><a:defRPr sz=\"1800\"><a:latin typeface=\"+mn-lt\"/></a:defRPr></a:lvl1pPr></p:defaultTextStyle></p:presentation>",
            ns()
        ),
    ));
    out.push(("ppt/_rels/presentation.xml.rels".into(), rels(&prels)));
    for (n, (_, _, xml, srels)) in slides.iter().enumerate() {
        out.push((format!("ppt/slides/slide{}.xml", n + 1), xml.clone()));
        out.push((
            format!("ppt/slides/_rels/slide{}.xml.rels", n + 1),
            rels(srels),
        ));
        types.push((
            format!("/ppt/slides/slide{}.xml", n + 1),
            format!("{CT}slide+xml"),
        ));
    }
    out.push(("ppt/slideMasters/slideMaster1.xml".into(), master()));
    out.push((
        "ppt/slideMasters/_rels/slideMaster1.xml.rels".into(),
        rels(&[
            r3("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
            r3("rId2", "slideLayout", "../slideLayouts/slideLayout2.xml"),
            r3("rId3", "slideLayout", "../slideLayouts/slideLayout3.xml"),
            r3("rId4", "theme", "../theme/theme1.xml"),
        ]),
    ));
    for (k, l) in layouts().into_iter().enumerate() {
        out.push((format!("ppt/slideLayouts/slideLayout{}.xml", k + 1), l));
        out.push((
            format!("ppt/slideLayouts/_rels/slideLayout{}.xml.rels", k + 1),
            rels(&[r3(
                "rId1",
                "slideMaster",
                "../slideMasters/slideMaster1.xml",
            )]),
        ));
    }
    out.push((
        "ppt/theme/theme1.xml".into(),
        theme("Liberation Sans", "Liberation Serif"),
    ));
    out.push((
        "ppt/presProps.xml".into(),
        format!("{HEAD}<p:presentationPr {}/>", ns()),
    ));
    out.push((
        "ppt/viewProps.xml".into(),
        format!("{HEAD}<p:viewPr {}/>", ns()),
    ));
    out.push(("ppt/tableStyles.xml".into(), table_styles("E9EBF5")));
    out.push((
        "docProps/core.xml".into(),
        format!("{HEAD}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Golden review</dc:title></cp:coreProperties>"),
    ));
    out.push((
        "docProps/app.xml".into(),
        format!("{HEAD}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>k2-mpc-pptx test generator</Application></Properties>"),
    ));
    let overrides: String = types
        .iter()
        .map(|(p, t)| format!("<Override PartName=\"{p}\" ContentType=\"{t}\"/>"))
        .collect();
    out[0].1 = format!(
        "{HEAD}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>{overrides}</Types>"
    );
    out.into_iter().map(|(n, c)| (n, c.into_bytes())).collect()
}

/// The golden deck's bytes.
pub fn deck(extra: Extra) -> Vec<u8> {
    stored_zip(&parts(extra))
}
