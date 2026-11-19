//! Test support: our own generated packages (the default fixtures), written by an independent
//! zip writer (`zipgen`); a second zip reader (the `zip` crate); and a small list merge a test
//! pack uses for the one content part each format's rows edit.
#![allow(dead_code)]

pub mod damage;
pub mod merges;
pub mod zipgen;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use k2_mpc_opc::merge::{PartMerge, Resolution};
use k2_mpc_opc::Limits;
use zipgen::{build, Desc, Rng, ZEntry};

pub const PKG_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub const PKG_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
pub const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const CT_XML: &str = "application/xml";
pub const CT_RELS: &str = "application/vnd.openxmlformats-package.relationships+xml";
pub const CT_PNG: &str = "image/png";

/// The three formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Docx,
    Pptx,
    Xlsx,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Docx, Format::Pptx, Format::Xlsx];

    pub fn name(self) -> &'static str {
        match self {
            Format::Docx => "docx",
            Format::Pptx => "pptx",
            Format::Xlsx => "xlsx",
        }
    }

    /// The main part, whose list (paragraphs, slide ids, sheets) the rows edit.
    pub fn main(self) -> &'static str {
        match self {
            Format::Docx => "word/document.xml",
            Format::Pptx => "ppt/presentation.xml",
            Format::Xlsx => "xl/workbook.xml",
        }
    }
}

/// How a fixture's zip is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Deflate at the default level, sizes in the headers.
    Plain,
    /// Every entry stored.
    Stored,
    /// Deflate at level 9 (its flag bits set), a unix writer's version and attributes.
    Max,
    /// Data descriptors with their signature.
    DescSig,
    /// Data descriptors without one.
    DescNoSig,
    /// Local and central extra fields that differ, entry comments, an archive comment.
    Extra,
    /// A UTF-8 part name with its flag, and folder entries.
    Utf8,
    /// Per entry at random: method, level, descriptor; the entries in reverse order.
    Mixed,
}

impl Layout {
    pub const ALL: [Layout; 8] = [
        Layout::Plain,
        Layout::Stored,
        Layout::Max,
        Layout::DescSig,
        Layout::DescNoSig,
        Layout::Extra,
        Layout::Utf8,
        Layout::Mixed,
    ];
}

/// A package as a model the rows can edit: content types, relationships per source part,
/// the main part's list items, and the other parts.
#[derive(Debug, Clone)]
pub struct Model {
    pub format: Format,
    pub defaults: Vec<(String, String)>,
    pub overrides: Vec<(String, String)>,
    /// Relationships by source part name (`""`: the package): (id, type, target).
    pub rels: BTreeMap<String, Vec<(String, String, String)>>,
    /// The main part's list items, as XML.
    pub items: Vec<String>,
    /// Other parts, in order.
    pub parts: Vec<(String, Vec<u8>)>,
}

fn rt(kind: &str) -> String {
    format!("{REL}/{kind}")
}

/// Synthetic image bytes (not a real image: the package layer never reads them).
pub fn blob(seed: u64, len: usize) -> Vec<u8> {
    let mut r = Rng::new(seed);
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    while v.len() < len {
        v.push(r.next() as u8);
    }
    v
}

impl Model {
    /// A small package of `format`, its text varied by `seed`.
    pub fn new(format: Format, seed: u64) -> Model {
        let mut m = Model {
            format,
            defaults: vec![
                ("rels".into(), CT_RELS.into()),
                ("xml".into(), CT_XML.into()),
            ],
            overrides: Vec::new(),
            rels: BTreeMap::new(),
            items: Vec::new(),
            parts: Vec::new(),
        };
        let core = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Synthetic fixture</dc:title><dc:creator>Author A</dc:creator></cp:coreProperties>";
        let app = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>fixture generator</Application></Properties>";
        m.rels.insert(
            String::new(),
            vec![
                ("rId1".into(), rt("officeDocument"), format.main().into()),
                (
                    "rId2".into(),
                    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties".into(),
                    "docProps/core.xml".into(),
                ),
                ("rId3".into(), rt("extended-properties"), "docProps/app.xml".into()),
            ],
        );
        let words = [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
        ];
        let mut r = Rng::new(seed + 11);
        let mut text = |n: usize| -> String {
            (0..n)
                .map(|_| words[r.below(words.len() as u64) as usize])
                .collect::<Vec<_>>()
                .join(" ")
        };
        match format {
            Format::Docx => {
                m.defaults.push(("png".into(), CT_PNG.into()));
                m.overrides.push(("/word/document.xml".into(), "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml".into()));
                m.overrides.push((
                    "/word/styles.xml".into(),
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"
                        .into(),
                ));
                m.rels.insert(
                    "word/document.xml".into(),
                    vec![
                        ("rId1".into(), rt("styles"), "styles.xml".into()),
                        ("rId2".into(), rt("image"), "media/image1.png".into()),
                    ],
                );
                for i in 0..(3 + seed as usize) {
                    m.items
                        .push(format!("<w:p><w:r><w:t>{} {i}</w:t></w:r></w:p>", text(4)));
                }
                m.items.push(
                    "<w:p><w:r><w:t>a picture</w:t></w:r><a:blip r:embed=\"rId2\"/></w:p>".into(),
                );
                m.parts.push(("word/styles.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"{W}\"><w:style w:styleId=\"Normal\"/></w:styles>").into_bytes()));
                m.parts
                    .push(("word/media/image1.png".into(), blob(seed, 300)));
            }
            Format::Pptx => {
                let ct = |k: &str| {
                    format!("application/vnd.openxmlformats-officedocument.presentationml.{k}+xml")
                };
                m.overrides
                    .push(("/ppt/presentation.xml".into(), ct("presentation.main")));
                m.overrides
                    .push(("/ppt/slides/slide1.xml".into(), ct("slide")));
                m.overrides.push((
                    "/ppt/slideLayouts/slideLayout1.xml".into(),
                    ct("slideLayout"),
                ));
                m.overrides.push((
                    "/ppt/slideMasters/slideMaster1.xml".into(),
                    ct("slideMaster"),
                ));
                m.overrides.push((
                    "/ppt/theme/theme1.xml".into(),
                    "application/vnd.openxmlformats-officedocument.theme+xml".into(),
                ));
                m.rels.insert(
                    "ppt/presentation.xml".into(),
                    vec![
                        (
                            "rId1".into(),
                            rt("slideMaster"),
                            "slideMasters/slideMaster1.xml".into(),
                        ),
                        ("rId2".into(), rt("slide"), "slides/slide1.xml".into()),
                        ("rId3".into(), rt("theme"), "theme/theme1.xml".into()),
                    ],
                );
                m.rels.insert(
                    "ppt/slides/slide1.xml".into(),
                    vec![(
                        "rId1".into(),
                        rt("slideLayout"),
                        "../slideLayouts/slideLayout1.xml".into(),
                    )],
                );
                m.rels.insert(
                    "ppt/slideLayouts/slideLayout1.xml".into(),
                    vec![(
                        "rId1".into(),
                        rt("slideMaster"),
                        "../slideMasters/slideMaster1.xml".into(),
                    )],
                );
                m.rels.insert(
                    "ppt/slideMasters/slideMaster1.xml".into(),
                    vec![
                        (
                            "rId1".into(),
                            rt("slideLayout"),
                            "../slideLayouts/slideLayout1.xml".into(),
                        ),
                        ("rId2".into(), rt("theme"), "../theme/theme1.xml".into()),
                    ],
                );
                m.items.push("<p:sldId id=\"256\" r:id=\"rId2\"/>".into());
                m.parts.push((
                    "ppt/slides/slide1.xml".into(),
                    slide_xml(&text(5 + seed as usize)),
                ));
                m.parts.push(("ppt/slideLayouts/slideLayout1.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:sldLayout xmlns:p=\"{P}\"><p:cSld name=\"Title\"/></p:sldLayout>").into_bytes()));
                m.parts.push(("ppt/slideMasters/slideMaster1.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:sldMaster xmlns:p=\"{P}\" xmlns:r=\"{R}\"><p:cSld/><p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst></p:sldMaster>").into_bytes()));
                m.parts.push(("ppt/theme/theme1.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<a:theme xmlns:a=\"{A}\" name=\"Plain\"/>").into_bytes()));
            }
            Format::Xlsx => {
                let ct = |k: &str| {
                    format!("application/vnd.openxmlformats-officedocument.spreadsheetml.{k}+xml")
                };
                m.overrides
                    .push(("/xl/workbook.xml".into(), ct("sheet.main")));
                m.overrides
                    .push(("/xl/worksheets/sheet1.xml".into(), ct("worksheet")));
                m.overrides
                    .push(("/xl/sharedStrings.xml".into(), ct("sharedStrings")));
                m.overrides.push(("/xl/styles.xml".into(), ct("styles")));
                m.rels.insert(
                    "xl/workbook.xml".into(),
                    vec![
                        (
                            "rId1".into(),
                            rt("worksheet"),
                            "worksheets/sheet1.xml".into(),
                        ),
                        ("rId2".into(), rt("styles"), "styles.xml".into()),
                        (
                            "rId3".into(),
                            rt("sharedStrings"),
                            "sharedStrings.xml".into(),
                        ),
                    ],
                );
                m.items
                    .push("<sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/>".into());
                let rows: String = (1..=(3 + seed))
                    .map(|i| format!("<row r=\"{i}\"><c r=\"A{i}\"><v>{}</v></c><c r=\"B{i}\" t=\"s\"><v>0</v></c></row>", i * 7))
                    .collect();
                m.parts.push(("xl/worksheets/sheet1.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"{S}\"><sheetData>{rows}</sheetData></worksheet>").into_bytes()));
                m.parts.push(("xl/sharedStrings.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<sst xmlns=\"{S}\" count=\"1\" uniqueCount=\"1\"><si><t>{}</t></si></sst>", text(3)).into_bytes()));
                m.parts.push(("xl/styles.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"{S}\"><cellXfs count=\"1\"><xf/></cellXfs></styleSheet>").into_bytes()));
            }
        }
        m.overrides.push((
            "/docProps/core.xml".into(),
            "application/vnd.openxmlformats-package.core-properties+xml".into(),
        ));
        m.overrides.push((
            "/docProps/app.xml".into(),
            "application/vnd.openxmlformats-officedocument.extended-properties+xml".into(),
        ));
        m.parts
            .push(("docProps/core.xml".into(), core.as_bytes().to_vec()));
        m.parts
            .push(("docProps/app.xml".into(), app.as_bytes().to_vec()));
        m
    }

    /// The main part's XML.
    pub fn main_xml(&self) -> String {
        let items = self.items.concat();
        let head = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
        match self.format {
            Format::Docx => format!("{head}<w:document xmlns:w=\"{W}\" xmlns:r=\"{R}\" xmlns:a=\"{A}\"><w:body>{items}</w:body></w:document>"),
            Format::Pptx => format!("{head}<p:presentation xmlns:p=\"{P}\" xmlns:r=\"{R}\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst><p:sldIdLst>{items}</p:sldIdLst><p:sldSz cx=\"9144000\" cy=\"6858000\"/></p:presentation>"),
            Format::Xlsx => format!("{head}<workbook xmlns=\"{S}\" xmlns:r=\"{R}\"><sheets>{items}</sheets></workbook>"),
        }
    }

    pub fn types_xml(&self) -> String {
        let mut s = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"{PKG_TYPES}\">");
        for (e, c) in &self.defaults {
            s.push_str(&format!("<Default Extension=\"{e}\" ContentType=\"{c}\"/>"));
        }
        for (p, c) in &self.overrides {
            s.push_str(&format!("<Override PartName=\"{p}\" ContentType=\"{c}\"/>"));
        }
        s.push_str("</Types>");
        s
    }

    pub fn rels_xml(list: &[(String, String, String)]) -> String {
        let mut s = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"{PKG_RELS}\">");
        for (id, ty, target) in list {
            s.push_str(&format!(
                "<Relationship Id=\"{id}\" Type=\"{ty}\" Target=\"{target}\"/>"
            ));
        }
        s.push_str("</Relationships>");
        s
    }

    fn rels_name(source: &str) -> String {
        k2_mpc_opc::name::rels_part_for(source)
    }

    /// Every part in package order: content types, the package's relationships, the main
    /// part and its relationships, then each other part followed by its own.
    pub fn render(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = vec![(
            "[Content_Types].xml".to_owned(),
            self.types_xml().into_bytes(),
        )];
        let put_rels = |out: &mut Vec<(String, Vec<u8>)>, source: &str| {
            if let Some(l) = self.rels.get(source).filter(|l| !l.is_empty()) {
                out.push((Self::rels_name(source), Self::rels_xml(l).into_bytes()));
            }
        };
        put_rels(&mut out, "");
        out.push((self.format.main().to_owned(), self.main_xml().into_bytes()));
        put_rels(&mut out, self.format.main());
        for (n, c) in &self.parts {
            out.push((n.clone(), c.clone()));
            put_rels(&mut out, n);
        }
        out
    }

    pub fn add_part(&mut self, name: &str, content: Vec<u8>, ct: Option<&str>) {
        self.parts.push((name.to_owned(), content));
        if let Some(c) = ct {
            self.overrides.push((format!("/{name}"), c.to_owned()));
        }
    }

    pub fn set_part(&mut self, name: &str, content: Vec<u8>) {
        let p = self
            .parts
            .iter_mut()
            .find(|(n, _)| n == name)
            .expect("a part");
        p.1 = content;
    }

    pub fn remove_part(&mut self, name: &str) {
        self.parts.retain(|(n, _)| n != name);
        let o = format!("/{name}");
        self.overrides.retain(|(p, _)| *p != o);
        self.rels.remove(name);
    }

    pub fn add_rel(&mut self, source: &str, id: &str, kind: &str, target: &str) {
        self.rels.entry(source.to_owned()).or_default().push((
            id.to_owned(),
            rt(kind),
            target.to_owned(),
        ));
    }

    pub fn remove_rel(&mut self, source: &str, id: &str) {
        if let Some(l) = self.rels.get_mut(source) {
            l.retain(|r| r.0 != id);
        }
    }

    /// The package's bytes, laid out plainly.
    pub fn bytes(&self) -> Vec<u8> {
        package(&self.render(), Layout::Plain, 0)
    }
}

pub fn slide_xml(text: &str) -> Vec<u8> {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:sld xmlns:p=\"{P}\" xmlns:a=\"{A}\"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title 1\"/></p:nvSpPr><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>").into_bytes()
}

/// A package's bytes from its parts, in `layout`.
pub fn package(parts: &[(String, Vec<u8>)], layout: Layout, seed: u64) -> Vec<u8> {
    let mut r = Rng::new(seed + 99);
    let mut entries: Vec<ZEntry> = Vec::new();
    if layout == Layout::Utf8 {
        let mut dirs = BTreeSet::new();
        for (n, _) in parts {
            if let Some((d, _)) = n.split_once('/') {
                dirs.insert(format!("{d}/"));
            }
        }
        for d in dirs {
            let mut e = ZEntry::new(&d, b"");
            e.method = 0;
            entries.push(e);
        }
    }
    for (n, c) in parts {
        let mut e = ZEntry::new(n, c);
        match layout {
            Layout::Plain => {}
            Layout::Stored => e.method = 0,
            Layout::Max => {
                e.level = 9;
                e.made_by = 0x031E;
                e.external_attr = 0o100644 << 16;
            }
            Layout::DescSig => e.desc = Desc::Sig,
            Layout::DescNoSig => e.desc = Desc::NoSig,
            Layout::Extra => {
                e.local_extra = vec![0x55, 0x54, 5, 0, 1, 0x10, 0x20, 0x30, 0x40];
                e.central_extra = vec![0x55, 0x54, 1, 0, 1];
                e.comment = format!("entry {n}").into_bytes();
            }
            Layout::Utf8 => {}
            Layout::Mixed => {
                e.method = if r.below(3) == 0 { 0 } else { 8 };
                e.level = [1, 6, 9][r.below(3) as usize];
                e.desc = [Desc::None, Desc::Sig, Desc::NoSig][r.below(3) as usize];
                e.time = r.below(0xBF7F) as u16;
            }
        }
        entries.push(e);
    }
    if layout == Layout::Utf8 {
        // a part whose name needs UTF-8: covered by the xml default
        let mut e = ZEntry::new(
            "customXml/\u{00ed}tem\u{00e9}1.xml",
            b"<?xml version=\"1.0\"?><x/>",
        );
        e.utf8 = true;
        entries.push(e);
    }
    if layout == Layout::Mixed {
        entries.reverse();
    }
    let comment: &[u8] = if layout == Layout::Extra {
        b"an archive comment"
    } else {
        b""
    };
    build(&entries, comment)
}

/// Every default fixture: (name, bytes), three formats by eight layouts by two seeds.
pub fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for f in Format::ALL {
        for (li, l) in Layout::ALL.into_iter().enumerate() {
            for seed in 0..2u64 {
                let m = Model::new(f, seed);
                out.push((
                    format!("{}-{l:?}-{seed}", f.name()).to_lowercase(),
                    package(&m.render(), l, seed * 8 + li as u64),
                ));
            }
        }
    }
    out
}

/// A second zip reader's view of a package: (name, content), in directory order.
pub fn second_reader(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut a = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(a.len());
    for i in 0..a.len() {
        let mut f = a.by_index(i).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).map_err(|e| e.to_string())?;
        let name = f.name().map_err(|e| e.to_string())?.into_owned();
        out.push((name, v));
    }
    Ok(out)
}

pub fn limits() -> Limits {
    Limits::default()
}

/// One package's checks: it opens, writes back to itself, the second reader agrees on every
/// entry, and it has no package problem. `Err` names what failed.
pub fn package_holds(name_: &str, bytes: &[u8]) -> Result<(), String> {
    let l = limits();
    let p = k2_mpc_opc::OpcPackage::open(bytes, &l).map_err(|e| format!("{name_}: {e}"))?;
    if p.encode().map_err(|e| e.to_string())? != bytes {
        return Err(format!("{name_}: doesn't write back to itself"));
    }
    let theirs = second_reader(bytes).map_err(|e| format!("{name_}: second reader: {e}"))?;
    if theirs.len() != p.zip.entries().len() {
        return Err(format!("{name_}: the readers count entries differently"));
    }
    for (i, (n, content)) in theirs.iter().enumerate() {
        let e = &p.zip.entries()[i];
        if &e.name != n || p.zip.inflate(i).map_err(|e| e.to_string())? != *content {
            return Err(format!("{name_}: the readers disagree on {n}"));
        }
    }
    let view: BTreeMap<String, (&str, &[u8])> = p
        .parts
        .iter()
        .map(|(k, x)| (k.clone(), (x.name.as_str(), x.content.as_slice())))
        .collect();
    let found = k2_mpc_opc::package::problems(&view, &l);
    if !found.is_empty() {
        return Err(format!("{name_}: problems {found:?}"));
    }
    Ok(())
}

/// Every default fixture holds (the round-trip leg).
pub fn fixtures_hold() -> Result<(), String> {
    let all = fixtures();
    if all.len() != 48 {
        return Err(format!("{} fixtures, not 48", all.len()));
    }
    for (n, b) in &all {
        package_holds(n, b)?;
    }
    println!("fixtures: {} packages, each byte-exact", all.len());
    Ok(())
}

/// The test pack: merges the one list each format's main part holds (paragraphs, slide ids,
/// sheets) by element: theirs' items, less those ours deleted, then ours' new items. Every
/// other part both sides changed is a conflict.
pub struct ListPack;

const LISTS: [&str; 3] = ["body", "sldIdLst", "sheets"];

/// A list's items: each element's byte range and text.
type Items = Vec<(std::ops::Range<usize>, String)>;

fn list_items(text: &str) -> Option<(usize, Items)> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let list = doc
        .descendants()
        .find(|n| n.is_element() && LISTS.contains(&n.tag_name().name()))?;
    let r = list.range();
    let close = r.start + text[r.clone()].rfind("</")?;
    let items = list
        .children()
        .filter(|c| c.is_element())
        .map(|c| (c.range(), text[c.range()].to_owned()))
        .collect();
    Some((close, items))
}

impl PartMerge for ListPack {
    fn both_changed(
        &self,
        key: &str,
        base: Option<&[u8]>,
        ours: &[u8],
        theirs: &[u8],
    ) -> Resolution {
        let conflict = || Resolution::Conflict(format!("both sides changed {key}"));
        if !Format::ALL.iter().any(|f| f.main() == key) {
            return conflict();
        }
        let s = |b: &[u8]| String::from_utf8(b.to_vec()).ok();
        let (Some(b), Some(o), Some(t)) = (base.and_then(s), s(ours), s(theirs)) else {
            return conflict();
        };
        let (Some((_, bi)), Some((_, oi)), Some((close, ti))) =
            (list_items(&b), list_items(&o), list_items(&t))
        else {
            return conflict();
        };
        let set = |v: &[(std::ops::Range<usize>, String)]| -> BTreeSet<String> {
            v.iter().map(|x| x.1.clone()).collect()
        };
        let (bs, os, ts) = (set(&bi), set(&oi), set(&ti));
        let mut out = String::new();
        let mut at = 0;
        for (r, x) in &ti {
            if bs.contains(x) && !os.contains(x) {
                out.push_str(&t[at..r.start]);
                at = r.end;
            }
        }
        out.push_str(&t[at..close]);
        for (_, x) in &oi {
            if !bs.contains(x) && !ts.contains(x) {
                out.push_str(x);
            }
        }
        out.push_str(&t[close..]);
        Resolution::Merged(out.into_bytes())
    }
}

/// One write's plan holds: its ranges are in order, apart and inside the file; the file with
/// them applied (by this function, not the crate) is what the record-by-record writer gives;
/// and `write` gives the same. `Err` names what failed; `Ok` is the plan's range count.
pub fn plan_holds(
    name_: &str,
    p: &k2_mpc_opc::OpcPackage<'_>,
    changes: &BTreeMap<String, k2_mpc_opc::Change>,
) -> Result<usize, String> {
    let bytes = p.bytes();
    let plan = p
        .write_plan(changes)
        .map_err(|e| format!("{name_}: write_plan: {e}"))?;
    let mut end = None;
    for s in &plan {
        if end.is_some_and(|e| s.at <= e) {
            return Err(format!(
                "{name_}: ranges out of order or touching at {}",
                s.at
            ));
        }
        let e = s.at.checked_add(s.len).filter(|&e| e <= bytes.len());
        end = Some(e.ok_or_else(|| format!("{name_}: a range outside the file at {}", s.at))?);
    }
    // applied here, independently of the crate's own applier
    let mut applied = Vec::new();
    let mut at = 0;
    for s in &plan {
        applied.extend_from_slice(&bytes[at..s.at]);
        applied.extend_from_slice(&s.with);
        at = s.at + s.len;
    }
    applied.extend_from_slice(&bytes[at..]);
    let whole = p
        .write_by_records(changes)
        .map_err(|e| format!("{name_}: write_by_records: {e}"))?;
    if applied != whole {
        return Err(format!(
            "{name_}: the plan applied isn't the write ({} ranges, {} bytes against {})",
            plan.len(),
            applied.len(),
            whole.len()
        ));
    }
    if p.write(changes)
        .map_err(|e| format!("{name_}: write: {e}"))?
        != whole
    {
        return Err(format!("{name_}: write isn't its plan applied"));
    }
    Ok(plan.len())
}

/// The change sets the plan rows write on a package: none; the main part replaced; the first
/// entry replaced; a part removed; the last entry removed; a part added; all three at once.
pub fn plan_changes(
    p: &k2_mpc_opc::OpcPackage<'_>,
) -> Vec<(&'static str, BTreeMap<String, k2_mpc_opc::Change>)> {
    use k2_mpc_opc::{name, Change};
    let key_of = |i: usize| name::key(&p.zip.entries()[i].name);
    let n = p.zip.entries().len();
    let main = p
        .parts
        .keys()
        .find(|k| Format::ALL.iter().any(|f| f.main() == k.as_str()))
        .cloned()
        .unwrap_or_else(|| key_of(n / 2));
    let edited = |k: &str| {
        let mut c = p.parts[k].content.clone();
        c.extend_from_slice(b"<!-- planned -->");
        Change::Replace(c)
    };
    let one = |k: String, c: Change| BTreeMap::from([(k, c)]);
    let added = || Change::Add {
        name: "customXml/planned9.xml".into(),
        content: b"<?xml version=\"1.0\"?><item/>".to_vec(),
    };
    let first = key_of(0);
    let last = key_of(n - 1);
    let app = name::key("docProps/app.xml");
    let removed = if p.parts.contains_key(&app) {
        app
    } else {
        key_of(n / 2)
    };
    let mut all = one(main.clone(), edited(&main));
    all.insert(removed.clone(), Change::Remove);
    all.insert(name::key("customXml/planned9.xml"), added());
    vec![
        ("none", BTreeMap::new()),
        ("main replaced", one(main.clone(), edited(&main))),
        ("first replaced", one(first.clone(), edited(&first))),
        ("one removed", one(removed, Change::Remove)),
        ("last removed", one(last, Change::Remove)),
        (
            "one added",
            one(name::key("customXml/planned9.xml"), added()),
        ),
        ("all three", all),
    ]
}

/// Every default fixture's plans hold, for every change set (the plan leg). It fails unless
/// some plan left a gap of the file's own bytes between two ranges.
pub fn plans_hold() -> Result<(), String> {
    let l = limits();
    let (mut writes, mut ranges, mut gapped) = (0, 0, 0);
    for (n, b) in fixtures() {
        let p = k2_mpc_opc::OpcPackage::open(&b, &l).map_err(|e| format!("{n}: {e}"))?;
        for (what, ch) in plan_changes(&p) {
            let r = plan_holds(&format!("{n} ({what})"), &p, &ch)?;
            writes += 1;
            ranges += r;
            gapped += usize::from(r > 1);
        }
    }
    if writes == 0 || gapped == 0 {
        return Err(format!(
            "plans: {writes} writes, {gapped} with a gap: nothing exercised"
        ));
    }
    println!("plans: {writes} writes, {ranges} ranges ({gapped} writes with more than one), each applied equal to the write");
    Ok(())
}
