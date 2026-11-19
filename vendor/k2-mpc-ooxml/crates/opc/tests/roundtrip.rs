//! Byte-exact round trips on our own generated packages (docx, pptx and xlsx shapes, eight zip
//! layouts each), and the splicing writer: it changes only what it was asked to.

mod common;

use std::collections::BTreeMap;

use common::{fixtures, fixtures_hold, limits, plans_hold, second_reader};
use k2_mpc_opc::zip::Package;
use k2_mpc_opc::{name, Change, OpcPackage};

#[test]
fn every_fixture_round_trips_byte_for_byte() {
    if let Err(e) = fixtures_hold() {
        panic!("{e}");
    }
}

/// write_plan_applied_equals_write: on every fixture, for each change set, the write's plan
/// (ranges in order, apart, inside the file) applied to the file is the record-by-record write,
/// and `write` is the same. Mutant: `opc_plan_skips_gap`.
#[test]
fn write_plan_applied_equals_write() {
    if let Err(e) = plans_hold() {
        panic!("{e}");
    }
}

#[test]
fn the_fixtures_differ_in_layout() {
    // eight layouts give eight different files of one model
    let all = fixtures();
    let docx0: Vec<&Vec<u8>> = all
        .iter()
        .filter(|(n, _)| n.starts_with("docx-") && n.ends_with("-0"))
        .map(|(_, b)| b)
        .collect();
    assert_eq!(docx0.len(), 8);
    for i in 0..docx0.len() {
        for j in 0..i {
            assert_ne!(docx0[i], docx0[j], "layouts {i} and {j} gave one file");
        }
    }
}

#[test]
fn an_unchanged_write_is_the_file() {
    let l = limits();
    for (n, b) in fixtures() {
        let p = OpcPackage::open(&b, &l).expect("opens");
        assert_eq!(p.write(&BTreeMap::new()).expect("write"), b, "{n}");
        // a replacement with the same content keeps the raw record
        let mut same = BTreeMap::new();
        let main = p
            .parts
            .keys()
            .find(|k| {
                k.ends_with("document.xml")
                    || k.ends_with("presentation.xml")
                    || k.ends_with("workbook.xml")
            })
            .expect("main")
            .clone();
        same.insert(
            main.clone(),
            Change::Replace(p.parts[&main].content.clone()),
        );
        assert_eq!(p.write(&same).expect("write"), b, "{n}");
    }
}

#[test]
fn the_writer_changes_only_what_it_was_asked_to() {
    let l = limits();
    for (n, b) in fixtures() {
        let p = OpcPackage::open(&b, &l).expect("opens");
        let main = p
            .parts
            .keys()
            .find(|k| {
                k.ends_with("document.xml")
                    || k.ends_with("presentation.xml")
                    || k.ends_with("workbook.xml")
            })
            .expect("main")
            .clone();
        let mut edited = p.parts[&main].content.clone();
        edited.extend_from_slice(b"<!-- edited -->");
        let mut changes = BTreeMap::new();
        changes.insert(main.clone(), Change::Replace(edited.clone()));
        changes.insert(name::key("docProps/app.xml"), Change::Remove);
        changes.insert(
            name::key("customXml/item9.xml"),
            Change::Add {
                name: "customXml/item9.xml".into(),
                content: b"<?xml version=\"1.0\"?><item/>".to_vec(),
            },
        );
        let out = p.write(&changes).expect("write");
        // deterministic
        assert_eq!(p.write(&changes).expect("write"), out, "{n}");
        let q = Package::read(&out, &l).expect("reads back");
        // every untouched entry is the same record, in the same order
        let mut j = 0;
        for (i, e) in p.zip.entries().iter().enumerate() {
            let k = name::key(&e.name);
            if k == name::key("docProps/app.xml") {
                continue;
            }
            assert_eq!(q.entries()[j].name, e.name, "{n}: order");
            if k == main {
                assert_eq!(q.inflate(j).expect("inflate"), edited, "{n}");
                assert_eq!(q.entries()[j].method, e.method, "{n}: method kept");
            } else {
                assert_eq!(q.record_bytes(j), p.zip.record_bytes(i), "{n}: {}", e.name);
            }
            j += 1;
        }
        assert_eq!(
            q.entries()[j].name,
            "customXml/item9.xml",
            "{n}: added at the end"
        );
        assert_eq!(j + 1, q.entries().len(), "{n}");
        // the second reader agrees
        let theirs = second_reader(&out).expect("second reader");
        assert!(theirs
            .iter()
            .any(|(x, c)| x == "customXml/item9.xml" && c.starts_with(b"<?xml")));
        assert!(!theirs.iter().any(|(x, _)| x == "docProps/app.xml"));
        // and it reopens: written back to itself
        let r = OpcPackage::open(&out, &l).expect("reopens");
        assert_eq!(r.encode().expect("encode"), out);
    }
}

#[test]
fn the_writer_refuses_what_it_cannot_do() {
    let l = limits();
    let (_, b) = &fixtures()[0];
    let p = OpcPackage::open(b, &l).expect("opens");
    let one = |k: &str, c: Change| {
        let mut m = BTreeMap::new();
        m.insert(k.to_owned(), c);
        p.write(&m).map(|_| ()).map_err(|e| e.code)
    };
    assert_eq!(one("not/there.xml", Change::Remove), Err("zip_corrupt"));
    assert_eq!(
        one(
            "word/document.xml",
            Change::Add {
                name: "word/document.xml".into(),
                content: vec![]
            }
        ),
        Err("zip_duplicate")
    );
    assert_eq!(
        one(
            "a/../b.xml",
            Change::Add {
                name: "a/../b.xml".into(),
                content: vec![]
            }
        ),
        Err("zip_name")
    );
    assert_eq!(
        one(
            "x.xml",
            Change::Add {
                name: "y.xml".into(),
                content: vec![]
            }
        ),
        Err("zip_name")
    );
}
