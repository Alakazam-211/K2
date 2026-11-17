//! **The view in every edit result** (`mpc.result/1`'s `view`, `EditResult::view`): where a
//! change is, by the addresses a person's view and a still use. A keyed doc's are the containers
//! and units the agent's presence moves to (the last container touched wins); a 3D doc's
//! primary address is the unit; a deleted container's is the next one, else the previous one;
//! a text doc's are its changed lines and the pack's named spans (the innermost holding the
//! change). An agent's verb moves its own view address there; a person's never moves. Public
//! API only.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{ListOp, MapOp};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::placement::{Precision, SpaceSlot, SpaceSrc};
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, GridWrite, GridWriteOptions, Host, Pack,
    PackContent, PackError, Participant, ParticipantKind, PlayState, Rights, RosterEntry, Session,
    SessionManager, Stay, TextFilePack, TextUnit, TimePosition, UndoOutcome, UndoRequest,
    WriteOptions, WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, ToolOptions, VerbCtx};
use k2_mpc_base::view_index::{Src, UnitFields, ViewSchema};
use k2_mpc_base::{Key, Value};

#[path = "grid_pack/mod.rs"]
mod grid_pack;
#[path = "sheet_pack/mod.rs"]
mod sheet_pack;

// ---------- a deck: slides hold shapes ----------

const SLIDES: &str = "slides";
const SHAPES: &str = "shapes";
const SHAPE_F: &str = "shape_f";
const SEP: char = '\u{1f}';

const SCHEMA: Schema = Schema {
    name: "toy-deck",
    roots: &[
        (SLIDES, Kind::MovableList),
        (SHAPES, Kind::MovableList),
        (SHAPE_F, Kind::Map),
    ],
};

const VIEW: ViewSchema = ViewSchema {
    containers: SLIDES,
    units: SHAPES,
    unit_container_field: "slide",
    native_unit: "px",
    unit_fields: UnitFields {
        kind: Src::ListValue,
        ..UnitFields::NONE
    },
    ..ViewSchema::NONE
};

/// The same deck placed in 3D: its view is the object, not the group.
const VIEW_3D: ViewSchema = ViewSchema {
    space: Some(SpaceSlot {
        unit_fields: &["xform"],
        container_fields: &[],
        space: SpaceSrc::Pack { roots: &[] },
        precision: Precision::F64,
    }),
    ..VIEW
};

struct Deck {
    three_d: bool,
}

const S: &[PersonAction] = &[PersonAction::Format];

const VERBS: &[PackVerb] = &[
    PackVerb::structured_edit(
        "shape set",
        "shape set <shape> <field> <value>",
        S,
        &[0, 2],
        shape_set,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "pair set",
        "pair set <a> <b> <field> <value>",
        S,
        &[0, 2],
        pair_set,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape delete",
        "shape delete <shape>",
        S,
        &[0, 2],
        shape_delete,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "slide delete",
        "slide delete <slide>",
        S,
        &[0, 2],
        slide_delete,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape play",
        "shape play <shape> <at>",
        S,
        &[0, 2],
        shape_play,
    )
    .positions(&[]),
    PackVerb::structured_edit(
        "shape at",
        "shape at <shape> <view address>",
        S,
        &[0, 2],
        shape_at,
    )
    .positions(&[]),
];

fn usage(m: &str) -> ToolError {
    ToolError::usage(m.to_owned())
}

/// One field set of shape `shape`.
fn set(shape: &str, field: &str, value: &str) -> MapOp {
    MapOp::Set {
        key: format!("{shape}{SEP}{field}"),
        value: Value::str(value),
    }
}

fn map_edit(ops: Vec<MapOp>) -> Vec<Edit> {
    vec![Edit::Map {
        root: SHAPE_F.into(),
        ops,
    }]
}

/// `shape set <shape> <field> <value>`.
fn shape_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, field, value] = args else {
        return Err(usage("shape set <shape> <field> <value>"));
    };
    let op = set(shape, field, value);
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(map_edit(vec![op]), Vec::new(), 0))
    }))
}

/// `pair set <a> <b> <field> <value>`: `a`'s field, then `b`'s.
fn pair_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let [a, b, field, value] = args else {
        return Err(usage("pair set <a> <b> <field> <value>"));
    };
    let ops = vec![set(a, field, value), set(b, field, value)];
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(map_edit(ops), Vec::new(), 0))
    }))
}

/// A delete of element `key` of list root `root`.
fn delete(root: &'static str, args: &[String]) -> Result<StructEdit, ToolError> {
    let [key] = args else {
        return Err(usage("<verb> <key>"));
    };
    let key = Key::new(key).map_err(|e| usage(&e.to_string()))?;
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(
            vec![Edit::List {
                root: root.into(),
                ops: vec![ListOp::Delete { key }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

fn shape_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    delete(SHAPES, args)
}

fn slide_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    delete(SLIDES, args)
}

/// `shape play <shape> <at>`: the shape's `at` field, and the agent's playhead there, paused,
/// in the shape's slide `@sl:p1`.
fn shape_play(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, at] = args else {
        return Err(usage("shape play <shape> <at>"));
    };
    let op = set(shape, "at", at);
    let at = at.clone();
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        let t = TimePosition::new(&at, PlayState::Paused)
            .and_then(|t| t.within("@sl:p1"))
            .map_err(|e| usage(&e.to_string()))?;
        Ok(StructOutput::new(map_edit(vec![op]), Vec::new(), 0).time_position(t))
    }))
}

/// `shape at <shape> <address>`: a field set, and the verb says its view is `address`.
fn shape_at(args: &[String]) -> Result<StructEdit, ToolError> {
    let [shape, address] = args else {
        return Err(usage("shape at <shape> <address>"));
    };
    let op = set(shape, "seen", "1");
    let address = address.clone();
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        Ok(StructOutput::new(map_edit(vec![op]), Vec::new(), 0).viewing(address))
    }))
}

impl Pack for Deck {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(SHAPE_F, SHAPES)]
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(if self.three_d { VIEW_3D } else { VIEW })
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        match root {
            SLIDES => format!("@sl:{}", key.as_str()),
            SHAPES => format!("@sh:{}", key.as_str()),
            _ => key.as_str().to_owned(),
        }
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        let k = match root {
            SLIDES => address.strip_prefix("@sl:")?,
            SHAPES => address.strip_prefix("@sh:")?,
            _ => address,
        };
        Key::new(k).ok()
    }
    /// Slides p1, p2, p3; shapes s1 and s2 on p1, s3 on p2.
    fn decode(&self, _: &[u8]) -> Result<PackContent, Declined> {
        let k = |s: &str| Key::new(s).unwrap();
        let mut f = BTreeMap::new();
        for (s, p) in [("s1", "p1"), ("s2", "p1"), ("s3", "p2")] {
            f.insert(format!("{s}{SEP}slide"), Value::str(p));
        }
        let list = |ks: &[&str], v: &str| {
            Neutral::List(ks.iter().map(|x| (k(x), Value::str(v))).collect())
        };
        Ok(PackContent {
            roots: vec![
                (SLIDES.into(), list(&["p1", "p2", "p3"], "slide")),
                (SHAPES.into(), list(&["s1", "s2", "s3"], "box")),
                (SHAPE_F.into(), Neutral::Map(f)),
            ],
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
}

// ---------- a text pack with sections: each `#` heading to the next of its level or above ----

struct Md(TextFilePack);

impl Pack for Md {
    fn tokens(&self) -> k2_mpc_base::text::lines::Tokens {
        self.0.tokens()
    }
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(bytes)
    }
    fn encode(&self, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.0.encode(content, codec)
    }
    fn normalize_input<'a>(&self, root: &str, text: &'a str) -> std::borrow::Cow<'a, str> {
        self.0.normalize_input(root, text)
    }
    fn writes_bom(&self, root: &str, codec: &[u8]) -> bool {
        self.0.writes_bom(root, codec)
    }
    /// `@h:<name lowercased>` for each heading, its lines to the next heading of its level or
    /// above.
    fn text_units(&self, _root: &str, text: &str) -> Vec<TextUnit> {
        let lines: Vec<&str> = text.lines().collect();
        let level = |l: &str| l.chars().take_while(|c| *c == '#').count();
        let mut out = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let n = level(l);
            if n == 0 {
                continue;
            }
            let end = lines[i + 1..]
                .iter()
                .position(|m| (1..=n).contains(&level(m)))
                .map_or(lines.len(), |p| i + 1 + p);
            let name = l[n..].trim().to_owned();
            let address = format!("@h:{}", name.to_lowercase());
            out.push(TextUnit::new(address, "heading", name, i + 1, end));
        }
        out
    }
}

// ---------- the fixture ----------

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    _dir: TmpDir,
    _mgr: SessionManager,
    s: Arc<Session>,
    root: &'static str,
}

fn open(tag: &str, pack: Arc<dyn Pack>, path: &str, bytes: &[u8], root: &'static str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-view-address-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_700_000_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c);
    let key = DocKey {
        space: "ws".into(),
        path: path.into(),
    };
    let s = mgr.open(pack, &key, Some(bytes)).unwrap();
    Fx {
        _dir: TmpDir(dir),
        _mgr: mgr,
        s,
        root,
    }
}

fn deck(tag: &str, three_d: bool) -> Fx {
    open(tag, Arc::new(Deck { three_d }), "d.toy", b"toy", SHAPES)
}

const MD: &str = "# A\nintro\n## B\nbody\n## C\nend\n";

fn md(tag: &str) -> Fx {
    let pack = Arc::new(Md(TextFilePack::new("markdown", 1 << 20)));
    open(tag, pack, "n.md", MD.as_bytes(), TextFilePack::ROOT)
}

impl Fx {
    fn join(&self, pid: &str, kind: ParticipantKind) -> ConnId {
        self.s
            .join(
                Participant::new(pid, pid, "#2f9e8f", kind, "tab"),
                Stay::Held,
            )
            .unwrap()
            .conn
    }
    fn version(&self) -> u64 {
        self.s.status().unwrap().version
    }
    /// `words` with `--if-version` (the doc's version now), its result as data.
    fn run(&self, c: ConnId, words: &str, stdin: &[u8]) -> Json {
        let mut a: Vec<String> = words.split(' ').map(str::to_owned).collect();
        a.extend(["--if-version".to_owned(), self.version().to_string()]);
        let o = self
            .s
            .run_tool_with(
                c,
                self.root,
                &a,
                stdin,
                &ToolOptions::new().result_json(true),
            )
            .unwrap_or_else(|e| panic!("{words}: {e}"));
        assert_eq!(o.exit, 0, "{words}: {}", o.stderr_summary);
        o.result().expect("the call's result").clone()
    }
    fn entry(&self, viewer: ConnId, pid: &str) -> RosterEntry {
        self.s
            .roster(viewer)
            .unwrap()
            .into_iter()
            .find(|e| e.participant.pid == pid)
            .unwrap_or_else(|| panic!("no roster row for {pid}"))
    }
}

/// The value at `path` (object keys) in `v`, as compact JSON; `<absent>` when a key is missing.
fn at(v: &Json, path: &str) -> String {
    let mut cur = v;
    for k in path.split('.') {
        match cur.get(k) {
            Some(x) => cur = x,
            None => return "<absent>".to_owned(),
        }
    }
    cur.to_json()
}

/// The result's view, which an edit that changed something always has.
fn view(r: &Json) -> String {
    let v = at(r, "view");
    assert!(v != "null" && v != "<absent>", "the change's view: {v}");
    v
}

// ---------- the rows ----------

/// An edit's view: its container (the primary address), its units, no cells, lines or time.
#[test]
fn edit_result_has_view() {
    let f = deck("has", false);
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "shape set s1 color red", b"");
    assert_eq!(
        view(&r),
        r#"{"address":"@sl:p1","containers":["@sl:p1"],"units":[{"address":"@sh:s1","live":true}],"units_more":0,"cells":null,"lines":[],"time":null}"#,
        "the change's view"
    );
}

/// The view is where the agent's presence went: the last container touched is the address and
/// the focus, the units are the selection, and the agent's view address moved there.
#[test]
fn view_is_the_presence() {
    let f = deck("presence", false);
    let a = f.join("bot", ParticipantKind::Agent);
    let p = f.join("ada", ParticipantKind::Person);
    let r = f.run(a, "pair set s1 s3 color red", b"");
    view(&r);
    assert_eq!(at(&r, "view.containers"), r#"["@sl:p1","@sl:p2"]"#);
    let e = f.entry(p, "bot");
    let focus = e.focus.as_ref().map(|x| x.address.clone());
    assert_eq!(
        at(&r, "view.address"),
        Json::from(focus.as_deref()).to_json(),
        "the view is the presence"
    );
    assert_eq!(
        at(&r, "view.address"),
        "\"@sl:p2\"",
        "the view is the presence"
    );
    let sel: Vec<String> = e.selection.map(|s| s.addresses).unwrap_or_default();
    assert_eq!(
        at(&r, "view.units"),
        Json::List(
            sel.iter()
                .map(|a| Json::obj([
                    ("address", Json::from(a.as_str())),
                    ("live", Json::from(true))
                ]))
                .collect()
        )
        .to_json(),
        "the view is the presence"
    );
    assert_eq!(
        e.view_address.as_deref(),
        Some("@sl:p2"),
        "the agent's view address follows its verb"
    );
}

/// A unit the change deleted is listed, not live; a deleted slide's view is the next slide, or
/// the previous one when it was the last.
#[test]
fn view_on_delete() {
    let f = deck("delete", false);
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "shape delete s1", b"");
    view(&r);
    assert_eq!(
        at(&r, "view.units"),
        r#"[{"address":"@sh:s1","live":false}]"#,
        "a deleted unit is not live"
    );
    assert_eq!(at(&r, "view.address"), "\"@sl:p1\"");
    let r = f.run(a, "slide delete p2", b"");
    view(&r);
    assert_eq!(
        at(&r, "view.address"),
        "\"@sl:p3\"",
        "after a slide delete, the next slide, else the previous"
    );
    let r = f.run(a, "slide delete p3", b"");
    assert_eq!(
        at(&r, "view.address"),
        "\"@sl:p1\"",
        "after a slide delete, the next slide, else the previous"
    );
}

/// With a 3D slot the primary address is the unit: a still frames the object.
#[test]
fn view_3d_is_the_unit() {
    let f = deck("3d", true);
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "shape set s3 xform 1", b"");
    view(&r);
    assert_eq!(
        at(&r, "view.address"),
        "\"@sh:s3\"",
        "in 3D, the view is the object"
    );
    assert_eq!(at(&r, "view.containers"), r#"["@sl:p2"]"#);
}

/// A person's verb gives its view in the result, and moves nothing of theirs.
#[test]
fn view_never_moves_a_person() {
    let f = deck("person", false);
    let p = f.join("ada", ParticipantKind::Person);
    let r = f.run(p, "shape set s1 color red", b"");
    assert_eq!(at(&r, "view.address"), "\"@sl:p1\"", "the change's view");
    let e = f.entry(p, "ada");
    assert_eq!(e.view_address, None, "a person's view never moves");
}

/// The verb's playhead is the view's time.
#[test]
fn view_time() {
    let f = deck("time", false);
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "shape play s1 1000", b"");
    view(&r);
    assert_eq!(
        at(&r, "view.time"),
        r#"{"at":"1000","state":"paused","in":"@sl:p1","rate":null}"#,
        "the view's time is the verb's playhead"
    );
}

/// A verb's own view address is the view's, when the grammar reads it; one it doesn't is left
/// out (the kit's stays).
#[test]
fn view_address_is_grammar() {
    let f = deck("grammar", false);
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "shape at s1 @cam:7", b"");
    assert_eq!(at(&r, "view.address"), "\"@cam:7\"", "the verb's own view");
    let long = format!("@sl:{}", "x".repeat(600));
    let r = f.run(a, &format!("shape at s2 {long}"), b"");
    assert_eq!(
        at(&r, "view.address"),
        "\"@sl:p1\"",
        "a view address the grammar refuses is left out"
    );
    let e = f.entry(a, "bot");
    assert_eq!(
        e.view_address.as_deref(),
        Some("@sl:p1"),
        "a view address the grammar refuses is left out"
    );
}

/// A plain text change: its lines, by `L<first>`.
#[test]
fn view_text_lines() {
    let pack = Arc::new(TextFilePack::new("text", 1 << 20));
    let f = open("lines", pack, "t.txt", b"one\n", TextFilePack::ROOT);
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "append", b"two\n");
    assert_eq!(
        view(&r),
        r#"{"address":"L2","containers":[],"units":[],"units_more":0,"cells":null,"lines":[{"root":"source","first":2,"last":2,"address":"L2"}],"time":null}"#,
        "a text change's view: its lines"
    );
}

/// A text pack's sections: the innermost holding the change is the address.
#[test]
fn view_markdown_section() {
    let f = md("section");
    let a = f.join("bot", ParticipantKind::Agent);
    let r = f.run(a, "insert --line 4", b"new\n");
    view(&r);
    assert_eq!(at(&r, "view.address"), "\"@h:b\"", "the innermost section");
    assert_eq!(
        at(&r, "view.containers"),
        r#"["@h:b"]"#,
        "the innermost section"
    );
    let e = f.entry(a, "bot");
    assert_eq!(e.view_address.as_deref(), Some("@h:b"));
}

/// A host's own write asks for the view and gets the same as a verb's.
#[test]
fn view_host_run() {
    let f = md("host");
    let a = f.join("bot", ParticipantKind::Agent);
    let v = f.version();
    f.s.read(a).unwrap();
    let new = MD.replace("end\n", "end\nmore\n");
    let rep =
        f.s.write_base_with_options(
            a,
            TextFilePack::ROOT,
            v,
            &new,
            &WriteOptions::new().view(true),
        )
        .unwrap();
    let res = match rep.outcome {
        WriteOutcome::Applied(res) => res,
        other => panic!("the write applies: {other:?}"),
    };
    let view = res.view.expect("the change's view");
    assert_eq!(view.address.as_deref(), Some("@h:c"), "the change's view");
    assert_eq!(view.lines.len(), 1);
    // without the option, none
    let v = f.version();
    f.s.read(a).unwrap();
    let rep =
        f.s.write_base_with_options(
            a,
            TextFilePack::ROOT,
            v,
            &format!("{new}x\n"),
            &WriteOptions::new(),
        )
        .unwrap();
    let res = match rep.outcome {
        WriteOutcome::Applied(res) => res,
        other => panic!("the write applies: {other:?}"),
    };
    assert_eq!(res.view, None, "no view unless asked");
}

/// **The view's known limits, each as it is today.** Each check fails when its limit stops
/// holding: a change that starts giving a view (or a fuller one) unannounced. Announce it in the
/// CHANGELOG, then move the check to a row of its own.
#[test]
fn known_limits_of_the_view() {
    const ANNOUNCE: &str =
        "known limit changed: announce it in the CHANGELOG and move the check to a row";
    // 1. a revert gives no view (HistoryOptions has no view option)
    let f = deck("limit-revert", false);
    let a = f.join("bot", ParticipantKind::Agent);
    f.run(a, "shape set s1 color red", b"");
    let change = f.version();
    match f.s.revert(a, change, false).unwrap() {
        UndoOutcome::Applied { result, .. } => assert_eq!(result.view, None, "revert: {ANNOUNCE}"),
        other => panic!("the revert applies: {other:?}"),
    }
    // 1b. a restore gives none either (the same options)
    let f = md("limit-restore");
    let a = f.join("bot", ParticipantKind::Agent);
    f.run(a, "append", b"more\n");
    let first = f.s.versions(a).unwrap()[0].id;
    let res = f.s.restore(a, first).unwrap();
    assert_eq!(res.view, None, "restore: {ANNOUNCE}");
    // 2. a grid host write (`write --base` on a table) gives no view
    let pack = Arc::new(grid_pack::GridLite);
    let f = open(
        "limit-grid",
        pack,
        "t.csv",
        b"id,city\nk1,Oslo\n",
        k2_mpc_base::table::ROWS,
    );
    let a = f.join("bot", ParticipantKind::Agent);
    let base = f.version();
    f.s.read(a).unwrap();
    let rep =
        f.s.write_base_grid(a, base, b"id,city\nk1,Lima\n", &GridWriteOptions::default())
            .unwrap();
    match rep.outcome {
        GridWrite::Applied(res) => assert_eq!(res.view, None, "grid host write: {ANNOUNCE}"),
        other => panic!("the grid write applies: {other:?}"),
    }
    // 3. a grid-slot change's cells come only from a verb's report (`StructOutput::writing`):
    //    the same cells written with no report give none
    let v = sheet_pack::vector();
    let step = v
        .steps
        .iter()
        .find(|s| s.verb.as_deref() == Some("cell set"))
        .expect("the vector has a cell set");
    let f = open(
        "limit-cells",
        Arc::new(sheet_pack::Sheet::default()),
        "b.sheet",
        v.file.as_bytes(),
        sheet_pack::ROWS,
    );
    let a = f.join("bot", ParticipantKind::Agent);
    let opts = EditOptions::new().if_version(f.version()).view(true);
    let res = f.s.edit_ops(a, step.edits.clone(), &opts).unwrap();
    let v = res.view.expect("an edit asked for its view gives one");
    assert!(
        !v.units.is_empty(),
        "control: the step wrote a row's cells: {v:?}"
    );
    assert_eq!(v.cells, None, "cells without a report: {ANNOUNCE}");
}

/// An undo of several changes: its view holds every container they touched.
#[test]
fn view_multi_step_undo() {
    let f = deck("undo", false);
    let a = f.join("bot", ParticipantKind::Agent);
    f.run(a, "shape set s1 color red", b"");
    f.run(a, "shape set s3 color blue", b"");
    let mut req = UndoRequest::default();
    req.steps = 2;
    req.view = true;
    match f.s.undo_with(a, &req).unwrap() {
        UndoOutcome::Applied { steps, result, .. } => {
            assert_eq!(steps, 2, "both steps undone");
            let v = result.view.expect("an undo asked for its view gives one");
            let mut c = v.containers.clone();
            c.sort();
            assert_eq!(
                c,
                ["@sl:p1", "@sl:p2"],
                "every container the undo touched: {v:?}"
            );
        }
        other => panic!("the undo applies: {other:?}"),
    }
}
