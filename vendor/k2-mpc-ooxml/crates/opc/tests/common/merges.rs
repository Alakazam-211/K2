//! The merge rows: two saves of one base that each add a part and pick the same next id, by
//! format; the content-type and relationship merges by key; deletes; refusals to guess; the
//! soundness check. Each leg returns `Err` with what failed, so a planted bug can be shown
//! caught by the leg written to catch it.
#![allow(dead_code)]

use k2_mpc_opc::merge::{merge3_bytes, Generic, Outcome, PartMerge, Report};
use k2_mpc_opc::rels::Relationships;
use k2_mpc_opc::zip::Package;
use k2_mpc_opc::{name, OpcPackage};

use super::{blob, limits, package_holds, slide_xml, Format, ListPack, Model, CT_PNG};

type R = Result<(), String>;
type Row = (&'static str, fn() -> R);

fn merged(
    b: &Model,
    o: &Model,
    t: &Model,
    pack: &dyn PartMerge,
) -> Result<(Vec<u8>, Report), String> {
    match merge3_bytes(&b.bytes(), &o.bytes(), &t.bytes(), pack, &limits()) {
        Outcome::Merged(x, r) => Ok((x, r)),
        Outcome::Conflict(why) => Err(format!("a conflict: {why}")),
        Outcome::Unsupported(d) => Err(format!("unsupported: {d}")),
    }
}

fn conflict(row: &str, b: &Model, o: &Model, t: &Model, pack: &dyn PartMerge, says: &str) -> R {
    match merge3_bytes(&b.bytes(), &o.bytes(), &t.bytes(), pack, &limits()) {
        Outcome::Conflict(why) if why.contains(says) => Ok(()),
        Outcome::Conflict(why) => Err(format!(
            "{row}: a conflict, but {why:?} doesn't say {says:?}"
        )),
        other => Err(format!("{row}: not a conflict: {}", short(&other))),
    }
}

fn short(o: &Outcome) -> String {
    match o {
        Outcome::Merged(_, r) => format!("merged ({r:?})"),
        Outcome::Conflict(w) => format!("conflict {w}"),
        Outcome::Unsupported(d) => format!("unsupported {d}"),
    }
}

fn content(bytes: &[u8], part: &str) -> Result<Vec<u8>, String> {
    let p = OpcPackage::open(bytes, &limits()).map_err(|e| e.to_string())?;
    p.content(&name::key(part))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| format!("no {part}"))
}

fn text(bytes: &[u8], part: &str) -> Result<String, String> {
    String::from_utf8(content(bytes, part)?).map_err(|e| e.to_string())
}

/// The relationships of `source`: (id, target) pairs.
fn rels(bytes: &[u8], source: &str) -> Result<Vec<(String, String)>, String> {
    let rp = name::rels_part_for(source);
    let c = content(bytes, &rp)?;
    let r = Relationships::parse(&rp, &c, &limits()).map_err(|e| e.to_string())?;
    Ok(r.list.into_iter().map(|x| (x.id, x.target)).collect())
}

fn check(cond: bool, what: impl FnOnce() -> String) -> R {
    if cond {
        Ok(())
    } else {
        Err(what())
    }
}

/// The docx case the shared layer exists for: two saves that each add an image pick the same
/// part name (`image2.png`) and the same relationship id (`rId3`). Before it, this was a
/// conflict on `word/_rels/document.xml.rels`.
pub fn two_images() -> R {
    let base = Model::new(Format::Docx, 0);
    let (img_o, img_t) = (blob(101, 400), blob(202, 500));
    let mut o = base.clone();
    o.add_part("word/media/image2.png", img_o.clone(), None);
    o.add_rel("word/document.xml", "rId3", "image", "media/image2.png");
    o.items
        .push("<w:p><w:r><w:t>ours' picture</w:t></w:r><a:blip r:embed=\"rId3\"/></w:p>".into());
    let mut t = base.clone();
    t.add_part("word/media/image2.png", img_t.clone(), None);
    t.add_rel("word/document.xml", "rId3", "image", "media/image2.png");
    t.items
        .push("<w:p><w:r><w:t>theirs' picture</w:t></w:r><a:blip r:embed=\"rId3\"/></w:p>".into());
    // without a content merge, both changed the document: still a conflict, but on the
    // document only, not on the relationships
    conflict(
        "two images, generic",
        &base,
        &o,
        &t,
        &Generic,
        "both sides changed word/document.xml",
    )?;
    let (m, r) = merged(&base, &o, &t, &ListPack).map_err(|e| format!("two images: {e}"))?;
    package_holds("two images", &m)?;
    check(
        r.renamed
            == vec![(
                "word/media/image2.png".to_owned(),
                "word/media/image3.png".to_owned(),
            )],
        || format!("two images: renamed {:?}", r.renamed),
    )?;
    check(
        r.reminted
            == vec![(
                "word/_rels/document.xml.rels".to_owned(),
                "rId3".to_owned(),
                "rId4".to_owned(),
            )],
        || format!("two images: re-minted {:?}", r.reminted),
    )?;
    let rl = rels(&m, "word/document.xml")?;
    let want = [("rId3", "media/image2.png"), ("rId4", "media/image3.png")];
    for (id, target) in want {
        check(rl.iter().any(|(i, x)| i == id && x == target), || {
            format!("two images: no {id} -> {target} in {rl:?}")
        })?;
    }
    check(content(&m, "word/media/image2.png")? == img_t, || {
        "two images: theirs' image moved".into()
    })?;
    check(content(&m, "word/media/image3.png")? == img_o, || {
        "two images: ours' image isn't image3".into()
    })?;
    let doc = text(&m, "word/document.xml")?;
    check(
        doc.contains("<w:t>ours' picture</w:t></w:r><a:blip r:embed=\"rId4\"/>")
            && doc.contains("<w:t>theirs' picture</w:t></w:r><a:blip r:embed=\"rId3\"/>"),
        || format!("two images: the references aren't each side's own: {doc}"),
    )?;
    // laid out as theirs: every entry theirs had and the merge didn't change is its record
    let tb = t.bytes();
    let (tz, mz) = (
        Package::read(&tb, &limits()).map_err(|e| e.to_string())?,
        Package::read(&m, &limits()).map_err(|e| e.to_string())?,
    );
    for (i, e) in tz.entries().iter().enumerate() {
        let unchanged =
            !["word/document.xml", "word/_rels/document.xml.rels"].contains(&e.name.as_str());
        if unchanged && mz.entries()[i].name == e.name && mz.record_bytes(i) != tz.record_bytes(i) {
            return Err(format!("two images: {} isn't theirs' record", e.name));
        }
        if mz.entries()[i].name != e.name {
            return Err(format!("two images: entry {i} isn't theirs' {}", e.name));
        }
    }
    check(
        mz.entries().last().map(|e| e.name.as_str()) == Some("word/media/image3.png"),
        || "two images: ours' new part isn't after theirs'".into(),
    )?;
    // a pure function of its inputs
    let (again, _) = merged(&base, &o, &t, &ListPack)?;
    check(again == m, || "two images: a second run differs".into())
}

/// Two saves that each add a slide: `slide2.xml`, its relationships part, the presentation's
/// `rId4` and a content-type override, all colliding. (The slides' `sldId`s differ here: a
/// colliding native id is the pptx pack's to re-mint, with the same rule.)
pub fn two_slides() -> R {
    let base = Model::new(Format::Pptx, 0);
    let add = |m: &mut Model, words: &str, sld: u32| {
        m.add_part(
            "ppt/slides/slide2.xml",
            slide_xml(words),
            Some("application/vnd.openxmlformats-officedocument.presentationml.slide+xml"),
        );
        m.add_rel(
            "ppt/slides/slide2.xml",
            "rId1",
            "slideLayout",
            "../slideLayouts/slideLayout1.xml",
        );
        m.add_rel("ppt/presentation.xml", "rId4", "slide", "slides/slide2.xml");
        m.items
            .push(format!("<p:sldId id=\"{sld}\" r:id=\"rId4\"/>"));
    };
    let (mut o, mut t) = (base.clone(), base.clone());
    add(&mut o, "ours' new slide", 258);
    add(&mut t, "theirs' new slide", 257);
    let (m, r) = merged(&base, &o, &t, &ListPack).map_err(|e| format!("two slides: {e}"))?;
    package_holds("two slides", &m)?;
    check(
        r.renamed
            == vec![(
                "ppt/slides/slide2.xml".to_owned(),
                "ppt/slides/slide3.xml".to_owned(),
            )],
        || format!("two slides: renamed {:?}", r.renamed),
    )?;
    let rl = rels(&m, "ppt/presentation.xml")?;
    for (id, target) in [("rId4", "slides/slide2.xml"), ("rId5", "slides/slide3.xml")] {
        check(rl.iter().any(|(i, x)| i == id && x == target), || {
            format!("two slides: no {id} -> {target} in {rl:?}")
        })?;
    }
    check(
        text(&m, "ppt/slides/slide3.xml")?.contains("ours' new slide"),
        || "two slides: slide3 isn't ours'".into(),
    )?;
    check(
        text(&m, "ppt/slides/slide2.xml")?.contains("theirs' new slide"),
        || "two slides: slide2 isn't theirs'".into(),
    )?;
    // ours' slide kept its own relationships part, under its new name
    let l3 = rels(&m, "ppt/slides/slide3.xml")?;
    check(
        l3 == vec![(
            "rId1".to_owned(),
            "../slideLayouts/slideLayout1.xml".to_owned(),
        )],
        || format!("two slides: slide3's rels {l3:?}"),
    )?;
    let types = text(&m, "[Content_Types].xml")?;
    check(
        types.contains("PartName=\"/ppt/slides/slide2.xml\"")
            && types.contains("PartName=\"/ppt/slides/slide3.xml\""),
        || format!("two slides: content types {types}"),
    )?;
    let pres = text(&m, "ppt/presentation.xml")?;
    check(
        pres.contains("<p:sldId id=\"257\" r:id=\"rId4\"/>")
            && pres.contains("<p:sldId id=\"258\" r:id=\"rId5\"/>"),
        || format!("two slides: presentation {pres}"),
    )
}

/// Two saves that each add a sheet: `sheet2.xml`, the workbook's `rId4` and an override. (The
/// `sheetId`s differ here: a colliding native id is the xlsx pack's to re-mint.)
pub fn two_sheets() -> R {
    let base = Model::new(Format::Xlsx, 1);
    let add = |m: &mut Model, v: u32, sheet_id: u32, sheet: &str| {
        m.add_part(
            "xl/worksheets/sheet2.xml",
            format!("<?xml version=\"1.0\"?><worksheet xmlns=\"{}\"><sheetData><row r=\"1\"><c r=\"A1\"><v>{v}</v></c></row></sheetData></worksheet>", super::S).into_bytes(),
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"),
        );
        m.add_rel(
            "xl/workbook.xml",
            "rId4",
            "worksheet",
            "worksheets/sheet2.xml",
        );
        m.items.push(format!(
            "<sheet name=\"{sheet}\" sheetId=\"{sheet_id}\" r:id=\"rId4\"/>"
        ));
    };
    let (mut o, mut t) = (base.clone(), base.clone());
    add(&mut o, 111, 3, "Ours");
    add(&mut t, 222, 2, "Theirs");
    let (m, r) = merged(&base, &o, &t, &ListPack).map_err(|e| format!("two sheets: {e}"))?;
    package_holds("two sheets", &m)?;
    check(r.reminted.len() == 1, || {
        format!("two sheets: re-minted {:?}", r.reminted)
    })?;
    check(
        text(&m, "xl/worksheets/sheet3.xml")?.contains("<v>111</v>"),
        || "two sheets: sheet3 isn't ours'".into(),
    )?;
    check(
        text(&m, "xl/worksheets/sheet2.xml")?.contains("<v>222</v>"),
        || "two sheets: sheet2 isn't theirs'".into(),
    )?;
    let wb = text(&m, "xl/workbook.xml")?;
    check(
        wb.contains("name=\"Theirs\" sheetId=\"2\" r:id=\"rId4\"")
            && wb.contains("name=\"Ours\" sheetId=\"3\" r:id=\"rId5\""),
        || format!("two sheets: workbook {wb}"),
    )
}

/// Both saves add the same image the same way: one part, one relationship, nothing renamed.
pub fn same_image_twice() -> R {
    let base = Model::new(Format::Docx, 0);
    let img = blob(77, 300);
    let mut o = base.clone();
    o.add_part("word/media/image2.png", img.clone(), None);
    o.add_rel("word/document.xml", "rId3", "image", "media/image2.png");
    o.items.push("<w:p><a:blip r:embed=\"rId3\"/></w:p>".into());
    let mut t = o.clone();
    t.set_part(
        "word/styles.xml",
        b"<?xml version=\"1.0\"?><w:styles xmlns:w=\"urn:x\"/>".to_vec(),
    );
    let (m, r) = merged(&base, &o, &t, &ListPack).map_err(|e| format!("same image: {e}"))?;
    check(r == Report::default(), || format!("same image: {r:?}"))?;
    check(m == t.bytes(), || "same image: not theirs' file".into())
}

/// Content types merge by key: each side's new `Default` lands; one side's new override lands;
/// a key both sides set differently is a conflict.
pub fn content_types() -> R {
    let base = Model::new(Format::Docx, 0);
    let mut o = base.clone();
    o.defaults.push(("jpeg".into(), "image/jpeg".into()));
    o.add_part("word/media/photo1.jpeg", blob(5, 50), None);
    o.add_rel("word/document.xml", "rId3", "image", "media/photo1.jpeg");
    let mut t = base.clone();
    t.defaults.push(("gif".into(), "image/gif".into()));
    t.add_part("word/media/anim1.gif", blob(6, 50), None);
    t.add_rel("word/document.xml", "rId3", "image", "media/anim1.gif");
    let (m, r) = merged(&base, &o, &t, &Generic).map_err(|e| format!("content types: {e}"))?;
    package_holds("content types", &m)?;
    check(r.reminted.len() == 1 && r.renamed.is_empty(), || {
        format!("content types: {r:?}")
    })?;
    let ty = text(&m, "[Content_Types].xml")?;
    check(
        ty.contains("Extension=\"jpeg\"")
            && ty.contains("Extension=\"gif\"")
            && ty.contains(CT_PNG),
        || format!("content types: {ty}"),
    )?;
    // one key, two values
    let mut o2 = base.clone();
    o2.defaults.push(("bin".into(), "application/x-one".into()));
    let mut t2 = base.clone();
    t2.defaults.push(("bin".into(), "application/x-two".into()));
    t2.set_part(
        "word/styles.xml",
        b"<?xml version=\"1.0\"?><w:styles xmlns:w=\"urn:x\"/>".to_vec(),
    );
    conflict(
        "a content type set two ways",
        &base,
        &o2,
        &t2,
        &Generic,
        "content type of bin",
    )
}

/// A relationship both sides retargeted differently is a conflict; one side's retarget lands.
pub fn relationship_changes() -> R {
    let base = Model::new(Format::Docx, 0);
    let retarget = |m: &mut Model, to: &str| {
        let l = m.rels.get_mut("word/document.xml").expect("rels");
        l.iter_mut().find(|r| r.0 == "rId2").expect("rId2").2 = to.to_owned();
        m.add_part(&format!("word/{to}"), blob(9, 40), None);
    };
    let (mut o, mut t) = (base.clone(), base.clone());
    retarget(&mut o, "media/ours.png");
    retarget(&mut t, "media/theirs.png");
    conflict(
        "one relationship retargeted two ways",
        &base,
        &o,
        &t,
        &Generic,
        "relationship rId2",
    )?;
    // one side's retarget, the other's new relationship
    let mut t = base.clone();
    t.add_part("word/media/image5.png", blob(10, 40), None);
    t.add_rel("word/document.xml", "rId9", "image", "media/image5.png");
    let (m, _) = merged(&base, &o, &t, &Generic).map_err(|e| format!("retarget: {e}"))?;
    package_holds("retarget", &m)?;
    let rl = rels(&m, "word/document.xml")?;
    check(
        rl.contains(&("rId2".into(), "media/ours.png".into()))
            && rl.contains(&("rId9".into(), "media/image5.png".into())),
        || format!("retarget: {rl:?}"),
    )
}

/// A part, its relationship and its override that one side deleted go; deleted against
/// changed is a conflict.
pub fn deletes() -> R {
    let base = Model::new(Format::Docx, 0);
    let mut o = base.clone();
    o.remove_part("word/styles.xml");
    o.remove_rel("word/document.xml", "rId1");
    let mut t = base.clone();
    t.items.push("<w:p><w:r><w:t>new</w:t></w:r></w:p>".into());
    let (m, _) = merged(&base, &o, &t, &Generic).map_err(|e| format!("deletes: {e}"))?;
    package_holds("deletes", &m)?;
    let p = OpcPackage::open(&m, &limits()).map_err(|e| e.to_string())?;
    check(p.content("word/styles.xml").is_none(), || {
        "deletes: styles came back".into()
    })?;
    check(
        text(&m, "word/document.xml")?.contains("<w:t>new</w:t>"),
        || "deletes: theirs' edit lost".into(),
    )?;
    let mut t2 = base.clone();
    t2.set_part(
        "word/styles.xml",
        b"<?xml version=\"1.0\"?><w:styles xmlns:w=\"urn:x\"/>".to_vec(),
    );
    conflict(
        "deleted against changed",
        &base,
        &o,
        &t2,
        &Generic,
        "the other removed it",
    )
}

/// The package's own relationships: both add `rId4`; ours' is re-minted, with no part to
/// rewrite.
pub fn package_relationships() -> R {
    let base = Model::new(Format::Docx, 0);
    let mut o = base.clone();
    o.add_part(
        "docProps/custom.xml",
        b"<?xml version=\"1.0\"?><Properties xmlns=\"urn:x\"/>".to_vec(),
        Some("application/vnd.openxmlformats-officedocument.custom-properties+xml"),
    );
    o.add_rel("", "rId4", "custom-properties", "docProps/custom.xml");
    let mut t = base.clone();
    t.defaults.push(("jpeg".into(), "image/jpeg".into()));
    t.add_part("docProps/thumbnail.jpeg", blob(3, 64), None);
    t.add_rel("", "rId4", "thumbnail", "docProps/thumbnail.jpeg");
    let (m, r) = merged(&base, &o, &t, &Generic).map_err(|e| format!("package rels: {e}"))?;
    package_holds("package rels", &m)?;
    check(
        r.reminted == vec![("_rels/.rels".into(), "rId4".into(), "rId5".into())],
        || format!("package rels: {r:?}"),
    )?;
    let rl = rels(&m, "")?;
    check(
        rl.contains(&("rId4".into(), "docProps/thumbnail.jpeg".into()))
            && rl.contains(&("rId5".into(), "docProps/custom.xml".into())),
        || format!("package rels: {rl:?}"),
    )
}

/// A colliding id that ours' part also holds in an attribute that isn't a relationship
/// reference: which uses are references can't be told, so it's a conflict, never a guess.
pub fn unknown_reference() -> R {
    let base = Model::new(Format::Docx, 0);
    let mut o = base.clone();
    o.add_part("word/media/image2.png", blob(1, 40), None);
    o.add_rel("word/document.xml", "rId3", "image", "media/image2.png");
    o.items
        .push("<w:p w:tag=\"rId3\"><a:blip r:embed=\"rId3\"/></w:p>".into());
    let mut t = base.clone();
    t.add_part("word/media/image2.png", blob(2, 40), None);
    t.add_rel("word/document.xml", "rId3", "image", "media/image2.png");
    t.items.push("<w:p><a:blip r:embed=\"rId3\"/></w:p>".into());
    conflict(
        "an id in another attribute",
        &base,
        &o,
        &t,
        &ListPack,
        "isn't a relationship reference",
    )
}

/// A part both sides changed, with no content merge for it: a conflict.
pub fn both_changed_part() -> R {
    let base = Model::new(Format::Xlsx, 0);
    let (mut o, mut t) = (base.clone(), base.clone());
    o.set_part(
        "xl/styles.xml",
        b"<?xml version=\"1.0\"?><styleSheet xmlns=\"urn:o\"/>".to_vec(),
    );
    t.set_part(
        "xl/styles.xml",
        b"<?xml version=\"1.0\"?><styleSheet xmlns=\"urn:t\"/>".to_vec(),
    );
    conflict(
        "both changed styles",
        &base,
        &o,
        &t,
        &Generic,
        "both sides changed xl/styles.xml",
    )
}

/// The identities: an unchanged side gives back the other side's bytes.
pub fn identities() -> R {
    let (b, o) = (
        Model::new(Format::Pptx, 0).bytes(),
        Model::new(Format::Pptx, 1).bytes(),
    );
    let l = limits();
    let is = |x: Outcome, want: &[u8]| matches!(x, Outcome::Merged(m, _) if m == want);
    check(is(merge3_bytes(&b, &b, &b, &Generic, &l), &b), || {
        "b,b,b".into()
    })?;
    check(is(merge3_bytes(&b, &b, &o, &Generic, &l), &o), || {
        "b,b,o".into()
    })?;
    check(is(merge3_bytes(&b, &o, &b, &Generic, &l), &o), || {
        "b,o,b".into()
    })?;
    match merge3_bytes(&b, &o, b"not a package", &Generic, &l) {
        Outcome::Unsupported(d) if d.code == "not_zip" => Ok(()),
        other => Err(format!("a damaged side: {}", short(&other))),
    }
}

/// The soundness check: a merge that would hold a problem neither side has is a conflict,
/// and one a side already has doesn't stop a merge.
pub fn soundness() -> R {
    // theirs refers again to the relationship ours deleted, with its part
    let base = Model::new(Format::Docx, 0);
    let mut o = base.clone();
    o.items.retain(|i| !i.contains("rId2"));
    o.remove_rel("word/document.xml", "rId2");
    o.remove_part("word/media/image1.png");
    let mut t = base.clone();
    t.items.push("<w:p><a:blip r:embed=\"rId2\"/></w:p>".into());
    conflict(
        "a reference to a deleted relationship",
        &base,
        &o,
        &t,
        &ListPack,
        "missing relationship rId2",
    )?;
    // theirs retargets a relationship to a part ours deleted
    let mut base2 = Model::new(Format::Docx, 0);
    base2.add_part("word/media/image9.png", blob(4, 40), None);
    let mut o2 = base2.clone();
    o2.remove_part("word/media/image9.png");
    let mut t2 = base2.clone();
    t2.rels.get_mut("word/document.xml").expect("rels")[1].2 = "media/image9.png".into();
    conflict(
        "a retarget to a deleted part",
        &base2,
        &o2,
        &t2,
        &Generic,
        "targets missing",
    )?;
    // theirs already has a part with no content type: ours' edit still merges
    let mut base3 = Model::new(Format::Docx, 0);
    base3.add_part("word/notes.dat", b"x".to_vec(), None);
    let mut o3 = base3.clone();
    o3.items
        .push("<w:p><w:r><w:t>ours</w:t></w:r></w:p>".into());
    let mut t3 = base3.clone();
    t3.set_part(
        "word/styles.xml",
        b"<?xml version=\"1.0\"?><w:styles xmlns:w=\"urn:x\"/>".to_vec(),
    );
    merged(&base3, &o3, &t3, &Generic)
        .map(|_| ())
        .map_err(|e| format!("a known problem: {e}"))
}

/// Every named merge row.
pub fn named() -> R {
    let rows: [Row; 13] = [
        ("two images", two_images),
        ("two slides", two_slides),
        ("two sheets", two_sheets),
        ("same image twice", same_image_twice),
        ("content types", content_types),
        ("relationship changes", relationship_changes),
        ("deletes", deletes),
        ("package relationships", package_relationships),
        ("unknown reference", unknown_reference),
        ("both changed a part", both_changed_part),
        ("identities", identities),
        ("soundness", soundness),
        ("fixtures merge with themselves", self_merges),
    ];
    let mut bad = Vec::new();
    for (n, f) in rows {
        if let Err(e) = f() {
            bad.push(format!("{n}: {e}"));
        }
    }
    if bad.is_empty() {
        println!("merge rows: {} named rows hold", rows.len());
        Ok(())
    } else {
        Err(bad.join("\n"))
    }
}

/// Each format's fixtures, edited on both sides apart: the merge holds both edits, and every
/// part neither side changed is theirs' record.
pub fn self_merges() -> R {
    for f in Format::ALL {
        let base = Model::new(f, 0);
        let mut o = base.clone();
        o.items.push(match f {
            Format::Docx => "<w:p><w:r><w:t>ours</w:t></w:r></w:p>".to_owned(),
            Format::Pptx => "<p:sldId id=\"300\" r:id=\"rId2\"/>".to_owned(),
            Format::Xlsx => "<sheet name=\"Alias\" sheetId=\"9\" r:id=\"rId1\"/>".to_owned(),
        });
        let mut t = base.clone();
        let other = t.parts[0].0.clone();
        let mut c = t.parts[0].1.clone();
        c.extend_from_slice(b"<!-- theirs -->");
        t.set_part(&other, c.clone());
        let (m, _) = merged(&base, &o, &t, &Generic).map_err(|e| format!("{}: {e}", f.name()))?;
        package_holds(f.name(), &m)?;
        check(content(&m, &other)? == c, || {
            format!("{}: theirs' edit lost", f.name())
        })?;
        check(content(&m, f.main())? == o.main_xml().into_bytes(), || {
            format!("{}: ours' edit lost", f.name())
        })?;
    }
    Ok(())
}
