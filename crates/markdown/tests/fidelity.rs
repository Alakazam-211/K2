//! Round-trip fidelity on a corpus of tricky Markdown files.
//!
//! For every file in `tests/corpus/`:
//! 1. bytes -> doc -> store -> reload -> bytes is identical (BOM, CRLF, no final newline,
//!    front matter, raw HTML, fences, conflict markers, combining marks, emoji);
//! 2. an agent edit of one word changes exactly that word in the written file.

use std::path::Path;

use k2_mpc_base::Doc;
use k2_mpc_markdown::{codec, ROOT, SCHEMA};

const IMPORT: u64 = 2;
const AGENT: u64 = 77;

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .expect("corpus dir")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    assert!(v.len() >= 6, "corpus went missing: {} files", v.len());
    v
}

/// First ASCII word of 3+ letters with a lowercase letter (so uppercasing changes it).
fn a_word(s: &str) -> &str {
    s.split(|c: char| !c.is_ascii_alphabetic())
        .find(|w| w.len() >= 3 && w.bytes().any(|b| b.is_ascii_lowercase()))
        .expect("corpus file has a word")
}

#[test]
fn every_file_round_trips_byte_exact() {
    for (name, bytes) in corpus() {
        let (text, fmt) = codec::decode(&bytes).unwrap_or_else(|d| panic!("{name}: declined: {d}"));
        let mut doc = Doc::new(SCHEMA).unwrap();
        doc.replace_text_as(IMPORT, ROOT, &text).unwrap();
        let reloaded = Doc::load(SCHEMA, &doc.encode_state().expect("encode state")).unwrap();
        let out = codec::encode(&reloaded.text(ROOT).unwrap(), fmt);
        assert_eq!(out, bytes, "{name}: bytes changed through doc + store");
    }
}

#[test]
fn a_one_word_edit_changes_only_that_word() {
    for (name, bytes) in corpus() {
        let (text, fmt) = codec::decode(&bytes).unwrap();
        let mut doc = Doc::new(SCHEMA).unwrap();
        doc.replace_text_as(IMPORT, ROOT, &text).unwrap();
        let w = a_word(&text).to_owned();
        let up = w.to_uppercase();
        doc.replace_text_as(AGENT, ROOT, &text.replacen(&w, &up, 1))
            .unwrap();
        let out = codec::encode(&doc.text(ROOT).unwrap(), fmt);
        let expected = String::from_utf8_lossy(&bytes)
            .replacen(&w, &up, 1)
            .into_bytes();
        assert_eq!(
            out, expected,
            "{name}: an edit of {w:?} touched more than that word"
        );
    }
}

#[test]
fn mixed_line_endings_are_declined_not_normalised() {
    assert!(codec::decode(b"a\r\nb\nc\n").is_err());
}
