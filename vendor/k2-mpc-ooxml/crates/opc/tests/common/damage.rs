//! The refusal rows: damaged and hostile packages, each with the code it must be refused
//! with. Built by the independent writer (`zipgen`), then patched by offset.
#![allow(dead_code)]

use k2_mpc_opc::{Limits, OpcPackage};

use super::zipgen::{build, Desc, ZEntry};
use super::{package, Format, Layout, Model};

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The central directory's offset (a package with no archive comment).
fn cd_off(b: &[u8]) -> usize {
    u32_at(b, b.len() - 22 + 16) as usize
}

fn base_entries() -> Vec<ZEntry> {
    Model::new(Format::Docx, 0)
        .render()
        .iter()
        .map(|(n, c)| ZEntry::new(n, c))
        .collect()
}

fn with_entry(e: ZEntry) -> Vec<u8> {
    let mut v = base_entries();
    v.push(e);
    build(&v, b"")
}

/// (row, bytes, the limits, the code it must be refused with).
pub fn rows() -> Vec<(&'static str, Vec<u8>, Limits, &'static str)> {
    let d = Limits::default();
    let good = package(&Model::new(Format::Docx, 0).render(), Layout::Plain, 0);
    let mut rows: Vec<(&'static str, Vec<u8>, Limits, &'static str)> = Vec::new();
    let mut add = |n, b, c| rows.push((n, b, d, c));

    add(
        "not a zip",
        b"plain text, not a package at all".to_vec(),
        "not_zip",
    );
    add("empty", Vec::new(), "not_zip");
    add(
        "truncated to half",
        good[..good.len() / 2].to_vec(),
        "not_zip",
    );
    let mut g = good.clone();
    g.extend_from_slice(b"xx");
    add("bytes after the end record", g, "zip_layout");
    let mut g = b"PREFIX".to_vec();
    g.extend_from_slice(&good);
    add("bytes before the first record", g, "zip_layout");
    // a gap between two records
    let mut v = base_entries();
    v[0].junk_after = b"JUNK".to_vec();
    add("a gap between records", build(&v, b""), "zip_layout");
    // a record nested inside another's data: the classic overlapping bomb
    let mut v = base_entries();
    let mut inner = ZEntry::new("word/inner.xml", b"<x/>");
    inner.method = 0;
    let mut outer = ZEntry::new("word/outer.bin", b"");
    outer.method = 0;
    let prefix = b"0123".to_vec();
    outer.content = [prefix.clone(), inner.local_record()].concat();
    let at: u32 = v.iter().map(|e| e.local_record().len() as u32).sum::<u32>()
        + 30
        + outer.name.len() as u32
        + prefix.len() as u32;
    inner.nested_at = Some(at);
    v.push(outer);
    v.push(inner);
    add(
        "a record inside another's data",
        build(&v, b""),
        "zip_overlap",
    );
    // a local header naming another file
    let mut g = good.clone();
    g[30] ^= 0x20;
    add(
        "a local header naming another file",
        g,
        "zip_local_mismatch",
    );
    // a data descriptor that disagrees with the directory
    let mut v = base_entries();
    v[1].desc = Desc::Sig;
    let at = v[0].local_record().len() + 30 + v[1].name.len() + v[1].data().len() + 4;
    let mut g = build(&v, b"");
    g[at] ^= 0xFF;
    add("a lying data descriptor", g, "zip_descriptor");
    // zip64 in its forms
    let mut e = ZEntry::new("word/z.xml", b"<z/>");
    e.local_extra = vec![0x01, 0x00, 0x00, 0x00];
    e.central_extra = vec![0x01, 0x00, 0x00, 0x00];
    add("a zip64 extra field", with_entry(e), "zip64");
    let mut g = good.clone();
    let n = g.len();
    g[n - 22 + 10] = 0xFF;
    g[n - 22 + 11] = 0xFF;
    g[n - 22 + 8] = 0xFF;
    g[n - 22 + 9] = 0xFF;
    add("a zip64 entry count", g, "zip64");
    let mut g = good.clone();
    let n = g.len();
    g[n - 22 + 4] = 1;
    add("a split archive", g, "zip_multi_disk");
    // flags and methods
    let mut g = good.clone();
    let c = cd_off(&g);
    g[c + 8] |= 1;
    add("the encryption flag", g, "encrypted");
    let mut g = good.clone();
    let c = cd_off(&g);
    g[c + 10] = 12;
    g[8] = 12;
    add("bzip2", g, "zip_method");
    // names
    for (row, name) in [
        ("a name that climbs out", "../evil.xml"),
        ("an absolute name", "/evil.xml"),
        ("a backslash", "word\\evil.xml"),
        ("a control character", "word/ev\u{1}il.xml"),
        ("an empty segment", "word//evil.xml"),
    ] {
        add(row, with_entry(ZEntry::new(name, b"<x/>")), "zip_name");
    }
    // duplicates: two entries naming one part
    add(
        "a repeated part",
        with_entry(ZEntry::new("word/document.xml", b"<x/>")),
        "zip_duplicate",
    );
    add(
        "a part repeated in another case",
        with_entry(ZEntry::new("Word/Document.XML", b"<x/>")),
        "zip_duplicate",
    );
    // content
    let mut v = base_entries();
    v[0].method = 0;
    let mut g = build(&v, b"");
    // flip a stored byte: the headers still agree, the CRC doesn't
    let at = 30 + v[0].name.len() + 3;
    g[at] ^= 0x01;
    add("a bad CRC", g, "zip_corrupt");
    add(
        "a compression bomb",
        with_entry(ZEntry::new("word/bomb.bin", &vec![0u8; 2 << 20])),
        "zip_ratio",
    );
    let mut cfb = k2_mpc_opc::package::CFB_MAGIC.to_vec();
    cfb.extend_from_slice(&[0u8; 512]);
    add("an encrypted package (a compound file)", cfb, "encrypted");
    // the package
    let mut v = base_entries();
    v.retain(|e| e.name != "[Content_Types].xml");
    add("no content types", build(&v, b""), "no_content_types");
    add(
        "a DOCTYPE",
        with_entry(ZEntry::new(
            "word/x.xml",
            b"<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"b\">]><x>&a;</x>",
        )),
        "doctype",
    );
    add(
        "UTF-16 XML",
        with_entry(ZEntry::new("word/x.xml", b"\xFF\xFE<\0x\0/\0>\0")),
        "xml_encoding",
    );
    let mut v = base_entries();
    v[0].content = b"<Types xmlns=\"x\"><Default".to_vec();
    add(
        "content types that don't parse",
        build(&v, b""),
        "xml_invalid",
    );
    let mut v = base_entries();
    v[0].content = b"\xFF\xFF".to_vec();
    v[0].name = "[Content_Types].xml".into();
    add(
        "content types that aren't UTF-8",
        build(&v, b""),
        "xml_encoding",
    );

    // the limits
    let lim = |f: &dyn Fn(&mut Limits)| {
        let mut l = d;
        f(&mut l);
        l
    };
    rows.push((
        "over max_file",
        good.clone(),
        lim(&|l| l.max_file = 100),
        "too_large",
    ));
    rows.push((
        "over max_entries",
        good.clone(),
        lim(&|l| l.max_entries = 2),
        "zip_entries",
    ));
    rows.push((
        "over max_name_len",
        good.clone(),
        lim(&|l| l.max_name_len = 5),
        "zip_name",
    ));
    rows.push((
        "over max_entry",
        good.clone(),
        lim(&|l| l.max_entry = 10),
        "too_large",
    ));
    rows.push((
        "over max_total",
        good.clone(),
        lim(&|l| l.max_total = 100),
        "too_large",
    ));
    rows.push((
        "over max_nodes",
        good.clone(),
        lim(&|l| l.max_nodes = 3),
        "too_large",
    ));
    rows.push((
        "over max_depth",
        good.clone(),
        lim(&|l| l.max_depth = 1),
        "too_large",
    ));
    rows.push((
        "over max_ratio",
        good,
        lim(&|l| {
            l.max_ratio = 1;
            l.ratio_floor = 1;
        }),
        "zip_ratio",
    ));
    rows
}

/// Every refusal row refuses with its code (the refusal leg). `Err` lists each one that didn't.
pub fn refusals_hold() -> Result<(), String> {
    let mut bad = Vec::new();
    let all = rows();
    for (row, bytes, limits, want) in &all {
        match OpcPackage::open(bytes, limits) {
            Err(d) if d.code == *want => {}
            Err(d) => bad.push(format!("{row}: {} ({}), not {want}", d.code, d.message)),
            Ok(_) => bad.push(format!("{row}: opened, not {want}")),
        }
    }
    if bad.is_empty() {
        println!("refusals: {} rows, each with its code", all.len());
        Ok(())
    } else {
        Err(bad.join("\n"))
    }
}
