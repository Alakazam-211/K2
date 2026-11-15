//! The render hash's rows, on the golden sample deck: which slides' hashes change at each frame of
//! the golden turn (revs 41 to 46; the expected hashes are listed in these rows) and in the
//! golden's variants `V1`, `V2`, `V3`, `V5` and `V6`. "Events say where to look; the hash says
//! whether to re-render": a slide's hash changes exactly when something in its render closure does.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_pptx::testgen::golden::{self, Extra, TITLE_CONTENT};
use k2_mpc_pptx::testgen::{stored_zip, with_part};
use k2_mpc_pptx::verbs::{apply, Edit, LayoutSel, Place};
use k2_mpc_pptx::view;

use super::limits;

type Hashes = BTreeMap<String, [u8; 32]>;

fn hashes(bytes: &[u8]) -> Result<Hashes, String> {
    let ix = view::index(bytes, &limits()).map_err(|d| d.to_string())?;
    Ok(ix
        .containers
        .iter()
        .map(|c| (c.key.clone(), *c.render_hash.as_bytes()))
        .collect())
}

/// The slides whose hash differs between two decks, or that only the second has.
fn changed(a: &Hashes, b: &Hashes) -> BTreeSet<String> {
    b.iter()
        .filter(|(k, h)| a.get(*k) != Some(*h))
        .map(|(k, _)| k.clone())
        .collect()
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| (*k).to_owned()).collect()
}

fn edit(b: &[u8], e: Edit) -> Result<Vec<u8>, String> {
    apply(b, &e, &limits())
        .map(|a| a.bytes)
        .map_err(|e| e.to_string())
}

fn check(row: &str, what: &str, a: &[u8], b: &[u8], want: &[&str]) -> Result<(), String> {
    let got = changed(&hashes(a)?, &hashes(b)?);
    if got == set(want) {
        Ok(())
    } else {
        Err(format!(
            "{row}: {what}: hashes changed {got:?}, expected {want:?}"
        ))
    }
}

fn text(slide: u32, shape: u32, t: &str) -> Edit {
    Edit::SetText {
        slide,
        shape,
        text: t.into(),
    }
}

/// The golden turn, frame by frame (revs 41 to 46).
pub fn row_turn() -> Result<(), String> {
    const ROW: &str = "row_turn";
    let r40 = golden::deck(Extra::None);
    let r41 = edit(&r40, text(256, 2, "Q4 review"))?;
    check(ROW, "rev 41 shape text", &r40, &r41, &["@sld:256"])?;
    let r42 = edit(
        &r41,
        Edit::MoveShape {
            slide: 257,
            shape: 7,
            x: Some(7_500_000),
            y: Some(4_000_000),
            w: None,
            h: None,
        },
    )?;
    check(ROW, "rev 42 shape move", &r41, &r42, &["@sld:257"])?;
    let r43 = edit(
        &r42,
        Edit::AddSlide {
            layout: LayoutSel::Id(TITLE_CONTENT),
            place: Some(Place::After(259)),
        },
    )?;
    check(
        ROW,
        "rev 43 slide add at the end",
        &r42,
        &r43,
        &["@sld:260"],
    )?;
    let r44 = edit(&r43, text(260, 2, "Risks"))?;
    check(ROW, "rev 44 shape text", &r43, &r44, &["@sld:260"])?;
    let r45 = edit(
        &r44,
        Edit::MoveSlide {
            slide: 259,
            place: Place::After(257),
        },
    )?;
    check(
        ROW,
        "rev 45 slide move (the slide-number case)",
        &r44,
        &r45,
        &["@sld:259"],
    )?;
    let r46 = edit(&r45, text(260, 3, "Supplier delays\nHiring pace"))?;
    check(ROW, "rev 46 shape text", &r45, &r46, &["@sld:260"])
}

/// Variant `V1`: `slide add --first` from rev 42: every slide shifts, only 259 shows its number.
pub fn row_v1_slide_added_first() -> Result<(), String> {
    const ROW: &str = "row_v1_slide_added_first";
    let r40 = golden::deck(Extra::None);
    let r41 = edit(&r40, text(256, 2, "Q4 review"))?;
    let r42 = edit(
        &r41,
        Edit::MoveShape {
            slide: 257,
            shape: 7,
            x: Some(7_500_000),
            y: Some(4_000_000),
            w: None,
            h: None,
        },
    )?;
    let v1 = edit(
        &r42,
        Edit::AddSlide {
            layout: LayoutSel::Id(TITLE_CONTENT),
            place: Some(Place::First),
        },
    )?;
    check(ROW, "V1", &r42, &v1, &["@sld:260", "@sld:259"])
}

fn variant(part: &str, from: &str, to: &str) -> Result<Vec<u8>, String> {
    let parts = golden::parts(Extra::None);
    let old = parts
        .iter()
        .find(|(n, _)| n == part)
        .map(|(_, c)| String::from_utf8_lossy(c).into_owned())
        .ok_or_else(|| format!("no part {part}"))?;
    if !old.contains(from) {
        return Err(format!("{part} doesn't hold {from:?}"));
    }
    Ok(stored_zip(&with_part(
        &parts,
        part,
        old.replacen(from, to, 1).into_bytes(),
    )))
}

/// Variant `V2`: a save that changes only `tableStyles.xml`: only the slide with a table.
pub fn row_v2_table_style() -> Result<(), String> {
    const ROW: &str = "row_v2_table_style";
    let v2 =
        variant("ppt/tableStyles.xml", "E9EBF5", "F5E9E9").map_err(|e| format!("{ROW}: {e}"))?;
    check(ROW, "V2", &golden::deck(Extra::None), &v2, &["@sld:258"])
}

/// Variant `V3`: a save that changes only the Title Only layout: its two slides, never the
/// others under the same master.
pub fn row_v3_one_layout() -> Result<(), String> {
    const ROW: &str = "row_v3_one_layout";
    let v3 = variant(
        "ppt/slideLayouts/slideLayout3.xml",
        "preserve=\"1\"",
        "preserve=\"1\" userDrawn=\"1\"",
    )
    .map_err(|e| format!("{ROW}: {e}"))?;
    check(
        ROW,
        "V3",
        &golden::deck(Extra::None),
        &v3,
        &["@sld:257", "@sld:258"],
    )
}

/// Variant `V5`: a save that changes only the theme: every slide.
pub fn row_v5_theme() -> Result<(), String> {
    const ROW: &str = "row_v5_theme";
    let v5 =
        variant("ppt/theme/theme1.xml", "4472C4", "4472C5").map_err(|e| format!("{ROW}: {e}"))?;
    check(
        ROW,
        "V5",
        &golden::deck(Extra::None),
        &v5,
        &["@sld:256", "@sld:257", "@sld:258", "@sld:259"],
    )
}

/// Variant `V6`: an edit of slide 259, which slide 257's arrow links to: only 259.
pub fn row_v6_linked_slide() -> Result<(), String> {
    const ROW: &str = "row_v6_linked_slide";
    let r40 = golden::deck(Extra::None);
    let v6 = edit(&r40, text(259, 3, "Hire three engineers"))?;
    check(ROW, "V6", &r40, &v6, &["@sld:259"])
}
