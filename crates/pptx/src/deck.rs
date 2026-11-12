//! Opening a `.pptx`: the package (the shared package layer's [`OpcPackage`]: every part's
//! content, held to its CRC), the deck (slides in presentation order by `sldId`, masters and
//! their layouts, the notes master), and the format's refusals, each a typed [`Decline`]:
//!
//! | Code | What |
//! |---|---|
//! | the package layer's codes | the zip, `encrypted`, `doctype`, `xml_encoding`, `zip_layout`... |
//! | `macros` | a VBA part or a macro-enabled content type |
//! | `strict` | Strict OOXML |
//! | `not_pptx` | no content types, or the main part isn't a presentation |
//! | `no_presentation` | no main part, or it isn't in the package |
//! | `template` | a template |
//! | `xml_invalid` | a part this pack reads doesn't parse, or has the wrong root |
//! | `bad_rels` | a relationships part this pack reads repeats an id |
//! | `bad_slide_list` | a slide id out of range or repeated, or one that doesn't name a slide part |
//! | `bad_slide` | a slide, notes slide, layout or master without a shape tree it can split |
//!
//! A declined file stays at tier 0: presence, a lease, versions and conflict copies.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_opc::rels::Relationships;
use k2_mpc_opc::OpcPackage;
use roxmltree::Document;

use crate::slide::Tree;
use crate::xmlx::{child, children, is, parse_xml, P, P_STRICT, R};
use crate::{Decline, Limits};

/// The main part of a presentation.
pub const CT_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
/// The main part of a show (`.ppsx`): the same structure.
pub const CT_SHOW: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow.main+xml";
pub const CT_TEMPLATE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.template.main+xml";
pub const CT_SLIDE: &str = "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
pub const CT_LAYOUT: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml";
pub const CT_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml";
pub const CT_NOTES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
pub const CT_NOTES_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";

pub const REL_OFFICE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
pub const REL_OFFICE_STRICT: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument";
pub const REL_SLIDE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
pub const REL_LAYOUT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
pub const REL_MASTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
pub const REL_NOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
pub const REL_NOTES_MASTER: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster";

/// The lowest and highest slide id (`ST_SlideId`).
pub const SLIDE_ID_MIN: u32 = 256;
pub const SLIDE_ID_MAX: u32 = 2_147_483_647;

/// One slide, in presentation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideRef {
    /// Its `sldId`: native and durable (a reorder doesn't change it, a deleted one isn't reused).
    pub id: u32,
    /// The relationship from the presentation part.
    pub rid: String,
    /// The slide part's key.
    pub part: String,
    /// The slide part's name as written.
    pub name: String,
    /// `show="0"`.
    pub hidden: bool,
    /// Its layout part's key.
    pub layout: Option<String>,
    /// Its notes slide part's key.
    pub notes: Option<String>,
}

/// One layout of a master.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutRef {
    /// Its `sldLayoutId`.
    pub id: Option<u64>,
    pub part: String,
    /// `p:cSld/@name`.
    pub name: String,
    /// `type` (`title`, `obj`...).
    pub kind: Option<String>,
}

/// One master and its layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterRef {
    /// Its `sldMasterId`.
    pub id: Option<u64>,
    pub part: String,
    pub layouts: Vec<LayoutRef>,
}

/// The deck's structure.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Deck {
    pub slides: Vec<SlideRef>,
    pub masters: Vec<MasterRef>,
    /// The notes master's key.
    pub notes_master: Option<String>,
    /// `p:sldSz` in EMU.
    pub size: Option<(i64, i64)>,
    /// The presentation holds a section list.
    pub sections: bool,
}

impl Deck {
    /// The slide with this id.
    pub fn slide(&self, id: u32) -> Option<&SlideRef> {
        self.slides.iter().find(|s| s.id == id)
    }

    /// Every layout, with its master's key.
    pub fn layouts(&self) -> impl Iterator<Item = (&MasterRef, &LayoutRef)> {
        self.masters
            .iter()
            .flat_map(|m| m.layouts.iter().map(move |l| (m, l)))
    }

    /// The slide id that owns each slide part.
    pub fn slide_of_part(&self) -> BTreeMap<String, u32> {
        self.slides.iter().map(|s| (s.part.clone(), s.id)).collect()
    }
}

/// An open `.pptx`.
#[derive(Debug, Clone)]
pub struct Pptx<'a> {
    pub opc: OpcPackage<'a>,
    /// The presentation part's key.
    pub main: String,
    pub deck: Deck,
}

fn invalid(what: &str) -> Decline {
    Decline::new("xml_invalid", what.to_owned())
}

fn bad_list(what: impl Into<String>) -> Decline {
    Decline::new("bad_slide_list", what)
}

/// The text of a part, by key.
pub fn part_text<'p>(opc: &'p OpcPackage<'_>, key: &str) -> Result<&'p str, Decline> {
    let p = opc
        .parts
        .get(key)
        .ok_or_else(|| Decline::new("xml_invalid", format!("{key} isn't in the package")))?;
    std::str::from_utf8(&p.content)
        .map_err(|_| Decline::new("xml_encoding", format!("{:?} isn't UTF-8", p.name)))
}

/// The relationships of a part (by key), by id; none when it has no relationships part.
pub fn rels_of(opc: &OpcPackage<'_>, key: &str, limits: &Limits) -> Result<Relationships, Decline> {
    let name = opc.parts.get(key).map_or(key, |p| p.name.as_str());
    let r = opc.relationships(name, limits)?.unwrap_or_default();
    if r.by_id().is_none() {
        return Err(Decline::new(
            "bad_rels",
            format!("the relationships of {key} repeat an id"),
        ));
    }
    Ok(r)
}

/// The key of the internal part relationship `rid` of `key` names, with its type.
pub fn target_of(
    opc: &OpcPackage<'_>,
    rels: &Relationships,
    source_key: &str,
    rid: &str,
) -> Option<(String, String)> {
    let source = opc
        .parts
        .get(source_key)
        .map_or(source_key, |p| p.name.as_str());
    let r = rels.list.iter().find(|r| r.id == rid)?;
    let t = r.target_key(source)?;
    if opc.parts.contains_key(&t) {
        Some((t, r.rel_type.clone()))
    } else {
        None
    }
}

/// The first internal target of a type in a part's relationships.
fn first_target(
    opc: &OpcPackage<'_>,
    rels: &Relationships,
    source_key: &str,
    rel_type: &str,
) -> Option<String> {
    let source = opc
        .parts
        .get(source_key)
        .map_or(source_key, |p| p.name.as_str());
    rels.list
        .iter()
        .filter(|r| r.rel_type == rel_type && !r.external)
        .find_map(|r| r.target_key(source).filter(|k| opc.parts.contains_key(k)))
}

/// Parse a part as XML and split its shape tree.
fn check_tree(opc: &OpcPackage<'_>, key: &str, limits: &Limits) -> Result<(), Decline> {
    let text = part_text(opc, key)?;
    let doc = parse_xml(text, limits)?;
    Tree::parse(&doc)?;
    Ok(())
}

impl<'a> Pptx<'a> {
    /// Open a file. The package layer inflates and checks every entry, and the package must
    /// write back to exactly these bytes; if it couldn't, it is declined rather than merged.
    pub fn open(bytes: &'a [u8], limits: &Limits) -> Result<Pptx<'a>, Decline> {
        let opc = OpcPackage::open_with(bytes, limits, |name, _| {
            if name.to_ascii_lowercase().ends_with("vbaproject.bin") {
                Err(Decline::new("macros", "the package holds macros"))
            } else {
                Ok(())
            }
        })
        .map_err(|d| match d.code {
            "no_content_types" => Decline::new("not_pptx", "the package has no content types"),
            _ => d,
        })?;
        if opc
            .types
            .overrides
            .values()
            .chain(opc.types.defaults.values())
            .any(|t| t.contains("macroEnabled") || t.contains("vbaProject"))
        {
            return Err(Decline::new("macros", "a macro-enabled content type"));
        }
        let rels = opc.relationships("", limits)?.unwrap_or_default();
        if rels.list.iter().any(|r| r.rel_type == REL_OFFICE_STRICT) {
            return Err(Decline::new("strict", "Strict OOXML"));
        }
        let main = rels
            .of_type(REL_OFFICE)
            .and_then(|r| r.target_key(""))
            .filter(|k| opc.parts.contains_key(k))
            .ok_or_else(|| Decline::new("no_presentation", "no main part in the package"))?;
        match opc.types.of(&main) {
            Some(CT_MAIN) | Some(CT_SHOW) => {}
            Some(CT_TEMPLATE) => return Err(Decline::new("template", "a template")),
            _ => {
                return Err(Decline::new(
                    "not_pptx",
                    "the main part isn't a presentation",
                ))
            }
        }
        let deck = read_deck(&opc, &main, limits)?;
        Ok(Pptx { opc, main, deck })
    }

    /// The file's bytes, written back from the package: identical to what was opened.
    pub fn encode(&self) -> Result<Vec<u8>, Decline> {
        self.opc.encode()
    }

    /// A part's text, by key.
    pub fn text(&self, key: &str) -> Result<&str, Decline> {
        part_text(&self.opc, key)
    }

    /// A part's name as written, by key.
    pub fn name_of(&self, key: &str) -> String {
        self.opc
            .parts
            .get(key)
            .map_or_else(|| key.to_owned(), |p| p.name.clone())
    }
}

fn read_deck(opc: &OpcPackage<'_>, main: &str, limits: &Limits) -> Result<Deck, Decline> {
    let text = part_text(opc, main)?;
    let doc: Document<'_> = parse_xml(text, limits)?;
    let root = doc.root_element();
    if root.tag_name().namespace() == Some(P_STRICT) {
        return Err(Decline::new("strict", "Strict OOXML"));
    }
    if !is(root, P, "presentation") {
        return Err(Decline::new(
            "not_pptx",
            "the main part isn't a presentation",
        ));
    }
    let rels = rels_of(opc, main, limits)?;
    let mut deck = Deck {
        sections: root
            .descendants()
            .any(|d| d.is_element() && d.tag_name().name() == "sectionLst"),
        size: child(root, P, "sldSz").map(|s| {
            let n = |a: &str| -> i64 {
                s.attribute(a)
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0)
            };
            (n("cx"), n("cy"))
        }),
        ..Deck::default()
    };

    // slides, in presentation order
    let mut ids = BTreeSet::new();
    let mut parts = BTreeSet::new();
    if let Some(list) = child(root, P, "sldIdLst") {
        for s in list.children().filter(|c| c.is_element()) {
            if !is(s, P, "sldId") {
                return Err(bad_list("the slide list holds another element"));
            }
            let id: u32 = s
                .attribute("id")
                .and_then(|v| v.trim().parse().ok())
                .filter(|v| (SLIDE_ID_MIN..=SLIDE_ID_MAX).contains(v))
                .ok_or_else(|| bad_list("a slide id is out of range"))?;
            let rid = s
                .attribute((R, "id"))
                .ok_or_else(|| bad_list("a slide has no relationship"))?;
            let (part, ty) = target_of(opc, &rels, main, rid)
                .ok_or_else(|| bad_list("a slide's relationship names no part"))?;
            if ty != REL_SLIDE || opc.types.of(&part) != Some(CT_SLIDE) {
                return Err(bad_list("a slide's relationship doesn't name a slide"));
            }
            if !ids.insert(id) {
                return Err(bad_list(format!("slide id {id} repeats")));
            }
            if !parts.insert(part.clone()) {
                return Err(bad_list("two slide ids name one slide part"));
            }
            let stext = part_text(opc, &part)?;
            let sdoc = parse_xml(stext, limits)?;
            let sroot = sdoc.root_element();
            if !is(sroot, P, "sld") {
                return Err(invalid("a slide part's root isn't a slide"));
            }
            Tree::parse(&sdoc)?;
            let srels = rels_of(opc, &part, limits)?;
            let notes = first_target(opc, &srels, &part, REL_NOTES);
            if let Some(n) = &notes {
                check_tree(opc, n, limits)?;
            }
            deck.slides.push(SlideRef {
                id,
                rid: rid.to_owned(),
                name: opc.parts[&part].name.clone(),
                hidden: sroot
                    .attribute("show")
                    .is_some_and(|v| v == "0" || v == "false"),
                layout: first_target(opc, &srels, &part, REL_LAYOUT),
                notes,
                part,
            });
        }
    }

    // masters and their layouts
    if let Some(list) = child(root, P, "sldMasterIdLst") {
        for m in children(list, P, "sldMasterId") {
            let Some((part, _)) = m
                .attribute((R, "id"))
                .and_then(|rid| target_of(opc, &rels, main, rid))
            else {
                continue;
            };
            let mtext = part_text(opc, &part)?;
            let mdoc = parse_xml(mtext, limits)?;
            Tree::parse(&mdoc)?;
            let mrels = rels_of(opc, &part, limits)?;
            let mut layouts = Vec::new();
            if let Some(ll) = child(mdoc.root_element(), P, "sldLayoutIdLst") {
                for l in children(ll, P, "sldLayoutId") {
                    let Some((lpart, _)) = l
                        .attribute((R, "id"))
                        .and_then(|rid| target_of(opc, &mrels, &part, rid))
                    else {
                        continue;
                    };
                    let ltext = part_text(opc, &lpart)?;
                    let ldoc = parse_xml(ltext, limits)?;
                    Tree::parse(&ldoc)?;
                    let lroot = ldoc.root_element();
                    layouts.push(LayoutRef {
                        id: l.attribute("id").and_then(|v| v.trim().parse().ok()),
                        name: child(lroot, P, "cSld")
                            .and_then(|c| c.attribute("name"))
                            .unwrap_or("")
                            .to_owned(),
                        kind: lroot.attribute("type").map(str::to_owned),
                        part: lpart,
                    });
                }
            }
            deck.masters.push(MasterRef {
                id: m.attribute("id").and_then(|v| v.trim().parse().ok()),
                part,
                layouts,
            });
        }
    }

    // the notes master
    deck.notes_master = child(root, P, "notesMasterIdLst")
        .and_then(|l| child(l, P, "notesMasterId"))
        .and_then(|n| n.attribute((R, "id")))
        .and_then(|rid| target_of(opc, &rels, main, rid))
        .filter(|(k, _)| opc.types.of(k) == Some(CT_NOTES_MASTER))
        .map(|(k, _)| k);
    Ok(deck)
}

/// The relative target from the part `from` (a name) to the part `to` (a name), as a
/// relationships part writes it (`slides/slide3.xml`, `../slideLayouts/slideLayout1.xml`).
pub fn relative(from: &str, to: &str) -> String {
    let from = from.strip_prefix('/').unwrap_or(from);
    let to = to.strip_prefix('/').unwrap_or(to);
    let fdir: Vec<&str> = match from.rsplit_once('/') {
        Some((d, _)) => d.split('/').collect(),
        None => Vec::new(),
    };
    let tsegs: Vec<&str> = to.split('/').collect();
    let (tdir, tfile) = tsegs.split_at(tsegs.len() - 1);
    let common = fdir
        .iter()
        .zip(tdir.iter())
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count();
    let mut out: Vec<&str> = vec![".."; fdir.len() - common];
    out.extend(&tdir[common..]);
    out.push(tfile[0]);
    out.join("/")
}
