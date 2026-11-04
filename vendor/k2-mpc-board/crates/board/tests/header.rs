//! The header read: what a host lists boards with, without opening each one. Every call is held to
//! a full open. On every file the open accepts, they give what the open gives. On every file it
//! refuses, each one answers, with a typed refusal or a result, and never stops without one.

#[path = "support/mod.rs"]
mod support;

use k2_mpc_board::json::{self, Fields, Value};
use k2_mpc_board::*;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use support::forge::{manifest_text, package, sha_hex, Forge, DEFLATE, E, P, STORE};

/// A host's preview bound, as the docs give it.
const BOUND: u64 = 2 * 1024 * 1024;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors")
}

/// The preview as a full open reads it: none, its bytes, or the code its read refuses with.
type OpenedPreview = Option<Result<(String, Vec<u8>), RefusalCode>>;

/// What a full open gives, in the header's terms.
struct Opened {
    guard: Sha256,
    versions: (u32, u32),
    parts: Vec<(String, String, String, u64)>,
    preview_size: Option<u64>,
    preview: OpenedPreview,
    objects: u64,
    title: Option<String>,
}

fn opened(bytes: &[u8], l: &Limits) -> Result<Opened, Refusal> {
    let mut pkg = Package::open(Cursor::new(bytes), l)?;
    let m = pkg.manifest().clone();
    let shown = m.parts.iter().find(|p| p.role == "preview");
    let preview = shown.map(|p| {
        pkg.read_part(&p.name)
            .map(|b| (p.media_type.clone(), b))
            .map_err(|r| r.code)
    });
    let title = match pkg.board().settings.get("title") {
        Some(Value::Str(s)) => Some(s.clone()),
        _ => None,
    };
    Ok(Opened {
        guard: pkg.package_guard(),
        versions: (m.version, m.min_reader),
        parts: m
            .parts
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.role.clone(),
                    p.media_type.clone(),
                    p.bytes,
                )
            })
            .collect(),
        preview_size: shown.map(|p| p.bytes),
        preview,
        objects: pkg.board().objects.len() as u64,
        title,
    })
}

/// On a file the open accepts, each call gives what the open gives. Says whether it accepted.
fn agree(name: &str, bytes: &[u8]) -> bool {
    let l = Limits::V1;
    let Ok(want) = opened(bytes, &l) else {
        return false;
    };
    let h = read_header(Cursor::new(bytes), &l);
    assert!(
        h.is_ok(),
        "{name}: the open accepts it, and the header refuses: {:?}",
        h.as_ref().err()
    );
    let h = h.unwrap();
    assert!(!h.checked, "{name}: a header is never checked");
    assert_eq!(h.guard, want.guard, "{name}: the header's guard");
    assert_eq!(
        (h.version, h.min_reader),
        want.versions,
        "{name}: the versions"
    );
    let parts: Vec<_> = h
        .parts
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                p.role.clone(),
                p.media_type.clone(),
                p.bytes,
            )
        })
        .collect();
    assert_eq!(parts, want.parts, "{name}: the parts");
    assert_eq!(h.preview, want.preview_size, "{name}: the preview's size");

    let s = read_summary(Cursor::new(bytes), &l);
    assert!(
        s.is_ok(),
        "{name}: the open accepts it, and the summary refuses: {:?}",
        s.as_ref().err()
    );
    let s = s.unwrap();
    assert!(!s.checked, "{name}: a summary is never checked");
    assert_eq!(s.guard, want.guard, "{name}: the summary's guard");
    assert_eq!(s.objects, want.objects, "{name}: the count");
    assert_eq!(s.title, want.title, "{name}: the title");

    let got = read_preview(Cursor::new(bytes), &l, BOUND);
    let same = match (&want.preview, &got) {
        (None, Ok(None)) => true,
        (Some(Ok((media, b))), Ok(Some(p))) => &p.bytes == b && &p.media_type == media,
        (Some(Err(code)), Err(r)) => r.code == *code,
        _ => false,
    };
    assert!(
        same,
        "{name}: the preview: the open gives {:?}, the read {:?}",
        want.preview
            .as_ref()
            .map(|p| p.as_ref().map(|(m, b)| (m, b.len()))),
        got.as_ref()
            .map(|p| p.as_ref().map(|p| (&p.media_type, p.bytes.len())))
    );
    true
}

/// What a summary has to refuse whenever an open does: the board's bytes as they stream.
const STREAM: &[RefusalCode] = &[
    RefusalCode::LimitRatio,
    RefusalCode::ZipCrc,
    RefusalCode::ZipSizeMismatch,
    RefusalCode::ZipDeflateInvalid,
    RefusalCode::PartHash,
    RefusalCode::JsonUtf8,
    RefusalCode::JsonSyntax,
    RefusalCode::JsonSurrogate,
    RefusalCode::LimitDepth,
    RefusalCode::LimitValues,
    RefusalCode::LimitObjects,
];

/// On any file, each call answers: a typed refusal or a result, never a stop without one. Where
/// the open refuses the board's bytes as they stream, the summary refuses too.
fn answers(name: &str, bytes: &[u8]) {
    let l = Limits::V1;
    let known = |r: &Refusal| RefusalCode::ALL.contains(&r.code);
    let header = std::panic::catch_unwind(|| read_header(Cursor::new(bytes), &l));
    let preview = std::panic::catch_unwind(|| read_preview(Cursor::new(bytes), &l, BOUND));
    let summary = std::panic::catch_unwind(|| read_summary(Cursor::new(bytes), &l));
    assert!(
        header.is_ok() && preview.is_ok() && summary.is_ok(),
        "{name}: a call stopped without an answer"
    );
    let (header, preview, summary) = (header.unwrap(), preview.unwrap(), summary.unwrap());
    for r in [
        header.as_ref().err(),
        preview.as_ref().err(),
        summary.as_ref().err(),
    ]
    .into_iter()
    .flatten()
    {
        assert!(known(r), "{name}: an unknown code: {r}");
    }
    if let Err(r) = Package::open(Cursor::new(bytes), &l) {
        if header.is_ok() && STREAM.contains(&r.code) {
            assert!(
                summary.is_err(),
                "{name}: the open refuses the board's bytes ({r}), and the summary gives {summary:?}"
            );
        }
    }
}

fn check(name: &str, bytes: &[u8]) -> bool {
    answers(name, bytes);
    agree(name, bytes)
}

/// A package whose `board.json` is these bytes, whatever they are.
fn package_of(board: &[u8]) -> Vec<u8> {
    let row = (
        "board.json".to_string(),
        "application/json".to_string(),
        "board".to_string(),
        board.len(),
        sha_hex(board),
    );
    let manifest = manifest_text(None, &[row]);
    Forge::new(vec![
        E::new("manifest.json", manifest.as_bytes(), STORE),
        E::new("board.json", board, DEFLATE),
    ])
    .build()
}

/// A JSON vector as a value inside a board, so a summary has to read through it.
fn in_a_board(value: &[u8]) -> Vec<u8> {
    let mut out = b"{\"frames\":[],\"later\":".to_vec();
    out.extend_from_slice(value);
    out.extend_from_slice(b",\"objects\":[],\"people\":{},\"settings\":{}}");
    out
}

/// Every vector's input: a zip vector as it is, and a JSON one as a package's `board.json`, both
/// alone and as a value inside a board.
fn vector_files() -> Vec<(String, Vec<u8>)> {
    let root = vectors();
    let Value::Obj(index) = json::parse(
        &std::fs::read(root.join("index.json")).expect("the index"),
        &Limits::V1,
    )
    .expect("JSON") else {
        panic!("an object");
    };
    let Value::Arr(cases) = &index["cases"] else {
        panic!("a list");
    };
    let mut out = Vec::new();
    for case in cases {
        let Value::Obj(c) = case else {
            panic!("an object");
        };
        let (Some(Value::Str(name)), Some(Value::Str(input)), Some(Value::Str(layer))) =
            (c.get("name"), c.get("input"), c.get("layer"))
        else {
            continue;
        };
        let bytes = std::fs::read(root.join(input)).expect("the input");
        if layer == "zip" {
            out.push((format!("zip/{name}"), bytes));
        } else {
            out.push((format!("{layer}/{name}"), package_of(&bytes)));
            out.push((
                format!("{layer}/{name} in a board"),
                package_of(&in_a_board(&bytes)),
            ));
        }
    }
    out
}

#[test]
fn every_vector_agrees_with_the_open() {
    let (mut accepted, mut refused) = (0, 0);
    for (name, bytes) in vector_files() {
        if check(&name, &bytes) {
            accepted += 1;
        } else {
            refused += 1;
        }
    }
    assert!(accepted >= 30, "only {accepted} vectors open");
    assert!(refused >= 60, "only {refused} vectors are refused");
}

/// A JSON string, in one of the spellings a writer may use.
fn spelt(s: &str, rng: &mut Rng) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        let u = c as u32;
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ if u < 0x20 => out.push_str(&format!("\\u{u:04x}")),
            _ if u > 0xFFFF && rng.below(3) == 0 => {
                let v = u - 0x10000;
                out.push_str(&format!(
                    "\\u{:04X}\\u{:04x}",
                    0xD800 + (v >> 10),
                    0xDC00 + (v & 0x3FF)
                ));
            }
            _ if u <= 0xFFFF && rng.below(6) == 0 => out.push_str(&format!("\\u{u:04x}")),
            '/' if rng.below(2) == 0 => out.push_str("\\/"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

const WORDS: &[&str] = &[
    "",
    "hello",
    "objects",
    "settings",
    "title",
    "}]\",{[",
    "caf\u{e9}",
    "\u{1F600} smile",
    "a\"b\\c",
    "tab\tline\n",
    "/slash/",
];

const SPACES: [&str; 7] = ["", "", "", " ", "\n  ", "\t", "\r\n"];

fn ws(rng: &mut Rng) -> &'static str {
    SPACES[rng.below(SPACES.len())]
}

/// Members in a random order, with random spaces.
fn object_of(members: Vec<(String, String)>, rng: &mut Rng) -> String {
    let mut members = members;
    for i in (1..members.len()).rev() {
        members.swap(i, rng.below(i + 1));
    }
    let inner: Vec<String> = members
        .into_iter()
        .map(|(k, v)| format!("{}\"{k}\"{}:{}{v}{}", ws(rng), ws(rng), ws(rng), ws(rng)))
        .collect();
    format!("{{{}}}", inner.join(","))
}

fn word(rng: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..1 + rng.below(3) {
        s.push_str(WORDS[rng.below(WORDS.len())]);
    }
    s
}

/// A valid board in a random spelling: settings anywhere, decoys that name `objects`, `settings`
/// and `title` deeper down, and now and then one large enough to cross every chunk.
fn random_board(rng: &mut Rng, round: usize) -> String {
    let n = if round % 10 == 9 { 300 } else { rng.below(40) };
    let mut objects = Vec::new();
    for i in 0..n {
        let mut m = Vec::new();
        let text = if round % 10 == 9 {
            "x\u{e9}\"".repeat(130)
        } else {
            word(rng)
        };
        match rng.below(3) {
            0 => {
                m.push(("id".into(), format!("\"s-{i}\"")));
                m.push(("type".into(), "\"sticky\"".into()));
                m.push(("text".into(), spelt(&text, rng)));
            }
            1 => {
                m.push(("id".into(), format!("\"k-{i}\"")));
                m.push(("type".into(), "\"card\"".into()));
                m.push(("title".into(), spelt(&word(rng), rng)));
                let check = object_of(
                    vec![
                        ("done".into(), "false".into()),
                        ("id".into(), "\"c\"".into()),
                        ("text".into(), spelt(&text, rng)),
                    ],
                    rng,
                );
                m.push(("checks".into(), format!("[{check}]")));
            }
            _ => {
                m.push(("id".into(), format!("\"t-{i}\"")));
                m.push(("type".into(), "\"text\"".into()));
                m.push((
                    "later".into(),
                    "{\"settings\":{\"title\":\"decoy\"},\"objects\":[1,[2,{}]]}".into(),
                ));
            }
        }
        objects.push(object_of(m, rng));
    }
    let mut settings = Vec::new();
    if rng.below(4) != 0 {
        settings.push(("title".to_string(), spelt(&word(rng), rng)));
    }
    if rng.below(2) == 0 {
        settings.push(("voting".to_string(), "{\"active\":true}".to_string()));
    }
    let mut top = vec![
        ("frames".to_string(), "[]".to_string()),
        ("people".to_string(), "{}".to_string()),
        (
            "objects".to_string(),
            format!("[{}]", objects.join(&format!(",{}", ws(rng)))),
        ),
        ("settings".to_string(), object_of(settings, rng)),
    ];
    if rng.below(2) == 0 {
        top.push((
            "later".to_string(),
            "{\"objects\":[{\"id\":\"no\"}],\"settings\":{\"title\":\"decoy\"}}".to_string(),
        ));
    }
    format!("{}{}{}", ws(rng), object_of(top, rng), ws(rng))
}

#[test]
fn random_spellings_agree_with_the_open() {
    let mut rng = Rng(0x5EED_4EAD_E400_0001);
    for round in 0..300 {
        let text = random_board(&mut rng, round);
        let bytes = Forge::new(package(&text, Vec::new()).0).build();
        assert!(
            check(&format!("random {round}"), &bytes),
            "random {round}: the generator made a board the open refuses: {:?}",
            Package::open(Cursor::new(&bytes), &Limits::V1).err()
        );
    }
}

/// A file as a writer makes it, with a preview and a part this version doesn't know.
fn written(board: &Board, preview: Option<Vec<u8>>) -> Vec<u8> {
    let mut parts = MemParts::new();
    if let Some(p) = preview {
        parts.set_preview(p);
    }
    parts.add(
        "extras/later.bin",
        "later",
        "application/x-later",
        b"keep".to_vec(),
        Fields::new(),
    );
    let mut held = board.clone();
    for h in board.blob_refs() {
        apply_group(
            &mut held,
            &Group {
                id: "clear".into(),
                by: None,
                ops: vec![Op::ClearBlob { hash: h }],
            },
        )
        .expect("clears");
    }
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(Lineage::from_bytes([3; 16])),
        &held,
        &mut parts,
        &Limits::V1,
    )
    .expect("it writes");
    out
}

#[test]
fn the_corpus_agrees_with_the_open_as_a_writer_writes_it() {
    let mut n = 0;
    for (name, bytes) in vector_files() {
        if !name.starts_with("fidelity/") {
            continue;
        }
        let pkg = Package::open(Cursor::new(&bytes), &Limits::V1).expect("a corpus board opens");
        let file = written(pkg.board(), Some(format!("preview of {name}").into_bytes()));
        assert!(check(&name, &file), "{name}: the written file doesn't open");
        n += 1;
    }
    assert!(n >= 15, "only {n} corpus boards");
}

/// Random damage to a valid file: each call answers, and agrees wherever the open accepts.
#[test]
fn damage_to_a_file_is_answered_and_agrees_where_it_opens() {
    let board =
        Board::from_json(random_board(&mut Rng(7), 3).as_bytes(), &Limits::V1).expect("a board");
    let good = written(&board, Some(vec![9u8; 300]));
    let mut rng = Rng(0xDA4A_6E00_0000_0003);
    for i in 0..3000 {
        let mut bytes = good.clone();
        for _ in 0..1 + rng.below(3) {
            let at = rng.below(bytes.len());
            bytes[at] ^= 1 << rng.below(8);
        }
        if i % 7 == 0 {
            bytes.truncate(rng.below(bytes.len()));
        }
        check(&format!("damage {i}"), &bytes);
    }
}

#[test]
fn a_manifest_from_another_board_reads_as_a_header_and_refuses_as_a_summary() {
    let bytes =
        std::fs::read(vectors().join("zip/manifest-other-board.k5board")).expect("the vector");
    let open = Package::open(Cursor::new(&bytes), &Limits::V1).err();
    assert_eq!(
        open.map(|r| r.code),
        Some(RefusalCode::PartHash),
        "the open refuses it"
    );
    let guard = package_guard_of(Cursor::new(&bytes), &Limits::V1).ok();
    let h = read_header(Cursor::new(&bytes), &Limits::V1);
    assert!(
        matches!(&h, Ok(h) if Some(h.guard) == guard),
        "the header reads the manifest alone, with its guard: {h:?}"
    );
    let s = read_summary(Cursor::new(&bytes), &Limits::V1);
    assert!(
        matches!(&s, Err(r) if r.code == RefusalCode::PartHash),
        "the summary checks the hash: {s:?}"
    );
}

/// A reader that writes down every range read from it.
struct Counting<'a> {
    inner: Cursor<&'a [u8]>,
    reads: Vec<(u64, u64)>,
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let at = self.inner.position();
        let n = self.inner.read(buf)?;
        self.reads.push((at, at + n as u64));
        Ok(n)
    }
}

impl Seek for Counting<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(to)
    }
}

/// Where a part's data sits in a zip, from its central directory.
fn data_range(z: &[u8], want: &str) -> (u64, u64) {
    let le16 = |i: usize| usize::from(u16::from_le_bytes([z[i], z[i + 1]]));
    let le32 = |i: usize| u32::from_le_bytes([z[i], z[i + 1], z[i + 2], z[i + 3]]) as usize;
    let end = (0..=z.len() - 22)
        .rev()
        .find(|&i| z[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .expect("an end record");
    let mut p = le32(end + 16);
    for _ in 0..le16(end + 10) {
        let csize = le32(p + 20);
        let (n, x, c) = (le16(p + 28), le16(p + 30), le16(p + 32));
        let local = le32(p + 42);
        if &z[p + 46..p + 46 + n] == want.as_bytes() {
            let data = local + 30 + le16(local + 26) + le16(local + 28);
            return (data as u64, (data + csize) as u64);
        }
        p += 46 + n + x + c;
    }
    panic!("no {want}");
}

fn touches(reads: &[(u64, u64)], (from, to): (u64, u64)) -> bool {
    reads.iter().any(|&(a, b)| a < to && from < b)
}

#[test]
fn the_header_and_the_preview_never_read_the_boards_bytes() {
    let board =
        Board::from_json(random_board(&mut Rng(11), 9).as_bytes(), &Limits::V1).expect("a board");
    let bytes = written(&board, Some(vec![5u8; 64]));
    let range = data_range(&bytes, "board.json");
    assert!(range.1 - range.0 > 1000, "a board.json worth noticing");
    let mut c = Counting {
        inner: Cursor::new(&bytes),
        reads: Vec::new(),
    };
    let h = read_header(&mut c, &Limits::V1);
    assert!(h.is_ok(), "the header reads: {h:?}");
    assert!(
        !touches(&c.reads, range),
        "the header read board.json: {:?}",
        c.reads
    );
    c.reads.clear();
    let p = read_preview(&mut c, &Limits::V1, BOUND);
    assert!(matches!(&p, Ok(Some(_))), "the preview reads: {p:?}");
    assert!(
        !touches(&c.reads, range),
        "the preview read board.json: {:?}",
        c.reads
    );
    c.reads.clear();
    let s = read_summary(&mut c, &Limits::V1);
    assert!(s.is_ok(), "the summary reads: {s:?}");
    assert!(
        touches(&c.reads, range),
        "the summary reads board.json, so this can see it"
    );
}

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed);
    (0..n).map(|_| rng.next() as u8).collect()
}

#[test]
fn a_preview_at_its_bound_reads_and_one_byte_over_refuses() {
    let board = Board::empty();
    let at = noise(BOUND as usize, 21);
    let bytes = written(&board, Some(at.clone()));
    let p = read_preview(Cursor::new(&bytes), &Limits::V1, BOUND);
    assert!(
        matches!(&p, Ok(Some(p)) if p.bytes == at && p.media_type == "image/png"),
        "a preview at the bound reads: {:?}",
        p.as_ref().map(|p| p.as_ref().map(|p| p.bytes.len()))
    );
    let h = read_header(Cursor::new(&bytes), &Limits::V1);
    assert!(matches!(&h, Ok(h) if h.preview == Some(BOUND)), "{h:?}");

    let over = noise(BOUND as usize + 1, 22);
    let bytes = written(&board, Some(over));
    let r = read_preview(Cursor::new(&bytes), &Limits::V1, BOUND);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::LimitPartBytes),
        "one byte over the host's bound refuses: {:?}",
        r.as_ref().map(|p| p.as_ref().map(|p| p.bytes.len()))
    );
    let h = read_header(Cursor::new(&bytes), &Limits::V1);
    assert!(
        matches!(&h, Ok(h) if h.preview == Some(BOUND + 1)),
        "the header still reads, and says how big it is: {h:?}"
    );
}

#[test]
fn a_preview_whose_bytes_differ_from_the_manifest_refuses_and_none_is_none() {
    let small = "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{}}";
    let (mut entries, _) = package(
        small,
        vec![P {
            name: "preview.png",
            media: "image/png",
            role: "preview",
            data: b"the real preview".to_vec(),
        }],
    );
    entries[2].data = b"the other preview".to_vec()[..16].to_vec();
    let bytes = Forge::new(entries).build();
    let h = read_header(Cursor::new(&bytes), &Limits::V1);
    assert!(h.is_ok(), "the header reads the manifest alone: {h:?}");
    let r = read_preview(Cursor::new(&bytes), &Limits::V1, BOUND);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::PartHash),
        "bytes that don't match the manifest refuse: {r:?}"
    );

    let none = Forge::new(package(small, Vec::new()).0).build();
    let p = read_preview(Cursor::new(&none), &Limits::V1, BOUND);
    assert!(matches!(p, Ok(None)), "no preview is none: {p:?}");
}

fn summary_of(text: &str, l: &Limits) -> Result<Summary, Refusal> {
    read_summary(
        Cursor::new(Forge::new(package(text, Vec::new()).0).build()),
        l,
    )
}

fn open_code(text: &str, l: &Limits) -> Option<RefusalCode> {
    let bytes = Forge::new(package(text, Vec::new()).0).build();
    Package::open(Cursor::new(&bytes), l).err().map(|r| r.code)
}

#[test]
fn the_summary_holds_the_formats_bounds() {
    let tight = Limits {
        depth: 8,
        values: 60,
        objects: 3,
        ..Limits::V1
    };
    let nest = |n: usize| {
        format!(
            "{{\"frames\":[],\"later\":{}1{},\"objects\":[],\"people\":{{}},\"settings\":{{}}}}",
            "[".repeat(n),
            "]".repeat(n)
        )
    };
    let fine = nest(7);
    let s = summary_of(&fine, &tight);
    assert!(s.is_ok(), "depth 8 is at the bound: {s:?}");
    let deep = nest(8);
    assert_eq!(open_code(&deep, &tight), Some(RefusalCode::LimitDepth));
    let r = summary_of(&deep, &tight);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::LimitDepth),
        "depth 9 is past it: {r:?}"
    );

    let many = format!(
        "{{\"frames\":[],\"later\":[{}],\"objects\":[],\"people\":{{}},\"settings\":{{}}}}",
        vec!["0"; 60].join(",")
    );
    assert_eq!(open_code(&many, &tight), Some(RefusalCode::LimitValues));
    let r = summary_of(&many, &tight);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::LimitValues),
        "more values than the bound: {r:?}"
    );

    let sticky = |i: usize| format!("{{\"id\":\"s-{i}\",\"type\":\"sticky\"}}");
    let four: Vec<String> = (0..4).map(sticky).collect();
    let crowd = format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{}},\"settings\":{{}}}}",
        four.join(",")
    );
    assert_eq!(open_code(&crowd, &tight), Some(RefusalCode::LimitObjects));
    let r = summary_of(&crowd, &tight);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::LimitObjects),
        "more objects than the bound: {r:?}"
    );

    let long = format!(
        "{{\"frames\":[],\"objects\":[],\"people\":{{}},\"settings\":{{\"title\":\"{}\"}}}}",
        "\u{e9}".repeat(201)
    );
    assert_eq!(open_code(&long, &Limits::V1), Some(RefusalCode::LimitField));
    let r = summary_of(&long, &Limits::V1);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::LimitField),
        "a title over its field's limit: {r:?}"
    );
    let at = long.replace(&"\u{e9}".repeat(201), &"\u{e9}".repeat(200));
    let s = summary_of(&at, &Limits::V1);
    assert!(
        matches!(&s, Ok(s) if s.title.as_ref().map(|t| t.chars().count()) == Some(200)),
        "a title at its limit: {s:?}"
    );
}

#[test]
fn the_summary_holds_the_ratio_as_it_inflates() {
    let text = format!(
        "{{\"frames\":[],\"objects\":[],\"people\":{{}},\"settings\":{{}}{}}}",
        " ".repeat(4 * 1024 * 1024)
    );
    let bytes = Forge::new(package(&text, Vec::new()).0).build();
    let open = Package::open(Cursor::new(&bytes), &Limits::V1).err();
    assert_eq!(open.map(|r| r.code), Some(RefusalCode::LimitRatio));
    let r = read_summary(Cursor::new(&bytes), &Limits::V1);
    assert!(
        matches!(&r, Err(e) if e.code == RefusalCode::LimitRatio),
        "past the ratio: {r:?}"
    );
}

#[test]
fn the_title_comes_from_settings_wherever_settings_sits() {
    let l = Limits::V1;
    let first = "{\"settings\":{\"voting\":{\"active\":true},\"title\":\"T\\u00e9 \\ud83d\\ude00\"},\"objects\":[{\"id\":\"s-1\",\"title\":\"no\",\"type\":\"sticky\"}],\"people\":{},\"later\":{\"title\":\"no\",\"settings\":{\"title\":\"no\"}},\"frames\":[]}";
    let s = summary_of(first, &l);
    assert!(
        matches!(&s, Ok(s) if s.title.as_deref() == Some("T\u{e9} \u{1F600}") && s.objects == 1),
        "settings first: {s:?}"
    );
    let none =
        "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"voting\":{\"active\":false}}}";
    let s = summary_of(none, &l);
    assert!(
        matches!(&s, Ok(s) if s.title.is_none() && s.objects == 0),
        "no title: {s:?}"
    );
}

#[test]
fn the_summary_refuses_what_has_no_board_shape() {
    let l = Limits::V1;
    let cases: &[(&str, &str, RefusalCode)] = &[
        ("a list at the top", "[]", RefusalCode::BoardShape),
        (
            "objects that aren't a list",
            "{\"frames\":[],\"objects\":{},\"people\":{},\"settings\":{}}",
            RefusalCode::BoardShape,
        ),
        (
            "settings that aren't an object",
            "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":[]}",
            RefusalCode::BoardShape,
        ),
        (
            "no frames",
            "{\"objects\":[],\"people\":{},\"settings\":{}}",
            RefusalCode::BoardShape,
        ),
        (
            "objects twice",
            "{\"frames\":[],\"objects\":[],\"objects\":[],\"people\":{},\"settings\":{}}",
            RefusalCode::JsonDuplicateKey,
        ),
        (
            "a title twice",
            "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"title\":\"a\",\"title\":\"b\"}}",
            RefusalCode::JsonDuplicateKey,
        ),
        (
            "a title that isn't text",
            "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{\"title\":7}}",
            RefusalCode::FieldKind,
        ),
    ];
    for (what, text, code) in cases {
        assert_eq!(
            open_code(text, &l),
            Some(*code),
            "{what}: the open's refusal"
        );
        let r = summary_of(text, &l);
        assert!(
            matches!(&r, Err(e) if e.code == *code),
            "{what}: wanted {code:?}, got {r:?}"
        );
    }
}

#[test]
fn the_summary_refuses_what_isnt_json_where_the_open_does() {
    let l = Limits::V1;
    let board = |later: &[u8]| in_a_board(later);
    let mut whole = board(b"1");
    whole.push(b'x');
    let mut cut = board(b"1");
    cut.pop();
    let cases: Vec<(&str, Vec<u8>, RefusalCode)> = vec![
        ("content after the document", whole, RefusalCode::JsonSyntax),
        ("a document cut short", cut, RefusalCode::JsonSyntax),
        (
            "a byte that starts no character",
            board(b"\"\xff\""),
            RefusalCode::JsonUtf8,
        ),
        (
            "a broken continuation",
            board(b"\"\xc3\x28\""),
            RefusalCode::JsonUtf8,
        ),
        (
            "an overlong form",
            board(b"\"\xe0\x80\x80\""),
            RefusalCode::JsonUtf8,
        ),
        (
            "a byte no character starts with, then a continuation",
            board(b"\"\xc0\x80\""),
            RefusalCode::JsonUtf8,
        ),
        (
            "a lone low surrogate",
            board(b"\"\\udc00\""),
            RefusalCode::JsonSurrogate,
        ),
        (
            "a control character",
            board(b"\"a\x01b\""),
            RefusalCode::JsonSyntax,
        ),
        ("a leading zero", board(b"01"), RefusalCode::JsonSyntax),
        (
            "an unknown escape",
            board(b"\"\\q\""),
            RefusalCode::JsonSyntax,
        ),
    ];
    for (what, text, code) in cases {
        let bytes = package_of(&text);
        let open = Package::open(Cursor::new(&bytes), &l).err().map(|r| r.code);
        assert_eq!(open, Some(code), "{what}: the open's refusal");
        let r = read_summary(Cursor::new(&bytes), &l);
        assert!(
            matches!(&r, Err(e) if e.code == code),
            "{what}: wanted {code:?}, got {r:?}"
        );
    }
}
