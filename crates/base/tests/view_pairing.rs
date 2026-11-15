//! The view hooks read the carrier's **pairing** (`ViewCarrier::pairing`): a pack whose units
//! carry facts only the file holds finds each unit's by its key, not by its live position, so
//! the view index is kept through changes between carriers.
//!
//! The toy pack, `paired-view`: one box (a container) of items (units) minted `i1`, `i2`, ...
//! in file order; the file is one item a line, `name|color`; the colour is the file's alone
//! (the store holds the name), a derived value of each unit (`color`) read from the carrier
//! through the pairing. A live insert, move or delete makes no new carrier.
//!
//! - a unit added between carriers has no colour, every other unit keeps its own, and keeping
//!   the index builds only the units the change touched (`take_view_unit_builds`), not the
//!   whole index;
//! - the planted pack bug "the derive reads the carrier by live position" is caught by name;
//! - a pack that doesn't pair gets no pairing.
//!
//! Each row is a function returning `Err("<row name>: ...")`. Public API only.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{set_view_checks, take_view_unit_builds};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, ListOp, MapOp};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, Host, Pack, PackContent, PackError, Pairing,
    Participant, ParticipantKind, Rights, Session, SessionManager, SpliceError, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{
    ContainerFields, Derived, FieldClass, Src, UnitFields, ViewIndex, ViewSchema, ViewSource,
    ViewTouched,
};
use k2_mpc_base::{Key, Value};

const BOXES: &str = "boxes";
const ITEMS: &str = "items";
const ITEM_F: &str = "item_f";
const BOX: &str = "b1";

const SCHEMA: Schema = Schema {
    name: "paired-view",
    roots: &[
        (BOXES, Kind::MovableList),
        (ITEMS, Kind::MovableList),
        (ITEM_F, Kind::Map),
    ],
};

const VIEW: ViewSchema = ViewSchema {
    containers: BOXES,
    units: ITEMS,
    unit_container_field: "box",
    container_id_field: None,
    native_unit: "paired-item",
    page: None,
    container_fields: ContainerFields {
        title: Src::None,
        hidden: Src::None,
        layout: Src::None,
        parent: Src::None,
    },
    unit_fields: UnitFields {
        name: Src::Field("name"),
        ..UnitFields::NONE
    },
    field_class: &[("name", FieldClass::Text), ("box", FieldClass::Meta)],
    derived: Derived {
        containers: &[],
        units: &[],
    },
    unit_derived: &["color"],
    ..ViewSchema::NONE
};

thread_local! {
    /// The planted pack bug: the derive reads the carrier by the unit's live position.
    static BY_POSITION: Cell<bool> = const { Cell::new(false) };
    /// What the last derive saw: whether its carrier had a pairing.
    static SAW_PAIRING: Cell<Option<bool>> = const { Cell::new(None) };
}

/// The toy pack, pairing (`true`) or not.
struct Paired(bool);

fn prop(k: &str, f: &str) -> String {
    format!("{k}\u{1f}{f}")
}

/// The file's items: (name, color), in file order.
fn lines(bytes: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| match l.split_once('|') {
            Some((n, c)) => (n.to_owned(), c.to_owned()),
            None => (l.to_owned(), String::new()),
        })
        .collect()
}

/// Each unit's colour in `index`: from the carrier through the pairing (by key); with no
/// pairing, or under the plant, by the unit's live position.
fn color_of(source: &ViewSource<'_>, index: &ViewIndex, at: usize) -> Option<String> {
    let ViewSource::Live(_, Some(carrier)) = source else {
        return None;
    };
    let file = carrier.decoded(|b| Some(lines(b)))?;
    let key = &index.units[at].key;
    let pos = match carrier.pairing() {
        Some(p) if !BY_POSITION.with(Cell::get) => {
            p.keys(ITEMS)?.iter().position(|k| k.as_str() == key)?
        }
        _ => at,
    };
    file.get(pos)
        .map(|(_, c)| c.clone())
        .filter(|c| !c.is_empty())
}

fn derive(source: &ViewSource<'_>, index: &mut ViewIndex, which: &[usize]) {
    if let ViewSource::Live(_, Some(c)) = source {
        SAW_PAIRING.with(|s| s.set(Some(c.pairing().is_some())));
    }
    for &at in which {
        if let Some(c) = color_of(source, index, at) {
            index.units[at].set_derived("color", c);
        }
    }
}

impl Pack for Paired {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(ITEM_F, ITEMS)]
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let (mut items, mut f) = (Vec::new(), BTreeMap::new());
        for (i, (name, _)) in lines(bytes).iter().enumerate() {
            let k = format!("i{}", i + 1);
            f.insert(prop(&k, "name"), Value::str(name));
            f.insert(prop(&k, "box"), Value::str(BOX));
            items.push((Key::new(k).unwrap(), Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                (
                    BOXES.into(),
                    Neutral::List(vec![(Key::new(BOX).unwrap(), Value::Null)]),
                ),
                (ITEMS.into(), Neutral::List(items)),
                (ITEM_F.into(), Neutral::Map(f)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, content: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        self.splice(content, &BTreeMap::new())
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    fn wants_pairing(&self) -> bool {
        self.0
    }
    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        content: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        let file = lines(carrier);
        let keys = pairing.keys(ITEMS).unwrap_or(&[]);
        if keys.len() != file.len() {
            return Err(SpliceError::PairingMismatch("items".into()));
        }
        let colors: BTreeMap<String, String> = keys
            .iter()
            .zip(file)
            .map(|(k, (_, c))| (k.to_string(), c))
            .collect();
        Ok(self.splice(content, &colors)?)
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VIEW)
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        let all: Vec<usize> = (0..index.units.len()).collect();
        derive(&source, index, &all);
    }
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &ViewTouched,
    ) -> bool {
        // without the pairing a unit's file facts can't be found once units came or went
        let paired = matches!(&source, ViewSource::Live(_, Some(c)) if c.pairing().is_some());
        if !paired {
            return false;
        }
        let which: Vec<usize> = (0..index.units.len())
            .filter(|&i| touched.units.contains(&index.units[i].key))
            .collect();
        derive(&source, index, &which);
        true
    }
}

impl Paired {
    /// The content's items as lines, each with its colour from `colors` (by key) or none.
    fn splice(
        &self,
        content: &[(String, Neutral)],
        colors: &BTreeMap<String, String>,
    ) -> Result<Vec<u8>, PackError> {
        let root = |n: &str| content.iter().find(|(r, _)| r == n).map(|(_, v)| v);
        let (Some(Neutral::List(items)), Some(Neutral::Map(f))) = (root(ITEMS), root(ITEM_F))
        else {
            return Err(PackError("no items".into()));
        };
        let mut out = String::new();
        for (k, _) in items {
            let name = f
                .get(&prop(k.as_str(), "name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let color = colors.get(k.as_str()).map_or("", String::as_str);
            out.push_str(&format!("{name}|{color}\n"));
        }
        Ok(out.into_bytes())
    }
}

// ---------- harness ----------

struct TestHost;

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_000_000
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: std::path::PathBuf,
    s: Arc<Session>,
    ada: ConnId,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const FILE: &str = "Alpha|red\nBeta|green\nGamma|blue\n";

fn fx(tag: &str, pairs: bool) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-view-pairing-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let path = ws.join("list.items");
    std::fs::write(&path, FILE).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        k2_mpc_base::blob::BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            k2_mpc_base::blob::BlobConfig::default(),
            Arc::new(|| 1_000_000),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, Arc::new(TestHost), c).with_blob_store(blobs);
    let key = DocKey {
        space: "ws".into(),
        path: "list.items".into(),
    };
    let s = mgr
        .open_file(Arc::new(Paired(pairs)), &key, &path, &ws)
        .unwrap();
    let ada = s
        .join(
            Participant::new("ada", "Ada", "#123456", ParticipantKind::Person, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    Fx { dir, s, ada }
}

impl Fx {
    fn edit(&self, edits: Vec<Edit>) -> Result<(), String> {
        self.s
            .edit_ops(self.ada, edits, &EditOptions::new())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    /// Each unit's colour in the view index now, by key, in the index's order.
    fn colors(&self) -> Result<Vec<(String, Option<String>)>, String> {
        let ix = self
            .s
            .view_index(self.ada)
            .map_err(|e| e.to_string())?
            .ok_or("no view index")?;
        Ok(ix
            .units
            .iter()
            .map(|u| (u.key.clone(), u.derived_value("color").map(str::to_owned)))
            .collect())
    }
}

fn want(pairs: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(k, c)| (k.to_string(), c.map(str::to_owned)))
        .collect()
}

/// A unit added, moved or deleted between carriers: every other unit keeps its own file facts,
/// the new one has none, and each change builds only the units it touched.
fn row_a_unit_added_between_carriers_keeps_the_facts() -> Result<(), String> {
    const ROW: &str = "row_a_unit_added_between_carriers_keeps_the_facts";
    let checks = set_view_checks(false);
    let r = (|| {
        let f = fx("added", true);
        let got = f.colors()?;
        let w = want(&[
            ("i1", Some("red")),
            ("i2", Some("green")),
            ("i3", Some("blue")),
        ]);
        if got != w {
            return Err(format!("{ROW}: control: the open's colours {got:?}"));
        }
        take_view_unit_builds();
        f.edit(vec![
            Edit::List {
                root: ITEMS.into(),
                ops: vec![ListOp::Insert {
                    key: Key::new("n1").unwrap(),
                    at: Anchor::Start,
                    value: Value::Null,
                }],
            },
            Edit::Map {
                root: ITEM_F.into(),
                ops: vec![
                    MapOp::Set {
                        key: prop("n1", "name"),
                        value: Value::str("New"),
                    },
                    MapOp::Set {
                        key: prop("n1", "box"),
                        value: Value::str(BOX),
                    },
                ],
            },
        ])?;
        let got = f.colors()?;
        let w = want(&[
            ("n1", None),
            ("i1", Some("red")),
            ("i2", Some("green")),
            ("i3", Some("blue")),
        ]);
        if got != w {
            return Err(format!("{ROW}: after an insert at the start: {got:?}"));
        }
        let built = take_view_unit_builds();
        if built != 1 {
            return Err(format!(
                "{ROW}: after an insert, keeping the index built {built} units (the new one: 1)"
            ));
        }
        f.edit(vec![Edit::List {
            root: ITEMS.into(),
            ops: vec![ListOp::Move {
                key: Key::new("i3").unwrap(),
                to: Anchor::Start,
            }],
        }])?;
        f.edit(vec![Edit::List {
            root: ITEMS.into(),
            ops: vec![ListOp::Delete {
                key: Key::new("i1").unwrap(),
            }],
        }])?;
        let got = f.colors()?;
        let w = want(&[("i3", Some("blue")), ("n1", None), ("i2", Some("green"))]);
        if got != w {
            return Err(format!("{ROW}: after a move and a delete: {got:?}"));
        }
        let built = take_view_unit_builds();
        if built > 1 {
            return Err(format!(
                "{ROW}: after a move and a delete, keeping the index built {built} units"
            ));
        }
        Ok(())
    })();
    set_view_checks(checks);
    r
}

#[test]
fn a_unit_added_between_carriers_keeps_the_facts() {
    row_a_unit_added_between_carriers_keeps_the_facts().unwrap();
}

#[test]
fn plant_derive_by_position_is_caught() {
    row_a_unit_added_between_carriers_keeps_the_facts().expect("the row passes as built");
    BY_POSITION.with(|p| p.set(true));
    let e = row_a_unit_added_between_carriers_keeps_the_facts();
    BY_POSITION.with(|p| p.set(false));
    let e = e.expect_err("the row passes with the derive reading by position");
    assert!(
        e.starts_with("row_a_unit_added_between_carriers_keeps_the_facts: after an insert"),
        "caught, but not where the row says: {e}"
    );
}

/// A pack that doesn't pair: its hooks see a carrier with no pairing.
#[test]
fn a_pack_that_does_not_pair_gets_no_pairing() {
    let f = fx("unpaired", false);
    SAW_PAIRING.with(|s| s.set(None));
    f.colors().unwrap();
    assert_eq!(SAW_PAIRING.with(Cell::get), Some(false));
    // and a pairing pack's do
    let f = fx("paired", true);
    SAW_PAIRING.with(|s| s.set(None));
    f.colors().unwrap();
    assert_eq!(SAW_PAIRING.with(Cell::get), Some(true));
}
