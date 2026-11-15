//! The view index on a board-shaped toy pack: pages
//! (containers) and items (units), live as keyed roots and on the blob tier as a structure index,
//! one file read both ways. Public API only.
//!
//! The rows: the schema gives the expected index from both sources at one version, and the two
//! are identical but for their source fields; addresses round-trip, and the presence setters and
//! claims speak them; the paged form reassembles. With a carrier (a variant of the toy whose
//! file holds lines the live store never models): the derive hook reads it, N reads of an
//! unchanged doc decode it zero times (one decode per carrier), a change reaches what is
//! derived from it, and a keyed watch's change frame carries the render hash of the page it
//! re-rendered, read through the same kept index. Planted faults (`k2_mpc_base::differential::ViewMutant`) are caught by
//! name: each `plant_*_is_caught` row needs its check to pass as built and to fail under the
//! fault where it says.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{take_carrier_decodes, with_view_mutant, ViewMutant};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::MapOp;
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::index::{
    Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs, NodeRect,
};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, BlobPack, BlobSession, Config, ConnId, Declined, EditOptions, ElemFocus, Host,
    Joined, Pack, PackContent, PackError, Participant, ParticipantKind, Rights, Session,
    SessionError, SessionManager, Stay, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{
    key_candidates, Bbox, BlobView, Container, ContainerFields, Derived, EditableText, FieldClass,
    Flip, FontUse, GeomPart, Geometry, Layout, NodeSrc, Page, PageSpec, Placeholder, Src, Tier,
    Unit, UnitFields, UnitText, ViewIndex, ViewPageError, ViewSchema, ViewSource, VIEW_PAGE_BUDGET,
    VIEW_PAGE_MIN, VIEW_TEXT_MAX,
};
use k2_mpc_base::view_index::{LayoutRefused, ViewRefusal};
use k2_mpc_base::{Key, Value};

const PAGES: &str = "pages";
const ITEMS: &str = "items";
const PAGE_F: &str = "page_f";
const ITEM_F: &str = "item_f";
const BOARD: &str = "board";

const SCHEMA: Schema = Schema {
    name: "board-lite",
    roots: &[
        (PAGES, Kind::MovableList),
        (ITEMS, Kind::MovableList),
        (PAGE_F, Kind::Map),
        (ITEM_F, Kind::Map),
        (BOARD, Kind::Map),
    ],
};

const PARTS: &[GeomPart] = &[
    GeomPart::X,
    GeomPart::Y,
    GeomPart::W,
    GeomPart::H,
    GeomPart::Rot,
    GeomPart::FlipH,
    GeomPart::FlipV,
];

const EDIT: &[(&str, &str)] = &[("1", "plain"), ("fmt", "formatted"), ("0", "never")];

/// The toy's view schema: pages hold items; an item's `page` field holds its page's `id`.
const VIEW: ViewSchema = ViewSchema {
    containers: PAGES,
    units: ITEMS,
    unit_container_field: "page",
    container_id_field: Some("id"),
    native_unit: "px",
    page: Some(PageSpec {
        root: BOARD,
        w: "w",
        h: "h",
    }),
    container_fields: ContainerFields {
        title: Src::Field("title"),
        hidden: Src::Field("hidden"),
        layout: Src::Field("layout"),
        parent: Src::None,
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        name: Src::Field("name"),
        alt: Src::Field("alt"),
        text: Src::Field("text"),
        hidden: Src::Field("hidden"),
        editable_text: Src::Values {
            field: "edit",
            values: EDIT,
        },
        placeholder: Src::Field("ph"),
    },
    geometry: Some(Geometry {
        field: "geom",
        parts: PARTS,
        rot_unit: "60000th-degree",
    }),
    field_class: &[
        ("geom", FieldClass::Geometry),
        ("text", FieldClass::Text),
        ("hidden", FieldClass::Visibility),
        ("name", FieldClass::Meta),
    ],
    field_class_prefix: &[],
    grid: None,
    derived: Derived {
        containers: &[],
        units: &["name_is_generic"],
    },
    blob: Some(BlobView {
        container_kind: "page",
        skip_kinds: &["note"],
        page: Some(("w", "h")),
        layout_kind: Some("layout"),
        container_title: NodeSrc::Name,
        container_layout: NodeSrc::Attr("layout"),
        unit_alt: NodeSrc::Attr("alt"),
        unit_rot: NodeSrc::Attr("rot"),
        unit_flip_h: NodeSrc::Attr("fh"),
        unit_flip_v: NodeSrc::Attr("fv"),
        unit_editable_text: NodeSrc::Attr("edit"),
        placeholder_type: NodeSrc::Attr("ph_type"),
        placeholder_idx: NodeSrc::Attr("ph_idx"),
        ..BlobView::NONE
    }),
    unit_derived: &[],
    container_derived: &[],
    space: None,
    nesting: &[],
    time_unit: None,
};

// ----- the toy's file -----

/// One page line: `P|id|title|hidden|layout`.
struct PageLine {
    id: u32,
    title: String,
    hidden: bool,
    layout: Option<String>,
}

/// One item line: `I|page|id|kind|name|alt|geom|text|hidden|edit|ph` (`-` for none; geom
/// empty for none).
struct ItemLine {
    page: u32,
    id: u32,
    kind: String,
    name: String,
    alt: Option<String>,
    geom: String,
    text: Option<String>,
    hidden: bool,
    edit: String,
    ph: Option<String>,
}

struct Board {
    w: String,
    h: String,
    pages: Vec<PageLine>,
    items: Vec<ItemLine>,
    /// `L|id|name|master`.
    layouts: Vec<(u32, String, String)>,
    /// `N|page|text`: a page's note (on the blob tier only).
    notes: Vec<(u32, String)>,
}

fn opt(s: &str) -> Option<String> {
    (s != "-").then(|| s.to_owned())
}

fn parse(bytes: &[u8]) -> Result<Board, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not utf-8".to_owned())?;
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().unwrap_or("").split('|').collect();
    let ["BOARD", w, h] = head[..] else {
        return Err("no BOARD header".into());
    };
    let mut b = Board {
        w: w.to_owned(),
        h: h.to_owned(),
        pages: Vec::new(),
        items: Vec::new(),
        layouts: Vec::new(),
        notes: Vec::new(),
    };
    let num = |s: &str| s.parse::<u32>().map_err(|_| format!("bad number {s:?}"));
    for line in lines {
        let f: Vec<&str> = line.split('|').collect();
        match f[..] {
            ["P", id, title, hidden, layout] => b.pages.push(PageLine {
                id: num(id)?,
                title: title.to_owned(),
                hidden: hidden == "1",
                layout: opt(layout),
            }),
            ["I", page, id, kind, name, alt, geom, text, hidden, edit, ph] => {
                b.items.push(ItemLine {
                    page: num(page)?,
                    id: num(id)?,
                    kind: kind.to_owned(),
                    name: name.to_owned(),
                    alt: opt(alt),
                    geom: geom.to_owned(),
                    text: opt(text),
                    hidden: hidden == "1",
                    edit: edit.to_owned(),
                    ph: opt(ph),
                })
            }
            ["L", id, name, master] => b.layouts.push((num(id)?, name.into(), master.into())),
            ["N", page, text] => b.notes.push((num(page)?, text.into())),
            _ => return Err(format!("bad line {line:?}")),
        }
    }
    Ok(b)
}

fn page_key(id: u32) -> Key {
    Key::new(format!("p{id}")).unwrap()
}

fn item_key(page: u32, id: u32) -> Key {
    Key::new(format!("p{page}-{id}")).unwrap()
}

fn prop(k: &Key, f: &str) -> String {
    format!("{k}\u{1f}{f}")
}

/// A number with no sign and no leading zero.
fn canon_num(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}

/// The toy names an item generically when its name ends in a space and a number.
fn generic(name: &str) -> bool {
    name.rsplit_once(' ')
        .is_some_and(|(w, n)| !w.is_empty() && canon_num(n).is_some())
}

/// The live side.
struct BoardPack;

impl Pack for BoardPack {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(PAGE_F, PAGES), (ITEM_F, ITEMS)]
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let b = parse(bytes).map_err(|m| Declined {
            code: "not_board",
            message: m,
        })?;
        let (mut pages, mut items) = (Vec::new(), Vec::new());
        let (mut pf, mut itf, mut board) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        board.insert("w".to_owned(), Value::str(&b.w));
        board.insert("h".to_owned(), Value::str(&b.h));
        for (id, name, master) in &b.layouts {
            board.insert(format!("layout/{id}/name"), Value::str(name));
            board.insert(format!("layout/{id}/master"), Value::str(master));
        }
        for p in &b.pages {
            let k = page_key(p.id);
            pf.insert(prop(&k, "id"), Value::str(p.id.to_string()));
            pf.insert(prop(&k, "title"), Value::str(&p.title));
            pf.insert(
                prop(&k, "hidden"),
                Value::str(if p.hidden { "1" } else { "0" }),
            );
            if let Some(l) = &p.layout {
                pf.insert(prop(&k, "layout"), Value::str(l));
            }
            pages.push((k, Value::str("page")));
        }
        for i in &b.items {
            let k = item_key(i.page, i.id);
            itf.insert(prop(&k, "page"), Value::str(i.page.to_string()));
            itf.insert(prop(&k, "name"), Value::str(&i.name));
            itf.insert(prop(&k, "geom"), Value::str(&i.geom));
            itf.insert(
                prop(&k, "hidden"),
                Value::str(if i.hidden { "1" } else { "0" }),
            );
            itf.insert(prop(&k, "edit"), Value::str(&i.edit));
            for (f, v) in [("alt", &i.alt), ("text", &i.text), ("ph", &i.ph)] {
                if let Some(v) = v {
                    itf.insert(prop(&k, f), Value::str(v));
                }
            }
            items.push((k, Value::str(&i.kind)));
        }
        Ok(PackContent {
            roots: vec![
                (PAGES.into(), Neutral::List(pages)),
                (ITEMS.into(), Neutral::List(items)),
                (PAGE_F.into(), Neutral::Map(pf)),
                (ITEM_F.into(), Neutral::Map(itf)),
                (BOARD.into(), Neutral::Map(board)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VIEW)
    }
    /// The layouts from the board map (live), and each item's generic name (both tiers).
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        derive(source, index);
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match (root, key.as_str().strip_prefix('p')) {
            (PAGES, Some(id)) => format!("@pg:{id}"),
            (ITEMS, Some(rest)) => match rest.split_once('-') {
                Some((p, i)) => format!("@pg:{p}/it:{i}"),
                None => key.to_string(),
            },
            _ => key.to_string(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        let rest = address.strip_prefix("@pg:")?;
        match (root, rest.split_once("/it:")) {
            (PAGES, None) => Some(page_key(canon_num(rest)?)),
            (ITEMS, Some((p, i))) => Some(item_key(canon_num(p)?, canon_num(i)?)),
            _ => None,
        }
    }
}

fn derive(source: ViewSource<'_>, index: &mut ViewIndex) {
    if let ViewSource::Live(state, _) = source {
        let board = state.roots.iter().find_map(|(r, root)| match root {
            k2_mpc_base::elem::KeyedRoot::Map(m) if r == BOARD => Some(m),
            _ => None,
        });
        let mut ids = BTreeSet::new();
        for k in board.into_iter().flat_map(|m| m.keys()) {
            if let Some(id) = k
                .strip_prefix("layout/")
                .and_then(|r| r.split_once('/'))
                .and_then(|(id, _)| id.parse::<u32>().ok())
            {
                ids.insert(id);
            }
        }
        let get = |k: String| {
            board
                .and_then(|m| m.get(&k))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        for id in ids {
            index.layouts.push(Layout {
                key: format!("@lay:{id}"),
                name: get(format!("layout/{id}/name")).unwrap_or_default(),
                master: get(format!("layout/{id}/master")),
            });
        }
    }
    for u in &mut index.units {
        u.name_is_generic = Some(generic(&u.name));
    }
}

/// The blob side: the same file read into a structure index.
struct BoardIndex;

const VERBS: IndexVerbs =
    IndexVerbs::new("page list", "page show", "page diff", "page blame", None);

impl BlobPack for BoardIndex {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("board-lite", 1, &VERBS))
    }
    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        _limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let mut bytes = Vec::new();
        src.read_to_end(&mut bytes).map_err(IndexRefused::Read)?;
        let b = parse(&bytes).map_err(|why| IndexRefused::Unreadable { why })?;
        let mut d = IndexDraft::new("board-1");
        d.attrs.insert("w".into(), b.w.clone());
        d.attrs.insert("h".into(), b.h.clone());
        for p in &b.pages {
            let mut n = DraftNode::new("page", p.title.clone(), Digest::of(p.title.as_bytes()));
            n.key = Some(format!("@pg:{}", p.id));
            n.visible = !p.hidden;
            if let Some(l) = &p.layout {
                n.attrs.insert("layout".into(), l.clone());
            }
            d.nodes.push(n);
            let page_ix = (d.nodes.len() - 1) as u32;
            // top to bottom: the front first (the file lists them back to front)
            for i in b.items.iter().filter(|i| i.page == p.id).rev() {
                let mut n = DraftNode::new(
                    i.kind.clone(),
                    i.name.clone(),
                    Digest::of(i.name.as_bytes()),
                );
                n.key = Some(format!("@pg:{}/it:{}", i.page, i.id));
                n.parent = Some(page_ix);
                n.visible = !i.hidden;
                n.text = i.text.clone();
                let g: Vec<&str> = i.geom.split(',').collect();
                if let [x, y, w, h, rot, fh, fv] = g[..] {
                    let n64 = |s: &str| s.parse::<i64>().unwrap();
                    n.bounds = Some(NodeRect {
                        left: n64(x),
                        top: n64(y),
                        right: n64(x) + n64(w),
                        bottom: n64(y) + n64(h),
                    });
                    n.attrs.insert("rot".into(), rot.into());
                    n.attrs.insert("fh".into(), fh.into());
                    n.attrs.insert("fv".into(), fv.into());
                }
                if let Some(a) = &i.alt {
                    n.attrs.insert("alt".into(), a.clone());
                }
                n.attrs.insert("edit".into(), i.edit.clone());
                if let Some(ph) = &i.ph {
                    let (ty, idx) = ph.split_once(':').unwrap_or((ph, ""));
                    n.attrs.insert("ph_type".into(), ty.into());
                    if !idx.is_empty() {
                        n.attrs.insert("ph_idx".into(), idx.into());
                    }
                }
                d.nodes.push(n);
            }
            for (_, t) in b.notes.iter().filter(|(pg, _)| *pg == p.id) {
                let mut n = DraftNode::new("note", "", Digest::of(t.as_bytes()));
                n.key = Some(format!("@note:{}", p.id));
                n.parent = Some(page_ix);
                n.text = Some(t.clone());
                d.nodes.push(n);
            }
        }
        let mut masters: BTreeMap<String, u32> = BTreeMap::new();
        for (id, name, master) in &b.layouts {
            let m = match masters.get(master) {
                Some(m) => *m,
                None => {
                    let mut n = DraftNode::new("master", "", Digest::of(master.as_bytes()));
                    n.key = Some(master.clone());
                    d.nodes.push(n);
                    let m = (d.nodes.len() - 1) as u32;
                    masters.insert(master.clone(), m);
                    m
                }
            };
            let mut n = DraftNode::new("layout", name.clone(), Digest::of(name.as_bytes()));
            n.key = Some(format!("@lay:{id}"));
            n.parent = Some(m);
            d.nodes.push(n);
        }
        Ok(d)
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VIEW)
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        derive(source, index);
    }
}

/// A pack that declares no addresses: its addresses are its keys.
struct Plain;

impl Pack for Plain {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn decode(&self, _: &[u8]) -> Result<PackContent, Declined> {
        Err(Declined {
            code: "unused",
            message: "unused".into(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

/// A text longer than the view keeps.
fn long_text() -> String {
    "x".repeat(VIEW_TEXT_MAX + 476)
}

/// The sample board: three pages (one hidden, one empty), four items (page 2's listed between
/// page 1's), two layouts on one master, a note.
fn sample() -> Vec<u8> {
    format!(
        "BOARD|1600|900\n\
         P|1|Intro|0|@lay:1\n\
         P|2|Second|1|-\n\
         P|3|Empty|0|@lay:2\n\
         I|1|1|box|Box 1|-|10,20,300,200,0,0,0|Hello|0|1|title:\n\
         I|1|2|text|Caption|A note on it|400,20,300,100,5400000,1,0|{}|1|fmt|-\n\
         I|2|1|box|Box 1|-|0,0,10,10,0,0,1|-|0|0|:12\n\
         I|1|3|pic|Photo|-||-|0|0|body:1\n\
         L|1|Title Slide|@mst:1\n\
         L|2|Blank|@mst:1\n\
         N|1|speaker words\n",
        long_text()
    )
    .into_bytes()
}

/// What the sample's index must be, from either source.
fn expected(tier: Tier, rev: Option<u64>, version: Option<u64>) -> ViewIndex {
    let c = |key: &str, position, title: &str, hidden, layout: Option<&str>, n| Container {
        key: key.into(),
        position,
        title: Some(title.into()),
        hidden,
        layout: layout.map(Into::into),
        unit_count: n,
        ..Container::default()
    };
    let text = |s: &str, truncated| {
        Some(UnitText {
            summary: s.into(),
            truncated,
        })
    };
    let ph = |ty: &str, idx: Option<&str>| {
        Some(Placeholder {
            ty: ty.into(),
            idx: idx.map(Into::into),
        })
    };
    let bbox = |x, y, w, h| Some(Bbox { x, y, w, h });
    ViewIndex {
        tier,
        rev,
        version,
        page: Some(Page {
            w: 1600,
            h: 900,
            unit: "px".into(),
        }),
        fonts: Vec::new(),
        space: None,
        layouts: vec![
            Layout {
                key: "@lay:1".into(),
                name: "Title Slide".into(),
                master: Some("@mst:1".into()),
            },
            Layout {
                key: "@lay:2".into(),
                name: "Blank".into(),
                master: Some("@mst:1".into()),
            },
        ],
        containers: vec![
            c("@pg:1", 1, "Intro", false, Some("@lay:1"), 3),
            c("@pg:2", 2, "Second", true, None, 1),
            c("@pg:3", 3, "Empty", false, Some("@lay:2"), 0),
        ],
        units: vec![
            Unit {
                key: "@pg:1/it:1".into(),
                container: "@pg:1".into(),
                live: true,
                kind: "box".into(),
                name: "Box 1".into(),
                name_is_generic: Some(true),
                z: 0,
                bbox: bbox(10, 20, 300, 200),
                text: text("Hello", false),
                placeholder: ph("title", None),
                editable_text: Some(EditableText::Plain),
                ..Unit::default()
            },
            Unit {
                key: "@pg:1/it:2".into(),
                container: "@pg:1".into(),
                live: true,
                kind: "text".into(),
                name: "Caption".into(),
                name_is_generic: Some(false),
                alt: Some("A note on it".into()),
                z: 1,
                bbox: bbox(400, 20, 300, 100),
                rot: 5_400_000,
                flip: Flip { h: true, v: false },
                text: text(&"x".repeat(VIEW_TEXT_MAX), true),
                hidden: true,
                editable_text: Some(EditableText::Formatted),
                ..Unit::default()
            },
            Unit {
                key: "@pg:1/it:3".into(),
                container: "@pg:1".into(),
                live: true,
                kind: "pic".into(),
                name: "Photo".into(),
                name_is_generic: Some(false),
                z: 2,
                placeholder: ph("body", Some("1")),
                editable_text: Some(EditableText::Never),
                ..Unit::default()
            },
            Unit {
                key: "@pg:2/it:1".into(),
                container: "@pg:2".into(),
                live: true,
                kind: "box".into(),
                name: "Box 1".into(),
                name_is_generic: Some(true),
                z: 0,
                bbox: bbox(0, 0, 10, 10),
                flip: Flip { h: false, v: true },
                placeholder: ph("", Some("12")),
                editable_text: Some(EditableText::Never),
                ..Unit::default()
            },
        ],
        ..ViewIndex::default()
    }
}

// ----- the fixture -----

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(std::path::PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    mgr: SessionManager,
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-view-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(100_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c).with_blob_store(blobs);
    Fx {
        _dir: TmpDir(dir),
        mgr,
    }
}

fn key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

impl Fx {
    fn live(&self, bytes: &[u8]) -> Arc<Session> {
        self.mgr
            .open(Arc::new(BoardPack), &key("sample.board"), Some(bytes))
            .unwrap()
    }
    fn blob(&self, bytes: &[u8]) -> Arc<BlobSession> {
        let s = self
            .mgr
            .open_blob(
                Arc::new(BoardIndex),
                &key("sample.blob"),
                Some(&mut &bytes[..]),
            )
            .unwrap();
        s.index_job(None)
            .unwrap()
            .expect("a job is due")
            .run()
            .expect("the job records");
        s
    }
}

fn join(s: &Session, pid: &str, kind: ParticipantKind) -> Joined {
    s.join(
        Participant::new(pid, pid, "#2f9e8f", kind, "tab"),
        Stay::Held,
    )
    .unwrap()
}

fn join_blob(s: &BlobSession, pid: &str) -> ConnId {
    s.join(
        Participant::new(pid, pid, "#123456", ParticipantKind::Person, "tab"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

/// Every place two JSON values differ, by path (`units[1].hidden`), each once.
fn diff(path: &str, got: &Json, want: &Json, out: &mut Vec<String>) {
    match (got, want) {
        (Json::Obj(a), Json::Obj(b)) => {
            let mut keys: Vec<&String> = a.iter().map(|(k, _)| k).collect();
            for (k, _) in b {
                if !keys.contains(&k) {
                    keys.push(k);
                }
            }
            for k in keys {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match (got.get(k), want.get(k)) {
                    (Some(x), Some(y)) => diff(&p, x, y, out),
                    (x, y) => out.push(format!(
                        "{p}: got {}, want {}",
                        x.map_or("(none)".into(), Json::to_json),
                        y.map_or("(none)".into(), Json::to_json)
                    )),
                }
            }
        }
        (Json::List(a), Json::List(b)) => {
            if a.len() != b.len() {
                out.push(format!("{path}: {} items, want {}", a.len(), b.len()));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}[{i}]"), x, y, out);
            }
        }
        _ if got != want => out.push(format!(
            "{path}: got {}, want {}",
            got.to_json(),
            want.to_json()
        )),
        _ => {}
    }
}

fn check(what: &str, got: &ViewIndex, want: &ViewIndex, out: &mut Vec<String>) {
    let mut d = Vec::new();
    diff("", &got.to_value(), &want.to_value(), &mut d);
    out.extend(d.into_iter().map(|l| format!("{what}: {l}")));
}

/// The sample's index read live and from the blob tier at one version.
fn both(f: &Fx) -> Result<(ViewIndex, ViewIndex, u64), String> {
    let s = f.live(&sample());
    let ada = join(&s, "ada", ParticipantKind::Person);
    let live = s
        .view_index(ada.conn)
        .map_err(|e| format!("live: {e}"))?
        .ok_or("live: no view index")?;
    let rev = s.status().unwrap().version;
    let b = f.blob(&sample());
    let bc = join_blob(&b, "ada");
    let blob = b
        .view_index(bc, None)
        .map_err(|e| format!("blob: {e}"))?
        .ok_or("blob: no view index")?;
    Ok((live, blob, rev))
}

// ----- the rows -----

/// The schema gives the expected index from both sources, and the two are the same but for
/// `tier`, `rev` and `version`.
fn row_both_sources() -> Result<(), String> {
    let f = fx("both");
    let (live, blob, rev) = both(&f)?;
    let mut out = Vec::new();
    if live.tier != Tier::Live || live.rev != Some(rev) || live.version.is_some() {
        out.push(format!(
            "live: tier {:?}, rev {:?}, version {:?}; want live, {rev}, none",
            live.tier, live.rev, live.version
        ));
    }
    if blob.tier != Tier::Blob || blob.rev.is_some() || blob.version.is_none() {
        out.push(format!(
            "blob: tier {:?}, rev {:?}, version {:?}; want blob, none, a version",
            blob.tier, blob.rev, blob.version
        ));
    }
    check(
        "live",
        &live,
        &expected(Tier::Live, Some(rev), None),
        &mut out,
    );
    check(
        "blob",
        &blob,
        &expected(Tier::Blob, None, blob.version),
        &mut out,
    );
    let mut same = blob.clone();
    (same.tier, same.rev, same.version) = (live.tier, live.rev, live.version);
    check("live vs blob", &live, &same, &mut out);
    match out.is_empty() {
        true => Ok(()),
        false => Err(out.join("\n")),
    }
}

#[test]
fn the_schema_gives_one_index_from_both_sources() {
    row_both_sources().unwrap();
}

#[test]
fn plant_schema_field_ignored_is_caught() {
    row_both_sources().expect("the row passes as built");
    let e = with_view_mutant(ViewMutant::SchemaFieldIgnored, row_both_sources)
        .expect_err("the row passes with every item shown");
    assert!(
        e.contains("live: units[1].hidden: got false, want true"),
        "{e}"
    );
    assert!(
        !e.lines().any(|l| l.starts_with("blob:")),
        "the blob tier isn't the plant's: {e}"
    );
}

/// Every element's address leads back to it, through the pack and through the session; the
/// index speaks addresses; the presence setters and claims by address set what they name, a
/// unit's focus going to its container through the schema.
fn row_addresses() -> Result<(), String> {
    let f = fx("addr");
    let s = f.live(&sample());
    let ada = join(&s, "ada", ParticipantKind::Person);
    let atlas = join(&s, "atlas", ParticipantKind::Agent);
    let table: &[(&str, Key, &str)] = &[
        (PAGES, page_key(1), "@pg:1"),
        (PAGES, page_key(2), "@pg:2"),
        (PAGES, page_key(3), "@pg:3"),
        (ITEMS, item_key(1, 1), "@pg:1/it:1"),
        (ITEMS, item_key(1, 2), "@pg:1/it:2"),
        (ITEMS, item_key(1, 3), "@pg:1/it:3"),
        (ITEMS, item_key(2, 1), "@pg:2/it:1"),
    ];
    for (root, k, addr) in table {
        let a = s.address_of(root, k);
        if a != *addr {
            return Err(format!(
                "round trip: {root} {k} has the address {a}, want {addr}"
            ));
        }
        match s.resolve_address(ada.conn, addr) {
            Ok((r, back)) if r == *root && back == *k => {}
            got => {
                return Err(format!(
                    "round trip: {addr} resolves to {got:?}, want {root} {k}"
                ))
            }
        }
        let other = if *root == PAGES { ITEMS } else { PAGES };
        if BoardPack.key_of(other, addr).is_some() {
            return Err(format!(
                "round trip: {addr} reads as an element of {other} too"
            ));
        }
    }
    let ix = s.view_index(ada.conn).unwrap().unwrap();
    let mut index_keys: Vec<&str> = ix.containers.iter().map(|c| c.key.as_str()).collect();
    index_keys.extend(ix.units.iter().map(|u| u.key.as_str()));
    let table_keys: Vec<&str> = table.iter().map(|(_, _, a)| *a).collect();
    if index_keys != table_keys {
        return Err(format!(
            "round trip: the index's keys are {index_keys:?}, want {table_keys:?}"
        ));
    }
    for bad in ["@pg:01", "@pg:9", "p1", "@pg:1/it:", "@pg:1/it:9"] {
        match s.resolve_address(ada.conn, bad) {
            Err(SessionError::BadPosition(_)) => {}
            got => return Err(format!("round trip: {bad:?} resolves to {got:?}")),
        }
    }
    // the setters by address
    let got = s
        .set_focus_at(ada.conn, Some("@pg:2"))
        .map_err(|e| e.to_string())?;
    let entry = |pid: &str| {
        s.roster(ada.conn)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .unwrap()
    };
    if got.as_deref() != Some("@pg:2")
        || entry("ada").focus != Some(ElemFocus::new(PAGES, page_key(2)))
    {
        return Err(format!(
            "round trip: focus at @pg:2 gave {got:?}, {:?}",
            entry("ada").focus
        ));
    }
    let sel = vec!["@pg:1/it:1".to_owned(), "@pg:1/it:3".to_owned()];
    let got = s
        .set_selection_at(ada.conn, &sel)
        .map_err(|e| e.to_string())?;
    let keys = entry("ada").selection.map(|x| x.keys);
    if got != sel || keys != Some(vec![item_key(1, 1), item_key(1, 3)]) {
        return Err(format!("round trip: selection gave {got:?}, {keys:?}"));
    }
    let mixed = vec!["@pg:1".to_owned(), "@pg:1/it:1".to_owned()];
    if !matches!(
        s.set_selection_at(ada.conn, &mixed),
        Err(SessionError::BadPosition(_))
    ) {
        return Err("round trip: a selection over two roots is taken".into());
    }
    // a range sets the focus to its item's page, through the schema (no `container_of`)
    let got = s
        .set_range_at(ada.conn, Some(("@pg:1/it:1", "text", 0, 5)))
        .map_err(|e| e.to_string())?;
    if got.as_deref() != Some("@pg:1/it:1")
        || entry("ada").focus != Some(ElemFocus::new(PAGES, page_key(1)))
    {
        return Err(format!(
            "round trip: a range on @pg:1/it:1 gave {got:?}, focus {:?}",
            entry("ada").focus
        ));
    }
    // a claim by address: the agent's focus goes to the page, its selection to the item
    let info = s
        .claim_elems_at(atlas.conn, &["@pg:2/it:1".to_owned()], None, None, false)
        .map_err(|e| e.to_string())?;
    let claimed = match &info.span {
        k2_mpc_base::session::ClaimSpan::Elems { keys, .. } => keys.clone(),
        other => return Err(format!("round trip: the claim's span is {other:?}")),
    };
    if claimed != vec![item_key(2, 1)]
        || entry("atlas").focus != Some(ElemFocus::new(PAGES, page_key(2)))
    {
        return Err(format!(
            "round trip: a claim on @pg:2/it:1 holds {claimed:?}, focus {:?}",
            entry("atlas").focus
        ));
    }
    // the defaults: an address is the key
    let k = item_key(1, 1);
    if Plain.address_of(ITEMS, &k) != "p1-1" || Plain.key_of(ITEMS, "p1-1") != Some(k.clone()) {
        return Err("round trip: the default address isn't the key".into());
    }
    match key_candidates(&Plain, "p1-1") {
        Ok(c) if c == vec![(PAGES, k.clone()), (ITEMS, k.clone())] => {}
        got => return Err(format!("round trip: the default's candidates are {got:?}")),
    }
    Ok(())
}

#[test]
fn addresses_round_trip_and_the_setters_speak_them() {
    row_addresses().unwrap();
}

#[test]
fn plant_address_not_inverse_is_caught() {
    row_addresses().expect("the row passes as built");
    let e = with_view_mutant(ViewMutant::AddressNotInverse, row_addresses)
        .expect_err("the row passes with addresses read as keys");
    assert!(e.starts_with("round trip: @pg:1 resolves to Err"), "{e}");
}

/// A board with one page of many items, each with a long text, and two small pages.
fn big() -> Vec<u8> {
    let mut s = String::from("BOARD|800|600\nP|1|Many|0|-\nP|2|Few|0|-\nP|3|None|0|-\n");
    for i in 1..=60 {
        s.push_str(&format!(
            "I|1|{i}|box|Item {i}|alt {i}|{i},{i},5,5,0,0,0|{}|0|1|-\n",
            "y".repeat(900)
        ));
    }
    s.push_str("I|2|1|box|One|-||-|0|0|-\nI|2|2|box|Two|-||-|0|0|-\n");
    s.into_bytes()
}

/// Every container's pages, read from 0 by `next`, give its units back in order; a page is over
/// its budget only when it holds one unit; the snapshot is everything but the units.
fn row_paging() -> Result<(), String> {
    let f = fx("page");
    let s = f.live(&big());
    let ada = join(&s, "ada", ParticipantKind::Person);
    let ix = s.view_index(ada.conn).unwrap().ok_or("no view index")?;
    if ix.units.len() != 62 {
        return Err(format!("the board has {} units, want 62", ix.units.len()));
    }
    for budget in [VIEW_PAGE_MIN, VIEW_PAGE_BUDGET, 1 << 22] {
        let mut all = Vec::new();
        let mut pages_of_many = 0;
        for c in &ix.containers {
            let mut from = 0;
            loop {
                let p = ix
                    .units_page(&c.key, from, budget)
                    .map_err(|e| format!("{} from {from}: {e}", c.key))?;
                let len = p.to_value().to_json().len();
                if len > budget && p.units.len() > 1 {
                    return Err(format!("{} from {from}: {len} bytes over {budget}", c.key));
                }
                if p.from != from || p.total != c.unit_count as usize {
                    return Err(format!(
                        "{} from {from}: page from {} of {}",
                        c.key, p.from, p.total
                    ));
                }
                if c.key == "@pg:1" {
                    pages_of_many += 1;
                }
                all.extend(p.units);
                match p.next {
                    Some(n) if n > from => from = n,
                    Some(n) => return Err(format!("{}: next {n} after {from}", c.key)),
                    None => break,
                }
            }
        }
        if all != ix.units {
            return Err(format!(
                "budget {budget}: the pages don't give the units back"
            ));
        }
        let want_many = budget < 1 << 22;
        if want_many != (pages_of_many > 1) {
            return Err(format!(
                "budget {budget}: {pages_of_many} pages for 60 units"
            ));
        }
    }
    let snap = ix.snapshot();
    if snap.containers != ix.containers || snap.page != ix.page || snap.layouts != ix.layouts {
        return Err("the snapshot isn't the index's".into());
    }
    let sv = snap.to_value(&VIEW);
    if sv.get("units").is_some() || sv.get("schema") != Some(&VIEW.to_value()) {
        return Err(format!("the snapshot's JSON: {}", sv.to_json()));
    }
    match ix.units_page("@pg:1", 0, VIEW_PAGE_MIN - 1) {
        Err(ViewPageError::BudgetTooSmall { .. }) => {}
        got => return Err(format!("a small budget: {got:?}")),
    }
    match ix.units_page("@pg:9", 0, VIEW_PAGE_MIN) {
        Err(ViewPageError::UnknownContainer(_)) => {}
        got => return Err(format!("an unknown container: {got:?}")),
    }
    match ix.units_page("@pg:2", 3, VIEW_PAGE_MIN) {
        Err(ViewPageError::PastEnd { from: 3, total: 2 }) => {}
        got => return Err(format!("past the end: {got:?}")),
    }
    match ix.units_page("@pg:3", 0, VIEW_PAGE_MIN) {
        Ok(p) if p.units.is_empty() && p.next.is_none() && p.total == 0 => {}
        got => return Err(format!("an empty page: {got:?}")),
    }
    Ok(())
}

#[test]
fn the_paged_form_reassembles() {
    row_paging().unwrap();
}

/// A pack with no view schema has no view index; a schema that doesn't fit the doc is the
/// pack's bug, refused loudly.
#[test]
fn no_schema_no_index_and_a_bad_schema_is_refused() {
    struct NoView;
    impl Pack for NoView {
        fn schema(&self) -> Schema {
            SCHEMA
        }
        fn composite(&self) -> &'static [(&'static str, &'static str)] {
            &[(PAGE_F, PAGES), (ITEM_F, ITEMS)]
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            BoardPack.decode(b)
        }
        fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
            Ok(Vec::new())
        }
    }
    struct BadView;
    impl Pack for BadView {
        fn schema(&self) -> Schema {
            SCHEMA
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            BoardPack.decode(b)
        }
        fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
            Ok(Vec::new())
        }
        // no composite maps: the items' container field can't be read
        fn view_schema(&self) -> Option<ViewSchema> {
            Some(VIEW)
        }
    }
    let f = fx("noview");
    let s = f
        .mgr
        .open(Arc::new(NoView), &key("a.board"), Some(&sample()))
        .unwrap();
    let c = join(&s, "ada", ParticipantKind::Person);
    assert_eq!(s.view_index(c.conn).unwrap(), None);
    assert_eq!(s.view_schema(), None);
    let s = f
        .mgr
        .open(Arc::new(BadView), &key("b.board"), Some(&sample()))
        .unwrap();
    let c = join(&s, "ada", ParticipantKind::Person);
    match s.view_index(c.conn) {
        Err(SessionError::Pack(e)) => assert!(e.to_string().contains("composite"), "{e}"),
        got => panic!("a bad schema gave {got:?}"),
    }
    assert!(VIEW.check(&SCHEMA, BoardPack.composite()).is_ok());
}

// ----- the carrier: what only the file's bytes hold -----

/// One **kid line** of a carrier board, `K|page|item|id|dx|dy|w|h|font`: a unit nested in item
/// `item` of page `page`, at (`dx`, `dy`) from the item's box, whose text uses `font`. The live
/// store never models it: only the file's bytes (the carrier) hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kid {
    page: u32,
    item: u32,
    id: u32,
    dx: i64,
    dy: i64,
    w: i64,
    h: i64,
    font: String,
}

/// The kid lines of a file; the rest is the board's.
fn split_kids(bytes: &[u8]) -> (Vec<u8>, Vec<Kid>) {
    let text = String::from_utf8_lossy(bytes);
    let (mut rest, mut kids) = (String::new(), Vec::new());
    for line in text.lines() {
        let f: Vec<&str> = line.split('|').collect();
        match f[..] {
            ["K", page, item, id, dx, dy, w, h, font] => kids.push(Kid {
                page: page.parse().unwrap_or(0),
                item: item.parse().unwrap_or(0),
                id: id.parse().unwrap_or(0),
                dx: dx.parse().unwrap_or(0),
                dy: dy.parse().unwrap_or(0),
                w: w.parse().unwrap_or(0),
                h: h.parse().unwrap_or(0),
                font: font.to_owned(),
            }),
            _ => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    (rest.into_bytes(), kids)
}

fn kid_line(k: &Kid) -> String {
    format!(
        "K|{}|{}|{}|{}|{}|{}|{}|{}\n",
        k.page, k.item, k.id, k.dx, k.dy, k.w, k.h, k.font
    )
}

/// The board's lines for `content` (pages, items, layouts), as the file writes them.
fn board_text(content: &[(String, Neutral)]) -> Result<String, PackError> {
    let bad = |m: &str| PackError(format!("the board content: {m}"));
    let root = |n: &str| content.iter().find(|(r, _)| r == n).map(|(_, v)| v);
    let list = |n: &str| match root(n) {
        Some(Neutral::List(l)) => Ok(l.clone()),
        _ => Err(bad(n)),
    };
    let map = |n: &str| match root(n) {
        Some(Neutral::Map(m)) => Ok(m.clone()),
        _ => Err(bad(n)),
    };
    let (pf, itf, board) = (map(PAGE_F)?, map(ITEM_F)?, map(BOARD)?);
    let get = |m: &BTreeMap<String, Value>, k: &str| -> Option<String> {
        m.get(k).and_then(|v| v.as_str()).map(str::to_owned)
    };
    let or_dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
    let mut out = format!(
        "BOARD|{}|{}\n",
        get(&board, "w").unwrap_or_default(),
        get(&board, "h").unwrap_or_default()
    );
    for (k, _) in list(PAGES)? {
        let id = k
            .as_str()
            .strip_prefix('p')
            .ok_or_else(|| bad("a page key"))?;
        out.push_str(&format!(
            "P|{id}|{}|{}|{}\n",
            get(&pf, &prop(&k, "title")).unwrap_or_default(),
            get(&pf, &prop(&k, "hidden")).unwrap_or_default(),
            or_dash(get(&pf, &prop(&k, "layout")))
        ));
    }
    for (k, v) in list(ITEMS)? {
        let (page, id) = k
            .as_str()
            .strip_prefix('p')
            .and_then(|r| r.split_once('-'))
            .ok_or_else(|| bad("an item key"))?;
        let f = |n: &str| get(&itf, &prop(&k, n));
        out.push_str(&format!(
            "I|{page}|{id}|{}|{}|{}|{}|{}|{}|{}|{}\n",
            v.as_str().unwrap_or_default(),
            f("name").unwrap_or_default(),
            or_dash(f("alt")),
            f("geom").unwrap_or_default(),
            or_dash(f("text")),
            f("hidden").unwrap_or_default(),
            f("edit").unwrap_or_default(),
            or_dash(f("ph")),
        ));
    }
    let ids: BTreeSet<u32> = board
        .keys()
        .filter_map(|k| k.strip_prefix("layout/")?.split_once('/')?.0.parse().ok())
        .collect();
    for id in ids {
        out.push_str(&format!(
            "L|{id}|{}|{}\n",
            get(&board, &format!("layout/{id}/name")).unwrap_or_default(),
            get(&board, &format!("layout/{id}/master")).unwrap_or_default()
        ));
    }
    Ok(out)
}

/// The board, keeping a carrier: its file may hold kid lines ([`Kid`]), which the live
/// store never models. `encode_from` gives the carrier back for the content it decodes to, and
/// otherwise the content's board lines followed by the carrier's kid lines. The derive hook
/// reads the kids from the carrier: each a nested unit of its item, placed from the item's
/// **live** box (the live store wins), its font in the index's fonts and its page's, and each
/// page's render hash over the page's kid lines and its items' live boxes.
struct CarrierBoard;

impl Pack for CarrierBoard {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        BoardPack.composite()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        BoardPack.decode(&split_kids(bytes).0)
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Err(PackError(
            "a carrier board is written into its carrier".into(),
        ))
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    fn encode_from(
        &self,
        carrier: &[u8],
        content: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        let (rest, kids) = split_kids(carrier);
        let held = BoardPack
            .decode(&rest)
            .map_err(|d| PackError(d.message))?
            .roots;
        let same = held.len() == content.len()
            && held
                .iter()
                .all(|(r, n)| content.iter().any(|(r2, n2)| r == r2 && n == n2));
        if same {
            return Ok(carrier.to_vec());
        }
        let mut out = board_text(content)?;
        for k in &kids {
            out.push_str(&kid_line(k));
        }
        Ok(out.into_bytes())
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VIEW)
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        derive(source, index);
        let ViewSource::Live(_, Some(carrier)) = source else {
            return;
        };
        let Some(kids) = carrier.decoded(|b| Some(split_kids(b).1)) else {
            return;
        };
        let addr = |k: &Kid| format!("@pg:{}/it:{}", k.page, k.item);
        let mut units = Vec::with_capacity(index.units.len() + kids.len());
        for u in std::mem::take(&mut index.units) {
            let mine: Vec<&Kid> = kids.iter().filter(|k| addr(k) == u.key).collect();
            let (key, container, at) = (u.key.clone(), u.container.clone(), u.bbox);
            units.push(u);
            for (z, k) in mine.into_iter().enumerate() {
                units.push(Unit {
                    key: format!("{}/kid:{}", addr(k), k.id),
                    container: container.clone(),
                    parent: Some(key.clone()),
                    live: false,
                    kind: "kid".into(),
                    name: format!("Kid {}", k.id),
                    name_is_generic: Some(true),
                    z: z as u32,
                    // from the item's box as the live store holds it
                    bbox: at.map(|b| Bbox {
                        x: b.x + k.dx,
                        y: b.y + k.dy,
                        w: k.w,
                        h: k.h,
                    }),
                    ..Unit::default()
                });
            }
        }
        index.units = units;
        for k in kids {
            if !index.fonts.iter().any(|f| f.family == k.font) {
                index.fonts.push(FontUse {
                    family: k.font.clone(),
                    script: "latin".into(),
                    from: "explicit".into(),
                    embedded: false,
                });
            }
        }
        for c in &mut index.containers {
            let on: Vec<&Kid> = kids
                .iter()
                .filter(|k| format!("@pg:{}", k.page) == c.key)
                .collect();
            let mut fonts: Vec<u32> = on
                .iter()
                .filter_map(|k| index.fonts.iter().position(|f| f.family == k.font))
                .map(|i| i as u32)
                .collect();
            fonts.sort_unstable();
            fonts.dedup();
            c.fonts = fonts;
            // the page's kid lines and its items' live boxes: what it draws
            let mut lines: String = on.iter().map(|k| kid_line(k)).collect();
            for u in index
                .units
                .iter()
                .filter(|u| u.container == c.key && u.live)
            {
                lines.push_str(&format!("{} {:?}\n", u.key, u.bbox));
            }
            c.render_hash =
                Some(carrier.render_hash(&Digest::of(format!("{}\n{lines}", c.key).as_bytes())));
        }
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        BoardPack.address_of(root, key)
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        BoardPack.key_of(root, address)
    }
}

/// A carrier board: two pages, item 1 of page 1 with two kids (two fonts), item 2 with none.
const CARRIED: &str = "BOARD|1600|900\n\
    P|1|Intro|0|@lay:1\n\
    P|2|Second|0|-\n\
    I|1|1|box|Group 1|-|100,200,300,200,0,0,0|-|0|0|-\n\
    I|1|2|text|Caption|-|400,20,300,100,0,0,0|Hi|0|1|-\n\
    I|2|1|box|Box 1|-|0,0,10,10,0,0,0|-|0|0|-\n\
    L|1|Title Slide|@mst:1\n\
    K|1|1|5|10|20|50|60|Inter\n\
    K|1|1|6|100|20|50|60|Serif Pro\n";

/// A live carrier board on a file (the carrier is the file's bytes), and a person.
struct Carried {
    _dir: TmpDir,
    path: std::path::PathBuf,
    s: Arc<Session>,
    ada: ConnId,
    _mgr: SessionManager,
}

fn carried(tag: &str) -> Carried {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-view-carried-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(100_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c).with_blob_store(blobs);
    let path = ws.join("carried.board");
    std::fs::write(&path, CARRIED).unwrap();
    let s = mgr
        .open_file(Arc::new(CarrierBoard), &key("carried.board"), &path, &ws)
        .unwrap();
    let ada = join(&s, "ada", ParticipantKind::Person).conn;
    Carried {
        _dir: TmpDir(dir),
        path,
        s,
        ada,
        _mgr: mgr,
    }
}

impl Carried {
    fn index(&self) -> Result<ViewIndex, String> {
        self.s
            .view_index(self.ada)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no view index".to_owned())
    }

    /// Set the box of item `item` of page `page` (one change), unwritten.
    fn move_item(&self, page: u32, item: u32, geom: &str) {
        let mut opts = EditOptions::new();
        opts.base = Some(self.s.status().unwrap().version);
        self.s
            .edit_ops(
                self.ada,
                vec![Edit::Map {
                    root: ITEM_F.into(),
                    ops: vec![MapOp::Set {
                        key: prop(&item_key(page, item), "geom"),
                        value: Value::str(geom),
                    }],
                }],
                &opts,
            )
            .unwrap();
    }
}

/// A unit's key, parent and box.
type Placed = (String, Option<String>, Option<(i64, i64, i64, i64)>);

/// Every unit's key, parent and box, in order.
fn placed(ix: &ViewIndex) -> Vec<Placed> {
    ix.units
        .iter()
        .map(|u| {
            (
                u.key.clone(),
                u.parent.clone(),
                u.bbox.map(|b| (b.x, b.y, b.w, b.h)),
            )
        })
        .collect()
}

/// The carrier board's units with item 1 of page 1 at `item`: its kids follow it.
fn want_placed(item: (i64, i64)) -> Vec<Placed> {
    let s = |v: &str| v.to_owned();
    vec![
        (s("@pg:1/it:1"), None, Some((item.0, item.1, 300, 200))),
        (
            s("@pg:1/it:1/kid:5"),
            Some(s("@pg:1/it:1")),
            Some((item.0 + 10, item.1 + 20, 50, 60)),
        ),
        (
            s("@pg:1/it:1/kid:6"),
            Some(s("@pg:1/it:1")),
            Some((item.0 + 100, item.1 + 20, 50, 60)),
        ),
        (s("@pg:1/it:2"), None, Some((400, 20, 300, 100))),
        (s("@pg:2/it:1"), None, Some((0, 0, 10, 10))),
    ]
}

/// The live index reads the carrier for what the store doesn't model: the kids, nested after
/// their item and placed from its live box, their fonts (the index's and each page's), each
/// page's render hash, and the kids counted in their page's units.
fn row_the_derive_reads_the_carrier() -> Result<(), String> {
    let f = carried("reads");
    let ix = f.index()?;
    let mut out = Vec::new();
    if placed(&ix) != want_placed((100, 200)) {
        out.push(format!(
            "units {:?}, want {:?}",
            placed(&ix),
            want_placed((100, 200))
        ));
    }
    let fonts: Vec<&str> = ix.fonts.iter().map(|f| f.family.as_str()).collect();
    if fonts != ["Inter", "Serif Pro"] {
        out.push(format!("fonts {fonts:?}, want [Inter, Serif Pro]"));
    }
    let pages: Vec<(Vec<u32>, bool, u32)> = ix
        .containers
        .iter()
        .map(|c| (c.fonts.clone(), c.render_hash.is_some(), c.unit_count))
        .collect();
    if pages != [(vec![0, 1], true, 4), (vec![], true, 1)] {
        out.push(format!(
            "pages (fonts, render hash, units) {pages:?}, want [([0, 1], true, 4), ([], true, 1)]"
        ));
    }
    if ix.containers.len() == 2 && ix.containers[0].render_hash == ix.containers[1].render_hash {
        out.push("the two pages' render hashes are the same".into());
    }
    match out.is_empty() {
        true => Ok(()),
        false => Err(format!(
            "row_the_derive_reads_the_carrier:\n{}",
            out.join("\n")
        )),
    }
}

#[test]
fn the_derive_reads_the_carrier() {
    row_the_derive_reads_the_carrier().unwrap();
}

#[test]
fn plant_derive_ignores_carrier_is_caught() {
    row_the_derive_reads_the_carrier().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::DeriveIgnoresCarrier,
        row_the_derive_reads_the_carrier,
    )
    .expect_err("the row passes with no carrier handed to the hook");
    assert!(
        e.contains("row_the_derive_reads_the_carrier:\nunits ")
            && e.contains("fonts [], want [Inter, Serif Pro]"),
        "caught, but not where the row says: {e}"
    );
}

/// How many reads the counter row makes of an unchanged doc.
const READS: usize = 50;

/// The carrier is decoded once per carrier, never per read: the first read decodes it once;
/// `READS` more reads of the unchanged doc decode it zero times and give the same index; a
/// change of the doc (a new rev) builds the index again with zero decodes; the kit's write (a
/// new carrier) decodes once, and `READS` reads after it zero times.
fn row_reads_decode_the_carrier_once_per_carrier() -> Result<(), String> {
    const ROW: &str = "row_reads_decode_the_carrier_once_per_carrier";
    let f = carried("decodes");
    take_carrier_decodes();
    let first = f.index()?;
    let n = take_carrier_decodes();
    if n != 1 {
        return Err(format!(
            "{ROW}: the first read: {n} carrier decodes, want 1"
        ));
    }
    for i in 0..READS {
        let ix = f.index()?;
        let n = take_carrier_decodes();
        if n != 0 || ix != first {
            return Err(format!(
                "{ROW}: unchanged read {}: {n} carrier decodes (want 0), same index: {}",
                i + 1,
                ix == first
            ));
        }
    }
    println!("{ROW}: {READS} reads with no change: 0 carrier decodes");
    f.move_item(1, 2, "500,20,300,100,0,0,0");
    let after = f.index()?;
    let n = take_carrier_decodes();
    if n != 0 || after.rev == first.rev {
        return Err(format!(
            "{ROW}: the read after a change: {n} carrier decodes (want 0), rev {:?} (was {:?})",
            after.rev, first.rev
        ));
    }
    if !f.s.flush(f.ada).map_err(|e| e.to_string())?.written {
        return Err(format!("{ROW}: control: the kit wrote the file"));
    }
    let written = std::fs::read_to_string(&f.path).map_err(|e| e.to_string())?;
    if !written.contains("I|1|2|text|Caption|-|500,20,") || !written.contains("K|1|1|6|") {
        return Err(format!(
            "{ROW}: control: the write keeps the kids: {written:?}"
        ));
    }
    f.index()?;
    let n = take_carrier_decodes();
    if n != 1 {
        return Err(format!(
            "{ROW}: the read after the kit's write (a new carrier): {n} carrier decodes, want 1"
        ));
    }
    for _ in 0..READS {
        f.index()?;
    }
    let n = take_carrier_decodes();
    if n != 0 {
        return Err(format!(
            "{ROW}: {READS} reads after the write: {n} carrier decodes, want 0"
        ));
    }
    println!("{ROW}: after the kit's write, 1 decode, then {READS} reads: 0 carrier decodes");
    Ok(())
}

#[test]
fn reads_decode_the_carrier_once_per_carrier() {
    row_reads_decode_the_carrier_once_per_carrier().unwrap();
}

/// A change reaches the derived items: after the item holding the kids moves (unwritten: the
/// carrier still holds the old box), its kids are placed from its new live box; after the
/// kit's write, the same.
fn row_a_change_reaches_the_derived_items() -> Result<(), String> {
    const ROW: &str = "row_a_change_reaches_the_derived_items";
    let f = carried("change");
    let before = f.index()?;
    if placed(&before) != want_placed((100, 200)) {
        return Err(format!("{ROW}: control: before {:?}", placed(&before)));
    }
    f.move_item(1, 1, "1000,500,300,200,0,0,0");
    let after = f.index()?;
    if placed(&after) != want_placed((1000, 500)) {
        return Err(format!(
            "{ROW}: after the move, units {:?}, want {:?}",
            placed(&after),
            want_placed((1000, 500))
        ));
    }
    f.s.flush(f.ada).map_err(|e| e.to_string())?;
    let written = f.index()?;
    if placed(&written) != want_placed((1000, 500)) {
        return Err(format!(
            "{ROW}: after the write, units {:?}",
            placed(&written)
        ));
    }
    Ok(())
}

#[test]
fn a_change_reaches_the_derived_items() {
    row_a_change_reaches_the_derived_items().unwrap();
}

#[test]
fn plant_derive_cache_stale_is_caught() {
    row_a_change_reaches_the_derived_items().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::DeriveCacheStale,
        row_a_change_reaches_the_derived_items,
    )
    .expect_err("the row passes with the index kept across a change");
    assert!(
        e.starts_with("row_a_change_reaches_the_derived_items: after the move, units "),
        "caught, but not where the row says: {e}"
    );
}

/// A keyed watch's change frame carries the new render hash of the page the change re-rendered,
/// and of no other, read through the kept index: after the frame, a read of the index is the
/// same index and decodes nothing.
fn row_a_change_frame_carries_the_new_render_hash() -> Result<(), String> {
    const ROW: &str = "row_a_change_frame_carries_the_new_render_hash";
    let f = carried("watch");
    let mut w =
        f.s.watch_keyed(f.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .map_err(|e| e.to_string())?
            .ok_or("control: no watch")?;
    f.s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    let before = f.index()?;
    take_carrier_decodes();
    f.move_item(1, 1, "1000,500,300,200,0,0,0");
    let frames = w.try_frames();
    let changes: Vec<_> = frames
        .iter()
        .filter_map(|fr| match fr {
            WatchFrame::Change(c) => Some(c),
            _ => None,
        })
        .collect();
    if changes.len() != 1 {
        return Err(format!(
            "{ROW}: control: {} change frames, want 1",
            changes.len()
        ));
    }
    let after = f.index()?;
    let n = take_carrier_decodes();
    let hex = |ix: &ViewIndex, i: usize| ix.containers[i].render_hash.map(|h| h.to_hex());
    let want = vec![(
        "@pg:1".to_owned(),
        hex(&after, 0).ok_or("control: no hash")?,
    )];
    if hex(&before, 0) == hex(&after, 0) || hex(&before, 1) != hex(&after, 1) {
        return Err(format!(
            "{ROW}: control: page 1's hash must change, page 2's not"
        ));
    }
    if changes[0].extras.render_hashes != want {
        return Err(format!(
            "{ROW}: the frame's render hashes {:?}, want {want:?}",
            changes[0].extras.render_hashes
        ));
    }
    if n != 0 {
        return Err(format!(
            "{ROW}: the frame and the read after it: {n} carrier decodes, want 0"
        ));
    }
    Ok(())
}

#[test]
fn a_change_frame_carries_the_new_render_hash() {
    row_a_change_frame_carries_the_new_render_hash().unwrap();
}

#[test]
fn plant_frame_misses_render_hash_is_caught() {
    row_a_change_frame_carries_the_new_render_hash().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::FrameMissesRenderHash,
        row_a_change_frame_carries_the_new_render_hash,
    )
    .expect_err("the row passes with no render hash on the frame");
    assert!(
        e.starts_with(
            "row_a_change_frame_carries_the_new_render_hash: the frame's render hashes []"
        ),
        "caught, but not where the row says: {e}"
    );
}

// ----- a container's layout is a layout reference -----

/// What `f` unwound with, or `None` when it returns.
fn unwound_with<R>(f: impl FnOnce() -> R) -> Option<String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(_) => None,
        Err(p) => Some(
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default(),
        ),
    }
}

/// The sample with page 3's layout `bad` (`-`: page 3 has none), and with no layout lines when
/// `layouts` is false.
fn layout_sample(bad: &str, layouts: bool) -> Vec<u8> {
    let s = String::from_utf8(sample()).unwrap();
    let s = s.replace("P|3|Empty|0|@lay:2", &format!("P|3|Empty|0|{bad}"));
    let s = match layouts {
        true => s,
        false => s
            .lines()
            .filter(|l| !l.starts_with("L|"))
            .map(|l| format!("{l}\n"))
            .collect(),
    };
    s.into_bytes()
}

/// The live and the blob index of `bytes`, each in a fresh doc.
fn both_of(tag: &str, bytes: &[u8]) -> (Result<ViewIndex, String>, Result<ViewIndex, String>) {
    let f = fx(tag);
    let live = {
        let s = f.live(bytes);
        let ada = join(&s, "ada", ParticipantKind::Person);
        s.view_index(ada.conn)
            .map_err(|e| format!("live: {e}"))
            .and_then(|i| i.ok_or_else(|| "live: no view index".to_owned()))
    };
    let blob = {
        let b = f.blob(bytes);
        let bc = join_blob(&b, "ada");
        b.view_index(bc, None)
            .map_err(|e| format!("blob: {e}"))
            .and_then(|i| i.ok_or_else(|| "blob: no view index".to_owned()))
    };
    (live, blob)
}

/// A page whose layout field holds `slideLayout2.xml`, where the index holds layouts, fails a
/// test build loudly on both tiers, naming the page and the value; so does one that isn't an
/// address at all (`@layout`), with no layouts.
fn row_a_bad_layout_fails_a_test_build() -> Result<(), String> {
    const ROW: &str = "row_a_bad_layout_fails_a_test_build";
    let cases = [
        ("slideLayout2.xml", true, "no_such_layout"),
        ("@layout", false, "not_an_address"),
    ];
    for (bad, layouts, why) in cases {
        let want = format!("@pg:3: the pack's layout \"{bad}\" isn't a layout reference ({why})");
        for tier in ["live", "blob"] {
            let got = unwound_with(|| {
                let (live, blob) = both_of("bad-layout", &layout_sample(bad, layouts));
                match tier {
                    "live" => live.map(|_| ()),
                    _ => blob.map(|_| ()),
                }
            });
            match got {
                None => {
                    return Err(format!(
                        "{ROW}: {tier}: the layout {bad:?} built and nothing refused it"
                    ))
                }
                Some(e) if !e.contains(&want) => {
                    return Err(format!(
                        "{ROW}: {tier}: refused, but not as the kit says: {e}\nwant {want}"
                    ))
                }
                Some(_) => {}
            }
        }
    }
    // a change too: a good layout made bad on a live doc that kept its index
    let f = fx("bad-layout-change");
    let s = f.live(&sample());
    let ada = join(&s, "ada", ParticipantKind::Person);
    s.view_index(ada.conn).unwrap().unwrap();
    let got = unwound_with(|| {
        s.edit_ops(
            ada.conn,
            vec![Edit::Map {
                root: PAGE_F.into(),
                ops: vec![MapOp::Set {
                    key: prop(&page_key(3), "layout"),
                    value: Value::str("slideLayout2.xml"),
                }],
            }],
            &EditOptions::new(),
        )
        .unwrap();
        s.view_index(ada.conn)
    });
    let want =
        "@pg:3: the pack's layout \"slideLayout2.xml\" isn't a layout reference (no_such_layout)";
    match got {
        None => Err(format!(
            "{ROW}: a change: the layout built and nothing refused it"
        )),
        Some(e) if !e.contains(want) => Err(format!(
            "{ROW}: a change: refused, but not as the kit says: {e}"
        )),
        Some(_) => Ok(()),
    }
}

#[test]
fn a_bad_layout_fails_a_test_build() {
    row_a_bad_layout_fails_a_test_build().unwrap();
}

#[test]
fn plant_layout_unchecked_is_caught() {
    row_a_bad_layout_fails_a_test_build().expect("the row passes as built");
    let e = with_view_mutant(
        ViewMutant::LayoutUnchecked,
        row_a_bad_layout_fails_a_test_build,
    )
    .expect_err("the row passes with no layout check");
    assert!(
        e.starts_with(
            "row_a_bad_layout_fails_a_test_build: live: the layout \"slideLayout2.xml\" built and \
             nothing refused it"
        ),
        "{e}"
    );
}

/// As a release build reads it (the test build's failure off): a bad layout is left out of
/// its page and recorded, on both tiers, the other pages keep theirs; with no layouts in the
/// index, a bare key is a layout reference and a malformed address isn't; the sample's own
/// layouts are carried and nothing is refused.
#[test]
fn a_bad_layout_is_not_carried_and_is_recorded_where_unchecked() {
    let refused = |container: &str, value: &str, why: LayoutRefused| ViewRefusal::Layout {
        container: container.into(),
        value: value.into(),
        why,
    };
    let layout_of = |ix: &ViewIndex, c: &str| {
        ix.containers
            .iter()
            .find(|x| x.key == c)
            .unwrap_or_else(|| panic!("no container {c}"))
            .layout
            .clone()
    };
    // the sample: every layout carried, nothing refused
    let (live, blob) = both_of("layout-good", &sample());
    for (tier, ix) in [("live", live.unwrap()), ("blob", blob.unwrap())] {
        assert!(ix.refusals.is_empty(), "{tier}: {:?}", ix.refusals);
        assert_eq!(layout_of(&ix, "@pg:1").as_deref(), Some("@lay:1"), "{tier}");
        assert_eq!(layout_of(&ix, "@pg:3").as_deref(), Some("@lay:2"), "{tier}");
    }
    let cases: [(&str, bool, Option<LayoutRefused>); 4] = [
        ("slideLayout2.xml", true, Some(LayoutRefused::NoSuchLayout)),
        ("@lay:9", true, Some(LayoutRefused::NoSuchLayout)),
        ("slideLayout2.xml", false, None),
        ("@layout", false, Some(LayoutRefused::NotAnAddress)),
    ];
    for (bad, layouts, why) in cases {
        let (live, blob) = with_view_mutant(ViewMutant::LayoutUnchecked, || {
            both_of("layout-unchecked", &layout_sample(bad, layouts))
        });
        for (tier, ix) in [("live", live.unwrap()), ("blob", blob.unwrap())] {
            let what = format!("{tier}, {bad:?}, layouts {layouts}");
            assert_eq!(
                layout_of(&ix, "@pg:1").as_deref(),
                Some("@lay:1"),
                "{what}: page 1 keeps its own"
            );
            match why {
                Some(w) => {
                    assert_eq!(layout_of(&ix, "@pg:3"), None, "{what}: not carried");
                    assert_eq!(ix.refusals, vec![refused("@pg:3", bad, w)], "{what}");
                }
                None => {
                    assert_eq!(
                        layout_of(&ix, "@pg:3").as_deref(),
                        Some(bad),
                        "{what}: carried"
                    );
                    assert!(ix.refusals.is_empty(), "{what}: {:?}", ix.refusals);
                }
            }
        }
    }
}
