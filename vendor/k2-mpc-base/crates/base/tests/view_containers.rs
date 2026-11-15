//! Containers in containers: a container's parent in the view index ([`Container::parent`]), on
//! an outline-shaped toy pack (collections holding collections, and objects in collections, as a
//! scene's outliner shows them), live as keyed roots and on the blob tier as a structure index,
//! one file read both ways. Public API only.
//!
//! The rows: the containers' tree comes from both sources, the same, in the flat order, with
//! the parent read from a field (its key, mapped to an address) or set by the pack's derive
//! hook; a parent the index can't carry (no key, no such container, the container itself, a
//! cycle, deeper than `CONTAINER_DEPTH_MAX`) fails a test build loudly, also on a doc that keeps
//! its index through a change; a release build leaves it out, records it, and keeps the rest
//! of the tree (a cycle broken at its first container, a deep container listed at the top
//! level); the snapshot carries `parent` only when a container has one; a keyed watch's change
//! frames carry each changed parent as `parent_after` (a named change, a container that comes,
//! one cleared, and one the pack derives from another root, with no delta of its own); the
//! client's golden.
//!
//! And a container's derived values ([`Container::derived`], the container half of a unit's): the
//! schema's names only, in its order, cut, on the index, the host's read and the snapshot, the
//! schema's names checked; a change frame carries the values a change set or removed under
//! `derived.containers`, and only those; the client's golden.
//!
//! Planted faults (`k2_mpc_base::differential::ViewMutant`) are caught by name: each
//! `plant_*_is_caught` row needs its check to pass as built and to fail under the fault where it
//! says.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{with_view_mutant, ViewMutant};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, KeyedRoot, KeyedState, ListOp, MapOp};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::index::{
    Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs,
};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    watch_state_json, AuditEvent, BlobPack, BlobSession, Config, ConnId, Declined, EditOptions,
    Host, KeyedWatch, Pack, PackContent, PackError, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::view_index::{
    BlobView, Container, ContainerFields, Derived, FieldClass, NodeSrc, ParentRefused, Src, Tier,
    UnitFields, ViewIndex, ViewRefusal, ViewSchema, ViewSource, ViewTouched, CONTAINER_DEPTH_MAX,
    CONTAINER_DERIVED_MAX, VIEW_TEXT_MAX,
};
use k2_mpc_base::{Key, Value};

const COLLS: &str = "colls";
const COLL_F: &str = "coll_f";
const OBJS: &str = "objs";
const OBJ_F: &str = "obj_f";
/// The parents again, `cN` -> `cM`: what the derive hook reads in the derived mode.
const TREE: &str = "tree";

const SCHEMA: Schema = Schema {
    name: "outline",
    roots: &[
        (COLLS, Kind::MovableList),
        (COLL_F, Kind::Map),
        (OBJS, Kind::MovableList),
        (OBJ_F, Kind::Map),
        (TREE, Kind::Map),
    ],
};

const NO_FIELDS: &[&str] = &[];
const PARENT_FIELD: &[&str] = &["parent"];

/// The toy's view schema: collections (containers) hold objects (units); a collection's parent
/// is its `parent` field (the parent's key), or, `derived`, the pack's (from the tree map).
const fn view(derived: bool) -> ViewSchema {
    ViewSchema {
        containers: COLLS,
        units: OBJS,
        unit_container_field: "coll",
        native_unit: "scene-unit",
        container_fields: ContainerFields {
            title: Src::Field("name"),
            parent: if derived {
                Src::Derived("parent")
            } else {
                Src::Field("parent")
            },
            ..ContainerFields::NONE
        },
        unit_fields: UnitFields {
            kind: Src::ListValue,
            name: Src::Field("name"),
            ..UnitFields::NONE
        },
        field_class: &[("name", FieldClass::Text), ("parent", FieldClass::Meta)],
        derived: Derived {
            containers: if derived { PARENT_FIELD } else { NO_FIELDS },
            units: NO_FIELDS,
        },
        blob: Some(BlobView {
            container_kind: "coll",
            container_title: NodeSrc::Name,
            container_parent: NodeSrc::Attr("parent"),
            ..BlobView::NONE
        }),
        ..ViewSchema::NONE
    }
}

// ----- the toy's file -----

/// A parent as the file writes it: `-` none, a number (a collection's), or `=raw` (a value as
/// it is, for the bad cases).
fn parent_cell(s: &str) -> Option<Parent> {
    match s {
        "-" => None,
        _ => Some(match s.strip_prefix('=') {
            Some(raw) => Parent::Raw(raw.to_owned()),
            None => Parent::Coll(s.parse().expect("a collection number")),
        }),
    }
}

#[derive(Debug, Clone)]
enum Parent {
    Coll(u32),
    Raw(String),
}

impl Parent {
    /// As the live store holds it: the parent's key, or the raw value.
    fn live(&self) -> String {
        match self {
            Parent::Coll(n) => ckey(*n).to_string(),
            Parent::Raw(r) => r.clone(),
        }
    }
    /// As the structure index holds it: the parent's node key (its address), or the raw value.
    fn node(&self) -> String {
        match self {
            Parent::Coll(n) => caddr(*n),
            Parent::Raw(r) => r.clone(),
        }
    }
}

struct Outline {
    /// `C|n|name|parent`.
    colls: Vec<(u32, String, Option<Parent>)>,
    /// `O|coll|n|name`.
    objs: Vec<(u32, u32, String)>,
}

fn parse(bytes: &[u8]) -> Result<Outline, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "not utf-8".to_owned())?;
    let mut lines = text.lines();
    if lines.next() != Some("OUTLINE") {
        return Err("no OUTLINE header".into());
    }
    let mut o = Outline {
        colls: Vec::new(),
        objs: Vec::new(),
    };
    let num = |s: &str| s.parse::<u32>().map_err(|_| format!("bad number {s:?}"));
    for line in lines {
        let f: Vec<&str> = line.split('|').collect();
        match f[..] {
            ["C", n, name, parent] => o.colls.push((num(n)?, name.into(), parent_cell(parent))),
            ["O", c, n, name] => o.objs.push((num(c)?, num(n)?, name.into())),
            _ => return Err(format!("bad line {line:?}")),
        }
    }
    Ok(o)
}

fn ckey(n: u32) -> Key {
    Key::new(format!("c{n}")).unwrap()
}

fn okey(n: u32) -> Key {
    Key::new(format!("o{n}")).unwrap()
}

fn caddr(n: u32) -> String {
    format!("@coll:{n}")
}

fn prop(k: &Key, f: &str) -> String {
    format!("{k}\u{1f}{f}")
}

/// A number with no sign and no leading zero.
fn canon(s: &str) -> Option<u32> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    s.parse().ok()
}

fn map_root<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    state.roots.iter().find_map(|(r, root)| match root {
        KeyedRoot::Map(m) if r == name => Some(m),
        _ => None,
    })
}

/// The live side. `derived`: the parents are the derive hook's, from the tree map.
struct OutlinePack {
    derived: bool,
}

impl OutlinePack {
    /// The derive hook's parents: each container's from the tree map (a key, mapped to its
    /// address as the kit maps a field's), `None` when it has none there.
    fn derive_parents(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        let ViewSource::Live(state, _) = source else {
            return;
        };
        if !self.derived {
            return;
        }
        let tree = map_root(state, TREE);
        for c in &mut index.containers {
            let Some(k) = self.key_of(COLLS, &c.key) else {
                continue;
            };
            c.parent = tree
                .and_then(|m| m.get(k.as_str()))
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(|v| match Key::new(v) {
                    Ok(p) => self.address_of(COLLS, &p),
                    Err(_) => v.to_owned(),
                });
        }
    }
}

impl Pack for OutlinePack {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(COLL_F, COLLS), (OBJ_F, OBJS)]
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let o = parse(bytes).map_err(|m| Declined {
            code: "not_outline",
            message: m,
        })?;
        let (mut colls, mut objs) = (Vec::new(), Vec::new());
        let (mut cf, mut of, mut tree) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        for (n, name, parent) in &o.colls {
            let k = ckey(*n);
            cf.insert(prop(&k, "name"), Value::str(name));
            if let Some(p) = parent {
                cf.insert(prop(&k, "parent"), Value::str(p.live()));
                tree.insert(k.to_string(), Value::str(p.live()));
            }
            colls.push((k, Value::str("coll")));
        }
        for (c, n, name) in &o.objs {
            let k = okey(*n);
            of.insert(prop(&k, "coll"), Value::str(ckey(*c).as_str()));
            of.insert(prop(&k, "name"), Value::str(name));
            objs.push((k, Value::str("obj")));
        }
        Ok(PackContent {
            roots: vec![
                (COLLS.into(), Neutral::List(colls)),
                (COLL_F.into(), Neutral::Map(cf)),
                (OBJS.into(), Neutral::List(objs)),
                (OBJ_F.into(), Neutral::Map(of)),
                (TREE.into(), Neutral::Map(tree)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(view(self.derived))
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        self.derive_parents(source, index);
    }
    /// By change: every parent derived again (the tree map is small), so the kept index is a
    /// whole build's (test builds check it after every change).
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        _: &ViewTouched,
    ) -> bool {
        self.derive_parents(source, index);
        true
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match (root, canon(&key.as_str()[1..])) {
            (COLLS, Some(n)) => caddr(n),
            (OBJS, Some(n)) => format!("@obj:{n}"),
            _ => key.to_string(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        match root {
            COLLS => Some(ckey(canon(address.strip_prefix("@coll:")?)?)),
            OBJS => Some(okey(canon(address.strip_prefix("@obj:")?)?)),
            _ => None,
        }
    }
}

/// The blob side: the same file read into a structure index, every collection a root node
/// with its parent's node key in the `parent` attribute.
struct OutlineIndex;

const VERBS: IndexVerbs =
    IndexVerbs::new("coll list", "coll show", "coll diff", "coll blame", None);

impl BlobPack for OutlineIndex {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("outline", 1, &VERBS))
    }
    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        _limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let mut bytes = Vec::new();
        src.read_to_end(&mut bytes).map_err(IndexRefused::Read)?;
        let o = parse(&bytes).map_err(|why| IndexRefused::Unreadable { why })?;
        let mut d = IndexDraft::new("outline-1");
        for (n, name, parent) in &o.colls {
            let mut c = DraftNode::new("coll", name.clone(), Digest::of(name.as_bytes()));
            c.key = Some(caddr(*n));
            if let Some(p) = parent {
                c.attrs.insert("parent".into(), p.node());
            }
            d.nodes.push(c);
            let at = (d.nodes.len() - 1) as u32;
            // top to bottom: the front first (the file lists them back to front)
            for (_, on, oname) in o.objs.iter().filter(|(c, _, _)| c == n).rev() {
                let mut u = DraftNode::new("obj", oname.clone(), Digest::of(oname.as_bytes()));
                u.key = Some(format!("@obj:{on}"));
                u.parent = Some(at);
                d.nodes.push(u);
            }
        }
        Ok(d)
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(view(false))
    }
}

// ----- the fixture -----

struct TestHost;

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        100_000
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
        "k2mb-cparent-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(|| 100_000),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, Arc::new(TestHost), c).with_blob_store(blobs);
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

/// A live doc and a connection to it.
struct Live {
    s: Arc<Session>,
    ada: ConnId,
}

impl Fx {
    fn live(&self, bytes: &[u8], derived: bool) -> Live {
        self.live_with(
            Arc::new(OutlinePack { derived }),
            if derived { "d.outline" } else { "f.outline" },
            bytes,
        )
    }
    fn live_with(&self, pack: Arc<dyn Pack>, path: &str, bytes: &[u8]) -> Live {
        let s = self.mgr.open(pack, &key(path), Some(bytes)).unwrap();
        let ada = s
            .join(
                Participant::new("ada", "Ada", "#2f9e8f", ParticipantKind::Person, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn;
        Live { s, ada }
    }
    fn blob(&self, bytes: &[u8]) -> Arc<BlobSession> {
        let s = self
            .mgr
            .open_blob(
                Arc::new(OutlineIndex),
                &key("b.outline"),
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

impl Live {
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
    fn edit(&self, edits: Vec<Edit>) {
        let mut opts = EditOptions::new();
        opts.base = Some(self.version());
        self.s.edit_ops(self.ada, edits, &opts).unwrap();
    }
    fn index(&self) -> ViewIndex {
        self.s.view_index(self.ada).unwrap().unwrap()
    }
    fn watch(&self) -> (KeyedWatch, Vec<WatchFrame>) {
        let mut w = self
            .s
            .watch_keyed(self.ada, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
            .unwrap()
            .unwrap();
        let frames = self.s.watch_snapshot(&mut w).unwrap();
        (w, frames)
    }
}

fn set_op(root: &str, key: &str, value: &str) -> Edit {
    Edit::Map {
        root: root.into(),
        ops: vec![MapOp::Set {
            key: key.into(),
            value: Value::str(value),
        }],
    }
}

fn delete_op(root: &str, key: &str) -> Edit {
    Edit::Map {
        root: root.into(),
        ops: vec![MapOp::Delete { key: key.into() }],
    }
}

/// The live index of `bytes` in a fresh doc, its parents from the field.
fn live_of(tag: &str, bytes: &[u8]) -> Result<ViewIndex, String> {
    let f = fx(tag);
    let l = f.live(bytes, false);
    l.s.view_index(l.ada)
        .map_err(|e| format!("live: {e}"))
        .and_then(|i| i.ok_or_else(|| "live: no view index".to_owned()))
}

/// The blob index of `bytes` in a fresh doc.
fn blob_of(tag: &str, bytes: &[u8]) -> Result<ViewIndex, String> {
    let f = fx(tag);
    let b = f.blob(bytes);
    let bc = b
        .join(
            Participant::new("ada", "ada", "#123456", ParticipantKind::Person, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn;
    b.view_index(bc, None)
        .map_err(|e| format!("blob: {e}"))
        .and_then(|i| i.ok_or_else(|| "blob: no view index".to_owned()))
}

/// The live and the blob index of `bytes`, each in a fresh doc.
fn both_of(tag: &str, bytes: &[u8]) -> (Result<ViewIndex, String>, Result<ViewIndex, String>) {
    (live_of(tag, bytes), blob_of(tag, bytes))
}

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

/// Each container's parent, by address, in the containers' order.
fn parents(ix: &ViewIndex) -> Vec<(String, Option<String>)> {
    ix.containers
        .iter()
        .map(|c| (c.key.clone(), c.parent.clone()))
        .collect()
}

fn want_parents(pairs: &[(u32, Option<u32>)]) -> Vec<(String, Option<String>)> {
    pairs
        .iter()
        .map(|(c, p)| (caddr(*c), p.map(caddr)))
        .collect()
}

/// The sample: a scene (1) holding props (2) holding chairs (3), and cameras (5); lights (4) at
/// the top level; an object in each but the props.
const SAMPLE: &str = "OUTLINE
C|1|Scene|-
C|2|Props|1
C|3|Chairs|2
C|4|Lights|-
C|5|Cameras|1
O|1|1|Floor
O|3|2|Chair
O|4|3|Lamp
O|5|4|Camera
";

/// The sample's tree.
const SAMPLE_TREE: &[(u32, Option<u32>)] = &[
    (1, None),
    (2, Some(1)),
    (3, Some(2)),
    (4, None),
    (5, Some(1)),
];

/// The sample with collection `n`'s parent cell `cell`.
fn with_parent(n: u32, cell: &str) -> String {
    SAMPLE
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            match f[..] {
                ["C", c, name, _] if c == n.to_string() => format!("C|{c}|{name}|{cell}\n"),
                _ => format!("{l}\n"),
            }
        })
        .collect()
}

/// A chain of `n` collections, each inside the one before it.
fn chain(n: u32) -> String {
    let mut s = String::from("OUTLINE\n");
    for i in 1..=n {
        let p = if i == 1 {
            "-".to_owned()
        } else {
            (i - 1).to_string()
        };
        s.push_str(&format!("C|{i}|Level {i}|{p}\n"));
    }
    s
}

// ----- the rows -----

/// The containers' tree from both sources: each container's parent by address (a field's key
/// mapped through the pack's addresses on the live tier, a node key on the blob tier), the same
/// on both, the flat order and the units unchanged, nothing refused; and the same tree when the
/// pack's derive hook sets the parents.
fn row_the_tree_comes_from_both_tiers() -> Result<(), String> {
    const ROW: &str = "row_the_tree_comes_from_both_tiers";
    view(false)
        .check(&SCHEMA, OutlinePack { derived: false }.composite())
        .map_err(|e| format!("{ROW}: control: the schema is refused: {e}"))?;
    let (live, blob) = both_of("tree", SAMPLE.as_bytes());
    let (live, blob) = (live?, blob?);
    if live.tier != Tier::Live || blob.tier != Tier::Blob || live.containers.len() != 5 {
        return Err(format!(
            "{ROW}: control: {:?} / {:?}, {} containers",
            live.tier,
            blob.tier,
            live.containers.len()
        ));
    }
    let want = want_parents(SAMPLE_TREE);
    for (tier, ix) in [("live", &live), ("blob", &blob)] {
        if parents(ix) != want {
            return Err(format!(
                "{ROW}: {tier}: the parents are {:?}, want {want:?}",
                parents(ix)
            ));
        }
        let pos: Vec<u32> = ix.containers.iter().map(|c| c.position).collect();
        if pos != [1, 2, 3, 4, 5] {
            return Err(format!("{ROW}: {tier}: the flat order is {pos:?}"));
        }
        if !ix.refusals.is_empty() {
            return Err(format!("{ROW}: {tier}: refused {:?}", ix.refusals));
        }
    }
    let strip = |ix: &ViewIndex| ViewIndex {
        tier: Tier::Live,
        rev: None,
        version: None,
        ..ix.clone()
    };
    if strip(&live) != strip(&blob) {
        return Err(format!(
            "{ROW}: the tiers differ:\nlive {:?}\nblob {:?}",
            live.containers, blob.containers
        ));
    }
    // the pack's derive hook: the same tree
    let f = fx("tree-derived");
    let d = f.live(SAMPLE.as_bytes(), true);
    let got = parents(&d.index());
    if got != want {
        return Err(format!(
            "{ROW}: derived: the parents are {got:?}, want {want:?}"
        ));
    }
    Ok(())
}

#[test]
fn the_tree_comes_from_both_tiers() {
    row_the_tree_comes_from_both_tiers().unwrap();
}

/// A parent the index can't carry fails a test build loudly, naming the container, the value
/// and why, on each tier where it can happen: one naming no container, the container itself, a
/// live value that isn't a key, a cycle (named at its first container), a container deeper than
/// `CONTAINER_DEPTH_MAX`; and a good parent made bad on a live doc that keeps its index.
#[allow(clippy::type_complexity)]
fn row_a_bad_parent_fails_a_test_build() -> Result<(), String> {
    const ROW: &str = "row_a_bad_parent_fails_a_test_build";
    let msg = |c: &str, v: &str, why: &str| {
        format!("{c}: the pack's parent {v:?} isn't one the index carries ({why})")
    };
    let deep = CONTAINER_DEPTH_MAX as u32 + 1;
    // (case, file, the tiers it is checked on with each one's message)
    let cases: Vec<(&str, String, Vec<(&str, String)>)> = vec![
        (
            "no such container",
            with_parent(4, "9"),
            vec![
                ("live", msg("@coll:4", "@coll:9", "no_such_container")),
                ("blob", msg("@coll:4", "@coll:9", "no_such_container")),
            ],
        ),
        (
            "its own",
            with_parent(4, "4"),
            vec![
                ("live", msg("@coll:4", "@coll:4", "own_parent")),
                ("blob", msg("@coll:4", "@coll:4", "own_parent")),
            ],
        ),
        (
            "not a key",
            with_parent(4, "=Bad Name"),
            vec![
                ("live", msg("@coll:4", "Bad Name", "not_a_key")),
                ("blob", msg("@coll:4", "Bad Name", "no_such_container")),
            ],
        ),
        (
            "a cycle",
            with_parent(2, "3"),
            vec![
                ("live", msg("@coll:2", "@coll:3", "cycle")),
                ("blob", msg("@coll:2", "@coll:3", "cycle")),
            ],
        ),
        (
            "too deep",
            chain(deep + 1),
            vec![
                ("live", msg(&caddr(deep), &caddr(deep - 1), "too_deep")),
                ("blob", msg(&caddr(deep), &caddr(deep - 1), "too_deep")),
            ],
        ),
    ];
    for (case, file, tiers) in cases {
        for (tier, want) in tiers {
            let got = unwound_with(|| match tier {
                "live" => live_of("bad-parent", file.as_bytes()).map(|_| ()),
                _ => blob_of("bad-parent", file.as_bytes()).map(|_| ()),
            });
            match got {
                None => {
                    return Err(format!(
                        "{ROW}: {case}: {tier}: the parent built and nothing refused it"
                    ))
                }
                Some(e) if !e.contains(&want) => {
                    return Err(format!(
                        "{ROW}: {case}: {tier}: refused, but not as the kit says: {e}\nwant {want}"
                    ))
                }
                Some(_) => {}
            }
        }
    }
    // the deepest a container may sit builds: a chain of exactly CONTAINER_DEPTH_MAX
    let (live, blob) = both_of("deep-ok", chain(deep - 1).as_bytes());
    for (tier, ix) in [("live", live), ("blob", blob)] {
        let ix = ix.map_err(|e| format!("{ROW}: at the limit: {tier}: {e}"))?;
        let last = ix.containers.last().and_then(|c| c.parent.clone());
        if last != Some(caddr(deep - 2)) {
            return Err(format!(
                "{ROW}: at the limit: {tier}: the deepest container's parent is {last:?}"
            ));
        }
    }
    // a change too: a good parent made bad on a live doc that keeps its index
    let f = fx("bad-parent-change");
    let l = f.live(SAMPLE.as_bytes(), false);
    l.index();
    let got = unwound_with(|| {
        l.edit(vec![set_op(COLL_F, &prop(&ckey(5), "parent"), "c9")]);
        l.s.view_index(l.ada)
    });
    let want = msg("@coll:5", "@coll:9", "no_such_container");
    match got {
        None => Err(format!(
            "{ROW}: a change: the parent built and nothing refused it"
        )),
        Some(e) if !e.contains(&want) => Err(format!(
            "{ROW}: a change: refused, but not as the kit says: {e}\nwant {want}"
        )),
        Some(_) => Ok(()),
    }
}

#[test]
fn a_bad_parent_fails_a_test_build() {
    row_a_bad_parent_fails_a_test_build().unwrap();
}

/// `row` passes as built and fails under `mutant` with an error starting `starts`.
fn caught(row: fn() -> Result<(), String>, mutant: ViewMutant, starts: &str) {
    row().expect("the row passes as built");
    let e = with_view_mutant(mutant, row).expect_err("the row passes with the fault planted");
    assert!(e.starts_with(starts), "{mutant:?}: {e}");
}

#[test]
fn plant_parent_unchecked_is_caught() {
    caught(
        row_a_bad_parent_fails_a_test_build,
        ViewMutant::ParentUnchecked,
        "row_a_bad_parent_fails_a_test_build: no such container: live: the parent built and \
         nothing refused it",
    );
}

#[test]
fn plant_parent_not_validated_is_caught() {
    caught(
        row_a_bad_parent_fails_a_test_build,
        ViewMutant::ParentNotValidated,
        "row_a_bad_parent_fails_a_test_build: no such container: live: the parent built and \
         nothing refused it",
    );
}

#[test]
fn plant_parent_cycle_accepted_is_caught() {
    caught(
        row_a_bad_parent_fails_a_test_build,
        ViewMutant::ParentCycleAccepted,
        "row_a_bad_parent_fails_a_test_build: a cycle: live: the parent built and nothing \
         refused it",
    );
}

#[test]
fn plant_parent_depth_unbounded_is_caught() {
    caught(
        row_a_bad_parent_fails_a_test_build,
        ViewMutant::ParentDepthUnbounded,
        "row_a_bad_parent_fails_a_test_build: too deep: live: the parent built and nothing \
         refused it",
    );
}

/// As a release build reads it (the test build's failure off): a bad parent is left out (its
/// container is at the top level, in its place in the flat order) and recorded, the rest of the
/// tree stands; a cycle is broken at its first container, the others keep their parents; a
/// container one level too deep is at the top level and the one inside it keeps its parent; a
/// doc that keeps its index through changes refuses what a whole build does, and takes a good
/// parent back.
#[test]
#[allow(clippy::type_complexity)]
fn a_bad_parent_is_not_carried_and_is_recorded_where_unchecked() {
    let refused = |c: &str, v: &str, why| ViewRefusal::Parent {
        container: c.into(),
        value: v.into(),
        why,
    };
    let tree_with = |n: u32, p: Option<u32>| -> Vec<(String, Option<String>)> {
        let mut t = want_parents(SAMPLE_TREE);
        t[(n - 1) as usize].1 = p.map(caddr);
        t
    };
    let deep = CONTAINER_DEPTH_MAX as u32 + 1;
    let cases: Vec<(String, &str, Vec<(String, Option<String>)>, ViewRefusal)> = vec![
        (
            with_parent(4, "9"),
            "both",
            tree_with(4, None),
            refused("@coll:4", "@coll:9", ParentRefused::NoSuchContainer),
        ),
        (
            with_parent(4, "4"),
            "both",
            tree_with(4, None),
            refused("@coll:4", "@coll:4", ParentRefused::OwnParent),
        ),
        (
            with_parent(4, "=Bad Name"),
            "live",
            tree_with(4, None),
            refused("@coll:4", "Bad Name", ParentRefused::NotAKey),
        ),
        (
            with_parent(4, "=Bad Name"),
            "blob",
            tree_with(4, None),
            refused("@coll:4", "Bad Name", ParentRefused::NoSuchContainer),
        ),
        // 2 in 3 and 3 in 2: broken at 2, 3 keeps 2
        (
            with_parent(2, "3"),
            "both",
            tree_with(2, None),
            refused("@coll:2", "@coll:3", ParentRefused::Cycle),
        ),
    ];
    for (file, tiers, want, why) in cases {
        let (live, blob) = with_view_mutant(ViewMutant::ParentUnchecked, || {
            both_of("unchecked", file.as_bytes())
        });
        for (tier, ix) in [("live", live), ("blob", blob)] {
            if tiers != "both" && tiers != tier {
                continue;
            }
            let ix = ix.unwrap();
            assert_eq!(parents(&ix), want, "{tier}, {why:?}");
            assert_eq!(ix.refusals, vec![why.clone()], "{tier}");
            let pos: Vec<u32> = ix.containers.iter().map(|c| c.position).collect();
            assert_eq!(pos, [1, 2, 3, 4, 5], "{tier}: the flat order stays");
        }
    }
    // one level too deep: at the top level, and the one inside it keeps its parent (level 2)
    let (live, blob) = with_view_mutant(ViewMutant::ParentUnchecked, || {
        both_of("unchecked-deep", chain(deep + 1).as_bytes())
    });
    for (tier, ix) in [("live", live.unwrap()), ("blob", blob.unwrap())] {
        let at = |n: u32| ix.containers[(n - 1) as usize].parent.clone();
        assert_eq!(at(deep), None, "{tier}: the container too deep");
        assert_eq!(at(deep + 1), Some(caddr(deep)), "{tier}: the one inside it");
        assert_eq!(
            at(deep - 1),
            Some(caddr(deep - 2)),
            "{tier}: the one above it"
        );
        assert_eq!(
            ix.refusals,
            vec![refused(
                &caddr(deep),
                &caddr(deep - 1),
                ParentRefused::TooDeep
            )],
            "{tier}"
        );
    }
    // kept through changes: a parent made bad, then good again
    let f = fx("unchecked-change");
    let l = f.live(SAMPLE.as_bytes(), false);
    l.index();
    with_view_mutant(ViewMutant::ParentUnchecked, || {
        l.edit(vec![set_op(COLL_F, &prop(&ckey(5), "parent"), "c9")]);
        let ix = l.index();
        assert_eq!(parents(&ix), tree_with(5, None), "made bad");
        assert_eq!(
            ix.refusals,
            vec![refused(
                "@coll:5",
                "@coll:9",
                ParentRefused::NoSuchContainer
            )]
        );
        // another container's change keeps the refusal (checked again, not rebuilt)
        l.edit(vec![set_op(COLL_F, &prop(&ckey(1), "name"), "World")]);
        let ix = l.index();
        assert_eq!(
            ix.refusals,
            vec![refused(
                "@coll:5",
                "@coll:9",
                ParentRefused::NoSuchContainer
            )],
            "kept"
        );
        // the container it names comes: the parent is good now, with no change to 5
        l.edit(vec![
            Edit::List {
                root: COLLS.into(),
                ops: vec![ListOp::Insert {
                    key: ckey(9),
                    at: Anchor::End,
                    value: Value::str("coll"),
                }],
            },
            set_op(COLL_F, &prop(&ckey(9), "name"), "Late"),
        ]);
        let ix = l.index();
        assert!(ix.refusals.is_empty(), "{:?}", ix.refusals);
        assert_eq!(
            ix.containers[4].parent.as_deref(),
            Some("@coll:9"),
            "taken once good"
        );
    });
}

/// The snapshot carries a container's `parent` only when it has one (a top-level container
/// has no `parent` key at all, as before), and the schema's container fields carry `parent`
/// only when the pack declares one.
fn row_the_snapshot_carries_parent_only_when_set() -> Result<(), String> {
    const ROW: &str = "row_the_snapshot_carries_parent_only_when_set";
    let f = fx("wire");
    let l = f.live(SAMPLE.as_bytes(), false);
    let (_w, frames) = l.watch();
    let head = frames
        .first()
        .map(WatchFrame::to_value)
        .ok_or(format!("{ROW}: control: no snapshot"))?;
    let vjson = head
        .get("view")
        .ok_or(format!("{ROW}: control: no view in the head"))?;
    let Some(Json::List(cs)) = vjson.get("containers") else {
        return Err(format!(
            "{ROW}: control: no containers in {}",
            vjson.to_json()
        ));
    };
    if cs.len() != 5 {
        return Err(format!("{ROW}: control: {} containers", cs.len()));
    }
    for (c, (key, p)) in cs.iter().zip(want_parents(SAMPLE_TREE)) {
        let got = c.get("parent");
        match (&p, got) {
            (Some(p), Some(Json::Str(g))) if g == p => {}
            (None, None) => {}
            _ => {
                return Err(format!(
                    "{ROW}: {key}: the snapshot's container is {}, want parent {p:?}",
                    c.to_json()
                ))
            }
        }
    }
    let fields = |vs: &ViewSchema| {
        vs.to_value()
            .get("container_fields")
            .map(Json::to_json)
            .unwrap_or_default()
    };
    let declared = fields(&view(false));
    if declared != r#"{"title":"name","hidden":null,"layout":null,"parent":"parent"}"# {
        return Err(format!("{ROW}: a schema with parents: {declared}"));
    }
    let none = fields(&ViewSchema {
        container_fields: ContainerFields {
            parent: Src::None,
            ..view(false).container_fields
        },
        ..view(false)
    });
    if none != r#"{"title":"name","hidden":null,"layout":null}"# {
        return Err(format!("{ROW}: a schema without: {none}"));
    }
    Ok(())
}

#[test]
fn the_snapshot_carries_parent_only_when_set() {
    row_the_snapshot_carries_parent_only_when_set().unwrap();
}

#[test]
fn plant_parent_dropped_on_wire_is_caught() {
    caught(
        row_the_snapshot_carries_parent_only_when_set,
        ViewMutant::ParentDroppedOnWire,
        "row_the_snapshot_carries_parent_only_when_set: @coll:2: the snapshot's container is",
    );
}

/// The change frames among `frames`, as JSON.
fn changes(frames: &[WatchFrame]) -> Vec<Json> {
    frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Change(_)))
        .map(WatchFrame::to_value)
        .collect()
}

/// Every `parent_after` a change frame's container deltas carry, by container, in order.
fn parents_after(frame: &Json) -> Vec<(String, Option<String>)> {
    let Some(Json::List(cs)) = frame.get("containers_changed") else {
        return Vec::new();
    };
    cs.iter()
        .filter_map(|d| {
            let key = d.get("key")?.as_str()?.to_owned();
            match d.get("parent_after")? {
                Json::Str(s) => Some((key, Some(s.clone()))),
                Json::Null => Some((key, None)),
                other => Some((key, Some(format!("not a string: {}", other.to_json())))),
            }
        })
        .collect()
}

/// One change's frame against the index before and after it: every container whose parent
/// differs (or that came) has a delta carrying `parent_after` with its parent after, and every
/// `parent_after` the frame carries is the parent after.
fn frame_fits(
    step: &str,
    frame: &Json,
    before: &ViewIndex,
    after: &ViewIndex,
) -> Result<(), String> {
    let carried = parents_after(frame);
    for (key, now) in parents(after) {
        let was = before.container(&key).map(|c| c.parent.clone());
        if was.as_ref() == Some(&now) {
            continue;
        }
        if !carried.iter().any(|(k, p)| *k == key && *p == now) {
            return Err(format!(
                "{step}: {key}'s parent is now {now:?} and no container delta carries it: {}",
                frame
                    .get("containers_changed")
                    .map(Json::to_json)
                    .unwrap_or_default()
            ));
        }
    }
    for (key, p) in &carried {
        let now = after.container(key).map(|c| c.parent.clone());
        if now.as_ref() != Some(p) {
            return Err(format!(
                "{step}: {key} carries parent_after {p:?}, the index has {now:?}"
            ));
        }
    }
    Ok(())
}

/// One live doc's watch through `steps` (each an edit and its name): every change frame fits the
/// index before and after it, and names what `expect` says (a container and its parent after,
/// with its delta's op and fields), and nothing more. Returns the frames.
#[allow(clippy::type_complexity)]
fn watch_through(
    row: &str,
    derived: bool,
    steps: Vec<(&str, Vec<Edit>, Vec<(&str, Option<&str>, &str, &[&str])>)>,
) -> Result<(Vec<WatchFrame>, Vec<ViewIndex>, Live, Fx), String> {
    let f = fx(if derived { "frames-d" } else { "frames-f" });
    let l = f.live(SAMPLE.as_bytes(), derived);
    let (mut w, mut frames) = l.watch();
    let mut before = l.index();
    let mut after_each = vec![before.clone()];
    for (name, edits, expect) in steps {
        l.edit(edits);
        let fs = w.try_frames();
        let ch = changes(&fs);
        let frame = ch
            .first()
            .ok_or(format!("{row}: {name}: control: no change frame"))?;
        let after = l.index();
        frame_fits(&format!("{row}: {name}"), frame, &before, &after)?;
        let Some(Json::List(cs)) = frame.get("containers_changed") else {
            return Err(format!("{row}: {name}: control: no containers_changed"));
        };
        let got: Vec<(String, Option<String>, String, String)> = cs
            .iter()
            .filter(|d| d.get("parent_after").is_some())
            .map(|d| {
                let s = |k: &str| d.get(k).map(Json::to_json).unwrap_or_default();
                (
                    d.get("key").and_then(Json::as_str).unwrap_or("").to_owned(),
                    match d.get("parent_after") {
                        Some(Json::Str(p)) => Some(p.clone()),
                        _ => None,
                    },
                    d.get("op").and_then(Json::as_str).unwrap_or("").to_owned(),
                    s("fields"),
                )
            })
            .collect();
        let want: Vec<(String, Option<String>, String, String)> = expect
            .iter()
            .map(|(k, p, op, fields)| {
                (
                    (*k).to_owned(),
                    p.map(str::to_owned),
                    (*op).to_owned(),
                    Json::list(fields.iter().copied()).to_json(),
                )
            })
            .collect();
        if got != want {
            return Err(format!(
                "{row}: {name}: the deltas carrying parent_after are {got:?}, want {want:?}"
            ));
        }
        frames.extend(fs);
        after_each.push(after.clone());
        before = after;
    }
    if w.needs_snapshot() {
        return Err(format!("{row}: the watch was cut"));
    }
    Ok((frames, after_each, l, f))
}

fn insert_coll(n: u32, name: &str) -> Vec<Edit> {
    vec![
        Edit::List {
            root: COLLS.into(),
            ops: vec![ListOp::Insert {
                key: ckey(n),
                at: Anchor::End,
                value: Value::str("coll"),
            }],
        },
        set_op(COLL_F, &prop(&ckey(n), "name"), name),
    ]
}

/// The field mode's steps: a parent set (a named change), a container that comes inside
/// another, a parent cleared, a rename and a move that leave every parent as it was.
#[allow(clippy::type_complexity)]
fn field_steps() -> Vec<(
    &'static str,
    Vec<Edit>,
    Vec<(
        &'static str,
        Option<&'static str>,
        &'static str,
        &'static [&'static str],
    )>,
)> {
    let mut comes = insert_coll(6, "Rigs");
    comes.push(set_op(COLL_F, &prop(&ckey(6), "parent"), "c1"));
    vec![
        (
            "a parent set",
            vec![set_op(COLL_F, &prop(&ckey(3), "parent"), "c4")],
            vec![("@coll:3", Some("@coll:4"), "props", PARENT_FIELD)],
        ),
        (
            "a container comes inside another",
            comes,
            vec![("@coll:6", Some("@coll:1"), "add", NO_FIELDS)],
        ),
        (
            "a parent cleared",
            vec![delete_op(COLL_F, &prop(&ckey(2), "parent"))],
            vec![("@coll:2", None, "props", PARENT_FIELD)],
        ),
        (
            "a rename",
            vec![set_op(COLL_F, &prop(&ckey(1), "name"), "World")],
            vec![],
        ),
        (
            "a move",
            vec![Edit::List {
                root: COLLS.into(),
                ops: vec![ListOp::Move {
                    key: ckey(4),
                    to: Anchor::Start,
                }],
            }],
            vec![],
        ),
    ]
}

/// The derived mode's steps: parents the pack reads from the tree map, so a change to it touches
/// no container: each container whose parent changed gets a `props` delta naming no field.
#[allow(clippy::type_complexity)]
fn derived_steps() -> Vec<(
    &'static str,
    Vec<Edit>,
    Vec<(
        &'static str,
        Option<&'static str>,
        &'static str,
        &'static [&'static str],
    )>,
)> {
    let mut comes = insert_coll(6, "Rigs");
    comes.push(set_op(TREE, "c6", "c1"));
    vec![
        (
            "a derived parent set",
            vec![set_op(TREE, "c3", "c4")],
            vec![("@coll:3", Some("@coll:4"), "props", NO_FIELDS)],
        ),
        (
            "a container comes inside another",
            comes,
            vec![("@coll:6", Some("@coll:1"), "add", NO_FIELDS)],
        ),
        (
            "a derived parent cleared",
            vec![delete_op(TREE, "c2")],
            vec![("@coll:2", None, "props", NO_FIELDS)],
        ),
        (
            "a rename",
            vec![set_op(COLL_F, &prop(&ckey(1), "name"), "World")],
            vec![],
        ),
    ]
}

/// A keyed watch's change frames carry each changed parent as `parent_after`: a parent set by
/// its field (a `props` delta naming it), a container that comes inside another (its `add`), a
/// parent cleared (`null`), nothing for a change that moves no parent; and a parent the pack
/// derives from another root, which no op on the container changed (a `props` delta naming no
/// field). The watch is never cut.
fn row_change_frames_carry_parent_after() -> Result<(), String> {
    const ROW: &str = "row_change_frames_carry_parent_after";
    watch_through(ROW, false, field_steps())?;
    watch_through(ROW, true, derived_steps())?;
    Ok(())
}

#[test]
fn change_frames_carry_parent_after() {
    row_change_frames_carry_parent_after().unwrap();
}

#[test]
fn plant_frame_misses_parent_is_caught() {
    caught(
        row_change_frames_carry_parent_after,
        ViewMutant::FrameMissesParent,
        "row_change_frames_carry_parent_after: a parent set: @coll:3's parent is now \
         Some(\"@coll:4\") and no container delta carries it",
    );
}

// ----- the client's golden -----

fn golden(name: &str, got: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test")
        .join(name);
    if std::env::var_os("MPC_BLESS").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the kit computes now (MPC_BLESS=1 regenerates it)\n--- now:\n{got}",
        path.display()
    );
}

/// Each container's parent as JSON: `{address: parent or null}`, in order.
fn parents_json(ix: &ViewIndex) -> Json {
    Json::Obj(
        ix.containers
            .iter()
            .map(|c: &Container| (c.key.clone(), Json::from(c.parent.as_ref())))
            .collect(),
    )
}

/// One mode's run for the client: its frames (a change frame's `client` written as 1), the
/// doc's state and version at the end, and every container's parent after the snapshot and
/// after each change frame.
fn golden_run(derived: bool) -> Json {
    const ROW: &str = "golden";
    let steps = if derived {
        derived_steps()
    } else {
        field_steps()
    };
    let (frames, after_each, l, _f) =
        watch_through(ROW, derived, steps).unwrap_or_else(|e| panic!("{e}"));
    let frames: Vec<Json> = frames
        .iter()
        .map(|fr| match fr.to_value() {
            Json::Obj(m) if matches!(fr, WatchFrame::Change(_)) => Json::Obj(
                m.into_iter()
                    .map(|(k, v)| match k.as_str() {
                        "client" => (k, Json::from(1u64)),
                        _ => (k, v),
                    })
                    .collect(),
            ),
            v => v,
        })
        .collect();
    let state = l.s.keyed_state_for_tests().unwrap();
    Json::obj([
        ("frames", Json::List(frames)),
        (
            "final_json",
            Json::from(watch_state_json(&OutlinePack { derived }, &state)),
        ),
        ("final_rev", Json::from(l.version())),
        (
            "parents",
            Json::List(after_each.iter().map(parents_json).collect()),
        ),
    ])
}

/// The frames a watcher of the sample gets in each mode, and every container's parent after the
/// snapshot and after each change, for the JS client (`golden/keyed_frames_container_parent.json`;
/// `MPC_BLESS=1` rewrites it).
#[test]
fn golden_frames_with_container_parents() {
    let mut text = Json::obj([
        (
            "features",
            Json::List(vec![Json::from(KEYED_WATCH_FEATURE)]),
        ),
        ("field", golden_run(false)),
        ("derived", golden_run(true)),
    ])
    .to_json_pretty();
    text.push('\n');
    golden("golden/keyed_frames_container_parent.json", &text);
}

// ----- a container's derived values -----

/// The toy with derived values per container (a timeline track's label and kind): `label`, the
/// name in capitals (a name `Long` gives a text past `VIEW_TEXT_MAX`), and `kind`, the
/// collection's `kind` field when it has one. The hook sets them in another order than the
/// schema's, with a name the schema doesn't declare (`secret`). The file may add `K|n|kind`
/// lines.
struct ValuesPack;

const VALUES_VIEW: ViewSchema = ViewSchema {
    container_derived: &["label", "kind"],
    ..view(false)
};

impl ValuesPack {
    fn derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        let ViewSource::Live(state, _) = source else {
            return;
        };
        let cf = map_root(state, COLL_F);
        let field = |k: &Key, f: &str| {
            cf.and_then(|m| m.get(&prop(k, f)))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        for c in &mut index.containers {
            let Some(k) = OutlinePack { derived: false }.key_of(COLLS, &c.key) else {
                continue;
            };
            c.derived.clear();
            c.set_derived("secret", "not for the wire");
            if let Some(kind) = field(&k, "kind") {
                c.set_derived("kind", kind);
            }
            let name = field(&k, "name").unwrap_or_default();
            let label = match name.as_str() {
                "Long" => "L".repeat(VIEW_TEXT_MAX + 10),
                _ => name.to_uppercase(),
            };
            c.set_derived("label", label);
        }
    }
}

impl Pack for ValuesPack {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        OutlinePack { derived: false }.composite()
    }
    /// The outline's roots, with each `K|n|kind` line's `kind` field.
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        let text = String::from_utf8_lossy(bytes);
        let (kinds, rest): (Vec<&str>, Vec<&str>) = text.lines().partition(|l| l.starts_with("K|"));
        let mut rest = rest.join("\n");
        rest.push('\n');
        let mut content = OutlinePack { derived: false }.decode(rest.as_bytes())?;
        for (root, n) in &mut content.roots {
            if let (true, Neutral::Map(m)) = (root.as_str() == COLL_F, n) {
                for l in &kinds {
                    if let ["K", c, kind] = l.split('|').collect::<Vec<_>>()[..] {
                        let c: u32 = c.parse().expect("a collection number");
                        m.insert(prop(&ckey(c), "kind"), Value::str(kind));
                    }
                }
            }
        }
        Ok(content)
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(VALUES_VIEW)
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        self.derive(source, index);
    }
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        _: &ViewTouched,
    ) -> bool {
        self.derive(source, index);
        true
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        OutlinePack { derived: false }.address_of(root, key)
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        OutlinePack { derived: false }.key_of(root, address)
    }
}

/// The sample with kinds on the scene and the cameras, and a sixth collection named `Long`.
fn values_sample() -> String {
    format!("{SAMPLE}C|6|Long|-\nK|1|scene\nK|5|camera\n")
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(n, t)| ((*n).to_owned(), (*t).to_owned()))
        .collect()
}

/// A container's derived values ride the index, the host's read and the snapshot, as the
/// schema declares them: its names only (the hook's `secret` dropped), in the schema's order
/// (`label` before `kind`, whatever order the hook set them in), a long text cut; a container
/// with none has no `derived` on the wire. The schema carries its names, and one without them
/// encodes as before.
fn row_container_derived_rides_the_index_and_the_snapshot() -> Result<(), String> {
    const ROW: &str = "row_container_derived_rides_the_index_and_the_snapshot";
    let f = fx("values");
    let l = f.live_with(
        Arc::new(ValuesPack),
        "v.outline",
        values_sample().as_bytes(),
    );
    let ix = l.index();
    let c1 = ix
        .container("@coll:1")
        .ok_or(format!("{ROW}: control: no @coll:1"))?;
    let names: Vec<&str> = c1.derived.iter().map(|(n, _)| n.as_str()).collect();
    if let Some(n) = names
        .iter()
        .find(|n| !VALUES_VIEW.container_derived.contains(n))
    {
        return Err(format!(
            "{ROW}: @coll:1 keeps a name the schema doesn't declare: {n}"
        ));
    }
    if names != ["label", "kind"] {
        return Err(format!(
            "{ROW}: @coll:1's names are in the order {names:?}, want [\"label\", \"kind\"]"
        ));
    }
    if c1.derived != pairs(&[("label", "SCENE"), ("kind", "scene")]) {
        return Err(format!("{ROW}: @coll:1's values {:?}", c1.derived));
    }
    let c2 = ix
        .container("@coll:2")
        .ok_or(format!("{ROW}: control: no @coll:2"))?;
    if c2.derived != pairs(&[("label", "PROPS")]) || c2.derived_value("kind").is_some() {
        return Err(format!("{ROW}: @coll:2 (no kind): {:?}", c2.derived));
    }
    let long = ix
        .container("@coll:6")
        .and_then(|c| c.derived_value("label"))
        .map(str::len);
    if long != Some(VIEW_TEXT_MAX) {
        return Err(format!(
            "{ROW}: a long label is {long:?} bytes, want {VIEW_TEXT_MAX}"
        ));
    }
    // the host's read by container
    let read =
        l.s.view_container(l.ada, "@coll:1")
            .map_err(|e| format!("{ROW}: control: {e}"))?
            .map(|c| c.derived);
    if read.as_ref() != Some(&c1.derived) {
        return Err(format!("{ROW}: the host's read gives {read:?}"));
    }
    // the snapshot
    let (_w, frames) = l.watch();
    let head = frames
        .first()
        .map(WatchFrame::to_value)
        .ok_or(format!("{ROW}: control: no snapshot"))?;
    let view = head
        .get("view")
        .ok_or(format!("{ROW}: control: no view in the head"))?;
    let Some(Json::List(cs)) = view.get("containers") else {
        return Err(format!("{ROW}: control: no containers"));
    };
    let wire = |key: &str| {
        cs.iter()
            .find(|c| c.get("key").and_then(Json::as_str) == Some(key))
            .and_then(|c| c.get("derived"))
            .map(Json::to_json)
    };
    let want = r#"{"label":"SCENE","kind":"scene"}"#;
    if wire("@coll:1").as_deref() != Some(want) {
        return Err(format!(
            "{ROW}: the snapshot's @coll:1 has derived {:?}, want {want}",
            wire("@coll:1")
        ));
    }
    let schema = view
        .get("schema")
        .and_then(|s| s.get("container_derived"))
        .map(Json::to_json);
    if schema.as_deref() != Some(r#"["label","kind"]"#) {
        return Err(format!(
            "{ROW}: the schema's container_derived is {schema:?}"
        ));
    }
    // a pack that declares none: no `derived` on its containers, none in its schema
    let g = fx("values-none");
    let p = g.live(SAMPLE.as_bytes(), false);
    let (_w, frames) = p.watch();
    let head = frames
        .first()
        .map(WatchFrame::to_value)
        .unwrap_or(Json::Null);
    let view = head.get("view").unwrap_or(&Json::Null);
    let Some(Json::List(cs)) = view.get("containers") else {
        return Err(format!("{ROW}: control: a pack with none: no containers"));
    };
    if let Some(c) = cs.iter().find(|c| c.get("derived").is_some()) {
        return Err(format!(
            "{ROW}: a pack with none carries some: {}",
            c.to_json()
        ));
    }
    if view
        .get("schema")
        .and_then(|s| s.get("container_derived"))
        .is_some()
    {
        return Err(format!("{ROW}: a schema with none names some"));
    }
    Ok(())
}

#[test]
fn container_derived_rides_the_index_and_the_snapshot() {
    row_container_derived_rides_the_index_and_the_snapshot().unwrap();
}

#[test]
fn plant_container_derived_undeclared_kept_is_caught() {
    caught(
        row_container_derived_rides_the_index_and_the_snapshot,
        ViewMutant::ContainerDerivedUndeclaredKept,
        "row_container_derived_rides_the_index_and_the_snapshot: @coll:1 keeps a name the schema \
         doesn't declare: secret",
    );
}

#[test]
fn plant_container_derived_order_lost_is_caught() {
    caught(
        row_container_derived_rides_the_index_and_the_snapshot,
        ViewMutant::ContainerDerivedOrderLost,
        "row_container_derived_rides_the_index_and_the_snapshot: @coll:1's names are in the order \
         [\"kind\", \"label\"]",
    );
}

#[test]
fn plant_container_derived_dropped_on_wire_is_caught() {
    caught(
        row_container_derived_rides_the_index_and_the_snapshot,
        ViewMutant::ContainerDerivedDroppedOnWire,
        "row_container_derived_rides_the_index_and_the_snapshot: the snapshot's @coll:1 has \
         derived None",
    );
}

/// The schema's container names are checked as the units' are: at most
/// `CONTAINER_DERIVED_MAX`, each a good name, none twice.
#[test]
fn the_schema_checks_its_container_names() {
    let composites = ValuesPack.composite();
    VALUES_VIEW
        .check(&SCHEMA, composites)
        .expect("the toy's names");
    let refused = |names: &'static [&'static str], says: &str| {
        let vs = ViewSchema {
            container_derived: names,
            ..VALUES_VIEW
        };
        match vs.check(&SCHEMA, composites) {
            Err(e) if e.contains(says) => {}
            other => panic!("{names:?} gave {other:?}, want an error saying {says:?}"),
        }
    };
    refused(&["label", "label"], "declared twice");
    refused(&["a b"], "isn't 1 to");
    refused(&[""], "isn't 1 to");
    let many: Vec<&'static str> = (0..=CONTAINER_DERIVED_MAX)
        .map(|i| &*Box::leak(format!("n{i}").into_boxed_str()))
        .collect();
    refused(Box::leak(many.into_boxed_slice()), "at most");
}

/// The containers' part of a change frame's `derived`: `{container: {name: text or null}}`, as
/// compact JSON (`None`: no such part).
fn frame_container_derived(frame: &Json) -> Option<String> {
    frame
        .get("derived")
        .and_then(|d| d.get("containers"))
        .map(Json::to_json)
}

/// The values steps: a rename (its label), a kind set, a kind removed, a container that comes
/// (all its values), and a parent set (no value moves).
#[allow(clippy::type_complexity)]
fn values_steps() -> Vec<(&'static str, Vec<Edit>, Option<&'static str>)> {
    let mut comes = insert_coll(7, "Rigs");
    comes.push(set_op(COLL_F, &prop(&ckey(7), "kind"), "rig"));
    vec![
        (
            "a rename",
            vec![set_op(COLL_F, &prop(&ckey(1), "name"), "World")],
            Some(r#"{"@coll:1":{"label":"WORLD"}}"#),
        ),
        (
            "a kind set",
            vec![set_op(COLL_F, &prop(&ckey(4), "kind"), "light")],
            Some(r#"{"@coll:4":{"kind":"light"}}"#),
        ),
        (
            "a kind removed",
            vec![delete_op(COLL_F, &prop(&ckey(1), "kind"))],
            Some(r#"{"@coll:1":{"kind":null}}"#),
        ),
        (
            "a container comes",
            comes,
            Some(r#"{"@coll:7":{"label":"RIGS","kind":"rig"}}"#),
        ),
        (
            "a parent set",
            vec![set_op(COLL_F, &prop(&ckey(3), "parent"), "c4")],
            None,
        ),
    ]
}

/// A keyed watch's change frame carries the containers' derived values the change set or
/// removed, under `derived.containers`, and only those; a change that moves none carries no such
/// part. Returns the frames, the doc and every container's values after the snapshot and each
/// change.
#[allow(clippy::type_complexity)]
fn values_watch(row: &str) -> Result<(Vec<WatchFrame>, Vec<Json>, Live, Fx), String> {
    let f = fx("values-frames");
    let l = f.live_with(
        Arc::new(ValuesPack),
        "v.outline",
        values_sample().as_bytes(),
    );
    let (mut w, mut frames) = l.watch();
    let values = |ix: &ViewIndex| {
        Json::Obj(
            ix.containers
                .iter()
                .map(|c| {
                    (
                        c.key.clone(),
                        Json::obj(c.derived.iter().map(|(n, t)| (n.as_str(), Json::from(t)))),
                    )
                })
                .collect(),
        )
    };
    let mut after_each = vec![values(&l.index())];
    for (name, edits, want) in values_steps() {
        l.edit(edits);
        let fs = w.try_frames();
        let ch = changes(&fs);
        let frame = ch
            .first()
            .ok_or(format!("{row}: {name}: control: no change frame"))?;
        let got = frame_container_derived(frame);
        if got.as_deref() != want {
            return Err(format!(
                "{row}: {name}: the frame's derived.containers is {got:?}, want {want:?}"
            ));
        }
        frames.extend(fs);
        after_each.push(values(&l.index()));
    }
    if w.needs_snapshot() {
        return Err(format!("{row}: the watch was cut"));
    }
    Ok((frames, after_each, l, f))
}

fn row_a_change_frame_carries_the_containers_derived_values() -> Result<(), String> {
    values_watch("row_a_change_frame_carries_the_containers_derived_values").map(|_| ())
}

#[test]
fn a_change_frame_carries_the_containers_derived_values() {
    row_a_change_frame_carries_the_containers_derived_values().unwrap();
}

#[test]
fn plant_frame_misses_container_derived_is_caught() {
    caught(
        row_a_change_frame_carries_the_containers_derived_values,
        ViewMutant::FrameMissesContainerDerived,
        "row_a_change_frame_carries_the_containers_derived_values: a rename: the frame's \
         derived.containers is None",
    );
}

/// The frames a watcher of the values sample gets, and every container's derived values after
/// the snapshot and after each change, for the JS client
/// (`golden/keyed_frames_container_derived.json`; `MPC_BLESS=1` rewrites it).
#[test]
fn golden_frames_with_container_derived_values() {
    let (frames, after_each, l, _f) = values_watch("golden").unwrap_or_else(|e| panic!("{e}"));
    let frames: Vec<Json> = frames
        .iter()
        .map(|fr| match fr.to_value() {
            Json::Obj(m) if matches!(fr, WatchFrame::Change(_)) => Json::Obj(
                m.into_iter()
                    .map(|(k, v)| match k.as_str() {
                        "client" => (k, Json::from(1u64)),
                        _ => (k, v),
                    })
                    .collect(),
            ),
            v => v,
        })
        .collect();
    let state = l.s.keyed_state_for_tests().unwrap();
    let mut text = Json::obj([
        (
            "features",
            Json::List(vec![Json::from(KEYED_WATCH_FEATURE)]),
        ),
        ("frames", Json::List(frames)),
        (
            "final_json",
            Json::from(watch_state_json(&ValuesPack, &state)),
        ),
        ("final_rev", Json::from(l.version())),
        ("derived", Json::List(after_each)),
    ])
    .to_json_pretty();
    text.push('\n');
    golden("golden/keyed_frames_container_derived.json", &text);
}
