//! The refusal rows: each malformed or unsupported input is declined with its code (the file
//! then stays at tier 0), and no damage panics the reader or the index.

mod common;

use common::limits;
use k2_mpc_base::index::{IndexLimits, IndexRefused};
use k2_mpc_base::session::BlobPack;
use k2_mpc_pptx::testgen::{self, with_part};
use k2_mpc_pptx::{Pptx, PptxPack};

const PRES: &str = "ppt/presentation.xml";
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";

fn parts() -> Vec<(String, Vec<u8>)> {
    testgen::parts(&testgen::sample())
}

fn get(parts: &[(String, Vec<u8>)], name: &str) -> String {
    String::from_utf8(parts.iter().find(|(n, _)| n == name).expect(name).1.clone()).unwrap()
}

fn edit(name: &str, from: &str, to: &str) -> Vec<u8> {
    let p = parts();
    let t = get(&p, name);
    assert!(t.contains(from), "{name} holds {from:?}");
    testgen::stored_zip(&with_part(&p, name, t.replacen(from, to, 1).into_bytes()))
}

fn edit2(name: &str, a: (&str, &str), b: (&str, &str)) -> Vec<u8> {
    let p = parts();
    let t = get(&p, name);
    assert!(t.contains(a.0) && t.contains(b.0), "{name}");
    let t = t.replacen(a.0, a.1, 1).replacen(b.0, b.1, 1);
    testgen::stored_zip(&with_part(&p, name, t.into_bytes()))
}

fn code(bytes: &[u8]) -> &'static str {
    match Pptx::open(bytes, &limits()) {
        Ok(_) => "ok",
        Err(d) => d.code,
    }
}

#[test]
fn each_refusal_row_has_its_code() {
    let ct = "[Content_Types].xml";
    let main_ct = "presentationml.presentation.main+xml";
    let mut with_macros = parts();
    with_macros.push(("ppt/vbaProject.bin".into(), b"not really".to_vec()));
    let mut cfb = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    cfb.extend([0u8; 600]);
    let no_types: Vec<(String, Vec<u8>)> = parts().into_iter().filter(|(n, _)| n != ct).collect();
    let rows: Vec<(&str, Vec<u8>, &str)> = vec![
        ("a generated deck", testgen::deck(&testgen::sample()), "ok"),
        ("a VBA part", testgen::stored_zip(&with_macros), "macros"),
        (
            "a macro-enabled main part",
            edit(
                ct,
                main_ct,
                "presentationml.presentation.macroEnabled.main+xml",
            ),
            "macros",
        ),
        (
            "a template",
            edit(ct, main_ct, "presentationml.template.main+xml"),
            "template",
        ),
        (
            "a document, not a presentation",
            edit(ct, main_ct, "wordprocessingml.document.main+xml"),
            "not_pptx",
        ),
        (
            "no content types",
            testgen::stored_zip(&no_types),
            "not_pptx",
        ),
        (
            "no main part relationship",
            edit(
                "_rels/.rels",
                "relationships/officeDocument",
                "relationships/other",
            ),
            "no_presentation",
        ),
        (
            "Strict OOXML",
            edit(
                PRES,
                &format!("xmlns:p=\"{P}\""),
                "xmlns:p=\"http://purl.oclc.org/ooxml/presentationml/main\"",
            ),
            "strict",
        ),
        (
            "a slide id under 256",
            edit(PRES, "id=\"256\"", "id=\"255\""),
            "bad_slide_list",
        ),
        (
            "a slide id over the range",
            edit(PRES, "id=\"256\"", "id=\"2147483648\""),
            "bad_slide_list",
        ),
        (
            "a repeated slide id",
            edit(PRES, "id=\"257\"", "id=\"256\""),
            "bad_slide_list",
        ),
        (
            "a slide relationship that names nothing",
            edit(
                PRES,
                "<p:sldId id=\"256\" r:id=\"rId10\"/>",
                "<p:sldId id=\"256\" r:id=\"rId99\"/>",
            ),
            "bad_slide_list",
        ),
        (
            "a slide relationship to the master",
            edit(
                PRES,
                "<p:sldId id=\"256\" r:id=\"rId10\"/>",
                "<p:sldId id=\"256\" r:id=\"rId1\"/>",
            ),
            "bad_slide_list",
        ),
        (
            "a slide list with another element",
            edit(PRES, "<p:sldIdLst>", "<p:sldIdLst><p:other/>"),
            "bad_slide_list",
        ),
        (
            "a repeated presentation relationship id",
            edit(
                "ppt/_rels/presentation.xml.rels",
                "Id=\"rId3\"",
                "Id=\"rId2\"",
            ),
            "bad_rels",
        ),
        (
            "a slide without a shape tree",
            edit2(
                "ppt/slides/slide1.xml",
                ("<p:spTree>", "<p:shapes>"),
                ("</p:spTree>", "</p:shapes>"),
            ),
            "bad_slide",
        ),
        (
            "a slide whose shape tree has no properties",
            edit2(
                "ppt/slides/slide2.xml",
                ("<p:nvGrpSpPr>", "<p:x>"),
                ("</p:nvGrpSpPr>", "</p:x>"),
            ),
            "bad_slide",
        ),
        (
            "a slide that isn't XML",
            edit("ppt/slides/slide1.xml", "<p:cSld>", "<p:cSld><<"),
            "xml_invalid",
        ),
        (
            "a slide root that isn't a slide",
            edit2(
                "ppt/slides/slide1.xml",
                ("<p:sld ", "<p:notes "),
                ("</p:sld>", "</p:notes>"),
            ),
            "xml_invalid",
        ),
        (
            "a shape after the tree's extensions",
            edit(
                "ppt/slides/slide1.xml",
                "</p:spTree>",
                "<p:extLst/><p:sp/></p:spTree>",
            ),
            "bad_slide",
        ),
        (
            "a shape id that isn't a number",
            edit("ppt/slides/slide1.xml", "id=\"2\"", "id=\"two\""),
            "bad_slide",
        ),
        (
            "a DOCTYPE",
            edit("ppt/slides/slide1.xml", "<p:sld ", "<!DOCTYPE x><p:sld "),
            "doctype",
        ),
        ("an encrypted package", cfb, "encrypted"),
    ];
    let mut n = 0;
    for (what, bytes, want) in rows {
        let got = code(&bytes);
        assert_eq!(got, want, "{what}");
        n += 1;
    }
    // not a zip at all: some package code, never a panic, never ok
    assert_ne!(code(b"hello, not a deck"), "ok");
    println!("refusal rows: {n}");
}

#[test]
fn the_index_refuses_what_the_reader_declines() {
    let pack = PptxPack::default();
    let l = IndexLimits::new(100_000, 64, 64 << 20, 64 << 10);
    let bad = edit(PRES, "id=\"257\"", "id=\"256\"");
    match pack.index(&mut bad.as_slice(), bad.len() as u64, &l) {
        Err(IndexRefused::Unreadable { why }) => assert_eq!(why, "declined: bad_slide_list"),
        other => panic!("{other:?}"),
    }
    let doc = edit(
        "[Content_Types].xml",
        "presentationml.presentation.main+xml",
        "wordprocessingml.document.main+xml",
    );
    assert!(matches!(
        pack.index(&mut doc.as_slice(), doc.len() as u64, &l),
        Err(IndexRefused::Unsupported)
    ));
}

#[test]
fn damage_never_panics() {
    let l = limits();
    let pack = PptxPack::default();
    let il = IndexLimits::new(100_000, 64, 64 << 20, 64 << 10);
    let f = testgen::deck(&testgen::sample());
    for n in (0..f.len()).step_by(7) {
        let _ = Pptx::open(&f[..n], &l);
        let _ = pack.index(&mut &f[..n], n as u64, &il);
    }
    let mut opened = 0;
    for i in (0..f.len()).step_by(11) {
        for v in [0x00, 0xFF, f[i] ^ 0x20] {
            let mut g = f.clone();
            g[i] = v;
            if let Ok(x) = Pptx::open(&g, &l) {
                assert_eq!(x.encode().expect("encode"), g);
                opened += 1;
            }
            let _ = pack.index(&mut g.as_slice(), g.len() as u64, &il);
        }
    }
    assert!(opened > 0, "no flip left the file readable");
}
