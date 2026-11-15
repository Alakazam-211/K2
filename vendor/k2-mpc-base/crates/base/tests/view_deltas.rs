//! View deltas in the keyed watch's change frames (`client/WIRE.md`, "View deltas";
//! `src/view_delta.rs`): what each change did to the view, per container and unit, worked out by
//! the kit from the change's ops and the pack's view schema, so a watcher keeps its view without
//! running pack code.
//!
//! - **The contract's goldens:** a deck with the contract's view schema replays the contract's
//!   scripted agent turn (revs 41 to 46) and its cases C (a hidden slide), E (a delete and its
//!   undo) and F (a hidden unit); each frame's `units`, `containers_changed` and `containers`
//!   are the golden's (`tests/golden/view_turn.json`, hand-made from the contract).
//! - **An oracle over random traces:** seeded traces of every list and field op on both kinds
//!   of container naming (by an id field, and by key), undo and redo; for each change the view
//!   index before and after it ([`Session::view_index`]) says what moved, and the frame's deltas
//!   must say it: the containers' order replays from the positions, a unit that came, went,
//!   moved, turned, flipped, changed text, visibility or container has its delta, its geometry
//!   after is the unit's, and no delta names a unit the view never had.
//! - **The client's vectors:** the same traces' frames, with the view index at checkpoints
//!   (`client/test/vectors/view_deltas.json`): the JS client's view, kept from the snapshot's
//!   view and the deltas, must equal each (`client/test/view-deltas.test.ts`).
//! - **No schema, no deltas:** a pack without a view schema gets none of the three fields.
//!
//! Planted fault caught by name: `WatchMutant::DeltaGeomStale` (by the golden row and the
//! oracle).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_watch_mutant, WatchMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, ListOp, MapOp};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, Host, KeyedWatch, Pack, PackContent,
    PackError, Participant, ParticipantKind, Rights, Session, SessionManager, Stay, UndoRequest,
    WatchFrame, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_delta::{UnitDelta, UnitOp, ViewDeltas};
use k2_mpc_base::view_index::{
    unit_value, ContainerFields, Derived, FieldClass, GeomPart, Geometry, PageSpec, Src, Unit,
    UnitFields, ViewIndex, ViewSchema,
};
use k2_mpc_base::{Edit, Key, Value};

// ---------- the deck: slides and shapes, as the contract's view schema reads a deck ----------

const SLIDES: &str = "slides";
const SLIDE_F: &str = "slide_f";
const SHAPES: &str = "shapes";
const SHAPE_F: &str = "shape_f";
const DECK: &str = "deck";
const SEP: char = '\u{1f}';

const SCHEMA: Schema = Schema {
    name: "deck-deltas",
    roots: &[
        (SLIDES, Kind::MovableList),
        (SLIDE_F, Kind::Map),
        (SHAPES, Kind::MovableList),
        (SHAPE_F, Kind::Map),
        (DECK, Kind::Map),
    ],
};

const EDIT_WORDS: &[(&str, &str)] = &[("1", "plain"), ("fmt", "formatted"), ("0", "never")];

const PARTS: &[GeomPart] = &[
    GeomPart::X,
    GeomPart::Y,
    GeomPart::W,
    GeomPart::H,
    GeomPart::Rot,
    GeomPart::FlipH,
    GeomPart::FlipV,
];

const FIELD_CLASS: &[(&str, FieldClass)] = &[
    ("geom", FieldClass::Geometry),
    ("text", FieldClass::Text),
    ("notes", FieldClass::Text),
    ("hidden", FieldClass::Visibility),
    ("fill", FieldClass::Style),
    ("slide", FieldClass::Meta),
    ("fid", FieldClass::Meta),
    ("name", FieldClass::Meta),
    ("descr", FieldClass::Meta),
    ("ph", FieldClass::Meta),
    ("edit", FieldClass::Meta),
    ("sid", FieldClass::Meta),
    ("layout", FieldClass::Meta),
];

/// The contract's deck schema: a shape's `slide` field holds its slide's `sid`; `geom` is
/// `x,y,w,h,rot,fh,fv`; the title is the pack's (derived), so the kit leaves it empty.
const BY_ID: ViewSchema = ViewSchema {
    containers: SLIDES,
    units: SHAPES,
    unit_container_field: "slide",
    container_id_field: Some("sid"),
    native_unit: "emu",
    page: Some(PageSpec {
        root: DECK,
        w: "cx",
        h: "cy",
    }),
    container_fields: ContainerFields {
        title: Src::Derived("slide_title"),
        hidden: Src::Field("hidden"),
        layout: Src::Field("layout"),
        parent: Src::None,
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        name: Src::Field("name"),
        alt: Src::Field("descr"),
        text: Src::Field("text"),
        hidden: Src::Field("hidden"),
        editable_text: Src::Values {
            field: "edit",
            values: EDIT_WORDS,
        },
        placeholder: Src::Field("ph"),
    },
    geometry: Some(Geometry {
        field: "geom",
        parts: PARTS,
        rot_unit: "60000th-degree",
    }),
    field_class: FIELD_CLASS,
    derived: Derived {
        containers: &["title"],
        units: &[],
    },
    ..ViewSchema::NONE
};

/// The other naming: a shape's `slide` field holds its slide's key; a slide's title is its own
/// field and its visibility a `show` field read through words (no class for it: the kit takes
/// the schema's hidden field as the visibility); a box with no rotation or flips; no page.
const BY_KEY: ViewSchema = ViewSchema {
    container_id_field: None,
    page: None,
    container_fields: ContainerFields {
        title: Src::Field("title"),
        hidden: Src::Values {
            field: "show",
            values: &[("0", "true"), ("1", "false")],
        },
        layout: Src::Field("layout"),
        parent: Src::None,
    },
    geometry: Some(Geometry {
        field: "geom",
        parts: &[GeomPart::X, GeomPart::Y, GeomPart::W, GeomPart::H],
        rot_unit: "",
    }),
    derived: Derived::NONE,
    ..BY_ID
};

/// A deck, opened without a file from lines: `S <key> <field> <value>` sets a slide's field
/// (the slide exists once named), `U <key> <kind>` a shape, `u <key> <field> <value>` its
/// field, `D <key> <value>` the deck's. A slide `s256` is `@sld:256` on the wire, a shape
/// `s256-7` `@sld:256/sp:7`. `view`: its view schema, or none.
struct Deck {
    view: Option<ViewSchema>,
}

fn addr(root: &str, key: &str) -> String {
    match root {
        SLIDES => match key.strip_prefix('s') {
            Some(n) => format!("@sld:{n}"),
            None => key.to_owned(),
        },
        SHAPES => match key.strip_prefix('s').and_then(|r| r.split_once('-')) {
            Some((a, b)) => format!("@sld:{a}/sp:{b}"),
            None => key.to_owned(),
        },
        _ => key.to_owned(),
    }
}

impl Pack for Deck {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(SLIDE_F, SLIDES), (SHAPE_F, SHAPES)]
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        self.view
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        addr(root, key.as_str())
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        let rest = address.strip_prefix("@sld:")?;
        match (root, rest.split_once("/sp:")) {
            (SLIDES, None) => Key::new(format!("s{rest}")).ok(),
            (SHAPES, Some((a, b))) => Key::new(format!("s{a}-{b}")).ok(),
            _ => None,
        }
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut slides, mut sf, mut shapes, mut uf, mut deck) = (
            Vec::<(Key, Value)>::new(),
            BTreeMap::new(),
            Vec::<(Key, Value)>::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let bad = |line: &str| Declined {
            code: "malformed",
            message: format!("bad line {line:?}"),
        };
        for line in std::str::from_utf8(b).map_err(|_| bad("utf-8"))?.lines() {
            let p: Vec<&str> = line.splitn(4, ' ').collect();
            let val = |s: &str| Value::str(s.replace("\\n", "\n"));
            match p.as_slice() {
                ["S", key, field, value] => {
                    let kk = k(key);
                    if !slides.iter().any(|(x, _)| *x == kk) {
                        slides.push((kk, Value::Null));
                    }
                    sf.insert(format!("{key}{SEP}{field}"), val(value));
                }
                ["U", key, kind] => shapes.push((k(key), Value::str(*kind))),
                ["u", key, field, value] => {
                    uf.insert(format!("{key}{SEP}{field}"), val(value));
                }
                ["D", key, value] => {
                    deck.insert((*key).to_owned(), Value::str(*value));
                }
                _ => return Err(bad(line)),
            }
        }
        Ok(PackContent {
            roots: vec![
                (SLIDES.into(), Neutral::List(slides)),
                (SLIDE_F.into(), Neutral::Map(sf)),
                (SHAPES.into(), Neutral::List(shapes)),
                (SHAPE_F.into(), Neutral::Map(uf)),
                (DECK.into(), Neutral::Map(deck)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

/// The contract's deck at rev 40 (its slides' ids and layouts, its shapes' kinds, names,
/// placeholders, boxes and texts; an empty `geom` takes the layout's box).
const REV40: &str = "\
D cx 12192000
D cy 6858000
S s256 sid 256
S s256 layout @layout:2147483649
S s257 sid 257
S s257 layout @layout:2147483651
S s258 sid 258
S s258 layout @layout:2147483651
S s259 sid 259
S s259 layout @layout:2147483650
U s256-2 placeholder
u s256-2 slide 256
u s256-2 fid 2
u s256-2 ph ctrTitle:
u s256-2 name Title 1
u s256-2 text Quarterly review
u s256-2 edit 1
U s256-3 placeholder
u s256-3 slide 256
u s256-3 fid 3
u s256-3 ph subTitle:1
u s256-3 name Subtitle 2
u s256-3 text Team update
u s256-3 edit 1
U s257-2 placeholder
u s257-2 slide 257
u s257-2 fid 2
u s257-2 ph title:
u s257-2 name Title 1
u s257-2 text Highlights
u s257-2 edit 1
U s257-4 group
u s257-4 slide 257
u s257-4 fid 4
u s257-4 name Group 4
u s257-4 geom 1000000,1500000,4000000,2000000,5400000,0,0
u s257-4 edit 0
U s257-7 shape
u s257-7 slide 257
u s257-7 fid 7
u s257-7 name Right Arrow 7
u s257-7 geom 7000000,2000000,3000000,1000000,1800000,1,0
u s257-7 edit 1
U s258-2 placeholder
u s258-2 slide 258
u s258-2 fid 2
u s258-2 ph title:
u s258-2 name Title 1
u s258-2 text Regional numbers
u s258-2 edit 1
U s258-4 table
u s258-4 slide 258
u s258-4 fid 4
u s258-4 name Table 3
u s258-4 geom 838200,1825625,10515600,3000000,0,0,0
u s258-4 text Region Q3 Q4
u s258-4 edit 0
U s259-2 placeholder
u s259-2 slide 259
u s259-2 fid 2
u s259-2 ph title:
u s259-2 name Title 1
u s259-2 text Next steps
u s259-2 edit 1
U s259-3 placeholder
u s259-3 slide 259
u s259-3 fid 3
u s259-3 ph body:1
u s259-3 name Content Placeholder 2
u s259-3 text Hire two engineers\\nShip the v2 beta
u s259-3 edit 1
U s259-4 placeholder
u s259-4 slide 259
u s259-4 fid 4
u s259-4 ph sldNum:12
u s259-4 name Slide Number Placeholder 3
u s259-4 text 4
u s259-4 edit 0
";

// ---------- the host and the fixture ----------

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        match who.pid.as_str() {
            "ada" => Rights::read_only(),
            _ => Rights::full(),
        }
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(10, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-view-deltas-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c);
    Fx { dir, mgr }
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn open(f: &Fx, view: Option<ViewSchema>, name: &str, bytes: &str) -> Arc<Session> {
    let key = DocKey {
        space: "ws".into(),
        path: name.into(),
    };
    f.mgr
        .open(Arc::new(Deck { view }), &key, Some(bytes.as_bytes()))
        .unwrap()
}

fn join(s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
    let p = Participant::new(pid, pid, "#6d5bd0", kind, "cli");
    s.join(p, Stay::Held).unwrap().conn
}

fn version(s: &Session) -> u64 {
    s.status().unwrap().version
}

/// A change, guarded by the version now; `true` when it made one.
fn raw(s: &Session, c: ConnId, edits: Vec<Edit>) -> bool {
    let v = version(s);
    s.edit_ops(c, edits, &EditOptions::new().if_version(v))
        .is_ok_and(|r| r.version > v)
}

/// A write to field `field` of element `key` (of the composite map `_root`, named for reading).
fn set(_root: &str, key: &str, field: &str, value: Option<&str>) -> MapOp {
    let key = format!("{key}{SEP}{field}");
    match value {
        Some(v) => MapOp::Set {
            key,
            value: Value::str(v),
        },
        None => MapOp::Delete { key },
    }
}

fn map(root: &str, ops: Vec<MapOp>) -> Edit {
    Edit::Map {
        root: root.into(),
        ops,
    }
}

fn list(root: &str, ops: Vec<ListOp>) -> Edit {
    Edit::List {
        root: root.into(),
        ops,
    }
}

fn listed() -> WatchOptions {
    WatchOptions::new(&[KEYED_WATCH_FEATURE])
}

fn change_frames(frames: &[WatchFrame]) -> Vec<&k2_mpc_base::session::WatchChange> {
    frames
        .iter()
        .filter_map(|f| match f {
            WatchFrame::Change(c) => Some(&**c),
            _ => None,
        })
        .collect()
}

/// A frame's view fields as JSON: `units`, `containers_changed` and `containers`.
fn view_fields(f: &WatchFrame) -> Json {
    let Json::Obj(m) = f.to_value() else {
        panic!("a frame is an object");
    };
    let get = |n: &str| {
        m.iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
            .unwrap_or(Json::Null)
    };
    Json::obj([
        ("units", get("units")),
        ("containers_changed", get("containers_changed")),
        ("containers", get("containers")),
    ])
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../client/test/vectors")
}

/// Field `name` of a JSON object.
fn field<'a>(v: &'a Json, name: &str) -> &'a Json {
    match v {
        Json::Obj(m) => m
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
            .unwrap_or(&Json::Null),
        _ => &Json::Null,
    }
}

// ---------- the contract's goldens ----------

/// The contract's turn and cases C, E and F, on the deck at rev 40: each step's frame, by name.
fn turn_frames() -> Vec<(String, WatchFrame)> {
    let f = fx("turn");
    let s = open(&f, Some(BY_ID), "turn.deck", REV40);
    let bot = join(&s, "atlas", ParticipantKind::Agent);
    let own = join(&s, "own", ParticipantKind::Person);
    let ada = join(&s, "ada", ParticipantKind::Person);
    let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    s.watch_snapshot(&mut w).unwrap();
    let mut out: Vec<(String, WatchFrame)> = Vec::new();
    let mut step = |name: &str, ok: bool, w: &mut KeyedWatch| {
        assert!(ok, "control: step {name} made a change");
        let fs = w.try_frames();
        assert_eq!(fs.len(), 1, "one frame for {name}: {fs:?}");
        out.push((name.to_owned(), fs.into_iter().next().unwrap()));
    };
    let text = |key: &str, t: &str| vec![map(SHAPE_F, vec![set(SHAPE_F, key, "text", Some(t))])];
    step(
        "shape text",
        raw(&s, bot, text("s256-2", "Q4 review")),
        &mut w,
    );
    let mv = vec![map(
        SHAPE_F,
        vec![set(
            SHAPE_F,
            "s257-7",
            "geom",
            Some("7500000,4000000,3000000,1000000,1800000,1,0"),
        )],
    )];
    step("shape move", raw(&s, bot, mv), &mut w);
    let mut fields = Vec::new();
    for (key, fid, ph, name) in [
        ("s260-2", "2", "title:", "Title 1"),
        ("s260-3", "3", "body:1", "Content Placeholder 2"),
    ] {
        for (fl, v) in [
            ("slide", "260"),
            ("fid", fid),
            ("ph", ph),
            ("name", name),
            ("edit", "1"),
        ] {
            fields.push(set(SHAPE_F, key, fl, Some(v)));
        }
    }
    let add = vec![
        list(
            SLIDES,
            vec![ListOp::Insert {
                key: k("s260"),
                at: Anchor::End,
                value: Value::Null,
            }],
        ),
        map(
            SLIDE_F,
            vec![
                set(SLIDE_F, "s260", "sid", Some("260")),
                set(SLIDE_F, "s260", "layout", Some("@layout:2147483650")),
            ],
        ),
        list(
            SHAPES,
            ["s260-2", "s260-3"]
                .iter()
                .map(|x| ListOp::Insert {
                    key: k(x),
                    at: Anchor::End,
                    value: Value::str("placeholder"),
                })
                .collect(),
        ),
        map(SHAPE_F, fields),
    ];
    step("slide add", raw(&s, bot, add), &mut w);
    step("shape text", raw(&s, bot, text("s260-2", "Risks")), &mut w);
    let mv = vec![list(
        SLIDES,
        vec![ListOp::Move {
            key: k("s259"),
            to: Anchor::Before(k("s258")),
        }],
    )];
    step("slide move", raw(&s, bot, mv), &mut w);
    step(
        "shape text",
        raw(&s, bot, text("s260-3", "Supplier delays\nHiring pace")),
        &mut w,
    );
    // case C: a hidden slide
    let hide = vec![map(
        SLIDE_F,
        vec![set(SLIDE_F, "s258", "hidden", Some("1"))],
    )];
    step("C slide hide", raw(&s, own, hide), &mut w);
    // case E: a delete, then its undo
    let del = vec![list(SHAPES, vec![ListOp::Delete { key: k("s260-3") }])];
    step("E shape delete", raw(&s, bot, del), &mut w);
    let undone = s.undo_with(bot, &UndoRequest::steps(1)).is_ok();
    step("E undo", undone, &mut w);
    // case F: a unit hidden (the contract's outside save; here a person's change)
    let hide = vec![map(
        SHAPE_F,
        vec![set(SHAPE_F, "s257-7", "hidden", Some("1"))],
    )];
    step("F hidden", raw(&s, own, hide), &mut w);
    out
}

fn expected_turn() -> Vec<Json> {
    let text = fs::read_to_string(golden_dir().join("view_turn.json")).unwrap();
    let v = parse_json(&text);
    let mut out: Vec<Json> = Vec::new();
    let pick = |fr: &Json| {
        Json::obj([
            ("units", field(fr, "units").clone()),
            (
                "containers_changed",
                field(fr, "containers_changed").clone(),
            ),
            ("containers", field(fr, "containers").clone()),
        ])
    };
    if let Json::List(t) = field(&v, "turn") {
        out.extend(t.iter().map(pick));
    }
    for c in ["C", "E", "F"] {
        if let Json::List(t) = field(field(&v, "cases"), c) {
            out.extend(t.iter().map(pick));
        }
    }
    out
}

fn row_turn_golden() -> Result<(), String> {
    let got = turn_frames();
    let want = expected_turn();
    if got.len() != want.len() || want.len() != 10 {
        return Err(format!(
            "row_turn_golden: {} frames, the golden has {}",
            got.len(),
            want.len()
        ));
    }
    for ((name, f), want) in got.iter().zip(&want) {
        let g = view_fields(f);
        if g.to_json() != want.to_json() {
            return Err(format!(
                "row_turn_golden: step {name}\n--- got:  {}\n--- want: {}",
                g.to_json(),
                want.to_json()
            ));
        }
    }
    Ok(())
}

#[test]
fn the_contract_turn_gives_the_contract_deltas() {
    row_turn_golden().unwrap();
}

#[test]
fn plant_delta_geom_stale_is_caught_by_the_golden() {
    let e = with_watch_mutant(WatchMutant::DeltaGeomStale, row_turn_golden)
        .expect_err("plant DeltaGeomStale must fail the golden row");
    assert!(
        e.contains("step shape move"),
        "caught at the move, not elsewhere: {e}"
    );
}

// ---------- the oracle over random traces ----------

/// xorshift64*.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> Option<&'a T> {
        (!v.is_empty()).then(|| &v[self.below(v.len())])
    }
}

/// A small deck for the traces: five slides, four shapes each.
fn small_deck(by_key: bool) -> String {
    let mut out = String::from("D cx 9144000\nD cy 6858000\n");
    for n in 1..=5 {
        out.push_str(&format!(
            "S s{n} sid {n}\nS s{n} layout @layout:{}\n",
            n % 2
        ));
        if by_key {
            out.push_str(&format!("S s{n} title Slide {n}\nS s{n} show 1\n"));
        }
    }
    for n in 1..=5 {
        for m in 1..=4 {
            let key = format!("s{n}-{m}");
            let slide = if by_key {
                format!("s{n}")
            } else {
                n.to_string()
            };
            out.push_str(&format!("U {key} shape\nu {key} slide {slide}\n"));
            out.push_str(&format!(
                "u {key} name Shape {m}\nu {key} text text {n}.{m}\n"
            ));
            out.push_str(&format!(
                "u {key} geom {m}00,{n}00,50,40,0,0,0\nu {key} edit 1\n"
            ));
        }
    }
    out
}

struct Doc<'a> {
    s: &'a Session,
    by_key: bool,
}

impl Doc<'_> {
    fn state(&self) -> k2_mpc_base::elem::KeyedState {
        self.s.keyed_state_for_tests().unwrap()
    }
    /// A list root's live and deleted keys now.
    fn keys(&self, root: &str) -> (Vec<Key>, Vec<Key>) {
        for (r, x) in self.state().roots {
            if let (true, k2_mpc_base::elem::KeyedRoot::List { live, hidden, .. }) = (r == root, x)
            {
                return (
                    live.into_iter().map(|(k, _)| k).collect(),
                    hidden.into_keys().collect(),
                );
            }
        }
        (Vec::new(), Vec::new())
    }
    /// What a shape's `slide` field holds for slide `key`.
    fn slide_ref(&self, key: &Key) -> String {
        if self.by_key {
            key.as_str().to_owned()
        } else {
            key.as_str().trim_start_matches('s').to_owned()
        }
    }
}

const WORDS: [&str; 8] = [
    "a",
    "b c",
    "\u{e9}",
    "\u{65e5}\u{672c}",
    "\u{1F389}",
    "x\ny",
    "",
    " q ",
];

fn geom(rng: &mut Rng, by_key: bool) -> String {
    match rng.below(12) {
        0 => String::new(),
        1 => "not a box".into(),
        _ => {
            let n = |rng: &mut Rng| (rng.below(5) * 100).to_string();
            let mut p = vec![n(rng), n(rng), n(rng), n(rng)];
            if !by_key {
                p.push((rng.below(3) * 5_400_000).to_string());
                p.push(rng.below(2).to_string());
                p.push(rng.below(2).to_string());
            }
            p.join(",")
        }
    }
}

/// One random change by `who`; `true` when it made one.
fn step(d: &Doc<'_>, rng: &mut Rng, who: ConnId, minted: &mut u32) -> bool {
    let s = d.s;
    let (slides, gone_slides) = d.keys(SLIDES);
    let (shapes, gone_shapes) = d.keys(SHAPES);
    let r = rng.below(100);
    let edits: Vec<Edit> = match r {
        // a shape's field: geometry, text, name, visibility, placeholder, edit, style, alt
        0..=34 => {
            let Some(u) = rng.pick(&shapes).cloned() else {
                return false;
            };
            let (fl, v) = match rng.below(9) {
                0..=2 => ("geom", Some(geom(rng, d.by_key))),
                3 => ("text", Some(WORDS[rng.below(8)].to_owned())),
                4 => ("name", Some(WORDS[rng.below(8)].to_owned())),
                5 => (
                    "hidden",
                    [Some("1"), Some("0"), None][rng.below(3)].map(str::to_owned),
                ),
                6 => (
                    "ph",
                    [Some("title:"), Some("body:1"), Some(":3"), None][rng.below(4)]
                        .map(str::to_owned),
                ),
                7 => (
                    "edit",
                    [Some("1"), Some("fmt"), Some("0"), Some("x")][rng.below(4)].map(str::to_owned),
                ),
                _ => (
                    ["fill", "descr"][rng.below(2)],
                    Some(WORDS[rng.below(8)].to_owned()),
                ),
            };
            vec![map(
                SHAPE_F,
                vec![set(SHAPE_F, u.as_str(), fl, v.as_deref())],
            )]
        }
        // a shape moves to another slide (or to one that isn't there)
        35..=40 => {
            let Some(u) = rng.pick(&shapes).cloned() else {
                return false;
            };
            let to = match rng.pick(&slides) {
                Some(c) if rng.below(6) != 0 => d.slide_ref(c),
                _ => "999".into(),
            };
            vec![map(
                SHAPE_F,
                vec![set(SHAPE_F, u.as_str(), "slide", Some(&to))],
            )]
        }
        // shapes: insert (with fields), delete, restore, move, a new kind
        41..=48 => {
            let Some(c) = rng.pick(&slides).cloned() else {
                return false;
            };
            *minted += 1;
            let key = format!("s{}-{}", c.as_str().trim_start_matches('s'), 100 + *minted);
            let at = match rng.pick(&shapes) {
                Some(a) if rng.below(3) != 0 => Anchor::After(a.clone()),
                _ => Anchor::Start,
            };
            vec![
                list(
                    SHAPES,
                    vec![ListOp::Insert {
                        key: k(&key),
                        at,
                        value: Value::str("shape"),
                    }],
                ),
                map(
                    SHAPE_F,
                    vec![
                        set(SHAPE_F, &key, "slide", Some(&d.slide_ref(&c))),
                        set(SHAPE_F, &key, "geom", Some(&geom(rng, d.by_key))),
                        set(SHAPE_F, &key, "text", Some(WORDS[rng.below(8)])),
                    ],
                ),
            ]
        }
        49..=53 => {
            let Some(u) = rng.pick(&shapes).cloned() else {
                return false;
            };
            vec![list(SHAPES, vec![ListOp::Delete { key: u }])]
        }
        54..=57 => {
            let Some(u) = rng.pick(&gone_shapes).cloned() else {
                return false;
            };
            vec![list(SHAPES, vec![ListOp::Restore { key: u }])]
        }
        58..=63 => {
            let (Some(u), Some(b)) = (rng.pick(&shapes).cloned(), rng.pick(&shapes).cloned())
            else {
                return false;
            };
            let to = [Anchor::Start, Anchor::End, Anchor::Before(b)][rng.below(3)].clone();
            vec![list(SHAPES, vec![ListOp::Move { key: u, to }])]
        }
        64..=66 => {
            let Some(u) = rng.pick(&shapes).cloned() else {
                return false;
            };
            vec![list(
                SHAPES,
                vec![ListOp::Set {
                    key: u,
                    value: Value::str(["shape", "picture", "group"][rng.below(3)]),
                }],
            )]
        }
        // slides: insert, delete, restore, move, their fields
        67..=70 => {
            *minted += 1;
            let key = format!("s{}", 500 + *minted);
            let at = match rng.pick(&slides) {
                Some(a) if rng.below(3) != 0 => Anchor::After(a.clone()),
                _ => Anchor::Start,
            };
            vec![
                list(
                    SLIDES,
                    vec![ListOp::Insert {
                        key: k(&key),
                        at,
                        value: Value::Null,
                    }],
                ),
                map(
                    SLIDE_F,
                    vec![
                        set(SLIDE_F, &key, "sid", Some(&key[1..])),
                        set(SLIDE_F, &key, "layout", Some("@layout:7")),
                    ],
                ),
            ]
        }
        71..=74 => {
            let Some(c) = rng.pick(&slides).cloned() else {
                return false;
            };
            vec![list(SLIDES, vec![ListOp::Delete { key: c }])]
        }
        75..=78 => {
            let Some(c) = rng.pick(&gone_slides).cloned() else {
                return false;
            };
            vec![list(SLIDES, vec![ListOp::Restore { key: c }])]
        }
        79..=82 => {
            let (Some(c), Some(b)) = (rng.pick(&slides).cloned(), rng.pick(&slides).cloned())
            else {
                return false;
            };
            let to = [Anchor::Start, Anchor::End, Anchor::Before(b)][rng.below(3)].clone();
            vec![list(SLIDES, vec![ListOp::Move { key: c, to }])]
        }
        83..=88 => {
            let Some(c) = rng.pick(&slides).cloned() else {
                return false;
            };
            let (fl, v) = match rng.below(5) {
                0 => ("hidden", [Some("1"), Some("0"), None][rng.below(3)]),
                1 => ("show", [Some("1"), Some("0"), Some("x")][rng.below(3)]),
                2 => ("title", [Some("Intro"), Some(""), None][rng.below(3)]),
                3 => ("layout", [Some("@layout:1"), Some(""), None][rng.below(3)]),
                // a slide's id (never another's): its shapes go with it, and come back
                _ => ("sid", None),
            };
            let id = c.as_str()[1..].to_owned();
            let sid = [id.clone(), format!("{id}x")][rng.below(2)].clone();
            let v = if fl == "sid" { Some(sid.as_str()) } else { v };
            vec![map(SLIDE_F, vec![set(SLIDE_F, c.as_str(), fl, v)])]
        }
        // the page
        89..=90 => vec![map(
            DECK,
            vec![MapOp::Set {
                key: ["cx", "cy"][rng.below(2)].into(),
                value: Value::str([" 100", "200", "x"][rng.below(3)]),
            }],
        )],
        // two changes in one: a shape moved and given new text, a slide hidden
        91..=94 => {
            let (Some(u), Some(c)) = (rng.pick(&shapes).cloned(), rng.pick(&slides).cloned())
            else {
                return false;
            };
            vec![
                list(
                    SHAPES,
                    vec![ListOp::Move {
                        key: u.clone(),
                        to: Anchor::Start,
                    }],
                ),
                map(
                    SHAPE_F,
                    vec![
                        set(SHAPE_F, u.as_str(), "text", Some(WORDS[rng.below(8)])),
                        set(SHAPE_F, u.as_str(), "geom", Some(&geom(rng, d.by_key))),
                    ],
                ),
                map(SLIDE_F, vec![set(SLIDE_F, c.as_str(), "hidden", Some("1"))]),
            ]
        }
        // undo and redo
        _ => {
            let v = version(s);
            let _ = if rng.below(2) == 0 {
                s.undo_with(who, &UndoRequest::steps(1 + rng.below(2)))
            } else {
                s.redo_with(who, &UndoRequest::steps(1))
            };
            return version(s) > v;
        }
    };
    raw(s, who, edits)
}

/// A unit's geometry as the oracle reads a delta's string: the box, rotation and flips.
fn geometry_of(v: &str, parts: &[GeomPart]) -> Option<(i64, i64, i64, i64, i64, bool, bool)> {
    let vals: Vec<&str> = v.split(',').collect();
    if v.is_empty() || vals.len() != parts.len() {
        return None;
    }
    let mut g = (0, 0, 0, 0, 0, false, false);
    for (p, s) in parts.iter().zip(vals) {
        let n = || s.trim().parse::<i64>().ok();
        let f = || match s.trim() {
            "0" | "false" => Some(false),
            "1" | "true" => Some(true),
            _ => None,
        };
        match p {
            GeomPart::X => g.0 = n()?,
            GeomPart::Y => g.1 = n()?,
            GeomPart::W => g.2 = n()?,
            GeomPart::H => g.3 = n()?,
            GeomPart::Rot => g.4 = n()?,
            GeomPart::FlipH => g.5 = f()?,
            GeomPart::FlipV => g.6 = f()?,
        }
    }
    Some(g)
}

fn unit_geometry(u: &Unit) -> Option<(i64, i64, i64, i64, i64, bool, bool)> {
    u.bbox
        .map(|b| (b.x, b.y, b.w, b.h, u.rot, u.flip.h, u.flip.v))
}

/// What one change did to the view (`b` before it, `a` after it), against its deltas.
fn oracle(b: &ViewIndex, a: &ViewIndex, d: &ViewDeltas, parts: &[GeomPart]) -> Result<(), String> {
    // the containers' order replays from the positions
    let mut order: Vec<String> = b.containers.iter().map(|c| c.key.clone()).collect();
    for c in &d.containers {
        let at = |p: Option<u32>| p.map(|p| p as usize - 1);
        match c.op.as_str() {
            "add" | "restore" => {
                let i = at(c.position_after).ok_or("an add with no position")?;
                if i > order.len() || order.contains(&c.key) {
                    return Err(format!(
                        "{} {} at {i} doesn't fit {order:?}",
                        c.op.as_str(),
                        c.key
                    ));
                }
                order.insert(i, c.key.clone());
            }
            "delete" => {
                let i = at(c.position_before).ok_or("a delete with no position")?;
                if order.get(i) != Some(&c.key) {
                    return Err(format!("delete {} at {i}: {order:?}", c.key));
                }
                order.remove(i);
            }
            "reorder" => {
                let (i, j) = (at(c.position_before), at(c.position_after));
                let (Some(i), Some(j)) = (i, j) else {
                    return Err("a reorder with no positions".into());
                };
                if order.get(i) != Some(&c.key) || j >= order.len() {
                    return Err(format!("reorder {} {i}->{j}: {order:?}", c.key));
                }
                let x = order.remove(i);
                order.insert(j, x);
            }
            _ => {}
        }
    }
    let after: Vec<String> = a.containers.iter().map(|c| c.key.clone()).collect();
    if order != after {
        return Err(format!(
            "the containers replay to {order:?}, the view has {after:?}"
        ));
    }
    // a container that stayed and changed has its delta
    for c in &a.containers {
        let Some(o) = b.containers.iter().find(|x| x.key == c.key) else {
            continue;
        };
        let named = d.containers.iter().filter(|x| x.key == c.key);
        if o.hidden != c.hidden
            && !named
                .clone()
                .any(|x| x.op.as_str() == "hide_show" && x.hidden_after == Some(c.hidden))
        {
            return Err(format!(
                "{} hidden {} -> {}: no hide_show",
                c.key, o.hidden, c.hidden
            ));
        }
        if (o.title != c.title || o.layout != c.layout)
            && !named.clone().any(|x| !x.fields.is_empty())
        {
            return Err(format!("{}'s fields changed: no delta names them", c.key));
        }
    }
    // the units
    let ub: HashMap<&str, &Unit> = b.units.iter().map(|u| (u.key.as_str(), u)).collect();
    let ua: HashMap<&str, &Unit> = a.units.iter().map(|u| (u.key.as_str(), u)).collect();
    let mut by: HashMap<&str, &UnitDelta> = HashMap::new();
    for u in &d.units {
        if by.insert(u.key.as_str(), u).is_some() {
            return Err(format!("{} has two deltas", u.key));
        }
        if !ub.contains_key(u.key.as_str()) && !ua.contains_key(u.key.as_str()) {
            return Err(format!(
                "a delta for {}, which the view never showed",
                u.key
            ));
        }
    }
    let has = |u: &UnitDelta, o: &str| u.ops.iter().any(|x| x.as_str() == o);
    let mut keys: Vec<&str> = ub.keys().chain(ua.keys()).copied().collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let d = by.get(key);
        match (ub.get(key), ua.get(key)) {
            (None, Some(n)) => {
                let d = d.ok_or_else(|| format!("{key} came into view: no delta"))?;
                if !(has(d, "add") || has(d, "restore"))
                    || d.container.as_deref() != Some(&n.container)
                {
                    return Err(format!("{key} came into view in {}: {d:?}", n.container));
                }
                let g = d.geom_after.as_deref().and_then(|g| geometry_of(g, parts));
                if g != unit_geometry(n) {
                    return Err(format!(
                        "{key} came with geometry {:?}: {d:?}",
                        unit_geometry(n)
                    ));
                }
            }
            (Some(o), None) => {
                let d = d.ok_or_else(|| format!("{key} left the view: no delta"))?;
                if d.ops != [UnitOp::Delete] || d.container.as_deref() != Some(&o.container) {
                    return Err(format!("{key} left the view from {}: {d:?}", o.container));
                }
            }
            (Some(o), Some(n)) => {
                let moved = unit_geometry(o) != unit_geometry(n);
                let changed = moved
                    || o.hidden != n.hidden
                    || o.text != n.text
                    || o.container != n.container
                    || o.kind != n.kind
                    || o.name != n.name
                    || o.placeholder != n.placeholder
                    || o.editable_text != n.editable_text;
                let Some(d) = d else {
                    if changed {
                        return Err(format!("{key} changed: no delta"));
                    }
                    continue;
                };
                if d.container.as_deref() != Some(&n.container) {
                    return Err(format!("{key} is in {}: {d:?}", n.container));
                }
                if moved {
                    let geo = has(d, "move_resize") || has(d, "rotate") || has(d, "flip");
                    let g = d.geom_after.as_deref().and_then(|g| geometry_of(g, parts));
                    let gb = d.geom_before.as_deref().and_then(|g| geometry_of(g, parts));
                    if !geo || g != unit_geometry(n) || gb != unit_geometry(o) {
                        return Err(format!(
                            "{key} geometry {:?} -> {:?}: {d:?}",
                            unit_geometry(o),
                            unit_geometry(n)
                        ));
                    }
                }
                if o.hidden != n.hidden
                    && !(has(d, "hide_show") && d.hidden_after == Some(n.hidden))
                {
                    return Err(format!("{key} hidden -> {}: {d:?}", n.hidden));
                }
                if o.text != n.text && !has(d, "text") {
                    return Err(format!("{key} text changed: {d:?}"));
                }
            }
            (None, None) => {}
        }
    }
    Ok(())
}

/// One seeded trace on the deck (`by_key`: the second naming): the watcher's frames, and the
/// view index at checkpoints (after the frame numbered `seq`). Every change is checked by the
/// oracle.
struct Trace {
    frames: Vec<WatchFrame>,
    checks: Vec<(u64, ViewIndex)>,
    schema: ViewSchema,
}

fn run_trace(seed: u64, by_key: bool, steps: usize) -> Result<Trace, String> {
    let f = fx(&format!("trace{seed}{by_key}"));
    let schema = if by_key { BY_KEY } else { BY_ID };
    let s = open(&f, Some(schema), "t.deck", &small_deck(by_key));
    let own = join(&s, "own", ParticipantKind::Person);
    let bot = join(&s, "atlas", ParticipantKind::Agent);
    let ada = join(&s, "ada", ParticipantKind::Person);
    let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    let mut frames = s.watch_snapshot(&mut w).unwrap();
    let mut checks = Vec::new();
    let d = Doc { s: &s, by_key };
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut minted = 0;
    let parts = schema.geometry.map_or(&[][..], |g| g.parts);
    let mut done = 0;
    let mut tries = 0;
    while done < steps && tries < steps * 20 {
        tries += 1;
        let before = s.view_index(ada).unwrap().expect("a view index");
        let who = if rng.below(3) == 0 { bot } else { own };
        if !step(&d, &mut rng, who, &mut minted) {
            if !w.try_frames().is_empty() {
                return Err(format!("seed {seed}: a frame for no change"));
            }
            continue;
        }
        done += 1;
        let after = s.view_index(ada).unwrap().expect("a view index");
        let got = w.try_frames();
        let ch = change_frames(&got);
        if ch.len() != got.len() || ch.is_empty() {
            return Err(format!("seed {seed} step {done}: frames {got:?}"));
        }
        // an undo of two steps is two frames: the oracle reads the last against the view before
        // the first, so only a single frame is checked
        if ch.len() == 1 {
            let deltas = ch[0].deltas.as_ref().ok_or("a frame with no deltas")?;
            oracle(&before, &after, deltas, parts)
                .map_err(|e| format!("seed {seed} by_key {by_key} step {done}: {e}"))?;
        }
        let last = got.last().map(WatchFrame::seq).unwrap_or(0);
        frames.extend(got);
        if done % 9 == 0 || done == steps {
            checks.push((last, after));
        }
    }
    if done < steps {
        return Err(format!("seed {seed}: only {done} steps of {steps}"));
    }
    Ok(Trace {
        frames,
        checks,
        schema,
    })
}

const SEEDS: &[u64] = &[1, 2, 3, 4, 5];
const STEPS: usize = 45;

fn row_traces() -> Result<(), String> {
    for &seed in SEEDS {
        for by_key in [false, true] {
            run_trace(seed, by_key, STEPS)?;
        }
    }
    Ok(())
}

#[test]
fn every_change_s_deltas_say_what_the_view_did() {
    row_traces().unwrap();
}

#[test]
fn plant_delta_geom_stale_is_caught_by_the_oracle() {
    let e = with_watch_mutant(WatchMutant::DeltaGeomStale, row_traces)
        .expect_err("plant DeltaGeomStale must fail the oracle");
    assert!(e.contains("geometry"), "caught on a geometry: {e}");
}

// ---------- no schema, no deltas ----------

#[test]
fn a_pack_without_a_view_schema_gets_no_deltas() {
    let f = fx("none");
    let s = open(&f, None, "n.deck", REV40);
    let own = join(&s, "own", ParticipantKind::Person);
    let ada = join(&s, "ada", ParticipantKind::Person);
    let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    s.watch_snapshot(&mut w).unwrap();
    assert!(raw(
        &s,
        own,
        vec![map(
            SHAPE_F,
            vec![set(SHAPE_F, "s256-2", "text", Some("x"))]
        )]
    ));
    let fs = w.try_frames();
    let ch = change_frames(&fs);
    assert_eq!(ch.len(), 1);
    assert!(ch[0].deltas.is_none());
    let Json::Obj(m) = fs[0].to_value() else {
        panic!("an object")
    };
    for n in ["units", "containers_changed", "containers"] {
        assert!(
            m.iter().all(|(k, _)| k != n),
            "{n} on a frame with no schema"
        );
    }
}

// ---------- the client's vectors ----------

fn golden(path: &std::path::Path, got: &str) {
    if std::env::var_os("MPC_BLESS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, got).unwrap();
        return;
    }
    let want = fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the kit encodes now (MPC_BLESS=1 regenerates it)",
        path.display()
    );
}

/// Fixed numbers for the run's random client IDs, in order of appearance.
fn stable(frames: &[WatchFrame]) -> Json {
    let mut ids: HashMap<u64, u64> = HashMap::new();
    Json::List(
        frames
            .iter()
            .map(|f| {
                let mut f = f.clone();
                if let WatchFrame::Change(c) = &mut f {
                    let n = ids.len() as u64 + 1;
                    c.client = *ids.entry(c.client).or_insert(n);
                    c.claim = None;
                }
                f.to_value()
            })
            .collect(),
    )
}

/// A view index as the client compares it: the snapshot's view (with its schema) and each
/// container's units.
fn view_value(schema: &ViewSchema, ix: &ViewIndex) -> Json {
    let mut units: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    for c in &ix.containers {
        units.insert(c.key.clone(), Vec::new());
    }
    for u in &ix.units {
        units
            .entry(u.container.clone())
            .or_default()
            .push(unit_value(u));
    }
    Json::obj([
        ("view", ix.snapshot().to_value(schema)),
        (
            "units",
            Json::obj(units.into_iter().map(|(k, v)| (k, Json::List(v)))),
        ),
    ])
}

#[test]
fn the_clients_view_vectors_are_current() {
    let mut cases = Vec::new();
    for &seed in &SEEDS[..4] {
        for by_key in [false, true] {
            let t = run_trace(seed, by_key, STEPS).unwrap();
            cases.push(Json::obj([
                ("seed", Json::from(seed)),
                ("by_key", Json::from(by_key)),
                ("frames", stable(&t.frames)),
                (
                    "checks",
                    Json::List(
                        t.checks
                            .iter()
                            .map(|(seq, ix)| {
                                Json::obj([
                                    ("after_seq", Json::from(*seq)),
                                    ("expect", view_value(&t.schema, ix)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]));
        }
    }
    golden(
        &vectors_dir().join("view_deltas.json"),
        &(Json::obj([
            (
                "about",
                Json::from(
                    "k2-mpc-base view delta vectors for the JS client: per seeded trace on a \
                     deck with a view schema (slides named by an id field, or by key), the \
                     frames a watcher got (a snapshot in pages, then one frame per change, \
                     with units, containers_changed and containers), and the view index at \
                     checkpoints: the snapshot's view and each container's units, which the \
                     client's view must equal after the frame numbered after_seq. Written by \
                     crates/base/tests/view_deltas.rs (MPC_BLESS=1).",
                ),
            ),
            ("cases", Json::List(cases)),
        ])
        .to_json()
            + "\n"),
    );
}

// ---------- a JSON reader for the golden (the kit has a writer only) ----------

fn parse_json(s: &str) -> Json {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i);
    skip(b, &mut i);
    assert_eq!(i, b.len(), "trailing bytes in the golden");
    v
}

fn skip(b: &[u8], i: &mut usize) {
    while *i < b.len() && b[*i].is_ascii_whitespace() {
        *i += 1;
    }
}

fn value(b: &[u8], i: &mut usize) -> Json {
    skip(b, i);
    match b[*i] {
        b'{' => {
            *i += 1;
            let mut m: Vec<(String, Json)> = Vec::new();
            loop {
                skip(b, i);
                if b[*i] == b'}' {
                    *i += 1;
                    break;
                }
                let Json::Str(k) = value(b, i) else {
                    panic!("a key")
                };
                skip(b, i);
                assert_eq!(b[*i], b':');
                *i += 1;
                m.push((k, value(b, i)));
                skip(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            Json::Obj(m)
        }
        b'[' => {
            *i += 1;
            let mut l = Vec::new();
            loop {
                skip(b, i);
                if b[*i] == b']' {
                    *i += 1;
                    break;
                }
                l.push(value(b, i));
                skip(b, i);
                if b[*i] == b',' {
                    *i += 1;
                }
            }
            Json::List(l)
        }
        b'"' => {
            *i += 1;
            let mut out = String::new();
            loop {
                let c = b[*i];
                *i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let e = b[*i];
                        *i += 1;
                        match e {
                            b'n' => out.push('\n'),
                            b't' => out.push('\t'),
                            b'"' => out.push('"'),
                            b'\\' => out.push('\\'),
                            b'/' => out.push('/'),
                            b'u' => {
                                let h = std::str::from_utf8(&b[*i..*i + 4]).unwrap();
                                *i += 4;
                                out.push(
                                    char::from_u32(u32::from_str_radix(h, 16).unwrap()).unwrap(),
                                );
                            }
                            _ => panic!("an escape"),
                        }
                    }
                    _ => {
                        // a UTF-8 sequence, whole
                        let start = *i - 1;
                        let len = match c {
                            0..=0x7f => 1,
                            0xc0..=0xdf => 2,
                            0xe0..=0xef => 3,
                            _ => 4,
                        };
                        *i = start + len;
                        out.push_str(std::str::from_utf8(&b[start..*i]).unwrap());
                    }
                }
            }
            Json::Str(out)
        }
        b't' => {
            *i += 4;
            Json::Bool(true)
        }
        b'f' => {
            *i += 5;
            Json::Bool(false)
        }
        b'n' => {
            *i += 4;
            Json::Null
        }
        _ => {
            let start = *i;
            while *i < b.len() && (b[*i] == b'-' || b[*i].is_ascii_digit()) {
                *i += 1;
            }
            let n: i64 = std::str::from_utf8(&b[start..*i]).unwrap().parse().unwrap();
            Json::from(n)
        }
    }
}
