//! Byte fidelity on the corpus (`tests/corpus/<family>/…`, refusals in `refused/`).
//!
//! For every accepted file, through the pack its file name picks:
//! 1. bytes -> doc -> store -> reload -> bytes is identical;
//! 2. a no-op edit gives identical bytes;
//! 3. a one-word edit changes exactly that word;
//! 4. random edits (property test): the written file is the original bytes outside the
//!    edited regions, byte for byte, and the inserted text in the file's own line-ending
//!    style inside them. The expectation is computed on the original bytes by offset
//!    arithmetic, not by the codec under test.
//!
//! Every refused file (`refused/<name>.<code>.<ext>`) is refused with that code.
//!
//! The `csv/` folder is the `csv-as-text` family: tables as text, held to the same rules,
//! with its own random-edit run whose inserts are cells, quotes and delimiters.
//!
//! `PROPTEST_CASES` overrides the random-edit case counts (512 each by default), for longer
//! runs on a compute node.

use std::path::{Path, PathBuf};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::session::Pack;
use k2_mpc_base::Doc;
use k2_mpc_text::{lookup, Eol, Family, Lookup, TextPack, ROOT, SCHEMA};
use proptest::prelude::*;

const IMPORT: u64 = 2;
const AGENT: u64 = 77;

struct File {
    rel: String,
    bytes: Vec<u8>,
}

impl File {
    fn pack(&self) -> TextPack {
        TextPack::for_path(&self.rel).unwrap_or_else(|| panic!("{}: not text", self.rel))
    }
    fn decode(&self) -> (String, Vec<u8>) {
        let c = self
            .pack()
            .decode(&self.bytes)
            .unwrap_or_else(|d| panic!("{}: refused: {d}", self.rel));
        match &c.roots[..] {
            [(r, Neutral::Text(t))] if r == ROOT => (t.clone(), c.codec),
            other => panic!("{}: roots {other:?}", self.rel),
        }
    }
    fn encode(&self, text: &str, codec: &[u8]) -> Vec<u8> {
        self.pack()
            .encode(&[(ROOT.to_owned(), Neutral::Text(text.to_owned()))], codec)
            .unwrap_or_else(|e| panic!("{}: {e}", self.rel))
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("corpus dir") {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out)
        } else {
            out.push(p)
        }
    }
}

/// `(accepted, refused)`, sorted by path.
fn corpus() -> (Vec<File>, Vec<File>) {
    let root = corpus_dir();
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();
    let (mut ok, mut refused) = (Vec::new(), Vec::new());
    for p in paths {
        let rel = p
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let f = File {
            bytes: std::fs::read(&p).unwrap(),
            rel,
        };
        if f.rel.starts_with("refused/") {
            refused.push(f)
        } else {
            ok.push(f)
        }
    }
    assert!(ok.len() >= 46, "corpus went missing: {} files", ok.len());
    assert!(refused.len() >= 7, "refusal corpus went missing");
    (ok, refused)
}

fn family_of_dir(rel: &str) -> Family {
    match rel.split('/').next().unwrap() {
        "code" => Family::Code,
        "config" => Family::Config,
        "markup" => Family::Markup,
        "diagram" => Family::Diagram,
        "text" => Family::Text,
        "csv" => Family::CsvAsText,
        d => panic!("corpus folder {d:?} is not a family"),
    }
}

/// The corpus covers what the brief asks for; fails if a case is lost.
#[test]
fn the_corpus_covers_every_family_and_every_hard_case() {
    let (ok, _) = corpus();
    for f in Family::ALL {
        let n = ok.iter().filter(|x| family_of_dir(&x.rel) == *f).count();
        assert!(n >= 2, "{f:?}: {n} corpus files");
    }
    // each file is looked up as its folder's family, by name or extension (never "unknown")
    for f in &ok {
        assert!(
            matches!(lookup(&f.rel), Lookup::Known { .. }),
            "{}: {:?}",
            f.rel,
            lookup(&f.rel)
        );
        assert_eq!(f.pack().family(), family_of_dir(&f.rel), "{}", f.rel);
    }
    let has = |what: &str, pred: &dyn Fn(&File) -> bool| {
        let n = ok.iter().filter(|f| pred(f)).count();
        assert!(n >= 1, "no corpus file with {what}");
        n
    };
    let eol = |f: &File| f.decode().1[2];
    let text = |f: &File| f.decode().0;
    assert!(has("CRLF", &|f| eol(f) == 1) >= 6);
    has("CR only", &|f| eol(f) == 2);
    assert!(has("a BOM", &|f| f.bytes.starts_with(b"\xEF\xBB\xBF")) >= 3);
    has("a BOM and nothing else", &|f| f.bytes == b"\xEF\xBB\xBF");
    assert!(
        has("no final newline", &|f| !f.bytes.is_empty()
            && !f.bytes.ends_with(b"\n")
            && !f.bytes.ends_with(b"\r")
            && f.bytes != b"\xEF\xBB\xBF")
            >= 4
    );
    has("an empty file", &|f| f.bytes.is_empty());
    has("emoji", &|f| text(f).contains('🎉'));
    has("a ZWJ sequence", &|f| text(f).contains('\u{200D}'));
    has("a combining mark", &|f| text(f).contains('\u{0301}'));
    has("RTL text", &|f| text(f).contains('ש'));
    has("CJK", &|f| text(f).contains('日'));
    has("a line over 10,000 bytes", &|f| {
        text(f).split('\n').any(|l| l.len() > 10_000)
    });
    has("tabs", &|f| text(f).contains('\t'));
    has("trailing spaces", &|f| text(f).contains("   \n"));
    has("control bytes", &|f| text(f).contains('\u{000C}'));
    for name in [
        "config/tsconfig.json",      // JSON with comments and trailing commas
        "config/docker-compose.yml", // YAML anchors, merge keys, two documents
        "config/Cargo.toml",         // TOML
        "markup/index.html",         // HTML with style and script
        "markup/icon.svg",           // SVG
        "markup/scatter.min.svg",    // minified SVG, one long line
        "diagram/flow.mmd",          // Mermaid
        "diagram/classes.puml",      // PlantUML
        "diagram/deps.dot",          // Graphviz
        "diagram/board.drawio",      // draw.io XML
        "diagram/compressed.dio",    // compressed draw.io
        "code/Makefile",             // a well-known name, tabs
        "code/Dockerfile",           // a well-known name
        "config/.editorconfig",      // a dotfile
        "text/classic-mac.txt",      // CR only
        "text/windows.txt",          // CRLF, no final newline
    ] {
        assert!(ok.iter().any(|f| f.rel == name), "{name} is missing");
    }
    let langs = ["rs", "ts", "go", "py", "sh", "sql", "css", "bat"];
    for l in langs {
        assert!(
            ok.iter().any(|f| f.rel.ends_with(&format!(".{l}"))),
            "no .{l} file"
        );
    }
}

/// The CSV/TSV cases the csv-as-text family must hold; fails if one is lost.
#[test]
fn the_csv_corpus_covers_the_table_cases() {
    let (ok, refused) = corpus();
    let csv: Vec<&File> = ok.iter().filter(|f| f.rel.starts_with("csv/")).collect();
    assert!(csv.len() >= 6, "{} csv corpus files", csv.len());
    for f in &csv {
        assert_eq!(f.pack().family(), Family::CsvAsText, "{}", f.rel);
        assert_eq!(f.pack().family().name(), "csv-as-text");
    }
    let has = |what: &str, pred: &dyn Fn(&File, &str) -> bool| {
        assert!(
            csv.iter().any(|f| pred(f, &f.decode().0)),
            "no csv corpus file with {what}"
        );
    };
    let eol = |f: &File| f.decode().1[2];
    has("CRLF", &|f, _| eol(f) == 1);
    has("CR only (classic Mac Excel)", &|f, _| eol(f) == 2);
    has("a BOM and CRLF (Excel CSV UTF-8)", &|f, _| {
        f.bytes.starts_with(b"\xEF\xBB\xBF") && eol(f) == 1
    });
    has("a quoted field with a line break inside", &|_, t| {
        // a line with an odd number of quotes opens a quoted field that runs on
        t.split('\n').any(|l| l.matches('"').count() % 2 == 1)
    });
    has("doubled quotes", &|_, t| t.contains("\"\""));
    has("100+ columns", &|_, t| {
        t.lines()
            .next()
            .is_some_and(|h| h.split(',').count() >= 100)
    });
    has("the semicolon dialect", &|_, t| {
        t.lines().next().is_some_and(|h| h.split(';').count() >= 4)
    });
    has("a .tsv", &|f, t| {
        f.rel.ends_with(".tsv") && t.contains('\t')
    });
    has("no final newline", &|f, _| {
        !f.bytes.ends_with(b"\n") && !f.bytes.ends_with(b"\r")
    });
    // the Excel habit this mode can't take: CRLF rows with a bare LF inside a cell
    let mixed = refused
        .iter()
        .find(|f| f.rel.ends_with(".csv"))
        .expect("a refused csv");
    let d = mixed.pack().decode(&mixed.bytes).unwrap_err();
    assert_eq!(d.code, "mixed_line_endings", "{}", d.message);
}

#[test]
fn every_file_round_trips_byte_exact_through_doc_and_store() {
    let (ok, _) = corpus();
    for f in &ok {
        let (text, codec) = f.decode();
        assert!(!text.contains('\r'), "{}: doc text holds a CR", f.rel);
        let mut doc = Doc::new(SCHEMA).unwrap();
        doc.replace_text_as(IMPORT, ROOT, &text).unwrap();
        let reloaded = Doc::load(SCHEMA, &doc.encode_state().expect("encode state")).unwrap();
        let out = f.encode(&reloaded.text(ROOT).unwrap(), &codec);
        assert_eq!(out, f.bytes, "{}: bytes changed through doc + store", f.rel);
    }
}

#[test]
fn a_no_op_edit_gives_identical_bytes() {
    let (ok, _) = corpus();
    for f in &ok {
        let (text, codec) = f.decode();
        let mut doc = Doc::new(SCHEMA).unwrap();
        doc.replace_text_as(IMPORT, ROOT, &text).unwrap();
        // the same text again, and the text as the file's bytes (CRLF, CR) normalised in
        let as_file = String::from_utf8(f.bytes.clone()).unwrap();
        let as_file = as_file.strip_prefix('\u{FEFF}').unwrap_or(&as_file);
        for again in [text.as_str(), &f.pack().normalize_input(ROOT, as_file)] {
            assert_eq!(again, text, "{}", f.rel);
            doc.replace_text_as(AGENT, ROOT, again).unwrap();
        }
        assert_eq!(
            f.encode(&doc.text(ROOT).unwrap(), &codec),
            f.bytes,
            "{}",
            f.rel
        );
    }
}

/// First ASCII word of 3+ letters with a lowercase letter (so uppercasing changes it).
fn a_word(s: &str) -> Option<&str> {
    s.split(|c: char| !c.is_ascii_alphabetic())
        .find(|w| w.len() >= 3 && w.bytes().any(|b| b.is_ascii_lowercase()))
}

#[test]
fn a_one_word_edit_changes_only_that_word() {
    let (ok, _) = corpus();
    let mut edited = 0;
    for f in &ok {
        let (text, codec) = f.decode();
        let Some(w) = a_word(&text).map(str::to_owned) else {
            assert!(
                text.trim().is_empty(),
                "{}: no word found in a file with text",
                f.rel
            );
            continue;
        };
        let mut doc = Doc::new(SCHEMA).unwrap();
        doc.replace_text_as(IMPORT, ROOT, &text).unwrap();
        let up = w.to_uppercase();
        doc.replace_text_as(AGENT, ROOT, &text.replacen(&w, &up, 1))
            .unwrap();
        let out = f.encode(&doc.text(ROOT).unwrap(), &codec);
        // the word is ASCII letters, so its first occurrence in the bytes is the same one
        let at = find(&f.bytes, w.as_bytes()).unwrap();
        let mut expected = f.bytes.clone();
        expected[at..at + w.len()].copy_from_slice(up.as_bytes());
        assert_eq!(out, expected, "{}: editing {w:?} touched more", f.rel);
        edited += 1;
    }
    assert!(edited >= 43, "only {edited} files had a word");
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn every_refused_file_is_refused_with_its_code() {
    let (_, refused) = corpus();
    for f in &refused {
        // refused/<stem>.<code>.<ext>
        let name = f.rel.trim_start_matches("refused/");
        let mut parts = name.rsplitn(3, '.');
        let (_ext, code) = (parts.next().unwrap(), parts.next().unwrap());
        let pack = TextPack::for_path(&f.rel).expect("refusal files have text-ish names");
        let d = pack
            .decode(&f.bytes)
            .err()
            .unwrap_or_else(|| panic!("{}: accepted", f.rel));
        assert_eq!(d.code, code, "{}: {}", f.rel, d.message);
        assert!(!d.message.is_empty());
    }
    let codes: std::collections::BTreeSet<&str> = refused
        .iter()
        .map(|f| f.rel.rsplit('.').nth(1).unwrap())
        .collect();
    assert_eq!(
        codes.into_iter().collect::<Vec<_>>(),
        ["binary", "mixed_line_endings", "not_utf8"]
    );
}

#[test]
fn control_bytes_decide_unknown_types_only() {
    let (ok, refused) = corpus();
    let known = ok
        .iter()
        .find(|f| f.rel == "text/controls-known-type.txt")
        .unwrap();
    let unknown = refused
        .iter()
        .find(|f| f.rel == "refused/controls.binary.xyz")
        .unwrap();
    assert_eq!(known.bytes, unknown.bytes, "the same bytes under two names");
    assert_eq!(lookup(&unknown.rel), Lookup::Unknown);
    assert!(known.pack().decode(&known.bytes).is_ok());
    assert_eq!(
        TextPack::for_path(&unknown.rel)
            .unwrap()
            .decode(&unknown.bytes)
            .unwrap_err()
            .code,
        "binary"
    );
}

#[test]
fn too_large_is_refused_per_type() {
    let log = TextPack::for_path("x.log").unwrap();
    let txt = TextPack::for_path("x.txt").unwrap();
    let big = vec![b'a'; (1 << 20) + 1];
    assert_eq!(log.decode(&big).unwrap_err().code, "too_large");
    assert!(txt.decode(&big).is_ok());
    let huge = vec![b'a'; (4 << 20) + 1];
    assert_eq!(txt.decode(&huge).unwrap_err().code, "too_large");
}

// ---- random edits ----

fn eol_of(codec: &[u8]) -> Eol {
    match codec[2] {
        0 => Eol::Lf,
        1 => Eol::Crlf,
        2 => Eol::Cr,
        x => panic!("eol {x}"),
    }
}

/// Byte offset in the file of doc offset `at` (independent of the codec under test).
fn file_offset(text: &str, at: usize, bom: bool, eol: Eol) -> usize {
    let extra = eol.as_str().len() - 1;
    usize::from(bom) * 3 + at + text[..at].matches('\n').count() * extra
}

/// One edit: replace doc chars `[a, b)` (as fractions of the text) with `insert`.
#[derive(Debug, Clone)]
struct RandEdit {
    a: f64,
    len: f64,
    insert: String,
}

fn rand_edit() -> impl Strategy<Value = RandEdit> {
    (
        0.0..=1.0f64,
        prop_oneof![Just(0.0), 0.0..0.05f64, 0.0..0.5f64],
        proptest::string::string_regex(
            "(x|yz|\n|\r\n|\r|\t|é|日本|🎉|👩\u{200D}💻|\u{0301}|\u{FEFF}| ){0,12}",
        )
        .unwrap(),
    )
        .prop_map(|(a, len, insert)| RandEdit { a, len, insert })
}

fn char_floor(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512)
}

fn csv_edit() -> impl Strategy<Value = RandEdit> {
    (
        0.0..=1.0f64,
        prop_oneof![Just(0.0), 0.0..0.05f64, 0.0..0.5f64],
        proptest::string::string_regex(
            "(,|;|\t|\"|\"\"|x|42|1,5|\n|\r\n|\r|é|日本|🎉|\u{FEFF}| ){0,12}",
        )
        .unwrap(),
    )
        .prop_map(|(a, len, insert)| RandEdit { a, len, insert })
}

/// Applies `edits` to `f` through a doc and checks the written bytes (see the module docs).
fn check_random_edits(f: &File, edits: &[RandEdit]) -> Result<(), TestCaseError> {
    let pack = f.pack();
    let (mut text, codec) = f.decode();
    let (bom, eol) = (codec[1] == 1, eol_of(&codec));
    let mut doc = Doc::new(SCHEMA).unwrap();
    doc.replace_text_as(IMPORT, ROOT, &text).unwrap();
    let mut expected = f.bytes.clone();
    for e in edits {
        let a = char_floor(&text, (e.a * text.len() as f64) as usize);
        let b = char_floor(&text, a + (e.len * text.len() as f64) as usize).max(a);
        let ins = pack.normalize_input(ROOT, &e.insert).into_owned();
        // the expectation, on bytes: keep [..A] and [B..], put the insert in the file's style
        let (fa, fb) = (
            file_offset(&text, a, bom, eol),
            file_offset(&text, b, bom, eol),
        );
        let middle = ins.replace('\n', eol.as_str());
        expected.splice(fa..fb, middle.into_bytes());
        text.replace_range(a..b, &ins);
        doc.replace_text_as(AGENT, ROOT, &text).unwrap();
    }
    let reloaded = Doc::load(SCHEMA, &doc.encode_state().unwrap()).unwrap();
    prop_assert_eq!(reloaded.text(ROOT).unwrap(), text.clone());
    let out = f.encode(&text, &codec);
    prop_assert!(out == expected, "{}: edits {:?}", f.rel, edits);
    // the written file is accepted again and is a fixed point: decode + encode = same bytes
    let back = pack
        .decode(&out)
        .unwrap_or_else(|d| panic!("{}: written file refused: {d}", f.rel));
    prop_assert_eq!(&pack.encode(&back.roots, &back.codec).unwrap(), &out);
    // and reads back as the same text, unless the text itself now starts with U+FEFF in
    // a file without a BOM (those bytes are a BOM to any reader)
    if bom || !text.starts_with('\u{FEFF}') {
        prop_assert_eq!(&back.roots[0].1, &Neutral::Text(text.clone()));
        prop_assert_eq!(back.codec[1], codec[1]);
        if text.contains('\n') {
            prop_assert_eq!(back.codec[2], codec[2]);
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, .. ProptestConfig::default() })]

    /// Untouched regions stay byte-identical; each edit lands in the file's own style.
    #[test]
    fn random_edits_keep_every_untouched_byte(pick in any::<prop::sample::Index>(),
                                              edits in proptest::collection::vec(rand_edit(), 1..5)) {
        let (ok, _) = corpus();
        check_random_edits(&ok[pick.index(ok.len())], &edits)?;
    }

    /// The same on the csv-as-text files only, with inserts made of cells, quotes and
    /// delimiters (a table edited as text).
    #[test]
    fn random_cell_edits_keep_every_untouched_csv_byte(pick in any::<prop::sample::Index>(),
                                                       edits in proptest::collection::vec(csv_edit(), 1..6)) {
        let (ok, _) = corpus();
        let csv: Vec<&File> = ok.iter().filter(|f| f.rel.starts_with("csv/")).collect();
        check_random_edits(csv[pick.index(csv.len())], &edits)?;
    }
}
