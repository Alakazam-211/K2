//! Shared test helpers: the limits, decks from the generator, and a second writer (the `zip`
//! crate) for deflated variants.
#![allow(dead_code)]

pub mod blob_rows;
pub mod hash_rows;
pub mod kit_view_golden;
pub mod live;
pub mod live_rows;
pub mod matrix;
pub mod splice_rows;
pub mod view_rows;

use std::io::{Cursor, Read, Write};

use k2_mpc_pptx::testgen::{self, DeckSpec, Shape, Slide};
use k2_mpc_pptx::Limits;

pub fn limits() -> Limits {
    Limits::default()
}

/// The same parts, deflated by the `zip` crate (a writer sharing no code with ours).
pub fn deflated(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (n, c) in parts {
        w.start_file(n.as_str(), opt).expect("start");
        w.write_all(c).expect("write");
    }
    w.finish().expect("finish").into_inner()
}

/// Every entry of a package, by the `zip` crate (name, content), in order.
pub fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut a = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
    let mut out = Vec::new();
    for i in 0..a.len() {
        let mut f = a.by_index(i).expect("entry");
        let mut c = Vec::new();
        f.read_to_end(&mut c).expect("read");
        out.push((f.name().expect("a name").into_owned(), c));
    }
    out
}

/// One entry's content, by name.
pub fn entry(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    entries(bytes)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| c)
}

/// One entry's content as text.
pub fn text(bytes: &[u8], name: &str) -> String {
    String::from_utf8(entry(bytes, name).unwrap_or_else(|| panic!("no {name}"))).expect("utf-8")
}

/// The named specs every fixture row runs over.
pub fn specs() -> Vec<(&'static str, DeckSpec)> {
    let sample = testgen::sample();
    let mut pretty = sample.clone();
    pretty.pretty = true;
    let mut sections = sample.clone();
    sections.sections = vec![
        ("Opening".into(), vec![256, 257]),
        ("Detail".into(), vec![258, 259]),
    ];
    let mut no_notes = sample.clone();
    no_notes.notes_master = false;
    let empty = DeckSpec::new(Vec::new());
    let one = DeckSpec::new(vec![Slide::titled(256, "Only", "One line")]);
    let mut big = DeckSpec::new(
        (0..40)
            .map(|i| {
                let mut s = Slide::titled(256 + i * 3, &format!("Slide {i}"), "a\nb");
                s.shapes.push(Shape::text_box(
                    4,
                    (100, 200, 300, 400),
                    &format!("box {i} & <more>"),
                ));
                s
            })
            .collect(),
    );
    big.pretty = true;
    vec![
        ("sample", sample),
        ("pretty", pretty),
        ("sections", sections),
        ("no-notes-master", no_notes),
        ("empty", empty),
        ("one", one),
        ("forty", big),
    ]
}
