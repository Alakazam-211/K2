//! The keyed watch (`client/WIRE.md`, "The keyed watch"; `session/live/watch.rs`): a person
//! watching a live keyed doc gets a snapshot in pages, then one numbered frame per change, and
//! nothing at all from a host that doesn't list the feature.
//!
//! - **Replay:** seeded traces of agent verbs (with turns and held claims), people's raw ops
//!   (every list and map op, text hunks), undo, redo, revert, outside saves and presence noise;
//!   a replica that applies only the frames ends at the doc's own keyed state, byte for byte
//!   (`watch_state_json`: by address, as the frames name elements). The traces are also the JS client's vectors
//!   (`client/test/vectors/keyed_watch.json`, replayed by `client/test/keyed-watch.test.ts`).
//! - **Pages:** a snapshot split under a byte cap reassembles exactly; one entry over the frame
//!   cap refuses it (`MPC-E401`).
//! - **Gaps:** a replica sees a skipped `seq`; a watch that fell behind, or a change over the
//!   frame cap, asks for a snapshot, and the replica lands on the doc's state again.
//! - **The gate:** a host that doesn't list `keyed_watch` gets no watch; a text doc has none.
//! - **Presence is never counted:** selections, pointers, focus and claims take no number.
//! - **The fields:** actor, client, turn, verb, claim, change kind, ops by key and field, and a
//!   pack's own `derived` and `render_hashes`.
//! - **Goldens:** `client/test/golden/keyed_snapshot.json` and `keyed_frames.json`
//!   (`MPC_BLESS=1` writes them, and the vectors).
//!
//! - **Roster events speak addresses:** the selection, focus, range and claim events, the
//!   setters' replies and a fresh roster name every element by its address too.
//!
//! Planted faults caught by name: `WatchMutant::SnapshotOffByOne`, `ViewMutant::RosterKeysLeak`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{with_view_mutant, with_watch_mutant, ViewMutant, WatchMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, KeyedRoot, KeyedState, ListOp, ListOpKind, MapOp, MapOpKind};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    watch_state_json, AuditEvent, ClaimId, ClaimPolicy, Config, ConnId, Declined, DiskCheck,
    EditOptions, ElemFocus, ElemRange, ElemSelection, Host, KeyedWatch, OutsideSaveOutcome, Pack,
    PackContent, PackError, Participant, ParticipantKind, Point, Reconciled, Rights, RosterEvent,
    Session, SessionError, SessionManager, Stay, TextFilePack, UndoRequest, View, WatchExtras,
    WatchFrame, WatchOp, WatchOptions, WatchOutsideSave, WatchPart, WatchRefusal,
    KEYED_WATCH_FEATURE, WATCH_FRAME_MAX, WATCH_PAGE_BYTES_MIN,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::text::diff::{self, Hunk};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, ToolOptions, VerbCtx};
use k2_mpc_base::view_index::{
    unit_value, ContainerFields, Nesting, Src, Unit, UnitFields, ViewSchema, NESTING_MAX,
    VIEW_PAGE_MIN,
};
use k2_mpc_base::{Edit, Key, Turn, Value};

// ---------- the test pack: a board with a file behind it ----------

const SHAPES: &str = "shapes";
const FIELDS: &str = "fields";
const META: &str = "meta";
const NOTES: &str = "notes";
const SEP: char = '\u{1f}';

const BOARD: Schema = Schema {
    name: "board-watch",
    roots: &[
        (SHAPES, Kind::MovableList),
        (FIELDS, Kind::Map),
        (META, Kind::Map),
        (NOTES, Kind::Text),
    ],
};

/// A board: `shapes` (a MovableList, each value the shape's kind), `fields` (a Map of
/// `<shape>\x1f<field>`, composite on `shapes`), `meta` (a plain Map) and `notes` (a text). The
/// file is one line a thing, tab-separated, values escaped (`\\`, `\t`, `\n`): `S key value`
/// (`~`: no value) for each live shape in order, `F shape field value`, `M key value`, `N text`.
/// `extras`: the pack gives each change a `derived` value and a render hash
/// ([`Pack::watch_extras`]).
struct Board {
    extras: bool,
}

#[derive(Default, Clone)]
struct Parsed {
    shapes: Vec<(Key, Value)>,
    fields: BTreeMap<String, Value>,
    meta: BTreeMap<String, Value>,
    notes: String,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn val_text(v: &Value) -> String {
    match v.as_str() {
        Some(s) => esc(s),
        None => "~".into(),
    }
}

fn parse(b: &[u8]) -> Result<Parsed, Declined> {
    let bad = |message: String| Declined {
        code: "malformed",
        message,
    };
    let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
    let mut p = Parsed::default();
    for line in s.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        match parts.as_slice() {
            ["S", k, v] => {
                let key = Key::new(*k).map_err(|e| bad(e.to_string()))?;
                let v = match *v {
                    "~" => Value::Null,
                    v => Value::str(unesc(v)),
                };
                p.shapes.push((key, v));
            }
            ["F", k, f, v] => {
                p.fields
                    .insert(format!("{k}{SEP}{f}"), Value::str(unesc(v)));
            }
            ["M", k, v] => {
                p.meta.insert((*k).to_owned(), Value::str(unesc(v)));
            }
            ["N", t] => p.notes = unesc(t),
            _ => return Err(bad(format!("bad line {line:?}"))),
        }
    }
    Ok(p)
}

fn render(p: &Parsed) -> Vec<u8> {
    let mut out = String::new();
    for (k, v) in &p.shapes {
        out.push_str(&format!("S\t{k}\t{}\n", val_text(v)));
    }
    for (k, v) in &p.fields {
        let (shape, field) = k.split_once(SEP).expect("a field key");
        out.push_str(&format!("F\t{shape}\t{field}\t{}\n", val_text(v)));
    }
    for (k, v) in &p.meta {
        out.push_str(&format!("M\t{k}\t{}\n", val_text(v)));
    }
    out.push_str(&format!("N\t{}\n", esc(&p.notes)));
    out.into_bytes()
}

impl Pack for Board {
    fn schema(&self) -> Schema {
        BOARD
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(FIELDS, SHAPES)]
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    /// A shape `a1` is `@sh:a1` on the wire.
    fn address_of(&self, root: &str, key: &Key) -> String {
        match root {
            SHAPES => format!("@sh:{key}"),
            _ => key.to_string(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        match root {
            SHAPES => Key::new(address.strip_prefix("@sh:")?).ok(),
            _ => Key::new(address).ok(),
        }
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let p = parse(b)?;
        Ok(PackContent {
            roots: vec![
                (SHAPES.into(), Neutral::List(p.shapes)),
                (FIELDS.into(), Neutral::Map(p.fields)),
                (META.into(), Neutral::Map(p.meta)),
                (NOTES.into(), Neutral::Text(p.notes)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let mut p = Parsed::default();
        for (r, n) in c {
            match (r.as_str(), n) {
                (SHAPES, Neutral::List(l)) => p.shapes = l.clone(),
                (FIELDS, Neutral::Map(m)) => p.fields = m.clone(),
                (META, Neutral::Map(m)) => p.meta = m.clone(),
                (NOTES, Neutral::Text(t)) => p.notes = t.clone(),
                _ => return Err(PackError(format!("unexpected root {r}"))),
            }
        }
        Ok(render(&p))
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let p = parse(bytes)?;
        let old: &[(Key, Value)] = synced.list(SHAPES).unwrap_or(&[]);
        let hidden = synced.hidden(SHAPES);
        let new_keys: Vec<&Key> = p.shapes.iter().map(|(k, _)| k).collect();
        let mut list = Vec::new();
        for (k, _) in old.iter().filter(|(k, _)| !new_keys.contains(&k)) {
            list.push(ListOp::Delete { key: k.clone() });
        }
        let mut prev: Option<Key> = None;
        for (k, v) in &p.shapes {
            match old.iter().find(|(o, _)| o == k) {
                Some((_, ov)) => {
                    if ov != v {
                        list.push(ListOp::Set {
                            key: k.clone(),
                            value: v.clone(),
                        });
                    }
                }
                None if hidden.is_some_and(|h| h.contains_key(k)) => {
                    list.push(ListOp::Restore { key: k.clone() });
                    list.push(ListOp::Set {
                        key: k.clone(),
                        value: v.clone(),
                    });
                }
                None => list.push(ListOp::Insert {
                    key: k.clone(),
                    at: prev.clone().map_or(Anchor::Start, Anchor::After),
                    value: v.clone(),
                }),
            }
            prev = Some(k.clone());
        }
        let empty = BTreeMap::new();
        let map_ops = |old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>| {
            let mut ops = Vec::new();
            for (k, v) in new {
                if old.get(k) != Some(v) {
                    ops.push(MapOp::Set {
                        key: k.clone(),
                        value: v.clone(),
                    });
                }
            }
            for k in old.keys().filter(|k| !new.contains_key(*k)) {
                ops.push(MapOp::Delete { key: k.clone() });
            }
            ops
        };
        let fields = map_ops(synced.map(FIELDS).unwrap_or(&empty), &p.fields);
        let meta = map_ops(synced.map(META).unwrap_or(&empty), &p.meta);
        let notes = diff::hunks(synced.text(NOTES).unwrap_or(""), &p.notes);
        let mut edits = Vec::new();
        if !list.is_empty() {
            edits.push(Edit::List {
                root: SHAPES.into(),
                ops: list,
            });
        }
        if !fields.is_empty() {
            edits.push(Edit::Map {
                root: FIELDS.into(),
                ops: fields,
            });
        }
        if !meta.is_empty() {
            edits.push(Edit::Map {
                root: META.into(),
                ops: meta,
            });
        }
        if !notes.is_empty() {
            edits.push(Edit::Text {
                root: NOTES.into(),
                hunks: notes,
            });
        }
        Ok(Reconciled {
            edits,
            codec: vec![1],
            ..Reconciled::default()
        })
    }
    fn watch_extras(
        &self,
        ops: &[k2_mpc_base::elem::Applied],
        state: &KeyedState,
        _: Option<k2_mpc_base::view_index::ViewChange<'_>>,
    ) -> WatchExtras {
        if !self.extras {
            return WatchExtras::default();
        }
        let live = state
            .roots
            .iter()
            .find_map(|(r, k)| match k {
                KeyedRoot::List { live, .. } if r == SHAPES => Some(live.len()),
                _ => None,
            })
            .unwrap_or(0);
        WatchExtras::new()
            .derived(Json::obj([
                ("roots", Json::from(ops.len())),
                ("live", Json::from(live)),
            ]))
            .render_hash("board", format!("rh:{live}"))
    }
}

const S: &[PersonAction] = &[PersonAction::Format];

const VERBS: &[PackVerb] = &[
    PackVerb::structured_edit("shape move", "shape move <shape>", S, &[0, 2], shape_move)
        .positions(&[]),
    PackVerb::structured_edit(
        "shape add",
        "shape add <key> <after>",
        S,
        &[0, 2],
        shape_add,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape text",
        "shape text <shape> <text>",
        S,
        &[0, 2],
        shape_text,
    )
    .positions(&[]),
];

fn key_arg(a: Option<&String>) -> Result<Key, ToolError> {
    let a = a.ok_or_else(|| ToolError::usage("name a shape"))?;
    Key::new(a).map_err(|_| ToolError::usage(format!("{a:?} is not a key")))
}

/// `shape move <shape>`: its `geom` goes one step on.
fn shape_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let k = key_arg(args.first())?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let at = format!("{k}{SEP}geom");
        let n = match ctx.view().map(FIELDS).and_then(|m| m.get(&at)) {
            Some(Value::Str(s)) => s.parse::<u64>().unwrap_or(0),
            _ => 0,
        };
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: at,
                    value: Value::Str((n + 1).to_string()),
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape add <key> <after>`: a new shape after `after` (`start`: first), with `geom` 0: two
/// roots in one change.
fn shape_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let k = key_arg(args.first())?;
    let at = match args.get(1).map(String::as_str) {
        None | Some("start") => Anchor::Start,
        Some(_) => Anchor::After(key_arg(args.get(1))?),
    };
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(
            vec![
                Edit::List {
                    root: SHAPES.into(),
                    ops: vec![ListOp::Insert {
                        key: k.clone(),
                        at: at.clone(),
                        value: Value::str("box"),
                    }],
                },
                Edit::Map {
                    root: FIELDS.into(),
                    ops: vec![MapOp::Set {
                        key: format!("{k}{SEP}geom"),
                        value: Value::str("0"),
                    }],
                },
            ],
            Vec::new(),
            0,
        ))
    }))
}

/// `shape text <shape> <text>`: its `text` field.
fn shape_text(args: &[String]) -> Result<StructEdit, ToolError> {
    let k = key_arg(args.first())?;
    let t = args.get(1).cloned().unwrap_or_default();
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key: format!("{k}{SEP}text"),
                    value: Value::str(t.clone()),
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

// ---------- the host and the fixture ----------

struct TestHost {
    now: AtomicU64,
    /// `cleo` has no rights (from when it is set: the host takes them away).
    cleo_revoked: AtomicBool,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        match who.pid.as_str() {
            "ada" => Rights::read_only(),
            "cleo" if self.cleo_revoked.load(Ordering::SeqCst) => Rights::none(),
            _ => Rights::full(),
        }
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
    /// A conflict copy is a doc of the space, by its file name.
    fn document_at(&self, file: &Path) -> Option<DocKey> {
        let name = file.file_name()?.to_str()?;
        name.contains("conflict").then(|| doc_key(name))
    }
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

const NAME: &str = "plan.board";

fn fx(tag: &str, set: impl FnOnce(&mut Config)) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-keyed-watch-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
        cleo_revoked: AtomicBool::new(false),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    set(&mut c);
    let mgr = SessionManager::new(store, host.clone(), c);
    Fx {
        _dir: TmpDir(dir),
        ws,
        host,
        mgr,
    }
}

fn doc_key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

/// The board every scenario starts from: four shapes, fields, meta and notes.
const START: &str = "S\ta1\tbox\nS\ta2\tline\nS\ta3\t~\nS\ta4\tbox\n\
F\ta1\tgeom\t0\nF\ta1\ttext\tHello\nF\ta2\tgeom\t3\nF\ta4\ttext\tcaf\\\\ \u{e9}\n\
M\ttitle\tPlan\nN\tfirst line\\nsecond \u{1F600} line\n";

impl Fx {
    fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    fn board(&self, extras: bool) -> Arc<Session> {
        if !self.path().exists() {
            fs::write(self.path(), START).unwrap();
        }
        self.mgr
            .open_file(
                Arc::new(Board { extras }),
                &doc_key(NAME),
                &self.path(),
                &self.ws,
            )
            .unwrap()
    }
    fn tick(&self) {
        self.host.now.fetch_add(250, Ordering::SeqCst);
        self.mgr.flush_rosters();
    }
}

fn person(pid: &str, name: &str) -> Participant {
    Participant::new(pid, name, "#2f9e8f", ParticipantKind::Person, "tab")
}

fn agent() -> Participant {
    Participant::new("atlas", "Atlas", "#6d5bd0", ParticipantKind::Agent, "cli")
}

fn join(s: &Session, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn turn(n: u64) -> Turn {
    Turn::new(&format!("t:9c41e07ab2d35f68.{n}")).unwrap()
}

fn listed() -> WatchOptions {
    WatchOptions::new(&[KEYED_WATCH_FEATURE])
}

/// The doc's own keyed state, as canonical JSON by address (the engine's, not the live view's).
fn doc_json(s: &Session) -> String {
    watch_state_json(
        &Board { extras: false },
        &s.keyed_state_for_tests().unwrap(),
    )
}

fn version(s: &Session) -> u64 {
    s.status().unwrap().version
}

/// An agent verb, guarded by the version now, with a turn. `None` when it changed nothing or
/// was refused.
fn verb(s: &Session, c: ConnId, words: &[String], t: Turn) -> Option<u64> {
    let mut a: Vec<String> = words.to_vec();
    a.extend(["--if-version".to_owned(), version(s).to_string()]);
    let out = s
        .run_tool_with(c, SHAPES, &a, b"", &ToolOptions::new().turn(t))
        .ok()?;
    (out.exit == 0 && out.changed.is_some()).then_some(out.version)
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|s| s.to_string()).collect()
}

/// A raw change, guarded by the version now. `None` when refused or nothing changed.
fn raw(s: &Session, c: ConnId, edits: Vec<Edit>) -> Option<u64> {
    let v = version(s);
    let r = s
        .edit_ops(c, edits, &EditOptions::new().if_version(v))
        .ok()?;
    (r.version > v).then_some(r.version)
}

// ---------- a replica that knows only the frames ----------

/// One root of a watcher's state: elements by address, as the frames name them.
enum Root {
    List {
        live: Vec<(String, Value)>,
        hidden: BTreeMap<String, Value>,
    },
    Map(BTreeMap<String, Value>),
    Text(String),
}

/// A watcher's state from frames alone, as the JS client's `KeyedWatch` keeps it: strict, so a
/// frame that doesn't fit is an error, never guessed at. `units`: the view index's units by
/// container, as the snapshot's pages gave them.
/// A snapshot being built: the state so far, pages got, pages, `upto`.
type Building = (Vec<(String, Root)>, u32, u32, u64);

#[derive(Default)]
struct Replica {
    state: Option<Vec<(String, Root)>>,
    building: Option<Building>,
    view: Option<Json>,
    units: BTreeMap<String, Vec<Unit>>,
    seq: u64,
    rev: u64,
}

fn root_mut<'a>(st: &'a mut [(String, Root)], root: &str) -> Result<&'a mut Root, String> {
    st.iter_mut()
        .find(|(r, _)| r == root)
        .map(|(_, x)| x)
        .ok_or_else(|| format!("no root {root}"))
}

fn empty_state(schema: &Schema) -> Vec<(String, Root)> {
    schema
        .roots
        .iter()
        .map(|(n, kind)| {
            let r = match kind {
                Kind::Text => Root::Text(String::new()),
                Kind::Map => Root::Map(BTreeMap::new()),
                _ => Root::List {
                    live: Vec::new(),
                    hidden: BTreeMap::new(),
                },
            };
            ((*n).to_owned(), r)
        })
        .collect()
}

fn map_key(key: &str, field: Option<&str>) -> String {
    match field {
        Some(f) => format!("{key}{SEP}{f}"),
        None => key.to_owned(),
    }
}

fn val(v: &Value) -> Json {
    v.as_str().map_or(Json::Null, Json::from)
}

/// The replica's state in `watch_state_json`'s form (written here independently).
fn state_json(st: &[(String, Root)]) -> String {
    let pair = |k: &String, v: &Value| Json::List(vec![Json::from(k.as_str()), val(v)]);
    let roots = st
        .iter()
        .map(|(name, r)| {
            let mut v = vec![("name".to_owned(), Json::from(name.as_str()))];
            match r {
                Root::Text(t) => {
                    v.push(("kind".into(), Json::from("text")));
                    v.push(("text".into(), Json::from(t.as_str())));
                }
                Root::List { live, hidden } => {
                    v.push(("kind".into(), Json::from("list")));
                    v.push((
                        "live".into(),
                        Json::List(live.iter().map(|(k, x)| pair(k, x)).collect()),
                    ));
                    v.push((
                        "hidden".into(),
                        Json::List(hidden.iter().map(|(k, x)| pair(k, x)).collect()),
                    ));
                }
                Root::Map(m) => {
                    v.push(("kind".into(), Json::from("map")));
                    v.push((
                        "entries".into(),
                        Json::List(m.iter().map(|(k, x)| pair(k, x)).collect()),
                    ));
                }
            }
            Json::Obj(v)
        })
        .collect();
    Json::obj([("roots", Json::List(roots))]).to_json()
}

fn apply_op(st: &mut [(String, Root)], o: &WatchOp) -> Result<(), String> {
    match o {
        WatchOp::Text { root, hunks } => {
            let Root::Text(t) = root_mut(st, root)? else {
                return Err(format!("{root} isn't a text"));
            };
            diff::validate(t, hunks).map_err(|e| format!("hunks on {root}: {e:?}"))?;
            *t = diff::apply(t, hunks);
        }
        WatchOp::List {
            root,
            op,
            key,
            index_before,
            index_after,
            after,
            hidden: was_hidden,
            ..
        } => {
            let Root::List { live, hidden } = root_mut(st, root)? else {
                return Err(format!("{root} isn't a list"));
            };
            let at = |live: &Vec<(String, Value)>, i: Option<usize>| {
                i.filter(|&i| live.get(i).is_some_and(|(x, _)| x == key))
                    .ok_or_else(|| format!("{op:?} {key}: not at {i:?}"))
            };
            match (op, was_hidden) {
                (ListOpKind::Insert, false) => {
                    let i = index_after
                        .filter(|&i| i <= live.len())
                        .ok_or("insert index")?;
                    if live.iter().any(|(x, _)| x == key) || hidden.contains_key(key) {
                        return Err(format!("insert of {key}, already there"));
                    }
                    live.insert(i, (key.clone(), after.clone().ok_or("insert value")?));
                }
                (ListOpKind::Move, false) => {
                    let b = at(live, *index_before)?;
                    let a = index_after
                        .filter(|&a| a < live.len())
                        .ok_or("move index")?;
                    let e = live.remove(b);
                    live.insert(a, e);
                }
                (ListOpKind::Delete, false) => {
                    let b = at(live, *index_before)?;
                    let (x, v) = live.remove(b);
                    hidden.insert(x, v);
                }
                (ListOpKind::Restore, false) => {
                    let a = index_after
                        .filter(|&a| a <= live.len())
                        .ok_or("restore index")?;
                    let v = hidden.remove(key).ok_or("restore of a key not hidden")?;
                    live.insert(a, (key.clone(), v));
                }
                (ListOpKind::Set, false) => {
                    let a = at(live, *index_after)?;
                    live[a].1 = after.clone().ok_or("set value")?;
                }
                (ListOpKind::Set, true) => {
                    let slot = hidden
                        .get_mut(key)
                        .ok_or("hidden set of a key not hidden")?;
                    *slot = after.clone().ok_or("set value")?;
                }
                (ListOpKind::Move, true) => {}
                (ListOpKind::Drop, _) => {
                    hidden.remove(key).ok_or("drop of a key not hidden")?;
                }
                _ => return Err(format!("{op:?} hidden={was_hidden} doesn't fit")),
            }
        }
        WatchOp::Map {
            root,
            op,
            key,
            field,
            after,
            ..
        } => {
            let Root::Map(m) = root_mut(st, root)? else {
                return Err(format!("{root} isn't a map"));
            };
            let mk = map_key(key, field.as_deref());
            match (op, after) {
                (MapOpKind::Set, Some(v)) => {
                    m.insert(mk, v.clone());
                }
                (MapOpKind::Delete, None) => {
                    m.remove(&mk).ok_or("delete of an absent key")?;
                }
                _ => return Err(format!("map {op:?} {mk}: bad value")),
            }
        }
        _ => return Err("an op this replica doesn't know".into()),
    }
    Ok(())
}

impl Replica {
    fn apply(&mut self, f: &WatchFrame) -> Result<(), String> {
        match f {
            WatchFrame::Snapshot(h) => {
                self.seq = h.seq;
                self.state = None;
                self.units.clear();
                self.view = h.view.as_ref().map(|(vs, v)| v.to_value(vs));
                let st = empty_state(&h.schema);
                if h.pages == 0 {
                    self.state = Some(st);
                    self.rev = h.upto;
                } else {
                    self.building = Some((st, 0, h.pages, h.upto));
                }
            }
            WatchFrame::Page(p) => {
                if p.seq != self.seq + 1 {
                    return Err(format!("gap: page seq {} after {}", p.seq, self.seq));
                }
                let Some((st, got, pages, upto)) = self.building.as_mut() else {
                    return Err("a page without a snapshot".into());
                };
                if p.page != *got + 1 || p.pages != *pages || p.upto != *upto {
                    return Err(format!("page {} of {} out of order", p.page, p.pages));
                }
                match &p.part {
                    WatchPart::Units(u) => {
                        let have = self.units.entry(u.container.clone()).or_default();
                        if u.from != have.len() {
                            return Err(format!("units of {} from {}", u.container, u.from));
                        }
                        have.extend(u.units.iter().cloned());
                    }
                    part => match (part, root_mut(st, &p.root)?) {
                        (WatchPart::Live(l), Root::List { live, .. }) => {
                            live.extend(l.iter().cloned())
                        }
                        (WatchPart::Hidden(h), Root::List { hidden, .. }) => {
                            hidden.extend(h.iter().cloned())
                        }
                        (WatchPart::Entries(e), Root::Map(m)) => {
                            for (key, field, v) in e {
                                m.insert(map_key(key, field.as_deref()), v.clone());
                            }
                        }
                        (WatchPart::Text(t), Root::Text(x)) => x.push_str(t),
                        _ => return Err(format!("page for {} doesn't fit its root", p.root)),
                    },
                }
                *got += 1;
                self.seq = p.seq;
                if got == pages {
                    let (st, _, _, upto) = self.building.take().expect("building");
                    self.state = Some(st);
                    self.rev = upto;
                }
            }
            WatchFrame::Change(c) => {
                if c.seq != self.seq + 1 {
                    return Err(format!("gap: change seq {} after {}", c.seq, self.seq));
                }
                let st = self.state.as_mut().ok_or("a change before a snapshot")?;
                if c.rev <= self.rev {
                    return Err(format!("change at rev {} at or before {}", c.rev, self.rev));
                }
                for o in &c.ops {
                    apply_op(st, o)?;
                }
                self.seq = c.seq;
                self.rev = c.rev;
            }
            // unnumbered, and they change nothing
            WatchFrame::Refused(_) | WatchFrame::OutsideSave(_) => {}
            _ => return Err("a frame this replica doesn't know".into()),
        }
        Ok(())
    }

    fn json(&self) -> String {
        state_json(self.state.as_ref().expect("a state"))
    }
}

fn replay(frames: &[WatchFrame]) -> Result<Replica, String> {
    let mut r = Replica::default();
    for f in frames {
        r.apply(f)?;
    }
    Ok(r)
}

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    match ok {
        true => Ok(()),
        false => Err(format!("{row}: {}", what())),
    }
}

/// Everything the watch has queued now, and a snapshot whenever it asks for one.
fn pump(s: &Session, w: &mut KeyedWatch, out: &mut Vec<WatchFrame>) {
    loop {
        out.extend(w.try_frames());
        if !w.needs_snapshot() {
            return;
        }
        out.extend(s.watch_snapshot(w).unwrap());
    }
}

// ---------- the traces ----------

/// xorshift64*: the traces don't depend on a crate's generator.
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
}

const BITS: [&str; 9] = [
    "a",
    "b",
    " ",
    "\n",
    "\u{e9}",
    "\u{65e5}",
    "\u{1F389}",
    "\"q\"",
    "\t\\",
];

fn bits(rng: &mut Rng, max: usize) -> String {
    (0..1 + rng.below(max))
        .map(|_| BITS[rng.below(BITS.len())])
        .collect()
}

/// The doc's live and deleted shapes now.
fn shapes_now(s: &Session) -> (Vec<Key>, Vec<Key>) {
    let st = s.keyed_state_for_tests().unwrap();
    for (r, x) in st.roots {
        if let (SHAPES, KeyedRoot::List { live, hidden, .. }) = (r.as_str(), x) {
            return (
                live.into_iter().map(|(k, _)| k).collect(),
                hidden.into_keys().collect(),
            );
        }
    }
    (Vec::new(), Vec::new())
}

fn map_now(s: &Session, root: &str) -> BTreeMap<String, Value> {
    let st = s.keyed_state_for_tests().unwrap();
    st.roots
        .into_iter()
        .find_map(|(r, x)| match x {
            KeyedRoot::Map(m) if r == root => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

fn notes_now(s: &Session) -> String {
    let st = s.keyed_state_for_tests().unwrap();
    st.roots
        .into_iter()
        .find_map(|(r, x)| match x {
            KeyedRoot::Text(t) if r == NOTES => Some(t),
            _ => None,
        })
        .unwrap_or_default()
}

/// A trace's run: the frames the watcher got, and the doc's state at the end.
struct Trace {
    frames: Vec<WatchFrame>,
    /// A snapshot taken after the last change, numbered on.
    late: Vec<WatchFrame>,
    final_json: String,
    final_rev: u64,
}

/// One seeded trace: an agent's verbs (turns, held claims), a person's raw ops, notes text,
/// undo and redo, reverts and a restore, outside saves, and presence noise; a watcher (`ada`,
/// read only) drained after every step.
fn run_trace(seed: u64, steps: usize, page_bytes: usize) -> Trace {
    let f = fx(&format!("trace{seed}"), |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let bot = join(&s, agent());
    let ada = join(&s, person("ada", "Ada"));
    // a board big enough for more than one page at the smallest cap, two shapes deleted
    let ops = (0..60)
        .map(|i| ListOp::Insert {
            key: k(&format!("b{i}")),
            at: Anchor::End,
            value: Value::str(format!("shape {i} of the board")),
        })
        .collect();
    let fields = (0..60)
        .map(|i| MapOp::Set {
            key: format!("b{i}{SEP}text"),
            value: Value::str(format!("text of b{i} \u{e9}")),
        })
        .collect();
    raw(
        &s,
        own,
        vec![
            Edit::List {
                root: SHAPES.into(),
                ops,
            },
            Edit::Map {
                root: FIELDS.into(),
                ops: fields,
            },
        ],
    )
    .expect("control: the board");
    let gone = ["b4", "b9"].map(|g| ListOp::Delete { key: k(g) }).to_vec();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: gone,
        }],
    )
    .expect("control: the deletes");
    let mut w = s
        .watch_keyed(ada, &listed().page_bytes(page_bytes))
        .unwrap()
        .expect("a watch");
    let mut frames = s.watch_snapshot(&mut w).unwrap();
    let named = s.name_version(own, "start", None).unwrap();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut minted = 0u32;
    let mut made: Vec<u64> = Vec::new();
    let mut turn_n = 10;
    let mut claimed = false;
    for _ in 0..steps {
        let (live, hidden) = shapes_now(&s);
        let pick =
            |rng: &mut Rng, v: &[Key]| (!v.is_empty()).then(|| v[rng.below(v.len())].clone());
        let r = rng.below(100);
        let done = match r {
            // an agent's verbs, some under a held claim
            0..=27 => {
                let Some(target) = pick(&mut rng, &live) else {
                    continue;
                };
                if rng.below(3) == 0 && !claimed {
                    claimed = s
                        .claim_elems(bot, SHAPES, std::slice::from_ref(&target), None, None, true)
                        .is_ok();
                }
                turn_n += rng.below(2) as u64;
                let cmd = match r % 3 {
                    0 => words(&["shape", "move", target.as_str()]),
                    1 => {
                        minted += 1;
                        words(&["shape", "add", &format!("n{minted}"), target.as_str()])
                    }
                    _ => words(&["shape", "text", target.as_str(), &bits(&mut rng, 4)]),
                };
                let v = verb(&s, bot, &cmd, turn(turn_n));
                if claimed && rng.below(3) == 0 {
                    let _ = s.release_claim(bot, None);
                    claimed = false;
                }
                v
            }
            // a person's list ops
            28..=36 => {
                minted += 1;
                let key = k(&format!("p{minted}"));
                let at = match pick(&mut rng, &live) {
                    Some(a) if rng.below(4) != 0 => Anchor::After(a),
                    _ => Anchor::Start,
                };
                raw(
                    &s,
                    own,
                    vec![
                        Edit::List {
                            root: SHAPES.into(),
                            ops: vec![ListOp::Insert {
                                key: key.clone(),
                                at,
                                value: if rng.below(3) == 0 {
                                    Value::Null
                                } else {
                                    Value::str(bits(&mut rng, 3))
                                },
                            }],
                        },
                        Edit::Map {
                            root: FIELDS.into(),
                            ops: vec![MapOp::Set {
                                key: format!("{key}{SEP}text"),
                                value: Value::str(bits(&mut rng, 5)),
                            }],
                        },
                    ],
                )
            }
            37..=41 => {
                let (Some(a), Some(b)) = (pick(&mut rng, &live), pick(&mut rng, &live)) else {
                    continue;
                };
                let to = match rng.below(3) {
                    0 => Anchor::Start,
                    1 => Anchor::End,
                    _ => Anchor::Before(b),
                };
                raw(
                    &s,
                    own,
                    vec![Edit::List {
                        root: SHAPES.into(),
                        ops: vec![ListOp::Move { key: a, to }],
                    }],
                )
            }
            42..=47 => {
                let Some(a) = pick(&mut rng, &live) else {
                    continue;
                };
                raw(
                    &s,
                    own,
                    vec![Edit::List {
                        root: SHAPES.into(),
                        ops: vec![ListOp::Delete { key: a }],
                    }],
                )
            }
            48..=51 => {
                let Some(a) = pick(&mut rng, &hidden) else {
                    continue;
                };
                raw(
                    &s,
                    own,
                    vec![Edit::List {
                        root: SHAPES.into(),
                        ops: vec![ListOp::Restore { key: a }],
                    }],
                )
            }
            52..=55 => {
                let all: Vec<Key> = live.iter().chain(&hidden).cloned().collect();
                let Some(a) = pick(&mut rng, &all) else {
                    continue;
                };
                raw(
                    &s,
                    own,
                    vec![Edit::List {
                        root: SHAPES.into(),
                        ops: vec![ListOp::Set {
                            key: a,
                            value: Value::str(bits(&mut rng, 3)),
                        }],
                    }],
                )
            }
            56..=57 => {
                let Some(a) = pick(&mut rng, &hidden) else {
                    continue;
                };
                // a deleted element's place moves: nothing shows
                let to = match pick(&mut rng, &live) {
                    Some(b) => Anchor::After(b),
                    None => Anchor::Start,
                };
                raw(
                    &s,
                    own,
                    vec![Edit::List {
                        root: SHAPES.into(),
                        ops: vec![ListOp::Move { key: a, to }],
                    }],
                )
            }
            // map ops: a field of any shape, or meta
            58..=65 => {
                let (root, key) = match rng.below(2) {
                    0 => {
                        let all: Vec<Key> = live.iter().chain(&hidden).cloned().collect();
                        let Some(a) = pick(&mut rng, &all) else {
                            continue;
                        };
                        (
                            FIELDS,
                            format!("{a}{SEP}{}", ["geom", "text", "fill"][rng.below(3)]),
                        )
                    }
                    _ => (META, ["title", "owner", "theme"][rng.below(3)].to_owned()),
                };
                let op = match rng.below(3) {
                    0 if map_now(&s, root).contains_key(&key) => MapOp::Delete { key },
                    _ => MapOp::Set {
                        key,
                        value: Value::str(bits(&mut rng, 4)),
                    },
                };
                raw(
                    &s,
                    own,
                    vec![Edit::Map {
                        root: root.into(),
                        ops: vec![op],
                    }],
                )
            }
            // the notes, by hunks at character boundaries
            66..=73 => {
                let t = notes_now(&s);
                let b: Vec<usize> = t.char_indices().map(|(i, _)| i).chain([t.len()]).collect();
                let (mut x, mut y) = (b[rng.below(b.len())], b[rng.below(b.len())]);
                if x > y {
                    std::mem::swap(&mut x, &mut y);
                }
                if rng.below(2) == 0 {
                    y = x;
                }
                let insert = if y > x && rng.below(2) == 0 {
                    String::new()
                } else {
                    bits(&mut rng, 4)
                };
                raw(
                    &s,
                    own,
                    vec![Edit::Text {
                        root: NOTES.into(),
                        hunks: vec![Hunk {
                            start: x,
                            end: y,
                            insert,
                        }],
                    }],
                )
            }
            74..=78 => {
                let _ = match rng.below(2) {
                    0 => s.undo_with(own, &UndoRequest::steps(1)),
                    _ => s.undo_with(bot, &UndoRequest::steps(1).turn(turn(turn_n))),
                };
                None
            }
            79..=81 => {
                let c = if rng.below(2) == 0 { own } else { bot };
                let _ = s.redo_with(c, &UndoRequest::steps(1));
                None
            }
            82..=84 => {
                if made.is_empty() {
                    continue;
                }
                let at = made[rng.below(made.len())];
                let _ = s.revert(own, at, false);
                None
            }
            85 => {
                let _ = s.restore(own, named);
                None
            }
            // an outside save: the file as the kit last wrote it, changed by another app
            86..=91 => {
                let _ = s.flush(own);
                let mut p = parse(&fs::read(f.path()).unwrap()).unwrap();
                match rng.below(4) {
                    0 if !p.shapes.is_empty() => {
                        let i = rng.below(p.shapes.len());
                        p.shapes.remove(i);
                    }
                    1 => {
                        minted += 1;
                        let i = rng.below(p.shapes.len() + 1);
                        p.shapes
                            .insert(i, (k(&format!("x{minted}")), Value::str("ink")));
                    }
                    2 => {
                        p.meta.insert("theme".into(), Value::str(bits(&mut rng, 3)));
                        p.notes.push_str(&bits(&mut rng, 3));
                    }
                    _ => {
                        if let Some((key, _)) = p.shapes.first() {
                            p.fields.insert(format!("{key}{SEP}geom"), Value::str("99"));
                        }
                    }
                }
                f.host.now.fetch_add(2_000, Ordering::SeqCst);
                fs::write(f.path(), render(&p)).unwrap();
                let _ = s.file_changed();
                None
            }
            // presence: never a frame, never a number
            _ => {
                if let Some(a) = pick(&mut rng, &live) {
                    let _ = s.set_selection(own, Some(ElemSelection::new(SHAPES, vec![a.clone()])));
                    let _ =
                        s.set_pointer(own, Some(Point::new(rng.below(500) as f64, 3.0).unwrap()));
                    let _ = s.set_focus(own, Some(ElemFocus::new(SHAPES, a.clone())));
                    let _ = s.claim_elems(own, SHAPES, &[a], Some(60_000), None, false);
                }
                let _ = s.release_claim(own, None);
                f.tick();
                None
            }
        };
        if let Some(v) = done {
            made.push(v);
        }
        f.host.now.fetch_add(37, Ordering::SeqCst);
        pump(&s, &mut w, &mut frames);
    }
    // a fixed tail, so every kind of change is in every trace: an undo and its redo, a revert of
    // the person's own last change, a restore of the version named at the start
    let ins = |key: &str| {
        raw(
            &s,
            own,
            vec![Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Insert {
                    key: k(key),
                    at: Anchor::Start,
                    value: Value::str(key),
                }],
            }],
        )
    };
    ins("tail1").expect("control: tail1");
    pump(&s, &mut w, &mut frames);
    s.undo_with(own, &UndoRequest::steps(1))
        .expect("control: the undo");
    s.redo_with(own, &UndoRequest::steps(1))
        .expect("control: the redo");
    pump(&s, &mut w, &mut frames);
    let v = ins("tail2").expect("control: tail2");
    s.revert(own, v, false).expect("control: the revert");
    pump(&s, &mut w, &mut frames);
    s.restore(own, named).expect("control: the restore");
    pump(&s, &mut w, &mut frames);
    let late = s.watch_snapshot(&mut w).unwrap();
    Trace {
        frames,
        late,
        final_json: doc_json(&s),
        final_rev: version(&s),
    }
}

/// The seeds of the replay row and the client's vectors: an odd seed snapshots in small pages.
const SEEDS: &[u64] = &[1, 2, 3, 4, 5, 6];
const STEPS: usize = 36;

fn trace_page_bytes(seed: u64) -> usize {
    match seed % 2 {
        1 => WATCH_PAGE_BYTES_MIN,
        _ => 64 * 1024,
    }
}

fn row_traces_replay() -> Result<(), String> {
    const ROW: &str = "row_traces_replay";
    let mut kinds = std::collections::BTreeSet::new();
    for &seed in SEEDS {
        let t = run_trace(seed, STEPS, trace_page_bytes(seed));
        for f in &t.frames {
            if let WatchFrame::Change(c) = f {
                kinds.insert(c.kind.as_str());
                if c.claim.is_some() {
                    kinds.insert("(a claim)");
                }
                if c.actor.kind == ParticipantKind::Agent && c.turn.is_some() && c.verb.is_some() {
                    kinds.insert("(an agent's verb in a turn)");
                }
            }
        }
        if seed % 2 == 1 {
            let pages = t
                .frames
                .iter()
                .filter(|f| matches!(f, WatchFrame::Page(_)))
                .count();
            need(pages > 6, ROW, || {
                format!("seed {seed}: a snapshot of {pages} pages at the smallest cap")
            })?;
        }
        let changes = t
            .frames
            .iter()
            .filter(|f| matches!(f, WatchFrame::Change(_)))
            .count();
        need(changes >= STEPS / 3, ROW, || {
            format!("seed {seed}: only {changes} change frames (the trace barely changed the doc)")
        })?;
        let r = replay(&t.frames).map_err(|e| format!("{ROW}: seed {seed}: {e}"))?;
        need(r.json() == t.final_json, ROW, || {
            format!(
                "seed {seed}: the replica isn't the doc\n  replica {}\n  doc     {}",
                r.json(),
                t.final_json
            )
        })?;
        need(r.rev == t.final_rev, ROW, || {
            format!(
                "seed {seed}: the replica is at rev {}, the doc at {}",
                r.rev, t.final_rev
            )
        })?;
        // and a replica that also takes the late snapshot lands there too
        let mut r2 = r;
        for f in &t.late {
            r2.apply(f)
                .map_err(|e| format!("{ROW}: seed {seed}: the late snapshot: {e}"))?;
        }
        need(r2.json() == t.final_json, ROW, || {
            format!("seed {seed}: the late snapshot isn't the doc")
        })?;
    }
    let want = [
        "(a claim)",
        "(an agent's verb in a turn)",
        "edit",
        "external",
        "redo",
        "restore",
        "revert",
        "undo",
    ];
    need(want.iter().all(|k| kinds.contains(k)), ROW, || {
        format!("the traces hold {kinds:?}, not all of {want:?}")
    })
}

#[test]
fn traces_replay_to_the_docs_keyed_state() {
    row_traces_replay().unwrap();
}

// ---------- pages ----------

fn row_pages_reassemble() -> Result<(), String> {
    const ROW: &str = "row_pages_reassemble";
    let f = fx("pages", |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let ada = join(&s, person("ada", "Ada"));
    // 300 shapes with fields, 40 of them deleted, a long note with every escape and width
    let mut ops = Vec::new();
    let mut fields = Vec::new();
    for i in 0..300 {
        ops.push(ListOp::Insert {
            key: k(&format!("m{i}")),
            at: Anchor::End,
            value: Value::str(format!("kind-{i}-\u{e9}")),
        });
        fields.push(MapOp::Set {
            key: format!("m{i}{SEP}text"),
            value: Value::str(format!("shape {i} \"q\" \u{1F389}")),
        });
    }
    raw(
        &s,
        own,
        vec![
            Edit::List {
                root: SHAPES.into(),
                ops,
            },
            Edit::Map {
                root: FIELDS.into(),
                ops: fields,
            },
        ],
    )
    .ok_or("control: the shapes went in")?;
    let gone: Vec<ListOp> = (0..40)
        .map(|i| ListOp::Delete {
            key: k(&format!("m{}", i * 7)),
        })
        .collect();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: gone,
        }],
    )
    .ok_or("control: the deletes")?;
    let note: String =
        (0..2_000).map(|i| BITS[i % BITS.len()]).collect::<String>() + "\u{1}\u{2028}";
    let t = notes_now(&s);
    raw(
        &s,
        own,
        vec![Edit::Text {
            root: NOTES.into(),
            hunks: vec![Hunk {
                start: t.len(),
                end: t.len(),
                insert: note,
            }],
        }],
    )
    .ok_or("control: the note")?;
    let cap = WATCH_PAGE_BYTES_MIN;
    let mut w = s
        .watch_keyed(ada, &listed().page_bytes(cap))
        .unwrap()
        .ok_or("a watch")?;
    let frames = s
        .watch_snapshot(&mut w)
        .map_err(|e| format!("{ROW}: {e}"))?;
    let pages = frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Page(_)))
        .count();
    need(pages >= 20, ROW, || {
        format!("the snapshot is in {pages} pages, not many")
    })?;
    for f in &frames {
        let n = f.to_json().len();
        need(n <= cap, ROW, || {
            format!(
                "frame {} is {n} bytes, over the {cap}-byte page cap",
                f.seq()
            )
        })?;
    }
    // every kind of part is there, and the text came in more than one run
    let texts = frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Page(p) if matches!(p.part, WatchPart::Text(_))))
        .count();
    let hidden = frames
        .iter()
        .any(|f| matches!(f, WatchFrame::Page(p) if matches!(p.part, WatchPart::Hidden(_))));
    need(texts > 1 && hidden, ROW, || {
        format!("text runs {texts}, a hidden page {hidden}")
    })?;
    let seqs: Vec<u64> = frames.iter().map(WatchFrame::seq).collect();
    need(
        seqs == (1..=frames.len() as u64).collect::<Vec<_>>(),
        ROW,
        || format!("seqs {seqs:?}"),
    )?;
    let r = replay(&frames).map_err(|e| format!("{ROW}: {e}"))?;
    need(r.json() == doc_json(&s), ROW, || {
        "the pages don't reassemble to the doc".into()
    })?;
    need(r.rev == version(&s), ROW, || {
        "the snapshot's upto isn't the doc's version".into()
    })?;
    // the default cap: one page per root
    let mut w2 = s.watch_keyed(ada, &listed()).unwrap().ok_or("a watch")?;
    let one = s
        .watch_snapshot(&mut w2)
        .map_err(|e| format!("{ROW}: {e}"))?;
    need(one.len() == 1 + 5, ROW, || {
        format!("{} frames at the default cap (head + 5 parts)", one.len())
    })?;
    // one entry over the frame cap refuses the snapshot, MPC-E401
    let mut w3 = s
        .watch_keyed(ada, &listed().page_bytes(cap).frame_max(2_048))
        .unwrap()
        .ok_or("a watch")?;
    raw(
        &s,
        own,
        vec![Edit::Map {
            root: META.into(),
            ops: vec![MapOp::Set {
                key: "big".into(),
                value: Value::str("x".repeat(3_000)),
            }],
        }],
    )
    .ok_or("control: the big value")?;
    match s.watch_snapshot(&mut w3) {
        Err(SessionError::Declined(d)) if d.code == "too_large" => {}
        other => return Err(format!("{ROW}: an entry over the frame cap: {other:?}")),
    }
    Ok(())
}

#[test]
fn a_snapshot_in_pages_reassembles_exactly() {
    row_pages_reassemble().unwrap();
}

// ---------- the snapshot's edge, and gaps ----------

/// A change queued when a snapshot is taken is in it: it never goes out after it.
fn row_snapshot_edge() -> Result<(), String> {
    const ROW: &str = "row_snapshot_edge";
    let f = fx("edge", |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let ada = join(&s, person("ada", "Ada"));
    let mut w = s.watch_keyed(ada, &listed()).unwrap().ok_or("a watch")?;
    let mut frames = s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    let t = notes_now(&s);
    let ins = |s: &Session, key: &str| {
        raw(
            s,
            own,
            vec![Edit::List {
                root: SHAPES.into(),
                ops: vec![ListOp::Insert {
                    key: k(key),
                    at: Anchor::End,
                    value: Value::Null,
                }],
            }],
        )
    };
    ins(&s, "e1").ok_or("control: e1")?;
    raw(
        &s,
        own,
        vec![Edit::Text {
            root: NOTES.into(),
            hunks: vec![Hunk {
                start: t.len(),
                end: t.len(),
                insert: "+".into(),
            }],
        }],
    )
    .ok_or("control: the note")?;
    // a new snapshot while both changes are still queued: they're in it
    frames.extend(s.watch_snapshot(&mut w).map_err(|e| e.to_string())?);
    ins(&s, "e2").ok_or("control: e2")?;
    frames.extend(w.try_frames());
    let after: Vec<u64> = frames
        .iter()
        .filter_map(|f| match f {
            WatchFrame::Change(c) => Some(c.rev),
            _ => None,
        })
        .collect();
    need(after == vec![version(&s)], ROW, || {
        format!("changes sent after the snapshot: revs {after:?}")
    })?;
    let r = replay(&frames).map_err(|e| format!("{ROW}: {e}"))?;
    need(r.json() == doc_json(&s), ROW, || {
        "the replica isn't the doc".into()
    })
}

#[test]
fn a_change_in_a_snapshot_never_goes_out_after_it() {
    row_snapshot_edge().unwrap();
}

#[test]
fn plant_snapshot_off_by_one_is_caught() {
    let got = with_watch_mutant(WatchMutant::SnapshotOffByOne, row_snapshot_edge);
    let e = got.expect_err("the plant SnapshotOffByOne isn't caught");
    assert!(
        e.starts_with("row_snapshot_edge: "),
        "caught elsewhere: {e}"
    );
}

/// A skipped number is a gap the replica sees; a watch that fell behind its queue asks for a
/// snapshot, and the frames land on the doc's state again.
fn row_gaps() -> Result<(), String> {
    const ROW: &str = "row_gaps";
    let f = fx("gaps", |c| c.subscriber_queue = 4);
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let ada = join(&s, person("ada", "Ada"));
    let mut w = s.watch_keyed(ada, &listed()).unwrap().ok_or("a watch")?;
    let mut frames = s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    let set = |s: &Session, n: usize| {
        raw(
            s,
            own,
            vec![Edit::Map {
                root: META.into(),
                ops: vec![MapOp::Set {
                    key: "n".into(),
                    value: Value::str(n.to_string()),
                }],
            }],
        )
    };
    for n in 0..3 {
        set(&s, n).ok_or("control: a set")?;
    }
    frames.extend(w.try_frames());
    // one frame lost on the way: the replica sees the gap at the next
    let mut lossy = frames.clone();
    let lost = lossy
        .iter()
        .position(|f| matches!(f, WatchFrame::Change(_)))
        .ok_or("a change")?;
    lossy.remove(lost);
    match replay(&lossy) {
        Err(e) if e.starts_with("gap: ") => {}
        other => {
            return Err(format!(
                "{ROW}: a skipped seq wasn't seen: {:?}",
                other.map(|r| r.seq)
            ))
        }
    }
    // ten more than the queue holds: cut off, then a snapshot
    for n in 3..13 {
        set(&s, n).ok_or("control: a set")?;
    }
    let before = frames.len();
    frames.extend(w.try_frames());
    need(frames.len() - before == 4, ROW, || {
        format!("{} frames from a queue of 4", frames.len() - before)
    })?;
    need(w.needs_snapshot(), ROW, || {
        "a watch past its queue doesn't ask for a snapshot".into()
    })?;
    frames.extend(s.watch_snapshot(&mut w).map_err(|e| e.to_string())?);
    set(&s, 99).ok_or("control: a set")?;
    frames.extend(w.try_frames());
    need(!w.needs_snapshot(), ROW, || {
        "still asking after the snapshot".into()
    })?;
    let r = replay(&frames).map_err(|e| format!("{ROW}: {e}"))?;
    need(r.json() == doc_json(&s), ROW, || {
        "the replica isn't the doc after the snapshot".into()
    })?;
    // a change over the frame cap isn't sent: a snapshot instead
    let mut w2 = s
        .watch_keyed(ada, &listed().page_bytes(1_024).frame_max(2_048))
        .unwrap()
        .ok_or("a watch")?;
    let mut f2 = s.watch_snapshot(&mut w2).map_err(|e| e.to_string())?;
    let t = notes_now(&s);
    raw(
        &s,
        own,
        vec![Edit::Text {
            root: NOTES.into(),
            hunks: vec![Hunk {
                start: t.len(),
                end: t.len(),
                insert: "y".repeat(3_000),
            }],
        }],
    )
    .ok_or("control: the long note")?;
    let sent = w2.try_frames();
    need(sent.is_empty() && w2.needs_snapshot(), ROW, || {
        format!(
            "a change over the cap: {} frames sent, needs a snapshot {}",
            sent.len(),
            w2.needs_snapshot()
        )
    })?;
    f2.extend(s.watch_snapshot(&mut w2).map_err(|e| e.to_string())?);
    let r2 = replay(&f2).map_err(|e| format!("{ROW}: {e}"))?;
    need(r2.json() == doc_json(&s), ROW, || {
        "the snapshot after the long change isn't the doc".into()
    })
}

#[test]
fn gaps_are_seen_and_a_snapshot_mends_them() {
    row_gaps().unwrap();
}

// ---------- the gate ----------

#[test]
fn a_host_without_the_feature_gets_no_watch() {
    let f = fx("gate", |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let ada = join(&s, person("ada", "Ada"));
    let none: [&str; 0] = [];
    for opts in [
        WatchOptions::new(&none),
        WatchOptions::new(&["presence", "claims", "focus", "range"]),
        WatchOptions::new(&["KEYED_WATCH", "keyed-watch", "keyed_watch2"]),
    ] {
        assert!(!opts.listed());
        assert!(
            s.watch_keyed(ada, &opts).unwrap().is_none(),
            "a watch without the feature: {opts:?}"
        );
    }
    assert!(s.keyed_watchable());
    // with the feature listed among others
    let mut w = s
        .watch_keyed(ada, &WatchOptions::new(&["presence", KEYED_WATCH_FEATURE]))
        .unwrap()
        .expect("listed: a watch");
    // and the change stream a host already has is the same with a watch open
    let rx = s.subscribe(own).unwrap();
    let _ = s.watch_snapshot(&mut w).unwrap();
    raw(
        &s,
        own,
        vec![Edit::Map {
            root: META.into(),
            ops: vec![MapOp::Set {
                key: "a".into(),
                value: Value::str("1"),
            }],
        }],
    )
    .unwrap();
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(
        events.len(),
        1,
        "one change event, as without a watch: {events:?}"
    );
    // a text doc has no keyed watch
    let p = f.ws.join("a.md");
    fs::write(&p, "# hi\n").unwrap();
    let md = f
        .mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &doc_key("a.md"),
            &p,
            &f.ws,
        )
        .unwrap();
    let c = join(&md, person("own", "Owen"));
    assert!(!md.keyed_watchable());
    assert!(md.watch_keyed(c, &listed()).unwrap().is_none());
}

// ---------- presence is never counted ----------

#[test]
fn presence_takes_no_number() {
    let f = fx("presence", |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let bot = join(&s, agent());
    let ada = join(&s, person("ada", "Ada"));
    let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    let mut frames = s.watch_snapshot(&mut w).unwrap();
    let head = frames.len() as u64;
    for n in 0..5u64 {
        let _ = s.set_selection(
            own,
            Some(ElemSelection::new(SHAPES, vec![k("a1"), k("a2")])),
        );
        let _ = s.set_pointer(own, Some(Point::new(n as f64, 2.0).unwrap()));
        let _ = s.set_focus(bot, Some(ElemFocus::new(SHAPES, k("a2"))));
        let c: ClaimId = s
            .claim_elems(bot, SHAPES, &[k("a4")], None, Some("busy".into()), true)
            .unwrap()
            .id;
        f.tick();
        frames.extend(w.try_frames());
        raw(
            &s,
            own,
            vec![Edit::Map {
                root: META.into(),
                ops: vec![MapOp::Set {
                    key: "n".into(),
                    value: Value::str(n.to_string()),
                }],
            }],
        )
        .unwrap();
        s.release_claim(bot, Some(c)).unwrap();
        f.tick();
        frames.extend(w.try_frames());
    }
    let seqs: Vec<u64> = frames[head as usize..]
        .iter()
        .map(WatchFrame::seq)
        .collect();
    assert_eq!(
        seqs,
        (head + 1..=head + 5).collect::<Vec<_>>(),
        "five changes, five numbers"
    );
    assert!(frames[head as usize..]
        .iter()
        .all(|f| matches!(f, WatchFrame::Change(_))));
    assert_eq!(w.seq(), head + 5);
}

// ---------- roster events speak addresses ----------

/// Every element the roster hands out is named by its address too (`Pack::address_of`: the
/// board's `a1` is `@sh:a1`): the setters' replies, the selection, focus and range events (an
/// agent's claim moves its own), the claim's start event, and a fresh roster's rows, devices
/// and claims.
fn row_roster_addresses() -> Result<(), String> {
    let f = fx("roster-addr", |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let bot = join(&s, agent());
    let ada = join(&s, person("ada", "Ada"));
    let rx = s.subscribe_roster(ada).unwrap();
    let want = |got: &[String], w: &[&str], what: &str| {
        need(got == w, "row_roster_addresses", || {
            format!("{what}: {got:?}, want {w:?}")
        })
    };
    let sel = s
        .set_selection(
            own,
            Some(ElemSelection::new(SHAPES, vec![k("a1"), k("a2")])),
        )
        .unwrap()
        .unwrap();
    want(
        &sel.addresses,
        &["@sh:a1", "@sh:a2"],
        "set_selection's reply",
    )?;
    let foc = s
        .set_focus(own, Some(ElemFocus::new(SHAPES, k("a2"))))
        .unwrap()
        .unwrap();
    want(&[foc.address], &["@sh:a2"], "set_focus's reply")?;
    let rng = s
        .set_range(own, Some(ElemRange::new(SHAPES, k("a1"), "text", 0, 2)))
        .unwrap()
        .unwrap();
    want(&[rng.address], &["@sh:a1"], "set_range's reply")?;
    let claim = s
        .claim_elems(bot, SHAPES, &[k("a4")], None, Some("busy".into()), true)
        .unwrap();
    want(&claim.addresses, &["@sh:a4"], "claim_elems' reply")?;
    f.tick();
    let mut seen = 0;
    for ev in rx.try_iter() {
        match ev {
            RosterEvent::Selection {
                selection: Some(x), ..
            } => {
                let w: Vec<String> = x.keys.iter().map(|k| format!("@sh:{k}")).collect();
                let w: Vec<&str> = w.iter().map(String::as_str).collect();
                want(&x.addresses, &w, "a selection event")?;
                seen |= 1;
            }
            RosterEvent::Focus { focus: Some(x), .. } => {
                want(
                    &[x.address],
                    &[format!("@sh:{}", x.key).as_str()],
                    "a focus event",
                )?;
                seen |= 2;
            }
            RosterEvent::Range { range: Some(x), .. } => {
                want(&[x.address], &["@sh:a1"], "a range event")?;
                seen |= 4;
            }
            RosterEvent::ClaimStartedElems { addresses, .. } => {
                want(&addresses, &["@sh:a4"], "claim_started_elems")?;
                seen |= 8;
            }
            _ => {}
        }
    }
    need(seen == 15, "row_roster_addresses", || {
        format!("control: every kind of event came ({seen:#b})")
    })?;
    for e in s.roster(ada).unwrap() {
        let rows = std::iter::once((e.selection.as_ref(), e.focus.as_ref(), e.range.as_ref()))
            .chain(
                e.devices
                    .iter()
                    .map(|d| (d.selection.as_ref(), d.focus.as_ref(), d.range.as_ref())),
            );
        for (sel, foc, rng) in rows {
            for (k, a) in sel
                .iter()
                .flat_map(|x| x.keys.iter().zip(&x.addresses))
                .chain(foc.map(|x| (&x.key, &x.address)))
                .chain(rng.map(|x| (&x.key, &x.address)))
            {
                want(
                    std::slice::from_ref(a),
                    &[format!("@sh:{k}").as_str()],
                    "a roster row",
                )?;
            }
        }
        for c in &e.claims {
            want(&c.addresses, &["@sh:a4"], "a roster claim")?;
        }
    }
    Ok(())
}

#[test]
fn roster_events_speak_addresses() {
    row_roster_addresses().unwrap();
}

#[test]
fn plant_roster_keys_leak_is_caught() {
    let e = with_view_mutant(ViewMutant::RosterKeysLeak, row_roster_addresses)
        .expect_err("plant RosterKeysLeak must fail the row");
    assert!(e.contains("row_roster_addresses"), "{e}");
}

// ---------- the fields ----------

// ---------- refused verbs ----------

fn refusal_frames(frames: &[WatchFrame]) -> Vec<&WatchRefusal> {
    frames
        .iter()
        .filter_map(|f| match f {
            WatchFrame::Refused(r) => Some(&**r),
            _ => None,
        })
        .collect()
}

/// An agent's verb under [`ClaimPolicy::Respect`], guarded by the version now, with a turn.
fn respecting(
    s: &Session,
    c: ConnId,
    words: &[String],
    t: Turn,
) -> Result<k2_mpc_base::tools::ToolOutput, k2_mpc_base::tools::ToolRunError> {
    let mut a: Vec<String> = words.to_vec();
    a.extend(["--if-version".to_owned(), version(s).to_string()]);
    let mut o = ToolOptions::new().turn(t);
    o.policy = ClaimPolicy::Respect;
    s.run_tool_with(c, SHAPES, &a, b"", &o)
}

/// Brook (an agent) holds `a2`; Atlas (another agent, claims respected) moves `a1`, then `a2`.
/// Ada (read only), an App viewer and Cleo watch; the host takes Cleo's rights between the two
/// verbs, and her watch stays open. Only the refused verb makes a refusal frame, Ada and the App
/// viewer get one each and Cleo none, and it carries who, the verb, the unit by address, the
/// code, the holder and the claim (by handle and with no turn for the App viewer).
fn row_refusals() -> Result<(), String> {
    let f = fx("refused", |_| {});
    let s = f.board(false);
    let ada = join(&s, person("ada", "Ada"));
    let cleo = join(&s, person("cleo", "Cleo"));
    let atlas = join(&s, agent());
    let brook = join(
        &s,
        Participant::new("brook", "Brook", "#c2410c", ParticipantKind::Agent, "cli"),
    );
    let app = join(
        &s,
        Participant::new("guest", "Guest", "#ffaa00", ParticipantKind::App, "app"),
    );
    let mut wa = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    let mut wc = s.watch_keyed(cleo, &listed()).unwrap().unwrap();
    let mut wg = s.watch_keyed(app, &listed()).unwrap().unwrap();
    let mut fa = s.watch_snapshot(&mut wa).unwrap();
    let mut fc = s.watch_snapshot(&mut wc).unwrap();
    let mut fg = s.watch_snapshot(&mut wg).unwrap();
    let held = s
        .claim_elems(brook, SHAPES, &[k("a2")], None, None, true)
        .unwrap()
        .id;
    // an accepted verb: a change frame, no refusal
    let ok = respecting(&s, atlas, &words(&["shape", "move", "a1"]), turn(20))
        .map_err(|e| format!("the accepted verb was refused: {e}"))?;
    need(ok.changed.is_some(), "accepted", || {
        format!("the accepted verb changed nothing: {ok:?}")
    })?;
    fa.extend(wa.try_frames());
    need(refusal_frames(&fa).is_empty(), "accepted", || {
        format!("an accepted verb made a refusal frame: {fa:?}")
    })?;
    need(change_frames(&fa).len() == 1, "accepted", || {
        format!("the accepted verb's change frame: {fa:?}")
    })?;
    // the host takes Cleo's rights; then the refused verb
    f.host.cleo_revoked.store(true, Ordering::SeqCst);
    let before = version(&s);
    let err = match respecting(&s, atlas, &words(&["shape", "move", "a2"]), turn(21)) {
        Err(e) => e,
        Ok(o) => return Err(format!("the verb on Brook's claim was not refused: {o:?}")),
    };
    need(err.exit() == 5 && version(&s) == before, "refused", || {
        format!("the claim's refusal: exit {}, {err}", err.exit())
    })?;
    fa.extend(wa.try_frames());
    fc.extend(wc.try_frames());
    let got = refusal_frames(&fa);
    need(got.len() == 1, "refused", || {
        format!(
            "the refused verb made {} refusal frames for Ada: {fa:?}",
            got.len()
        )
    })?;
    let r = got[0];
    let want_actor = (
        "atlas".to_owned(),
        "Atlas".to_owned(),
        ParticipantKind::Agent,
    );
    need(
        (r.actor.id.clone(), r.actor.name.clone(), r.actor.kind) == want_actor,
        "refused",
        || format!("who: {:?}", r.actor),
    )?;
    need(r.verb == "shape move", "refused", || {
        format!("verb: {:?}", r.verb)
    })?;
    need(r.unit.as_deref() == Some("@sh:a2"), "refused", || {
        format!("unit (by address): {:?}", r.unit)
    })?;
    need(
        r.code == err.report().id() && r.code == "MPC-E301",
        "refused",
        || format!("code: {} (the call's {})", r.code, err.report().id()),
    )?;
    let holder = r.holder.as_ref().map(|h| (h.id.as_str(), h.name.as_str()));
    need(holder == Some(("brook", "Brook")), "refused", || {
        format!("holder: {:?}", r.holder)
    })?;
    need(
        r.claim.as_deref() == Some(held.to_string().as_str()),
        "refused",
        || format!("claim: {:?} (held {held})", r.claim),
    )?;
    need(
        r.turn.as_ref().map(|t| t.as_str()) == Some("t:9c41e07ab2d35f68.21"),
        "refused",
        || format!("turn: {:?}", r.turn),
    )?;
    let j = WatchFrame::Refused(Box::new(r.clone())).to_json();
    let want = format!(
        "{{\"type\":\"verb_refused\",\"at_ms\":{},\"actor\":{{\"id\":\"atlas\",\"name\":\"Atlas\",\
         \"kind\":\"agent\",\"colour\":\"#6d5bd0\"}},\"client\":{},\"turn\":\"t:9c41e07ab2d35f68.21\",\
         \"verb\":\"shape move\",\"unit\":\"@sh:a2\",\"code\":\"MPC-E301\",\"holder\":{{\"id\":\"brook\",\
         \"name\":\"Brook\",\"kind\":\"agent\",\"colour\":\"#c2410c\"}},\"claim\":\"{held}\"}}",
        r.at_ms, r.client
    );
    need(j == want, "refused", || {
        format!("the frame's JSON:\n{j}\nwant\n{want}")
    })?;
    let f = WatchFrame::Refused(Box::new(r.clone()));
    let last = change_frames(&fa).last().map_or(0, |c| c.seq);
    need(
        !f.numbered() && f.seq() == 0 && wa.seq() == last,
        "refused",
        || {
            format!(
                "a refusal takes no number: watch at {}, last change {last}",
                wa.seq()
            )
        },
    )?;
    need(refusal_frames(&fc).is_empty(), "no rights", || {
        format!("Cleo, without rights, got a refusal frame: {fc:?}")
    })?;
    fg.extend(wg.try_frames());
    let g = refusal_frames(&fg);
    need(g.len() == 1, "app viewer", || {
        format!("the App viewer's refusal frames: {fg:?}")
    })?;
    let g = g[0];
    let handle = |id: &str| id.starts_with('p') && id[1..].parse::<u32>().is_ok();
    need(
        handle(&g.actor.id)
            && g.holder
                .as_ref()
                .is_some_and(|h| handle(&h.id) && h.id != g.actor.id)
            && g.turn.is_none()
            && (g.verb.as_str(), g.unit.as_deref(), g.code)
                == ("shape move", Some("@sh:a2"), "MPC-E301"),
        "app viewer",
        || format!("by handle, no turn: {g:?}"),
    )?;
    Ok(())
}

#[test]
fn a_refused_verb_is_one_refusal_frame_for_viewers_with_rights_only() {
    row_refusals().unwrap();
}

// ---------- outside saves ----------

fn outside_frames(frames: &[WatchFrame]) -> Vec<&WatchOutsideSave> {
    frames
        .iter()
        .filter_map(|f| match f {
            WatchFrame::OutsideSave(o) => Some(&**o),
            _ => None,
        })
        .collect()
}

/// The board's file with one more `meta` entry, saved by another program.
fn save_outside(f: &Fx, key: &str) {
    let mut p = parse(&fs::read(f.path()).unwrap()).unwrap();
    p.meta.insert(key.into(), Value::str("x"));
    fs::write(f.path(), render(&p)).unwrap();
}

/// Each way an outside save goes is one unnumbered `outside_save` frame, after the save's own
/// change frame, with its fields: ingested (`rev`), kept both (`rev`, `kept`, the copy's name,
/// path and doc address), declined (`code`, `message`, once while it stays declined) and missing
/// (once while it stays gone). Nothing for a check that finds nothing new or the kit's own
/// write. A watcher whose rights went gets none.
fn row_outside_saves() -> Result<(), String> {
    let f = fx("outside", |_| {});
    let s = f.board(false);
    let ada = join(&s, person("ada", "Ada"));
    let cleo = join(&s, person("cleo", "Cleo"));
    let own = join(&s, person("own", "Owen"));
    let mut wa = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    let mut wc = s.watch_keyed(cleo, &listed()).unwrap().unwrap();
    let mut fa = s.watch_snapshot(&mut wa).unwrap();
    let mut fc = s.watch_snapshot(&mut wc).unwrap();
    f.host.cleo_revoked.store(true, Ordering::SeqCst);
    let mut seen = 0;
    fn next(
        w: &mut KeyedWatch,
        fa: &mut Vec<WatchFrame>,
        seen: &mut usize,
    ) -> Vec<WatchOutsideSave> {
        let new = w.try_frames();
        fa.extend(new.iter().cloned());
        let got: Vec<WatchOutsideSave> = outside_frames(&new).into_iter().cloned().collect();
        *seen += got.len();
        got
    }
    // nothing new: no frame
    let _ = s.file_changed();
    let got = next(&mut wa, &mut fa, &mut seen);
    need(got.is_empty(), "unchanged", || format!("{got:?}"))?;
    // ingested: an outside save over the file as the kit read it
    f.tick();
    save_outside(&f, "one");
    let check = s.file_changed().unwrap();
    let DiskCheck::Ingested { version } = check else {
        return Err(format!("ingested: the check gave {check:?}"));
    };
    let new = wa.try_frames();
    fa.extend(new.iter().cloned());
    let kinds: Vec<&str> = new
        .iter()
        .map(|x| match x {
            WatchFrame::Change(_) => "change",
            WatchFrame::OutsideSave(_) => "outside_save",
            _ => "other",
        })
        .collect();
    need(kinds == ["change", "outside_save"], "ingested", || {
        format!("the save's frames: {kinds:?}")
    })?;
    seen += outside_frames(&new).len();
    let o = outside_frames(&new)[0];
    need(
        matches!(o.outcome, OutsideSaveOutcome::Ingested { rev } if rev == version)
            && o.doc == NAME
            && o.at_ms == f.host.now.load(Ordering::SeqCst),
        "ingested",
        || format!("{o:?}"),
    )?;
    let j = WatchFrame::OutsideSave(Box::new(o.clone())).to_json();
    let want = format!(
        "{{\"type\":\"outside_save\",\"at_ms\":{},\"doc\":\"{NAME}\",\"outcome\":\"ingested\",\"rev\":{version}}}",
        o.at_ms
    );
    need(j == want, "ingested", || format!("JSON {j}\nwant {want}"))?;
    let f0 = WatchFrame::OutsideSave(Box::new(o.clone()));
    need(!f0.numbered() && f0.seq() == 0, "ingested", || {
        "an outside save's frame takes a number".into()
    })?;
    // kept both: the kit wrote the file, then another program saved over it
    raw(
        &s,
        own,
        vec![Edit::Map {
            root: META.into(),
            ops: vec![MapOp::Set {
                key: "kit".into(),
                value: Value::str("wrote"),
            }],
        }],
    )
    .ok_or("kept both: the kit's edit")?;
    s.flush(own).unwrap();
    // the kit's own write: no frame
    let _ = s.file_changed();
    let got = next(&mut wa, &mut fa, &mut seen);
    need(got.is_empty(), "own write", || format!("{got:?}"))?;
    f.tick();
    save_outside(&f, "two");
    let check = s.file_changed().unwrap();
    let DiskCheck::Conflict {
        version,
        kept,
        copy: Some(copy),
    } = check
    else {
        return Err(format!("kept both: the check gave {check:?}"));
    };
    let got = next(&mut wa, &mut fa, &mut seen);
    let name = copy.file_name().unwrap().to_str().unwrap().to_owned();
    let ok = match got.as_slice() {
        [WatchOutsideSave {
            outcome:
                OutsideSaveOutcome::KeptBoth {
                    rev,
                    kept: k,
                    copy: Some(c),
                },
            ..
        }] => {
            *rev == version
                && *k == kept
                && c.name == name
                && c.path == name
                && c.doc.as_deref() == Some(name.as_str())
        }
        _ => false,
    };
    need(ok, "kept both", || {
        format!("{got:?} (copy {copy:?}, kept {kept})")
    })?;
    let j = WatchFrame::OutsideSave(Box::new(got[0].clone())).to_json();
    need(
        j.contains(&format!(
            "\"outcome\":\"kept_both\",\"rev\":{version},\"kept\":{kept},\"copy\":{{\"name\":\"{name}\",\"path\":\"{name}\",\"doc\":\"{name}\"}}}}"
        )),
        "kept both",
        || format!("JSON {j}"),
    )?;
    // declined: once, while it stays declined
    f.tick();
    fs::write(f.path(), b"S\tnot a line\n").unwrap();
    let check = s.file_changed().unwrap();
    let DiskCheck::Declined(why) = check else {
        return Err(format!("declined: the check gave {check:?}"));
    };
    let got = next(&mut wa, &mut fa, &mut seen);
    let ok = matches!(got.as_slice(),
        [WatchOutsideSave { outcome: OutsideSaveOutcome::Declined { code, message }, .. }]
            if *code == why.code && *message == why.message && *code == "malformed");
    need(ok, "declined", || format!("{got:?} ({why:?})"))?;
    f.tick();
    fs::write(f.path(), b"S\tstill not a line\n").unwrap();
    let _ = s.file_changed();
    let got = next(&mut wa, &mut fa, &mut seen);
    need(got.is_empty(), "declined again", || format!("{got:?}"))?;
    // missing: once, while it stays gone
    fs::remove_file(f.path()).unwrap();
    let check = s.file_changed().unwrap();
    need(matches!(check, DiskCheck::Missing), "missing", || {
        format!("the check gave {check:?}")
    })?;
    let got = next(&mut wa, &mut fa, &mut seen);
    let ok = matches!(
        got.as_slice(),
        [WatchOutsideSave {
            outcome: OutsideSaveOutcome::Missing,
            ..
        }]
    );
    need(ok, "missing", || format!("{got:?}"))?;
    let _ = s.file_changed();
    let got = next(&mut wa, &mut fa, &mut seen);
    need(got.is_empty(), "missing again", || format!("{got:?}"))?;
    need(seen == 4, "one frame a save", || {
        format!("{seen} outside_save frames for four outcomes")
    })?;
    fc.extend(wc.try_frames());
    need(outside_frames(&fc).is_empty(), "no rights", || {
        format!("Cleo, without rights, got outside_save frames: {fc:?}")
    })?;
    // the frames replay: an outside save's frame changes nothing
    replay(&fa).map(|_| ())
}

#[test]
fn each_outside_save_is_one_outside_save_frame_with_its_outcome() {
    row_outside_saves().unwrap();
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

#[test]
fn a_change_frame_carries_who_how_and_what() {
    let f = fx("fields", |_| {});
    let s = f.board(true);
    let own = join(&s, person("own", "Owen"));
    let bot = join(&s, agent());
    let ada = join(&s, person("ada", "Ada"));
    let app = join(
        &s,
        Participant::new("guest", "Guest", "#ffaa00", ParticipantKind::App, "app"),
    );
    let mut w = s.watch_keyed(ada, &listed()).unwrap().unwrap();
    let mut wa = s.watch_keyed(app, &listed()).unwrap().unwrap();
    let mut frames = s.watch_snapshot(&mut w).unwrap();
    let mut app_frames = s.watch_snapshot(&mut wa).unwrap();
    // the agent's verb under its held claim, with a turn
    let claim = s
        .claim_elems(bot, SHAPES, &[k("a1")], None, None, true)
        .unwrap()
        .id;
    let moved = verb(&s, bot, &words(&["shape", "move", "a1"]), turn(12)).unwrap();
    // two roots in one change
    let added = verb(&s, bot, &words(&["shape", "add", "n9", "a2"]), turn(12)).unwrap();
    // a person's raw op: no turn, no verb, no claim
    let set = raw(
        &s,
        own,
        vec![Edit::Map {
            root: META.into(),
            ops: vec![MapOp::Delete {
                key: "title".into(),
            }],
        }],
    )
    .unwrap();
    // the agent's undo
    s.undo_with(bot, &UndoRequest::steps(1).turn(turn(13)))
        .unwrap();
    let undone = version(&s);
    // an outside save
    let _ = s.flush(own);
    let mut p = parse(&fs::read(f.path()).unwrap()).unwrap();
    p.meta.insert("theme".into(), Value::str("dark"));
    fs::write(f.path(), render(&p)).unwrap();
    s.file_changed().unwrap();
    let saved = version(&s);
    frames.extend(w.try_frames());
    app_frames.extend(wa.try_frames());
    let ch = change_frames(&frames);
    let at = |rev: u64| {
        *ch.iter()
            .find(|c| c.rev == rev)
            .unwrap_or_else(|| panic!("no frame for rev {rev}: {ch:?}"))
    };
    let m = at(moved);
    assert_eq!(m.verb.as_deref(), Some("shape move"));
    assert_eq!(
        m.turn.as_ref().map(|t| t.as_str()),
        Some("t:9c41e07ab2d35f68.12")
    );
    assert_eq!(
        m.claim.as_deref(),
        Some(claim.to_string().as_str()),
        "the held claim"
    );
    assert_eq!(
        (m.actor.id.as_str(), m.actor.name.as_str(), m.actor.kind),
        ("atlas", "Atlas", ParticipantKind::Agent)
    );
    assert_eq!(m.actor.colour.as_deref(), Some("#6d5bd0"));
    assert_eq!(m.via, "cli");
    assert_eq!(m.kind.as_str(), "edit");
    assert!(
        matches!(
            m.ops.as_slice(),
            [WatchOp::Map { root, key, field: Some(fld), after: Some(Value::Str(v)), .. }]
                if root == FIELDS && key == "@sh:a1" && fld == "geom" && v == "1"
        ),
        "the move's op by key and field: {:?}",
        m.ops
    );
    let j = WatchFrame::Change(Box::new(m.clone())).to_json();
    assert!(
        j.contains("\"derived\":{\"roots\":1,\"live\":4}")
            && j.contains("\"render_hashes\":{\"board\":\"rh:4\"}"),
        "the pack's extras: {j}"
    );
    let a = at(added);
    assert_eq!(a.ops.len(), 2, "both roots in one frame: {:?}", a.ops);
    assert_eq!(a.claim, None, "n9 isn't claimed");
    let r = at(set);
    assert_eq!(
        (r.turn.as_ref(), r.verb.as_ref(), r.claim.as_ref()),
        (None, None, None)
    );
    assert_eq!(r.actor.kind, ParticipantKind::Person);
    assert!(
        matches!(
            r.ops.as_slice(),
            [WatchOp::Map {
                field: None,
                after: None,
                before: Some(_),
                ..
            }]
        ),
        "{:?}",
        r.ops
    );
    let u = at(undone);
    assert_eq!((u.kind.as_str(), u.verb.as_ref()), ("undo", None));
    let x = at(saved);
    assert_eq!(
        (x.actor.id.as_str(), x.actor.kind, x.actor.colour.as_ref()),
        ("kit:external", ParticipantKind::External, None)
    );
    assert!(WatchFrame::Change(Box::new(x.clone()))
        .to_json()
        .contains("\"kind\":\"file\",\"colour\":null"));
    // an App viewer sees handles and no turn
    let am = change_frames(&app_frames)
        .into_iter()
        .find(|c| c.rev == moved)
        .unwrap()
        .clone();
    assert_ne!(am.actor.id, "atlas");
    assert!(am.actor.id.starts_with('p'), "a handle: {}", am.actor.id);
    assert_eq!(am.turn, None);
    // and every frame replays
    let rep = replay(&frames).unwrap();
    assert_eq!(rep.json(), doc_json(&s));
}

// ---------- the view index in the snapshot ----------

const PAGES_R: &str = "pages";
const ITEMS_R: &str = "items";
const ITEM_F: &str = "item_f";

const DECK_SCHEMA: Schema = Schema {
    name: "deck-view",
    roots: &[
        (PAGES_R, Kind::MovableList),
        (ITEMS_R, Kind::MovableList),
        (ITEM_F, Kind::Map),
    ],
};

/// Pages (the containers) and items (the units, each naming its page in its `page` field).
const DECK_VIEW: ViewSchema = ViewSchema {
    containers: PAGES_R,
    units: ITEMS_R,
    unit_container_field: "page",
    native_unit: "px",
    container_fields: ContainerFields::NONE,
    unit_fields: UnitFields {
        kind: Src::ListValue,
        text: Src::Field("text"),
        ..UnitFields::NONE
    },
    ..ViewSchema::NONE
};

/// A deck with a view schema, opened without a file: `P <page>` and `I <page> <item> <text>`
/// lines. A page `p1` is `@pg:p1` on the wire, an item `i1` `@it:i1`.
struct ViewDeck;

impl Pack for ViewDeck {
    fn schema(&self) -> Schema {
        DECK_SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(ITEM_F, ITEMS_R)]
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(DECK_VIEW)
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match root {
            PAGES_R => format!("@pg:{key}"),
            ITEMS_R => format!("@it:{key}"),
            _ => key.to_string(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        let prefix = match root {
            PAGES_R => "@pg:",
            ITEMS_R => "@it:",
            _ => return None,
        };
        Key::new(address.strip_prefix(prefix)?).ok()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (mut pages, mut items, mut fields) = (Vec::new(), Vec::new(), BTreeMap::new());
        for line in std::str::from_utf8(b).unwrap().lines() {
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            match parts.as_slice() {
                ["P", p] => pages.push((k(p), Value::Null)),
                ["I", p, i, text] => {
                    items.push((k(i), Value::str("box")));
                    fields.insert(format!("{i}{SEP}page"), Value::str(*p));
                    fields.insert(format!("{i}{SEP}text"), Value::str(*text));
                }
                _ => {
                    return Err(Declined {
                        code: "malformed",
                        message: format!("bad line {line:?}"),
                    })
                }
            }
        }
        Ok(PackContent {
            roots: vec![
                (PAGES_R.into(), Neutral::List(pages)),
                (ITEMS_R.into(), Neutral::List(items)),
                (ITEM_F.into(), Neutral::Map(fields)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

/// `pages` pages of `items` items each, every item's text `len` characters.
fn deck_bytes(pages: usize, items: usize, len: usize) -> String {
    let mut out = String::new();
    for p in 1..=pages {
        out.push_str(&format!("P p{p}\n"));
    }
    for p in 1..=pages {
        for i in 1..=items {
            let text: String = format!("item {i} on page {p} \u{e9} ")
                .chars()
                .cycle()
                .take(len)
                .collect();
            out.push_str(&format!("I p{p} i{p}x{i} {text}\n"));
        }
    }
    out
}

fn open_deck(f: &Fx, name: &str, bytes: &str) -> Arc<Session> {
    f.mgr
        .open(Arc::new(ViewDeck), &doc_key(name), Some(bytes.as_bytes()))
        .unwrap()
}

/// The snapshot carries the view index ([`Session::view_index`]'s, at the snapshot's version):
/// its snapshot in the head, and each container's units in pages that reassemble to the index's;
/// the keyed state and the ops name every element by its address.
#[test]
fn the_snapshot_carries_the_view_index() {
    let f = fx("view", |_| {});
    let s = open_deck(&f, "v.deck", &deck_bytes(3, 150, 100));
    let own = join(&s, person("own", "Owen"));
    let ada = join(&s, person("ada", "Ada"));
    let mut w = s
        .watch_keyed(ada, &listed().page_bytes(WATCH_PAGE_BYTES_MIN))
        .unwrap()
        .unwrap();
    let mut frames = s.watch_snapshot(&mut w).unwrap();
    let ix = s.view_index(ada).unwrap().expect("a view index");
    let WatchFrame::Snapshot(head) = &frames[0] else {
        panic!("no head: {:?}", frames[0]);
    };
    assert_eq!(
        head.view,
        Some((DECK_VIEW, ix.snapshot())),
        "the head's view"
    );
    let units: Vec<&WatchFrame> = frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Page(p) if matches!(p.part, WatchPart::Units(_))))
        .collect();
    assert!(
        units.len() >= 6,
        "{} units pages for 3 x 150 units",
        units.len()
    );
    for f in &frames {
        let n = f.to_json().len();
        match f {
            WatchFrame::Page(p) if matches!(p.part, WatchPart::Units(_)) => {
                assert!(n <= VIEW_PAGE_MIN + 256, "a units page of {n} bytes")
            }
            // the head holds the view index's snapshot whole, under the frame cap
            WatchFrame::Snapshot(_) => assert!(n <= WATCH_FRAME_MAX, "a head of {n} bytes"),
            _ => assert!(n <= WATCH_PAGE_BYTES_MIN, "a page of {n} bytes"),
        }
    }
    let r = replay(&frames).unwrap();
    for c in &ix.containers {
        let got = r.units.get(&c.key).map(Vec::as_slice).unwrap_or(&[]);
        assert_eq!(got, ix.units_of(&c.key).unwrap(), "the units of {}", c.key);
    }
    assert_eq!(
        r.units.values().map(Vec::len).sum::<usize>(),
        ix.units.len()
    );
    assert!(r.view.is_some());
    let state = s.keyed_state_for_tests().unwrap();
    assert_eq!(r.json(), watch_state_json(&ViewDeck, &state));
    for f in &frames {
        if let WatchFrame::Page(p) = f {
            if let WatchPart::Live(l) = &p.part {
                assert!(
                    l.iter().all(|(a, _)| a.starts_with('@')),
                    "keys, not addresses: {l:?}"
                );
            }
            if let WatchPart::Entries(e) = &p.part {
                assert!(e.iter().all(|(a, _, _)| a.starts_with("@it:")), "{e:?}");
            }
        }
    }
    // the ops by address
    raw(
        &s,
        own,
        vec![Edit::List {
            root: ITEMS_R.into(),
            ops: vec![ListOp::Move {
                key: k("i1x3"),
                to: Anchor::Start,
            }],
        }],
    )
    .unwrap();
    raw(
        &s,
        own,
        vec![Edit::Map {
            root: ITEM_F.into(),
            ops: vec![MapOp::Set {
                key: format!("i2x5{SEP}text"),
                value: Value::str("moved"),
            }],
        }],
    )
    .unwrap();
    frames.extend(w.try_frames());
    let ch = change_frames(&frames);
    assert!(
        matches!(ch[0].ops.as_slice(), [WatchOp::List { key, .. }] if key == "@it:i1x3"),
        "{:?}",
        ch[0].ops
    );
    assert!(
        matches!(ch[1].ops.as_slice(), [WatchOp::Map { key, field: Some(fl), .. }] if key == "@it:i2x5" && fl == "text"),
        "{:?}",
        ch[1].ops
    );
    let r = replay(&frames).unwrap();
    assert_eq!(
        r.json(),
        watch_state_json(&ViewDeck, &s.keyed_state_for_tests().unwrap())
    );
}

// ---------- goldens and the client's vectors ----------

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../client/test/golden")
}

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../client/test/vectors")
}

fn golden(path: &Path, got: &str) {
    if std::env::var_os("MPC_BLESS").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, got).unwrap();
        return;
    }
    let want = fs::read_to_string(path)
        .unwrap_or_else(|_| panic!("{} is missing (MPC_BLESS=1 writes it)", path.display()));
    assert!(
        want == got,
        "{} differs from what the kit encodes now (MPC_BLESS=1 regenerates it)\n--- now:\n{got}",
        path.display()
    );
}

fn file(v: Json) -> String {
    let mut s = v.to_json_pretty();
    s.push('\n');
    s
}

/// Fixed numbers for the run's random client IDs and claim ids, in order of appearance.
#[derive(Default)]
struct Stable {
    clients: HashMap<u64, u64>,
    claims: HashMap<String, String>,
}

impl Stable {
    fn frame(&mut self, f: &WatchFrame) -> Json {
        let mut f = f.clone();
        if let WatchFrame::Change(c) = &mut f {
            let n = self.clients.len() as u64 + 1;
            c.client = *self.clients.entry(c.client).or_insert(n);
            if let Some(id) = c.claim.take() {
                let n = self.claims.len() + 1;
                c.claim = Some(
                    self.claims
                        .entry(id)
                        .or_insert_with(|| format!("claim-{n}"))
                        .clone(),
                );
            }
        }
        f.to_value()
    }
    fn frames(&mut self, fs: &[WatchFrame]) -> Json {
        Json::List(fs.iter().map(|f| self.frame(f)).collect())
    }
}

/// The client's vectors: each seed's frames, the doc's final state and version, and a late
/// snapshot (numbered on) a client that lost a frame can mend with.
#[test]
fn the_clients_vectors_are_current() {
    let mut cases = Vec::new();
    for &seed in SEEDS {
        let t = run_trace(seed, STEPS, trace_page_bytes(seed));
        let mut st = Stable::default();
        cases.push(Json::obj([
            ("seed", Json::from(seed)),
            ("page_bytes", Json::from(trace_page_bytes(seed))),
            ("frames", st.frames(&t.frames)),
            ("late", st.frames(&t.late)),
            ("final_json", Json::from(t.final_json.as_str())),
            ("final_rev", Json::from(t.final_rev)),
        ]));
    }
    golden(
        &vectors_dir().join("keyed_watch.json"),
        &(Json::obj([
            (
                "about",
                Json::from(
                    "k2-mpc-base keyed watch vectors for the JS client: per seeded trace, the \
                     frames a watcher got (a snapshot in pages, then one frame per change), a \
                     late snapshot numbered on, and the doc's final keyed state as canonical \
                     JSON and its version. Written by crates/base/tests/keyed_watch.rs \
                     (MPC_BLESS=1).",
                ),
            ),
            ("feature", Json::from(KEYED_WATCH_FEATURE)),
            ("cases", Json::List(cases)),
        ])
        .to_json()
            + "\n"),
    );
}

/// A small deck with a view schema: its snapshot (the view index's snapshot in the head, its
/// units in pages), its state by address, and the units by container.
fn view_golden(f: &Fx) -> Json {
    let d = open_deck(f, "small.deck", &deck_bytes(2, 3, 12));
    let ada = join(&d, person("ada", "Ada"));
    let mut w = d.watch_keyed(ada, &listed()).unwrap().unwrap();
    let frames = d.watch_snapshot(&mut w).unwrap();
    let r = replay(&frames).unwrap();
    let state = d.keyed_state_for_tests().unwrap();
    assert_eq!(
        r.json(),
        watch_state_json(&ViewDeck, &state),
        "control: the deck replays"
    );
    let units = r
        .units
        .iter()
        .map(|(c, us)| (c.clone(), Json::List(us.iter().map(unit_value).collect())));
    Json::obj([
        ("frames", Stable::default().frames(&frames)),
        (
            "final_json",
            Json::from(watch_state_json(&ViewDeck, &state).as_str()),
        ),
        ("units", Json::obj(units)),
    ])
}

/// `keyed_snapshot.json`: a board's snapshot in pages of the smallest cap (its shapes over more
/// than one page, deleted ones on a page of their own), and the state it reassembles to.
/// `keyed_frames.json`: one scripted session's frames (an agent's turn of verbs under a held
/// claim, a person's ops on every kind of root, writes to a deleted element, an undo, an outside
/// save) after its snapshot, and the final state.
#[test]
fn the_keyed_frames_are_golden() {
    let f = fx("golden", |_| {});
    let s = f.board(false);
    let own = join(&s, person("own", "Owen"));
    let bot = join(&s, agent());
    let ada = join(&s, person("ada", "Ada"));
    // enough shapes for more than one page, three of them deleted
    let ops = (0..80)
        .map(|i| ListOp::Insert {
            key: k(&format!("g{i}")),
            at: Anchor::End,
            value: Value::str(format!("grid cell {i}")),
        })
        .collect();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops,
        }],
    )
    .unwrap();
    let gone = ["g3", "g17", "g31"]
        .map(|g| ListOp::Delete { key: k(g) })
        .to_vec();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: gone,
        }],
    )
    .unwrap();
    let mut w = s
        .watch_keyed(ada, &listed().page_bytes(WATCH_PAGE_BYTES_MIN))
        .unwrap()
        .unwrap();
    let mut frames = s.watch_snapshot(&mut w).unwrap();
    let live_pages = frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Page(p) if matches!(p.part, WatchPart::Live(_))))
        .count();
    assert!(
        live_pages > 1,
        "control: the shapes span {live_pages} pages"
    );
    let snap_json = doc_json(&s);
    let mut st = Stable::default();
    golden(
        &golden_dir().join("keyed_snapshot.json"),
        &file(Json::obj([
            ("page_bytes", Json::from(WATCH_PAGE_BYTES_MIN)),
            ("frames", st.frames(&frames)),
            ("final_json", Json::from(snap_json.as_str())),
            ("final_rev", Json::from(version(&s))),
            ("view", view_golden(&f)),
        ])),
    );
    let tick = || f.host.now.fetch_add(1_000, Ordering::SeqCst);
    s.claim_elems(
        bot,
        SHAPES,
        &[k("a1"), k("a2")],
        None,
        Some("tidying".into()),
        true,
    )
    .unwrap();
    tick();
    verb(&s, bot, &words(&["shape", "move", "a1"]), turn(12)).unwrap();
    tick();
    verb(
        &s,
        bot,
        &words(&["shape", "text", "a2", "Caf\u{e9} \"two\"\t\u{1F389}"]),
        turn(12),
    )
    .unwrap();
    tick();
    verb(&s, bot, &words(&["shape", "add", "n1", "a2"]), turn(12)).unwrap();
    s.release_claim(bot, None).unwrap();
    tick();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: vec![ListOp::Move {
                key: k("a4"),
                to: Anchor::Start,
            }],
        }],
    )
    .unwrap();
    tick();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: vec![ListOp::Delete { key: k("a3") }],
        }],
    )
    .unwrap();
    tick();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: vec![ListOp::Set {
                key: k("a3"),
                value: Value::str("gone"),
            }],
        }],
    )
    .unwrap();
    tick();
    raw(
        &s,
        own,
        vec![Edit::List {
            root: SHAPES.into(),
            ops: vec![ListOp::Move {
                key: k("a3"),
                to: Anchor::Start,
            }],
        }],
    )
    .unwrap();
    tick();
    raw(
        &s,
        own,
        vec![Edit::Map {
            root: META.into(),
            ops: vec![
                MapOp::Delete {
                    key: "title".into(),
                },
                MapOp::Set {
                    key: "owner".into(),
                    value: Value::str("Owen"),
                },
            ],
        }],
    )
    .unwrap();
    tick();
    let t = notes_now(&s);
    raw(
        &s,
        own,
        vec![Edit::Text {
            root: NOTES.into(),
            hunks: vec![
                Hunk {
                    start: 0,
                    end: 5,
                    insert: "1st".into(),
                },
                Hunk {
                    start: t.len(),
                    end: t.len(),
                    insert: "\n\u{e9}nd".into(),
                },
            ],
        }],
    )
    .unwrap();
    tick();
    s.undo_with(bot, &UndoRequest::steps(1).turn(turn(13)))
        .unwrap();
    tick();
    let _ = s.flush(own);
    let mut p = parse(&fs::read(f.path()).unwrap()).unwrap();
    p.meta.insert("theme".into(), Value::str("dark"));
    fs::write(f.path(), render(&p)).unwrap();
    s.file_changed().unwrap();
    frames.extend(w.try_frames());
    assert!(!w.needs_snapshot());
    let r = replay(&frames).unwrap();
    assert_eq!(r.json(), doc_json(&s), "control: the frames replay");
    golden(
        &golden_dir().join("keyed_frames.json"),
        &file(Json::obj([
            (
                "features",
                Json::List(vec![Json::from(KEYED_WATCH_FEATURE)]),
            ),
            ("frames", st.frames(&frames)),
            ("final_json", Json::from(doc_json(&s).as_str())),
            ("final_rev", Json::from(version(&s))),
        ])),
    );
}

// ---------- nesting in the view schema ----------

/// The deck's view schema with nesting declared: a box sits in a group, and a group in a group.
const NESTED_VIEW: ViewSchema = ViewSchema {
    nesting: &[
        Nesting {
            kind: "box",
            parent: "group",
        },
        Nesting {
            kind: "group",
            parent: "group",
        },
    ],
    ..DECK_VIEW
};

/// [`ViewDeck`] with [`NESTED_VIEW`].
struct NestedDeck;

impl Pack for NestedDeck {
    fn schema(&self) -> Schema {
        ViewDeck.schema()
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        ViewDeck.composite()
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(NESTED_VIEW)
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        ViewDeck.address_of(root, key)
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        ViewDeck.key_of(root, address)
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        ViewDeck.decode(b)
    }
    fn encode(&self, r: &[(String, Neutral)], c: &[u8]) -> Result<Vec<u8>, PackError> {
        ViewDeck.encode(r, c)
    }
}

/// A view schema's `nesting` read back from its JSON: `(kind, parent)` pairs, in order; `None`
/// when the JSON has no `nesting`.
fn nesting_of(v: &Json) -> Option<Vec<(String, String)>> {
    let Json::List(l) = v.get("nesting")? else {
        panic!("nesting is not a list: {}", v.to_json());
    };
    Some(
        l.iter()
            .map(|n| {
                let s = |k: &str| match n.get(k) {
                    Some(Json::Str(s)) => s.clone(),
                    other => panic!("nesting entry's {k}: {other:?}"),
                };
                (s("kind"), s("parent"))
            })
            .collect(),
    )
}

/// The deck's view schema as the snapshot carried it before nesting existed (the bytes of
/// `client/test/golden/keyed_snapshot.json`'s `view` head).
const DECK_VIEW_JSON: &str = "{\"containers\":\"pages\",\"units\":\"items\",\
\"unit_container_field\":\"page\",\"container_id_field\":null,\"native_unit\":\"px\",\"page\":null,\
\"container_fields\":{\"title\":null,\"hidden\":null,\"layout\":null},\"unit_fields\":{\"kind\":\
{\"list_value\":true},\"name\":null,\"alt\":null,\"text\":\"text\",\"hidden\":null,\
\"editable_text\":null,\"placeholder\":null},\"geometry\":null,\"field_class\":{},\"derived\":\
{\"containers\":[],\"units\":[]}}";

/// A schema that declares nesting carries it, in order, in its JSON and in a watch's snapshot
/// head, and reads back the same; a schema without it is the same bytes as before; the
/// declaration is checked.
fn row_nesting() -> Result<(), String> {
    let composites = ViewDeck.composite();
    NESTED_VIEW
        .check(&DECK_SCHEMA, composites)
        .map_err(|e| format!("check: the nested schema is refused: {e}"))?;
    let want = vec![
        ("box".to_owned(), "group".to_owned()),
        ("group".to_owned(), "group".to_owned()),
    ];
    let got = nesting_of(&NESTED_VIEW.to_value());
    need(got.as_ref() == Some(&want), "round trip", || {
        format!(
            "the schema's JSON gives {got:?}: {}",
            NESTED_VIEW.to_value().to_json()
        )
    })?;
    // through a watch: the snapshot's head carries the same schema
    let f = fx("nesting", |_| {});
    let d = f
        .mgr
        .open(
            Arc::new(NestedDeck),
            &doc_key("nested.deck"),
            Some(deck_bytes(2, 3, 12).as_bytes()),
        )
        .unwrap();
    let ada = join(&d, person("ada", "Ada"));
    let mut w = d.watch_keyed(ada, &listed()).unwrap().unwrap();
    let frames = d.watch_snapshot(&mut w).unwrap();
    let head = frames.first().map(WatchFrame::to_value);
    let schema = head
        .as_ref()
        .and_then(|h| h.get("view"))
        .and_then(|v| v.get("schema"))
        .ok_or_else(|| format!("round trip: no view schema in the head: {head:?}"))?;
    need(
        nesting_of(schema).as_ref() == Some(&want),
        "round trip",
        || format!("the head's schema: {}", schema.to_json()),
    )?;
    need(
        schema.to_json() == NESTED_VIEW.to_value().to_json(),
        "round trip",
        || {
            format!(
                "the head's schema differs from the pack's: {}",
                schema.to_json()
            )
        },
    )?;
    let r = replay(&frames)?;
    let state = d.keyed_state_for_tests().unwrap();
    need(
        r.json() == watch_state_json(&NestedDeck, &state),
        "round trip",
        || "the nested deck's snapshot replays".into(),
    )?;
    // a schema without it: the same bytes as before, with no `nesting`
    let plain = DECK_VIEW.to_value();
    need(nesting_of(&plain).is_none(), "without", || {
        format!("a schema without nesting has one: {}", plain.to_json())
    })?;
    need(plain.to_json() == DECK_VIEW_JSON, "without", || {
        format!("byte for byte:\n{}\nwas\n{DECK_VIEW_JSON}", plain.to_json())
    })?;
    // checked: a pair twice, an empty kind, too many
    let refused = |n: &'static [Nesting], says: &str| -> Result<(), String> {
        let vs = ViewSchema {
            nesting: n,
            ..DECK_VIEW
        };
        match vs.check(&DECK_SCHEMA, composites) {
            Err(e) if e.contains(says) => Ok(()),
            other => Err(format!(
                "check: {n:?} gave {other:?}, want an error saying {says:?}"
            )),
        }
    };
    let pair = Nesting {
        kind: "box",
        parent: "group",
    };
    refused(
        Box::leak(vec![pair, pair].into_boxed_slice()),
        "declared twice",
    )?;
    refused(
        Box::leak(
            vec![Nesting {
                kind: "",
                parent: "group",
            }]
            .into_boxed_slice(),
        ),
        "names a kind",
    )?;
    let many: Vec<Nesting> = (0..=NESTING_MAX)
        .map(|i| Nesting {
            kind: Box::leak(format!("k{i}").into_boxed_str()),
            parent: "group",
        })
        .collect();
    refused(Box::leak(many.into_boxed_slice()), "at most")?;
    // the client's golden: the nested deck's snapshot, its state and units
    let units = r
        .units
        .iter()
        .map(|(c, us)| (c.clone(), Json::List(us.iter().map(unit_value).collect())));
    golden(
        &golden_dir().join("keyed_snapshot_nesting.json"),
        &file(Json::obj([
            (
                "features",
                Json::List(vec![Json::from(KEYED_WATCH_FEATURE)]),
            ),
            ("frames", Stable::default().frames(&frames)),
            (
                "final_json",
                Json::from(watch_state_json(&NestedDeck, &state).as_str()),
            ),
            ("units", Json::obj(units)),
        ])),
    );
    Ok(())
}

#[test]
fn a_view_schema_declares_nesting_and_one_without_it_is_unchanged() {
    row_nesting().unwrap();
}
