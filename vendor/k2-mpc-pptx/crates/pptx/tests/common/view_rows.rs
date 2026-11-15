//! The rows of the pack's view index against the hand-made expected values, kept in these rows
//! field by field: the index at rev 40 and the cases `D`, `D2` and `D3` (the expected values are
//! hand-made; where the pack disagrees with one, the disagreement is listed in [`DISAGREE`] with
//! why, and printed).
//!
//! Each row returns `Err("<row>: ...")` listing every field that differs, so the planted faults
//! (`tests/mutants.rs`) can say which row caught them.

use k2_mpc_pptx::testgen::golden::{self, Extra};
use k2_mpc_pptx::view::{self, Bbox, Unit, ViewIndex};

use super::limits;

/// One unit as the golden gives it.
pub struct Want {
    pub key: &'static str,
    pub parent: Option<&'static str>,
    pub kind: &'static str,
    pub name: &'static str,
    pub generic: bool,
    pub z: u32,
    pub bbox: (i64, i64, i64, i64),
    pub inherited: bool,
    pub rot: i64,
    pub flip: (bool, bool),
    pub approx: bool,
    pub text: Option<&'static str>,
    pub fields: &'static [&'static str],
    pub ph: Option<(&'static str, Option<&'static str>)>,
    pub editable: bool,
}

#[allow(clippy::too_many_arguments)]
const fn w(
    key: &'static str,
    parent: Option<&'static str>,
    kind: &'static str,
    name: &'static str,
    generic: bool,
    z: u32,
    bbox: (i64, i64, i64, i64),
    inherited: bool,
    rot: i64,
    flip: (bool, bool),
    approx: bool,
    text: Option<&'static str>,
    fields: &'static [&'static str],
    ph: Option<(&'static str, Option<&'static str>)>,
    editable: bool,
) -> Want {
    Want {
        key,
        parent,
        kind,
        name,
        generic,
        z,
        bbox,
        inherited,
        rot,
        flip,
        approx,
        text,
        fields,
        ph,
        editable,
    }
}

const NF: (bool, bool) = (false, false);
const TITLE: (i64, i64, i64, i64) = (838_200, 365_125, 10_515_600, 1_325_563);

/// The expected index's units at rev 40.
pub const REV40: &[Want] = &[
    w(
        "@sld:256/sp:2",
        None,
        "placeholder",
        "Title 1",
        true,
        0,
        (1_524_000, 1_122_363, 9_144_000, 2_387_600),
        true,
        0,
        NF,
        false,
        Some("Quarterly review"),
        &[],
        Some(("ctrTitle", None)),
        true,
    ),
    w(
        "@sld:256/sp:3",
        None,
        "placeholder",
        "Subtitle 2",
        true,
        1,
        (1_524_000, 3_602_038, 9_144_000, 1_655_762),
        true,
        0,
        NF,
        false,
        Some("Team update"),
        &[],
        Some(("subTitle", Some("1"))),
        true,
    ),
    w(
        "@sld:257/sp:2",
        None,
        "placeholder",
        "Title 1",
        true,
        0,
        TITLE,
        true,
        0,
        NF,
        false,
        Some("Highlights"),
        &[],
        Some(("title", None)),
        true,
    ),
    w(
        "@sld:257/sp:4",
        None,
        "group",
        "Group 4",
        true,
        1,
        (1_000_000, 1_500_000, 4_000_000, 2_000_000),
        false,
        5_400_000,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:257/sp:5",
        Some("@sld:257/sp:4"),
        "shape",
        "Rectangle 5",
        true,
        0,
        (2_000_000, 500_000, 2_000_000, 2_000_000),
        false,
        5_400_000,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:257/sp:6",
        Some("@sld:257/sp:4"),
        "textbox",
        "Revenue callout",
        false,
        1,
        (2_000_000, 3_000_000, 2_000_000, 1_000_000),
        false,
        5_400_000,
        NF,
        false,
        Some("Revenue up 12%"),
        &[],
        None,
        false,
    ),
    w(
        "@sld:257/sp:7",
        None,
        "shape",
        "Right Arrow 7",
        true,
        2,
        (7_000_000, 2_000_000, 3_000_000, 1_000_000),
        false,
        1_800_000,
        (true, false),
        false,
        None,
        &[],
        None,
        true,
    ),
    w(
        "@sld:258/sp:2",
        None,
        "placeholder",
        "Title 1",
        true,
        0,
        TITLE,
        true,
        0,
        NF,
        false,
        Some("Regional numbers"),
        &[],
        Some(("title", None)),
        true,
    ),
    w(
        "@sld:258/sp:4",
        None,
        "table",
        "Table 3",
        true,
        1,
        (838_200, 1_825_625, 10_515_600, 3_000_000),
        false,
        0,
        NF,
        false,
        Some("Region\tQ3\tQ4\nNorth\t40\t44\nSouth\t35\t38"),
        &[],
        None,
        false,
    ),
    w(
        "@sld:259/sp:2",
        None,
        "placeholder",
        "Title 1",
        true,
        0,
        TITLE,
        true,
        0,
        NF,
        false,
        Some("Next steps"),
        &[],
        Some(("title", None)),
        true,
    ),
    w(
        "@sld:259/sp:3",
        None,
        "placeholder",
        "Content Placeholder 2",
        true,
        1,
        (838_200, 1_825_625, 10_515_600, 4_351_338),
        true,
        0,
        NF,
        false,
        Some("Hire two engineers\nShip the v2 beta"),
        &[],
        Some(("body", Some("1"))),
        true,
    ),
    w(
        "@sld:259/sp:4",
        None,
        "placeholder",
        "Slide Number Placeholder 3",
        true,
        2,
        (8_610_600, 6_356_350, 2_743_200, 365_125),
        true,
        0,
        NF,
        false,
        Some("4"),
        &["slidenum"],
        Some(("sldNum", Some("12"))),
        false,
    ),
];

/// Case `D`'s units (`@sld:261`).
pub const CASE_D: &[Want] = &[
    w(
        "@sld:261/sp:10",
        None,
        "group",
        "Group 10",
        true,
        0,
        (6_000_000, 4_000_000, 4_000_000, 1_000_000),
        false,
        0,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:261/sp:11",
        Some("@sld:261/sp:10"),
        "shape",
        "Rectangle 11",
        true,
        0,
        (6_000_000, 4_000_000, 2_000_000, 500_000),
        false,
        0,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:261/sp:12",
        Some("@sld:261/sp:10"),
        "shape",
        "Rectangle 12",
        true,
        1,
        (8_000_000, 4_500_000, 2_000_000, 500_000),
        false,
        2_700_000,
        NF,
        true,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:261/sp:13",
        Some("@sld:261/sp:10"),
        "shape",
        "Rectangle 13",
        true,
        2,
        (6_750_000, 3_750_000, 500_000, 2_000_000),
        false,
        5_400_000,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
];

/// Case `D2`'s units (`@sld:263`).
pub const CASE_D2: &[Want] = &[
    w(
        "@sld:263/sp:30",
        None,
        "group",
        "Group 30",
        true,
        0,
        (2_000_000, 1_000_000, 2_000_000, 2_000_000),
        false,
        5_400_000,
        (true, false),
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:263/sp:31",
        Some("@sld:263/sp:30"),
        "shape",
        "Rectangle 31",
        true,
        0,
        (3_250_000, 2_250_000, 1_000_000, 500_000),
        false,
        3_600_000,
        (true, false),
        false,
        None,
        &[],
        None,
        false,
    ),
];

/// Case `D2`'s variant: the group at 0°.
pub const CASE_D2_ROT0: &[Want] = &[
    w(
        "@sld:263/sp:30",
        None,
        "group",
        "Group 30",
        true,
        0,
        (2_000_000, 1_000_000, 2_000_000, 2_000_000),
        false,
        0,
        (true, false),
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:263/sp:31",
        Some("@sld:263/sp:30"),
        "shape",
        "Rectangle 31",
        true,
        0,
        (3_000_000, 1_000_000, 1_000_000, 500_000),
        false,
        19_800_000,
        (true, false),
        false,
        None,
        &[],
        None,
        false,
    ),
];

/// Case `D3`'s units (`@sld:262`).
pub const CASE_D3: &[Want] = &[
    w(
        "@sld:262/sp:20",
        None,
        "group",
        "Group 20",
        true,
        0,
        (6_000_000, 4_000_000, 4_000_000, 1_000_000),
        false,
        0,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:262/sp:21",
        Some("@sld:262/sp:20"),
        "group",
        "Group 21",
        true,
        0,
        (6_000_000, 4_000_000, 4_000_000, 1_000_000),
        false,
        2_700_000,
        NF,
        true,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:262/sp:22",
        Some("@sld:262/sp:21"),
        "shape",
        "Rectangle 22",
        true,
        0,
        (6_707_107, 4_073_223, 4_000_000, 500_000),
        false,
        2_700_000,
        NF,
        true,
        None,
        &[],
        None,
        false,
    ),
];

/// Case `D3` with `sp:22` counter-turned (315°): its angle at the scaled level is 0°, so step 6
/// alone would give an exact box; step 8 (approx inherited) flags it. Worked from the
/// geometry rule (`geom`), not from a golden: the centre and extent are case `D3`'s, `rot` is
/// 45° + 315° = 0°.
pub const CASE_D3_COUNTER: &[Want] = &[
    w(
        "@sld:262/sp:20",
        None,
        "group",
        "Group 20",
        true,
        0,
        (6_000_000, 4_000_000, 4_000_000, 1_000_000),
        false,
        0,
        NF,
        false,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:262/sp:21",
        Some("@sld:262/sp:20"),
        "group",
        "Group 21",
        true,
        0,
        (6_000_000, 4_000_000, 4_000_000, 1_000_000),
        false,
        2_700_000,
        NF,
        true,
        None,
        &[],
        None,
        false,
    ),
    w(
        "@sld:262/sp:22",
        Some("@sld:262/sp:21"),
        "shape",
        "Rectangle 22",
        true,
        0,
        (6_707_107, 4_073_223, 4_000_000, 500_000),
        false,
        0,
        NF,
        true,
        None,
        &[],
        None,
        false,
    ),
];

/// One container as the golden gives it: key, position, title, layout, fonts, has_fields,
/// unit_count.
pub type WantC = (
    &'static str,
    u32,
    &'static str,
    &'static str,
    &'static [usize],
    &'static [&'static str],
    u32,
);

pub const REV40_CONTAINERS: &[WantC] = &[
    (
        "@sld:256",
        1,
        "Quarterly review",
        "@layout:2147483649",
        &[0, 1],
        &[],
        2,
    ),
    (
        "@sld:257",
        2,
        "Highlights",
        "@layout:2147483651",
        &[0, 1, 3],
        &[],
        5,
    ),
    (
        "@sld:258",
        3,
        "Regional numbers",
        "@layout:2147483651",
        &[0, 2],
        &[],
        2,
    ),
    (
        "@sld:259",
        4,
        "Next steps",
        "@layout:2147483650",
        &[0, 1],
        &["slidenum"],
        3,
    ),
];

/// Where the pack disagrees with a hand-made golden: `(case, field, golden's, pack's, why)`.
/// The rows hold the pack to its own value and print each line.
pub const DISAGREE: &[(&str, &str, &str, &str, &str)] = &[(
    "D, D2, D3",
    "container fonts",
    "[0, 1]",
    "[1]",
    "the fifth slide has no title placeholder (the golden's own title is \"\"), so no text \
         on it falls back to the theme's major font; its shapes' style names the minor font only",
)];

fn unit<'a>(ix: &'a ViewIndex, key: &str) -> Option<&'a Unit> {
    ix.units.iter().find(|u| u.key.as_deref() == Some(key))
}

/// Every field of `want` against the pack's unit, each difference named.
pub fn diff_units(ix: &ViewIndex, want: &[Want]) -> Vec<String> {
    let mut out = Vec::new();
    for x in want {
        let Some(u) = unit(ix, x.key) else {
            out.push(format!("{}: no such unit", x.key));
            continue;
        };
        let mut d = |f: &str, got: String, want: String| {
            if got != want {
                out.push(format!("{} {f}: pack {got}, golden {want}", x.key));
            }
        };
        d(
            "parent",
            format!("{:?}", u.parent.as_deref()),
            format!("{:?}", x.parent),
        );
        d("live", u.live.to_string(), x.parent.is_none().to_string());
        d("kind", u.kind.into(), x.kind.into());
        d("name", u.name.clone(), x.name.into());
        d(
            "name_is_generic",
            u.name_is_generic.to_string(),
            x.generic.to_string(),
        );
        d("alt", format!("{:?}", u.alt), "None".into());
        d("z", u.z.to_string(), x.z.to_string());
        let (bx, by, bw, bh) = x.bbox;
        d(
            "bbox",
            format!("{:?}", u.bbox),
            format!(
                "{:?}",
                Some(Bbox {
                    x: bx,
                    y: by,
                    w: bw,
                    h: bh
                })
            ),
        );
        d(
            "bbox_inherited",
            u.bbox_inherited.to_string(),
            x.inherited.to_string(),
        );
        d("rot", u.rot.to_string(), x.rot.to_string());
        d(
            "flip",
            format!("{:?}", (u.flip_h, u.flip_v)),
            format!("{:?}", x.flip),
        );
        d("approx_box", u.approx_box.to_string(), x.approx.to_string());
        d(
            "text",
            format!(
                "{:?}",
                u.text.as_ref().map(|t| (t.summary.as_str(), t.truncated))
            ),
            format!("{:?}", x.text.map(|t| (t, false))),
        );
        d(
            "fields",
            format!("{:?}", u.fields),
            format!("{:?}", x.fields),
        );
        d("hidden", u.hidden.to_string(), "false".into());
        d(
            "placeholder",
            format!(
                "{:?}",
                u.placeholder
                    .as_ref()
                    .map(|p| (p.ty.as_str(), p.idx.as_deref()))
            ),
            format!("{:?}", x.ph),
        );
        d(
            "editable_text",
            u.editable_text.to_string(),
            x.editable.to_string(),
        );
        d(
            "container",
            u.container.clone(),
            x.key.split('/').next().unwrap_or("").into(),
        );
    }
    out
}

fn index(extra: Extra) -> Result<ViewIndex, String> {
    view::index(&golden::deck(extra), &limits()).map_err(|d| d.to_string())
}

fn fail(row: &str, diffs: Vec<String>) -> Result<(), String> {
    if diffs.is_empty() {
        Ok(())
    } else {
        Err(format!("{row}: {}", diffs.join("; ")))
    }
}

/// The expected index at rev 40, field by field: page, fonts, layouts, containers (all but the hash
/// tokens, which only must differ) and units.
pub fn row_index_rev40() -> Result<(), String> {
    const ROW: &str = "row_index_rev40";
    let ix = index(Extra::None).map_err(|e| format!("{ROW}: {e}"))?;
    let mut diffs = Vec::new();
    if ix.page != (12_192_000, 6_858_000) {
        diffs.push(format!("page {:?}", ix.page));
    }
    let fonts: Vec<(String, &str, &str, bool)> = ix
        .fonts
        .iter()
        .map(|f| (f.family.clone(), f.script, f.from, f.embedded))
        .collect();
    let want_fonts = vec![
        ("Liberation Sans".to_owned(), "latin", "theme-major", false),
        ("Liberation Serif".to_owned(), "latin", "theme-minor", false),
        ("DejaVu Sans".to_owned(), "latin", "explicit", false),
        ("Inter".to_owned(), "latin", "explicit", false),
    ];
    if fonts != want_fonts {
        diffs.push(format!("fonts {fonts:?}"));
    }
    let layouts: Vec<(String, String, Option<String>)> = ix
        .layouts
        .iter()
        .map(|l| (l.key.clone(), l.name.clone(), l.master.clone()))
        .collect();
    let m = Some("@master:2147483648".to_owned());
    let want_layouts = vec![
        (
            "@layout:2147483649".to_owned(),
            "Title Slide".to_owned(),
            m.clone(),
        ),
        (
            "@layout:2147483650".to_owned(),
            "Title and Content".to_owned(),
            m.clone(),
        ),
        ("@layout:2147483651".to_owned(), "Title Only".to_owned(), m),
    ];
    if layouts != want_layouts {
        diffs.push(format!("layouts {layouts:?}"));
    }
    if ix.containers.len() != REV40_CONTAINERS.len() {
        diffs.push(format!("{} containers", ix.containers.len()));
    }
    for (c, (key, pos, title, layout, fonts, has, count)) in
        ix.containers.iter().zip(REV40_CONTAINERS)
    {
        let got = (
            c.key.as_str(),
            c.position,
            c.title.as_str(),
            c.layout.as_deref(),
            c.fonts.as_slice(),
            c.has_fields.as_slice(),
            c.unit_count,
            c.hidden,
        );
        let want = (
            *key,
            *pos,
            *title,
            Some(*layout),
            *fonts,
            *has,
            *count,
            false,
        );
        if got != want {
            diffs.push(format!("container {got:?}, golden {want:?}"));
        }
    }
    let hashes: std::collections::BTreeSet<[u8; 32]> = ix
        .containers
        .iter()
        .map(|c| *c.render_hash.as_bytes())
        .collect();
    if hashes.len() != ix.containers.len() {
        diffs.push("two slides share a render hash".into());
    }
    let keys: Vec<Option<&str>> = ix.units.iter().map(|u| u.key.as_deref()).collect();
    let want_keys: Vec<Option<&str>> = REV40.iter().map(|x| Some(x.key)).collect();
    if keys != want_keys {
        diffs.push(format!("unit order {keys:?}"));
    }
    diffs.extend(diff_units(&ix, REV40));
    fail(ROW, diffs)
}

/// The fifth slide of a case: its units and its container (`position 5`, title `""`, Title
/// Only, no fields, the pack's fonts per [`DISAGREE`]).
fn row_case(row: &str, extra: Extra, want: &[Want], key: &str, count: u32) -> Result<(), String> {
    let ix = index(extra).map_err(|e| format!("{row}: {e}"))?;
    let mut diffs = diff_units(&ix, want);
    let units: Vec<&str> = ix
        .units
        .iter()
        .filter(|u| u.container == key)
        .filter_map(|u| u.key.as_deref())
        .collect();
    let order: Vec<&str> = want.iter().map(|x| x.key).collect();
    if units != order {
        diffs.push(format!("units {units:?}"));
    }
    match ix.containers.iter().find(|c| c.key == key) {
        None => diffs.push(format!("no container {key}")),
        Some(c) => {
            let got = (
                c.position,
                c.title.as_str(),
                c.layout.as_deref(),
                c.has_fields.len(),
                c.unit_count,
                c.fonts.as_slice(),
            );
            let want = (5, "", Some("@layout:2147483651"), 0, count, &[1usize][..]);
            if got != want {
                diffs.push(format!("container {got:?}, expected {want:?}"));
            }
        }
    }
    fail(row, diffs)
}

/// Case `D`: `approx_box` exactly on the 45° child of an unevenly scaled group; the 90° child an
/// exact box with its extent swapped.
pub fn row_case_d() -> Result<(), String> {
    row_case("row_case_d", Extra::D, CASE_D, "@sld:261", 4)
}

/// Case `D2`: under one group flip, `rot = group.rot − child.rot` (and its variant at 0°).
pub fn row_case_d2() -> Result<(), String> {
    row_case("row_case_d2", Extra::D2, CASE_D2, "@sld:263", 2)?;
    row_case("row_case_d2", Extra::D2Rot0, CASE_D2_ROT0, "@sld:263", 2)
}

/// Case `D3`: a child of an approximate group is approximate, its centre the true one.
pub fn row_case_d3() -> Result<(), String> {
    row_case("row_case_d3", Extra::D3, CASE_D3, "@sld:262", 3)
}

/// Case `D3` counter-turned: approx inherited where step 6 alone gives an exact box (step 8). This
/// is the row that catches `ApproxNotInherited`: in `D3` itself, `sp:22`'s angle at the scaled
/// level is 45°, so step 6 flags it whether or not step 8 does.
pub fn row_case_d3_counter() -> Result<(), String> {
    row_case(
        "row_case_d3_counter",
        Extra::D3Counter,
        CASE_D3_COUNTER,
        "@sld:262",
        3,
    )
}
