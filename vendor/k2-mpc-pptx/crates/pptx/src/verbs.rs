//! The agent's verbs: their definitions as data for the kit's registry ([`VERBS`],
//! [`registry`]), and what each one does to a deck ([`apply`], [`list`]).
//!
//! The host (K2) wires the command line and writes each result through the kit (`write
//! --base`), so an app's save that lands meanwhile merges by shape ([`crate::merge`]). Here is
//! what a verb means for one deck: a pure function from the deck's bytes to new bytes.
//!
//! **Splicing only.** A verb replaces the elements it changes (one shape, one slide list
//! entry, one transform's attributes) and adds the parts it makes (a slide, its relationships,
//! a notes slide); every other part is copied raw, and inside a changed part every other byte
//! stays. Nothing is re-serialised.
//!
//! **Ids** follow the app's rules: a new slide's `sldId` is one past the highest in the deck
//! (the deck's retired ids can't be seen, so a merge re-mints on collision), a new shape's
//! `cNvPr` id one past the highest on its slide. An existing id is never renumbered:
//! animations and connectors point at it.
//!
//! **Addresses** are the index's: `@sld:<sldId>` for a slide, `@sld:<sldId>/sp:<id>` for a
//! shape. Positions and sizes are in EMU.

use std::collections::BTreeMap;

use k2_mpc_base::addr::{self, Addr, Forms};
use k2_mpc_base::parity::{FileVerb, Registry, RegistryError, VerbKind};
use k2_mpc_base::tools::exit::{NOT_FOUND, OK, USAGE};
use k2_mpc_opc::name;
use k2_mpc_opc::Change;
use roxmltree::{Document, Node};

use crate::deck::{
    part_text, relative, Pptx, CT_NOTES, CT_SLIDE, REL_LAYOUT, REL_NOTES, REL_NOTES_MASTER,
    REL_SLIDE, SLIDE_ID_MAX, SLIDE_ID_MIN,
};
use crate::slide::{self, effective, find_shape, read_shapes, tree_node, Tree};
use crate::xmlx::{
    self, child, end_tag_start, escape_attr, escape_text, is, parse_xml, prefix_of, q,
    start_tag_end, A, MC, P, R,
};
use crate::{Decline, Limits};

/// The person actions the verbs serve (the host's parity rows, by name).
pub const ROWS: &[&str] = &[
    "browse slides",
    "add a slide",
    "reorder slides",
    "add a text box",
    "edit shape text",
    "move or resize a shape",
    "delete a shape",
    "write speaker notes",
    "resolve a clash",
];

const RD: VerbKind = VerbKind::Read;
const ED: VerbKind = VerbKind::Edit;
const READ: &[u8] = &[OK, USAGE, NOT_FOUND];
const EDIT: &[u8] = &[OK, USAGE, NOT_FOUND];

/// Every verb, as data for [`Registry::for_file`]: the live tier's, and the clash verbs. The
/// index's own verbs (`shape list`, `shape show`, `shape diff`, `shape blame`) come from the kit
/// ([`crate::index::VERBS`]); a deck that isn't live is served by [`registry`].
pub const VERBS: &[FileVerb] = &[
    FileVerb::new(
        "slide list",
        RD,
        "[--shapes] [--json] (each slide: address, position, layout, title, hidden, notes; \
         --shapes: each shape's address, kind, name, bounds and text)",
        READ,
        &["browse slides"],
    ),
    FileVerb::new(
        "slide add",
        ED,
        "--layout NAME|@layout:ID [--after @sld:ID | --first] (the layout's placeholders, \
         empty; default: last)",
        EDIT,
        &["add a slide"],
    ),
    FileVerb::new(
        "slide move",
        ED,
        "@sld:ID (--after @sld:ID | --first) (a deck with sections: exit 2)",
        EDIT,
        &["reorder slides"],
    ),
    FileVerb::new(
        "shape add",
        ED,
        "@sld:ID --kind textbox --at X,Y,W,H (EMU) --text TEXT (in front of the others)",
        EDIT,
        &["add a text box"],
    ),
    FileVerb::new(
        "shape text",
        ED,
        "@sld:ID/sp:N --text TEXT (the whole text body; each line a paragraph, keeping the \
         first paragraph's and run's formatting)",
        EDIT,
        &["edit shape text"],
    ),
    FileVerb::new(
        "shape move",
        ED,
        "@sld:ID/sp:N [--x X] [--y Y] [--w W] [--h H] (EMU; a shape placed by its layout \
         needs all four)",
        EDIT,
        &["move or resize a shape"],
    ),
    FileVerb::new(
        "shape delete",
        ED,
        "@sld:ID/sp:N (an animated or connected shape: exit 2)",
        EDIT,
        &["delete a shape"],
    ),
    FileVerb::new(
        "notes set",
        ED,
        "@sld:ID --text TEXT (makes the notes slide from the notes master when there is none)",
        EDIT,
        &["write speaker notes"],
    ),
    FileVerb::new(
        "conflicts list",
        RD,
        "[--json] (each held clash: id, address, why, base, person's, agent's)",
        READ,
        &["resolve a clash"],
    ),
    FileVerb::new(
        "conflict take",
        ED,
        "ID --agent|--person (the agent's is written only if the deck still holds the \
         person's)",
        EDIT,
        &["resolve a clash"],
    ),
];

/// The kit's registry for a deck that isn't live ([`Registry::for_blob_pack`] of
/// [`crate::PptxPack`]): the kit's tier-0 verbs, the index's (`shape list`, `shape show`,
/// `shape diff`, `shape blame`) and the deck's own ([`crate::blob_verbs::VERB_SET`]: `slide
/// list` and the seven edits). The clash verbs (`conflicts list`, `conflict take`) stay in
/// [`VERBS`] only, until the kit holds conflicts on tier 0.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::for_blob_pack(&crate::PptxPack::default())
}

/// Why a verb didn't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerbError {
    /// The deck can't be read by this pack (exit 2).
    Declined(Decline),
    /// Nothing at that address (exit 4).
    NotFound(String),
    /// A bad argument (exit 2).
    Usage(String),
    /// What this verb doesn't do to this deck (exit 2).
    Refused(String),
}

impl VerbError {
    pub fn exit(&self) -> u8 {
        match self {
            VerbError::NotFound(_) => NOT_FOUND,
            _ => USAGE,
        }
    }
}

impl std::fmt::Display for VerbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerbError::Declined(d) => write!(f, "{d}"),
            VerbError::NotFound(a) => write!(f, "nothing at {a}"),
            VerbError::Usage(m) | VerbError::Refused(m) => f.write_str(m),
        }
    }
}

impl From<Decline> for VerbError {
    fn from(d: Decline) -> Self {
        VerbError::Declined(d)
    }
}

fn usage(m: impl Into<String>) -> VerbError {
    VerbError::Usage(m.into())
}

fn refused(m: impl Into<String>) -> VerbError {
    VerbError::Refused(m.into())
}

/// Parse `@sld:ID` or `@sld:ID/sp:N`.
pub fn parse_address(s: &str) -> Result<(u32, Option<u32>), VerbError> {
    let bad = || usage(format!("{s:?} isn't @sld:ID or @sld:ID/sp:N"));
    match addr::parse(s, Forms::none().native()) {
        Ok(Addr::Native(segs)) => {
            let num = |v: &str| v.parse::<u32>().map_err(|_| bad());
            match segs.as_slice() {
                [(k, id)] if k == "sld" => Ok((num(id)?, None)),
                [(k, id), (k2, sp)] if k == "sld" && k2 == "sp" => Ok((num(id)?, Some(num(sp)?))),
                _ => Err(bad()),
            }
        }
        _ => Err(bad()),
    }
}

/// Where a slide goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    First,
    After(u32),
}

/// Which layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutSel {
    /// `@layout:ID` (its `sldLayoutId`).
    Id(u64),
    /// Its name, which must be unique.
    Name(String),
}

/// One edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// `slide add`: `place` `None` is last.
    AddSlide {
        layout: LayoutSel,
        place: Option<Place>,
    },
    /// `slide move`.
    MoveSlide { slide: u32, place: Place },
    /// `shape add --kind textbox`.
    AddTextBox {
        slide: u32,
        at: (i64, i64, i64, i64),
        text: String,
    },
    /// `shape text`.
    SetText {
        slide: u32,
        shape: u32,
        text: String,
    },
    /// `shape move`.
    MoveShape {
        slide: u32,
        shape: u32,
        x: Option<i64>,
        y: Option<i64>,
        w: Option<i64>,
        h: Option<i64>,
    },
    /// `shape delete`.
    DeleteShape { slide: u32, shape: u32 },
    /// `notes set`.
    SetNotes { slide: u32, text: String },
}

/// What an edit made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub bytes: Vec<u8>,
    /// The new slide's or shape's address, for an add.
    pub address: Option<String>,
}

/// The largest coordinate and size (`ST_Coordinate`, `ST_PositiveCoordinate`), in EMU.
pub const COORD_MAX: i64 = 27_273_042_316_900;

fn check_coord(v: i64, what: &str) -> Result<i64, VerbError> {
    if v.abs() > COORD_MAX {
        return Err(usage(format!("{what} is out of range")));
    }
    Ok(v)
}

fn check_size(v: i64, what: &str) -> Result<i64, VerbError> {
    if !(0..=COORD_MAX).contains(&v) {
        return Err(usage(format!("{what} is out of range")));
    }
    Ok(v)
}

/// Apply one edit to a deck's bytes.
pub fn apply(bytes: &[u8], edit: &Edit, limits: &Limits) -> Result<Applied, VerbError> {
    let p = Pptx::open(bytes, limits)?;
    let mut w = Work::new(&p, limits);
    w.edit(edit)?;
    let address = w.address.take();
    if w.changes.is_empty() {
        return Ok(Applied {
            bytes: bytes.to_vec(),
            address,
        });
    }
    let out = p.opc.write(&w.changes)?;
    // a guard on our own writer: what a verb writes, this pack reads
    Pptx::open(&out, limits).map_err(|d| {
        refused(format!(
            "the edit would make a deck this pack can't read: {d}"
        ))
    })?;
    Ok(Applied {
        bytes: out,
        address,
    })
}

/// One edit as the kit's tier-0 path takes it ([`plan`]): the ranges of the deck's bytes it
/// replaces, and the index's nodes (by key, [`crate::index`]) it changes and adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// In order and apart; empty when the edit changes nothing.
    pub spans: Vec<k2_mpc_opc::Span>,
    /// The new slide's or shape's address, for an add.
    pub address: Option<String>,
    /// The nodes it changes: the one it edits, the slide whose part holds it (a slide's node
    /// is its whole part), and those whose place moves with it (a slide's position, a shape's
    /// place in front of the ones behind it).
    pub touched: Vec<String>,
    /// The nodes it adds: a new slide with its shapes, a new shape, a new notes slide.
    pub created: Vec<String>,
}

/// Plan one edit: what [`apply`] writes, as ranges of `bytes` (the package layer's
/// `write_plan`, so every byte outside them is the deck's own), with the nodes it says it
/// changes and adds, worked out from the edit and the deck before it, never from the output.
/// The output is read back as [`apply`] reads it.
pub fn plan(bytes: &[u8], edit: &Edit, limits: &Limits) -> Result<Planned, VerbError> {
    let p = Pptx::open(bytes, limits)?;
    let mut w = Work::new(&p, limits);
    w.edit(edit)?;
    let address = w.address.take();
    if w.changes.is_empty() {
        return Ok(Planned {
            spans: Vec::new(),
            address,
            touched: Vec::new(),
            created: Vec::new(),
        });
    }
    let spans = p.opc.write_plan(&w.changes)?;
    let out = k2_mpc_opc::zip::apply(bytes, &spans)?;
    // a guard on our own writer: what a verb writes, this pack reads
    Pptx::open(&out, limits).map_err(|d| {
        refused(format!(
            "the edit would make a deck this pack can't read: {d}"
        ))
    })?;
    let (touched, created) = w.declared(edit, address.as_deref())?;
    Ok(Planned {
        spans,
        address,
        touched,
        created,
    })
}

/// The keys of `shapes` and every shape under them, on slide `sld`, in order.
fn shape_keys(shapes: &[slide::ShapeInfo], sld: u32, out: &mut Vec<String>) {
    for s in shapes {
        if let Some(id) = s.id {
            out.push(format!("@sld:{sld}/sp:{id}"));
        }
        shape_keys(&s.children, sld, out);
    }
}

/// Text for paragraphs: one per line (`\r\n` and `\r` read as a line end), each with the
/// paragraph properties `ppr` and run properties `rpr` given (as written).
fn paragraphs(a: &str, text: &str, ppr: &str, rpr: &str) -> Result<String, VerbError> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    for line in text.split('\n') {
        let esc = escape_text(line)
            .ok_or_else(|| usage("the text holds a character a deck can't carry"))?;
        if line.is_empty() {
            out.push_str(&format!("<{p}>{ppr}</{p}>", p = q(a, "p")));
        } else {
            out.push_str(&format!(
                "<{p}>{ppr}<{r}>{rpr}<{t}>{esc}</{t}></{r}></{p}>",
                p = q(a, "p"),
                r = q(a, "r"),
                t = q(a, "t")
            ));
        }
    }
    Ok(out)
}

/// `text` with `elem` appended as the root's last child.
fn append_to_root(text: &str, elem: &str, limits: &Limits) -> Result<String, VerbError> {
    let doc = parse_xml(text, limits)?;
    let root = doc.root_element();
    let (end, empty) = start_tag_end(text, root);
    let out = if empty {
        let qn = xmlx::qname(text, root);
        xmlx::splice(text, vec![(end - 2..end, format!(">{elem}</{qn}>"))])
    } else {
        let at = end_tag_start(text, root).ok_or_else(|| refused("a root without an end tag"))?;
        xmlx::splice(text, vec![(at..at, elem.to_owned())])
    };
    Ok(out)
}

/// The prefix bound to `ns` where `n` is, or `None`.
fn prefix_for<'a>(n: Node<'a, '_>, ns: &str) -> Option<&'a str> {
    n.lookup_prefix(ns)
}

struct Work<'p, 'a> {
    p: &'p Pptx<'a>,
    limits: &'p Limits,
    changes: BTreeMap<String, Change>,
    address: Option<String>,
    /// The id an add takes, when the live store fixed it already (the splice); else one past
    /// the highest, as the file verbs do.
    forced: Option<u32>,
    /// Slides as the edits so far left them, where they differ from the opened deck: one
    /// added, or given a notes slide. Several steps run on one open of the package (the
    /// splice's batch), so a later step finds what an earlier one made.
    slides: BTreeMap<u32, crate::deck::SlideRef>,
    /// Slides an earlier step deleted.
    deleted: std::collections::BTreeSet<u32>,
}

impl<'p, 'a> Work<'p, 'a> {
    fn new(p: &'p Pptx<'a>, limits: &'p Limits) -> Self {
        Work {
            p,
            limits,
            changes: BTreeMap::new(),
            address: None,
            forced: None,
            slides: BTreeMap::new(),
            deleted: std::collections::BTreeSet::new(),
        }
    }
}

impl Work<'_, '_> {
    fn slide(&self, id: u32) -> Result<&crate::deck::SlideRef, VerbError> {
        if self.deleted.contains(&id) {
            return Err(VerbError::NotFound(format!("@sld:{id}")));
        }
        self.slides
            .get(&id)
            .or_else(|| self.p.deck.slide(id))
            .ok_or_else(|| VerbError::NotFound(format!("@sld:{id}")))
    }

    /// A file verb's edit.
    fn edit(&mut self, edit: &Edit) -> Result<(), VerbError> {
        match edit {
            Edit::AddSlide { layout, place } => self.add_slide(layout, *place),
            Edit::MoveSlide { slide, place } => self.move_slide(*slide, *place),
            Edit::AddTextBox { slide, at, text } => self.add_text_box(*slide, *at, text),
            Edit::SetText { slide, shape, text } => {
                let key = self.slide(*slide)?.part.clone();
                self.set_text(&key, *slide, *shape, text)
            }
            Edit::MoveShape {
                slide,
                shape,
                x,
                y,
                w: cx,
                h: cy,
            } => self.move_shape(*slide, *shape, [*x, *y, *cx, *cy]),
            Edit::DeleteShape { slide, shape } => self.delete_shape(*slide, *shape),
            Edit::SetNotes { slide, text } => self.set_notes(*slide, text),
        }
    }

    /// One step of the live splice, on top of the steps before it.
    fn step(&mut self, step: &Step) -> Result<(), VerbError> {
        self.forced = None;
        match step {
            Step::Edit(e) => self.edit(e),
            Step::AddSlide { id, layout, place } => {
                self.forced = Some(*id);
                self.add_slide(&LayoutSel::Id(*layout), *place)
            }
            Step::AddTextBox {
                slide,
                id,
                at,
                text,
            } => {
                self.forced = Some(*id);
                self.add_text_box(*slide, *at, text)
            }
            Step::SetHidden { slide, hidden } => self.set_hidden(*slide, *hidden),
            Step::OrderShape {
                slide,
                shape,
                after,
            } => self.order_shape(*slide, *shape, *after),
            Step::DeleteSlide { slide } => self.delete_slide(*slide),
        }
    }

    /// A part's text as this edit leaves it so far.
    fn current(&self, key: &str) -> Result<String, VerbError> {
        match self.changes.get(key) {
            Some(Change::Replace(c)) | Some(Change::Add { content: c, .. }) => {
                String::from_utf8(c.clone()).map_err(|_| refused("not UTF-8"))
            }
            Some(Change::Remove) => Err(refused(format!("{key} was removed"))),
            None => Ok(part_text(&self.p.opc, key)?.to_owned()),
        }
    }

    fn replace(&mut self, key: &str, text: String) {
        self.changes
            .insert(key.to_owned(), Change::Replace(text.into_bytes()));
    }

    fn add(&mut self, name: &str, text: String) {
        self.changes.insert(
            name::key(name),
            Change::Add {
                name: name.to_owned(),
                content: text.into_bytes(),
            },
        );
    }

    /// Every part key the package has or this edit adds.
    fn taken(&self) -> std::collections::BTreeSet<String> {
        self.p
            .opc
            .parts
            .keys()
            .chain(self.changes.keys())
            .cloned()
            .collect()
    }

    /// Add a relationship from part `source` (a key) to the part named `target`: its
    /// relationships part is edited, or made. Returns the new id.
    fn add_rel(&mut self, source: &str, ty: &str, target: &str) -> Result<String, VerbError> {
        let src_name = match self.changes.get(source) {
            Some(Change::Add { name, .. }) => name.clone(),
            _ => self.p.name_of(source),
        };
        let rels_name = name::rels_part_for(&src_name);
        let rels_key = name::key(&rels_name);
        let tgt = relative(&src_name, target);
        let exists =
            self.p.opc.parts.contains_key(&rels_key) || self.changes.contains_key(&rels_key);
        if !exists {
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{}\" Target=\"{}\"/></Relationships>",
                escape_attr(ty),
                escape_attr(&tgt)
            );
            self.add(&rels_name, text);
            return Ok("rId1".into());
        }
        let text = self.current(&rels_key)?;
        let rels =
            k2_mpc_opc::rels::Relationships::parse(&rels_name, text.as_bytes(), self.limits)?;
        let mut n = rels
            .list
            .iter()
            .filter_map(|r| r.id.strip_prefix("rId").and_then(|x| x.parse::<u64>().ok()))
            .max()
            .unwrap_or(0)
            + 1;
        while rels.list.iter().any(|r| r.id == format!("rId{n}")) {
            n += 1;
        }
        let id = format!("rId{n}");
        let doc = parse_xml(&text, self.limits)?;
        let pre = prefix_of(&text, doc.root_element()).to_owned();
        let elem = format!(
            "<{} Id=\"{id}\" Type=\"{}\" Target=\"{}\"/>",
            q(&pre, "Relationship"),
            escape_attr(ty),
            escape_attr(&tgt)
        );
        let out = append_to_root(&text, &elem, self.limits)?;
        self.put(&rels_key, out);
        Ok(id)
    }

    /// Replace a part's text, keeping an add an add.
    fn put(&mut self, key: &str, text: String) {
        match self.changes.get_mut(key) {
            Some(Change::Add { content, .. }) => *content = text.into_bytes(),
            _ => self.replace(key, text),
        }
    }

    /// Add a content type override for the part named `name`.
    fn add_type(&mut self, name: &str, ct: &str) -> Result<(), VerbError> {
        let key = name::CONTENT_TYPES;
        let text = self.current(key)?;
        let doc = parse_xml(&text, self.limits)?;
        let pre = prefix_of(&text, doc.root_element()).to_owned();
        let elem = format!(
            "<{} PartName=\"/{}\" ContentType=\"{}\"/>",
            q(&pre, "Override"),
            escape_attr(name.strip_prefix('/').unwrap_or(name)),
            escape_attr(ct)
        );
        let out = append_to_root(&text, &elem, self.limits)?;
        self.put(key, out);
        Ok(())
    }

    fn add_slide(&mut self, sel: &LayoutSel, place: Option<Place>) -> Result<(), VerbError> {
        let p = self.p;
        // the layout
        let found: Vec<&crate::deck::LayoutRef> = p
            .deck
            .layouts()
            .map(|(_, l)| l)
            .filter(|l| match sel {
                LayoutSel::Id(id) => l.id == Some(*id),
                LayoutSel::Name(n) => l.name == *n,
            })
            .collect();
        let layout = match found.as_slice() {
            [one] => *one,
            [] => {
                return Err(VerbError::NotFound(match sel {
                    LayoutSel::Id(id) => format!("@layout:{id}"),
                    LayoutSel::Name(n) => format!("layout {n:?}"),
                }))
            }
            many => {
                let ids: Vec<String> = many
                    .iter()
                    .filter_map(|l| l.id.map(|i| format!("@layout:{i}")))
                    .collect();
                return Err(usage(format!(
                    "{} layouts have that name: name one by id ({})",
                    many.len(),
                    ids.join(", ")
                )));
            }
        };
        if let Some(Place::After(a)) = place {
            self.slide(a)?;
        }
        // the id: one past the highest, or the one the live store fixed
        let id = match self.forced {
            Some(f) if p.deck.slide(f).is_some() || self.slides.contains_key(&f) => {
                return Err(refused(format!("slide id {f} is taken")))
            }
            Some(f) => f,
            None => p
                .deck
                .slides
                .iter()
                .map(|s| s.id + 1)
                .chain(self.slides.keys().map(|k| k + 1))
                .max()
                .unwrap_or(SLIDE_ID_MIN)
                .max(SLIDE_ID_MIN),
        };
        if id > SLIDE_ID_MAX {
            return Err(refused("no slide id is left"));
        }
        // the part
        let dir = p
            .deck
            .slides
            .first()
            .and_then(|s| s.name.rsplit_once('/').map(|(d, _)| format!("{d}/")))
            .unwrap_or_else(|| "ppt/slides/".into());
        let first = format!("{dir}slide1.xml");
        let taken = self.taken();
        let slide_name = if taken.contains(&name::key(&first)) {
            name::mint(&first, &taken)
        } else {
            first
        };
        let layout_name = p.name_of(&layout.part);
        let ltext = p.text(&layout.part)?;
        let ldoc = parse_xml(ltext, self.limits)?;
        let mut shapes = String::new();
        let mut next = 2u32;
        if let Some(tree) = tree_node(ldoc.root_element()) {
            for c in tree.children().filter(|c| is(*c, P, "sp")) {
                let Some(ph) = child(c, P, "nvSpPr")
                    .and_then(|n| child(n, P, "nvPr"))
                    .and_then(|n| child(n, P, "ph"))
                else {
                    continue;
                };
                let ty = ph.attribute("type");
                if matches!(ty, Some("dt" | "ftr" | "sldNum" | "hdr")) {
                    continue;
                }
                let mut ph_xml = String::from("<p:ph");
                for a in ["type", "orient", "sz", "idx"] {
                    if let Some(v) = ph.attribute(a) {
                        ph_xml.push_str(&format!(" {a}=\"{}\"", escape_attr(v)));
                    }
                }
                ph_xml.push_str("/>");
                let nm = child(c, P, "nvSpPr")
                    .and_then(|n| child(n, P, "cNvPr"))
                    .and_then(|n| n.attribute("name"))
                    .unwrap_or("Placeholder");
                let body = if child(c, P, "txBody").is_some() {
                    "<p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody>"
                } else {
                    ""
                };
                shapes.push_str(&format!(
                    "<p:sp><p:nvSpPr><p:cNvPr id=\"{next}\" name=\"{}\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr>{ph_xml}</p:nvPr></p:nvSpPr><p:spPr/>{body}</p:sp>",
                    escape_attr(nm)
                ));
                next += 1;
            }
        }
        let sld = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:sld xmlns:a=\"{A}\" xmlns:r=\"{R}\" xmlns:p=\"{P}\"><p:cSld><p:spTree>{GRP}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
        );
        self.add(&slide_name, sld);
        let slide_key = name::key(&slide_name);
        self.add_rel(&slide_key, REL_LAYOUT, &layout_name)?;
        let main = p.main.clone();
        let rid = self.add_rel(&main, REL_SLIDE, &slide_name)?;
        self.add_type(&slide_name, CT_SLIDE)?;

        // the presentation's slide list (and its sections)
        let text = self.current(&main)?;
        let doc = parse_xml(&text, self.limits)?;
        let root = doc.root_element();
        let pp = prefix_of(&text, root).to_owned();
        let rdecl = match prefix_for(root, R) {
            Some(r) if !r.is_empty() => (r.to_owned(), String::new()),
            _ => ("r".to_owned(), format!(" xmlns:r=\"{R}\"")),
        };
        let entry = format!(
            "<{}{} id=\"{id}\" {}=\"{rid}\"/>",
            q(&pp, "sldId"),
            rdecl.1,
            q(&rdecl.0, "id")
        );
        let mut edits = Vec::new();
        match child(root, P, "sldIdLst") {
            Some(list) => {
                let (st, empty) = start_tag_end(&text, list);
                let items: Vec<Node<'_, '_>> =
                    list.children().filter(|c| is(*c, P, "sldId")).collect();
                if empty {
                    let qn = xmlx::qname(&text, list);
                    edits.push((st - 2..st, format!(">{entry}</{qn}>")));
                } else {
                    let at = match place {
                        Some(Place::First) => st,
                        Some(Place::After(a)) => items
                            .iter()
                            .find(|n| {
                                n.attribute("id").and_then(|v| v.parse::<u32>().ok()) == Some(a)
                            })
                            .map(|n| n.range().end)
                            .ok_or_else(|| VerbError::NotFound(format!("@sld:{a}")))?,
                        None => items.last().map_or(st, |n| n.range().end),
                    };
                    edits.push((at..at, entry));
                }
            }
            None => {
                let after = ["notesMasterIdLst", "handoutMasterIdLst", "sldMasterIdLst"]
                    .iter()
                    .find_map(|n| child(root, P, n))
                    .map(|n| n.range().end)
                    .unwrap_or_else(|| start_tag_end(&text, root).0);
                edits.push((
                    after..after,
                    format!("<{l}>{entry}</{l}>", l = q(&pp, "sldIdLst")),
                ));
            }
        }
        if let Some(sl) = root
            .descendants()
            .find(|d| d.is_element() && d.tag_name().name() == "sectionLst")
        {
            let sections: Vec<Node<'_, '_>> = sl
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "section")
                .collect();
            let lists: Vec<Node<'_, '_>> = sections
                .iter()
                .filter_map(|s| {
                    s.children()
                        .find(|c| c.is_element() && c.tag_name().name() == "sldIdLst")
                })
                .collect();
            let entry_of = |l: &Node<'_, '_>| -> String {
                format!("<{} id=\"{id}\"/>", q(prefix_of(&text, *l), "sldId"))
            };
            let target = match place {
                Some(Place::After(a)) => lists.iter().find_map(|l| {
                    l.children()
                        .find(|c| {
                            c.is_element()
                                && c.attribute("id").and_then(|v| v.parse::<u32>().ok()) == Some(a)
                        })
                        .map(|c| (*l, c.range().end))
                }),
                Some(Place::First) => lists.first().map(|l| (*l, start_tag_end(&text, *l).0)),
                None => lists.last().map(|l| {
                    let end = l
                        .children()
                        .rfind(|c| c.is_element())
                        .map_or(start_tag_end(&text, *l).0, |c| c.range().end);
                    (*l, end)
                }),
            };
            let (l, at) = target
                .ok_or_else(|| refused("the deck's sections don't name where the slide goes"))?;
            if start_tag_end(&text, l).1 {
                return Err(refused("an empty section's slide list"));
            }
            edits.push((at..at, entry_of(&l)));
        }
        let out = xmlx::splice(&text, edits);
        self.put(&main, out);
        self.slides.insert(
            id,
            crate::deck::SlideRef {
                id,
                rid,
                part: slide_key,
                name: slide_name,
                hidden: false,
                layout: Some(layout.part.clone()),
                notes: None,
            },
        );
        self.address = Some(format!("@sld:{id}"));
        Ok(())
    }

    fn move_slide(&mut self, id: u32, place: Place) -> Result<(), VerbError> {
        let p = self.p;
        self.slide(id)?;
        if let Place::After(a) = place {
            self.slide(a)?;
            if a == id {
                return Err(usage("a slide can't go after itself"));
            }
        }
        if p.deck.sections {
            return Err(refused(
                "the deck has sections: moving slides between them isn't in this tier",
            ));
        }
        let main = p.main.clone();
        let text = self.current(&main)?;
        let doc = parse_xml(&text, self.limits)?;
        let list =
            child(doc.root_element(), P, "sldIdLst").ok_or_else(|| refused("no slide list"))?;
        let items: Vec<Node<'_, '_>> = list.children().filter(|c| is(*c, P, "sldId")).collect();
        let idx = |want: u32| {
            items
                .iter()
                .position(|n| n.attribute("id").and_then(|v| v.parse::<u32>().ok()) == Some(want))
        };
        let i = idx(id).ok_or_else(|| VerbError::NotFound(format!("@sld:{id}")))?;
        let already = match place {
            Place::First => i == 0,
            Place::After(a) => idx(a).is_some_and(|j| j + 1 == i),
        };
        if already {
            return Ok(());
        }
        let el = items[i].range();
        let lead_start = if i == 0 {
            start_tag_end(&text, list).0
        } else {
            items[i - 1].range().end
        };
        let at = match place {
            Place::First => start_tag_end(&text, list).0,
            Place::After(a) => items[idx(a).expect("checked")].range().end,
        };
        let moved = text[el.clone()].to_owned();
        let out = xmlx::splice(
            &text,
            vec![(lead_start..el.end, String::new()), (at..at, moved)],
        );
        self.put(&main, out);
        Ok(())
    }

    fn add_text_box(
        &mut self,
        sld: u32,
        at: (i64, i64, i64, i64),
        text_in: &str,
    ) -> Result<(), VerbError> {
        let (x, y) = (check_coord(at.0, "x")?, check_coord(at.1, "y")?);
        let (cx, cy) = (
            check_size(at.2, "the width")?,
            check_size(at.3, "the height")?,
        );
        let key = self.slide(sld)?.part.clone();
        let text = self.current(&key)?;
        let doc = parse_xml(&text, self.limits)?;
        let tree = Tree::parse(&doc)?;
        let id = match self.forced {
            Some(f) if tree.all_ids().contains(&f) => {
                return Err(refused(format!("shape id {f} is taken on @sld:{sld}")))
            }
            Some(f) => f,
            None => tree
                .max_id()
                .checked_add(1)
                .ok_or_else(|| refused("no shape id is left"))?,
        };
        let tn = tree_node(doc.root_element()).ok_or_else(|| refused("no shape tree"))?;
        let pp = prefix_of(&text, tn).to_owned();
        let (pa, decl) = match prefix_for(tn, A) {
            Some(a) if !a.is_empty() => (a.to_owned(), String::new()),
            _ => ("a".to_owned(), format!(" xmlns:a=\"{A}\"")),
        };
        let rpr = format!("<{} lang=\"en-US\" dirty=\"0\"/>", q(&pa, "rPr"));
        let paras = paragraphs(&pa, text_in, "", &rpr)?;
        let e = |l: &str| q(&pp, l);
        let a = |l: &str| q(&pa, l);
        let sp = format!(
            "<{sp}{decl}><{nv}><{c} id=\"{id}\" name=\"TextBox {n}\"/><{cs} txBox=\"1\"/><{np}/></{nv}><{spr}><{xf}><{off} x=\"{x}\" y=\"{y}\"/><{ext} cx=\"{cx}\" cy=\"{cy}\"/></{xf}><{pg} prst=\"rect\"><{av}/></{pg}><{nf}/></{spr}><{tb}><{bp} wrap=\"square\" rtlCol=\"0\"><{sa}/></{bp}><{ls}/>{paras}</{tb}></{sp}>",
            sp = e("sp"),
            nv = e("nvSpPr"),
            c = e("cNvPr"),
            n = id - 1,
            cs = e("cNvSpPr"),
            np = e("nvPr"),
            spr = e("spPr"),
            xf = a("xfrm"),
            off = a("off"),
            ext = a("ext"),
            pg = a("prstGeom"),
            av = a("avLst"),
            nf = a("noFill"),
            tb = e("txBody"),
            bp = a("bodyPr"),
            sa = a("spAutoFit"),
            ls = a("lstStyle"),
        );
        let out = xmlx::splice(&text, vec![(tree.units_end..tree.units_end, sp)]);
        self.put(&key, out);
        self.address = Some(format!("@sld:{sld}/sp:{id}"));
        Ok(())
    }

    /// Replace the text body of shape `shape` in part `key` (a slide or a notes slide).
    fn set_text(
        &mut self,
        key: &str,
        sld: u32,
        shape: u32,
        text_in: &str,
    ) -> Result<(), VerbError> {
        let text = self.current(key)?;
        let doc = parse_xml(&text, self.limits)?;
        let node = find_shape(&doc, shape)
            .ok_or_else(|| VerbError::NotFound(format!("@sld:{sld}/sp:{shape}")))?;
        if is(node, MC, "AlternateContent") {
            return Err(refused("an alternate-content shape is edited whole only"));
        }
        if !is(node, P, "sp") {
            return Err(refused(format!(
                "@sld:{sld}/sp:{shape} is a {}, not a text shape",
                slide::read_shape(node).kind
            )));
        }
        let pp = prefix_of(&text, node).to_owned();
        let out = match child(node, P, "txBody") {
            Some(tb) => {
                let body_pr = child(tb, A, "bodyPr")
                    .ok_or_else(|| refused("a text body without its properties"))?;
                let pa = prefix_of(&text, body_pr).to_owned();
                let keep_end = child(tb, A, "lstStyle").unwrap_or(body_pr).range().end;
                let end = end_tag_start(&text, tb).ok_or_else(|| refused("an empty text body"))?;
                let first_p = child(tb, A, "p");
                let ppr = first_p
                    .and_then(|p| child(p, A, "pPr"))
                    .map_or("", |n| &text[n.range()]);
                let rpr = first_p
                    .and_then(|p| child(p, A, "r"))
                    .and_then(|r| child(r, A, "rPr"))
                    .map_or("", |n| &text[n.range()]);
                let paras = paragraphs(&pa, text_in, ppr, rpr)?;
                xmlx::splice(&text, vec![(keep_end..end, paras)])
            }
            None => {
                let after = child(node, P, "style")
                    .or_else(|| child(node, P, "spPr"))
                    .ok_or_else(|| refused("a shape without its properties"))?;
                let (pa, decl) = match prefix_for(node, A) {
                    Some(a) if !a.is_empty() => (a.to_owned(), String::new()),
                    _ => ("a".to_owned(), format!(" xmlns:a=\"{A}\"")),
                };
                let paras = paragraphs(&pa, text_in, "", "")?;
                let body = format!(
                    "<{tb}{decl}><{bp}/><{ls}/>{paras}</{tb}>",
                    tb = q(&pp, "txBody"),
                    bp = q(&pa, "bodyPr"),
                    ls = q(&pa, "lstStyle")
                );
                let at = after.range().end;
                xmlx::splice(&text, vec![(at..at, body)])
            }
        };
        self.put(key, out);
        Ok(())
    }

    fn move_shape(&mut self, sld: u32, shape: u32, v: [Option<i64>; 4]) -> Result<(), VerbError> {
        if v.iter().all(Option::is_none) {
            return Err(usage("give --x, --y, --w or --h"));
        }
        if let Some(x) = v[0] {
            check_coord(x, "x")?;
        }
        if let Some(y) = v[1] {
            check_coord(y, "y")?;
        }
        if let Some(w) = v[2] {
            check_size(w, "the width")?;
        }
        if let Some(h) = v[3] {
            check_size(h, "the height")?;
        }
        let key = self.slide(sld)?.part.clone();
        let text = self.current(&key)?;
        let doc = parse_xml(&text, self.limits)?;
        let node = find_shape(&doc, shape)
            .ok_or_else(|| VerbError::NotFound(format!("@sld:{sld}/sp:{shape}")))?;
        if is(node, MC, "AlternateContent") {
            return Err(refused("an alternate-content shape is edited whole only"));
        }
        let e = effective(node);
        let out = match slide::xfrm_node(e) {
            Some(x) => {
                let off = child(x, A, "off");
                let ext = child(x, A, "ext");
                let mut edits = Vec::new();
                for (i, (el, attr)) in [(off, "x"), (off, "y"), (ext, "cx"), (ext, "cy")]
                    .into_iter()
                    .enumerate()
                {
                    let Some(val) = v[i] else { continue };
                    let a = el
                        .and_then(|n| n.attribute_node(attr))
                        .ok_or_else(|| refused("a transform without its offset or extent"))?;
                    edits.push((a.range_value(), val.to_string()));
                }
                xmlx::splice(&text, edits)
            }
            None => {
                let [Some(x), Some(y), Some(w), Some(h)] = v else {
                    return Err(usage(
                        "the shape takes its place from its layout: give --x, --y, --w and --h",
                    ));
                };
                let props = if is(e, P, "grpSp") {
                    child(e, P, "grpSpPr")
                } else {
                    child(e, P, "spPr")
                }
                .ok_or_else(|| refused("a shape without its properties"))?;
                let pa = match prefix_for(props, A) {
                    Some(a) if !a.is_empty() => a.to_owned(),
                    _ => return Err(refused("no DrawingML prefix where the shape is")),
                };
                let xf = format!(
                    "<{xf}><{off} x=\"{x}\" y=\"{y}\"/><{ext} cx=\"{w}\" cy=\"{h}\"/></{xf}>",
                    xf = q(&pa, "xfrm"),
                    off = q(&pa, "off"),
                    ext = q(&pa, "ext")
                );
                let (st, empty) = start_tag_end(&text, props);
                if empty {
                    let qn = xmlx::qname(&text, props);
                    xmlx::splice(&text, vec![(st - 2..st, format!(">{xf}</{qn}>"))])
                } else {
                    xmlx::splice(&text, vec![(st..st, xf)])
                }
            }
        };
        self.put(&key, out);
        Ok(())
    }

    fn delete_shape(&mut self, sld: u32, shape: u32) -> Result<(), VerbError> {
        let key = self.slide(sld)?.part.clone();
        let text = self.current(&key)?;
        let doc = parse_xml(&text, self.limits)?;
        let tree = Tree::parse(&doc)?;
        let Some(i) = tree.find(shape) else {
            if find_shape(&doc, shape).is_some() {
                return Err(refused(
                    "the shape is inside a group, which is one shape in this tier",
                ));
            }
            return Err(VerbError::NotFound(format!("@sld:{sld}/sp:{shape}")));
        };
        if referenced(&doc, shape) {
            return Err(refused(
                "the shape is animated or connected: remove the animation or connector first",
            ));
        }
        let u = &tree.units[i];
        let out = xmlx::splice(&text, vec![(u.lead.start..u.range.end, String::new())]);
        self.put(&key, out);
        Ok(())
    }

    fn set_notes(&mut self, sld: u32, text_in: &str) -> Result<(), VerbError> {
        let s = self.slide(sld)?.clone();
        if let Some(nk) = &s.notes {
            let text = self.current(nk)?;
            let doc = parse_xml(&text, self.limits)?;
            let body = read_shapes(&doc).into_iter().find(|x| {
                x.placeholder
                    .as_ref()
                    .is_some_and(|p| p.kind.as_deref() == Some("body"))
            });
            let id = body
                .and_then(|b| b.id)
                .ok_or_else(|| refused("the notes slide has no notes placeholder"))?;
            return self.set_text(nk, sld, id, text_in);
        }
        let p = self.p;
        let master = p
            .deck
            .notes_master
            .clone()
            .ok_or_else(|| refused("the deck has no notes master to make notes from"))?;
        let taken = self.taken();
        let first = "ppt/notesSlides/notesSlide1.xml";
        let notes_name = if taken.contains(&name::key(first)) {
            name::mint(first, &taken)
        } else {
            first.to_owned()
        };
        let paras = paragraphs("a", text_in, "", "")?;
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<p:notes xmlns:a=\"{A}\" xmlns:r=\"{R}\" xmlns:p=\"{P}\"><p:cSld><p:spTree>{GRP}<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Slide Image Placeholder 1\"/><p:cNvSpPr><a:spLocks noGrp=\"1\" noRot=\"1\" noChangeAspect=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"sldImg\"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Notes Placeholder 2\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"
        );
        self.add(&notes_name, xml);
        let nkey = name::key(&notes_name);
        let master_name = p.name_of(&master);
        self.add_rel(&nkey, REL_NOTES_MASTER, &master_name)?;
        self.add_rel(&nkey, REL_SLIDE, &s.name)?;
        self.add_rel(&s.part, REL_NOTES, &notes_name)?;
        self.add_type(&notes_name, CT_NOTES)?;
        self.slides.insert(
            sld,
            crate::deck::SlideRef {
                notes: Some(nkey),
                ..s
            },
        );
        Ok(())
    }
}

impl Work<'_, '_> {
    /// The nodes an edit changes and adds ([`Planned`]), from the edit and the deck as it was
    /// opened; a new slide's shapes from the part this edit made.
    fn declared(
        &self,
        edit: &Edit,
        address: Option<&str>,
    ) -> Result<(Vec<String>, Vec<String>), VerbError> {
        let p = self.p;
        let sk = |id: u32| format!("@sld:{id}");
        let order: Vec<u32> = p.deck.slides.iter().map(|s| s.id).collect();
        let pos = |id: u32| order.iter().position(|x| *x == id);
        let made = || {
            address
                .map(str::to_owned)
                .ok_or_else(|| refused("the add made no address"))
        };
        let (mut touched, mut created) = (Vec::new(), Vec::new());
        match edit {
            Edit::AddSlide { place, .. } => {
                let addr = made()?;
                let (id, _) = parse_address(&addr)?;
                // every slide from where it goes on moves one place down
                let from = match place {
                    None => order.len(),
                    Some(Place::First) => 0,
                    Some(Place::After(a)) => pos(*a).map_or(order.len(), |i| i + 1),
                };
                touched.extend(order[from..].iter().map(|s| sk(*s)));
                created.push(addr);
                let part = self.slide(id)?.part.clone();
                let text = self.current(&part)?;
                let doc = parse_xml(&text, self.limits)?;
                shape_keys(&read_shapes(&doc), id, &mut created);
            }
            Edit::MoveSlide { slide, place } => {
                let mut after = order.clone();
                if let Some(i) = pos(*slide) {
                    after.remove(i);
                }
                let at = match place {
                    Place::First => 0,
                    Place::After(a) => after
                        .iter()
                        .position(|x| x == a)
                        .map_or(after.len(), |j| j + 1),
                };
                after.insert(at.min(after.len()), *slide);
                // each slide whose position changes
                touched.extend(
                    order
                        .iter()
                        .zip(&after)
                        .filter(|(b, a)| b != a)
                        .map(|(b, _)| sk(*b)),
                );
            }
            Edit::AddTextBox { slide, .. } => {
                touched.push(sk(*slide));
                created.push(made()?);
            }
            Edit::SetText { slide, shape, .. } | Edit::MoveShape { slide, shape, .. } => {
                touched.push(sk(*slide));
                touched.push(format!("@sld:{slide}/sp:{shape}"));
            }
            Edit::DeleteShape { slide, shape } => {
                touched.push(sk(*slide));
                let part = self.slide(*slide)?.part.clone();
                let text = part_text(&p.opc, &part)?;
                let doc = parse_xml(text, self.limits)?;
                let shapes = read_shapes(&doc);
                if let Some(i) = shapes.iter().position(|s| s.id == Some(*shape)) {
                    // the shape and what it holds go; each shape in front of it moves back one
                    shape_keys(&shapes[i..=i], *slide, &mut touched);
                    touched.extend(
                        shapes[i + 1..]
                            .iter()
                            .filter_map(|s| s.id)
                            .map(|id| format!("@sld:{slide}/sp:{id}")),
                    );
                }
            }
            Edit::SetNotes { slide, .. } => {
                let had = p.deck.slide(*slide).is_some_and(|s| s.notes.is_some());
                let key = format!("@notes:{slide}");
                if had {
                    touched.push(key);
                } else {
                    created.push(key);
                }
            }
        }
        // planted: only the node the edit names is declared, not its slide or what moves
        // with it
        if crate::mutant::active("pptx_declares_too_little") {
            let own = match edit {
                Edit::SetText { slide, shape, .. }
                | Edit::MoveShape { slide, shape, .. }
                | Edit::DeleteShape { slide, shape } => Some(format!("@sld:{slide}/sp:{shape}")),
                Edit::MoveSlide { slide, .. } => Some(sk(*slide)),
                _ => None,
            };
            touched.retain(|k| Some(k) == own.as_ref());
        }
        Ok((touched, created))
    }
}

/// Whether an animation (`spid`) or a connector end (`a:stCxn`, `a:endCxn`) names shape `id`.
pub(crate) fn referenced(doc: &Document<'_>, id: u32) -> bool {
    doc.descendants().filter(|n| n.is_element()).any(|n| {
        let v = |a: &str| n.attribute(a).and_then(|x| x.trim().parse::<u32>().ok());
        v("spid") == Some(id) || ((is(n, A, "stCxn") || is(n, A, "endCxn")) && v("id") == Some(id))
    })
}

const GRP: &str = "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>";

/// One slide, for `slide list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlideSummary {
    pub address: String,
    /// From 1.
    pub position: usize,
    /// Its layout's name.
    pub layout: Option<String>,
    pub title: String,
    pub hidden: bool,
    pub notes: Option<String>,
    pub shapes: Vec<ShapeSummary>,
}

/// One shape, for `slide list --shapes`, back to front, a group's shapes after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeSummary {
    pub address: Option<String>,
    pub kind: &'static str,
    pub name: String,
    /// x, y, w, h in EMU.
    pub bounds: Option<(i64, i64, i64, i64)>,
    pub text: Option<String>,
    /// 0 on the slide, 1 in a group...
    pub depth: usize,
}

/// `slide list`: every slide, with its shapes.
pub fn list(bytes: &[u8], limits: &Limits) -> Result<Vec<SlideSummary>, VerbError> {
    let p = Pptx::open(bytes, limits)?;
    let names: BTreeMap<&str, &str> = p
        .deck
        .layouts()
        .map(|(_, l)| (l.part.as_str(), l.name.as_str()))
        .collect();
    let mut out = Vec::new();
    for (i, s) in p.deck.slides.iter().enumerate() {
        let doc = parse_xml(p.text(&s.part)?, limits)?;
        let shapes = read_shapes(&doc);
        let mut flat = Vec::new();
        fn walk(v: &[slide::ShapeInfo], sld: u32, depth: usize, out: &mut Vec<ShapeSummary>) {
            for x in v {
                out.push(ShapeSummary {
                    address: x.id.map(|id| format!("@sld:{sld}/sp:{id}")),
                    kind: x.kind,
                    name: x.name.clone(),
                    bounds: x.xfrm.map(|f| (f.x, f.y, f.cx, f.cy)),
                    text: x.text.clone(),
                    depth,
                });
                walk(&x.children, sld, depth + 1, out);
            }
        }
        walk(&shapes, s.id, 0, &mut flat);
        let notes = match &s.notes {
            Some(n) => {
                let nd = parse_xml(p.text(n)?, limits)?;
                slide::placeholder_text(&read_shapes(&nd), &["body"])
            }
            None => None,
        };
        out.push(SlideSummary {
            address: format!("@sld:{}", s.id),
            position: i + 1,
            layout: s
                .layout
                .as_deref()
                .and_then(|l| names.get(l))
                .map(|n| (*n).to_owned()),
            title: slide::placeholder_text(&shapes, &["title", "ctrTitle"]).unwrap_or_default(),
            hidden: s.hidden,
            notes,
            shapes: flat,
        });
    }
    Ok(out)
}

/// `slide list` as text: one line per slide (and per shape, indented, with `--shapes`).
pub fn render(slides: &[SlideSummary], shapes: bool) -> String {
    let one = |s: &str| s.replace('\n', " / ");
    let mut out = String::new();
    for s in slides {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}{}\n",
            s.position,
            s.address,
            s.layout.as_deref().unwrap_or("-"),
            one(&s.title),
            if s.hidden { "\t(hidden)" } else { "" }
        ));
        if shapes {
            for x in &s.shapes {
                out.push_str(&format!(
                    "{}  {}\t{}\t{}{}\n",
                    "  ".repeat(x.depth),
                    x.address.as_deref().unwrap_or("(no id)"),
                    x.kind,
                    one(&x.name),
                    x.text
                        .as_deref()
                        .map_or(String::new(), |t| format!("\t{}", one(t)))
                ));
            }
        }
    }
    out
}

// ---------- the live splice's steps ----------

/// One step of the live splice ([`crate::live::splice`]): the file verbs' edits with the ids
/// the live store fixed, and the splices only the live store needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// `slide add` with its `sldId`; `place` `None` is last.
    AddSlide {
        id: u32,
        layout: u64,
        place: Option<Place>,
    },
    /// `shape add --kind textbox` with its `cNvPr` id.
    AddTextBox {
        slide: u32,
        id: u32,
        at: (i64, i64, i64, i64),
        text: String,
    },
    /// Any file verb's edit, as it is.
    Edit(Edit),
    /// Show or hide a slide (`show="0"`).
    SetHidden { slide: u32, hidden: bool },
    /// Put a top-level shape right after another (`None`: at the back).
    OrderShape {
        slide: u32,
        shape: u32,
        after: Option<u32>,
    },
    /// Remove a slide: its list entry, relationship, part, notes and content types.
    DeleteSlide { slide: u32 },
}

/// Steps of the live splice applied on **one open** of a deck's package: each step sees what
/// the steps before it made (their parts' text, the slides they added, the notes slides they
/// made), and the package is written once ([`Batch::write`]).
pub(crate) struct Batch<'p, 'a>(Work<'p, 'a>);

impl<'p, 'a> Batch<'p, 'a> {
    pub(crate) fn new(p: &'p Pptx<'a>, limits: &'p Limits) -> Self {
        Batch(Work::new(p, limits))
    }

    /// Apply one more step.
    pub(crate) fn step(&mut self, step: &Step) -> Result<(), VerbError> {
        self.0.step(step)
    }

    /// The package with every step's changes, written once; `None` when no step changed
    /// anything. No reading guard here: the caller reads the result back.
    pub(crate) fn write(self) -> Result<Option<Vec<u8>>, VerbError> {
        if self.0.changes.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.0.p.opc.write(&self.0.changes)?))
    }
}

/// The text with one child of `parent` (the first `pred` takes) cut out, with the bytes
/// between it and the child before it.
fn cut_child(
    text: &str,
    parent: Node<'_, '_>,
    pred: impl Fn(Node<'_, '_>) -> bool,
) -> Option<String> {
    let kids: Vec<Node<'_, '_>> = parent.children().filter(|c| c.is_element()).collect();
    let i = kids.iter().position(|c| pred(*c))?;
    let start = if i == 0 {
        start_tag_end(text, parent).0
    } else {
        kids[i - 1].range().end
    };
    Some(xmlx::splice(
        text,
        vec![(start..kids[i].range().end, String::new())],
    ))
}

impl Work<'_, '_> {
    fn set_hidden(&mut self, sld: u32, hidden: bool) -> Result<(), VerbError> {
        let key = self.slide(sld)?.part.clone();
        let text = self.current(&key)?;
        let doc = parse_xml(&text, self.limits)?;
        let root = doc.root_element();
        let out = match (root.attribute_node("show"), hidden) {
            (Some(a), true) => xmlx::splice(&text, vec![(a.range_value(), "0".into())]),
            (None, true) => {
                let (end, empty) = start_tag_end(&text, root);
                let at = if empty { end - 2 } else { end - 1 };
                xmlx::splice(&text, vec![(at..at, " show=\"0\"".into())])
            }
            (Some(a), false) => {
                let r = a.range();
                let start = if text[..r.start].ends_with(' ') {
                    r.start - 1
                } else {
                    r.start
                };
                xmlx::splice(&text, vec![(start..r.end, String::new())])
            }
            (None, false) => return Ok(()),
        };
        self.put(&key, out);
        Ok(())
    }

    fn order_shape(&mut self, sld: u32, shape: u32, after: Option<u32>) -> Result<(), VerbError> {
        let key = self.slide(sld)?.part.clone();
        let text = self.current(&key)?;
        let doc = parse_xml(&text, self.limits)?;
        let tree = Tree::parse(&doc)?;
        let i = tree
            .find(shape)
            .ok_or_else(|| VerbError::NotFound(format!("@sld:{sld}/sp:{shape}")))?;
        let at = match after {
            None => tree.units_start,
            Some(a) => {
                let j = tree
                    .find(a)
                    .ok_or_else(|| VerbError::NotFound(format!("@sld:{sld}/sp:{a}")))?;
                if j == i {
                    return Err(usage("a shape can't go after itself"));
                }
                tree.units[j].range.end
            }
        };
        let u = &tree.units[i];
        let moved = text[u.lead.start..u.range.end].to_owned();
        let out = xmlx::splice(
            &text,
            vec![(u.lead.start..u.range.end, String::new()), (at..at, moved)],
        );
        self.put(&key, out);
        Ok(())
    }

    /// Leave a part out, with its relationships part and its content type override.
    fn remove_part(&mut self, key: &str) -> Result<(), VerbError> {
        let name = self.p.name_of(key);
        self.changes.insert(key.to_owned(), Change::Remove);
        let rkey = name::key(&name::rels_part_for(&name));
        if self.p.opc.parts.contains_key(&rkey) {
            self.changes.insert(rkey, Change::Remove);
        }
        let ct = name::CONTENT_TYPES;
        let text = self.current(ct)?;
        let doc = parse_xml(&text, self.limits)?;
        let part = format!("/{}", name.strip_prefix('/').unwrap_or(&name));
        if let Some(out) = cut_child(&text, doc.root_element(), |c| {
            c.tag_name().name() == "Override"
                && c.attribute("PartName")
                    .is_some_and(|p| p.eq_ignore_ascii_case(&part))
        }) {
            self.put(ct, out);
        }
        Ok(())
    }

    fn delete_slide(&mut self, sld: u32) -> Result<(), VerbError> {
        let s = self.slide(sld)?.clone();
        if self.p.deck.slide(sld).is_none() {
            return Err(refused(
                "a slide added in this change can't be deleted in it",
            ));
        }
        if self.p.deck.sections {
            return Err(refused(
                "the deck has sections: deleting slides from them isn't in this tier",
            ));
        }
        let main = self.p.main.clone();
        let text = self.current(&main)?;
        let doc = parse_xml(&text, self.limits)?;
        let list =
            child(doc.root_element(), P, "sldIdLst").ok_or_else(|| refused("no slide list"))?;
        let out = cut_child(&text, list, |c| {
            is(c, P, "sldId") && c.attribute("id").and_then(|v| v.parse::<u32>().ok()) == Some(sld)
        })
        .ok_or_else(|| VerbError::NotFound(format!("@sld:{sld}")))?;
        self.put(&main, out);
        let rkey = name::key(&name::rels_part_for(&self.p.name_of(&main)));
        let rtext = self.current(&rkey)?;
        let rdoc = parse_xml(&rtext, self.limits)?;
        if let Some(out) = cut_child(&rtext, rdoc.root_element(), |c| {
            c.attribute("Id") == Some(s.rid.as_str())
        }) {
            self.put(&rkey, out);
        }
        self.remove_part(&s.part)?;
        if let Some(n) = &s.notes {
            let shared = self
                .p
                .deck
                .slides
                .iter()
                .any(|o| o.id != sld && o.notes.as_deref() == Some(n.as_str()));
            if !shared {
                self.remove_part(n)?;
            }
        }
        self.deleted.insert(sld);
        Ok(())
    }
}
